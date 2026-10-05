//! The attacher: one DevTools endpoint and every JS context reached through it, shimmed.
//!
//! This is the middle of `cdp_session` lifted out and given a name. That loop kept six variables
//! for one job - which contexts it still talks to, which it ever covered, their call counts, the
//! index each target keeps across re-attaches, how many shims failed, and the next index - and the
//! embedded-engine channel (docs/09) needs the same job done on a port it discovered rather than
//! one it opened. Two copies of that state would drift the first time one of them learned
//! something, so there is one, here, and both the Chromium session and the embedded probe drive it.
//!
//! The attacher owns the connection and the per-context bookkeeping. It does NOT own the clock:
//! every attach asks the caller for the clock's current origin, so a context that arrives after an
//! in-flight rate change starts on the same clock as the rest (rule 3). And it does not own the
//! context index counter: several attachers in one session (slice C) must hand out disjoint
//! indexes, because the index is the unit's identity on the wire.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::cdp;
use crate::cdp_audit::context_index_for;
use crate::cdp_requests::{CdpContext, Requests};
use crate::zone::{now_epoch_ms, WALL_MAX_MS};

/// How long a clock move waits for the pages' new hooks before the command is acknowledged (R4-S10,
/// ADR-20). A page that has not answered by then still gets its move once its hook comes back - this
/// bounds the wait, not the move.
pub(crate) const MOVE_WAIT_MS: u64 = 2_000;

/// How long the end of a session waits for the release and the last counts together. A GUI gives the
/// core two seconds after `end` (`CoreClient`), and the session has a verdict to write after this.
pub(crate) const END_WAIT: Duration = Duration::from_millis(750);

/// How long one pump turn goes on taking messages that are already here, so a burst of attaches
/// cannot hold the loop from its heartbeat (R4-S10). The rest is taken on the next turn.
const TURN_BUDGET: Duration = Duration::from_millis(100);

/// The clock origin a shim is built from: fake start and real start (both Unix-epoch ms), the wall
/// rate and the duration rate. A Chromium session runs both rates at the multiplier. A page inside a
/// natively hooked application follows that application's duration axis, which scales only under
/// `scale_duration` (docs/09 section 12.6).
///
/// `scheduled` is a rate change of a Chromium session still waiting for its instant (R4-S17). The
/// pages of an embedded engine follow the host's clock as it is, so theirs is always `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShimOrigin {
    pub(crate) fake0: i64,
    pub(crate) real0: i64,
    pub(crate) mult: i64,
    pub(crate) dur: i64,
    pub(crate) scheduled: Option<cdp::ScheduledRate>,
}

impl ShimOrigin {
    /// The shim source that puts a context on this clock, checking that it reads the zone `zone`
    /// (the session's bias, `None` for no check).
    fn shim(&self, zone: Option<i32>) -> String {
        cdp::build_shim(self.fake0, self.real0, self.mult, self.dur, self.scheduled, WALL_MAX_MS, zone)
    }
}

/// What a context answered when asked which clock it already reads (docs/09 section 12.17 point 3).
/// Asked before the shim goes in, by an attacher inside a natively hooked session: a context that
/// already reads the session clock through the native mechanism must not get the shim on top, or it
/// would scale the fake wall a second time and the audit would still call it covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockRead {
    /// Our own shim marker is there - a page that reloaded and re-ran the add-script hook.
    Shim,
    /// The reading stands nearer the session clock than the real one: covered natively already.
    Native,
    /// The reading is real, or there was no reading to judge by - the shim is what covers it.
    Real,
}

/// The probe: our marker if it is there, otherwise the context's own `Date.now()` as text.
const CLOCK_PROBE_EXPR: &str = "globalThis.__chronomock ? 'shim' : String(Date.now())";

/// Judge a probe reply against where the session clock and the real clock stand now. No reply, or one
/// that is not a number, is `Real`: the absence of evidence that a context is covered is not evidence
/// that it is, and the shim is the safe side. A reading equally far from both is `Real` as well - that
/// only happens with the session clock within milliseconds of the real one, where a second
/// substitution is the identity. The distances are unsigned, because the reading is whatever the
/// page's own `Date.now` returned - a subtraction near `i64::MIN` panicked the core (R4-N23).
pub(crate) fn classify_clock_read(reply: Option<&str>, fake_now_ms: i64, real_now_ms: i64) -> ClockRead {
    match reply {
        Some("shim") => ClockRead::Shim,
        Some(text) => match text.parse::<i64>() {
            Ok(read) if read.abs_diff(fake_now_ms) < read.abs_diff(real_now_ms) => ClockRead::Native,
            _ => ClockRead::Real,
        },
        None => ClockRead::Real,
    }
}

/// How many contexts one attacher will shim over its lifetime. The pid registry has the same shape
/// of ceiling (256 slots, `coverage.pid_registry_full`), for the same reason: a family that fans
/// out without bound must not grow the audit without bound. A context past this is counted in
/// `overflow` and not shimmed, and the caller says so (untouchable rule 4).
pub(crate) const MAX_CONTEXTS: usize = 256;

/// What one turn of the pump found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pumped {
    /// Nothing within the poll interval, or an event that changed no context.
    Idle,
    /// A context attached and was shimmed (or refused past the ceiling, or failed - see the counters).
    Attached,
    /// A context went away and was dropped from the live list.
    Detached,
    /// The connection is gone: the application closed, or the engine did.
    Closed,
}

impl Pumped {
    /// The outcome a turn of several messages reports: a closed connection over anything, an attach
    /// over a detach, anything over nothing.
    fn stronger(self, other: Pumped) -> Pumped {
        let rank = |p: &Pumped| match p {
            Pumped::Idle => 0,
            Pumped::Detached => 1,
            Pumped::Attached => 2,
            Pumped::Closed => 3,
        };
        if rank(&other) > rank(&self) { other } else { self }
    }
}

/// How many messages one turn of the pump handles after the first - all of them already here, so
/// none is waited for (R4-S14). One message a turn handled about ten a second on an engine inside a
/// natively hooked session, so a page load's seventy-odd events kept a worker auto-attach paused on
/// start waiting about seven seconds for its shim and its release. Bounded so a target that never
/// stops talking cannot keep the turn from ending.
const PUMP_DRAIN_MAX: usize = 256;

/// The rest of a pump turn after its first message: `next_ready` handles the next message that is
/// already here and says what it found, or `None` when nothing is. Stops at a closed connection, at
/// the end of what is here, at [`PUMP_DRAIN_MAX`], or once `in_budget` says the turn has run long
/// enough, and reports the strongest outcome of the turn.
fn drain_turn(first: Pumped, mut in_budget: impl FnMut() -> bool, mut next_ready: impl FnMut() -> Option<Pumped>) -> Pumped {
    let mut turn = first;
    for _ in 0..PUMP_DRAIN_MAX {
        if turn == Pumped::Closed || !in_budget() {
            break;
        }
        match next_ready() {
            Some(found) => turn = turn.stronger(found),
            None => break,
        }
    }
    turn
}

/// What one attacher covered, handed over when it is done - because it was closed by the engine, or
/// because the session ended. The counts and the seen list are the audit's evidence, and an attacher
/// dropped without this hand-over takes them with it (docs/09 section 12.17).
pub(crate) struct AttacherOutcome {
    pub(crate) seen: Vec<u32>,
    pub(crate) counts: BTreeMap<(u32, String), u64>,
    pub(crate) failed: usize,
    pub(crate) overflow: usize,
    /// How many pages missed a clock move (`chromium.clock_move_missed`).
    pub(crate) moves_missed: usize,
    /// How many contexts read a zone other than the session's at some point.
    pub(crate) zone_missed: usize,
    /// How many pages were hidden at some point while their timers ran faster (R4-N28).
    pub(crate) hidden_fast: usize,
}

pub(crate) struct Attacher {
    client: cdp::CdpClient,
    port: u16,
    /// Whether to ask a context which clock it reads before shimming it. Off for the Chromium
    /// session, whose contexts nothing else covers. On for an attacher inside a native session.
    probe_clock: bool,
    /// Contexts found already on the session clock and left alone, for the diagnostic line.
    native: usize,
    /// The context indexes refused past the ceiling, each once: a refused target that detaches and
    /// re-attaches keeps its index and must not be counted again.
    refused: HashSet<u32>,
    /// Who we still TALK to, and every request in flight to them (ADR-20).
    requests: Requests,
    /// Who this attacher ever COVERED, in attach order, append-only: the audit is a record of what
    /// happened, not of what is still open, so a context that reloaded or closed keeps its evidence
    /// (R2-W1). Once per CONTEXT rather than once per attach.
    seen: Vec<u32>,
    failed: usize,
    overflow: usize,
    /// targetId -> context index, so a re-attached context keeps the identity it already had.
    index_by_target: HashMap<String, u32>,
    /// The session is ending: a context that attaches now is let go without a shim. It would get a
    /// clock nobody moves or releases any more, and shimming it would hold the end for an attach.
    ending: bool,
    /// The session's zone bias (minutes west of UTC), which every context is put on through the
    /// engine's zone override. `None` sets no zone and checks none.
    zone: Option<i32>,
    /// A context went away since the zone was last given to the rest. The override is held by one
    /// context per renderer process, and the others in that process lose it with the holder, so the
    /// rest are given it again - once per turn, however many went away in it.
    zone_dirty: bool,
}

impl Attacher {
    /// Connect to the browser endpoint on a loopback host and port and arm auto-attach, so every
    /// page and worker the browser creates from now on arrives as an `attachedToTarget` event,
    /// paused until the shim is in, all of it by `deadline`. The pages that ALREADY exist are the
    /// caller's next call. The host is `127.0.0.1` or `::1` - whichever family the listener was
    /// found on.
    pub(crate) fn connect(host: &str, port: u16, deadline: Instant) -> io::Result<Attacher> {
        let mut client = cdp::CdpClient::connect_to_port(host, port, deadline)?;
        client.call_until(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
            None,
            deadline,
        )?;
        Ok(Attacher::over(client, port))
    }

    /// An attacher over a client already connected and armed - what [`Attacher::connect`] builds once
    /// the endpoint answered, and what a test builds over a socket of its own.
    fn over(client: cdp::CdpClient, port: u16) -> Attacher {
        Attacher {
            client,
            port,
            probe_clock: false,
            native: 0,
            requests: Requests::default(),
            seen: Vec::new(),
            failed: 0,
            overflow: 0,
            refused: HashSet::new(),
            index_by_target: HashMap::new(),
            ending: false,
            zone: None,
            zone_dirty: false,
        }
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Put every context this attacher shims from now on in the session's zone, `bias_min` minutes
    /// west of UTC as Windows counts it, and check that each one reads it. Called before the first
    /// attach, so no context is shimmed without it.
    pub(crate) fn set_zone(&mut self, bias_min: i32) {
        self.zone = Some(bias_min);
    }

    /// How many contexts read a zone other than the session's at some point - the override did not
    /// reach them, or their process lost it.
    pub(crate) fn zone_missed(&self) -> usize {
        self.requests.zone_missed()
    }

    /// How many pages were hidden at some point while their timers ran faster - an engine slows the
    /// timers of a hidden window, unless the session started it with that switched off (R4-N28).
    pub(crate) fn hidden_fast(&self) -> usize {
        self.requests.hidden_fast()
    }

    /// Shorten the client's poll interval and call deadline - see `CdpClient::set_budgets`. For an
    /// attacher driven from a loop that has its own cadence to keep.
    pub(crate) fn set_budgets(&mut self, poll: Duration, call: Duration) {
        self.client.set_budgets(poll, call);
    }

    /// Ask each context which clock it reads before shimming it (see [`ClockRead`]).
    pub(crate) fn probe_clock_before_shim(&mut self) {
        self.probe_clock = true;
    }

    /// How many contexts were found already on the session clock and left unshimmed.
    pub(crate) fn native(&self) -> usize {
        self.native
    }

    /// Hand over what this attacher covered. The connection closes with it.
    pub(crate) fn into_outcome(self) -> AttacherOutcome {
        let moves_missed = self.requests.moves_missed();
        let zone_missed = self.requests.zone_missed();
        let hidden_fast = self.requests.hidden_fast();
        AttacherOutcome {
            seen: self.seen,
            counts: self.requests.into_counts(),
            failed: self.failed,
            overflow: self.overflow,
            moves_missed,
            zone_missed,
            hidden_fast,
        }
    }

    /// Attach to every shimmable target the browser already has - the pages an embedded engine
    /// opened before the session found its port. Auto-attach delivers them too (measured in slice
    /// B), so this is the belt, asked for by name and idempotent: a target the index map already
    /// knows is skipped, whichever road it came by. Stops starting new attaches at `deadline` and
    /// leaves the rest to auto-attach. Returns how many were newly shimmed.
    pub(crate) fn attach_existing(&mut self, origin: ShimOrigin, next_index: &mut u32, deadline: Instant) -> io::Result<usize> {
        let reply = self.client.call_until("Target.getTargets", json!({}), None, deadline)?;
        let mut attached = 0;
        for (tid, ty) in new_targets(&reply, &self.index_by_target) {
            if Instant::now() >= deadline {
                break;
            }
            let params = json!({ "targetId": tid, "flatten": true });
            let Ok(reply) = self.client.call_until("Target.attachToTarget", params, None, deadline) else {
                continue;
            };
            let sid = reply["sessionId"].as_str().unwrap_or("").to_string();
            if sid.is_empty() {
                continue;
            }
            self.shim(sid, ty, tid, origin, next_index, Some(deadline));
            attached += 1;
        }
        Ok(attached)
    }

    /// One turn: poll the connection (bounded by the client's poll interval), act on what came, then
    /// on everything else that is already here, without waiting for more (R4-S14) and for no longer
    /// than [`TURN_BUDGET`].
    pub(crate) fn pump(&mut self, origin: ShimOrigin, next_index: &mut u32) -> Pumped {
        let polled = self.client.poll();
        let started = Instant::now();
        let first = self.handle(polled, origin, next_index);
        let turn = drain_turn(first, || started.elapsed() < TURN_BUDGET, || match self.client.poll_ready() {
            Ok(None) => None,
            ready => Some(self.handle(ready, origin, next_index)),
        });
        self.give_zone_again();
        turn
    }

    /// Give the session's zone again to every live context once one went away (see `zone_dirty`).
    /// Measured: a second page in a renderer process reads the holder's override, and goes back to the
    /// host's zone when the holder closes - asked again, it takes the override itself. Not while the
    /// session is ending, when the release takes the zone away.
    fn give_zone_again(&mut self) {
        if !self.zone_dirty || self.ending {
            return;
        }
        self.zone_dirty = false;
        if let Some(bias) = self.zone {
            self.requests.send_zone(&cdp::zone_id(bias), &mut self.client);
        }
    }

    /// Act on one polled message. Every answer goes to the request table, whatever it answers and
    /// whenever it comes (ADR-20).
    fn handle(&mut self, polled: io::Result<Option<cdp::Msg>>, origin: ShimOrigin, next_index: &mut u32) -> Pumped {
        match polled {
            Ok(Some(cdp::Msg::Response { id, result, error })) => {
                let reply = if error.is_none() { Ok(result) } else { Err(()) };
                self.requests.on_reply(id, reply, &mut self.client);
                Pumped::Idle
            }
            Ok(Some(cdp::Msg::Event { method, params, .. })) if method == "Target.attachedToTarget" => {
                let sid = params["sessionId"].as_str().unwrap_or("").to_string();
                let ty = params["targetInfo"]["type"].as_str().unwrap_or("").to_string();
                let tid = params["targetInfo"]["targetId"].as_str().unwrap_or("").to_string();
                if sid.is_empty() {
                    return Pumped::Idle;
                }
                if self.ending || !cdp::is_shimmable(&ty) {
                    // Auto-attach paused it on start, like everything it delivers. A target with no
                    // timer of ours to cover - or one that arrives as the session ends - is let go at
                    // once: left as it arrived, it would stay paused.
                    self.resume(&sid);
                    return Pumped::Idle;
                }
                self.shim(sid, ty, tid, origin, next_index, None);
                Pumped::Attached
            }
            // A context that went away - a closed window, a recycled worker. Dropped from the live
            // list with its requests in flight: its counts stay in the table and its index in `seen`.
            Ok(Some(cdp::Msg::Event { method, params, .. }))
                if method == "Target.detachedFromTarget" || method == "Target.targetDestroyed" =>
            {
                let sid = params["sessionId"].as_str().unwrap_or("");
                let tid = params["targetId"].as_str().unwrap_or("");
                if self.requests.forget(sid, tid) {
                    // It may have held its process's zone override for the others (see `zone_dirty`).
                    self.zone_dirty = self.zone.is_some();
                    Pumped::Detached
                } else {
                    Pumped::Idle
                }
            }
            Ok(_) => Pumped::Idle,
            Err(_) => Pumped::Closed,
        }
    }

    /// Handle what comes until `done` says so or `deadline` passes - the one place an attacher waits
    /// for answers. Messages are handled exactly as a turn handles them, so an attach or a detach in
    /// the meantime is not lost, and the deadline bounds the wait, never what is learned (ADR-20).
    fn wait_until(&mut self, deadline: Instant, origin: ShimOrigin, next_index: &mut u32, done: impl Fn(&Requests) -> bool) {
        while !done(&self.requests) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            let polled = self.client.poll_for(left);
            if self.handle(polled, origin, next_index) == Pumped::Closed {
                return;
            }
        }
    }

    /// Give a newly attached context its index and its shim. The index is keyed by the CDP targetId,
    /// which Chromium keeps across re-attaches (R3-7). The shim is built from the clock's CURRENT
    /// origin, never the session's initial one.
    fn shim(&mut self, sid: String, ty: String, tid: String, origin: ShimOrigin, next_index: &mut u32, cap: Option<Instant>) {
        // A target reached twice while it is live - auto-attach and the by-name attach can both
        // deliver the same page - stays one context: the shim itself is idempotent, but a second
        // session on the list would be polled and broadcast to twice. The second session is let go
        // so it does not sit paused.
        if !tid.is_empty() && self.requests.contexts().iter().any(|c| c.target_id == tid) {
            self.resume(&sid);
            return;
        }
        // One deadline for everything this attach asks the context (R4-S10): each call used to have
        // its own, so one context in a busy renderer could hold the session for several of them.
        let deadline = attach_deadline(Instant::now(), self.client.call_budget(), cap);
        let index = context_index_for(&tid, &mut self.index_by_target, next_index);
        if past_ceiling(&self.seen, index) {
            // Counted once per context, said by the caller (rule 4), and released: a context refused
            // the shim runs on the real clock, it does not stand paused for the rest of the session -
            // and when it re-attaches under the same index it is the same refused context, not another.
            if self.refused.insert(index) {
                self.overflow += 1;
            }
            self.resume(&sid);
            return;
        }
        if self.probe_clock && self.reads_session_clock_natively(&sid, origin, deadline) {
            // Covered by the native mechanism already: it has a row of its own under its pid, and a
            // shim here would scale the fake wall twice. Released, and not counted as a context.
            self.native += 1;
            self.resume(&sid);
            return;
        }
        let shim = origin.shim(self.zone);
        let zone = self.zone.map(cdp::zone_id);
        let injected = if cdp::is_worker(&ty) {
            cdp::inject_worker(&mut self.client, &sid, &shim, zone.as_deref(), deadline)
        } else {
            cdp::inject_page(&mut self.client, &sid, &shim, zone.as_deref(), deadline)
        };
        match injected {
            Ok(cdp::Injected { script, children }) => {
                if !children && cdp::starts_workers(&ty) {
                    // The context is on the session clock, but it refused auto-attach, so a worker it
                    // starts runs on the real clock unseen. Counted as one that could not be reached,
                    // so the verdict says some were not rather than that all were (rule 4).
                    self.failed += 1;
                }
                if !self.seen.contains(&index) {
                    self.seen.push(index);
                }
                // The target named its own context type, and that name becomes a coverage key in the
                // report and on the wire. Cleaned here, at the one place a context is built.
                self.requests.push(CdpContext::new(index, sid, cdp::sanitise_target_text(&ty), tid, script));
            }
            Err(_) => {
                // The shim did not take, and the injection stopped before its own resume call: let
                // the context run unshimmed rather than paused, and count it as uncovered.
                self.failed += 1;
                self.resume(&sid);
            }
        }
    }

    /// Ask a context which clock it reads and judge the answer against the session clock projected
    /// to now. One `Runtime.evaluate`, by the attach's deadline - a context that does not answer
    /// counts as real, and gets the shim.
    fn reads_session_clock_natively(&mut self, sid: &str, origin: ShimOrigin, deadline: Instant) -> bool {
        let real_now = now_epoch_ms();
        let fake_now = origin.fake0.saturating_add((real_now - origin.real0).saturating_mul(origin.mult));
        let params = json!({ "expression": CLOCK_PROBE_EXPR, "returnByValue": true });
        let reply = self.client.call_until("Runtime.evaluate", params, Some(sid), deadline).ok();
        let text = reply.as_ref().and_then(|r| r["result"]["value"].as_str());
        classify_clock_read(text, fake_now, real_now) == ClockRead::Native
    }

    /// Release a target that auto-attach paused on start. Not waited for: its answer changes nothing,
    /// a target that is not paused answers the same, and one that is already gone errors harmlessly.
    fn resume(&mut self, sid: &str) {
        let _ = self.client.send("Runtime.runIfWaitingForDebugger", json!({}), Some(sid));
    }

    /// The clock moved - a rate change, a jump, a resync: give every page a new-document hook built on
    /// `origin`, and `expr` to every live document, a page's once its new hook is back - so a page
    /// that loads a new document meanwhile already gets the new clock (R4-W5), a scheduled change
    /// included, which is why a Chromium session schedules its rate changes far enough ahead to cover
    /// both (R4-S17).
    ///
    /// Waits for the new hooks at most [`MOVE_WAIT_MS`] and then returns, all pages or not (R4-S10).
    /// A page that answers later still gets its move. Whether a page missed the move is read from its
    /// answers whenever they come, and handed over at the end ([`Attacher::moves_missed`]).
    pub(crate) fn move_clock(&mut self, expr: &str, origin: ShimOrigin, next_index: &mut u32) {
        let shim = origin.shim(self.zone);
        let hooks = self.requests.start_move(expr, &shim, &mut self.client);
        let deadline = Instant::now() + Duration::from_millis(MOVE_WAIT_MS);
        self.wait_until(deadline, origin, next_index, |r| r.answered(&hooks));
    }

    /// Ask every context for its call counts, without waiting - the answers are merged as they come.
    /// A context still answering the last request is not asked again (R4-S10).
    pub(crate) fn request_counts(&mut self) {
        self.requests.request_counts(&mut self.client);
    }

    /// Let every live context go and ask for its last counts, then wait for both until `deadline`, and
    /// count the contexts that did not confirm they were let go, each once: a page that answered
    /// anything but `ok` or `no-shim`, one that did not answer by the deadline, and one that may still
    /// have a hook - those may still be on the session clock, which the caller has to say (rule 6).
    ///
    /// The hooks go with the release. They die with the connection (measured: a page reloaded after
    /// the session comes up with no shim), but the connection outlives the release by the last look at
    /// the host's tree, and a page that navigated then loaded its next document on the session clock
    /// again and kept it after the session with no warning (R4-W5).
    ///
    /// The zone override goes last, once every answer is in or the deadline has passed. One context
    /// holds it for its whole renderer process, and the protocol does not order commands across
    /// sessions, so a reset sent with the last counts could reach the renderer before the count of
    /// another context, which then read the host's zone and was reported as having missed the
    /// session's (CodeRabbit on #89). A context let go no longer checks its zone ([`cdp::release_expr`]),
    /// and the last counts after this wait for the same deadline (`EmbeddedBridge::finish`), so a count
    /// still in flight when it passed is never read.
    pub(crate) fn release(&mut self, expr: &str, origin: ShimOrigin, next_index: &mut u32, deadline: Instant) -> u32 {
        self.ending = true;
        self.requests.request_counts(&mut self.client);
        self.requests.start_release(expr, &mut self.client);
        self.wait_until(deadline, origin, next_index, |r| r.release_settled() && r.counts_settled());
        // Measured on WebView2 154 (2026-10-05): taking the override away does not give a page the
        // host's zone back. The renderer keeps the last zone it was given, also once the connection
        // closes, so a page that outlives the session goes on reading the session's zone.
        if self.zone.is_some() {
            self.requests.send_zone("", &mut self.client);
        }
        self.requests.unreleased()
    }

    /// The end of the session: ask for the last counts and wait for them until `deadline`, then take
    /// every clock move a live page has not answered as one it missed. A context that attaches from
    /// here on is let go without a shim.
    pub(crate) fn settle(&mut self, origin: ShimOrigin, next_index: &mut u32, deadline: Instant) {
        self.ending = true;
        self.requests.request_counts(&mut self.client);
        self.wait_until(deadline, origin, next_index, Requests::counts_settled);
        self.requests.settle_moves();
    }

    /// How many pages missed at least one clock move so far.
    pub(crate) fn moves_missed(&self) -> usize {
        self.requests.moves_missed()
    }

    /// Evaluate a JS expression in one live context and return the string it produced, if any.
    /// For a probe reading what a page shows - `document.title` - not for the session.
    pub(crate) fn evaluate_string(&mut self, index: u32, expr: &str) -> Option<String> {
        let sid = self.requests.contexts().iter().find(|c| c.index == index)?.session_id.clone();
        let reply = self
            .client
            .call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }), Some(&sid))
            .ok()?;
        reply["result"]["value"].as_str().map(str::to_string)
    }

    /// The live contexts, in attach order.
    pub(crate) fn contexts(&self) -> &[CdpContext] {
        self.requests.contexts()
    }

    /// Every context index this attacher ever shimmed, in attach order.
    pub(crate) fn seen(&self) -> &[u32] {
        &self.seen
    }

    /// The counts, handed over for the end-of-session fold.
    pub(crate) fn into_counts(self) -> BTreeMap<(u32, String), u64> {
        self.requests.into_counts()
    }

    /// How many contexts attached and could not be shimmed, plus the ones shimmed that refused to
    /// attach the workers they start - a worker of theirs would run on the real clock unseen.
    pub(crate) fn failed(&self) -> usize {
        self.failed
    }

    /// How many contexts were refused past [`MAX_CONTEXTS`].
    pub(crate) fn overflow(&self) -> usize {
        self.overflow
    }
}

/// The deadline of one attach: its own budget from `now`, or the caller's deadline when that comes
/// first. The pages that already exist are attached under the deadline of the session's start, and an
/// attach made there that took a fresh budget of its own could double the silence before the first
/// heartbeat - ten seconds to connect, ten more for one busy page (CodeRabbit on #85).
fn attach_deadline(now: Instant, budget: Duration, cap: Option<Instant>) -> Instant {
    let own = now + budget;
    cap.map_or(own, |cap| cap.min(own))
}

/// Whether a context with this index is one too many: the ceiling is on contexts ever seen, so a
/// re-attach of a known context (same index) always gets back in, and only a NEW one past the
/// ceiling is refused.
fn past_ceiling(seen: &[u32], index: u32) -> bool {
    !seen.contains(&index) && seen.len() >= MAX_CONTEXTS
}

/// The targets in a `Target.getTargets` reply worth attaching to: shimmable, named, and not yet
/// known to this attacher. Pure over the reply, so the filter is tested on a made-up browser.
fn new_targets(reply: &serde_json::Value, known: &HashMap<String, u32>) -> Vec<(String, String)> {
    reply["targetInfos"]
        .as_array()
        .map(|targets| {
            targets
                .iter()
                .map(|t| {
                    (
                        t["targetId"].as_str().unwrap_or("").to_string(),
                        t["type"].as_str().unwrap_or("").to_string(),
                    )
                })
                .filter(|(tid, ty)| !tid.is_empty() && cdp::is_shimmable(ty) && !known.contains_key(tid))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured on Chromium: a second page in a renderer process reads the zone override the first
    /// one holds, and goes back to the host's zone when the holder closes. So when a context goes away,
    /// every one left is given the zone again - once, in the turn that saw it go - and the one that
    /// went is not.
    #[test]
    fn a_context_that_goes_away_has_the_zone_given_again_to_the_rest() {
        let mut detach_sent = false;
        let (port, browser) = crate::cdp::fake_browser_holding(move |request| {
            let id = request["id"].clone();
            let session = request["sessionId"].as_str().unwrap_or("");
            match request["method"].as_str().unwrap_or("") {
                "Target.getTargets" => vec![(
                    id,
                    json!({ "targetInfos": [{ "targetId": "T1", "type": "page" }, { "targetId": "T2", "type": "page" }] }),
                )],
                "Target.attachToTarget" => {
                    let target = request["params"]["targetId"].as_str().unwrap_or("");
                    vec![(id, json!({ "sessionId": target.replace('T', "S") }))]
                }
                "Page.addScriptToEvaluateOnNewDocument" => vec![(id, json!({ "identifier": "h" }))],
                "Runtime.evaluate" => vec![(id, json!({ "result": {} }))],
                // The second page is in: the first one closes.
                "Runtime.runIfWaitingForDebugger" if session == "S2" && !detach_sent => {
                    detach_sent = true;
                    let gone = json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "S1", "targetId": "T1" } });
                    vec![(id, json!({})), (serde_json::Value::Null, gone)]
                }
                _ => vec![(id, json!({}))],
            }
        });
        let ws = crate::cdp::WsClient::connect("127.0.0.1", port, "/", Instant::now() + Duration::from_secs(5)).unwrap();
        let mut attacher = Attacher::over(cdp::CdpClient::from_ws(ws), port);
        attacher.set_budgets(Duration::from_millis(20), Duration::from_secs(2));
        attacher.set_zone(-330);
        let origin = ShimOrigin { fake0: 0, real0: 0, mult: 1, dur: 1, scheduled: None };
        let mut next = 0;
        assert_eq!(attacher.attach_existing(origin, &mut next, Instant::now() + Duration::from_secs(2)).unwrap(), 2);
        let detached = (0..50).any(|_| attacher.pump(origin, &mut next) == Pumped::Detached);
        assert!(detached, "the fake browser said the first page went away");
        attacher.pump(origin, &mut next);
        drop(attacher);
        let log = browser.join().unwrap();
        let zone_to = |session: &str| {
            log.iter()
                .filter(|r| r["method"] == "Emulation.setTimezoneOverride" && r["sessionId"] == session)
                .inspect(|r| assert_eq!(r["params"]["timezoneId"], "GMT+05:30"))
                .count()
        };
        assert_eq!(zone_to("S1"), 1, "the page that went away is not asked again");
        assert_eq!(zone_to("S2"), 2, "the one left is given the zone again, once");
    }

    /// CodeRabbit on #89: two pages share one renderer and its zone override, and the protocol does not
    /// order commands across sessions. Here the second page's renderer is busy and runs its last count
    /// only when its release arrives, reading the host's zone if the override was taken away before.
    /// The release takes the zone away after the answers, so the count reads the session's zone, and
    /// the override is still taken away from both.
    #[test]
    fn the_release_takes_the_zone_away_only_after_the_last_counts() {
        let mut reset = false;
        let mut held: Option<serde_json::Value> = None;
        let (port, browser) = crate::cdp::fake_browser_holding(move |request| {
            let id = request["id"].clone();
            let session = request["sessionId"].as_str().unwrap_or("");
            let expr = request["params"]["expression"].as_str().unwrap_or("");
            let count = |reset: bool| json!({ "result": { "value": { "now": 1, "zone": u8::from(reset) } } });
            match request["method"].as_str().unwrap_or("") {
                "Target.getTargets" => vec![(
                    id,
                    json!({ "targetInfos": [{ "targetId": "T1", "type": "page" }, { "targetId": "T2", "type": "page" }] }),
                )],
                "Target.attachToTarget" => {
                    let target = request["params"]["targetId"].as_str().unwrap_or("");
                    vec![(id, json!({ "sessionId": target.replace('T', "S") }))]
                }
                "Page.addScriptToEvaluateOnNewDocument" => vec![(id, json!({ "identifier": "h" }))],
                "Emulation.setTimezoneOverride" => {
                    reset |= request["params"]["timezoneId"] == "";
                    vec![(id, json!({}))]
                }
                "Runtime.evaluate" if expr == cdp::COUNTS_EXPR && session == "S2" => {
                    held = Some(id);
                    Vec::new()
                }
                "Runtime.evaluate" if expr == cdp::COUNTS_EXPR => vec![(id, count(reset))],
                "Runtime.evaluate" if expr == cdp::release_expr() => {
                    let mut out = Vec::new();
                    if session == "S2"
                        && let Some(count_id) = held.take()
                    {
                        out.push((count_id, count(reset)));
                    }
                    out.push((id, json!({ "result": { "value": "ok" } })));
                    out
                }
                "Runtime.evaluate" => vec![(id, json!({ "result": {} }))],
                _ => vec![(id, json!({}))],
            }
        });
        let ws = crate::cdp::WsClient::connect("127.0.0.1", port, "/", Instant::now() + Duration::from_secs(5)).unwrap();
        let mut attacher = Attacher::over(cdp::CdpClient::from_ws(ws), port);
        attacher.set_budgets(Duration::from_millis(20), Duration::from_secs(2));
        attacher.set_zone(-330);
        let origin = ShimOrigin { fake0: 0, real0: 0, mult: 1, dur: 1, scheduled: None };
        let mut next = 0;
        assert_eq!(attacher.attach_existing(origin, &mut next, Instant::now() + Duration::from_secs(2)).unwrap(), 2);
        let unreleased = attacher.release(&cdp::release_expr(), origin, &mut next, Instant::now() + Duration::from_secs(2));
        assert_eq!(unreleased, 0, "both pages confirmed they were let go");
        assert_eq!(attacher.zone_missed(), 0, "the busy page's last count read the session's zone");
        drop(attacher);
        let log = browser.join().unwrap();
        let resets: Vec<usize> = (0..log.len())
            .filter(|&i| log[i]["method"] == "Emulation.setTimezoneOverride" && log[i]["params"]["timezoneId"] == "")
            .collect();
        assert_eq!(resets.len(), 2, "the override is taken away from both pages");
        let last_release = log.iter().rposition(|r| r["params"]["expression"] == cdp::release_expr());
        assert!(last_release.is_some_and(|r| resets.iter().all(|&i| i > r)), "the zone goes after the releases");
    }

    /// R4-S14: a turn takes everything that is already here, not one message - and reports what
    /// mattered most in it, so an attach in the middle of a burst of other events is not lost.
    #[test]
    fn a_pump_turn_takes_the_whole_burst_and_reports_its_strongest_outcome() {
        let mut burst = vec![Pumped::Idle, Pumped::Attached, Pumped::Detached, Pumped::Idle].into_iter();
        let mut taken = 0;
        let turn = drain_turn(Pumped::Idle, || true, || {
            let next = burst.next();
            taken += usize::from(next.is_some());
            next
        });
        assert_eq!(turn, Pumped::Attached);
        assert_eq!(taken, 4, "every message already here was handled in the turn");

        // A closed connection ends the turn at once and is what the turn reports.
        let mut after_close = 0;
        assert_eq!(drain_turn(Pumped::Closed, || true, || { after_close += 1; Some(Pumped::Attached) }), Pumped::Closed);
        assert_eq!(after_close, 0, "nothing is read after the connection closed");

        // A target that never stops talking cannot keep the turn going.
        let mut endless = 0;
        assert_eq!(drain_turn(Pumped::Idle, || true, || { endless += 1; Some(Pumped::Idle) }), Pumped::Idle);
        assert_eq!(endless, PUMP_DRAIN_MAX);
    }

    /// An attach under the caller's deadline ends by it, and one with no deadline of the caller's -
    /// or a later one - keeps its own budget.
    #[test]
    fn an_attach_ends_by_the_callers_deadline_when_that_comes_first() {
        let now = Instant::now();
        let budget = Duration::from_secs(10);
        let soon = now + Duration::from_secs(1);
        assert_eq!(attach_deadline(now, budget, Some(soon)), soon);
        assert_eq!(attach_deadline(now, budget, None), now + budget);
        assert_eq!(attach_deadline(now, budget, Some(now + Duration::from_secs(30))), now + budget);
    }

    /// R4-S10: a turn that has run out of its time stops taking messages, however many are here - a
    /// burst of attaches cannot hold the loop from its heartbeat. The rest waits for the next turn.
    #[test]
    fn a_pump_turn_stops_when_its_time_is_spent() {
        let mut budget = 3;
        let mut taken = 0;
        let turn = drain_turn(
            Pumped::Idle,
            || {
                budget -= 1;
                budget >= 0
            },
            || {
                taken += 1;
                Some(Pumped::Attached)
            },
        );
        assert_eq!(turn, Pumped::Attached);
        assert_eq!(taken, 3, "three messages fitted in the turn, the fourth waits");
    }

    /// The probe decides whether a page inside a natively hooked application gets the shim. Our own
    /// marker means a page that reloaded - it is ours, and it is tracked again. A reading nearer the
    /// session clock than the real one means the native mechanism covers it already, and a shim on
    /// top would scale the fake wall twice - so it is left alone. Anything else is real and gets the
    /// shim: no reply, a reply that is not a number, or a reading nearer the real clock.
    #[test]
    fn the_clock_probe_tells_ours_from_native_from_real() {
        let fake = 1_900_000_000_000;
        let real = 1_700_000_000_000;
        assert_eq!(classify_clock_read(Some("shim"), fake, real), ClockRead::Shim);
        assert_eq!(classify_clock_read(Some("1900000000500"), fake, real), ClockRead::Native);
        assert_eq!(classify_clock_read(Some("1700000000500"), fake, real), ClockRead::Real);
        assert_eq!(classify_clock_read(None, fake, real), ClockRead::Real, "no evidence is no hook");
        assert_eq!(classify_clock_read(Some("undefined"), fake, real), ClockRead::Real);
        // A reading equally far from both only happens with the session clock within milliseconds
        // of the real one, where a second substitution is the identity - so it gets the shim.
        assert_eq!(classify_clock_read(Some("1800000000000"), fake, real), ClockRead::Real);
    }

    /// The reading is the page's own `Date.now`, and a page can make that anything. At the ends of
    /// `i64` the old subtraction overflowed and panicked the core (R4-N23). Both ends, and a session
    /// clock before 1970, so the sign of neither distance is assumed.
    #[test]
    fn a_page_clock_at_the_ends_of_the_range_is_judged_rather_than_a_crash() {
        let fake = 1_900_000_000_000;
        let real = 1_700_000_000_000;
        let min = i64::MIN.to_string();
        let max = i64::MAX.to_string();
        assert_eq!(classify_clock_read(Some(&min), fake, real), ClockRead::Real);
        assert_eq!(classify_clock_read(Some(&max), fake, real), ClockRead::Native);
        assert_eq!(classify_clock_read(Some(&max), -fake, real), ClockRead::Real);
        assert_eq!(classify_clock_read(Some(&min), -fake, real), ClockRead::Native);
    }

    /// The ceiling is on contexts ever seen, and a re-attach of a known context never counts against
    /// it - the one decision the live path cannot exercise without 257 pages.
    #[test]
    fn the_ceiling_refuses_only_a_new_context_and_lets_a_known_one_back_in() {
        let full: Vec<u32> = (1..=MAX_CONTEXTS as u32).collect();
        assert!(past_ceiling(&full, MAX_CONTEXTS as u32 + 1));
        assert!(!past_ceiling(&full, 42));
        assert!(!past_ceiling(&full[..MAX_CONTEXTS - 1], MAX_CONTEXTS as u32 + 1));
        assert!(!past_ceiling(&[], 1));
    }

    /// Existing targets: pages and workers are wanted, a browser target and a target already
    /// indexed are not, and a target without an id is nothing to attach to by name.
    #[test]
    fn existing_targets_are_filtered_to_the_shimmable_unknown_named_ones() {
        let reply = json!({ "targetInfos": [
            { "targetId": "T-page", "type": "page" },
            { "targetId": "T-browser", "type": "browser" },
            { "targetId": "T-known", "type": "page" },
            { "targetId": "", "type": "page" },
            { "targetId": "T-worker", "type": "worker" },
        ]});
        let mut known = HashMap::new();
        known.insert("T-known".to_string(), 7);
        let wanted = new_targets(&reply, &known);
        assert_eq!(wanted, vec![("T-page".to_string(), "page".to_string()), ("T-worker".to_string(), "worker".to_string())]);
        assert!(new_targets(&json!({}), &known).is_empty());
    }
}
