//! The Chromium/Electron session: the second substitution mechanism, beside the native one.
//!
//! Where the native path injects a DLL and hooks 36 time channels, this one drives a debug port and
//! evaluates a time shim in every JS context (ADR-8, ADR-9). It speaks the same protocol and returns
//! the same verdicts, so the interface cannot tell which mechanism ran - only the coverage report can.
//!
//! Same boundary as `cdp_probe.rs`: `cdp/` is the transport client, this is the product using it.


use std::io::BufReader;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono_proto::{
    Command, Event, FollowedProcess, TargetSpec, TimeSpec,
    PROTOCOL_VERSION,
};

use crate::cdp;
use crate::output::diag;
use crate::zone::{epoch_ms_to_wall, now_epoch_ms};
use crate::events::{
    command_id, emit, ended_after_launch, ended_clean, unsupported_command,
};
use crate::wire::spawn_command_reader;
use crate::cdp_attach::{Attacher, Pumped, END_WAIT};
use crate::cdp_clock::{cdp_resolve_jump, cdp_schedule_expr, cdp_set_expr, CdpClock};
use crate::cdp_audit::{
    covered_channels, coverage_events, cdp_verdict, session_warnings,
    verdict_keys, SessionFacts,
};

/// A Chromium/Electron session driven over the Chrome DevTools Protocol, speaking the SAME machine
/// protocol as the native core (ADR-9). It is the inverse of the old human-report `driver_run_cdp`:
/// launch + shim + audit, but every outcome travels as `state`/`coverage`/`session_verdict`/`ended`,
/// so the GUI and CLI consume a CDP session exactly like a native one. Interactive commands (`query`
/// now, `set_multiplier`/`jump` in slice C7) arrive on the same stdin the native session reads.
///
/// `--ticks` is a DRIVER concern (the driver counts `state` heartbeats and sends `end`), so this loop
/// runs until `end`, stdin EOF, or the app closing - exactly like `run_session`. The app closing is
/// its debugging connection closing, not the launched process ending: a launcher that hands the
/// application over and exits leaves the browser, and the session, running (R4-S18).
pub(crate) fn cdp_session(target: TargetSpec, time: TimeSpec, reader: BufReader<std::io::Stdin>) -> i32 {
    // Resolve the fake-clock origin ONCE, in Unix-epoch ms, and share it with both the shim and every
    // `state` event, so the panel's fake clock matches what the app's own `Date.now()` reads (no skew
    // from the launch+attach duration). The moment is absolute here (the driver resolved a relative
    // --at before spawning) - an out-of-range moment is an honest error, never a silent fall-back to
    // real time (untouchable rule 4).
    let real_start_ms = now_epoch_ms();
    let bias = time.moment.tz_bias_min.unwrap_or(0);
    // The live session clock, computed Rust-side and kept in step with the shim: set_multiplier and
    // jump re-anchor it here and push the new origin to every context. The shim is built PER ATTACH from
    // this clock's current origin (below), so a context that attaches after an in-flight change starts on
    // the same clock as the others. A flag records whether a rate change happened in flight, so the end
    // report can carry the honest running-timer caveat (rule 4).
    let mut clock = match CdpClock::from_time_spec(&time, real_start_ms) {
        Ok(c) => c,
        Err(key) => {
            emit(&Event::Error {
                v: PROTOCOL_VERSION,
                id: Some(1),
                code: 1,
                key: key.into(),
                origin: "core".into(),
            });
            emit(&ended_clean());
            return 1;
        }
    };
    let mut rate_changed_in_flight = false;

    // Launch under our own isolated profile + debug port, then attach. Any failure is an honest error
    // event plus exit 2 (could not launch/attach), never a faked verdict.
    // The heartbeat starts here, not after the attach. Everything between accepting `start` and the
    // first `state` used to be silence, bounded by the port wait (15 s) - and the client's idle
    // watchdog is 15 s, so a slow Electron and a dead core looked the same to it. Measured on
    // the measured Electron app the gap is about 1.6 s, so this is about the tail, not the common case (R3-3).
    let mut last_beat = std::time::Instant::now();
    // The browser's own slowdown of timers in hidden windows is switched off when the session asked for
    // it (R4-N28) - otherwise a page hidden while time ran faster is reported below.
    let browser_args = cdp::with_background_timers(&target.args, target.keep_background_timers);
    let mut launched = match cdp::launch_chromium(&target.path, &browser_args, target.cwd.as_deref(), || {
        if last_beat.elapsed() >= std::time::Duration::from_secs(1) {
            last_beat = std::time::Instant::now();
            emit(&clock.state_event_at(now_epoch_ms()));
        }
    }) {
        Ok(l) => l,
        Err(e) => {
            emit(&Event::Error {
                v: PROTOCOL_VERSION,
                id: Some(1),
                code: 2,
                key: "target.launch_failed".into(),
                origin: "mechanism".into(),
            });
            diag!("chrono core: {e}");
            emit(&ended_clean());
            return 2;
        }
    };
    // The contexts, their counts and their indexes live in the attacher (cdp_attach) - the index
    // counter stays here, because it is the session's: several attachers in one session must hand
    // out disjoint indexes, and that is the shape the embedded-engine channel needs (docs/09).
    let mut next_index = 0u32;
    // The attacher connects to the browser endpoint, arms auto-attach and attaches by name to the
    // pages the browser already has - measured, auto-attach delivers those too, and the by-name pass
    // is the belt for an engine where it does not. Any failure is one honest error here, as before.
    // One deadline for all of it: no heartbeat beats in here (R4-N26).
    let start_by = Instant::now() + cdp::CONNECT_DEADLINE;
    // Every context goes on the session's zone, set before the first attach. The bias was checked when
    // the clock was built, so it is the zone the panel's fake wall is in.
    let mut attacher = match Attacher::connect("127.0.0.1", launched.port, start_by).and_then(|mut a| {
        a.set_zone(bias);
        a.attach_existing(clock.shim_origin(), &mut next_index, start_by).map(|_| a)
    }) {
        Ok(a) => a,
        Err(e) => {
            let residue = launched.shutdown_with_residue();
            emit(&Event::Error {
                v: PROTOCOL_VERSION,
                id: Some(1),
                code: 2,
                key: "target.attach_failed".into(),
                origin: "mechanism".into(),
            });
            diag!("chrono core: cannot attach over CDP: {e}");
            emit(&ended_after_launch(residue));
            return 2;
        }
    };
    // No start verdict for CDP: unlike the native guard window, at start there is nothing to judge yet
    // (contexts attach asynchronously and have made no time calls). The authoritative verdict is the
    // family `session_verdict` at end - emitting "undetermined" now would read as "not working" when it
    // only means "not audited yet".

    // A stdin reader thread turns command lines into `Command`s (end/query/set_multiplier/jump) so the
    // main loop can interleave them with CDP polling and the heartbeat without blocking on read_line.
    let rx = spawn_command_reader(reader);

    // Install the shim into every context as it attaches (page and its Web Workers), beat a ~1 s
    // `state` heartbeat, and sample per-context call counts, until `end`, stdin EOF, or the app closes.
    let mut app_closed = false;
    let heartbeat = Duration::from_secs(1);
    let mut deadline = Instant::now() + heartbeat;
    let mut last_audit = Instant::now();
    let (port, launched_pid) = (launched.port, launched.pid());
    let mut handoff = Handoff::default();

    'session: loop {
        // The launched process ending does not end the session (R4-S18) - it is noted, with the
        // process that holds the debugging port from then on.
        let alive = launched.is_running();
        handoff.observe(alive, Instant::now(), || port_holder(port, launched_pid));
        // Drain protocol commands without blocking. A clock move waits for the pages up to
        // `MOVE_WAIT_MS`, so the heartbeat is looked at after every command rather than once the
        // queue is empty - commands back to back cannot add their waits into one silence (R4-S10).
        loop {
            match rx.try_recv() {
                Ok(Command::End { .. }) => break 'session,
                Ok(command) => {
                    let moved = apply_command(command, &mut clock, &mut attacher, &mut next_index);
                    rate_changed_in_flight |= moved;
                    beat(&clock, &mut deadline, heartbeat);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'session, // stdin closed (EOF)
            }
        }
        // Poll CDP (bounded by the poll interval) for a newly attached context and shim it, drop one
        // that went away, and take the answers that came. The shim is built from the clock's CURRENT
        // origin, so a context attaching after an in-flight rate change or jump starts on the same
        // clock as every other context (one absolute origin - rule 3).
        if attacher.pump(clock.shim_origin(), &mut next_index) == Pumped::Closed {
            app_closed = true; // the connection dropped, i.e. the app exited
            break;
        }
        // ~1 s heartbeat (also when frozen) and ~1 s coverage sampling, asked for without waiting.
        beat(&clock, &mut deadline, heartbeat);
        if last_audit.elapsed() >= Duration::from_secs(1) {
            attacher.request_counts();
            last_audit = Instant::now();
        }
    }
    // Taken as the loop ends, before the settle below spends its wait: a launched process that ended
    // a few milliseconds before the connection closed is a browser that closed, not a handover.
    let followed = handoff.followed(Instant::now());
    // The last counts, and every clock move a page has not answered taken as missed.
    attacher.settle(clock.shim_origin(), &mut next_index, Instant::now() + END_WAIT);
    // Pages that took a clock move late or not at all (`chromium.clock_move_missed`, rule 6).
    let moves_missed = attacher.moves_missed();
    let seen = attacher.seen().to_vec();
    // A context the shim did not take in and a context refused past the ceiling are the same fact
    // to the verdict: a context that ran on the real clock (untouchable rule 4). The ceiling gets
    // its own warning so the reader learns WHY, not just that some were missed.
    let failed = attacher.failed();
    let past_ceiling = attacher.overflow();
    let zone_missed = attacher.zone_missed();
    let hidden_fast = attacher.hidden_fast();
    let counts = attacher.into_counts();
    let audited = !counts.is_empty();

    let covered = covered_channels(counts);

    let verdict = cdp_verdict(seen.len(), !covered.is_empty(), failed + past_ceiling);
    let (token, reason) = verdict_keys(&verdict);

    let facts = SessionFacts {
        app_closed,
        audited,
        rate_changed_in_flight,
        clock_moves_missed: moves_missed > 0,
        context_ceiling_reached: past_ceiling > 0,
        clock_clamped: clock.reached_range_end(now_epoch_ms()),
        followed_browser: followed.is_some(),
        zone_missed: zone_missed > 0,
        // Not said when the browser was started with the slowdown switched off - its timers kept the
        // session speed while hidden (measured, R4-N28).
        background_timers_slowed: hidden_fast > 0 && !target.keep_background_timers,
        main_process_uncovered: cdp::is_electron_target(&target.path),
    };
    for event in coverage_events(&seen, &covered, session_warnings(&facts)) {
        emit(&event);
    }

    emit_cdp_session_verdict(token, reason, seen.len() as u32, followed.unwrap_or_default());

    // Session timing from the live clock (correct across any in-flight rate changes and jumps): real is
    // the whole session, fake is the elapsed duration, and the end wall is where the fake clock landed.
    // Then tear down our instance (kill + remove the temp profile), reporting any cleanup residue
    // honestly via `ended.residue_keys` (rules 4, 6).
    let end_now = now_epoch_ms();
    let real_ms = clock.elapsed_real_ms(end_now);
    let fake_ms = clock.elapsed_fake_ms(end_now);
    let fake_end_wall = epoch_ms_to_wall(clock.fake_wall_ms(end_now), bias);
    let residue = launched.shutdown_with_residue();
    emit(&Event::Ended {
        v: PROTOCOL_VERSION,
        clean: residue.is_empty(),
        residue_keys: residue,
        target_exit_code: None,
        elapsed_real_ms: real_ms,
        elapsed_fake_ms: fake_ms,
        fake_end_wall: Some(fake_end_wall),
    });
    verdict.exit_code()
}




/// Emit `state` when the heartbeat is due - between commands as well as at the end of a turn.
fn beat(clock: &CdpClock, deadline: &mut Instant, every: Duration) {
    if Instant::now() >= *deadline {
        emit(&clock.state_event_at(now_epoch_ms()));
        *deadline = Instant::now() + every;
    }
}

/// Answer one command that arrived mid-session (`end` is the loop's own business). Returns whether
/// it changed the rate, for the end report's note about timers that were already running.
fn apply_command(command: Command, clock: &mut CdpClock, attacher: &mut Attacher, next_index: &mut u32) -> bool {
    match command {
        Command::Query { id, .. } => {
            emit(&clock.state_event_at(now_epoch_ms()));
            emit(&Event::Ack { v: PROTOCOL_VERSION, id });
            false
        }
        Command::SetMultiplier { id, multiplier, .. } => {
            // Schedule the new rate for one instant shortly ahead, on the panel and in every context
            // alike, so a page takes it over from its own segment at the panel's moment and its wall
            // never steps at the change (R4-S17). From that instant Date.now/new Date/performance.now
            // and new timers run at the new rate - only an already-queued setInterval keeps its old
            // cadence, which the end report warns about (rule 4).
            if !chrono_core::multiplier_in_range(multiplier) {
                emit(&command_error(id, "time.bad_multiplier"));
                return false;
            }
            let next = clock.set_multiplier_at(multiplier, now_epoch_ms());
            attacher.move_clock(&cdp_schedule_expr(next), clock.shim_origin(), next_index);
            emit(&Event::Ack { v: PROTOCOL_VERSION, id });
            emit(&clock.state_event_at(now_epoch_ms()));
            true
        }
        Command::Jump { id, to, .. } => {
            // Move the wall to a new fake instant (absolute, or a relative delta on the current fake
            // time), leaving the duration axis untouched (rule 3). A bad moment is an honest error,
            // never a silent no-op (rule 6).
            let now = now_epoch_ms();
            match cdp_resolve_jump(clock, &to, now) {
                Ok(new_fake) => {
                    clock.jump_to_at(new_fake, now);
                    attacher.move_clock(&cdp_set_expr(clock.shim_origin()), clock.shim_origin(), next_index);
                    emit(&Event::Ack { v: PROTOCOL_VERSION, id });
                    emit(&clock.state_event_at(now_epoch_ms()));
                }
                Err(key) => emit(&command_error(id, key)),
            }
            false
        }
        // A command this loop does not handle here (a second `start`, say). Answered rather than
        // dropped, and with the id, so a client waiting on `ack` learns the outcome.
        other => {
            emit(&unsupported_command(command_id(&other)));
            false
        }
    }
}

/// The honest error for a command that could not be applied (rule 6).
fn command_error(id: u64, key: &str) -> Event {
    Event::Error { v: PROTOCOL_VERSION, id: Some(id), code: 1, key: key.into(), origin: "core".into() }
}

/// The family verdict of a CDP session. No PID registry on this path: a CDP session tracks JS
/// contexts, not injected processes, so its per-context warnings already travel on the coverage
/// events, and it spawns nothing the hook could fail to follow - the two child fields stay empty.
/// `process_count` has carried the context count since this session existed and keeps doing so for
/// the clients that read it there. `context_count` says the same number under its own name.
/// `followed` names the browser a launcher handed the application to, when the session went on with
/// it after the launcher ended (R4-S18) - the same field the native session fills for the processes it
/// went on for (ADR-16), so the report and the window show it without knowing which path ran.
fn emit_cdp_session_verdict(token: &str, reason: &str, contexts: u32, followed: Vec<FollowedProcess>) {
    emit(&Event::SessionVerdict {
        v: PROTOCOL_VERSION,
        verdict: token.to_string(),
        reason_key: reason.to_string(),
        process_count: contexts,
        warning_keys: Vec::new(),
        uncovered_children: Vec::new(),
        uncovered_children_total: 0,
        context_count: contexts,
        engines: Vec::new(),
        followed,
    });
}

/// How long the debugging connection has to outlive the launched process before the session counts
/// the launch as handed over (R4-S18). A browser launched directly closes its socket while its process
/// is torn down, so its connection drops within milliseconds of its exit - without a margin, a loop
/// that happens to see the exit before the closed connection would read an ordinary close as a
/// handover.
const HANDOFF_MARGIN: Duration = Duration::from_secs(1);

/// What the session learns about the launched process ending while the browser goes on (R4-S18).
///
/// Measured before the change: a `.bat` that starts the browser and exits ended the session in about
/// 0.4 s with no verdict, and the browser with it, and so did a Chromium browser started straight
/// from its runtime folder, which hands over to a child of its own after about 0.1 s - the port then
/// belongs to that child.
#[derive(Debug, Default)]
struct Handoff {
    /// When the launched process was first SEEN ended, not when it ended: a launcher that exits
    /// before the browser opens its port is first seen on the loop's first turn.
    seen_ended: Option<Instant>,
    /// The process that held the debugging port once the launched one had ended.
    holder: Option<FollowedProcess>,
    /// When the socket table was last read for the holder, so it is read at most once a second.
    asked: Option<Instant>,
}

impl Handoff {
    /// One look at the launched process, every turn of the loop. Nothing happens while it runs. Once
    /// it has ended, `find_holder` reads the socket table, at most once per [`HANDOFF_MARGIN`], until
    /// it names a holder - a few milliseconds each time, and none at all once one is named.
    fn observe(&mut self, launched_alive: bool, now: Instant, find_holder: impl FnOnce() -> Option<FollowedProcess>) {
        if launched_alive || self.holder.is_some() {
            return;
        }
        self.seen_ended.get_or_insert(now);
        if self.asked.is_some_and(|at| now.duration_since(at) < HANDOFF_MARGIN) {
            return;
        }
        self.asked = Some(now);
        self.holder = find_holder();
    }

    /// `None` unless the session went on: the launched process ended and the connection outlived it by
    /// [`HANDOFF_MARGIN`] when the loop ended at `ended`. Otherwise the process the session went on
    /// with, or an empty list when the socket table never named one - the session went on all the
    /// same, and the key says that much.
    fn followed(&self, ended: Instant) -> Option<Vec<FollowedProcess>> {
        let seen = self.seen_ended?;
        (ended.saturating_duration_since(seen) >= HANDOFF_MARGIN).then(|| self.holder.iter().cloned().collect())
    }
}

/// The process holding the session's debugging port on a loopback address, other than the launched
/// one: the browser a launcher handed the application to. `None` when the table cannot be read or
/// nothing else holds the port. Its name goes through the same sieve as every other name a process
/// gives the report.
fn port_holder(port: u16, launched: u32) -> Option<FollowedProcess> {
    let table = chrono_mech::listening_sockets().ok()?;
    let pid = holder_pid(&table, port, launched)?;
    let image = chrono_mech::process_image_name(pid);
    Some(FollowedProcess { pid, image: image.as_deref().map(cdp::sanitise_target_text) })
}

/// The choice behind [`port_holder`], pure over the table so it is tested without a socket.
fn holder_pid(table: &[chrono_mech::Listener], port: u16, launched: u32) -> Option<u32> {
    table.iter().find(|l| l.port == port && l.loopback && l.pid != launched && l.pid != 0).map(|l| l.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener(pid: u32, port: u16, loopback: bool) -> chrono_mech::Listener {
        chrono_mech::Listener { pid, port, loopback, v6: false, addr_v4: 0 }
    }

    fn browser() -> FollowedProcess {
        FollowedProcess { pid: 7, image: Some("browser.exe".into()) }
    }

    /// A browser launched directly that closes: its connection drops within milliseconds of the
    /// exit, so the session ended with it and did not go on for anything (R4-S18).
    #[test]
    fn a_browser_that_closes_did_not_hand_anything_over() {
        let t0 = Instant::now();
        let mut h = Handoff::default();
        h.observe(true, t0, || panic!("the table is not read while the launched process runs"));
        h.observe(false, t0 + Duration::from_millis(10), || None);
        assert_eq!(h.followed(t0 + Duration::from_millis(40)), None);
    }

    /// A launcher that ends while the browser it started keeps the connection open: the session went
    /// on with that browser, and names it.
    #[test]
    fn a_launcher_that_ends_hands_the_session_to_the_port_holder() {
        let t0 = Instant::now();
        let mut h = Handoff::default();
        h.observe(false, t0, || Some(browser()));
        h.observe(false, t0 + Duration::from_millis(200), || panic!("a named holder is not asked for again"));
        assert_eq!(h.followed(t0 + Duration::from_secs(3)), Some(vec![browser()]));
        assert_eq!(h.followed(t0 + Duration::from_millis(999)), None, "the margin is a full second");
    }

    /// A table that cannot name the holder is read again, but not on every turn of the loop - and a
    /// session that went on still says so, with nobody named.
    #[test]
    fn the_table_is_read_at_most_once_a_second_until_it_names_the_holder() {
        let t0 = Instant::now();
        let mut h = Handoff::default();
        let mut reads = 0;
        for ms in [0, 50, 500, 999, 1_000, 1_400, 2_100] {
            h.observe(false, t0 + Duration::from_millis(ms), || {
                reads += 1;
                None
            });
        }
        assert_eq!(reads, 3, "read at 0, 1000 and 2100 ms");
        assert_eq!(h.followed(t0 + Duration::from_secs(5)), Some(Vec::new()));
    }

    /// A launched process that never ended hands nothing over, however long the session ran.
    #[test]
    fn a_launched_process_that_runs_to_the_end_hands_nothing_over() {
        let t0 = Instant::now();
        let mut h = Handoff::default();
        h.observe(true, t0, || panic!("not read"));
        assert_eq!(h.followed(t0 + Duration::from_secs(60)), None);
    }

    /// The holder is whoever else listens on the session's port on a loopback address - never the
    /// launched process, never a socket on another port or another address, never pid 0.
    #[test]
    fn the_holder_is_another_process_on_the_sessions_loopback_port() {
        let table = [
            listener(10, 9222, true),
            listener(0, 9222, true),
            listener(11, 9222, false),
            listener(12, 9333, true),
            listener(13, 9222, true),
        ];
        assert_eq!(holder_pid(&table, 9222, 10), Some(13));
        assert_eq!(holder_pid(&table, 9222, 13), Some(10), "the launched pid is the only one skipped");
        assert_eq!(holder_pid(&table[..4], 9222, 10), None);
        assert_eq!(holder_pid(&[], 9222, 10), None);
    }
}


