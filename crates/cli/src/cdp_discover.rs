//! Discovery: the DevTools port an embedded web engine opened somewhere in the session's family.
//!
//! The engine was told to open one (the variables in `embedded::engine_env`), but not where, and
//! not when - a WebView2 host creates its browser process when it first shows a page, which can be
//! a minute after the host started, and a Qt host opens the port inside itself. So this is a thread
//! that keeps looking, in the spirit of ADR-10: every second it reads the machine's listening
//! sockets (mech), keeps the loopback ones owned by a pid in the family where an engine can be - the
//! port reserved for Qt, a process whose executable sits beside the Chromium runtime (R4-N29), or one
//! with Qt WebEngine loaded - and asks each new one whether it is a DevTools endpoint (`/json/version` names a browser
//! WebSocket URL). The application's other servers are not sent the HTTP request - except those in a
//! process that runs the engine itself (Qt WebEngine, or an application built on CEF), whose every
//! loopback port is asked, because nothing tells its DevTools port from the rest before asking
//! (CodeRabbit on #89, a narrower rule is open). A yes goes to
//! the caller as [`Discovered`]. A no is remembered, with a few retries spaced out - an endpoint
//! bound a moment ago may not answer HTTP yet - and forgotten once the socket leaves the table, so a
//! port reused later starts fresh.
//!
//! Its own thread, because the HTTP probe has a ten-second read timeout and the application's own
//! HTTP server (a listener in the family that is not an engine) would otherwise stall the session
//! loop for that long. The family is the caller's to define - a hooked session has its registry, a
//! probe without the hook walks the process tree - and it is pushed here whenever it changes.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::cdp;
use crate::embedded::is_devtools_version;

/// A DevTools endpoint found in the family: the pid that holds it, the loopback host and port to
/// connect to (`::1` for an IPv6 socket, `127.0.0.1` otherwise), and what the engine calls itself in
/// `/json/version` (cleaned, for the report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Discovered {
    pub(crate) pid: u32,
    pub(crate) host: &'static str,
    pub(crate) port: u16,
    pub(crate) browser: String,
}

/// What the discovery thread has to say besides a find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    Found(Discovered),
    /// The table could not be read at all - said once, then the thread ends. The session goes on
    /// without the channel, and the caller reports the loss (untouchable rule 6).
    Unavailable(String),
    /// The port reserved for a Qt engine is held by something that is not a DevTools endpoint: a
    /// process outside the family bound it first, or one inside the family that never answered as
    /// DevTools (the application's own server landing on the same ephemeral port). Said once. The
    /// engine could not have bound it, so its pages stay unreached and the caller says why.
    PortTaken(u16),
}

/// How often the table is read.
const SWEEP: Duration = Duration::from_secs(1);
/// How many times a listener that did not answer as DevTools is asked again, and how far apart.
/// WebView2 binds its port about three quarters of a second before it serves HTTP on it.
const RETRIES: u32 = 3;
const RETRY_GAP: Duration = Duration::from_secs(2);

/// The caller's end: push the family's pids as they change, pull notices as they come. Dropping it
/// ends the thread.
pub(crate) struct Discovery {
    pids: Sender<Vec<u32>>,
    notices: Receiver<Notice>,
}

impl Discovery {
    /// Start the thread with an initial family and, when a port was reserved for a Qt engine, that
    /// port - so the thread can say when something else took it. A thread that cannot be started
    /// (handles or memory exhausted) is the caller's to report - the session goes on without the
    /// channel, which is what `Notice::Unavailable` promises for the table, and a panic here would
    /// end it instead.
    pub(crate) fn start(family: Vec<u32>, reserved: Option<u16>) -> std::io::Result<Discovery> {
        let (pid_tx, pid_rx) = mpsc::channel::<Vec<u32>>();
        let (notice_tx, notice_rx) = mpsc::channel::<Notice>();
        let _ = pid_tx.send(family);
        thread::Builder::new()
            .name("chrono-discover".into())
            .spawn(move || run(&pid_rx, &notice_tx, reserved))?;
        Ok(Discovery { pids: pid_tx, notices: notice_rx })
    }

    /// The family as of now. The thread uses the latest set it has received.
    pub(crate) fn update_family(&self, family: Vec<u32>) {
        let _ = self.pids.send(family);
    }

    /// The next notice, if one is waiting. Never blocks.
    pub(crate) fn try_recv(&self) -> Option<Notice> {
        self.notices.try_recv().ok()
    }
}

/// The thread body: sweep, probe what is new, report, sleep, until the caller is gone.
fn run(pids: &Receiver<Vec<u32>>, notices: &Sender<Notice>, reserved: Option<u16>) {
    let mut family: HashSet<u32> = HashSet::new();
    let mut memory = Memory::default();
    let mut taken_said = false;
    loop {
        // The latest family wins, and a closed channel means the caller dropped its handle.
        loop {
            match pids.try_recv() {
                Ok(set) => family = set.into_iter().collect(),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let table = match chrono_mech::listening_sockets() {
            Ok(t) => t,
            Err(e) => {
                let _ = notices.send(Notice::Unavailable(e));
                return;
            }
        };
        let now = Instant::now();
        if let Some(port) = reserved
            && !taken_said
            && reserved_port_taken(&table, &family, port, &memory)
        {
            taken_said = true;
            if notices.send(Notice::PortTaken(port)).is_err() {
                return;
            }
        }
        // Asked only where an engine can be (R4-N29): the request below is HTTP, and the application's
        // own servers in the family may speak something else. Checked on every sweep rather than kept
        // per pid - one query, one file lookup and, away from the runtime, one module list per listener
        // a second - and a pid the system hands to another process of the family is judged afresh.
        let candidates = memory
            .due(candidates(&table, &family), now)
            .into_iter()
            .filter(|l| worth_asking(l, reserved, may_be_engine));
        for candidate in candidates {
            let host = candidate.loopback_host();
            match probe(host, candidate.port) {
                Some(browser) => {
                    memory.found(candidate);
                    let found = Discovered { pid: candidate.pid, host, port: candidate.port, browser };
                    if notices.send(Notice::Found(found)).is_err() {
                        return;
                    }
                }
                None => memory.refused(candidate, now),
            }
        }
        thread::sleep(SWEEP);
    }
}

/// The loopback listeners owned by the family, whichever address family they are on. Pure over the
/// table, so the join is tested on a made-up machine.
fn candidates(table: &[chrono_mech::Listener], family: &HashSet<u32>) -> Vec<chrono_mech::Listener> {
    table.iter().filter(|l| l.loopback && family.contains(&l.pid)).copied().collect()
}

/// Whether a listener of the family is worth an HTTP request: it is on the port reserved for a Qt
/// engine, whose browser runs inside the application's own process, or `may_be_engine` says its process
/// can be a Chromium engine. Every family listener used to be asked, up to four times - a request a
/// server of the application that speaks another protocol than HTTP may take badly (R4-N29). Pure, so
/// the rule is tested on a made-up machine.
fn worth_asking(l: &chrono_mech::Listener, reserved: Option<u16>, may_be_engine: impl Fn(u32) -> bool) -> bool {
    reserved == Some(l.port) || may_be_engine(l.pid)
}

/// The library of Qt WebEngine's Chromium, in Qt 6 and Qt 5, release and debug builds. Qt runs the
/// engine's browser inside the application's own process and keeps its ICU data in a folder of its own
/// (`resources`), so a Qt host has no `icudtl.dat` beside its executable, and a Qt engine on any port
/// but the reserved one - set by the application itself, or found by a probe that reserved nothing -
/// went unasked (R4/16 review round, measured on a Qt WebEngine host).
const QT_WEBENGINE_LIBRARIES: [&str; 4] =
    ["Qt6WebEngineCore.dll", "Qt5WebEngineCore.dll", "Qt6WebEngineCored.dll", "Qt5WebEngineCored.dll"];

/// Whether the process `pid` can be a Chromium engine (see [`engine_signs`]).
fn may_be_engine(pid: u32) -> bool {
    engine_signs(chrono_mech::process_image_path(pid), || chrono_mech::process_has_any_module(pid, &QT_WEBENGINE_LIBRARIES))
}

/// Whether a process started from `image` can be a Chromium engine: its executable sits beside the ICU
/// data file every Chromium build ships (`icudtl.dat` - the WebView2 runtime, CEF and Chromium itself,
/// measured on this machine's WebView2 runtime folder), or it has Qt WebEngine loaded (`qt`, asked only
/// when the file is not there). A question the system would not answer - no image path, a module list
/// it would not give - is taken as a yes: a page left unreached would be coverage given up without a
/// word (rule 27), a request too many is what every listener got before. Pure over the two answers.
fn engine_signs(image: Option<std::path::PathBuf>, qt: impl FnOnce() -> chrono_mech::ModuleProbe) -> bool {
    let Some(path) = image else {
        return true;
    };
    path.parent().is_some_and(|dir| dir.join("icudtl.dat").exists()) || qt() != chrono_mech::ModuleProbe::NotLoaded
}

/// Whether the port reserved for a Qt engine is held by something that is not its DevTools endpoint:
/// a listener on it outside the family, or one inside the family whose retries as DevTools are spent.
/// Only a socket that stands in the way of the engine's own bind (`127.0.0.1:<port>`) counts - an IPv6
/// socket or one on another IPv4 address on the same port leaves that bind free, and would make this
/// a false alarm. Pure over the table and the memory, so every shape is tested without a socket.
fn reserved_port_taken(
    table: &[chrono_mech::Listener],
    family: &HashSet<u32>,
    port: u16,
    memory: &Memory,
) -> bool {
    table
        .iter()
        .filter(|l| l.port == port && l.blocks_ipv4_loopback_bind())
        .any(|l| !family.contains(&l.pid) || memory.spent(l))
}

/// Ask a port on a loopback host whether it is a DevTools endpoint. The browser name is the
/// engine's own text, cleaned on the way to the report.
fn probe(host: &str, port: u16) -> Option<String> {
    let reply = cdp::http_get_json(host, port, "/json/version", std::time::Instant::now() + cdp::CONNECT_DEADLINE).ok()?;
    is_devtools_version(&reply).then(|| {
        cdp::sanitise_target_text(reply.get("Browser").and_then(serde_json::Value::as_str).unwrap_or(""))
    })
}

/// The identity of one listener in memory: pid, port and address family - the same port on both
/// families is two sockets, and each answers for itself.
type Key = (u32, u16, bool);

fn key(l: &chrono_mech::Listener) -> Key {
    (l.pid, l.port, l.v6)
}

/// What each listener has already said, so a found endpoint is reported once, a refusing one is
/// asked again a bounded number of times, and a listener that left the table is forgotten.
#[derive(Default)]
struct Memory {
    found: HashSet<Key>,
    refused: HashMap<Key, (u32, Instant)>,
}

impl Memory {
    /// The listeners worth asking now: never the found ones, refused ones only when their gap has
    /// passed and their retries are not spent. Listeners no longer in the table drop out of memory.
    fn due(&mut self, present: Vec<chrono_mech::Listener>, now: Instant) -> Vec<chrono_mech::Listener> {
        let live: HashSet<Key> = present.iter().map(key).collect();
        self.found.retain(|k| live.contains(k));
        self.refused.retain(|k, _| live.contains(k));
        present
            .into_iter()
            .filter(|l| !self.found.contains(&key(l)))
            .filter(|l| match self.refused.get(&key(l)) {
                None => true,
                Some((attempts, last)) => *attempts <= RETRIES && now.duration_since(*last) >= RETRY_GAP,
            })
            .collect()
    }

    fn found(&mut self, l: chrono_mech::Listener) {
        self.refused.remove(&key(&l));
        self.found.insert(key(&l));
    }

    fn refused(&mut self, l: chrono_mech::Listener, now: Instant) {
        let entry = self.refused.entry(key(&l)).or_insert((0, now));
        entry.0 += 1;
        entry.1 = now;
    }

    /// Whether a listener has been asked every time it will be and never answered as DevTools.
    fn spent(&self, l: &chrono_mech::Listener) -> bool {
        self.refused.get(&key(l)).is_some_and(|(attempts, _)| *attempts > RETRIES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_mech::Listener;

    fn listener(pid: u32, port: u16, loopback: bool) -> Listener {
        let addr_v4 = if loopback { chrono_mech::IPV4_LOOPBACK_ADDR } else { 0x0501_A8C0 };
        Listener { pid, port, loopback, v6: false, addr_v4 }
    }

    fn listener6(pid: u32, port: u16) -> Listener {
        Listener { pid, port, loopback: true, v6: true, addr_v4: 0 }
    }

    #[test]
    fn only_loopback_listeners_of_the_family_are_candidates_on_either_family() {
        let table = [
            listener(10, 9222, true),
            listener(10, 8080, false),
            listener(99, 9333, true),
            listener(11, 9444, true),
            listener6(11, 9555),
        ];
        let family: HashSet<u32> = [10, 11].into_iter().collect();
        let found = candidates(&table, &family);
        assert_eq!(found, vec![listener(10, 9222, true), listener(11, 9444, true), listener6(11, 9555)]);
        assert_eq!(found[2].loopback_host(), "::1");
        assert!(candidates(&table, &HashSet::new()).is_empty());
    }

    /// The port reserved for a Qt engine is taken when something outside the family listens on it,
    /// or when a family process on it never answered as DevTools in all its retries - either way the
    /// engine could not have bound it (docs/09 section 12.17 point 4). A family listener still being
    /// asked is not taken yet.
    #[test]
    fn the_reserved_port_is_taken_by_a_stranger_or_by_a_family_socket_that_is_not_devtools() {
        let family: HashSet<u32> = [10].into_iter().collect();
        let mut memory = Memory::default();
        let t0 = Instant::now();

        let stranger = [listener(99, 40000, true)];
        assert!(reserved_port_taken(&stranger, &family, 40000, &memory));
        assert!(!reserved_port_taken(&stranger, &family, 40001, &memory), "another port is not ours");
        // Sockets on the same port that do NOT stand in the way of a bind to 127.0.0.1: an IPv6 one,
        // and an IPv4 one on a LAN address. The wildcard does.
        assert!(!reserved_port_taken(&[listener6(99, 40000)], &family, 40000, &memory), "IPv6 leaves the IPv4 bind free");
        assert!(!reserved_port_taken(&[listener(99, 40000, false)], &family, 40000, &memory), "a LAN address leaves it free");
        let wildcard = Listener { pid: 99, port: 40000, loopback: false, v6: false, addr_v4: chrono_mech::IPV4_ANY_ADDR };
        assert!(reserved_port_taken(&[wildcard], &family, 40000, &memory), "the wildcard claims every address");

        let own = listener(10, 40000, true);
        assert!(!reserved_port_taken(&[own], &family, 40000, &memory), "still being asked");
        for n in 0..=RETRIES {
            memory.refused(own, t0 + RETRY_GAP * n);
        }
        assert!(reserved_port_taken(&[own], &family, 40000, &memory), "retries spent, never DevTools");
    }

    #[test]
    fn a_found_endpoint_is_asked_once_and_a_refusing_one_a_bounded_number_of_times() {
        let mut memory = Memory::default();
        let t0 = Instant::now();
        let one = listener(10, 9222, true);

        assert_eq!(memory.due(vec![one], t0), vec![one]);
        memory.found(one);
        assert!(memory.due(vec![one], t0).is_empty(), "a found endpoint is not asked again");

        let refusing = listener(10, 8080, true);
        memory.refused(refusing, t0);
        assert!(memory.due(vec![refusing], t0).is_empty(), "not before the gap");
        let later = t0 + RETRY_GAP;
        assert_eq!(memory.due(vec![refusing], later), vec![refusing], "asked again after the gap");
        for n in 1..=RETRIES {
            memory.refused(refusing, later + RETRY_GAP * n);
        }
        let much_later = later + RETRY_GAP * (RETRIES + 2);
        assert!(memory.due(vec![refusing], much_later).is_empty(), "retries are spent");
    }

    #[test]
    fn the_same_port_on_the_other_family_is_another_listener() {
        let mut memory = Memory::default();
        let t0 = Instant::now();
        let v4 = listener(10, 9222, true);
        let v6 = listener6(10, 9222);
        memory.found(v4);
        assert_eq!(memory.due(vec![v4, v6], t0), vec![v6]);
    }

    #[test]
    fn a_listener_that_left_the_table_is_forgotten_and_asked_afresh_when_it_returns() {
        let mut memory = Memory::default();
        let t0 = Instant::now();
        let one = listener(10, 9222, true);
        memory.found(one);
        assert!(memory.due(vec![], t0).is_empty());
        // Gone from the table for one sweep, then back: a new engine on a reused port.
        assert_eq!(memory.due(vec![one], t0), vec![one]);
    }

    /// Only a listener where an engine can be is asked (R4-N29): the Qt port whoever holds it, and the
    /// listeners of a process beside the Chromium runtime. The application's own server elsewhere in
    /// the family is left alone.
    #[test]
    fn only_a_listener_where_an_engine_can_be_is_asked() {
        let engine = |pid: u32| pid == 20;
        assert!(worth_asking(&listener(20, 9222, true), None, engine), "a process beside the runtime");
        assert!(!worth_asking(&listener(10, 9222, true), None, engine), "the application's own server");
        assert!(worth_asking(&listener(10, 40000, true), Some(40000), engine), "the Qt port, held by the host itself");
        assert!(!worth_asking(&listener(10, 40001, true), Some(40000), engine), "the host's other ports");
    }

    /// The thread end to end, on a listener of this test's own process, which sits beside no Chromium
    /// runtime: it is the application's own server, and gets no HTTP request (R4-N29). The control is
    /// the same port announced as the one reserved for Qt, which is asked - so the silence is the rule,
    /// not a listener the thread could not see.
    #[test]
    fn the_applications_own_server_is_not_asked_and_the_qt_port_is() {
        let asked = |reserved: bool| {
            let server = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
            server.set_nonblocking(true).expect("non-blocking accept");
            let port = server.local_addr().expect("its address").port();
            let discovery = Discovery::start(vec![std::process::id()], reserved.then_some(port)).expect("the thread starts");
            let until = Instant::now() + Duration::from_millis(1500);
            let mut asked = false;
            while !asked && Instant::now() < until {
                asked = server.accept().is_ok();
                thread::sleep(Duration::from_millis(20));
            }
            drop(discovery);
            asked
        };
        assert!(!asked(false), "the application's own server is left alone");
        assert!(asked(true), "the Qt port is asked, whoever holds it");
    }

    /// The executable of this test sits beside no ICU data, so it is no engine. A pid nobody holds
    /// cannot be asked where its executable is, and is then asked about its port rather than skipped.
    #[test]
    fn an_engine_is_told_by_the_file_beside_its_executable() {
        assert!(!may_be_engine(std::process::id()));
        assert!(may_be_engine(u32::MAX - 3), "a process that cannot be asked is not skipped");
    }

    /// A Qt WebEngine host has no ICU data beside its executable, and is an engine by the library it
    /// loaded (R4/16 review round). The module list is asked only away from the runtime, and a list the
    /// system would not give is a yes, like a path it would not give.
    #[test]
    fn a_qt_engine_is_told_by_the_library_it_loaded() {
        use chrono_mech::ModuleProbe;
        let dir = std::env::temp_dir().join(format!("chrono-engine-signs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let host = dir.join("host.exe");
        let not_asked = || -> ModuleProbe { panic!("the module list is not asked beside the runtime") };

        assert!(!engine_signs(Some(host.clone()), || ModuleProbe::NotLoaded), "no runtime, no Qt: not an engine");
        assert!(engine_signs(Some(host.clone()), || ModuleProbe::Loaded), "Qt WebEngine loaded");
        assert!(engine_signs(Some(host.clone()), || ModuleProbe::Unknown), "a list the system would not give");
        assert!(engine_signs(None, not_asked), "a path the system would not give");
        std::fs::write(dir.join("icudtl.dat"), b"").expect("a marker file");
        assert!(engine_signs(Some(host), not_asked), "beside the runtime");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(QT_WEBENGINE_LIBRARIES.contains(&"Qt6WebEngineCore.dll") && QT_WEBENGINE_LIBRARIES.contains(&"Qt5WebEngineCore.dll"));
    }

    #[test]
    fn a_port_that_never_answers_is_no_endpoint_on_either_family() {
        // A port nobody listens on: the connection is refused at once, so this is a fast no.
        let free = cdp::free_loopback_port().expect("a free port");
        assert_eq!(probe("127.0.0.1", free), None);
        assert_eq!(probe("::1", free), None);
    }
}
