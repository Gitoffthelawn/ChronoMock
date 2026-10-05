//! What a Chromium session covered, and the verdict that follows from it.
//!
//! The coverage unit here is a JS context rather than an operating-system process, and the rule is
//! the one the native path obeys as well: a count belongs to the context that made it and is never
//! summed across contexts (untouchable rule 4). Everything in this module is a pure function over
//! what the session observed, so those rules can be checked without a browser - which matters,
//! because the bug `cdp_verdict` exists to pin needed a real Chromium and a gracefully closed
//! window to reproduce.

use std::collections::{BTreeMap, HashMap};

use chrono_core::Verdict;
use chrono_proto::{CoveredChannel, Event, PROTOCOL_VERSION, UNIT_CONTEXT};

/// A page took a rate change or a jump late or not at all, or did not take the new-document hook
/// that carries the clock: its clock stands apart from the session's until the next jump, and a
/// reload may bring back an older one. Said by the Chromium session and by the bridge to the pages
/// of an embedded engine alike - the same fact on both paths.
pub(crate) const KEY_CLOCK_MOVE_MISSED: &str = "chromium.clock_move_missed";

/// The channels the session can honestly call covered, in a stable order.
pub(crate) fn covered_channels(counts: BTreeMap<(u32, String), u64>) -> Vec<(u32, String, u64)> {
    // Coverage = APIs the app actually called (count > 0), per context - honest "covered", like native.
    let mut covered: Vec<(u32, String, u64)> = counts
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|((idx, ch), n)| (idx, ch, n))
        .collect();
    covered.sort();
    covered
}

/// The verdict of a CDP session, from the three facts that decide it. Pulled out of the session loop
/// so it can be tested without a browser - the bug it exists to pin needed a real Chromium and a
/// gracefully closed window to reproduce (R2-W1).
///
/// `shimmed` counts every context the session EVER covered, not the ones still attached. Chromium
/// destroys its targets while shutting down, so a healthy session with full coverage could reach the
/// end with an empty live list - counting those, it reported `fails` with exit code 11 and emitted no
/// coverage at all. Measured on a real Electron app: closing the window mid-session turned a `works` run with
/// four covered APIs into `DID NOT TAKE EFFECT (contexts: 0)`, exit 11. What a session covered does
/// not stop being true when the app closes.
pub(crate) fn cdp_verdict(shimmed: usize, any_covered: bool, failed: usize) -> Verdict {
    if shimmed == 0 {
        // Nothing was ever shimmed: the substitution genuinely never reached the app.
        Verdict::Fails
    } else if !any_covered {
        // Shimmed, but the app never called a time API - honest "we do not know", never a fake works.
        Verdict::Undetermined
    } else if failed > 0 {
        Verdict::Partial
    } else {
        Verdict::Works
    }
}

/// What the pages reached inside a natively hooked application contribute to the family verdict
/// (docs/09 section 12.7). `Undetermined` is the identity of `Verdict::combine`, so it is the answer
/// wherever there is nothing to judge: no engine was reached at all, or one was and had no page yet
/// (an application whose web view has not opened is not an application whose pages ran real).
/// With pages present the judgement is the Chromium session's own - every shimmed context that read
/// time is covered, a context refused or failed is uncovered.
pub(crate) fn embedded_verdict(reached: bool, shimmed: usize, any_covered: bool, failed: usize) -> Verdict {
    if !reached || (shimmed == 0 && failed == 0) {
        Verdict::Undetermined
    } else {
        cdp_verdict(shimmed, any_covered, failed)
    }
}

/// The wire token and the reason key of a verdict, decided in one place, so the token a client
/// branches on and the key it renders cannot come apart.
pub(crate) fn verdict_keys(verdict: &Verdict) -> (&'static str, &'static str) {
    match verdict {
        Verdict::Works => ("works", "chromium.contexts_covered"),
        Verdict::Partial => ("partial", "chromium.contexts_partial"),
        Verdict::Fails => ("fails", "chromium.no_contexts"),
        Verdict::Undetermined => ("undetermined", "chromium.no_time_calls"),
    }
}

/// The target ended after handing the application over to another process, and the session went on
/// with the browser that process runs instead of ending with the target (R4-S18). Its own key rather
/// than the native `session.followed_family`: a session on this path is held by its debugging
/// connection alone, so the native text's warning about a helper keeping the session open is not true
/// here.
pub(crate) const KEY_FOLLOWED_BROWSER: &str = "chromium.followed_browser";

/// A page or worker read a zone other than the session's at some point: the engine's zone override did
/// not reach it, or its renderer process lost the override with the page that held it (R4/16). Its
/// clock was the session's, its local time was not.
pub(crate) const KEY_ZONE_IS_HOST: &str = "chromium.zone_is_host";

/// What a finished CDP session observed about itself, each fact under its name. Named fields rather
/// than a row of booleans, because a call site with seven `true`/`false` in a row cannot be read and
/// two of them swapped still compiles.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SessionFacts {
    /// The connection closed, i.e. the application exited, before the session ended it.
    pub(crate) app_closed: bool,
    /// At least one context answered with its call counts.
    pub(crate) audited: bool,
    /// The rate changed while the session ran.
    pub(crate) rate_changed_in_flight: bool,
    /// A context took a clock move late or not at all.
    pub(crate) clock_moves_missed: bool,
    /// Contexts were refused past the ceiling.
    pub(crate) context_ceiling_reached: bool,
    /// The fake wall reached the end of its range and stood there.
    pub(crate) clock_clamped: bool,
    /// The target ended and the session went on with the browser it handed the application to.
    pub(crate) followed_browser: bool,
    /// A context read a zone other than the session's at some point.
    pub(crate) zone_missed: bool,
}

/// What a finished CDP session has to say about itself beyond the coverage numbers.
///
/// The launch is invasive by construction (our own profile, a debug port), so that one is always
/// said. The others are honest caveats rather than failures, and each would be invisible to the
/// reader if it were left out (rule 6).
pub(crate) fn session_warnings(facts: &SessionFacts) -> Vec<String> {
    let mut warnings = vec!["chromium.launched_with_debug_port".to_string()];
    if facts.followed_browser {
        // Said next to the launch, because it explains the shape of the whole session: the program
        // the tester named ended, and the session did not (R4-S18).
        warnings.push(KEY_FOLLOWED_BROWSER.to_string());
    }
    if facts.app_closed && !facts.audited {
        warnings.push("chromium.app_closed_before_audit".to_string());
    }
    if facts.rate_changed_in_flight {
        // Honest caveat: a rate change reaches Date.now/new Date/performance.now and every NEW timer at
        // once, but a setInterval already scheduled at the old rate keeps its old cadence - the JS engine
        // had already queued it (rule 4). The native hook has no equivalent gap (it divides Ctl live).
        warnings.push("chromium.rate_change_affects_running_timers".to_string());
    }
    if facts.clock_moves_missed {
        // A context took a rate change or a jump late, or not at all, or did not take the new
        // new-document hook: it runs apart from the panel until the next jump, and a reload may bring
        // back an older clock (R4-S17, R4-W5).
        warnings.push(KEY_CLOCK_MOVE_MISSED.to_string());
    }
    if facts.context_ceiling_reached {
        // The attacher shims at most MAX_CONTEXTS contexts in one session. The ones past that ran on
        // the real clock and have no row in the audit - the verdict already counts them as uncovered,
        // and this says why (rule 4).
        warnings.push("chromium.context_ceiling_reached".to_string());
    }
    if facts.zone_missed {
        // The shim in a context read an offset other than the session's (rule 4: the zone is said to
        // be the session's only where it was read to be).
        warnings.push(KEY_ZONE_IS_HOST.to_string());
    }
    if facts.clock_clamped {
        // The same key the native session uses (R4-S8): the wall reached the last instant it can hold
        // and stood there, so later readings are not what the chosen speed would have produced.
        warnings.push("time.fake_clock_clamped".to_string());
    }
    warnings
}

/// One `coverage` event per context the session covered.
///
/// Built rather than emitted, so the rules below can be read off a return value instead of a
/// stream: exactly one event per context, each carrying only its own counts, and the warnings on
/// the first event alone.
pub(crate) fn coverage_events(
    seen: &[u32],
    covered: &[(u32, String, u64)],
    mut warnings: Vec<String>,
) -> Vec<Event> {
    // Emit one `coverage` per attached context (pid = context index), never summed across contexts
    // (rule 4). The invasive-launch warning rides on the FIRST event, and if no context attached at
    // all we still emit one bare coverage - so the warning is never lost for an idle or zero-context app.
    if seen.is_empty() {
        return vec![Event::Coverage {
            v: PROTOCOL_VERSION,
            pid: 0,
            kind: UNIT_CONTEXT.to_string(),
            covered: Vec::new(),
            observed: Vec::new(),
            uncovered: Vec::new(),
            // A Chromium session hooks nothing, so there is no observer that could fail - and no module
            // that could arrive late.
            unobserved: Vec::new(),
            installed_late: Vec::new(),
            warning_keys: warnings,
        }];
    }
    seen.iter()
        .map(|index| {
            let chans: Vec<CoveredChannel> = covered
                .iter()
                .filter(|(idx, _, _)| idx == index)
                .map(|(_, ch, n)| CoveredChannel { channel: ch.clone(), calls: *n })
                .collect();
            Event::Coverage {
                v: PROTOCOL_VERSION,
                pid: *index,
                kind: UNIT_CONTEXT.to_string(),
                covered: chans,
                observed: Vec::new(),
                uncovered: Vec::new(),
                unobserved: Vec::new(),
                installed_late: Vec::new(),
                warning_keys: std::mem::take(&mut warnings),
            }
        })
        .collect()
}

/// The context index for a CDP target, stable across re-attaches.
///
/// Keyed by the CDP targetId, which Chromium keeps when a context re-attaches, rather than by a
/// counter that ticks once per attach. A recycled worker - an ordinary pattern in Electron apps,
/// and the very shape the CDP mechanism was built for - re-attaches under the SAME targetId, and
/// the counter gave it a new identity every time: `process_count` grew with the LENGTH of the
/// session instead of describing the application, `counts` gained entries per recycle and released
/// none, and the end-of-session emit walked seen x covered. The merge rule for counts is already
/// "max, so a peak survives a reload" - it was only ever missing a stable key (R3-7).
///
/// A target that names no id gets a fresh index: with no identity to match on, treating it as new
/// is the honest answer rather than a guess that would fold two contexts into one.
pub(crate) fn context_index_for(
    target_id: &str,
    index_by_target: &mut HashMap<String, u32>,
    next_index: &mut u32,
) -> u32 {
    if !target_id.is_empty()
        && let Some(existing) = index_by_target.get(target_id)
    {
        return *existing;
    }
    *next_index += 1;
    if !target_id.is_empty() {
        index_by_target.insert(target_id.to_string(), *next_index);
    }
    *next_index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worker that is recycled re-attaches under the same targetId, and used to be counted as a
    /// new context each time - so a long accelerated session (the flagship use case: "a day a
    /// minute") reported hundreds of contexts for an application with one worker (R3-7).
    #[test]
    fn a_reattached_context_keeps_the_index_it_already_had() {
        let mut map = HashMap::new();
        let mut next = 0u32;

        let page = context_index_for("T-page", &mut map, &mut next);
        let worker = context_index_for("T-worker", &mut map, &mut next);
        assert_eq!((page, worker), (1, 2));

        // The worker is recycled twice: same target, same index, and the counter does not move.
        assert_eq!(context_index_for("T-worker", &mut map, &mut next), worker);
        assert_eq!(context_index_for("T-worker", &mut map, &mut next), worker);
        assert_eq!(next, 2, "a re-attach must not mint a new context index");

        // A genuinely new target still gets one.
        assert_eq!(context_index_for("T-other", &mut map, &mut next), 3);

        // No id means no identity to match on, so each one is new rather than folded together -
        // two anonymous contexts are two contexts, and pretending otherwise would under-report.
        let a = context_index_for("", &mut map, &mut next);
        let b = context_index_for("", &mut map, &mut next);
        assert_ne!(a, b);
    }

    /// What the pages inside a natively hooked application contribute to the family (docs/09
    /// section 12.7). Nothing reached, or reached with no page to judge, is the identity of the fold -
    /// an application whose web view never opened is not one whose pages ran real. With pages the
    /// judgement is the Chromium session's own.
    #[test]
    fn the_pages_contribute_nothing_until_there_is_a_page_to_judge() {
        assert_eq!(embedded_verdict(false, 0, false, 0), Verdict::Undetermined);
        assert_eq!(embedded_verdict(true, 0, false, 0), Verdict::Undetermined, "an engine with no page yet");
        assert_eq!(embedded_verdict(true, 2, true, 0), Verdict::Works);
        assert_eq!(embedded_verdict(true, 2, false, 0), Verdict::Undetermined, "shimmed, never asked the time");
        assert_eq!(embedded_verdict(true, 2, true, 1), Verdict::Partial);
        assert_eq!(embedded_verdict(true, 0, false, 1), Verdict::Fails, "a page refused and none covered");
        // The identity of the fold: a family that works stays works with nothing to judge.
        assert_eq!(Verdict::Works.combine(embedded_verdict(true, 0, false, 0)), Verdict::Works);
    }

    #[test]
    fn cdp_verdict_counts_every_context_the_session_covered_not_the_survivors() {
        // R2-W1, the case that needed a real browser to reproduce: Chromium destroys its targets while
        // shutting down, so a healthy session could reach the verdict with an empty LIVE context list.
        // Counting survivors called it `fails` with exit code 11 and dropped the coverage entirely -
        // measured on a real Electron app, a closing window turned a four-channel `works` into
        // "DID NOT TAKE EFFECT (contexts: 0)". Two contexts shimmed and covered stays `works` however
        // many of them are still attached, because the argument is what the session covered.
        assert_eq!(cdp_verdict(2, true, 0), Verdict::Works);

        // Nothing ever shimmed is the one genuine failure: the substitution never reached the app.
        assert_eq!(cdp_verdict(0, false, 0), Verdict::Fails);

        // Shimmed but never asked the time: honest "we do not know", never a fake works (rule 4).
        assert_eq!(cdp_verdict(1, false, 0), Verdict::Undetermined);

        // Some contexts failed to take the shim: covered in part, and said so.
        assert_eq!(cdp_verdict(3, true, 1), Verdict::Partial);
    }

    fn counts(entries: &[(u32, &str, u64)]) -> BTreeMap<(u32, String), u64> {
        entries.iter().map(|(i, ch, n)| ((*i, (*ch).to_string()), *n)).collect()
    }

    fn channels(event: &Event) -> Vec<(String, u64)> {
        match event {
            Event::Coverage { covered, .. } => {
                covered.iter().map(|c| (c.channel.clone(), c.calls)).collect()
            }
            _ => panic!("expected a coverage event"),
        }
    }

    /// Untouchable rule 4 on the Chromium path: the same API called in two contexts is two events
    /// with their own counts, never one event with the calls added up. A summed number would claim
    /// coverage for one context out of another context's evidence.
    #[test]
    fn coverage_is_one_event_per_context_and_never_summed() {
        let covered = covered_channels(counts(&[(0, "page Date.now", 7), (1, "page Date.now", 5)]));
        let events = coverage_events(&[0, 1], &covered, Vec::new());

        assert_eq!(events.len(), 2, "two contexts, two events");
        assert_eq!(channels(&events[0]), vec![("page Date.now".to_string(), 7)]);
        assert_eq!(channels(&events[1]), vec![("page Date.now".to_string(), 5)]);
    }

    /// An API the app never called is not coverage. Reporting a zero as covered would be the audit
    /// claiming an effect it did not measure.
    #[test]
    fn an_api_that_was_never_called_is_not_reported_as_covered() {
        let covered = covered_channels(counts(&[(0, "page Date.now", 0), (0, "page setInterval", 3)]));
        let events = coverage_events(&[0], &covered, Vec::new());

        assert_eq!(channels(&events[0]), vec![("page setInterval".to_string(), 3)]);
    }

    /// The warnings ride the FIRST event only, so a reader sees each one once rather than once per
    /// context.
    #[test]
    fn the_warnings_ride_the_first_event_only() {
        let covered = covered_channels(counts(&[(0, "page Date.now", 1), (1, "worker Date.now", 1)]));
        let events = coverage_events(&[0, 1], &covered, vec!["chromium.launched_with_debug_port".into()]);

        let keys = |e: &Event| match e {
            Event::Coverage { warning_keys, .. } => warning_keys.clone(),
            _ => panic!("expected a coverage event"),
        };
        assert_eq!(keys(&events[0]), vec!["chromium.launched_with_debug_port"]);
        assert!(keys(&events[1]).is_empty(), "the second event does not repeat the warning");
    }

    /// A session where nothing ever attached still emits one bare coverage event, because the
    /// warnings travel on it - an idle or zero-context app would otherwise lose them entirely.
    #[test]
    fn a_session_with_no_context_still_carries_its_warnings() {
        let events = coverage_events(&[], &[], vec!["chromium.launched_with_debug_port".into()]);

        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Coverage { pid, covered, warning_keys, .. } => {
                assert_eq!(*pid, 0);
                assert!(covered.is_empty());
                assert_eq!(warning_keys, &vec!["chromium.launched_with_debug_port".to_string()]);
            }
            _ => panic!("expected a coverage event"),
        }
    }

    /// The three caveats are conditional and the invasive-launch note is not. An app that closed before
    /// anything could be audited says so, a rate changed in flight says so, because a running
    /// setInterval keeps its old cadence, and a session that refused contexts past its ceiling says
    /// so, because those ran on the real clock with no row in the audit (rule 4). A page that took a
    /// clock move late or not at all says so too, because its clock stands apart from the panel's, and
    /// a session that outlived the program the tester named says why.
    #[test]
    fn the_session_warnings_say_only_what_happened() {
        let audited = SessionFacts { audited: true, ..SessionFacts::default() };
        assert_eq!(session_warnings(&audited), vec!["chromium.launched_with_debug_port"]);
        assert_eq!(
            session_warnings(&SessionFacts { app_closed: true, ..SessionFacts::default() }),
            vec!["chromium.launched_with_debug_port", "chromium.app_closed_before_audit"]
        );
        assert_eq!(
            session_warnings(&SessionFacts { context_ceiling_reached: true, ..audited }),
            vec!["chromium.launched_with_debug_port", "chromium.context_ceiling_reached"]
        );
        assert_eq!(
            session_warnings(&SessionFacts { app_closed: true, rate_changed_in_flight: true, ..audited }),
            vec!["chromium.launched_with_debug_port", "chromium.rate_change_affects_running_timers"],
            "an app that closed AFTER being audited has nothing to apologise for"
        );
        assert_eq!(
            session_warnings(&SessionFacts { rate_changed_in_flight: true, clock_moves_missed: true, ..audited }),
            vec![
                "chromium.launched_with_debug_port",
                "chromium.rate_change_affects_running_timers",
                "chromium.clock_move_missed"
            ],
            "a page that took a clock move late or not at all says so (R4-S17)"
        );
        assert_eq!(
            session_warnings(&SessionFacts { clock_clamped: true, ..audited }),
            vec!["chromium.launched_with_debug_port", "time.fake_clock_clamped"],
            "a clock that stood at the end of its range says so, as it does natively (R4-S8)"
        );
        assert_eq!(
            session_warnings(&SessionFacts { followed_browser: true, ..audited }),
            vec!["chromium.launched_with_debug_port", KEY_FOLLOWED_BROWSER],
            "a session that went on after its target handed the application over says so (R4-S18)"
        );
        assert_eq!(
            session_warnings(&SessionFacts { zone_missed: true, ..audited }),
            vec!["chromium.launched_with_debug_port", KEY_ZONE_IS_HOST],
            "a context that read another zone says so"
        );
    }
}
