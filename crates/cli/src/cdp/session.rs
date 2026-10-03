//! The JS time shim and its injection into every context of a Chromium target (slice C3). The shim
//! is the CDP mechanism's equivalent of the native hook: it overrides the JS time APIs so the
//! target's own timers run on the session clock. Injection uses auto-attach so it reaches the page
//! AND its Web Workers, where an Electron app's timer often turns out to live.
//!
//! What the shim covers: `setInterval`/`setTimeout` scaling (the acceleration), `Date.now`,
//! `performance.now`, the `Date` constructor and its function form (`new Date()`, `Date()`, and a
//! subclass of `Date`, which stays an instance of itself), and `Intl.DateTimeFormat` formatting "now".
//! The zone is the host's - the instant is faked, not the local-time getters.

use super::CdpClient;
use serde_json::{json, Value};
use std::io;

/// The time shim, with `__MULT__`/`__DUR__`/`__FAKE_START__`/`__REAL_START__`/`__WALL_MAX__` filled
/// in by [`build_shim`]. A guard (`__chronomock`) keeps the originals wrapped exactly once: a second run
/// in the same document only sets the clock it carries, so of two new-document hooks the one added last
/// decides (R4-W5). `fakeNow` is
/// `fakeStart + (realNow - realStart) * M`, so M = 1 is a pure wall offset and M > 1 accelerates.
///
/// The clock (`M`/`fakeStart`/`realStart` and the duration anchor) lives in the mutable `__chronomock`
/// object, and every override reads it live, so the driver can change the rate or jump the wall in
/// flight by writing new values (slice C7) - the CDP equivalent of the native hook re-reading `Ctl`.
/// A rate change re-anchors the duration axis (`perfBase`/`perfAnchorReal`) so `performance.now` stays
/// continuous and never runs backward when the rate drops (untouchable rule 3). It cannot, however,
/// reschedule a `setInterval` already queued at the old rate - that stays at its old cadence (the
/// driver warns).
///
/// The wall and the duration axis have separate rates. `M` moves the wall (`Date`), `D` moves the
/// timers and `performance.now`. A Chromium session sets both to the multiplier. A page inside a
/// natively hooked application follows that application instead, whose duration axis scales only
/// under `scale_duration` (docs/09 section 12.6) - one rule for one application, whichever half of
/// it a timer runs in.
const SHIM_TEMPLATE: &str = r#"(function(){
  if (globalThis.__chronomock) {
    /* Installed already: by an older new-document script that ran first while this newer one was
       being registered (R4-W5), or by an earlier evaluate - this session's, or one that let the page go
       before this one reached it, which must not keep it on the clock it left behind. Scripts run in
       the order they were added, so the newest runs last and its clock stands - the duration axis
       re-anchored at the old rate first, as a rate change does, so performance.now never steps back. */
    var O = globalThis.__chronomock, p = O._realPerf ? O._realPerf() : 0;
    O.perfBase = (O.perfBase || 0) + (p - O.perfAnchorReal) * (O.D || 1); O.perfAnchorReal = p;
    O.M = __MULT__; O.D = __DUR__; O.fakeStart = __FAKE_START__; O.realStart = __REAL_START__; O.wallMax = __WALL_MAX__;
    return 'already';
  }
  var _OrigDate = Date;
  var _now = _OrigDate.now.bind(_OrigDate);
  /* Taken now, before the page's own scripts: a native Date() never calls the prototype's toString,
     so one the page replaced must not reach Date() either. */
  var _dateString = _OrigDate.prototype.toString;
  var _perf = (typeof performance !== 'undefined' && performance.now) ? performance.now.bind(performance) : null;
  var S = {
    M: __MULT__,                    /* wall rate: 0 = frozen, 1 = flow (wall offset only), N = accelerate */
    D: __DUR__,                     /* duration rate for timers and performance.now, never below 1 */
    fakeStart: __FAKE_START__,
    realStart: __REAL_START__,
    wallMax: __WALL_MAX__,          /* the last instant the session clock can hold - it stands there */
    perfBase: _perf ? _perf() : 0,  /* where performance.now stood when the shim arrived (R4-S16) */
    perfAnchorReal: _perf ? _perf() : 0,
    _realNow: _now,
    _realPerf: _perf,
    counts: { si: 0, st: 0, now: 0, date: 0, intl: 0, perf: 0 }
  };
  globalThis.__chronomock = S;
  function fakeNow(){ return Math.round(Math.min(S.fakeStart + (_now() - S.realStart) * S.M, S.wallMax)); }

  /* Replace Date so new Date() (no args), Date() and Date.now() read the session clock; every other
     form (parsing, explicit fields) is unchanged. Reflect.construct with new.target keeps a subclass
     (class X extends Date) an X - building a plain Date here dropped its prototype (R4-W6). */
  function CMDate() {
    if (!new.target) { S.counts.date++; return _dateString.call(new _OrigDate(fakeNow())); }
    if (arguments.length === 0) { S.counts.date++; return Reflect.construct(_OrigDate, [fakeNow()], new.target); }
    return Reflect.construct(_OrigDate, arguments, new.target);
  }
  CMDate.prototype = _OrigDate.prototype;
  CMDate.now = function(){ S.counts.now++; return fakeNow(); };
  CMDate.parse = _OrigDate.parse;
  CMDate.UTC = _OrigDate.UTC;
  /* The prototype is the original's, so its constructor has to point here, or new d.constructor()
     reads the real clock - and the name and arity are the native ones. */
  try { Object.defineProperty(_OrigDate.prototype, 'constructor', { value: CMDate, writable: true, configurable: true }); } catch (e) {}
  try { Object.defineProperty(CMDate, 'name', { value: 'Date' }); Object.defineProperty(CMDate, 'length', { value: 7 }); } catch (e) {}
  try { globalThis.Date = CMDate; } catch (e) { try { Date.now = CMDate.now; } catch (e2) {} }

  /* Intl.DateTimeFormat formats "now" when it is given no date, and reads that now itself - the real
     clock. format is a getter that hands out one bound function per formatter, so the wrapper is kept
     per formatter the same way. */
  var _DTF = globalThis.Intl && globalThis.Intl.DateTimeFormat;
  if (_DTF && _DTF.prototype) {
    var _fmtGet = (Object.getOwnPropertyDescriptor(_DTF.prototype, 'format') || {}).get;
    var _fmtOf = typeof WeakMap === 'function' ? new WeakMap() : null;
    if (_fmtGet && _fmtOf) {
      try {
        Object.defineProperty(_DTF.prototype, 'format', { configurable: true, get: function(){
          var f = _fmtOf.get(this);
          if (!f) {
            var real = _fmtGet.call(this);
            f = function(d){ if (d === undefined) { S.counts.intl++; d = fakeNow(); } return real(d); };
            _fmtOf.set(this, f);
          }
          return f;
        } });
      } catch (e) {}
    }
    var _ftp = _DTF.prototype.formatToParts;
    if (_ftp) {
      _DTF.prototype.formatToParts = function(d){ if (d === undefined) { S.counts.intl++; d = fakeNow(); } return _ftp.call(this, d); };
    }
  }

  /* setInterval/setTimeout read the duration rate (D || 1) live, so a NEW timer picks up the current
     rate; one already scheduled keeps its old cadence (the kernel already queued it). */
  var _si = globalThis.setInterval, _st = globalThis.setTimeout;
  if (_si) { globalThis.setInterval = function(fn, d){ S.counts.si++; var a = [].slice.call(arguments, 2); return _si.apply(globalThis, [fn, (d || 0) / (S.D || 1)].concat(a)); }; }
  if (_st) { globalThis.setTimeout = function(fn, d){ S.counts.st++; var a = [].slice.call(arguments, 2); return _st.apply(globalThis, [fn, (d || 0) / (S.D || 1)].concat(a)); }; }
  if (_perf) {
    performance.now = function(){ S.counts.perf++; return S.perfBase + (_perf() - S.perfAnchorReal) * (S.D || 1); };
  }
  return 'installed';
})()"#;

/// Read a context's per-API call counts (or `null` if the shim is not installed there). The counts
/// make an honest "covered means the app actually called it" report, the same way the native audit
/// counts channel queries - an override that was installed but never exercised is not "covered".
pub const COUNTS_EXPR: &str = "(globalThis.__chronomock && globalThis.__chronomock.counts) || null";

/// The APIs the shim counts, as (the name the report gives them, the key in the shim's `counts`).
/// `new Date` counts every read of the clock through the constructor, `new Date()` and `Date()` alike -
/// an app that reads the time only that way was reported as having called no time API at all (R4-S19).
/// `Intl.DateTimeFormat` counts a format of "now" (`format()` or `formatToParts()` with no date).
pub const COUNTED_APIS: [(&str, &str); 6] = [
    ("setInterval", "si"),
    ("setTimeout", "st"),
    ("Date.now", "now"),
    ("new Date", "date"),
    ("Intl.DateTimeFormat", "intl"),
    ("performance.now", "perf"),
];

/// Build the shim source for a session clock: `fake_start_ms`/`real_start_ms` are Unix-epoch ms, `mult`
/// the wall rate (0 freezes it) and `dur` the duration rate for timers and `performance.now` (never
/// below 1 - a frozen wall does not stop a timer, untouchable rule 3). The browser's own `Date.now`
/// supplies "real now" at run time, so all contexts share one clock origin as long as the driver's and
/// the browser's wall clocks agree (same machine).
///
/// `wall_max_ms` is the last instant the wall may show. The page's clock stands there, as the native
/// hook's does, instead of running on past what the session can name (R4-S8). A parameter rather than
/// a constant of this module, because this transport client knows nothing of the session's range.
pub fn build_shim(fake_start_ms: i64, real_start_ms: i64, mult: i64, dur: i64, wall_max_ms: i64) -> String {
    SHIM_TEMPLATE
        .replace("__MULT__", &mult.to_string())
        .replace("__DUR__", &dur.max(1).to_string())
        .replace("__FAKE_START__", &fake_start_ms.to_string())
        .replace("__REAL_START__", &real_start_ms.to_string())
        .replace("__WALL_MAX__", &wall_max_ms.to_string())
}

/// True for a CDP target type that runs the target's own JS (and so is worth shimming). GPU, browser,
/// and other infrastructure targets have no app timer to cover.
pub fn is_shimmable(target_type: &str) -> bool {
    matches!(
        target_type,
        "page" | "iframe" | "webview" | "worker" | "shared_worker" | "service_worker" | "dedicated_worker"
    )
}

/// Whether a CDP target type is a worker (vs a page/frame). Workers get the shim directly - pages also
/// cascade auto-attach so their own workers are reached.
pub fn is_worker(target_type: &str) -> bool {
    target_type.contains("worker")
}

/// Whether a context of this type can start workers of its own - the ones auto-attach set on it is
/// there to reach. A service worker cannot (`Worker` is not in its scope), so one that refuses
/// auto-attach leaves nothing on the real clock.
pub fn starts_workers(target_type: &str) -> bool {
    is_shimmable(target_type) && target_type != "service_worker"
}

/// What installing the shim into one context left behind.
pub struct Injected {
    /// A page's new-document hook, which [`renew_page_script`] needs to replace it when the clock
    /// moves. `None` for a worker, and for a page that answered without an identifier.
    pub script: Option<String>,
    /// Whether the context took auto-attach (see [`auto_attach_children`]).
    pub children: bool,
}

/// Install the shim into a page (or frame) session: as an add-script hook so every future document
/// gets it before its own scripts run, plus an immediate evaluate for the document already loaded.
/// Then cascade auto-attach so the page's Web Workers are attached and shimmed too.
pub fn inject_page(client: &mut CdpClient, session_id: &str, shim: &str) -> io::Result<Injected> {
    client.call("Page.enable", json!({}), Some(session_id)).ok();
    let added = client.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": shim }),
        Some(session_id),
    )?;
    let script = script_identifier(&added);
    evaluate_shim(client, session_id, shim)?;
    let children = auto_attach_children(client, session_id);
    client.call("Runtime.runIfWaitingForDebugger", json!({}), Some(session_id)).ok();
    Ok(Injected { script, children })
}

/// Install the shim into a worker session, before its script runs when the worker was paused on start
/// (waitForDebuggerOnStart), or immediately for a worker that is already alive but has not yet armed a
/// timer. Then release a paused worker so it proceeds with the overridden globals in place. A worker
/// has no new-document hook - one started later is a new target, shimmed from the clock of then.
pub fn inject_worker(client: &mut CdpClient, session_id: &str, shim: &str) -> io::Result<Injected> {
    evaluate_shim(client, session_id, shim)?;
    // A worker can start workers of its own, and auto-attach set on the page does not reach them: a
    // worker started by a worker read the real clock (R4-N25, measured on an Electron page). Set before
    // the worker is released, so one it starts in its first script is paused for the shim too.
    let children = auto_attach_children(client, session_id);
    client.call("Runtime.runIfWaitingForDebugger", json!({}), Some(session_id)).ok();
    Ok(Injected { script: None, children })
}

/// Ask a context to attach the workers it starts, paused for the shim. `false` when it answered with
/// an error or not at all: the context itself is shimmed, but a worker it starts would run on the
/// real clock unseen, and the caller has to count that (rule 4). Measured on Chromium 153: a page, a
/// dedicated worker, a nested one, a shared worker and a service worker all take it.
fn auto_attach_children(client: &mut CdpClient, session_id: &str) -> bool {
    client
        .call(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
            Some(session_id),
        )
        .is_ok()
}

/// The identifier `Page.addScriptToEvaluateOnNewDocument` answered with, if it gave one.
fn script_identifier(reply: &Value) -> Option<String> {
    reply.get("identifier").and_then(Value::as_str).map(str::to_string)
}

/// Replace a page's new-document hook with one built on the clock as it is now, and return the new
/// identifier. The hook carries its clock in its source, so one registered at attach handed every
/// document loaded after a jump or a rate change the clock from the attach - a reload after a jump to
/// 2031 came back in 2038 at the old rate (R4-W5, measured on an Electron page).
///
/// The new hook is added BEFORE the old one goes. A document starting in between runs both, in the
/// order they were added, and the shim's own guard lets the second set the clock - so no document
/// starts with no shim, nor with the old clock. A page that does not answer keeps its old hook, and
/// `None` comes back.
pub fn renew_page_script(client: &mut CdpClient, session_id: &str, old: Option<&str>, shim: &str) -> Option<String> {
    let added = client
        .call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": shim }), Some(session_id))
        .ok()?;
    let new = script_identifier(&added)?;
    if let Some(old) = old {
        client
            .call("Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": old }), Some(session_id))
            .ok();
    }
    Some(new)
}

/// Remove a page's new-document hook, so a document it loads after the session let it go starts on
/// the real clock. Best effort: a page that is gone has nothing left to remove.
pub fn remove_page_script(client: &mut CdpClient, session_id: &str, script: &str) {
    client
        .call("Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": script }), Some(session_id))
        .ok();
}

/// Evaluate the shim in a session's global context and surface a thrown exception as an error (the
/// shim must never fail silently - an uncovered context is an honest non-effect, not a hidden one).
fn evaluate_shim(client: &mut CdpClient, session_id: &str, shim: &str) -> io::Result<()> {
    let r = client.call(
        "Runtime.evaluate",
        json!({ "expression": shim, "returnByValue": true }),
        Some(session_id),
    )?;
    if let Some(exc) = r.get("exceptionDetails") {
        return Err(shim_error(exc));
    }
    Ok(())
}

/// The exception a target's JS engine reported for the shim, as an `io::Error`. Split out for the
/// reason `target_error` was: the text is written by the TARGET, so the one place that quotes it has
/// a name and a test.
///
/// Folded HERE, at the source, rather than at the one place that prints it today - the session loop
/// currently discards this error, and the obvious improvement (saying WHY a context could not be
/// shimmed) would carry the raw text into the report, which is evidence (untouchable rule 4).
fn shim_error(exception_details: &Value) -> io::Error {
    let text = exception_details
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("shim threw");
    io::Error::other(format!("shim evaluate failed: {}", super::sanitise_target_text(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shim runs inside the target's own JS engine, so the exception text it throws is the
    /// target's words. A newline there would add a line to output the tool did not write - the same
    /// property `target_error` protects for CDP errors, and the reason evidence stays trustworthy.
    #[test]
    fn a_shim_exception_cannot_forge_a_line() {
        let thrown = json!({ "text": "Uncaught\nchrono core: verdict: works" });
        let text = shim_error(&thrown).to_string();
        assert!(!text.contains('\n'), "target words reached the error raw: {text}");
        assert!(text.contains("Uncaught"), "the target's own words are still readable: {text}");

        // No `text` field at all is the ordinary shape of a malformed report, not a panic.
        assert!(shim_error(&json!({})).to_string().contains("shim threw"));
    }

    #[test]
    fn shim_substitutes_its_parameters() {
        let s = build_shim(1_700_000_000_000, 1_600_000_000_000, 60, 60, 900_000_000_000_000);
        assert!(s.contains("M: 60,"));
        assert!(s.contains("D: 60,"));
        assert!(s.contains("fakeStart: 1700000000000,"));
        assert!(s.contains("realStart: 1600000000000,"));
        assert!(s.contains("wallMax: 900000000000000,"));
        assert!(!s.contains("__MULT__"));
        assert!(!s.contains("__DUR__"));
        assert!(!s.contains("__FAKE_START__"));
        assert!(!s.contains("__WALL_MAX__"));
        // The wall is read through the end of the range, never past it (R4-S8).
        assert!(s.contains("Math.min(S.fakeStart + (_now() - S.realStart) * S.M, S.wallMax)"), "{s}");
    }

    /// The two rates are independent: a page inside a natively hooked application keeps its timers
    /// real while its wall runs fast (docs/09 section 12.6), and a frozen wall never stops a timer
    /// (untouchable rule 3), so the duration rate is floored at 1 whatever the caller passes.
    #[test]
    fn the_wall_rate_and_the_duration_rate_are_filled_in_separately() {
        let s = build_shim(0, 0, 60, 1, 0);
        assert!(s.contains("M: 60,"), "{s}");
        assert!(s.contains("D: 1,"), "{s}");
        assert!(s.contains("(S.D || 1)"), "timers read the duration rate, not the wall rate");
        assert!(!s.contains("/ (S.M || 1)"), "no timer divides by the wall rate any more");

        let frozen = build_shim(0, 0, 0, 0, 0);
        assert!(frozen.contains("M: 0,"), "{frozen}");
        assert!(frozen.contains("D: 1,"), "a frozen wall keeps timers at real speed: {frozen}");
    }

    /// R4-W5: a second run of the shim in one document - the newer of two new-document hooks, which
    /// runs last - sets the clock it carries instead of returning untouched, re-anchoring the duration
    /// axis at the old rate first so performance.now does not step back. The behaviour is measured
    /// in Node (tools/probes/r4-14/w5-test.mjs) and on a live page - this pins the source.
    #[test]
    fn a_second_run_sets_the_clock_it_carries() {
        let s = build_shim(1_000, 2_000, 1, 1, 3_000);
        let guard = &s[..s.find("return 'already';").expect("the guard returns early")];
        assert!(guard.contains("O.M = 1; O.D = 1; O.fakeStart = 1000; O.realStart = 2000; O.wallMax = 3000;"), "{guard}");
        assert!(guard.contains("O.perfBase = (O.perfBase || 0) + (p - O.perfAnchorReal) * (O.D || 1); O.perfAnchorReal = p;"));
        // The re-anchor comes before the new rate, or it would integrate the past at the new one.
        assert!(guard.find("O.perfAnchorReal = p").unwrap() < guard.find("O.D = 1").unwrap());
    }

    /// Every API the report names has its counter in the shim, and the shim counts nothing the report
    /// would drop - a key on one side only is a count that is never read or a row that is always zero.
    #[test]
    fn every_counted_api_has_its_counter_in_the_shim() {
        let start = SHIM_TEMPLATE.find("counts: {").expect("the shim declares its counters");
        let end = start + SHIM_TEMPLATE[start..].find('}').expect("the counters close");
        let declared: Vec<&str> = SHIM_TEMPLATE[start + "counts: {".len()..end]
            .split(',')
            .filter_map(|kv| kv.split(':').next().map(str::trim))
            .filter(|k| !k.is_empty())
            .collect();
        let reported: Vec<&str> = COUNTED_APIS.iter().map(|(_, key)| *key).collect();
        assert_eq!(declared, reported, "the shim's counters and the report's rows must match, in order");
    }

    /// R4-W6 and R4-S16 in the source the pages get. A subclass of Date is built with its own
    /// constructor, `Date()` reads the session clock, the prototype points back at the replacement,
    /// "now" formatted by Intl is the session's, and performance.now starts where it stood. The
    /// behaviour is measured in Node and on a live page (tools/probes/r4-14) - this pins the source.
    #[test]
    fn the_date_replacement_keeps_subclasses_and_the_clock_it_reports() {
        let s = build_shim(0, 0, 60, 60, 0);
        assert!(s.contains("Reflect.construct(_OrigDate, arguments, new.target)"), "a subclass keeps its prototype");
        assert!(s.contains("if (!new.target) { S.counts.date++; return _dateString.call(new _OrigDate(fakeNow())); }"));
        assert!(s.contains("Object.defineProperty(_OrigDate.prototype, 'constructor', { value: CMDate"));
        assert!(s.contains("if (d === undefined) { S.counts.intl++; d = fakeNow(); }"), "Intl formats the session's now");
        assert!(s.contains("perfBase: _perf ? _perf() : 0,"), "performance.now does not restart at 0");
    }

    /// A context that refused auto-attach is counted only when it can start workers of its own: a
    /// page or a dedicated or shared worker can, a service worker cannot, and a type that is never
    /// shimmed is never counted.
    #[test]
    fn only_a_context_that_can_start_workers_loses_them_by_refusing_auto_attach() {
        for ty in ["page", "iframe", "webview", "worker", "dedicated_worker", "shared_worker"] {
            assert!(starts_workers(ty), "{ty}");
        }
        assert!(!starts_workers("service_worker"));
        assert!(!starts_workers("browser"));
    }

    #[test]
    fn classifies_target_types() {
        assert!(is_shimmable("page"));
        assert!(is_shimmable("worker"));
        assert!(is_shimmable("service_worker"));
        assert!(!is_shimmable("browser"));
        assert!(!is_shimmable("other"));
        assert!(is_worker("dedicated_worker"));
        assert!(!is_worker("page"));
    }
}
