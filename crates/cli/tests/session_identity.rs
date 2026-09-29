//! A child started after its session ended runs on the real clock (R4-W2).
//!
//! A session often ends while the application keeps running - a `--ticks` cutoff, a Stop in the panel,
//! a core that died - and the hook then lets the application go back to the real clock (ADR-14). Until
//! R4/6 it still followed that application's children: a child started a minute later was injected,
//! found the control block its parent kept mapped, joined it and stayed on the session's date for good,
//! with the hook loaded. Measured before the fix on a probe of our own (`tools/probes/r4-6`), after a
//! clean end and after a killed core alike.
//!
//! The target is this test binary itself, as in `session_end.rs`: the ignored probe below does its work
//! only when the variable names a file. It writes the time it reads, outlives the session, then starts
//! itself as a child through `CreateProcessW` - the call the hook follows - and the child writes the
//! time it reads and whether the hook library is loaded in it.
//!
//! The second test starts ANOTHER session while that application still runs. Before the fix its later
//! child joined the second session - it read that session's date and stood in its coverage - and the
//! second core called the first session's ordered end a dead core.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where the probe writes what it read. Unset, the probe returns at once.
const PROBE_OUT: &str = "CHRONO_SESSION_IDENTITY_PROBE_OUT";

/// Set in the child the probe starts, so the same probe knows which half it is.
const PROBE_CHILD: &str = "CHRONO_SESSION_IDENTITY_PROBE_CHILD";

/// Milliseconds the probe waits before it starts its child. Unset, it waits `CHILD_AFTER_MS`.
const PROBE_CHILD_AFTER: &str = "CHRONO_SESSION_IDENTITY_PROBE_CHILD_AFTER_MS";

/// Set, the probe only writes one line and waits this many milliseconds - the second session's target.
const PROBE_IDLE: &str = "CHRONO_SESSION_IDENTITY_PROBE_IDLE_MS";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_starts_a_child_after_its_session_ends";

/// How long the probe waits before it starts its child, in real milliseconds. The session below ends
/// after two heartbeats, so the child starts about three seconds after the end. A plain sleep: the
/// session does not scale durations, so it is not shortened.
const CHILD_AFTER_MS: u64 = 5_000;

/// The session's moment, far enough ahead that no real clock reading can be mistaken for it.
const SESSION_AT: &str = "2077-01-01T00:00:00";

/// The second session's moment in the takeover test.
const SECOND_AT: &str = "2088-01-01T00:00:00";

/// Seconds between the Unix epoch and 2070-01-01, a line every reading on a session's clock here is
/// past and no real reading this century reaches.
const SESSION_LINE: u64 = 3_155_760_000;

/// The core runs one session at a time, and the two tests below each run one. Taken by both, so the
/// harness running them in parallel does not make one refuse the other.
static ONE_SESSION: Mutex<()> = Mutex::new(());

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const core::ffi::c_char) -> *mut core::ffi::c_void;
}

/// The wall clock as this process reads it, in whole seconds since the Unix epoch.
fn wall_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Whether the hook library is loaded in this process.
fn hooked() -> bool {
    // SAFETY: a lookup by name with a NUL-terminated literal, no other state.
    !unsafe { GetModuleHandleA(c"chrono_hook.dll".as_ptr()) }.is_null()
}

fn append(file: &Path, role: &str) {
    use std::io::Write;
    let line = format!("{role} {} {} {}\n", wall_seconds(), u8::from(hooked()), std::process::id());
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(file).expect("the probe opens its file");
    out.write_all(line.as_bytes()).expect("the probe writes its line");
}

/// A number of milliseconds from the variable `name`, when it is set.
fn millis(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

/// Writes `parent-start`, waits past the session, starts itself as the child, which writes `child`,
/// then writes `parent-after`. Each line holds the wall seconds read, 1 when the hook is loaded, and the
/// pid. With `PROBE_IDLE` set it writes `idle` and only waits.
#[test]
#[ignore = "the target of the two session tests in this file, not a test on its own"]
fn probe_starts_a_child_after_its_session_ends() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let out = PathBuf::from(out);
    if let Some(idle) = millis(PROBE_IDLE) {
        append(&out, "idle");
        std::thread::sleep(Duration::from_millis(idle));
        return;
    }
    if std::env::var_os(PROBE_CHILD).is_some() {
        append(&out, "child");
        return;
    }
    append(&out, "parent-start");
    std::thread::sleep(Duration::from_millis(millis(PROBE_CHILD_AFTER).unwrap_or(CHILD_AFTER_MS)));
    let me = std::env::current_exe().expect("the probe knows its own path");
    let child = Command::new(me)
        .args(["--ignored", "--exact", PROBE, "--test-threads", "1"])
        .env(PROBE_CHILD, "1")
        .status();
    append(&out, if child.is_ok() { "parent-after" } else { "parent-spawn-failed" });
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

/// What the probe wrote for one role.
struct Reading {
    seconds: u64,
    hooked: bool,
    pid: u32,
}

/// The line the probe wrote for `role`.
fn line(text: &str, role: &str) -> Option<Reading> {
    let mut parts = text.lines().find(|l| l.split_whitespace().next() == Some(role))?.split_whitespace().skip(1);
    Some(Reading { seconds: parts.next()?.parse().ok()?, hooked: parts.next()? == "1", pid: parts.next()?.parse().ok()? })
}

/// The library is built and the scratch directory `name` is empty, before a session starts.
fn prepare(name: &str) -> PathBuf {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this probe drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let dir = std::env::temp_dir().join(format!("chrono-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// `chrono run` on this test binary's probe at `at`, ending after `ticks` heartbeats, in `--json`.
fn session(at: &str, ticks: &str) -> Command {
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let mut run = Command::new(env!("CARGO_BIN_EXE_chrono"));
    run.args([
        "run",
        &me.display().to_string(),
        "--at",
        at,
        "--zone",
        "+00:00",
        "--ticks",
        ticks,
        "--json",
        "--args",
        &format!("--ignored --exact {PROBE} --test-threads 1"),
    ]);
    run
}

/// Whether the child's reading is the real clock, not a session's, with no hook in it.
fn is_real(child: &Reading, real_now: u64) -> bool {
    child.seconds <= real_now && real_now - child.seconds < 600 && !child.hooked
}

/// An application that outlives its session starts its children as it would without the tool: on the
/// real clock and without the hook, while its own start was on the session's clock.
#[test]
fn a_child_started_after_its_session_ended_runs_on_the_real_clock() {
    let _one = ONE_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let dir = prepare("session-identity");
    let file = dir.join("probe.txt");

    // The probe inherits the tool's error stream, so `output()` returns once the probe and its child are
    // done as well - which is what the result file needs.
    let out = session(SESSION_AT, "2").env(PROBE_OUT, &file).output().expect("the tool must run");
    let real_now = wall_seconds();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let context = || format!("probe: {text:?} stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));

    // The session took effect on the application, or nothing below says anything about it.
    let Some(start) = line(&text, "parent-start") else {
        panic!("the probe wrote nothing under the session. {}", context());
    };
    assert!(start.seconds > SESSION_LINE && start.hooked, "the application did not start on the session's clock. {}", context());
    // The session ended with the application still running, so the child really came after the end.
    assert!(
        stdout.lines().any(|l| l.contains("\"session_verdict\"") && l.contains("\"session.left_running\"")),
        "the session did not end before the application did, so this proves nothing. {}",
        context()
    );
    let Some(child) = line(&text, "child") else {
        panic!("the application started no child after the session. {}", context());
    };
    assert!(
        is_real(&child, real_now),
        "a child started after the session ended read {} (real {real_now}) with the hook {} - it joined a \
         session that was over (R4-W2). {}",
        child.seconds,
        if child.hooked { "loaded" } else { "not loaded" },
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pids the session's coverage events name.
fn coverage_pids(stdout: &str) -> Vec<u64> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|event| event["type"] == "coverage")
        .filter_map(|event| event["pid"].as_u64())
        .collect()
}

/// A second session started while an application of the first still runs takes the control block over,
/// says that the first one ended with its application running, and does not get that application's
/// later child: the child runs on the real clock and is not in the second session's coverage.
#[test]
fn a_second_session_neither_adopts_the_first_ones_application_nor_calls_its_end_a_death() {
    let _one = ONE_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let dir = prepare("session-takeover");
    let first_file = dir.join("first.txt");
    let second_file = dir.join("second.txt");

    // The first session ends after two heartbeats, and its application starts a child seven seconds in,
    // while the second session is running. Nobody reads its streams, so they go nowhere.
    let mut first = session(SESSION_AT, "2")
        .env(PROBE_OUT, &first_file)
        .env(PROBE_CHILD_AFTER, "7000")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the tool must run");
    let first_exit = first.wait().expect("the first session ends");
    assert!(line(&std::fs::read_to_string(&first_file).unwrap_or_default(), "parent-start").is_some(), "the first session never started its application");

    // The second session's application only waits, for longer than the first one's child takes to come.
    let second = session(SECOND_AT, "6")
        .env(PROBE_OUT, &second_file)
        .env(PROBE_IDLE, "8000")
        .stdin(Stdio::null())
        .output()
        .expect("the tool must run");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut first_text = String::new();
    while Instant::now() < deadline {
        first_text = std::fs::read_to_string(&first_file).unwrap_or_default();
        if line(&first_text, "parent-after").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let real_now = wall_seconds();
    let stdout = String::from_utf8_lossy(&second.stdout);
    let stderr = String::from_utf8_lossy(&second.stderr);
    let context = || format!("first exit {first_exit:?}, first probe: {first_text:?} second stdout: {stdout} second stderr: {stderr}");

    assert!(
        stderr.contains("took over the control block of a session that ended while its application kept running"),
        "the second core did not say it took over a session that had ended in order (R4-D19). {}",
        context()
    );
    assert!(!stderr.contains("a previous core had died"), "an ordered end was reported as a dead core. {}", context());
    let Some(child) = line(&first_text, "child") else {
        panic!("the first session's application started no child. {}", context());
    };
    assert!(
        is_real(&child, real_now),
        "the first session's application started a child that read {} (real {real_now}) with the hook {} - \
         it joined the second session (R4-W2). {}",
        child.seconds,
        if child.hooked { "loaded" } else { "not loaded" },
        context()
    );
    let covered = coverage_pids(&stdout);
    assert!(!covered.is_empty(), "the second session reported no coverage, so it proves nothing. {}", context());
    assert!(
        !covered.contains(&u64::from(child.pid)),
        "the second session's coverage names the first one's child {}. {}",
        child.pid,
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
