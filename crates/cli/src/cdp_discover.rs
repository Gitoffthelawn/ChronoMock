//! Discovery: the DevTools port an embedded web engine opened somewhere in the session's family.
//!
//! The engine was told to open one (the variables in `embedded::engine_env`), but not where, and
//! not when - a WebView2 host creates its browser process when it first shows a page, which can be
//! a minute after the host started, and a Qt host opens the port inside itself. So this is a thread
//! that keeps looking, in the spirit of ADR-10: every second it reads the machine's listening
//! sockets (mech), keeps the loopback ones owned by a pid in the family where an engine can be, and
//! asks each new one whether it is a DevTools endpoint (`/json/version` names a browser WebSocket URL).
//! Where an engine can be (R4-N29): the port the Qt engine was told to open, every port of a process
//! whose executable sits beside the Chromium runtime (the WebView2 runtime, CEF, Chromium - nothing
//! tells their DevTools port from the rest before asking), and in a process with Qt WebEngine loaded
//! only that told port - its other ports only once its engine runs without it, because the application
//! chose a port of its own. A process whose endpoint was found is not asked about its other ports
//! while that endpoint is open: an engine opens one. The application's own servers are not sent the
//! HTTP request otherwise. A yes goes to
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
use crate::embedded::{is_devtools_version, role_from_command_line};

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
    /// The port the Qt engine was told to open is held by something that is not a DevTools endpoint:
    /// a process outside the family bound it first, or one inside the family that never answered as
    /// DevTools (the application's own server landing on the same ephemeral port), while a process of
    /// the family has Qt WebEngine loaded. Said once. The engine could not have bound it, so its pages
    /// stay unreached and the caller says why.
    PortTaken(u16),
}

/// How often the table is read.
const SWEEP: Duration = Duration::from_secs(1);
/// How many times a listener that did not answer as DevTools is asked again, and how far apart.
/// WebView2 binds its port about three quarters of a second before it serves HTTP on it.
const RETRIES: u32 = 3;
const RETRY_GAP: Duration = Duration::from_secs(2);

/// How long the engine of a Qt process runs without a listener on the port it was told before that
/// process's other ports are asked - it chose a port of its own. Measured on a Qt WebEngine host
/// (2026-10-05, six starts): the DevTools port listens 243 to 2128 ms BEFORE the engine's first
/// subprocess starts, so an engine that took its port has it by then. The wait is margin for other
/// versions of Qt, and costs only the application that chose its own port.
const QT_ENGINE_GRACE: Duration = Duration::from_secs(2);

/// How often a family whose Qt port is held by something else is looked into for Qt WebEngine, while
/// the port stays held - a module list per process, so not on every sweep of a long session.
const QT_LOOK_GAP: Duration = Duration::from_secs(5);

/// The caller's end: push the family's pids as they change, pull notices as they come. Dropping it
/// ends the thread.
pub(crate) struct Discovery {
    pids: Sender<Vec<u32>>,
    notices: Receiver<Notice>,
}

impl Discovery {
    /// Start the thread with an initial family and, when one is known, the port a Qt engine was told
    /// to open ([`crate::embedded::qt_port_told`]) - so the thread asks that port first and can say
    /// when something else took it. A thread that cannot be started (handles or memory exhausted) is
    /// the caller's to report - the session goes on without the channel, which is what
    /// `Notice::Unavailable` promises for the table, and a panic here would end it instead.
    pub(crate) fn start(family: Vec<u32>, qt_port: Option<u16>) -> std::io::Result<Discovery> {
        let (pid_tx, pid_rx) = mpsc::channel::<Vec<u32>>();
        let (notice_tx, notice_rx) = mpsc::channel::<Notice>();
        let _ = pid_tx.send(family);
        thread::Builder::new()
            .name("chrono-discover".into())
            .spawn(move || run(&pid_rx, &notice_tx, qt_port))?;
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
fn run(pids: &Receiver<Vec<u32>>, notices: &Sender<Notice>, qt_port: Option<u16>) {
    let mut family: HashSet<u32> = HashSet::new();
    let mut looking = Looking::default();
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
        looking.engine_since.retain(|pid, _| family.contains(pid));
        if let Some(port) = qt_port
            && looking.qt_port_taken_now(&table, &family, port, now, family_has_qt)
            && notices.send(Notice::PortTaken(port)).is_err()
        {
            return;
        }
        for candidate in looking.listeners_to_ask(&table, &family, qt_port, now) {
            // An endpoint found earlier in this sweep closes the rest of its process, as on a later one.
            if qt_port != Some(candidate.port) && looking.memory.endpoint_open(candidate.pid) {
                continue;
            }
            let host = candidate.loopback_host();
            match probe(host, candidate.port) {
                Some(browser) => {
                    looking.memory.found(candidate);
                    let found = Discovered { pid: candidate.pid, host, port: candidate.port, browser };
                    if notices.send(Notice::Found(found)).is_err() {
                        return;
                    }
                }
                None => looking.memory.refused(candidate, now),
            }
        }
        thread::sleep(SWEEP);
    }
}

/// What the thread keeps from one sweep to the next.
#[derive(Default)]
struct Looking {
    memory: Memory,
    /// When the engine was first seen running in each Qt process of the family ([`QT_ENGINE_GRACE`]).
    engine_since: HashMap<u32, Instant>,
    /// The Qt port was said to be taken - once a session.
    taken_said: bool,
    /// When the family was last looked into for Qt WebEngine while its Qt port was held.
    qt_looked: Option<Instant>,
}

impl Looking {
    /// Whether the Qt port is to be said taken now: held by something that is not its engine
    /// ([`qt_port_taken`]) while `has_qt` says a process of the family has Qt WebEngine loaded. Every
    /// session reserves the port, also for an application with no Qt at all, and a stranger that took it
    /// there kept no engine from anything - saying the pages ran on the real clock would be false. Asked
    /// at most every [`QT_LOOK_GAP`] while the port stays held, and said once.
    fn qt_port_taken_now(
        &mut self,
        table: &[chrono_mech::Listener],
        family: &HashSet<u32>,
        port: u16,
        now: Instant,
        has_qt: impl Fn(&HashSet<u32>) -> bool,
    ) -> bool {
        if self.taken_said || !qt_port_taken(table, family, port, &self.memory) {
            return false;
        }
        if self.qt_looked.is_some_and(|at| now.duration_since(at) < QT_LOOK_GAP) {
            return false;
        }
        self.qt_looked = Some(now);
        self.taken_said = has_qt(family);
        self.taken_said
    }

    /// The listeners worth an HTTP request on this sweep (R4-N29): due by the memory, and where an
    /// engine can be ([`worth_asking`]). Judged afresh on every sweep rather than kept per pid - one file
    /// lookup and, away from the runtime, one module list per process a second - so a pid the system
    /// hands to another process of the family is judged as what it is now.
    fn listeners_to_ask(
        &mut self,
        table: &[chrono_mech::Listener],
        family: &HashSet<u32>,
        qt_port: Option<u16>,
        now: Instant,
    ) -> Vec<chrono_mech::Listener> {
        let due = self.memory.due(candidates(table, family), now);
        let mut signs: HashMap<u32, EngineSign> = HashMap::new();
        let mut no_engine: HashSet<u32> = HashSet::new();
        let mut asked = Vec::new();
        for l in due {
            let sign = *signs.entry(l.pid).or_insert_with(|| engine_sign_of(l.pid));
            let on_qt_port = qt_port.is_some_and(|port| table.iter().any(|t| t.pid == l.pid && t.port == port));
            let endpoint_open = self.memory.endpoint_open(l.pid);
            // The tree is read only for the one question it answers: a Qt process with a told port that it
            // does not listen on, and nothing found in it yet.
            let wants_engine = sign == EngineSign::Qt && qt_port.is_some() && !on_qt_port && !endpoint_open;
            let engine_for = if wants_engine { self.engine_for(l.pid, now, &mut no_engine) } else { None };
            if worth_asking(&l, qt_port, Asking { sign, endpoint_open, on_qt_port, engine_for }) {
                asked.push(l);
            }
        }
        asked
    }

    /// How long the engine has run in the process `pid`, `None` while it does not. A process found
    /// without one is looked at once a sweep.
    fn engine_for(&mut self, pid: u32, now: Instant, no_engine: &mut HashSet<u32>) -> Option<Duration> {
        if let Some(since) = self.engine_since.get(&pid) {
            return Some(now.duration_since(*since));
        }
        if no_engine.contains(&pid) || !engine_runs_in(pid) {
            no_engine.insert(pid);
            return None;
        }
        self.engine_since.insert(pid, now);
        Some(Duration::ZERO)
    }
}

/// The loopback listeners owned by the family, whichever address family they are on. Pure over the
/// table, so the join is tested on a made-up machine.
fn candidates(table: &[chrono_mech::Listener], family: &HashSet<u32>) -> Vec<chrono_mech::Listener> {
    table.iter().filter(|l| l.loopback && family.contains(&l.pid)).copied().collect()
}

/// What a listener's process shows of a Chromium engine (R4-N29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineSign {
    /// Nothing: a process of the application's own, whose servers are left alone.
    None,
    /// Beside the Chromium runtime, or a process the system would not say the executable of: every
    /// loopback port of it is asked, because nothing tells its DevTools port from the rest before asking.
    Runtime,
    /// Qt WebEngine loaded, or a module list the system would not give, away from the runtime: the
    /// engine runs inside this process and opens the port its variable names.
    Qt,
}

/// What decides whether one listener is asked, besides its port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Asking {
    sign: EngineSign,
    /// A DevTools endpoint of this process was found and is still open - an engine opens one.
    endpoint_open: bool,
    /// This process listens on the port the Qt engine was told to open.
    on_qt_port: bool,
    /// How long the engine has run in this process (a child with a Chromium role), `None` while not.
    engine_for: Option<Duration>,
}

/// Whether a listener of the family is worth an HTTP request. Every family listener used to be asked,
/// up to four times - a request a server of the application that speaks another protocol than HTTP may
/// take badly (R4-N29). Now:
/// - the port the Qt engine was told to open, whoever holds it - Qt runs the engine's browser inside
///   the application's own process,
/// - not another port of a process whose endpoint is open,
/// - every port of a process beside the Chromium runtime,
/// - in a Qt process only the told port, and its other ports once its engine has run for
///   [`QT_ENGINE_GRACE`] without listening there - the application chose its own port, in its code or
///   on its command line (`--remote-debugging-port`, Qt documentation). With no told port every port.
///
/// Pure, so the rule is tested on a made-up machine.
fn worth_asking(l: &chrono_mech::Listener, qt_port: Option<u16>, asking: Asking) -> bool {
    if qt_port == Some(l.port) {
        return true;
    }
    if asking.endpoint_open {
        return false;
    }
    match asking.sign {
        EngineSign::None => false,
        EngineSign::Runtime => true,
        EngineSign::Qt => {
            qt_port.is_none() || (!asking.on_qt_port && asking.engine_for.is_some_and(|ran| ran >= QT_ENGINE_GRACE))
        }
    }
}

/// The library of Qt WebEngine's Chromium, in Qt 6 and Qt 5, release and debug builds. Qt runs the
/// engine's browser inside the application's own process and keeps its ICU data in a folder of its own
/// (`resources`), so a Qt host has no `icudtl.dat` beside its executable (R4/16 review round, measured on
/// a Qt WebEngine host).
const QT_WEBENGINE_LIBRARIES: [&str; 4] =
    ["Qt6WebEngineCore.dll", "Qt5WebEngineCore.dll", "Qt6WebEngineCored.dll", "Qt5WebEngineCored.dll"];

/// What the process `pid` shows of an engine (see [`engine_sign`]).
fn engine_sign_of(pid: u32) -> EngineSign {
    engine_sign(chrono_mech::process_image_path(pid), || chrono_mech::process_has_any_module(pid, &QT_WEBENGINE_LIBRARIES))
}

/// What a process started from `image` shows of a Chromium engine: its executable sits beside the ICU
/// data file every Chromium build ships (`icudtl.dat` - the WebView2 runtime, CEF and Chromium itself,
/// measured on this machine's WebView2 runtime folder), or it has Qt WebEngine loaded (`qt`, asked only
/// when the file is not there). A question the system would not answer is not taken as "no engine": a
/// page left unreached would be coverage given up without a word (rule 27). An image path it would not
/// give asks every port, as every listener was asked before, and a module list it would not give is a
/// Qt process, whose told port is asked and its other ports once an engine runs in it. Pure over the
/// two answers.
fn engine_sign(image: Option<std::path::PathBuf>, qt: impl FnOnce() -> chrono_mech::ModuleProbe) -> EngineSign {
    let Some(path) = image else {
        return EngineSign::Runtime;
    };
    if path.parent().is_some_and(|dir| dir.join("icudtl.dat").exists()) {
        return EngineSign::Runtime;
    }
    match qt() {
        chrono_mech::ModuleProbe::NotLoaded => EngineSign::None,
        chrono_mech::ModuleProbe::Loaded | chrono_mech::ModuleProbe::Unknown => EngineSign::Qt,
    }
}

/// Whether a process of the family has Qt WebEngine loaded - a list read in full that names it. A list
/// the system would not give is no evidence here: this decides whether to SAY the engine could not bind
/// its port, which needs an engine there.
fn family_has_qt(family: &HashSet<u32>) -> bool {
    family
        .iter()
        .any(|&pid| chrono_mech::process_has_any_module(pid, &QT_WEBENGINE_LIBRARIES) == chrono_mech::ModuleProbe::Loaded)
}

/// Whether the engine runs in the process `pid`: a child of it carries a Chromium role (`--type=`) on its
/// command line. By the role, not the name, because a Qt application may start its engine's subprocesses
/// from an executable of its own (`QTWEBENGINEPROCESS_PATH`, Qt documentation, "Deploying Qt WebEngine
/// Applications"). A tree the system would not give is no engine yet: the told port is still asked, and
/// the next sweep looks again.
fn engine_runs_in(pid: u32) -> bool {
    chrono_mech::descendants_of(pid)
        .is_ok_and(|under| runs_engine(pid, &under, |child| chrono_mech::name_unhooked(child, pid).command_line))
}

/// Whether one of `pid`'s own children in `under` (pid, declared parent) has a Chromium role on the
/// command line `command_line` gives for it. Pure, so the rule is tested on a made-up tree.
fn runs_engine(pid: u32, under: &[(u32, u32)], command_line: impl Fn(u32) -> Option<String>) -> bool {
    under
        .iter()
        .filter(|&&(_, parent)| parent == pid)
        .any(|&(child, _)| command_line(child).as_deref().and_then(role_from_command_line).is_some())
}

/// Whether the port the Qt engine was told to open is held by something that is not its DevTools
/// endpoint: a listener on it outside the family, or one inside the family whose retries as DevTools
/// are spent. Only a socket that stands in the way of the engine's own bind (`127.0.0.1:<port>`) counts -
/// an IPv6 socket or one on another IPv4 address on the same port leaves that bind free, and would make
/// this a false alarm. Pure over the table and the memory, so every shape is tested without a socket.
fn qt_port_taken(
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

    /// Whether a DevTools endpoint of the process `pid` was found and has not left the table since.
    fn endpoint_open(&self, pid: u32) -> bool {
        self.found.iter().any(|&(found_pid, ..)| found_pid == pid)
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

    /// The Qt port is taken when something outside the family listens on it, or when a family process
    /// on it never answered as DevTools in all its retries - either way the engine could not have bound
    /// it (docs/09 section 12.17 point 4). A family listener still being asked is not taken yet.
    #[test]
    fn the_qt_port_is_taken_by_a_stranger_or_by_a_family_socket_that_is_not_devtools() {
        let family: HashSet<u32> = [10].into_iter().collect();
        let mut memory = Memory::default();
        let t0 = Instant::now();

        let stranger = [listener(99, 40000, true)];
        assert!(qt_port_taken(&stranger, &family, 40000, &memory));
        assert!(!qt_port_taken(&stranger, &family, 40001, &memory), "another port is not ours");
        // Sockets on the same port that do NOT stand in the way of a bind to 127.0.0.1: an IPv6 one,
        // and an IPv4 one on a LAN address. The wildcard does.
        assert!(!qt_port_taken(&[listener6(99, 40000)], &family, 40000, &memory), "IPv6 leaves the IPv4 bind free");
        assert!(!qt_port_taken(&[listener(99, 40000, false)], &family, 40000, &memory), "a LAN address leaves it free");
        let wildcard = Listener { pid: 99, port: 40000, loopback: false, v6: false, addr_v4: chrono_mech::IPV4_ANY_ADDR };
        assert!(qt_port_taken(&[wildcard], &family, 40000, &memory), "the wildcard claims every address");

        let own = listener(10, 40000, true);
        assert!(!qt_port_taken(&[own], &family, 40000, &memory), "still being asked");
        for n in 0..=RETRIES {
            memory.refused(own, t0 + RETRY_GAP * n);
        }
        assert!(qt_port_taken(&[own], &family, 40000, &memory), "retries spent, never DevTools");
    }

    /// Every session reserves a Qt port, also for an application with no Qt at all: a stranger holding
    /// it is said only when a process of the family has Qt WebEngine loaded - once, and the family is
    /// looked into at most every few seconds while the port stays held.
    #[test]
    fn the_qt_port_is_said_taken_only_with_qt_in_the_family_and_once() {
        let family: HashSet<u32> = [10].into_iter().collect();
        let stranger = [listener(99, 40000, true)];
        let t0 = Instant::now();
        let looks = std::cell::Cell::new(0);

        let mut no_qt = Looking::default();
        let without = |_: &HashSet<u32>| {
            looks.set(looks.get() + 1);
            false
        };
        assert!(!no_qt.qt_port_taken_now(&stranger, &family, 40000, t0, without), "no Qt, nothing kept from it");
        assert!(!no_qt.qt_port_taken_now(&stranger, &family, 40000, t0 + Duration::from_secs(1), without));
        assert_eq!(looks.get(), 1, "not looked into again before the gap");
        assert!(!no_qt.qt_port_taken_now(&stranger, &family, 40000, t0 + QT_LOOK_GAP, without));
        assert_eq!(looks.get(), 2, "looked into again after it: Qt may have loaded since");

        let mut qt = Looking::default();
        assert!(qt.qt_port_taken_now(&stranger, &family, 40000, t0, |_| true));
        assert!(!qt.qt_port_taken_now(&stranger, &family, 40000, t0 + QT_LOOK_GAP * 2, |_| true), "said once");
        assert!(!Looking::default().qt_port_taken_now(&[], &family, 40000, t0, |_| true), "a free port is not taken");
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

    fn asking(sign: EngineSign) -> Asking {
        Asking { sign, endpoint_open: false, on_qt_port: false, engine_for: None }
    }

    /// Only a listener where an engine can be is asked (R4-N29): the Qt port whoever holds it, and the
    /// listeners of a process beside the Chromium runtime. The application's own server elsewhere in
    /// the family is left alone.
    #[test]
    fn only_a_listener_where_an_engine_can_be_is_asked() {
        assert!(worth_asking(&listener(20, 9222, true), None, asking(EngineSign::Runtime)), "a process beside the runtime");
        assert!(worth_asking(&listener(20, 9333, true), Some(40000), asking(EngineSign::Runtime)), "every port of it");
        assert!(!worth_asking(&listener(10, 9222, true), None, asking(EngineSign::None)), "the application's own server");
        assert!(worth_asking(&listener(10, 40000, true), Some(40000), asking(EngineSign::None)), "the Qt port, whoever holds it");
        assert!(!worth_asking(&listener(10, 40001, true), Some(40000), asking(EngineSign::None)), "the host's other ports");
    }

    /// A Qt process (R4/16 (h)): its told port is asked, its other ports are not - while it listens on
    /// the told port, before its engine runs, and while the engine has run for less than the grace - and
    /// once the engine has run that long without the told port, the application chose its own, and they
    /// are. With no told port at all, every port of it is asked, as before.
    #[test]
    fn a_qt_process_is_asked_its_told_port_and_its_others_only_once_its_engine_runs_without_it() {
        let other = listener(10, 50000, true);
        let qt = |on_qt_port, engine_for| Asking { sign: EngineSign::Qt, endpoint_open: false, on_qt_port, engine_for };
        assert!(worth_asking(&listener(10, 40000, true), Some(40000), qt(true, None)), "the told port");
        assert!(!worth_asking(&other, Some(40000), qt(true, Some(QT_ENGINE_GRACE * 5))), "it listens on the told port");
        assert!(!worth_asking(&other, Some(40000), qt(false, None)), "no engine yet");
        assert!(!worth_asking(&other, Some(40000), qt(false, Some(QT_ENGINE_GRACE / 2))), "within the grace");
        assert!(worth_asking(&other, Some(40000), qt(false, Some(QT_ENGINE_GRACE))), "a port of its own");
        assert!(worth_asking(&other, None, qt(false, None)), "no told port: every port");
    }

    /// A process whose DevTools endpoint is open is not asked about its other ports - an engine opens one,
    /// beside the runtime as in a Qt process. The told port is still the told port.
    #[test]
    fn a_process_whose_endpoint_is_open_is_not_asked_again() {
        let open = |sign| Asking { sign, endpoint_open: true, on_qt_port: false, engine_for: Some(QT_ENGINE_GRACE * 5) };
        assert!(!worth_asking(&listener(20, 9333, true), None, open(EngineSign::Runtime)));
        assert!(!worth_asking(&listener(10, 50000, true), Some(40000), open(EngineSign::Qt)));
        assert!(worth_asking(&listener(10, 40000, true), Some(40000), open(EngineSign::Qt)));

        let mut memory = Memory::default();
        let devtools = listener(20, 9222, true);
        memory.found(devtools);
        assert!(memory.endpoint_open(20) && !memory.endpoint_open(10));
        let _ = memory.due(vec![listener(20, 9333, true)], Instant::now());
        assert!(!memory.endpoint_open(20), "the endpoint left the table: the process is asked afresh");
    }

    /// The engine runs where a child of the process carries a Chromium role on its command line - a
    /// grandchild or a child with no role is not it, and neither is a command line the system would not
    /// give. This test's own process has no such child.
    #[test]
    fn the_engine_runs_where_a_child_of_the_process_has_a_chromium_role() {
        let lines = |pid: u32| match pid {
            11 => Some("helper.exe --port 5".to_string()),
            12 => Some(r#""C:\qt\QtWebEngineProcess.exe" --type=renderer --lang=en"#.to_string()),
            13 => Some(r#"my-own-engine.exe --type=gpu-process"#.to_string()),
            _ => None,
        };
        assert!(!runs_engine(10, &[(11, 10)], lines), "a child with no role");
        assert!(runs_engine(10, &[(11, 10), (12, 10)], lines));
        assert!(runs_engine(10, &[(13, 10)], lines), "by the role, whatever the executable is called");
        assert!(!runs_engine(10, &[(12, 11), (11, 10)], lines), "a grandchild is not the process's own engine");
        assert!(!runs_engine(10, &[(14, 10)], lines), "a command line the system would not give");
        assert!(!engine_runs_in(std::process::id()));
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

    /// The executable of this test sits beside no ICU data and has no Qt WebEngine, so it is no engine. A
    /// pid nobody holds cannot be asked where its executable is, and is then asked about every port
    /// rather than skipped.
    #[test]
    fn an_engine_is_told_by_the_file_beside_its_executable() {
        assert_eq!(engine_sign_of(std::process::id()), EngineSign::None);
        assert_eq!(engine_sign_of(u32::MAX - 3), EngineSign::Runtime, "a process that cannot be asked is not skipped");
        assert!(!family_has_qt(&[std::process::id()].into_iter().collect()));
        assert!(!family_has_qt(&[u32::MAX - 3].into_iter().collect()), "a list the system would not give is no evidence of Qt");
    }

    /// A Qt WebEngine host has no ICU data beside its executable, and is a Qt engine by the library it
    /// loaded (R4/16 review round). The module list is asked only away from the runtime, and a list the
    /// system would not give is a Qt process - its told port is asked - not "no engine".
    #[test]
    fn a_qt_engine_is_told_by_the_library_it_loaded() {
        use chrono_mech::ModuleProbe;
        let dir = std::env::temp_dir().join(format!("chrono-engine-signs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let host = dir.join("host.exe");
        let not_asked = || -> ModuleProbe { panic!("the module list is not asked beside the runtime") };

        assert_eq!(engine_sign(Some(host.clone()), || ModuleProbe::NotLoaded), EngineSign::None, "no runtime, no Qt");
        assert_eq!(engine_sign(Some(host.clone()), || ModuleProbe::Loaded), EngineSign::Qt, "Qt WebEngine loaded");
        assert_eq!(engine_sign(Some(host.clone()), || ModuleProbe::Unknown), EngineSign::Qt, "a list the system would not give");
        assert_eq!(engine_sign(None, not_asked), EngineSign::Runtime, "a path the system would not give");
        std::fs::write(dir.join("icudtl.dat"), b"").expect("a marker file");
        assert_eq!(engine_sign(Some(host), not_asked), EngineSign::Runtime, "beside the runtime");
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
