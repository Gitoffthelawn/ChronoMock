//! An application that outlives its session is let go without its duration axes stepping back.
//!
//! A session ends while the application keeps running more often than not: a `--ticks` cutoff, a Stop in
//! the panel, a core that died. The hook then lets the application go, and until 2026-09-24 it did that by
//! handing back the real value of every clock. For the wall and the zone that is the point. For the tick
//! count, the unbiased interrupt time and QPC under `--scale-duration` / `--scale-qpc` it was a step BACK
//! by the whole acceleration - measured at x60 after 5.4 s, 316 s in one step, on x64 and x86 alike -
//! on the axis untouchable rule 3 says never rewinds.
//!
//! The target is this test binary itself, as in `duration_axis.rs`: the ignored probe below does its
//! work only when the variable names a file. It samples the axes for longer than the session lasts
//! and writes the largest step back it saw on each, plus how fast each moved at the start and the tick
//! count at the end, against a reference the hook never touches. The three interrupt-time clocks joined
//! the duration axis in R5-F4 and are let go the same way, so they are sampled beside the first three.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Where the probe writes what it measured. Unset, the probe returns at once.
const PROBE_OUT: &str = "CHRONO_SESSION_END_PROBE_OUT";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_samples_the_duration_axes_past_the_session_end";

/// How long the probe samples, in real milliseconds. The session below ends after two heartbeats, so the
/// probe sees about two seconds of the session and three of what comes after it.
const SAMPLE_MS: f64 = 5_000.0;

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
    fn QueryUnbiasedInterruptTime(time: *mut u64) -> i32;
}

// Exported by kernelbase alone (R5-F4).
#[cfg_attr(target_arch = "x86", link(name = "kernelbase", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernelbase", kind = "raw-dylib"))]
unsafe extern "system" {
    fn QueryInterruptTime(time: *mut u64);
    fn QueryInterruptTimePrecise(time: *mut u64);
    fn QueryUnbiasedInterruptTimePrecise(time: *mut u64);
}

/// The axes sampled, in the order the probe writes them.
const AXES: [&str; 6] = [
    "tick count",
    "interrupt time",
    "QPC",
    "QueryInterruptTime",
    "QueryInterruptTimePrecise",
    "QueryUnbiasedInterruptTimePrecise",
];

#[cfg_attr(target_arch = "x86", link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    /// The reference clock. The hook never touches it (ADR-2), so it stays real under any session.
    fn NtQueryPerformanceCounter(counter: *mut i64, frequency: *mut i64) -> i32;
}

/// Real milliseconds by the unhooked reference clock.
fn real_ms() -> f64 {
    let (mut counter, mut frequency) = (0i64, 0i64);
    // SAFETY: both pointers are to live locals.
    unsafe { NtQueryPerformanceCounter(&mut counter, &mut frequency) };
    counter as f64 * 1000.0 / frequency as f64
}

/// One reading of every axis in `AXES`, each in milliseconds.
fn axes(origin: Instant) -> [f64; 6] {
    let (mut quit, mut qit, mut qitp, mut quitp) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: the exports' documented signatures, each with a live out pointer or none.
    let tick = unsafe { GetTickCount64() };
    unsafe {
        QueryUnbiasedInterruptTime(&mut quit);
        QueryInterruptTime(&mut qit);
        QueryInterruptTimePrecise(&mut qitp);
        QueryUnbiasedInterruptTimePrecise(&mut quitp);
    }
    let ms = |hundreds_of_ns: u64| hundreds_of_ns as f64 / 10_000.0;
    [tick as f64, ms(quit), origin.elapsed().as_secs_f64() * 1000.0, ms(qit), ms(qitp), ms(quitp)]
}

/// Samples every few milliseconds for `SAMPLE_MS` of real time and writes, space separated: the rate of
/// every axis over the first real second, the tick rate over the last real second, and the largest step
/// back seen on every axis (0 when it never went back).
///
/// A rate per axis at the start, not the tick count's alone: the step-back check on an axis the session
/// never scaled cannot fail, so without it a session that stopped scaling one would pass here with
/// nothing measured on it.
#[test]
#[ignore = "the target of `an_application_left_running_keeps_its_duration_axes_moving_forward`, not a test on its own"]
fn probe_samples_the_duration_axes_past_the_session_end() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let origin = Instant::now();
    let start = real_ms();
    let mut last = axes(origin);
    let mut worst = [0.0f64; AXES.len()];
    let first = last;
    let mut head_rates = None;
    let mut tail_from = None;
    loop {
        // A sleep, not a spin: the session shortens it to its floor, which is still a millisecond, and
        // once the session is gone it is a plain millisecond again.
        std::thread::sleep(Duration::from_millis(5));
        let now = real_ms() - start;
        let reading = axes(origin);
        for (i, w) in worst.iter_mut().enumerate() {
            *w = w.min(reading[i] - last[i]);
        }
        last = reading;
        if head_rates.is_none() && now >= 1_000.0 {
            head_rates = Some(std::array::from_fn::<f64, 6, _>(|i| (reading[i] - first[i]) / now));
        }
        if tail_from.is_none() && now >= SAMPLE_MS - 1_000.0 {
            tail_from = Some((now, reading[0]));
        }
        if now >= SAMPLE_MS {
            let (t0, tick0) = tail_from.unwrap_or((now, reading[0]));
            let tail_rate = (reading[0] - tick0) / (now - t0).max(1.0);
            let heads = head_rates.unwrap_or([0.0; AXES.len()]).map(|r| format!("{r:.2}"));
            let backs = worst.map(|w| format!("{:.1}", -w));
            let line = format!("{} {tail_rate:.2} {}", heads.join(" "), backs.join(" "));
            std::fs::write(&out, line).expect("the probe writes its result");
            return;
        }
    }
}

/// What the probe wrote: the head rate of every axis, the tail rate of the tick count, and the step back
/// on every axis.
struct Measured {
    heads: [f64; 6],
    tail: f64,
    backs: [f64; 6],
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

/// What the probe wrote, or `None` when it wrote nothing or not every number.
fn measured(file: &std::path::Path) -> Option<Measured> {
    let text = std::fs::read_to_string(file).ok()?;
    let values: Vec<f64> = text.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    let all: [f64; 13] = values.try_into().ok()?;
    Some(Measured {
        heads: std::array::from_fn(|i| all[i]),
        tail: all[6],
        backs: std::array::from_fn(|i| all[7 + i]),
    })
}

/// Each axis by its name with a number, for a message.
fn named(values: &[f64; 6]) -> String {
    AXES.iter().zip(values).map(|(axis, v)| format!("{axis} {:.1}", v + 0.0)).collect::<Vec<_>>().join(", ")
}

/// An application still running when its session ends carries on at rate 1 from where its duration axes
/// stood, and the session says it left it running.
#[test]
fn an_application_left_running_keeps_its_duration_axes_moving_forward() {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this probe drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let dir = std::env::temp_dir().join(format!("chrono-session-end-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let probe_args = ["--ignored", "--exact", PROBE, "--test-threads", "1"];

    // The control first: with no session every axis moves at about the real rate and never back.
    // Without it a probe that measured nothing would pass everything below.
    let control = dir.join("control.txt");
    let run = Command::new(&me)
        .args(probe_args)
        .env(PROBE_OUT, &control)
        .output()
        .expect("the probe must run without a session");
    assert!(run.status.success(), "the probe failed without a session: {}", String::from_utf8_lossy(&run.stdout));
    let control = measured(&control).expect("the probe wrote nothing without a session");
    assert!(
        control.heads.iter().all(|&r| r < 2.0) && control.tail < 2.0,
        "without a session the axes moved at ({}) then x{}, so this probe cannot tell",
        named(&control.heads),
        control.tail
    );

    // The session ends after two heartbeats. The probe inherits the core's output, so `output()`
    // returns once the probe is done as well - which is what the result file needs.
    let session = dir.join("session.txt");
    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &me.display().to_string(),
            "--mode",
            "x60",
            "--scale-duration",
            "--scale-qpc",
            "--ticks",
            "2",
            "--json",
            "--args",
            &probe_args.join(" "),
        ])
        .env(PROBE_OUT, &session)
        .output()
        .expect("the tool must run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let context = || format!("stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));
    let Some(session) = measured(&session) else {
        panic!("the probe wrote nothing under the session. {}", context());
    };
    // Every axis the step-back check reads has to have been scaled, or that check proves nothing on it.
    assert!(
        session.heads.iter().all(|&r| r > 10.0),
        "during the session the axes moved at ({}), so the session did not scale every one. {}",
        named(&session.heads),
        context()
    );
    assert!(
        session.tail < 2.0,
        "the tick count still moved x{} after the session, so the probe did not outlive it. {}",
        session.tail,
        context()
    );
    assert!(
        session.backs.iter().all(|&b| b == 0.0),
        "letting the application go stepped its duration axes back by ({}) ms - untouchable rule 3. {}",
        named(&session.backs),
        context()
    );
    assert!(
        stdout.lines().any(|l| l.contains("\"session_verdict\"") && l.contains("\"session.left_running\"")),
        "the session let a running application go without saying so. {}",
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
