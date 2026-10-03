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

use serde_json::json;

use crate::cdp;
use crate::cdp_audit::context_index_for;
use crate::zone::{now_epoch_ms, WALL_MAX_MS};

/// One shimmed JS context of a Chromium target: the coverage unit of a CDP session (rule 4 - never
/// summed across contexts).
pub(crate) struct CdpContext {
    pub(crate) index: u32,
    pub(crate) session_id: String,
    pub(crate) ty: String,
    /// The CDP targetId, kept because `Target.targetDestroyed` names a target, not a session.
    pub(crate) target_id: String,
    /// A page's new-document hook, replaced whenever the clock moves (R4-W5). `None` for a worker,
    /// which has no hook - one started later is a new target and is shimmed from the clock of then.
    pub(crate) script: Option<String>,
}

/// The clock origin a shim is built from: fake start and real start (both Unix-epoch ms), the wall
/// rate and the duration rate. A Chromium session runs both rates at the multiplier. A page inside a
/// natively hooked application follows that application's duration axis, which scales only under
/// `scale_duration` (docs/09 section 12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShimOrigin {
    pub(crate) fake0: i64,
    pub(crate) real0: i64,
    pub(crate) mult: i64,
    pub(crate) dur: i64,
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
/// the end of what is here, or at [`PUMP_DRAIN_MAX`], and reports the strongest outcome of the turn.
fn drain_turn(first: Pumped, mut next_ready: impl FnMut() -> Option<Pumped>) -> Pumped {
    let mut turn = first;
    for _ in 0..PUMP_DRAIN_MAX {
        if turn == Pumped::Closed {
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
    /// Who we still TALK to - polling or broadcasting to a dead session costs the full read
    /// deadline inside the caller's loop.
    contexts: Vec<CdpContext>,
    /// Who this attacher ever COVERED, in attach order, append-only: the audit is a record of what
    /// happened, not of what is still open, so a context that reloaded or closed keeps its evidence
    /// (R2-W1). Once per CONTEXT rather than once per attach.
    seen: Vec<u32>,
    counts: BTreeMap<(u32, String), u64>,
    failed: usize,
    overflow: usize,
    /// targetId -> context index, so a re-attached context keeps the identity it already had.
    index_by_target: HashMap<String, u32>,
}

impl Attacher {
    /// Connect to the browser endpoint on a loopback host and port and arm auto-attach, so every
    /// page and worker the browser creates from now on arrives as an `attachedToTarget` event,
    /// paused until the shim is in. The pages that ALREADY exist are the caller's next call. The
    /// host is `127.0.0.1` or `::1` - whichever family the listener was found on.
    pub(crate) fn connect(host: &str, port: u16) -> io::Result<Attacher> {
        let mut client = cdp::CdpClient::connect_to_port(host, port)?;
        client.call(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
            None,
        )?;
        Ok(Attacher {
            client,
            port,
            probe_clock: false,
            native: 0,
            contexts: Vec::new(),
            seen: Vec::new(),
            counts: BTreeMap::new(),
            failed: 0,
            overflow: 0,
            refused: HashSet::new(),
            index_by_target: HashMap::new(),
        })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Shorten the client's poll interval and call deadline - see `CdpClient::set_budgets`. For an
    /// attacher driven from a loop that has its own cadence to keep.
    pub(crate) fn set_budgets(&mut self, poll: std::time::Duration, call: std::time::Duration) -> io::Result<()> {
        self.client.set_budgets(poll, call)
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
        AttacherOutcome { seen: self.seen, counts: self.counts, failed: self.failed, overflow: self.overflow }
    }

    /// Attach to every shimmable target the browser already has - the pages an embedded engine
    /// opened before the session found its port. Whether auto-attach reaches existing targets is a
    /// question this tool has never measured (docs/09 section 2), so this asks for them by name and
    /// stays idempotent: a target the index map already knows is skipped, whichever road it came by.
    /// Returns how many were newly shimmed.
    pub(crate) fn attach_existing(&mut self, origin: ShimOrigin, next_index: &mut u32) -> io::Result<usize> {
        let reply = self.client.call("Target.getTargets", json!({}), None)?;
        let mut attached = 0;
        for (tid, ty) in new_targets(&reply, &self.index_by_target) {
            let Ok(reply) = self.client.call("Target.attachToTarget", json!({ "targetId": tid, "flatten": true }), None)
            else {
                continue;
            };
            let sid = reply["sessionId"].as_str().unwrap_or("").to_string();
            if sid.is_empty() {
                continue;
            }
            self.shim(sid, ty, tid, origin, next_index);
            attached += 1;
        }
        Ok(attached)
    }

    /// One turn: poll the connection (bounded by the client's poll interval), act on what came, then
    /// on everything else that is already here, without waiting for more (R4-S14).
    pub(crate) fn pump(&mut self, origin: ShimOrigin, next_index: &mut u32) -> Pumped {
        let polled = self.client.poll();
        let first = self.handle(polled, origin, next_index);
        drain_turn(first, || match self.client.poll_ready() {
            Ok(None) => None,
            ready => Some(self.handle(ready, origin, next_index)),
        })
    }

    /// Act on one polled message.
    fn handle(&mut self, polled: io::Result<Option<cdp::Msg>>, origin: ShimOrigin, next_index: &mut u32) -> Pumped {
        match polled {
            Ok(Some(cdp::Msg::Event { method, params, .. })) if method == "Target.attachedToTarget" => {
                let sid = params["sessionId"].as_str().unwrap_or("").to_string();
                let ty = params["targetInfo"]["type"].as_str().unwrap_or("").to_string();
                let tid = params["targetInfo"]["targetId"].as_str().unwrap_or("").to_string();
                if sid.is_empty() {
                    return Pumped::Idle;
                }
                if !cdp::is_shimmable(&ty) {
                    // Auto-attach paused it on start, like everything it delivers. A target with no
                    // timer of ours to cover is let go at once - left as it arrived, it would stay
                    // paused for as long as the session ran.
                    self.resume(&sid);
                    return Pumped::Idle;
                }
                self.shim(sid, ty, tid, origin, next_index);
                Pumped::Attached
            }
            // A context that went away - a reload, a closed window, a recycled worker. Dropped from
            // the live list only: its counts stay in `counts` and its index in `seen`.
            Ok(Some(cdp::Msg::Event { method, params, .. }))
                if method == "Target.detachedFromTarget" || method == "Target.targetDestroyed" =>
            {
                let sid = params["sessionId"].as_str().unwrap_or("");
                let tid = params["targetId"].as_str().unwrap_or("");
                let before = self.contexts.len();
                self.contexts.retain(|c| {
                    let gone = (!sid.is_empty() && c.session_id == sid) || (!tid.is_empty() && c.target_id == tid);
                    !gone
                });
                if self.contexts.len() < before { Pumped::Detached } else { Pumped::Idle }
            }
            Ok(_) => Pumped::Idle,
            Err(_) => Pumped::Closed,
        }
    }

    /// Give a newly attached context its index and its shim. The index is keyed by the CDP targetId,
    /// which Chromium keeps across re-attaches (R3-7). The shim is built from the clock's CURRENT
    /// origin, never the session's initial one.
    fn shim(&mut self, sid: String, ty: String, tid: String, origin: ShimOrigin, next_index: &mut u32) {
        // A target reached twice while it is live - auto-attach and the by-name attach can both
        // deliver the same page - stays one context: the shim itself is idempotent, but a second
        // session on the list would be polled and broadcast to twice. The second session is let go
        // so it does not sit paused.
        if !tid.is_empty() && self.contexts.iter().any(|c| c.target_id == tid) {
            self.resume(&sid);
            return;
        }
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
        if self.probe_clock && self.reads_session_clock_natively(&sid, origin) {
            // Covered by the native mechanism already: it has a row of its own under its pid, and a
            // shim here would scale the fake wall twice. Released, and not counted as a context.
            self.native += 1;
            self.resume(&sid);
            return;
        }
        let shim = cdp::build_shim(origin.fake0, origin.real0, origin.mult, origin.dur, WALL_MAX_MS);
        let injected = if cdp::is_worker(&ty) {
            cdp::inject_worker(&mut self.client, &sid, &shim)
        } else {
            cdp::inject_page(&mut self.client, &sid, &shim)
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
                self.contexts.push(CdpContext {
                    index,
                    session_id: sid,
                    // The target named its own context type, and that name becomes a coverage key
                    // in the report and on the wire. Cleaned here, at the one place a context is
                    // built.
                    ty: cdp::sanitise_target_text(&ty),
                    target_id: tid,
                    script,
                });
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
    /// to now. One `Runtime.evaluate`, bounded by the client's call deadline - a context that does
    /// not answer counts as real, and gets the shim.
    fn reads_session_clock_natively(&mut self, sid: &str, origin: ShimOrigin) -> bool {
        let real_now = now_epoch_ms();
        let fake_now = origin.fake0.saturating_add((real_now - origin.real0).saturating_mul(origin.mult));
        let reply = self
            .client
            .call("Runtime.evaluate", json!({ "expression": CLOCK_PROBE_EXPR, "returnByValue": true }), Some(sid))
            .ok();
        let text = reply.as_ref().and_then(|r| r["result"]["value"].as_str());
        classify_clock_read(text, fake_now, real_now) == ClockRead::Native
    }

    /// Release a target that auto-attach paused on start. Best effort: a target that is not paused
    /// answers the same, and one that is already gone errors harmlessly.
    fn resume(&mut self, sid: &str) {
        let _ = self.client.call("Runtime.runIfWaitingForDebugger", json!({}), Some(sid));
    }

    /// The clock moved - a rate change, a jump, a resync: give every page a new-document hook built
    /// on `origin`, then push `expr` to every live document. The hook first, so a page that loads a
    /// new document meanwhile already gets the new clock (R4-W5).
    pub(crate) fn move_clock(&mut self, expr: &str, origin: ShimOrigin) {
        let shim = cdp::build_shim(origin.fake0, origin.real0, origin.mult, origin.dur, WALL_MAX_MS);
        let client = &mut self.client;
        for ctx in self.contexts.iter_mut().filter(|c| c.script.is_some()) {
            if let Some(renewed) = cdp::renew_page_script(client, &ctx.session_id, ctx.script.as_deref(), &shim) {
                ctx.script = Some(renewed);
            }
        }
        self.broadcast(expr);
    }

    /// Evaluate a JS expression in every live context (best-effort: a context that just closed
    /// errors and is skipped, so an in-flight update stays honest for the rest).
    pub(crate) fn broadcast(&mut self, expr: &str) {
        for ctx in &self.contexts {
            let _ = self.client.call(
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true }),
                Some(&ctx.session_id),
            );
        }
    }

    /// Evaluate the release expression in every live context and count the ones that did not confirm
    /// it. `ok` is a context let go, `no-shim` one that was never on the shim and has nothing to let go
    /// of. Anything else - an error, no answer - is a page that may still be on the session clock,
    /// which the caller has to say (rule 6).
    pub(crate) fn release(&mut self, expr: &str) -> u32 {
        // The hooks go first. They die with the connection (measured: a page reloaded after the
        // session comes up with no shim), but the connection outlives the release by the last look
        // at the host's tree, and a page that navigated then loaded its next document on the session
        // clock again and kept it after the session with no warning (R4-W5). A page that loads one
        // meanwhile has no shim, and answers `no-shim` below.
        let client = &mut self.client;
        for ctx in self.contexts.iter_mut() {
            if let Some(script) = ctx.script.take() {
                cdp::remove_page_script(client, &ctx.session_id, &script);
            }
        }
        let mut unconfirmed = 0;
        for ctx in &self.contexts {
            let reply = self.client.call(
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true }),
                Some(&ctx.session_id),
            );
            let confirmed = reply
                .ok()
                .and_then(|r| r["result"]["value"].as_str().map(|v| v == "ok" || v == "no-shim"))
                .unwrap_or(false);
            if !confirmed {
                unconfirmed += 1;
            }
        }
        unconfirmed
    }

    /// Evaluate a JS expression in one live context and return the string it produced, if any.
    /// For a probe reading what a page shows - `document.title` - not for the session.
    pub(crate) fn evaluate_string(&mut self, index: u32, expr: &str) -> Option<String> {
        let sid = self.contexts.iter().find(|c| c.index == index)?.session_id.clone();
        let reply = self
            .client
            .call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }), Some(&sid))
            .ok()?;
        reply["result"]["value"].as_str().map(str::to_string)
    }

    /// Read each live context's per-API call counts and merge them (by max, so a peak survives a
    /// reload) into the counts, keyed by `(context index, "type api")`. Returns whether any context
    /// answered - a dead context simply errors and is skipped, so the audit stays honest.
    pub(crate) fn poll_counts(&mut self) -> bool {
        let mut any = false;
        for c in &self.contexts {
            let read = self.client.call(
                "Runtime.evaluate",
                json!({ "expression": cdp::COUNTS_EXPR, "returnByValue": true }),
                Some(&c.session_id),
            );
            if let Ok(v) = read
                && let Some(obj) = v.get("result").and_then(|x| x.get("value")).and_then(serde_json::Value::as_object)
            {
                any = true;
                for (api, key) in cdp::COUNTED_APIS {
                    if let Some(n) = obj.get(key).and_then(serde_json::Value::as_u64) {
                        let entry = self.counts.entry((c.index, format!("{} {}", c.ty, api))).or_insert(0);
                        *entry = (*entry).max(n);
                    }
                }
            }
        }
        any
    }

    /// The live contexts, in attach order.
    pub(crate) fn contexts(&self) -> &[CdpContext] {
        &self.contexts
    }

    /// Every context index this attacher ever shimmed, in attach order.
    pub(crate) fn seen(&self) -> &[u32] {
        &self.seen
    }

    /// The counts, handed over for the end-of-session fold.
    pub(crate) fn into_counts(self) -> BTreeMap<(u32, String), u64> {
        self.counts
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

    /// R4-S14: a turn takes everything that is already here, not one message - and reports what
    /// mattered most in it, so an attach in the middle of a burst of other events is not lost.
    #[test]
    fn a_pump_turn_takes_the_whole_burst_and_reports_its_strongest_outcome() {
        let mut burst = vec![Pumped::Idle, Pumped::Attached, Pumped::Detached, Pumped::Idle].into_iter();
        let mut taken = 0;
        let turn = drain_turn(Pumped::Idle, || {
            let next = burst.next();
            taken += usize::from(next.is_some());
            next
        });
        assert_eq!(turn, Pumped::Attached);
        assert_eq!(taken, 4, "every message already here was handled in the turn");

        // A closed connection ends the turn at once and is what the turn reports.
        let mut after_close = 0;
        assert_eq!(drain_turn(Pumped::Closed, || { after_close += 1; Some(Pumped::Attached) }), Pumped::Closed);
        assert_eq!(after_close, 0, "nothing is read after the connection closed");

        // A target that never stops talking cannot keep the turn going.
        let mut endless = 0;
        assert_eq!(drain_turn(Pumped::Idle, || { endless += 1; Some(Pumped::Idle) }), Pumped::Idle);
        assert_eq!(endless, PUMP_DRAIN_MAX);
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
