//! The contexts one attacher drives, and every request in flight to them (R4-S10, ADR-20).
//!
//! The attacher used to ask each context in turn and wait for each answer, so a context busy with its
//! own JS held the session loop for a whole call deadline - ten seconds in a Chromium session, two
//! beside a native one, once a second, for every busy context (measured: a page and two workers
//! busy, and the driver stopped the core after fifteen silent seconds). Here a request is sent and
//! written down, and its answer is acted on whenever it comes. A deadline bounds how long the
//! caller waits, never what is learned: a hook identifier that comes late is still kept, a count
//! that comes late still counts, and a request with no answer by the end is a fact for the report.
//!
//! Pure over the sends: [`Outbox`] is the one thing it needs from a connection, so every rule here is
//! tested without a browser.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::cdp;

/// One shimmed JS context of a Chromium target: the coverage unit of a CDP session (rule 4 - never
/// summed across contexts).
pub(crate) struct CdpContext {
    pub(crate) index: u32,
    pub(crate) session_id: String,
    pub(crate) ty: String,
    /// The CDP targetId, kept because `Target.targetDestroyed` names a target, not a session.
    pub(crate) target_id: String,
    /// A page's new-document hooks, replaced whenever the clock moves (R4-W5). None for a worker,
    /// which has no hook - one started later is a new target and is shimmed from the clock of then.
    pub(crate) hooks: PageHooks,
    /// A count request is in flight. A context that has not answered the last one is not asked again
    /// until it does - a busy context does not pile up requests (R4-S10).
    counting: bool,
    /// Let go of at the end of the session: from here on it gets no clock move, and a hook that comes
    /// back is removed at once.
    released: bool,
    /// How it answered the release, once it has.
    release_answer: Option<bool>,
    /// What it showed for the zone after the session gave this machine's back: `getTimezoneOffset()` at
    /// the two instants of the read ([`cdp::zone_read_expr`]), once it answered with two numbers.
    zone_read: Option<[i32; 2]>,
    /// One past the ceiling of own rows (R5-P20): its counts go to the pool's row, not a row of its own.
    pooled: bool,
    /// A page whose shim is its new-document hook's to give, with no document to run in when it was
    /// reached (R5-F11): not covered until an audit answers from the shim (R5-D46). Until then a move or
    /// a release it cannot take is no miss - there is no document to take it.
    awaiting: bool,
    /// How many frames the page's own frame tree had in its process at the last audit, the page's own
    /// document left out - read before the audit, which says how many of them the shim reached (R5-P5).
    local_frames: Option<u64>,
    /// How many audits in a row found fewer frames reached than the tree had (R5-D47).
    frames_short: u8,
}

impl CdpContext {
    pub(crate) fn new(index: u32, session_id: String, ty: String, target_id: String, script: Option<String>) -> CdpContext {
        CdpContext {
            index,
            session_id,
            ty,
            target_id,
            hooks: PageHooks { current: script, unremoved: Vec::new() },
            counting: false,
            released: false,
            release_answer: None,
            zone_read: None,
            pooled: false,
            awaiting: false,
            local_frames: None,
            frames_short: 0,
        }
    }

    /// The same context, waiting for its first audit to show the shim (R5-F11).
    pub(crate) fn awaiting(self, awaiting: bool) -> CdpContext {
        CdpContext { awaiting, ..self }
    }
}

/// How many audits in a row have to find a frame of a page out of the shim's reach before the page is
/// said to have one (R5-D47): the frame tree and the audit are two questions, and a frame that comes or
/// goes between them is short for one audit, not for two.
const FRAMES_SHORT_AUDITS: u8 = 2;

/// Where a context let go of stands with its zone, judged from what it read after the session gave it
/// this machine's zone back (ADR-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZoneBack {
    /// It no longer reads the session's zone.
    Back,
    /// It still reads the session's fixed offset at both instants, where this machine's zone has
    /// another offset at one of them at least.
    Kept,
    /// It did not answer with two numbers, or it read the session's offset at both instants where this
    /// machine's offsets are not known - nothing to vouch for it by.
    Unanswered,
}

/// How the contexts let go of stand with their zone, counted by [`ZoneBack`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ZoneTally {
    pub(crate) back: usize,
    pub(crate) kept: usize,
    pub(crate) unanswered: usize,
}

impl ZoneTally {
    pub(crate) fn add(&mut self, other: ZoneTally) {
        self.back += other.back;
        self.kept += other.kept;
        self.unanswered += other.unanswered;
    }

    /// Whether some context cannot be said to be back on this machine's zone.
    pub(crate) fn any_not_back(&self) -> bool {
        self.kept + self.unanswered > 0
    }
}

/// Judge one context's zone after the release: `read` is what it showed at the two instants, `session`
/// the session's fixed offset (the same at both), `machine` this machine's offsets at them.
///
/// Kept is the session's offset at BOTH instants where the machine has another at one of them: anything
/// else means the session's zone has gone from the page. A reading that differs from the machine's too is
/// still back - an engine with older zone rules than the system's (a bundled Chromium) shows other
/// offsets for some zones with or without a session, and blaming the session for that would mislead. When
/// the session's offset is the machine's at both instants, the two cannot be told apart, and the local
/// times of this year are the same either way. Pure, so every case is tested without a page.
pub(crate) fn zone_back(read: Option<[i32; 2]>, session: i32, machine: Option<[i32; 2]>) -> ZoneBack {
    let on_session = [session, session];
    match read {
        None => ZoneBack::Unanswered,
        Some(read) if read != on_session => ZoneBack::Back,
        Some(_) => match machine {
            Some(machine) if machine == on_session => ZoneBack::Back,
            Some(_) => ZoneBack::Kept,
            None => ZoneBack::Unanswered,
        },
    }
}

/// The two offsets a zone read answered, `None` for anything but two numbers a zone can have.
fn read_offsets(reply: &Result<Value, ()>) -> Option<[i32; 2]> {
    let values = reply.as_ref().ok()?["result"]["value"].as_array()?;
    let [first, second] = values.as_slice() else {
        return None;
    };
    let minutes = |v: &Value| {
        v.as_f64()
            .filter(|m| m.is_finite() && m.abs() <= 24.0 * 60.0)
            .and_then(|m| i32::try_from(m.round() as i64).ok())
    };
    Some([minutes(first)?, minutes(second)?])
}

/// The new-document hooks of one page: the one that carries the clock now, and earlier ones whose
/// removal the page did not confirm. Those are still there as far as anyone knows, so every later
/// move and the release try again - forgotten, one would put a document loaded after the release
/// back on a session clock.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PageHooks {
    current: Option<String>,
    unremoved: Vec<String>,
}

impl PageHooks {
    /// No hook at all - what a page let go of has, once every removal is confirmed.
    const NONE: PageHooks = PageHooks { current: None, unremoved: Vec::new() };
}

/// What a connection does for the requests: send one and say its id, `None` when it could not be sent.
pub(crate) trait Outbox {
    fn ask(&mut self, method: &str, params: Value, session: &str) -> Option<u64>;
}

impl Outbox for cdp::CdpClient {
    fn ask(&mut self, method: &str, params: Value, session: &str) -> Option<u64> {
        self.send(method, params, Some(session)).ok()
    }
}

/// A request in flight, by what it was for.
#[derive(Debug)]
enum Asked {
    Counts { session: String, index: u32, ty: String },
    /// A page's frame tree, asked just before its counts (R5-P5).
    Frames { session: String },
    /// A new hook for a move. The move's expression goes to the page when the hook comes back, so the
    /// page has the new hook before its live document moves (R4-W5) - however long it takes.
    Hook { session: String, expr: Arc<str> },
    Unhook { session: String, script: String },
    Move { session: String },
    Release { session: String },
    /// This machine's zone given back at the release. The answer proves nothing about what the context
    /// reads, but once every one is in, every renderer process has taken the zone from its holder.
    Zone { session: String },
    /// What the context shows for the zone after that.
    ZoneRead { session: String },
}

impl Asked {
    fn session(&self) -> &str {
        match self {
            Asked::Counts { session, .. }
            | Asked::Frames { session }
            | Asked::Hook { session, .. }
            | Asked::Unhook { session, .. }
            | Asked::Move { session, .. }
            | Asked::Release { session }
            | Asked::Zone { session }
            | Asked::ZoneRead { session } => session,
        }
    }
}

const ADD_HOOK: &str = "Page.addScriptToEvaluateOnNewDocument";
const REMOVE_HOOK: &str = "Page.removeScriptToEvaluateOnNewDocument";
const EVALUATE: &str = "Runtime.evaluate";
const SET_ZONE: &str = "Emulation.setTimezoneOverride";
const FRAME_TREE: &str = "Page.getFrameTree";

/// The live contexts of one attacher, the requests in flight to them, and what their answers said.
#[derive(Default)]
pub(crate) struct Requests {
    contexts: Vec<CdpContext>,
    asked: HashMap<u64, Asked>,
    /// Per-context call counts, keyed by `(context index, "type api")`, merged by max so a peak
    /// survives a reload.
    counts: BTreeMap<(u32, String), u64>,
    /// What the pooled contexts that went away counted, summed by `"type api"` (R5-P20). A pooled
    /// context's own peaks move here when it goes, so the table is as large as the live list, however
    /// many workers an application starts and ends.
    pool_sum: BTreeMap<String, u64>,
    /// The pages that missed a clock move: one that did not take its new hook, one that answered the
    /// move with anything but `ok` or `no-shim` (`late` from R4-S17), and one that had not answered by
    /// the end. A page once, however many moves it missed - the warning is about pages, and the set
    /// stays as small as the list of contexts (CodeRabbit on #85).
    missed: BTreeSet<String>,
    /// The contexts, by index, whose shim once read a zone other than the session's. Kept like the
    /// counts: a context that closed keeps its evidence.
    zone_missed: BTreeSet<u32>,
    /// The pages, by index, that were hidden at some point while their timers ran faster (R4-N28).
    hidden_fast: BTreeSet<u32>,
    /// The contexts, by index, where a timer had to run more often than the engine's 4 ms floor at the
    /// session's speed (R5-P6).
    capped: BTreeSet<u32>,
    /// The pages, by index, that had a frame in their process the shim did not reach, audit after audit
    /// (R5-P5, R5-D47).
    frames_unreached: BTreeSet<u32>,
    /// The contexts whose audit showed the shim the new-document hook gave them since the caller last
    /// asked ([`Requests::take_confirmed`], R5-F11).
    confirmed: Vec<u32>,
    /// The zone read of the release ([`cdp::zone_read_expr`]), from the moment the zone is given back
    /// until it is sent - when the last context answered the zone, or when the caller waits no longer.
    zone_read: Option<String>,
}

impl Requests {
    pub(crate) fn push(&mut self, context: CdpContext) {
        self.contexts.push(context);
    }

    /// The live contexts, in attach order.
    pub(crate) fn contexts(&self) -> &[CdpContext] {
        &self.contexts
    }

    /// Drop a context that went away - a closed window, a recycled worker - with its requests in
    /// flight. They are not counted as missed: a page that is gone has no clock to stand apart. A
    /// reload does not detach its target, so it is not this. A pooled context's counts move to the
    /// pool's sum. Returns the target ids of what was dropped, empty when nothing was.
    pub(crate) fn forget(&mut self, session: &str, target: &str) -> Vec<String> {
        let is_gone = |c: &CdpContext| (!session.is_empty() && c.session_id == session) || (!target.is_empty() && c.target_id == target);
        let gone: Vec<(String, String, u32, bool)> = self
            .contexts
            .iter()
            .filter(|c| is_gone(c))
            .map(|c| (c.session_id.clone(), c.target_id.clone(), c.index, c.pooled))
            .collect();
        self.contexts.retain(|c| !is_gone(c));
        self.asked.retain(|_, a| !gone.iter().any(|(s, ..)| s == a.session()));
        for (_, _, index, pooled) in &gone {
            if *pooled {
                fold_into(&mut self.pool_sum, &mut self.counts, *index);
            }
        }
        gone.into_iter().map(|(_, target, ..)| target).collect()
    }

    /// Give every live context the zone override `zone` - a zone id, or an empty string to take it away.
    /// Not written down: the answer says nothing about what a context reads (the override is held by one
    /// context per renderer process), so the shim's own check is the evidence, and an answer that comes
    /// finds no request and is dropped.
    pub(crate) fn send_zone(&mut self, zone: &str, out: &mut impl Outbox) {
        for ctx in &self.contexts {
            let _ = out.ask(SET_ZONE, json!({ "timezoneId": zone }), &ctx.session_id);
        }
    }

    /// Give every live context `zone` back at the release - this machine's zone by name, or an empty
    /// string when the system names none - and ask each what it then shows with `read`. The read goes
    /// once every context answered the zone (ADR-21): the override of a renderer process has one holder,
    /// and a page read before its holder took the zone would show the session's and be judged kept. A
    /// context the zone could not even be sent to is read all the same, and answers for itself.
    pub(crate) fn give_zone_back(&mut self, zone: &str, read: &str, out: &mut impl Outbox) {
        for ctx in &self.contexts {
            if let Some(id) = out.ask(SET_ZONE, json!({ "timezoneId": zone }), &ctx.session_id) {
                self.asked.insert(id, Asked::Zone { session: ctx.session_id.clone() });
            }
        }
        self.zone_read = Some(read.to_string());
        if self.zones_settled() {
            self.read_zones_now(out);
        }
    }

    /// Whether no zone given back is still waiting for its answer.
    pub(crate) fn zones_settled(&self) -> bool {
        !self.asked.values().any(|a| matches!(a, Asked::Zone { .. }))
    }

    /// Send the zone read of the release to every live context now, whether every zone was answered or
    /// not - what the caller does once it can wait no longer. Once, and nothing before the zone was given.
    pub(crate) fn read_zones_now(&mut self, out: &mut impl Outbox) {
        let Some(read) = self.zone_read.take() else {
            return;
        };
        for ctx in &self.contexts {
            let params = json!({ "expression": read, "returnByValue": true });
            if let Some(id) = out.ask(EVALUATE, params, &ctx.session_id) {
                self.asked.insert(id, Asked::ZoneRead { session: ctx.session_id.clone() });
            }
        }
    }

    /// Whether the zone read of the release went out.
    pub(crate) fn zone_reads_sent(&self) -> bool {
        self.zone_read.is_none()
    }

    /// Whether the zone read went out and every context answered it.
    pub(crate) fn zone_reads_settled(&self) -> bool {
        self.zone_reads_sent() && !self.asked.values().any(|a| matches!(a, Asked::ZoneRead { .. }))
    }

    /// Where the live contexts stand with their zone after the release (see [`zone_back`]).
    pub(crate) fn zone_tally(&self, session: i32, machine: Option<[i32; 2]>) -> ZoneTally {
        let mut tally = ZoneTally::default();
        // A page still waiting for its first document has no zone to show (R5-F11).
        for ctx in self.contexts.iter().filter(|c| !c.awaiting) {
            match zone_back(ctx.zone_read, session, machine) {
                ZoneBack::Back => tally.back += 1,
                ZoneBack::Kept => tally.kept += 1,
                ZoneBack::Unanswered => tally.unanswered += 1,
            }
        }
        tally
    }

    /// How many contexts read a zone other than the session's at some point.
    pub(crate) fn zone_missed(&self) -> usize {
        self.zone_missed.len()
    }

    /// How many pages were hidden at some point while their timers ran faster.
    pub(crate) fn hidden_fast(&self) -> usize {
        self.hidden_fast.len()
    }

    /// How many contexts had a timer the engine's 4 ms floor held back at the session's speed.
    pub(crate) fn timers_capped(&self) -> usize {
        self.capped.len()
    }

    /// How many pages had a frame in their process the shim did not reach.
    pub(crate) fn frames_unreached(&self) -> usize {
        self.frames_unreached.len()
    }

    /// The indexes of the contexts that showed their shim since the last call (R5-F11).
    pub(crate) fn take_confirmed(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.confirmed)
    }

    /// Count the context of `index` in the pool's row (R5-P20) - decided when it is covered, which for
    /// a page waiting for its shim is after it was pushed.
    pub(crate) fn pool_context(&mut self, index: u32) {
        if let Some(ctx) = self.contexts.iter_mut().find(|c| c.index == index) {
            ctx.pooled = true;
        }
    }

    /// Ask every context that is not still answering the last request for its call counts, without
    /// waiting (R4-S10). A page is asked for its frame tree first, in the same session, so its answer
    /// comes first and the audit is judged against it (R5-P5). A worker has no frames.
    pub(crate) fn request_counts(&mut self, out: &mut impl Outbox) {
        for ctx in self.contexts.iter_mut().filter(|c| !c.counting) {
            if !cdp::is_worker(&ctx.ty)
                && let Some(id) = out.ask(FRAME_TREE, json!({}), &ctx.session_id)
            {
                self.asked.insert(id, Asked::Frames { session: ctx.session_id.clone() });
            }
            let params = json!({ "expression": cdp::COUNTS_EXPR, "returnByValue": true });
            if let Some(id) = out.ask(EVALUATE, params, &ctx.session_id) {
                ctx.counting = true;
                self.asked.insert(
                    id,
                    Asked::Counts { session: ctx.session_id.clone(), index: ctx.index, ty: ctx.ty.clone() },
                );
            }
        }
    }

    /// Start a clock move: a new hook for every page, built on `shim`, and `expr` for every live
    /// document - a page's once its new hook is back, a worker's at once. Returns the hook requests,
    /// for a caller that waits a bounded time for them before it acknowledges the move.
    pub(crate) fn start_move(&mut self, expr: &str, shim: &str, out: &mut impl Outbox) -> Vec<u64> {
        let expr: Arc<str> = Arc::from(expr);
        let mut hooks = Vec::new();
        for ctx in self.contexts.iter().filter(|c| !c.released) {
            let session = ctx.session_id.clone();
            if ctx.hooks.current.is_none() {
                send_move(&mut self.asked, &mut self.missed, &session, &expr, out);
                continue;
            }
            match out.ask(ADD_HOOK, json!({ "source": shim }), &session) {
                Some(id) => {
                    self.asked.insert(id, Asked::Hook { session, expr: Arc::clone(&expr) });
                    hooks.push(id);
                }
                None => {
                    // Not sent: the page keeps the hook it has, and its live document still moves.
                    self.missed.insert(session.clone());
                    send_move(&mut self.asked, &mut self.missed, &session, &expr, out);
                }
            }
        }
        hooks
    }

    /// Whether none of these requests is still in flight.
    pub(crate) fn answered(&self, ids: &[u64]) -> bool {
        ids.iter().all(|id| !self.asked.contains_key(id))
    }

    /// Let every context go: every hook it may have removed, and `expr` evaluated. From here on no
    /// clock move reaches it - a move still waiting for its hook would put the page back on the
    /// session clock after the session - and a hook that comes back is removed at once.
    pub(crate) fn start_release(&mut self, expr: &str, out: &mut impl Outbox) {
        for ctx in &mut self.contexts {
            ctx.released = true;
            ctx.hooks.unremoved.extend(ctx.hooks.current.take());
            unhook_all(ctx, &mut self.asked, out);
            let params = json!({ "expression": expr, "returnByValue": true });
            match out.ask(EVALUATE, params, &ctx.session_id) {
                Some(id) => {
                    self.asked.insert(id, Asked::Release { session: ctx.session_id.clone() });
                }
                None => ctx.release_answer = Some(false),
            }
        }
    }

    /// Whether every release and every hook request has been answered.
    pub(crate) fn release_settled(&self) -> bool {
        !self.asked.values().any(|a| matches!(a, Asked::Release { .. } | Asked::Hook { .. } | Asked::Unhook { .. }))
    }

    /// Whether every count request has been answered.
    pub(crate) fn counts_settled(&self) -> bool {
        !self.asked.values().any(|a| matches!(a, Asked::Counts { .. }))
    }

    /// How many contexts did not confirm they were let go: one that answered the release with
    /// anything but `ok` or `no-shim`, one that had not answered it, and one that may still have a
    /// hook - a removal it did not confirm, or a hook still on its way back.
    pub(crate) fn unreleased(&self) -> u32 {
        let still = |session: &str| {
            self.asked
                .values()
                .any(|a| a.session() == session && matches!(a, Asked::Hook { .. } | Asked::Unhook { .. }))
        };
        let count = self
            .contexts
            .iter()
            .filter(|c| c.release_answer != Some(true) || c.hooks != PageHooks::NONE || still(&c.session_id))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// At the end of the session: every move a live page has not answered is a move it missed.
    pub(crate) fn settle_moves(&mut self) {
        for asked in self.asked.values() {
            if let Asked::Hook { session, .. } | Asked::Move { session } = asked {
                self.missed.insert(session.clone());
            }
        }
    }

    /// How many pages missed at least one clock move.
    pub(crate) fn moves_missed(&self) -> usize {
        self.missed.len()
    }

    /// The counts, every pooled context's summed into the row of `pool` (R5-P20) - the live ones' peaks
    /// and what the ones that went away left in the pool's sum.
    pub(crate) fn into_counts(mut self, pool: Option<u32>) -> BTreeMap<(u32, String), u64> {
        let live_pooled: Vec<u32> = self.contexts.iter().filter(|c| c.pooled).map(|c| c.index).collect();
        for index in live_pooled {
            fold_into(&mut self.pool_sum, &mut self.counts, index);
        }
        if let Some(pool) = pool {
            for (api, n) in self.pool_sum {
                let entry = self.counts.entry((pool, api)).or_insert(0);
                *entry = entry.saturating_add(n);
            }
        }
        self.counts
    }

    /// Act on the answer to request `id`, whenever it comes. `reply` is the result, or `Err` for an
    /// error answer. An answer to a request this table did not make, or one dropped with its context,
    /// is nothing to act on.
    pub(crate) fn on_reply(&mut self, id: u64, reply: Result<Value, ()>, out: &mut impl Outbox) {
        let Some(asked) = self.asked.remove(&id) else {
            return;
        };
        let at = self.contexts.iter().position(|c| c.session_id == asked.session());
        match asked {
            Asked::Counts { index, ty, .. } => {
                if let Some(i) = at {
                    self.contexts[i].counting = false;
                }
                if let Ok(reply) = reply {
                    merge_counts(&mut self.counts, index, &ty, &reply);
                    if read_fact(&reply, "zone") {
                        self.zone_missed.insert(index);
                    }
                    if read_fact(&reply, "hidden") {
                        self.hidden_fast.insert(index);
                    }
                    if read_fact(&reply, "capped") {
                        self.capped.insert(index);
                    }
                    if let Some(i) = at {
                        self.audited(i, &reply);
                    }
                }
            }
            Asked::Frames { .. } => {
                if let Some(i) = at {
                    self.contexts[i].local_frames = reply.ok().as_ref().map(frames_below_root);
                }
            }
            Asked::Hook { session, expr } => {
                let new = reply.ok().as_ref().and_then(cdp::script_identifier);
                if new.is_none() {
                    // The page did not take the new hook: it keeps the one it had, and a reload brings
                    // back that hook's clock.
                    self.missed.insert(session.clone());
                }
                let Some(i) = at else {
                    return;
                };
                let ctx = &mut self.contexts[i];
                if let Some(new) = new {
                    if ctx.released {
                        ctx.hooks.unremoved.push(new);
                    } else {
                        ctx.hooks.unremoved.extend(ctx.hooks.current.replace(new));
                    }
                    unhook_all(ctx, &mut self.asked, out);
                }
                if !ctx.released {
                    send_move(&mut self.asked, &mut self.missed, &session, &expr, out);
                }
            }
            Asked::Unhook { script, .. } => {
                if reply.is_err()
                    && let Some(i) = at
                {
                    self.contexts[i].hooks.unremoved.push(script);
                }
            }
            Asked::Move { session } => {
                // A page waiting for its first document has none to move, and the hook the move gave it
                // first carries the new clock to it (R5-F11).
                let waiting = at.is_some_and(|i| self.contexts[i].awaiting);
                if !confirmed(&reply) && !waiting {
                    self.missed.insert(session);
                }
            }
            Asked::Release { .. } => {
                if let Some(i) = at {
                    let ctx = &mut self.contexts[i];
                    ctx.release_answer = Some(confirmed(&reply) || ctx.awaiting);
                }
            }
            Asked::Zone { .. } => {
                if self.zones_settled() {
                    self.read_zones_now(out);
                }
            }
            Asked::ZoneRead { .. } => {
                if let Some(i) = at {
                    self.contexts[i].zone_read = read_offsets(&reply);
                }
            }
        }
    }

    /// What an audit answered by the shim says beyond the counts: a page waiting for its shim has it
    /// (R5-F11), and how many frames of the page the shim reached, against the frame tree asked just
    /// before (R5-P5). A frame short in [`FRAMES_SHORT_AUDITS`] audits in a row is one the session could
    /// not reach. An answer that is not an object - `null`, no shim there - says neither.
    fn audited(&mut self, i: usize, reply: &Value) {
        let Some(audit) = reply["result"]["value"].as_object() else {
            return;
        };
        let ctx = &mut self.contexts[i];
        if ctx.awaiting {
            ctx.awaiting = false;
            self.confirmed.push(ctx.index);
        }
        if let Some(local) = ctx.local_frames.take() {
            let reached = audit.get("frames").and_then(Value::as_u64).unwrap_or(0);
            if local > reached {
                ctx.frames_short = ctx.frames_short.saturating_add(1);
                if ctx.frames_short >= FRAMES_SHORT_AUDITS {
                    self.frames_unreached.insert(ctx.index);
                }
            } else {
                ctx.frames_short = 0;
            }
        }
    }
}

/// How many frames a `Page.getFrameTree` answer holds below its root: the frames in the page's own
/// process (measured on Electron 44 - a frame of another site is a target of its own, and not in it).
/// Walked without recursion, so no page decides how deep the stack goes.
fn frames_below_root(reply: &Value) -> u64 {
    let mut below = 0u64;
    let mut open = vec![&reply["frameTree"]];
    while let Some(node) = open.pop() {
        if let Some(children) = node["childFrames"].as_array() {
            below = below.saturating_add(children.len() as u64);
            open.extend(children);
        }
    }
    below
}

/// Send a move's expression to one live document.
fn send_move(asked: &mut HashMap<u64, Asked>, missed: &mut BTreeSet<String>, session: &str, expr: &str, out: &mut impl Outbox) {
    match out.ask(EVALUATE, json!({ "expression": expr, "returnByValue": true }), session) {
        Some(id) => {
            asked.insert(id, Asked::Move { session: session.to_string() });
        }
        None => {
            missed.insert(session.to_string());
        }
    }
}

/// Ask a page to remove every hook it should not have any more. One that could not even be asked
/// stays on the list for the next try.
fn unhook_all(ctx: &mut CdpContext, asked: &mut HashMap<u64, Asked>, out: &mut impl Outbox) {
    let scripts = std::mem::take(&mut ctx.hooks.unremoved);
    for script in scripts {
        match out.ask(REMOVE_HOOK, json!({ "identifier": script }), &ctx.session_id) {
            Some(id) => {
                asked.insert(id, Asked::Unhook { session: ctx.session_id.clone(), script });
            }
            None => ctx.hooks.unremoved.push(script),
        }
    }
}

/// Move one pooled context's peaks out of the table and add them to the pool's sum (R5-P20).
fn fold_into(pool_sum: &mut BTreeMap<String, u64>, counts: &mut BTreeMap<(u32, String), u64>, index: u32) {
    let own: Vec<String> =
        counts.range((index, String::new())..).take_while(|((i, _), _)| *i == index).map(|((_, api), _)| api.clone()).collect();
    for api in own {
        if let Some(n) = counts.remove(&(index, api.clone())) {
            let entry = pool_sum.entry(api).or_insert(0);
            *entry = entry.saturating_add(n);
        }
    }
}

/// Merge one context's counts reply into the table, by max per `(index, "type api")`.
fn merge_counts(counts: &mut BTreeMap<(u32, String), u64>, index: u32, ty: &str, reply: &Value) {
    let Some(obj) = reply.get("result").and_then(|r| r.get("value")).and_then(Value::as_object) else {
        return;
    };
    for (api, key) in cdp::COUNTED_APIS {
        if let Some(n) = obj.get(key).and_then(Value::as_u64) {
            let entry = counts.entry((index, format!("{ty} {api}"))).or_insert(0);
            *entry = (*entry).max(n);
        }
    }
}

/// Whether a counts reply carries one of the shim's audit facts, each 1 once it happened: `zone` - the
/// context once read a zone other than the session's - `hidden` - the page was hidden while its timers
/// ran faster - and `capped` - a timer had to run more often than the engine's 4 ms floor (R5-P6). A
/// reply without it - a shim from an older build - says nothing.
fn read_fact(reply: &Value, fact: &str) -> bool {
    reply["result"]["value"][fact].as_u64().is_some_and(|n| n > 0)
}

/// Whether a context confirmed an evaluate that moves or lets go of its clock: `ok`, or `no-shim`
/// from one with no shim to move. Anything else did not take it - `late` from a page that got a
/// scheduled rate change after its instant (R4-S17), an error, or a reply with no value.
fn confirmed(reply: &Result<Value, ()>) -> bool {
    reply.as_ref().is_ok_and(|r| matches!(r["result"]["value"].as_str(), Some("ok" | "no-shim")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection that records what was asked and hands out ids in order. `refuse` makes every
    /// later send fail, the way a connection that is over does.
    #[derive(Default)]
    struct Wire {
        sent: Vec<(u64, String, String, Value)>,
        refuse: bool,
    }

    impl Outbox for Wire {
        fn ask(&mut self, method: &str, params: Value, session: &str) -> Option<u64> {
            if self.refuse {
                return None;
            }
            let id = self.sent.len() as u64 + 1;
            self.sent.push((id, method.to_string(), session.to_string(), params));
            Some(id)
        }
    }

    impl Wire {
        /// The ids of what was sent, by method and session, in order.
        fn ids(&self, method: &str, session: &str) -> Vec<u64> {
            self.sent.iter().filter(|(_, m, s, _)| m == method && s == session).map(|(id, ..)| *id).collect()
        }

        /// What was sent, as `method session` lines, from position `from` on.
        fn lines(&self, from: usize) -> Vec<String> {
            self.sent[from..].iter().map(|(_, m, s, _)| format!("{m} {s}")).collect()
        }
    }

    fn page(session: &str, script: &str) -> CdpContext {
        CdpContext::new(1, session.into(), "page".into(), format!("T-{session}"), Some(script.into()))
    }

    fn worker(session: &str) -> CdpContext {
        CdpContext::new(2, session.into(), "worker".into(), format!("T-{session}"), None)
    }

    fn value(v: &str) -> Result<Value, ()> {
        Ok(json!({ "result": { "type": "string", "value": v } }))
    }

    fn hook(id: &str) -> Result<Value, ()> {
        Ok(json!({ "identifier": id }))
    }

    fn requests(contexts: Vec<CdpContext>) -> Requests {
        let mut r = Requests::default();
        for c in contexts {
            r.push(c);
        }
        r
    }

    /// A frame tree answer with `children` below the page's own frame.
    fn tree(children: Value) -> Result<Value, ()> {
        Ok(json!({ "frameTree": { "frame": { "id": "root" }, "childFrames": children } }))
    }

    /// An audit answer from a shim that reached `frames` frames below its page.
    fn audit(frames: u64) -> Result<Value, ()> {
        Ok(json!({ "result": { "value": { "now": 1, "frames": frames } } }))
    }

    /// One audit of the page `P`: its frame tree answered with `frames_tree`, then its counts with
    /// `counts` - in that order, as the page's session answers them.
    fn audit_round(r: &mut Requests, wire: &mut Wire, frames_tree: Result<Value, ()>, counts: Result<Value, ()>) {
        let from = wire.sent.len();
        r.request_counts(wire);
        assert_eq!(wire.lines(from), [format!("{FRAME_TREE} P"), format!("{EVALUATE} P")], "the tree first");
        let ids: Vec<u64> = wire.sent[from..].iter().map(|(id, ..)| *id).collect();
        r.on_reply(ids[0], frames_tree, wire);
        r.on_reply(ids[1], counts, wire);
    }

    /// R5-P5: a page is asked for its frame tree just before its counts, in its own session, so the tree
    /// is answered first. A worker has no frames and is asked for its counts alone.
    #[test]
    fn a_page_is_asked_for_its_frame_tree_before_its_counts_and_a_worker_is_not() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.request_counts(&mut wire);
        assert_eq!(wire.lines(0), [format!("{FRAME_TREE} P"), format!("{EVALUATE} P"), format!("{EVALUATE} W")]);
    }

    /// R5-P5, R5-D47: a page whose shim reached fewer frames than its process holds, two audits in a
    /// row, has a frame the session could not reach. Short once is a frame that came or went between the
    /// two questions, an audit that reached them all starts the count over, and an audit with no tree
    /// before it - the tree could not be read - judges nothing. A shim of an older build says nothing of
    /// frames, and reached none.
    #[test]
    fn a_page_short_of_frames_two_audits_in_a_row_has_one_the_session_could_not_reach() {
        let two = json!([{ "frame": {}, "childFrames": [{ "frame": {} }] }]);
        let mut r = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        audit_round(&mut r, &mut wire, tree(two.clone()), audit(1));
        assert_eq!(r.frames_unreached(), 0, "short once is no proof");
        audit_round(&mut r, &mut wire, tree(two.clone()), audit(2));
        audit_round(&mut r, &mut wire, tree(two.clone()), audit(1));
        assert_eq!(r.frames_unreached(), 0, "all reached in between starts the count over");
        audit_round(&mut r, &mut wire, Err(()), audit(1));
        audit_round(&mut r, &mut wire, Err(()), audit(1));
        assert_eq!(r.frames_unreached(), 0, "no tree, no judgement");
        audit_round(&mut r, &mut wire, tree(two.clone()), audit(1));
        audit_round(&mut r, &mut wire, tree(two.clone()), audit(1));
        assert_eq!(r.frames_unreached(), 1, "short twice in a row");

        let mut old = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        let older = || Ok(json!({ "result": { "value": { "now": 1 } } }));
        audit_round(&mut old, &mut wire, tree(two.clone()), older());
        audit_round(&mut old, &mut wire, tree(two), older());
        assert_eq!(old.frames_unreached(), 1, "an older shim reached no frame");
    }

    /// The frames a tree holds below its root, however they are nested.
    #[test]
    fn the_frames_below_the_root_are_every_frame_of_the_tree_but_the_page() {
        assert_eq!(frames_below_root(&json!({ "frameTree": { "frame": {} } })), 0);
        let branchy = json!({ "frameTree": { "frame": {}, "childFrames": [
            { "frame": {}, "childFrames": [{ "frame": {} }, { "frame": {}, "childFrames": [{ "frame": {} }] }] },
            { "frame": {} }
        ] } });
        assert_eq!(frames_below_root(&branchy), 5);
        assert_eq!(frames_below_root(&json!({})), 0, "an answer with no tree holds no frame");
    }

    /// R5-P6: the audit says a timer was held back by the engine's 4 ms floor, once, for the context.
    #[test]
    fn a_timer_held_back_by_the_floor_is_read_from_the_audit() {
        let mut r = requests(vec![worker("W")]);
        let mut wire = Wire::default();
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[0], Ok(json!({ "result": { "value": { "now": 1, "capped": 0 } } })), &mut wire);
        assert_eq!(r.timers_capped(), 0);
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[1], Ok(json!({ "result": { "value": { "now": 1, "capped": 1 } } })), &mut wire);
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[2], Ok(json!({ "result": { "value": { "now": 1, "capped": 1 } } })), &mut wire);
        assert_eq!(r.timers_capped(), 1, "one context, however many audits say it");
    }

    /// R5-F11, R5-D46: a page waiting for its first document is confirmed by the first audit that
    /// answers from its shim, once - not by an error, and not by `null` (no shim there). Until then a
    /// move or a release it cannot take, having no document, is no miss, and it has no zone to show.
    #[test]
    fn a_page_waiting_for_its_shim_is_confirmed_by_an_audit_from_it() {
        let waiting = || CdpContext::new(3, "F".into(), "iframe".into(), "T-F".into(), Some("h0".into())).awaiting(true);
        let mut r = requests(vec![waiting()]);
        let mut wire = Wire::default();
        r.start_move("MOVE", "SHIM", &mut wire);
        r.on_reply(wire.ids(ADD_HOOK, "F")[0], hook("h1"), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "F")[0], Err(()), &mut wire);
        assert_eq!(r.moves_missed(), 0, "no document to move, and the new hook carries the clock to the first");
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "F")[1], Err(()), &mut wire);
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "F")[2], Ok(json!({ "result": { "value": null } })), &mut wire);
        assert!(r.take_confirmed().is_empty(), "an error and a null are not the shim");
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "F")[3], audit(0), &mut wire);
        assert_eq!(r.take_confirmed(), [3], "the first audit from the shim");
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "F")[4], audit(0), &mut wire);
        assert!(r.take_confirmed().is_empty(), "once");

        let mut still = requests(vec![waiting()]);
        let mut wire = Wire::default();
        still.start_release("RELEASE", &mut wire);
        still.on_reply(wire.ids(EVALUATE, "F")[0], Err(()), &mut wire);
        for id in wire.ids(REMOVE_HOOK, "F") {
            still.on_reply(id, Ok(json!({})), &mut wire);
        }
        assert_eq!(still.unreleased(), 0, "no document is nothing left on the session clock");
        assert_eq!(still.zone_tally(-330, Some([-60, -120])), ZoneTally::default(), "and no zone to show");
    }

    /// R5-P20: a pooled context's counts go to the pool's row, summed. One that went away leaves its
    /// peaks to the pool's sum and its entries leave the table, so the table is as large as the live
    /// list however many workers come and go. The live pooled ones are added at the end, and a context
    /// with a row of its own keeps it.
    #[test]
    fn a_pooled_context_leaves_its_counts_to_the_pool_and_its_place_in_the_table() {
        let pooled = |session: &str, index: u32| CdpContext::new(index, session.into(), "worker".into(), format!("T-{session}"), None);
        let mut r = requests(vec![page("P", "h0"), pooled("A", 7), pooled("B", 8)]);
        r.pool_context(7);
        r.pool_context(8);
        let mut wire = Wire::default();
        r.request_counts(&mut wire);
        let answer = |n: u64| Ok(json!({ "result": { "value": { "now": n } } }));
        r.on_reply(wire.ids(EVALUATE, "P")[0], answer(2), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "A")[0], answer(3), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "B")[0], answer(5), &mut wire);
        assert_eq!(r.forget("A", ""), vec!["T-A".to_string()]);
        assert!(!r.counts.keys().any(|(i, _)| *i == 7), "a pooled context that went away leaves the table");
        let counts = r.into_counts(Some(7));
        assert_eq!(counts.get(&(7, "worker Date.now".to_string())), Some(&8), "3 that went away plus 5 still live");
        assert_eq!(counts.get(&(1, "page Date.now".to_string())), Some(&2), "a row of its own stays its own");
        assert!(!counts.keys().any(|(i, _)| *i == 8), "a live pooled one is in the pool's row only");
    }

    /// R4-W5 kept without waiting on anyone: a page gets its move only after its new hook is back,
    /// and then the old hook goes. A worker, with no hook, gets its move at once. Nothing is sent to
    /// a page for its live document before its hook comes back, however long that takes.
    #[test]
    fn a_page_moves_after_its_new_hook_is_back_and_a_worker_moves_at_once() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        let waiting = r.start_move("MOVE", "SHIM", &mut wire);
        assert_eq!(wire.lines(0), [format!("{ADD_HOOK} P"), format!("{EVALUATE} W")]);
        assert_eq!(waiting, wire.ids(ADD_HOOK, "P"));
        assert!(!r.answered(&waiting));

        r.on_reply(waiting[0], hook("h1"), &mut wire);
        assert!(r.answered(&waiting));
        assert_eq!(wire.lines(2), [format!("{REMOVE_HOOK} P"), format!("{EVALUATE} P")]);
        assert_eq!(r.contexts()[0].hooks, PageHooks { current: Some("h1".into()), unremoved: vec![] });
    }

    /// The deadline bounds the wait, not what is learned: a hook that comes back after the caller
    /// stopped waiting is kept, and the page gets its move then. A move answered `ok` late is taken.
    #[test]
    fn a_hook_that_comes_back_late_is_kept_and_its_move_follows() {
        let mut r = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        let waiting = r.start_move("MOVE", "SHIM", &mut wire);
        // The caller gave up waiting here. Later:
        r.on_reply(waiting[0], hook("h1"), &mut wire);
        let moved = wire.ids(EVALUATE, "P");
        assert_eq!(moved.len(), 1);
        r.on_reply(moved[0], value("ok"), &mut wire);
        r.settle_moves();
        assert_eq!(r.moves_missed(), 0, "a late ok is a move taken");
        assert_eq!(r.contexts()[0].hooks.current.as_deref(), Some("h1"));
    }

    /// Whether a page missed a move is read from its answers, not from when they came: `late` from
    /// S17, a hook it did not take, and a move with no answer by the end are missed - each page once.
    #[test]
    fn a_missed_move_is_read_from_the_answer() {
        let mut r = requests(vec![page("A", "a0"), page("B", "b0"), worker("C"), worker("D")]);
        let mut wire = Wire::default();
        let waiting = r.start_move("MOVE", "SHIM", &mut wire);
        r.on_reply(waiting[0], hook("a1"), &mut wire);
        r.on_reply(waiting[1], Err(()), &mut wire);
        let a = wire.ids(EVALUATE, "A")[0];
        r.on_reply(a, value("late"), &mut wire);
        let b = wire.ids(EVALUATE, "B")[0];
        r.on_reply(b, value("ok"), &mut wire);
        let c = wire.ids(EVALUATE, "C")[0];
        r.on_reply(c, value("no-shim"), &mut wire);
        // D never answers.
        assert_eq!(r.moves_missed(), 2, "A said late, B kept its old hook");
        r.settle_moves();
        assert_eq!(r.moves_missed(), 3, "D had not answered by the end");
        assert_eq!(r.contexts()[1].hooks.current.as_deref(), Some("b0"), "B keeps the hook it had");
    }

    /// A page that misses two moves is one page that missed a move - the warning is about pages.
    #[test]
    fn a_page_is_counted_once_however_many_moves_it_missed() {
        let mut r = requests(vec![worker("W")]);
        let mut wire = Wire::default();
        r.start_move("ONE", "SHIM", &mut wire);
        r.start_move("TWO", "SHIM", &mut wire);
        for id in wire.ids(EVALUATE, "W") {
            r.on_reply(id, value("late"), &mut wire);
        }
        assert_eq!(r.moves_missed(), 1);
    }

    /// A removal the page did not confirm stays on the list and is tried again with the next move -
    /// and one ADD per move, however many old hooks wait (the review of #84, kept).
    #[test]
    fn a_hook_whose_removal_failed_is_tried_again_at_the_next_move() {
        let mut r = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        let first = r.start_move("ONE", "SHIM", &mut wire);
        r.on_reply(first[0], hook("h1"), &mut wire);
        let remove = wire.ids(REMOVE_HOOK, "P")[0];
        r.on_reply(remove, Err(()), &mut wire);
        assert_eq!(r.contexts()[0].hooks, PageHooks { current: Some("h1".into()), unremoved: vec!["h0".into()] });

        let mark = wire.sent.len();
        let second = r.start_move("TWO", "SHIM", &mut wire);
        r.on_reply(second[0], hook("h2"), &mut wire);
        let lines = wire.lines(mark);
        assert_eq!(lines.iter().filter(|l| l.starts_with(ADD_HOOK)).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.starts_with(REMOVE_HOOK)).count(), 2, "h0 again and h1");
    }

    /// The release wins over a move still waiting for its hook: that move's expression would put the
    /// page back on the session clock after the session, so it is never sent, and the hook that comes
    /// back is removed at once. Until it is, the page counts as not let go.
    #[test]
    fn a_hook_that_comes_back_after_the_release_is_removed_and_its_move_never_sent() {
        let mut r = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        let waiting = r.start_move("MOVE", "SHIM", &mut wire);
        r.start_release("RELEASE", &mut wire);
        let release = wire.ids(EVALUATE, "P");
        assert_eq!(release.len(), 1, "only the release, no move");
        r.on_reply(release[0], value("ok"), &mut wire);
        let unhook_h0 = wire.ids(REMOVE_HOOK, "P")[0];
        r.on_reply(unhook_h0, Ok(json!({})), &mut wire);
        assert_eq!(r.unreleased(), 1, "a hook is still on its way back");
        assert!(!r.release_settled());

        r.on_reply(waiting[0], hook("h1"), &mut wire);
        assert_eq!(wire.ids(EVALUATE, "P").len(), 1, "the move was never sent");
        let (unhook_h1, .., params) = wire.sent.last().unwrap().clone();
        assert_eq!(params, json!({ "identifier": "h1" }), "the hook that came back is the one removed");
        assert_eq!(r.unreleased(), 1, "until its removal is confirmed");
        r.on_reply(unhook_h1, Ok(json!({})), &mut wire);
        assert!(r.release_settled());
        assert_eq!(r.unreleased(), 0);
    }

    /// A page that did not confirm the release, did not answer it, or still has a hook it did not
    /// confirm removed is counted - each once.
    #[test]
    fn the_release_counts_every_page_it_cannot_vouch_for() {
        let mut r = requests(vec![page("A", "a0"), page("B", "b0"), worker("C"), worker("D")]);
        let mut wire = Wire::default();
        r.start_release("RELEASE", &mut wire);
        for s in ["A", "B", "C"] {
            let id = wire.ids(EVALUATE, s)[0];
            r.on_reply(id, value(if s == "C" { "late" } else { "ok" }), &mut wire);
        }
        r.on_reply(wire.ids(REMOVE_HOOK, "A")[0], Ok(json!({})), &mut wire);
        r.on_reply(wire.ids(REMOVE_HOOK, "B")[0], Err(()), &mut wire);
        // D never answers.
        assert_eq!(r.unreleased(), 3, "B kept a hook, C did not confirm, D did not answer");
    }

    /// A busy context is not asked for its counts again while the last request is in flight, and an
    /// answer that comes late still counts - by max, under the index and type it was asked for.
    #[test]
    fn a_busy_context_is_asked_for_its_counts_once_and_a_late_answer_counts() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.request_counts(&mut wire);
        let w = wire.ids(EVALUATE, "W")[0];
        r.on_reply(w, Ok(json!({ "result": { "value": { "now": 4 } } })), &mut wire);
        r.request_counts(&mut wire);
        assert_eq!(wire.ids(EVALUATE, "P").len(), 1, "P is still answering the first");
        assert_eq!(wire.ids(EVALUATE, "W").len(), 2, "W answered, so it is asked again");
        assert!(!r.counts_settled());
        let p = wire.ids(EVALUATE, "P")[0];
        r.on_reply(p, Ok(json!({ "result": { "value": { "now": 7 } } })), &mut wire);
        let counts = r.into_counts(None);
        assert_eq!(counts.get(&(1, "page Date.now".to_string())), Some(&7));
        assert_eq!(counts.get(&(2, "worker Date.now".to_string())), Some(&4));
    }

    /// A context that went away takes its requests with it, and they are not missed moves: a page
    /// that is gone has no clock to stand apart. Its late answers are nothing to act on.
    #[test]
    fn a_context_that_went_away_takes_its_requests_and_misses_nothing() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        let waiting = r.start_move("MOVE", "SHIM", &mut wire);
        assert_eq!(r.forget("", "T-P"), vec!["T-P".to_string()], "the target that went away is named");
        assert!(r.answered(&waiting));
        r.on_reply(waiting[0], hook("h1"), &mut wire);
        assert!(wire.ids(EVALUATE, "P").is_empty(), "nothing is sent to a page that is gone");
        r.settle_moves();
        assert_eq!(r.moves_missed(), 1, "only W, which never answered");
        assert!(r.forget("nobody", "").is_empty(), "nothing to drop");
    }

    /// A connection that can no longer send: the move is missed and the release is not confirmed -
    /// said, rather than waited for.
    #[test]
    fn a_request_that_could_not_be_sent_is_a_fact_at_once() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire { refuse: true, ..Wire::default() };
        assert!(r.start_move("MOVE", "SHIM", &mut wire).is_empty());
        assert_eq!(r.moves_missed(), 2);
        r.start_release("RELEASE", &mut wire);
        assert!(r.release_settled());
        assert_eq!(r.unreleased(), 2);
        assert_eq!(r.contexts()[0].hooks.unremoved, ["h0"], "a hook that could not be asked about is kept");
    }

    /// Only `ok`, and `no-shim` from a context with no shim to move, confirm a clock move or a
    /// release. `late` from a page that got a scheduled change after its instant does not (R4-S17),
    /// nor an error or a reply with no value.
    #[test]
    fn only_ok_and_no_shim_confirm_a_clock_move() {
        assert!(confirmed(&value("ok")));
        assert!(confirmed(&value("no-shim")));
        assert!(!confirmed(&value("late")));
        assert!(!confirmed(&Ok(json!({ "result": {} }))));
        assert!(!confirmed(&Err(())));
    }

    /// An answer this table did not ask for - a reply to a call that timed out, a resume nobody
    /// waits on - changes nothing.
    #[test]
    fn an_answer_nobody_asked_for_changes_nothing() {
        let mut r = requests(vec![page("P", "h0")]);
        let mut wire = Wire::default();
        r.on_reply(99, hook("stranger"), &mut wire);
        assert!(wire.sent.is_empty());
        assert_eq!(r.contexts()[0].hooks.current.as_deref(), Some("h0"));
    }

    /// A context whose shim once read another zone is counted once and stays counted - a page whose
    /// process lost the override and got it back still read the host's zone in between, and a page
    /// that closed keeps its evidence. A reply without the fact (an older shim) says nothing.
    #[test]
    fn a_context_that_once_read_another_zone_stays_counted() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        let answer = |zone: u64| Ok(json!({ "result": { "value": { "now": 1, "zone": zone } } }));
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "P")[0], answer(1), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[0], Ok(json!({ "result": { "value": { "now": 1 } } })), &mut wire);
        assert_eq!(r.zone_missed(), 1);
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "P")[1], answer(0), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[1], answer(0), &mut wire);
        assert_eq!(r.zone_missed(), 1, "a later right reading does not undo an earlier wrong one");
        assert!(!r.forget("P", "").is_empty());
        assert_eq!(r.zone_missed(), 1, "a closed page keeps its evidence");
    }

    /// A page hidden while its timers ran faster is counted once, and stays counted, like the zone.
    #[test]
    fn a_page_once_hidden_while_its_timers_ran_faster_stays_counted() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        let answer = |hidden: u64| Ok(json!({ "result": { "value": { "si": 3, "hidden": hidden } } }));
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "P")[0], answer(1), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[0], answer(0), &mut wire);
        assert_eq!(r.hidden_fast(), 1);
        r.request_counts(&mut wire);
        r.on_reply(wire.ids(EVALUATE, "P")[1], answer(0), &mut wire);
        assert_eq!(r.hidden_fast(), 1, "shown again, it was still hidden before");
        assert_eq!(r.zone_missed(), 0, "the two facts are told apart");
    }

    /// ADR-21 at the release: the zone given back goes to every live context, and what each then shows is
    /// read only once the LAST of them answered the zone - a page read before its renderer's holder took
    /// the zone would show the session's and be judged kept. The answers are any answer: a context that is
    /// not its process's holder answers with an error, and still counts.
    #[test]
    fn the_zone_read_goes_once_every_context_answered_the_zone_given_back() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.give_zone_back("Europe/Warsaw", "READ", &mut wire);
        assert_eq!(wire.lines(0), vec![format!("{SET_ZONE} P"), format!("{SET_ZONE} W")]);
        assert!(wire.sent.iter().all(|(.., params)| params["timezoneId"] == "Europe/Warsaw"));
        assert!(!r.zones_settled() && !r.zone_reads_sent());

        r.on_reply(wire.ids(SET_ZONE, "P")[0], Ok(json!({})), &mut wire);
        assert!(wire.ids(EVALUATE, "P").is_empty(), "W has not answered the zone yet");
        r.on_reply(wire.ids(SET_ZONE, "W")[0], Err(()), &mut wire);
        assert_eq!(wire.lines(2), vec![format!("{EVALUATE} P"), format!("{EVALUATE} W")]);
        assert!(wire.sent[2..].iter().all(|(.., params)| params["expression"] == "READ"));
        assert!(r.zone_reads_sent() && !r.zone_reads_settled());

        r.on_reply(wire.ids(EVALUATE, "P")[0], Ok(json!({ "result": { "value": [-60, -120] } })), &mut wire);
        r.on_reply(wire.ids(EVALUATE, "W")[0], Ok(json!({ "result": { "value": [-330, -330] } })), &mut wire);
        assert!(r.zone_reads_settled());
        assert_eq!(r.zone_tally(-330, Some([-60, -120])), ZoneTally { back: 1, kept: 1, unanswered: 0 });
    }

    /// A caller that waits no longer sends the read itself, once - a context whose zone answer never came
    /// is read all the same and answers for itself. Nothing is read before the zone was given back, and a
    /// read that could not be sent, or never answered, leaves its context unanswered.
    #[test]
    fn the_caller_sends_the_zone_read_when_it_waits_no_longer_and_only_once() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.read_zones_now(&mut wire);
        assert!(wire.sent.is_empty(), "nothing to read before the zone was given back");

        r.give_zone_back("", "READ", &mut wire);
        assert!(wire.sent.iter().all(|(.., params)| params["timezoneId"] == ""), "no name: the override is taken away");
        r.read_zones_now(&mut wire);
        r.read_zones_now(&mut wire);
        assert_eq!(wire.ids(EVALUATE, "P").len() + wire.ids(EVALUATE, "W").len(), 2, "once to each");
        r.on_reply(wire.ids(SET_ZONE, "P")[0], Ok(json!({})), &mut wire);
        r.on_reply(wire.ids(SET_ZONE, "W")[0], Ok(json!({})), &mut wire);
        assert_eq!(wire.ids(EVALUATE, "P").len(), 1, "the late zone answers send nothing again");
        r.on_reply(wire.ids(EVALUATE, "P")[0], Ok(json!({ "result": { "value": [0, 0] } })), &mut wire);
        assert_eq!(r.zone_tally(-330, Some([0, 0])), ZoneTally { back: 1, kept: 0, unanswered: 1 }, "W never answered");
        assert!(r.zone_tally(-330, Some([0, 0])).any_not_back(), "one that did not answer cannot be vouched for");
        assert!(!ZoneTally { back: 3, kept: 0, unanswered: 0 }.any_not_back());

        let mut gone = requests(vec![page("P", "h0")]);
        let mut refused = Wire { refuse: true, ..Wire::default() };
        gone.give_zone_back("Europe/Warsaw", "READ", &mut refused);
        assert!(gone.zone_reads_settled(), "nothing in flight on a connection that is over");
        assert_eq!(gone.zone_tally(-330, Some([-60, -120])).unanswered, 1);
    }

    /// The judgement of one context. Kept is the session's offset at both instants where the machine has
    /// another at one of them. A reading that is neither the session's nor the machine's is back - an
    /// engine with older zone rules shows it with or without a session. A machine on the session's
    /// offset all year cannot be told apart and is back, and without the machine's offsets a session
    /// reading cannot be vouched for.
    #[test]
    fn a_context_is_kept_only_on_the_sessions_offset_where_the_machine_has_another() {
        let warsaw = Some([-60, -120]);
        assert_eq!(zone_back(Some([-60, -120]), -330, warsaw), ZoneBack::Back);
        assert_eq!(zone_back(Some([-330, -330]), -330, warsaw), ZoneBack::Kept);
        assert_eq!(zone_back(Some([-120, -120]), -120, warsaw), ZoneBack::Kept, "summer's offset is not January's");
        assert_eq!(zone_back(Some([-60, -60]), -120, warsaw), ZoneBack::Back, "older rules, not the session's");
        assert_eq!(zone_back(Some([-330, -60]), -330, warsaw), ZoneBack::Back, "one instant off the session is enough");
        assert_eq!(zone_back(Some([-540, -540]), -540, Some([-540, -540])), ZoneBack::Back, "cannot be told apart");
        assert_eq!(zone_back(Some([-330, -330]), -330, None), ZoneBack::Unanswered);
        assert_eq!(zone_back(Some([-60, -120]), -330, None), ZoneBack::Back);
        assert_eq!(zone_back(None, -330, warsaw), ZoneBack::Unanswered);
    }

    /// Only two numbers a zone can have are a reading: fractions round to the minute, and anything else -
    /// one number, text, `null`, an error, an offset over a day - is no reading at all.
    #[test]
    fn a_zone_read_is_two_offsets_or_nothing() {
        let value = |v: Value| Ok(json!({ "result": { "value": v } }));
        assert_eq!(read_offsets(&value(json!([-60, -120]))), Some([-60, -120]));
        assert_eq!(read_offsets(&value(json!([-59.6, 330.2]))), Some([-60, 330]));
        assert_eq!(read_offsets(&value(json!([-60]))), None);
        assert_eq!(read_offsets(&value(json!([-60, -120, 0]))), None);
        assert_eq!(read_offsets(&value(json!(["-60", -120]))), None);
        assert_eq!(read_offsets(&value(json!([2000, 0]))), None);
        assert_eq!(read_offsets(&value(Value::Null)), None);
        assert_eq!(read_offsets(&Err(())), None);
    }

    /// A context that went away during the release takes its zone read with it: a closed page reads no
    /// zone, and it is neither kept nor unanswered.
    #[test]
    fn a_context_that_went_away_is_not_judged_on_its_zone() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.give_zone_back("Europe/Warsaw", "READ", &mut wire);
        assert!(!r.forget("W", "").is_empty());
        assert!(!r.zones_settled(), "P has not answered");
        r.on_reply(wire.ids(SET_ZONE, "P")[0], Ok(json!({})), &mut wire);
        assert_eq!(wire.ids(EVALUATE, "W").len(), 0, "nothing goes to a context that is gone");
        r.on_reply(wire.ids(EVALUATE, "P")[0], Ok(json!({ "result": { "value": [-60, -120] } })), &mut wire);
        assert_eq!(r.zone_tally(-330, Some([-60, -120])), ZoneTally { back: 1, kept: 0, unanswered: 0 });
    }

    /// The zone goes to every live context and nothing waits for it: its answer proves nothing, so it
    /// is not written down, and when it comes it is an answer nobody asked for.
    #[test]
    fn the_zone_goes_to_every_live_context_and_nothing_waits_for_it() {
        let mut r = requests(vec![page("P", "h0"), worker("W")]);
        let mut wire = Wire::default();
        r.send_zone("GMT+05:30", &mut wire);
        assert_eq!(wire.lines(0), vec![format!("{SET_ZONE} P"), format!("{SET_ZONE} W")]);
        assert!(wire.sent.iter().all(|(.., params)| params["timezoneId"] == "GMT+05:30"));
        assert!(r.asked.is_empty(), "nothing waits for the zone");
        assert!(r.counts_settled() && r.release_settled());
    }
}
