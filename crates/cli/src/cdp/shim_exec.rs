//! The time shim run, not read (R5-A10). Every test here builds the shim and the expressions a session
//! sends with the functions the session uses, puts them in a JS scope whose clocks the test drives
//! (`shim_harness.js`) and runs it with Node. The tests beside the shim check its source for substrings,
//! which a renamed field or a wrong sign goes through: W6 and S16 of the first report both did.
//!
//! Node is required. A machine without it fails these tests with a message rather than skipping them
//! - a test that is not run says nothing, and the CI image (`windows-latest`) carries Node.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{build_shim, release_expr, set_expr, zone_read_expr, ScheduledRate, COUNTS_EXPR};
use crate::cdp_clock::cdp_schedule_expr;

const HARNESS: &str = include_str!("shim_harness.js");

/// How long one scenario may run. A scenario finishes in well under a second, so one that runs this
/// long is a shim caught in a loop - it fails the test rather than hold the gate.
const SCENARIO_TIME: Duration = Duration::from_secs(30);

/// The harness's wall clock at the start, in Unix-epoch ms (`env.wall`).
const WALL0: i64 = 1_700_000_000_000;

/// 2038-01-19T03:14:07Z, the session's fake start in these tests.
const FAKE0: i64 = 2_147_483_647_000;

/// The last instant the wall may show, as the session passes it.
const WALL_MAX: i64 = 910_692_730_085_000;

/// A source text as a JS string literal.
fn js_text(source: &str) -> String {
    serde_json::to_string(source).expect("a string serialises")
}

/// Run `scenario` after the harness, with each of `vars` as a JS constant of that name (the value is
/// JS as written). Fails with what Node printed when a check failed, when Node ran no check at all, and
/// when Node is not there to run it.
fn run_scenario(name: &str, vars: &[(&str, String)], scenario: &str) {
    let mut js = String::from(HARNESS);
    for (var, value) in vars {
        js.push_str(&format!("const {var} = {value};\n"));
    }
    js.push_str(scenario);
    js.push_str("\nfinish();\n");
    let dir = std::env::temp_dir().join(format!("chrono-shim-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch folder for the scenario");
    let file = dir.join(format!("{name}.js"));
    std::fs::write(&file, js).expect("the scenario written");
    let spawned = Command::new("node").arg(&file).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = spawned.unwrap_or_else(|e| {
        panic!("node is needed for the shim's execution tests (R5-A10) and could not be started: {e} - install Node.js or put it on PATH")
    });
    // Both streams read while the scenario runs, so a full pipe cannot stall it.
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + SCENARIO_TIME;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let _ = std::fs::remove_file(&file);
    let text = format!("{}{}", stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    let status = status.unwrap_or_else(|| panic!("{name} ran past {SCENARIO_TIME:?} and was stopped:\n{text}"));
    assert!(status.success(), "{name}:\n{text}");
    assert!(text.contains("checks ") && !text.contains("checks 0,"), "{name} ran no check:\n{text}");
}

/// The shim of a session at x`mult` with the duration axis at `dur`, started at `FAKE0` on the
/// harness's wall, no change waiting, no zone.
fn shim(mult: i64, dur: i64) -> String {
    js_text(&build_shim(FAKE0, WALL0, mult, dur, None, WALL_MAX, None))
}

fn numbers() -> Vec<(&'static str, String)> {
    vec![("WALL0", WALL0.to_string()), ("FAKE0", FAKE0.to_string())]
}

/// The clocks a page reads after the shim: the wall at the session rate from the session's start,
/// `Date`, `new Date()`, `Date()` and a subclass of `Date` alike, a date built from a date's own
/// constructor too (R4-W6), a date given as one value or several left alone (R5-WY3 builds the one-value
/// date directly), `Date()` in the engine's own words even when the page replaced the prototype's
/// `toString`, "now" formatted by `Intl` the session's, whole or in parts, `performance.now` at the
/// duration rate from where it stood (R4-S16), and a timer's delay divided by it.
#[test]
fn the_page_reads_the_session_clock_through_every_form_of_date() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    run_scenario(
        "clocks",
        &vars,
        r"
        const p0 = performance.now();
        const answer = run(SHIM);
        check('a worker answers installed', answer === 'installed', answer);
        check('Date.now is the session clock at install', Date.now() === FAKE0, Date.now());
        check('performance.now goes on from where it stood', performance.now() === p0, performance.now() - p0);
        advance(1000);
        check('a real second is a minute at x60', Date.now() === FAKE0 + 60000, Date.now() - FAKE0);
        check('new Date() reads it', new Date().getTime() === Date.now());
        check('Date() reads it', Date() === new EngineDate(Date.now()).toString());
        check('a date names the session Date as its constructor', new Date().constructor === Date);
        check('and a date built from that constructor reads it', new (new Date().constructor)().getTime() === Date.now());
        // A native Date() never calls the prototype's toString, so a page's own does not reach it.
        const engineText = Date.prototype.toString;
        Date.prototype.toString = function() { return 'the page own text'; };
        const said = Date();
        Date.prototype.toString = engineText;
        check('Date() is the engine text, not one the page put on the prototype', said === new EngineDate(Date.now()).toString(), said);
        class Sub extends Date {}
        check('a subclass of Date stays itself', new Sub() instanceof Sub && new Sub().getTime() === Date.now());
        check('a subclass given a value stays itself', new Sub(5) instanceof Sub && new Sub(5).getTime() === 5);
        check('a date given as a number is left alone', new Date(5).getTime() === 5 && new Date(5) instanceof Date);
        check('a date given as text is left alone', new Date('2020-01-02T00:00:00Z').getTime() === 1577923200000);
        check('a date given fields is left alone', new Date(2020, 0, 2).getFullYear() === 2020);
        check('new Date(undefined) is an invalid date, as natively', Number.isNaN(new Date(undefined).getTime()));
        check('Date.name and Date.length are the native ones', Date.name === 'Date' && Date.length === 7);
        const year = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', year: 'numeric' }).format();
        check('Intl formats the session clock for now', year === '2038', year);
        const parts = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', year: 'numeric' }).formatToParts();
        check('and in parts', parts.some((p) => p.type === 'year' && p.value === '2038'), parts);
        check('performance.now runs at the duration rate', near(performance.now() - p0, 60000, 0.001), performance.now() - p0);
        const id = setTimeout(() => {}, 6000);
        check('a timer waits its delay divided by the rate', env.timers.find((t) => t.id === id).delay === 100);
        ",
    );
}

/// A frozen wall stands, and the duration axis runs at real speed under it (untouchable rule 3).
#[test]
fn a_frozen_wall_stands_while_the_duration_axis_runs() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(0, 1)));
    run_scenario(
        "frozen",
        &vars,
        r"
        run(SHIM);
        const p0 = performance.now();
        advance(5000);
        check('the wall stands', Date.now() === FAKE0, Date.now() - FAKE0);
        check('performance.now runs at real speed', near(performance.now() - p0, 5000, 0.001), performance.now() - p0);
        ",
    );
}

/// The wall and the duration axis have separate rates: a page inside a natively hooked application keeps
/// its timers real while its wall runs fast (docs/09 section 12.6), and a timer at the rate 1 goes to the
/// engine as the page gave it.
#[test]
fn a_timer_keeps_its_delay_when_only_the_wall_runs_fast() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 1)));
    run_scenario(
        "walltimers",
        &vars,
        r"
        run(SHIM);
        advance(1000);
        check('the wall runs at x60', Date.now() === FAKE0 + 60000, Date.now() - FAKE0);
        const id = setTimeout(() => {}, '250');
        check('a one-time timer at the rate 1 keeps its delay', env.timers.find((t) => t.id === id).delay === 250);
        let ticks = 0;
        setInterval(() => { ticks++; }, 100);
        advance(1000);
        check('a repeating one keeps its cadence', ticks === 10, ticks);
        ",
    );
}

/// What the session sends moves the clock as the session means it: a jump moves the wall and leaves
/// `performance.now` continuous, a release puts the wall back on the real clock and starts the audit
/// over, the counts come back as numbers - `Date()` counted with `new Date()` (R4-S19) - and the zone
/// read answers two offsets through the engine's own `Date`, whatever the page put in its place (ADR-21).
#[test]
fn the_session_moves_counts_and_lets_go_of_the_page() {
    let fake1 = FAKE0 + 86_400_000;
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("FAKE1", fake1.to_string()));
    vars.push(("JUMP", js_text(&set_expr(fake1, WALL0, 60, 60, None))));
    vars.push(("RELEASE", js_text(&release_expr())));
    vars.push(("COUNTS", js_text(COUNTS_EXPR)));
    vars.push(("ZONE_READ", js_text(&zone_read_expr([WALL0, WALL0 + 15_552_000_000]))));
    run_scenario(
        "moves",
        &vars,
        r"
        run(SHIM);
        Date.now(); Date.now(); new Date(); Date(); performance.now();
        const counts = run(COUNTS);
        check('the counts come back', counts.now === 2 && counts.date === 2 && counts.perf === 1, counts);
        check('a page with no frames reached none', counts.frames === 0 && counts.capped === 0, counts);
        const p0 = performance.now();
        check('a jump answers ok', run(JUMP) === 'ok');
        check('a jump moves the wall', Date.now() === FAKE1 + (env.wall - WALL0) * 60, Date.now() - FAKE1);
        check('a jump leaves performance.now where it was', near(performance.now(), p0, 0.001), performance.now() - p0);
        advance(1000);
        check('the jumped clock runs at the rate', Date.now() === FAKE1 + (env.wall - WALL0) * 60);
        const p1 = performance.now();
        check('the release answers ok', run(RELEASE) === 'ok');
        // Read before anything else calls the clock: every read after the release counts again.
        const after = run(COUNTS);
        check('the release starts the audit over', after.now === 0 && after.date === 0 && after.perf === 0, after);
        check('the released wall is the real one', Date.now() === env.wall, Date.now() - env.wall);
        check('performance.now never steps back at the release', performance.now() >= p1, performance.now() - p1);
        advance(1000);
        check('and runs at real speed after it', near(performance.now() - p1, 1000, 0.001), performance.now() - p1);
        const zone = run(ZONE_READ);
        check('the zone read answers two offsets', Array.isArray(zone) && zone.length === 2 && zone.every(Number.isFinite), zone);
        check('through the engine own Date', zone[0] === new EngineDate(WALL0).getTimezoneOffset(), zone);
        const sessionDate = globalThis.Date;
        globalThis.Date = function PageDate() { return { getTimezoneOffset() { return 7777; } }; };
        const replaced = run(ZONE_READ);
        globalThis.Date = sessionDate;
        check('a page that replaced Date does not answer for the engine', replaced[0] === zone[0] && replaced[1] === zone[1], replaced);
        ",
    );
}

/// A rate change scheduled for an instant takes effect at that instant, with the wall continuous across
/// it (R4-S17), the duration axis re-anchored at the rate it ran at and never back below a reading
/// already handed out (untouchable rule 3), and one received after its instant takes effect at once and
/// says `late`. It takes effect whichever clock is read first past the instant - `performance.now` and a
/// timer's delay as much as the wall - and the duration axis does not step back when a correction of the
/// system clock carries the wall past the instant while `performance.now` has not got there, and runs at
/// the new rate from there.
#[test]
fn a_scheduled_rate_change_takes_effect_at_its_instant() {
    let on_time = cdp_schedule_expr(ScheduledRate { at_ms: WALL0 + 500, mult: 1, dur: 1 });
    let late = cdp_schedule_expr(ScheduledRate { at_ms: WALL0, mult: 60, dur: 60 });
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("ON_TIME", js_text(&on_time)));
    vars.push(("LATE", js_text(&late)));
    run_scenario(
        "schedule",
        &vars,
        r"
        run(SHIM);
        const p0 = performance.now();
        check('a change ahead answers ok', run(ON_TIME) === 'ok');
        advance(400);
        check('until its instant the old rate', Date.now() === FAKE0 + 400 * 60, Date.now() - FAKE0);
        const p400 = performance.now();
        check('the duration axis at the old rate before it', near(p400 - p0, 400 * 60, 0.001), p400 - p0);
        // First read 200 ms past the instant: the new rate counts from the instant, not from the read.
        advance(300);
        check('from its instant the new rate', Date.now() === FAKE0 + 500 * 60 + 200, Date.now() - FAKE0);
        const p700 = performance.now();
        check('the duration axis at the new rate from its instant', near(p700 - p0, 500 * 60 + 200, 0.001), p700 - p0);
        check('a change after its instant says late', run(LATE) === 'late');
        check('and never steps the duration axis back', performance.now() >= p700, performance.now() - p700);
        ",
    );
    let mut first_perf = numbers();
    first_perf.push(("SHIM", shim(60, 60)));
    first_perf.push(("ON_TIME", js_text(&on_time)));
    run_scenario(
        "schedulefirstperf",
        &first_perf,
        r"
        run(SHIM);
        const p0 = performance.now();
        run(ON_TIME);
        // Nothing reads the wall: performance.now is the first clock read past the instant.
        advance(700);
        const p = performance.now() - p0;
        check('performance.now read first is at the new rate from the instant', near(p, 500 * 60 + 200, 0.001), p);
        ",
    );
    let mut first_timer = numbers();
    first_timer.push(("SHIM", shim(60, 60)));
    first_timer.push(("ON_TIME", js_text(&on_time)));
    run_scenario(
        "schedulefirsttimer",
        &first_timer,
        r"
        run(SHIM);
        run(ON_TIME);
        advance(700);
        const id = setTimeout(() => {}, 6000);
        check('a timer armed first past the instant waits at the new rate', env.timers.find((t) => t.id === id).delay === 6000);
        ",
    );
    let mut drifting = numbers();
    drifting.push(("SHIM", shim(60, 60)));
    drifting.push(("ON_TIME", js_text(&on_time)));
    run_scenario(
        "scheduledrift",
        &drifting,
        r"
        run(SHIM);
        run(ON_TIME);
        advance(499);
        const before = performance.now();
        // The wall is corrected 20 ms on, past the instant, while performance.now moves 5.
        drift(20, 5);
        const after = performance.now();
        check('performance.now does not step back when the wall runs ahead of it', after >= before, after - before);
        advance(1000);
        const step = performance.now() - after;
        check('and runs at the new rate from there', near(step, 1000, 0.001), step);
        ",
    );
}

/// A second install in the same document only sets the clock it carries, the last instant, the zone and
/// a change still to come included (R4-W5), a `__chronomock` that is not this shim makes the install
/// fail rather than report a covered context (rule 4) and is left as the page made it, and a page whose
/// own scripts are in it before the shim answers `running` (R5-P7).
#[test]
fn a_second_install_a_foreign_marker_and_a_running_page_are_told_apart() {
    let wall_max2 = FAKE0 + 5_000;
    let second = build_shim(FAKE0, WALL0, 60, 60, None, wall_max2, Some(-60));
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("SECOND", js_text(&second)));
    vars.push(("WALL_MAX2", wall_max2.to_string()));
    vars.push(("COUNTS", js_text(COUNTS_EXPR)));
    run_scenario(
        "installs",
        &vars,
        r"
        run(SHIM);
        advance(1000);
        check('a second install answers already', run(SECOND) === 'already');
        check('and the clock stands at the last instant it carried', Date.now() === WALL_MAX2, Date.now() - FAKE0);
        const offset = new EngineDate(FAKE0).getTimezoneOffset();
        check('and the zone it carried is the one judged', run(COUNTS).zone === (offset === -60 ? 0 : 1), offset);
        ",
    );
    let carried = build_shim(FAKE0, WALL0, 60, 60, Some(ScheduledRate { at_ms: WALL0 + 2_000, mult: 1, dur: 1 }), WALL_MAX, None);
    let mut change = numbers();
    change.push(("SHIM", shim(60, 60)));
    change.push(("CARRIED", js_text(&carried)));
    run_scenario(
        "secondchange",
        &change,
        r"
        run(SHIM);
        check('a second install answers already', run(CARRIED) === 'already');
        advance(3000);
        check('and the change it carried takes effect at its instant', Date.now() === FAKE0 + 2000 * 60 + 1000, Date.now() - FAKE0);
        ",
    );
    let mut foreign = numbers();
    foreign.push(("SHIM", shim(60, 60)));
    run_scenario(
        "foreign",
        &foreign,
        r"
        globalThis.__chronomock = { counts: {} };
        let threw = false;
        try { run(SHIM); } catch (e) { threw = true; }
        check('a foreign marker makes the install fail', threw);
        check('and nothing is replaced', Date === EngineDate);
        const left = Object.keys(globalThis.__chronomock);
        check('and the object under the name is left as the page made it', left.join() === 'counts', left);
        ",
    );
    let mut page = numbers();
    page.push(("SHIM", shim(60, 60)));
    run_scenario(
        "page",
        &page,
        r"
        globalThis.document = { scripts: { length: 2 }, hidden: false, addEventListener() {} };
        check('a page with its scripts in it answers running', run(SHIM) === 'running');
        ",
    );
    let mut blank = numbers();
    blank.push(("SHIM", shim(60, 60)));
    run_scenario(
        "blank",
        &blank,
        r"
        globalThis.document = { scripts: { length: 0 }, hidden: false, addEventListener() {} };
        check('a page with none answers installed', run(SHIM) === 'installed');
        ",
    );
}

/// R5-B8: the clock is closed over. `__chronomock` is not among the page's own globals, cannot be
/// replaced or deleted, is frozen, and hands out methods and nothing else. A shim of an older build left
/// in a page, with its clock in open fields, is still moved, let go and read by this build.
#[test]
fn the_clock_is_closed_over_and_its_marker_cannot_be_moved() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    run_scenario(
        "closed",
        &vars,
        r"
        run(SHIM);
        const m = globalThis.__chronomock;
        const d = Object.getOwnPropertyDescriptor(globalThis, '__chronomock');
        check('not among the enumerable globals', !Object.keys(globalThis).includes('__chronomock'));
        check('neither writable, enumerable nor configurable', !d.writable && !d.enumerable && !d.configurable, d);
        check('frozen', Object.isFrozen(m));
        check('only the methods', Object.keys(m).sort().join() === 'audit,release,schedule,set,zoneRead', Object.keys(m));
        try { globalThis.__chronomock = {}; } catch (e) {}
        try { delete globalThis.__chronomock; } catch (e) {}
        try { m.set = null; } catch (e) {}
        check('it stays', globalThis.__chronomock === m && typeof m.set === 'function');
        advance(1000);
        check('and the clock with it', Date.now() === FAKE0 + 60000, Date.now() - FAKE0);
        ",
    );
    let wall_max2 = FAKE0 + 7_000;
    let mut old = numbers();
    old.push(("SECOND", js_text(&build_shim(FAKE0, WALL0, 60, 60, None, wall_max2, Some(-60)))));
    old.push(("RELEASE", js_text(&release_expr())));
    old.push(("ZONE_READ", js_text(&zone_read_expr([1, 2]))));
    old.push(("WALL_MAX2", wall_max2.to_string()));
    run_scenario(
        "older",
        &old,
        r"
        const calls = [];
        // An older build's shim: its clock in open fields, a set of five arguments, no release.
        globalThis.__chronomock = {
          wallMax: 0, Z: null, zoneMissed: 1, hiddenFast: 1,
          counts: { si: 1, st: 1, now: 3, date: 1, intl: 1, perf: 1 },
          _Date: { prototype: {} },
          set(...args) { calls.push(args); return 'ok'; },
        };
        const o = globalThis.__chronomock;
        o._Date = function(v) { this.v = v; };
        o._Date.prototype.getTimezoneOffset = function() { return 7; };
        check('a second install over it answers already', run(SECOND) === 'already');
        check('and gives it the last instant and the zone in its fields', o.wallMax === WALL_MAX2 && o.Z === -60, o);
        check('and the clock through its set', calls.length === 1 && calls[0][0] === FAKE0 && calls[0][2] === 60, calls);
        check('the release answers ok', run(RELEASE) === 'ok');
        check('and zeroes its open fields', o.Z === null && o.zoneMissed === 0 && o.hiddenFast === 0 && o.counts.now === 0, o);
        check('and puts it on the real clock', JSON.stringify(calls[1].slice(0, 5)) === '[0,0,1,1,null]', calls[1]);
        check('its zone is read through the Date it kept', JSON.stringify(run(ZONE_READ)) === '[7,7]', run(ZONE_READ));
        ",
    );
}

/// R5-P6: a one-time timer's delay is rounded, not cut (1000 / 60 went to the engine as 16 and ran 4%
/// fast). A repeating one is a chain that keeps its cadence on average, takes a change of speed from its
/// next tick, makes no tick up, and is stopped by either clear from anywhere - inside its own callback
/// too - while a callback that throws does not stop it.
#[test]
fn a_repeating_timer_keeps_its_cadence_and_takes_a_new_rate_from_its_next_tick() {
    let fake1 = FAKE0 + 1_000_000;
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("SLOW", js_text(&set_expr(fake1, WALL0, 1, 1, None))));
    run_scenario(
        "intervals",
        &vars,
        r"
        run(SHIM);
        const once = setTimeout(() => {}, 1000);
        check('a one-time delay is rounded', env.timers.find((t) => t.id === once).delay === 17);
        clearTimeout(once);
        let ticks = 0, self = null, got = null;
        const id = setInterval(function(a, b) { ticks++; self = this; got = [a, b]; }, 1000, 'x', 'y');
        advance(1000);
        check('sixty ticks in a real second at x60, none lost to the cut', ticks === 60, ticks);
        check('the callback gets its arguments', got[0] === 'x' && got[1] === 'y', got);
        check('and the global object for this', self === globalThis);
        const before = waiting().due;
        check('the speed changes', run(SLOW) === 'ok');
        check('the tick already waiting keeps its time', waiting().due === before);
        advance(before - env.perf);
        const t1 = ticks;
        advance(999);
        check('the next tick waits a whole real second at x1', ticks === t1, ticks - t1);
        advance(1);
        check('and comes then', ticks === t1 + 1, ticks - t1);
        // Nothing runs for ten seconds: one late tick, then the cadence from now, none made up.
        stall(10000);
        advance(0);
        const t2 = ticks;
        check('a late tick runs once', t2 === t1 + 2, t2 - t1);
        check('and the next is a whole step from now', near(waiting().due - env.perf, 1000, 1), waiting().due - env.perf);
        clearTimeout(id);
        advance(5000);
        check('clearTimeout stops a repeating timer', ticks === t2, ticks - t2);
        let inside = 0;
        const own = setInterval(() => { inside++; clearInterval(own); }, 100);
        advance(5000);
        check('clearInterval inside its own callback stops it', inside === 1, inside);
        let thrown = 0;
        const loud = setInterval(() => { thrown++; throw new Error('boom'); }, 100);
        advance(350);
        check('a callback that throws does not stop it', thrown === 3, thrown);
        check('the throw is the engine reports', env.errors.length === 3, env.errors);
        clearInterval(loud);
        const text = setInterval('globalThis.textTicks = (globalThis.textTicks || 0) + 1', 100);
        advance(301);
        check('a text handler keeps the engine own repeating timer', globalThis.textTicks === 3, globalThis.textTicks);
        clearInterval(text);
        advance(1000);
        check('and its clear', globalThis.textTicks === 3, globalThis.textTicks);
        ",
    );
}

/// R5-P6: a timer that would have to run more often than every 4 ms at the session's speed, armed past
/// the nesting level the engine floors at (HTML: past 5, measured on Electron 44 from the seventh), is
/// noted in the audit - a repeating one from its seventh tick, a one-time chain from its seventh link.
/// Not at the rate 1, not for a delay of 0 the page asked for, and not for a timer the floor leaves alone.
#[test]
fn a_timer_the_four_ms_floor_holds_back_is_noted() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("COUNTS", js_text(COUNTS_EXPR)));
    vars.push(("SLOW", js_text(&set_expr(FAKE0, WALL0, 1, 1, None))));
    run_scenario(
        "floor",
        &vars,
        r"
        run(SHIM);
        let ticks = 0;
        const fast = setInterval(() => { ticks++; }, 100);
        while (ticks < 5) { advance(1); }
        check('five ticks in, not yet', run(COUNTS).capped === 0, ticks);
        while (ticks < 6) { advance(1); }
        check('from the seventh tick it is', run(COUNTS).capped === 1, ticks);
        // Under the floor the engine's own repeating timer keeps the cadence: one tick every 4 ms.
        const t6 = ticks;
        check('held by the engine own repeating timer', waiting().repeat === true, waiting());
        advance(400);
        check('held at the floor, one tick every 4 ms', ticks - t6 === 100, ticks - t6);
        run(SLOW);
        const t7 = ticks;
        advance(4);
        check('a change of speed is found at the next tick', ticks === t7 + 1, ticks - t7);
        check('and handed back to the chain', waiting().repeat === false && env.timers.length === 1, env.timers);
        advance(99);
        check('and the chain takes it from there', ticks === t7 + 1, ticks - t7);
        advance(1);
        check('at the new step', ticks === t7 + 2, ticks - t7);
        clearInterval(fast);
        advance(1000);
        check('and its clear stops it', ticks === t7 + 2, ticks - t7);
        ",
    );
    let mut chain = numbers();
    chain.push(("SHIM", shim(60, 60)));
    chain.push(("COUNTS", js_text(COUNTS_EXPR)));
    run_scenario(
        "floorchain",
        &chain,
        r"
        run(SHIM);
        let links = 0;
        const next = () => { links++; if (links < 10) { setTimeout(next, 100); } };
        setTimeout(next, 100);
        while (links < 5) { advance(1); }
        check('five links in, not yet', run(COUNTS).capped === 0, links);
        while (links < 6) { advance(1); }
        check('the sixth arms the seventh, which is', run(COUNTS).capped === 1, links);
        ",
    );
    let mut calm = numbers();
    calm.push(("SHIM", shim(60, 60)));
    calm.push(("FLOW", js_text(&build_shim(FAKE0, WALL0, 1, 1, None, WALL_MAX, None))));
    calm.push(("COUNTS", js_text(COUNTS_EXPR)));
    run_scenario(
        "floorcalm",
        &calm,
        r"
        run(SHIM);
        const zero = setInterval(() => {}, 0);
        const slow = setInterval(() => {}, 1000);
        advance(100);
        check('a delay of 0 and one past the floor are not noted', run(COUNTS).capped === 0);
        clearInterval(zero); clearInterval(slow);
        ",
    );
    run_scenario(
        "floorflow",
        &calm,
        r"
        run(FLOW);
        const fast = setInterval(() => {}, 1);
        advance(100);
        check('at the rate 1 nothing is noted', run(COUNTS).capped === 0);
        clearInterval(fast);
        ",
    );
}

/// R5-P8: `performance.timeOrigin` plus `performance.now()` is the session clock, at the start, as it
/// runs, and after a jump - the idiom a page builds "now" from.
#[test]
fn time_origin_plus_now_is_the_session_clock() {
    let fake1 = FAKE0 + 86_400_000;
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("JUMP", js_text(&set_expr(fake1, WALL0, 60, 60, None))));
    run_scenario(
        "origin",
        &vars,
        r"
        run(SHIM);
        const sum = () => performance.timeOrigin + performance.now();
        check('at the start', near(sum(), Date.now(), 1), sum() - Date.now());
        advance(1234);
        check('as it runs', near(sum(), Date.now(), 1), sum() - Date.now());
        run(JUMP);
        advance(10);
        check('after a jump', near(sum(), Date.now(), 1), sum() - Date.now());
        check('a getter, read each time', typeof Object.getOwnPropertyDescriptor(performance, 'timeOrigin').get === 'function');
        ",
    );
}

/// R5-P5: the session's commands reach a page's own document, and a frame of it is another one. The
/// moves, the release and the audit are handed down to every frame the page reaches - a frame in a frame
/// too, each in a realm of its own - and the audit counts their calls in the page's row and says how many
/// it reached. A frame of another origin, one with no shim and one with a shim of an older build are not
/// reached, and nothing below a frame of another origin is looked for.
#[test]
fn the_moves_the_audit_and_the_release_reach_the_frames_of_the_page() {
    let fake1 = FAKE0 + 86_400_000;
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("JUMP", js_text(&set_expr(fake1, WALL0, 60, 60, None))));
    vars.push(("SLOW", js_text(&cdp_schedule_expr(ScheduledRate { at_ms: WALL0 + 2_000, mult: 1, dur: 1 }))));
    vars.push(("RELEASE", js_text(&release_expr())));
    vars.push(("COUNTS", js_text(COUNTS_EXPR)));
    vars.push(("FAKE1", fake1.to_string()));
    run_scenario(
        "frames",
        &vars,
        r"
        asWindow(globalThis);
        const same = frame(), inner = frame(), bare = frame(), old = frame();
        hold(globalThis, same.g);
        hold(same.g, inner.g);
        hold(globalThis, foreignFrame());
        hold(globalThis, bare.g);
        hold(globalThis, old.g);
        old.run('globalThis.__chronomock = { set() { return 1; }, audit() { return {}; } }');
        // The page's new-document hook gives every frame of its process the shim (measured).
        run(SHIM); same.run(SHIM); inner.run(SHIM);
        same.run('Date.now(); Date.now()'); inner.run('Date.now()');
        const audit = run(COUNTS);
        check('the frames count in the page row', audit.now === 3, audit);
        check('and the audit says how many it reached', audit.frames === 2, audit);
        // window.length is the page's to replace, and its number decides nothing (CodeRabbit on #111).
        globalThis.length = 0;
        check('a page that sets length to 0 does not hide its frames', run(COUNTS).frames === 2);
        globalThis.length = 1e9;
        check('nor does one that sets it past every frame there can be', run(COUNTS).frames === 2);
        // Where the engine's getter could not be taken - a plain field, as in this frame - the count is
        // still held under the ceiling, so the audit does not loop a billion times.
        same.run('globalThis.length = 1e9');
        check('a frame with a page-chosen length is looked at in time', run(COUNTS).frames === 2);
        check('a jump answers ok', run(JUMP) === 'ok');
        advance(1000);
        const expected = FAKE1 + (env.wall - WALL0) * 60;
        check('the jump reaches a frame', same.run('Date.now()') === expected, same.run('Date.now()') - expected);
        check('and a frame in a frame', inner.run('Date.now()') === expected, inner.run('Date.now()') - expected);
        check('a frame with no shim keeps the real clock', bare.run('Date.now()') === env.wall);
        check('a rate change reaches them', run(SLOW) === 'ok');
        advance(2000);
        check('at its instant in a frame too', same.run('Date.now()') === Date.now() && inner.run('Date.now()') === Date.now());
        check('the release answers ok', run(RELEASE) === 'ok');
        // Read before anything else calls the clock: every read after the release counts again.
        const after = run(COUNTS);
        check('their audit starts over too', after.now === 0 && after.frames === 2, after);
        check('and lets the frames go', same.run('Date.now()') === env.wall && inner.run('Date.now()') === env.wall);
        ",
    );
}

/// A page hidden while its timers run faster is noted - on install, when its visibility changes, and on
/// every audit - and not at the rate 1 (R4-N28). The zone a context reads is judged against the session's
/// at install and at every audit, a session with no zone judges none, and a page let go is judged no
/// more: what it reads after the release is no evidence about the session's zone (CodeRabbit on #89).
#[test]
fn the_audit_notes_a_hidden_page_and_a_zone_it_did_not_read() {
    let mut vars = numbers();
    vars.push(("SHIM", shim(60, 60)));
    vars.push(("COUNTS", js_text(COUNTS_EXPR)));
    run_scenario(
        "hidden",
        &vars,
        r"
        let listener = null;
        globalThis.document = { scripts: { length: 0 }, hidden: false, addEventListener(type, fn) { if (type === 'visibilitychange') { listener = fn; } } };
        run(SHIM);
        check('a shown page is not noted', run(COUNTS).hidden === 0);
        document.hidden = true;
        listener();
        check('hidden while fast it is', run(COUNTS).hidden === 1);
        ",
    );
    let mut flow = numbers();
    flow.push(("FLOW", js_text(&build_shim(FAKE0, WALL0, 1, 1, None, WALL_MAX, None))));
    flow.push(("COUNTS", js_text(COUNTS_EXPR)));
    run_scenario(
        "hiddenflow",
        &flow,
        r"
        globalThis.document = { scripts: { length: 0 }, hidden: true, addEventListener() {} };
        run(FLOW);
        check('hidden at the rate 1 it is not', run(COUNTS).hidden === 0);
        ",
    );
    // The zone is the session's, handed to the shim through `set` as a second install hands it, so the
    // test reads the same either side of UTC.
    let mut zones = numbers();
    zones.push(("FLOW", js_text(&build_shim(FAKE0, WALL0, 1, 1, None, WALL_MAX, None))));
    zones.push(("COUNTS", js_text(COUNTS_EXPR)));
    zones.push(("RELEASE", js_text(&release_expr())));
    run_scenario(
        "zones",
        &zones,
        r"
        run(FLOW);
        const here = new EngineDate(FAKE0).getTimezoneOffset();
        check('a session with no zone judges none', run(COUNTS).zone === 0);
        globalThis.__chronomock.set(FAKE0, WALL0, 1, 1, null, undefined, here);
        check('a context that reads the session zone is not noted', run(COUNTS).zone === 0, here);
        globalThis.__chronomock.set(FAKE0, WALL0, 1, 1, null, undefined, here + 60);
        check('one that reads another zone is', run(COUNTS).zone === 1, here);
        // An offset no engine shows, so the page on the real clock can never read it by chance.
        globalThis.__chronomock.set(FAKE0, WALL0, 1, 1, null, undefined, 9999);
        run(RELEASE);
        check('a page let go is judged against no zone', run(COUNTS).zone === 0);
        ",
    );
}
