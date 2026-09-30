//! The bridge: what a native session holds to reach the web pages inside the application it hooked.
//!
//! An application with an embedded web engine (WebView2, Qt WebEngine) runs its pages in a process
//! the native hook does not enter, so those pages read the real clock while the host reads the
//! session's (slice A named them, slice B built the pieces, this is slice C - docs/09 section 12).
//! The session asks the engine to open a local debugging port through two environment variables,
//! a thread keeps looking for that port among the family's listeners, and every page and worker
//! behind it gets the same JS shim a Chromium session installs - built from the HOST's clock, so
//! there is one anchor and one rate for the whole application.
//!
//! One value in the session loop rather than seven variables, for the reason `Attacher` exists. The
//! bridge owns discovery, the attachers, the shared context index and the account of what was
//! covered. It does not own the clock: every call that needs an origin is handed one read off the
//! native session at that moment. And nothing network-bound runs on the session thread - connecting
//! to a found endpoint, which can take twenty seconds when an engine has gone quiet, happens on a
//! short-lived thread of its own, and the loop keeps its heartbeat (docs/09 section 12.17).

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use chrono_mech::ModuleProbe;
use chrono_proto::{ReachedEngine, TargetSpec};

use crate::cdp;
use crate::cdp_attach::{Attacher, AttacherOutcome, Pumped, ShimOrigin};
use crate::cdp_clock::{cdp_jump_expr, cdp_release_expr, cdp_set_multiplier_expr, drift_ms};
use crate::cdp_discover::{Discovered, Discovery, Notice};
use crate::embedded::engine_env;
use crate::output::diag;

/// How long a quiet engine socket may block one turn of the session loop, and how long a call to an
/// engine waits for its reply. The client's own defaults (500 ms and 10 s) suit a loop that drives
/// nothing else - this loop polls children every 100 ms and beats a heartbeat every second.
const POLL: Duration = Duration::from_millis(10);
const CALL: Duration = Duration::from_secs(2);

/// Past this much fake drift between what the pages hold and what the host reads, the pages are
/// re-anchored to the host (docs/09 section 12.5). At x60 that is 17 ms of real drift - below what
/// the two clock bases disagree by in ordinary running, so a re-anchor happens across a sleep or a
/// clock correction and not otherwise.
pub(crate) const DRIFT_MS: i64 = 1_000;

/// A DevTools endpoint was reached inside the application: a debugging port stands open in it for
/// as long as its engine runs, and the port is named in `session_verdict.engines`.
pub(crate) const KEY_DEBUG_PORT_OPEN: &str = "embedded.debug_port_open";
/// The pages of the application ran on the session clock through its engine's debugging port - said
/// in place of the strong claim that they read the real clock, which the reached pages refute.
pub(crate) const KEY_WEB_ENGINE_REACHED: &str = "embedded.web_engine_reached";
/// The strong claim slice A makes when a renderer ran without the hook, replaced by the one above.
const KEY_WEB_ENGINE_UNCOVERED: &str = "embedded.web_engine_uncovered";
/// An endpoint answered as DevTools and could not be attached to.
pub(crate) const KEY_ENGINE_UNREACHABLE: &str = "embedded.engine_unreachable";
/// The session could not look for engines at all: no port to reserve, no thread, no table.
pub(crate) const KEY_DISCOVERY_UNAVAILABLE: &str = "embedded.discovery_unavailable";
/// The port reserved for a Qt engine was held by something else when the engine would have bound it.
pub(crate) const KEY_QT_PORT_TAKEN: &str = "embedded.qt_port_taken";
/// The pages read the host machine's time zone, not the session's (ADR-8) - said when they differ.
pub(crate) const KEY_ZONE_IS_HOST: &str = "embedded.zone_is_host";
/// A WebView2 policy value in the registry was hidden by the session's variable for its duration.
pub(crate) const KEY_REGISTRY_ARGUMENTS_HIDDEN: &str = "embedded.registry_arguments_hidden";
/// A page still open when the session ended did not confirm it was let go, so it may keep the session
/// clock until it is reloaded or closed.
const KEY_PAGES_NOT_RELEASED: &str = "embedded.pages_not_released";
/// The application loaded WebView2 and the session never reached its web engine, for whatever reason
/// no other key names - so its pages may have run on the real clock (docs/09 section 12.12).
pub(crate) const KEY_WEBVIEW2_NOT_REACHED: &str = "embedded.webview2_not_reached";
/// Said beside the key above when the application's token is elevated: WebView2 ignores the
/// environment variable the session reaches its engine through for an elevated host (Microsoft Learn,
/// "Develop secure WebView2 apps"), which is the one reason the channel cannot do its work there.
pub(crate) const KEY_ELEVATED_HOST: &str = "embedded.elevated_host";

/// The library a WebView2 client loads into its own process when it creates an environment: the
/// application's own web view, as opposed to the processes of the engine it starts. Its presence in a
/// process of the family is what says the application uses WebView2 at all.
const WEBVIEW2_CLIENT_LIBRARY: &str = "EmbeddedBrowserWebView.dll";

/// How often the family is asked whether it has loaded WebView2. A look at a process's modules costs a
/// millisecond or two and the answer only ever turns from no to yes, so once a second is plenty.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// A process of the family seen with the WebView2 client library loaded, with what its token said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WebView2Host {
    pub(crate) pid: u32,
    /// `None` when the token could not be read - never taken for "not elevated".
    pub(crate) elevated: Option<bool>,
}

/// The warnings for an application that loaded WebView2 while the session never reached a web engine
/// in it. Said only when all three hold: a host was seen, no engine was reached, and no other key of
/// the channel already names why (an endpoint that answered and could not be attached to, or a
/// discovery that could not look) - a second line for the same fact would only repeat it. The
/// elevated line rides beside the first only when the token said elevated.
///
/// Pure, so each combination is tested without a session.
fn unreached_warnings(host: Option<WebView2Host>, reached: bool, explained: bool) -> Vec<String> {
    let Some(host) = host else {
        return Vec::new();
    };
    if reached || explained {
        return Vec::new();
    }
    let mut out = vec![KEY_WEBVIEW2_NOT_REACHED.to_string()];
    if host.elevated == Some(true) {
        out.push(KEY_ELEVATED_HOST.to_string());
    }
    out
}

/// What a native start needs from the channel before the target launches: the variables that make
/// an engine open its port, the port reserved for a Qt engine, and what there already is to say.
pub(crate) struct Launch {
    pub(crate) env: Vec<(String, String)>,
    qt_port: Option<u16>,
    enabled: bool,
    /// No loopback port could be reserved, so no variable was set and no engine will be found.
    unavailable: bool,
    /// A registry policy value the session's variable hides is there (docs/09 section 12.10).
    pub(crate) registry_hidden: bool,
}

impl Launch {
    /// The channel turned off: nothing in the environment, nothing to look for. What every session
    /// gets under `--no-embedded`, and what a Chromium target would get if it came this way.
    pub(crate) fn off() -> Launch {
        Launch { env: Vec::new(), qt_port: None, enabled: false, unavailable: false, registry_hidden: false }
    }

    /// Prepare the channel for a target: reserve the port a Qt engine needs telling, build the two
    /// variables on top of what the tester already has, and look whether a registry policy is about
    /// to be hidden. The three reads of the machine happen here, the composition in `compose`.
    pub(crate) fn for_target(target: &TargetSpec) -> Launch {
        if !target.embedded {
            return Launch::off();
        }
        let exe_name = std::path::Path::new(&target.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let qt_port = cdp::free_loopback_port().ok();
        let registry_hidden = chrono_mech::webview2_arguments_policy_present(&exe_name);
        Launch::compose(&chrono_mech::current_environment(), qt_port, registry_hidden)
    }

    /// The channel's launch from what the machine said: the tester's environment, the port reserved
    /// for a Qt engine (none when the machine could not hand one out), and whether a registry policy
    /// is about to be hidden. A machine without a port gets no variables at all - half a channel
    /// would find WebView2 and silently never Qt. Pure, so the shapes are tested with an environment
    /// of the test's choosing rather than whatever the tester's machine carries.
    fn compose(base: &[(String, String)], qt_port: Option<u16>, registry_hidden: bool) -> Launch {
        let Some(qt_port) = qt_port else {
            return Launch { env: Vec::new(), qt_port: None, enabled: true, unavailable: true, registry_hidden };
        };
        Launch { env: engine_env(base, qt_port), qt_port: Some(qt_port), enabled: true, unavailable: false, registry_hidden }
    }
}

/// What the bridge covered, handed over at the end of the session for the family verdict, the
/// per-context coverage events and the session warnings.
pub(crate) struct Outcome {
    pub(crate) seen: Vec<u32>,
    pub(crate) counts: BTreeMap<(u32, String), u64>,
    pub(crate) failed: usize,
    pub(crate) overflow: usize,
    /// Whether any endpoint was attached to at all. Without one there is nothing to judge.
    pub(crate) reached: bool,
    rate_changed: bool,
    pub(crate) engines: Vec<ReachedEngine>,
    warnings: Vec<String>,
    /// A process of the family that had WebView2 loaded, when one was seen at any point of the session.
    webview2: Option<WebView2Host>,
}

impl Outcome {
    /// Whether the application used WebView2 and the session never reached its engine, with nothing
    /// else in this outcome already saying why. The family cannot be `works` then: its pages may have
    /// run on the real clock, and a verdict that does not say so would be the audit claiming a channel
    /// it did not reach (untouchable rule 4).
    pub(crate) fn engine_missed(&self) -> bool {
        !self.unreached_warnings().is_empty()
    }

    fn unreached_warnings(&self) -> Vec<String> {
        let explained =
            self.warnings.iter().any(|w| w == KEY_ENGINE_UNREACHABLE || w == KEY_DISCOVERY_UNAVAILABLE);
        unreached_warnings(self.webview2, self.reached, explained)
    }

    /// The warnings the session verdict carries for this channel, each said only when it happened.
    /// `zone_differs` is whether the session zone is not the host machine's: the pages read the
    /// host's (ADR-8), which is a fact to say only when it makes a difference.
    pub(crate) fn session_warnings(&self, zone_differs: bool) -> Vec<String> {
        let mut out = self.warnings.clone();
        out.extend(self.unreached_warnings());
        let pages = !self.seen.is_empty();
        if self.reached {
            out.push(KEY_DEBUG_PORT_OPEN.to_string());
        }
        if self.overflow > 0 {
            out.push("chromium.context_ceiling_reached".to_string());
        }
        if self.rate_changed && pages {
            out.push("chromium.rate_change_affects_running_timers".to_string());
        }
        if pages && zone_differs {
            out.push(KEY_ZONE_IS_HOST.to_string());
        }
        out
    }

    /// Whether at least one page or worker was put on the session clock.
    pub(crate) fn pages_reached(&self) -> bool {
        !self.seen.is_empty()
    }
}

/// Reconcile slice A's claim with what this channel did: a renderer the hook never entered still
/// makes the strong warning, but once its pages were reached over the debugging port the claim that
/// they read the real clock is false, and the warning that replaces it says what happened instead.
pub(crate) fn reconcile_engine_warnings(warnings: &mut [String], pages_reached: bool) {
    if !pages_reached {
        return;
    }
    for w in warnings.iter_mut() {
        if w == KEY_WEB_ENGINE_UNCOVERED {
            *w = KEY_WEB_ENGINE_REACHED.to_string();
        }
    }
}

pub(crate) struct EmbeddedBridge {
    scale_duration: bool,
    discovery: Option<Discovery>,
    connects_tx: Sender<(Discovered, io::Result<Attacher>)>,
    connects: Receiver<(Discovered, io::Result<Attacher>)>,
    /// Ports a connect thread is working on, so a second `Found` for the same port waits for it.
    connecting: HashSet<u16>,
    attachers: Vec<Attacher>,
    /// What the attachers the engine closed had covered - kept, because the audit is a record of
    /// what happened (R2-W1), and an attacher dropped without it takes its evidence along.
    closed: Vec<AttacherOutcome>,
    /// One counter for every attacher: the index is a context's identity on the wire, and two
    /// attachers minting their own would both call their first page context 1.
    next_index: u32,
    engines: Vec<ReachedEngine>,
    warnings: Vec<String>,
    family: HashSet<u32>,
    /// The origin the pages hold, for the drift check - the last one broadcast, or the first one a
    /// page was shimmed from.
    pushed: Option<ShimOrigin>,
    rate_changed: bool,
    reached: bool,
    /// Whether the channel is on, which is what makes the family worth asking about WebView2.
    looking: bool,
    /// The first process of the family seen with the WebView2 client library loaded.
    host: Option<WebView2Host>,
    last_look: Option<Instant>,
}

impl EmbeddedBridge {
    /// Start looking for engines in a family, or stand inert when the channel is off or could not be
    /// set up - saying so in the latter case. Never fails: the session goes on either way.
    pub(crate) fn start(launch: &Launch, family: Vec<u32>, scale_duration: bool) -> EmbeddedBridge {
        let (connects_tx, connects) = mpsc::channel();
        let mut bridge = EmbeddedBridge {
            scale_duration,
            discovery: None,
            connects_tx,
            connects,
            connecting: HashSet::new(),
            attachers: Vec::new(),
            closed: Vec::new(),
            next_index: 0,
            engines: Vec::new(),
            warnings: Vec::new(),
            family: family.iter().copied().collect(),
            pushed: None,
            rate_changed: false,
            reached: false,
            looking: launch.enabled,
            host: None,
            last_look: None,
        };
        if !launch.enabled {
            return bridge;
        }
        if launch.unavailable {
            bridge.warn(KEY_DISCOVERY_UNAVAILABLE);
            return bridge;
        }
        match Discovery::start(family, launch.qt_port) {
            Ok(d) => bridge.discovery = Some(d),
            Err(e) => {
                diag!("chrono core: the engine discovery thread did not start: {e}");
                bridge.warn(KEY_DISCOVERY_UNAVAILABLE);
            }
        }
        bridge
    }

    /// Whether a turn of the loop has anything to pump. An inert bridge, or one still waiting for
    /// its first engine, costs the loop nothing.
    pub(crate) fn is_active(&self) -> bool {
        self.discovery.is_some() || !self.attachers.is_empty() || !self.connecting.is_empty()
    }

    /// The duration rate the pages run at: the host's, which scales only under `scale_duration`.
    pub(crate) fn scale_duration(&self) -> bool {
        self.scale_duration
    }

    /// The family as the session now knows it. Pushed to discovery only when it grew - pids are
    /// only ever added to a family.
    pub(crate) fn family(&mut self, pids: impl IntoIterator<Item = u32>) {
        let before = self.family.len();
        self.family.extend(pids);
        if self.family.len() > before
            && let Some(d) = &self.discovery
        {
            d.update_family(self.family.iter().copied().collect());
        }
    }

    /// Ask the family whether any process of it has loaded WebView2, at most once a second and only
    /// until one has. `hosts` are the processes the hook is in - the application itself - and not the
    /// engine's own processes, which carry the engine and not the client library. The hook cannot
    /// answer this for an elevated host, whose engine a system service starts, so it is asked from
    /// outside (docs/09 section 12.12).
    pub(crate) fn look_for_webview2(&mut self, hosts: &[u32]) {
        self.look_with(hosts, false, Instant::now(), chrono_mech::process_has_module, chrono_mech::process_elevated);
    }

    /// The same look once more as the session closes, whatever the cadence says: what turned up in the
    /// last second counts.
    pub(crate) fn look_for_webview2_last(&mut self, hosts: &[u32]) {
        self.look_with(hosts, true, Instant::now(), chrono_mech::process_has_module, chrono_mech::process_elevated);
    }

    /// The look over any two questions, so the cadence, the stopping once found and the order are
    /// tested with answers of the test's making.
    fn look_with(
        &mut self,
        hosts: &[u32],
        forced: bool,
        now: Instant,
        has: impl Fn(u32, &str) -> ModuleProbe,
        elevated: impl Fn(u32) -> Option<bool>,
    ) {
        if !self.looking || self.host.is_some() {
            return;
        }
        if !forced && self.last_look.is_some_and(|t| now.saturating_duration_since(t) < LOOK_EVERY) {
            return;
        }
        self.last_look = Some(now);
        self.host = hosts
            .iter()
            .find(|&&pid| has(pid, WEBVIEW2_CLIENT_LIBRARY) == ModuleProbe::Loaded)
            .map(|&pid| WebView2Host { pid, elevated: elevated(pid) });
    }

    /// One turn: take what discovery found and start connecting to it, take what connected and
    /// attach to its pages, and pump every attacher. `origin` is the host's clock now - every page
    /// shimmed this turn starts from it.
    pub(crate) fn pump(&mut self, origin: ShimOrigin) {
        self.take_notices();
        self.take_connects(origin);
        let mut i = 0;
        while i < self.attachers.len() {
            if self.attachers[i].pump(origin, &mut self.next_index) == Pumped::Closed {
                // The engine closed the connection - the application closed its web view, or the
                // engine restarted. Its evidence is handed over before the attacher goes, and a new
                // endpoint for the same pages will be found as a new port.
                let gone = self.attachers.swap_remove(i);
                self.closed.push(gone.into_outcome());
            } else {
                i += 1;
            }
        }
    }

    fn take_notices(&mut self) {
        let Some(discovery) = &self.discovery else {
            return;
        };
        let mut notices = Vec::new();
        while let Some(notice) = discovery.try_recv() {
            notices.push(notice);
        }
        for notice in notices {
            match notice {
                Notice::Found(found) => {
                    // One attacher per port: a socket that left the table for one sweep and came
                    // back is found twice, and a connect already under way is not started again.
                    let known = self.attachers.iter().any(|a| a.port() == found.port)
                        || self.connecting.contains(&found.port);
                    if !known {
                        self.spawn_connect(found);
                    }
                }
                Notice::PortTaken(_) => self.warn(KEY_QT_PORT_TAKEN),
                Notice::Unavailable(why) => {
                    diag!("chrono core: engine discovery stopped: {why}");
                    self.warn(KEY_DISCOVERY_UNAVAILABLE);
                    self.discovery = None;
                }
            }
        }
    }

    /// Connect on a thread of its own: `/json/version` and the WebSocket handshake each wait up to
    /// ten seconds on an engine that has gone quiet, and the session loop cannot stand still for that.
    fn spawn_connect(&mut self, found: Discovered) {
        let tx = self.connects_tx.clone();
        let port = found.port;
        let spawned = thread::Builder::new().name("chrono-attach".into()).spawn(move || {
            let result = Attacher::connect(found.host, found.port);
            let _ = tx.send((found, result));
        });
        match spawned {
            Ok(_) => {
                self.connecting.insert(port);
            }
            Err(e) => {
                diag!("chrono core: cannot start a thread to reach the engine on port {port}: {e}");
                self.warn(KEY_ENGINE_UNREACHABLE);
            }
        }
    }

    fn take_connects(&mut self, origin: ShimOrigin) {
        loop {
            match self.connects.try_recv() {
                Ok((found, Ok(mut attacher))) => {
                    self.connecting.remove(&found.port);
                    // Budgets that keep the loop's cadence, and the probe that keeps a page the hook
                    // already covers from being shimmed a second time.
                    if let Err(e) = attacher.set_budgets(POLL, CALL) {
                        diag!("chrono core: engine on port {}: {e}", found.port);
                    }
                    attacher.probe_clock_before_shim();
                    if self.pushed.is_none() {
                        self.pushed = Some(origin);
                    }
                    if let Err(e) = attacher.attach_existing(origin, &mut self.next_index) {
                        diag!("chrono core: engine on port {}: {e}", found.port);
                    }
                    self.reached = true;
                    self.engines.push(ReachedEngine { pid: found.pid, port: found.port, browser: found.browser });
                    self.attachers.push(attacher);
                }
                Ok((found, Err(e))) => {
                    self.connecting.remove(&found.port);
                    diag!("chrono core: cannot reach the engine on port {}: {e}", found.port);
                    self.warn(KEY_ENGINE_UNREACHABLE);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    /// Read every live context's call counts (once a second, like the Chromium session).
    pub(crate) fn poll_counts(&mut self) {
        for attacher in &mut self.attachers {
            attacher.poll_counts();
        }
    }

    /// Bring the pages back to the host's clock when the two have drifted apart - across a sleep, or
    /// a correction of the system clock (docs/09 section 12.5). The wall alone moves, as in a jump.
    pub(crate) fn resync(&mut self, fresh: ShimOrigin) {
        if self.attachers.is_empty() {
            return;
        }
        if let Some(pushed) = self.pushed
            && drift_ms(pushed, fresh).abs() > DRIFT_MS
        {
            self.broadcast(&cdp_jump_expr(fresh.fake0, fresh.real0));
            self.pushed = Some(fresh);
        }
    }

    /// The host changed its rate: push the host's new origin and both rates to every page.
    pub(crate) fn set_multiplier(&mut self, fresh: ShimOrigin) {
        if !self.attachers.is_empty() {
            self.rate_changed = true;
        }
        self.broadcast(&cdp_set_multiplier_expr(fresh.fake0, fresh.real0, fresh.mult, fresh.dur));
        self.pushed = Some(fresh);
    }

    /// The host jumped its wall: push the new origin, wall only.
    pub(crate) fn jump(&mut self, fresh: ShimOrigin) {
        self.broadcast(&cdp_jump_expr(fresh.fake0, fresh.real0));
        self.pushed = Some(fresh);
    }

    /// Let every page go before the connections close, the way the hook lets the host go: the wall
    /// back on the real clock and the duration axis on from where it stands at rate 1. For a session
    /// whose application outlives it - the caller decides that, a page of an application that has
    /// exited is gone and has nothing to let go of.
    ///
    /// Measured before this existed (2026-09-24, WebView2 host at x60): the host went back to the real
    /// clock at `end` and its page stayed on the session date, running on at the session rate for as
    /// long as it lived - also with no opt-in at all, because this channel is on by default. A page
    /// that does not confirm is named in the report rather than assumed let go (rule 6).
    pub(crate) fn release_pages(&mut self) {
        let expr = cdp_release_expr();
        let unconfirmed: u32 = self.attachers.iter_mut().map(|a| a.release(&expr)).sum();
        if unconfirmed > 0 {
            self.warn(KEY_PAGES_NOT_RELEASED);
        }
    }

    fn broadcast(&mut self, expr: &str) {
        for attacher in &mut self.attachers {
            attacher.broadcast(expr);
        }
    }

    fn warn(&mut self, key: &str) {
        if !self.warnings.iter().any(|w| w == key) {
            self.warnings.push(key.to_string());
        }
    }

    /// Hand over what the channel covered. Closes every connection. A page still open keeps its shim,
    /// which is why an application that outlives the session gets `release_pages` first. A document
    /// loaded after this starts without the shim: its registration dies with the connection (measured
    /// 2026-09-24, a reload after the session came back on the real clock).
    pub(crate) fn finish(mut self) -> Outcome {
        self.poll_counts();
        let mut outcome = Outcome {
            seen: Vec::new(),
            counts: BTreeMap::new(),
            failed: 0,
            overflow: 0,
            reached: self.reached,
            rate_changed: self.rate_changed,
            engines: self.engines,
            warnings: self.warnings,
            webview2: self.host,
        };
        let live = self.attachers.into_iter().map(|a| {
            if a.native() > 0 {
                diag!("chrono core: {} context(s) on port {} were already on the session clock natively", a.native(), a.port());
            }
            a.into_outcome()
        });
        for part in self.closed.into_iter().chain(live) {
            outcome.seen.extend(part.seen);
            outcome.counts.extend(part.counts);
            outcome.failed += part.failed;
            outcome.overflow += part.overflow;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(embedded: bool) -> TargetSpec {
        TargetSpec { path: "C:/apps/host.exe".into(), args: Vec::new(), cwd: None, embedded, console: Default::default() }
    }

    /// The opt-out means no variable in the environment and nothing to look for. On, the machine is
    /// asked for a port - and what the launch is made of is judged on `compose`, below, with an
    /// environment of the test's choosing: this machine's own may already carry a debugging switch,
    /// which `engine_env` honours by adding nothing, and a test that read it would fail on exactly
    /// the tester's machine it is meant to describe.
    #[test]
    fn the_launch_reads_the_machine_only_when_the_channel_is_on() {
        let off = Launch::for_target(&target(false));
        assert!(off.env.is_empty());
        assert!(!off.enabled);
        assert!(!off.unavailable);

        let on = Launch::for_target(&target(true));
        assert!(on.enabled);
        assert!(!on.unavailable, "this machine hands out loopback ports");
        assert!(on.qt_port.is_some());
    }

    /// From a bare environment the two variables are there - the WebView2 one asking for an
    /// ephemeral port, the Qt one naming the port that was reserved for it. Without a port there is
    /// no variable at all, and the launch says the channel is unavailable. The registry finding rides
    /// through untouched either way.
    #[test]
    fn the_launch_is_composed_from_the_environment_and_the_reserved_port() {
        let on = Launch::compose(&[], Some(45_001), false);
        assert!(on.enabled && !on.unavailable);
        assert_eq!(on.qt_port, Some(45_001));
        let names: Vec<&str> = on.env.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"), "{names:?}");
        assert!(names.contains(&"QTWEBENGINE_REMOTE_DEBUGGING"), "{names:?}");
        let qt = on.env.iter().find(|(n, _)| n == "QTWEBENGINE_REMOTE_DEBUGGING").map(|(_, v)| v.clone()).unwrap();
        assert_eq!(qt, "127.0.0.1:45001", "the Qt variable names the reserved port");

        let no_port = Launch::compose(&[], None, true);
        assert!(no_port.enabled && no_port.unavailable);
        assert!(no_port.env.is_empty(), "half a channel would find WebView2 and never Qt");
        assert!(no_port.registry_hidden);
    }

    /// Slice A's strong claim - the pages read the real clock - is refuted once the pages were
    /// reached, and the warning that replaces it says what happened instead. With no page reached
    /// the claim stands. The weak claim and everything else is left alone either way.
    #[test]
    fn the_strong_engine_warning_gives_way_once_the_pages_were_reached() {
        let mut reached = vec!["inheritance.children_uncovered".to_string(), KEY_WEB_ENGINE_UNCOVERED.to_string()];
        reconcile_engine_warnings(&mut reached, true);
        assert_eq!(reached, vec!["inheritance.children_uncovered", KEY_WEB_ENGINE_REACHED]);

        let mut unreached = vec![KEY_WEB_ENGINE_UNCOVERED.to_string()];
        reconcile_engine_warnings(&mut unreached, false);
        assert_eq!(unreached, vec![KEY_WEB_ENGINE_UNCOVERED]);

        let mut weak = vec!["embedded.web_engine_processes_uncovered".to_string()];
        reconcile_engine_warnings(&mut weak, true);
        assert_eq!(weak, vec!["embedded.web_engine_processes_uncovered"]);
    }

    fn outcome(reached: bool, seen: &[u32], overflow: usize, rate_changed: bool, warnings: &[&str]) -> Outcome {
        Outcome {
            seen: seen.to_vec(),
            counts: BTreeMap::new(),
            failed: 0,
            overflow,
            reached,
            rate_changed,
            engines: Vec::new(),
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
            webview2: None,
        }
    }

    /// Each warning only when it happened: the open port once an engine was reached, the ceiling
    /// once a page was refused past it, the running-timers caveat once the rate changed WITH pages
    /// to feel it, the zone once there are pages AND the zones differ. What the bridge noted along
    /// the way (an unreachable engine, say) comes first, in the order it was noted.
    #[test]
    fn the_session_warnings_say_only_what_happened() {
        assert!(outcome(false, &[], 0, false, &[]).session_warnings(true).is_empty());
        assert_eq!(outcome(true, &[], 0, true, &[]).session_warnings(true), vec![KEY_DEBUG_PORT_OPEN]);
        assert_eq!(
            outcome(true, &[1], 1, true, &[KEY_ENGINE_UNREACHABLE]).session_warnings(true),
            vec![
                KEY_ENGINE_UNREACHABLE,
                KEY_DEBUG_PORT_OPEN,
                "chromium.context_ceiling_reached",
                "chromium.rate_change_affects_running_timers",
                KEY_ZONE_IS_HOST,
            ]
        );
        assert_eq!(outcome(true, &[1], 0, false, &[]).session_warnings(false), vec![KEY_DEBUG_PORT_OPEN]);
    }

    fn host(elevated: Option<bool>) -> Option<WebView2Host> {
        Some(WebView2Host { pid: 42, elevated })
    }

    /// Every combination of the facts the claim rests on. No host seen says nothing, reached or not. A
    /// host with an engine reached says nothing. A host with nothing reached and nothing else naming why
    /// says the claim - and the elevated line only when the token SAID elevated: one that could not be
    /// read is not one that is not elevated, and neither is it elevated.
    #[test]
    fn an_unreached_webview2_is_said_only_when_a_host_was_seen_nothing_was_reached_and_nothing_else_says_why() {
        assert!(unreached_warnings(None, false, false).is_empty());
        assert!(unreached_warnings(None, true, false).is_empty());
        assert!(unreached_warnings(host(Some(true)), true, false).is_empty());
        assert_eq!(unreached_warnings(host(Some(false)), false, false), vec![KEY_WEBVIEW2_NOT_REACHED]);
        assert_eq!(unreached_warnings(host(None), false, false), vec![KEY_WEBVIEW2_NOT_REACHED]);
        assert_eq!(
            unreached_warnings(host(Some(true)), false, false),
            vec![KEY_WEBVIEW2_NOT_REACHED, KEY_ELEVATED_HOST]
        );
        assert!(unreached_warnings(host(Some(true)), false, true).is_empty(), "another key already names why");
    }

    /// The outcome applies that rule to what the bridge noted: an unreachable endpoint and a discovery
    /// that could not look each already explain a miss, a Qt port taken does not (it is about another
    /// engine), and the claim rides the session warnings in front of the open-port line.
    #[test]
    fn the_outcome_misses_its_engine_only_when_a_host_was_seen_and_no_key_already_says_why() {
        let with_host = |reached: bool, warnings: &[&str], seen: Option<WebView2Host>| {
            let mut o = outcome(reached, &[], 0, false, warnings);
            o.webview2 = seen;
            o
        };
        assert!(!outcome(false, &[], 0, false, &[]).engine_missed());
        assert!(with_host(false, &[], host(Some(true))).engine_missed());
        assert!(!with_host(true, &[], host(Some(true))).engine_missed());
        assert!(!with_host(false, &[KEY_ENGINE_UNREACHABLE], host(None)).engine_missed());
        assert!(!with_host(false, &[KEY_DISCOVERY_UNAVAILABLE], host(None)).engine_missed());
        assert!(with_host(false, &[KEY_QT_PORT_TAKEN], host(None)).engine_missed());
        assert_eq!(
            with_host(false, &[], host(Some(true))).session_warnings(false),
            vec![KEY_WEBVIEW2_NOT_REACHED, KEY_ELEVATED_HOST]
        );
    }

    /// A bridge with the channel on and no discovery to start: nothing is reserved and no thread runs.
    fn looking_bridge() -> EmbeddedBridge {
        let launch = Launch { env: Vec::new(), qt_port: None, enabled: true, unavailable: true, registry_hidden: false };
        EmbeddedBridge::start(&launch, vec![1], false)
    }

    /// The family is asked only when the channel is on, in order, stopping at the first process that
    /// has the library and reading the token of that one alone - and once one answered, never again.
    #[test]
    fn the_family_is_asked_about_webview2_only_when_the_channel_is_on_and_only_until_one_answers() {
        use std::cell::Cell;
        let asked = Cell::new(0u32);
        let has = |pid: u32, library: &str| {
            asked.set(asked.get() + 1);
            assert_eq!(library, WEBVIEW2_CLIENT_LIBRARY);
            if pid == 2 { ModuleProbe::Loaded } else { ModuleProbe::NotLoaded }
        };
        let token = |pid: u32| Some(pid == 2);
        let t0 = Instant::now();

        let mut off = EmbeddedBridge::start(&Launch::off(), vec![1], false);
        off.look_with(&[1, 2], true, t0, has, token);
        assert_eq!(asked.get(), 0, "a channel that is off asks nothing");
        assert_eq!(off.host, None);

        let mut on = looking_bridge();
        on.look_with(&[1, 2, 3], false, t0, has, token);
        assert_eq!(asked.get(), 2, "the walk stops at the first process that has it");
        assert_eq!(on.host, Some(WebView2Host { pid: 2, elevated: Some(true) }));
        on.look_with(&[1, 2, 3], true, t0 + Duration::from_secs(9), has, token);
        assert_eq!(asked.get(), 2, "once found, even a forced look does not ask again");
    }

    /// A look that found nothing waits out the cadence before the next one, unless it is the closing
    /// look, which ignores it. A process that could not be looked into is not one that has the library.
    #[test]
    fn a_look_that_found_nothing_waits_out_the_cadence_unless_it_is_forced() {
        use std::cell::Cell;
        let asked = Cell::new(0u32);
        let none = |_: u32, _: &str| {
            asked.set(asked.get() + 1);
            ModuleProbe::NotLoaded
        };
        let t0 = Instant::now();
        let mut bridge = looking_bridge();
        bridge.look_with(&[1], false, t0, none, |_| None);
        bridge.look_with(&[1], false, t0 + LOOK_EVERY / 2, none, |_| None);
        assert_eq!(asked.get(), 1, "inside the cadence nothing is asked");
        bridge.look_with(&[1], false, t0 + LOOK_EVERY, none, |_| None);
        assert_eq!(asked.get(), 2);
        bridge.look_with(&[1], true, t0 + LOOK_EVERY, none, |_| None);
        assert_eq!(asked.get(), 3, "the closing look ignores the cadence");

        bridge.look_with(&[1], true, t0 + LOOK_EVERY, |_: u32, _: &str| ModuleProbe::Unknown, |_| Some(true));
        assert_eq!(bridge.host, None, "an unknown answer is not a sighting");
    }
}
