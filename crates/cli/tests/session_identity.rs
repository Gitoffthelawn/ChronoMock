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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where the probe writes what it read. Unset, the probe returns at once.
const PROBE_OUT: &str = "CHRONO_SESSION_IDENTITY_PROBE_OUT";

/// Set in the child the probe starts, so the same probe knows which half it is.
const PROBE_CHILD: &str = "CHRONO_SESSION_IDENTITY_PROBE_CHILD";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_starts_a_child_after_its_session_ends";

/// How long the probe waits before it starts its child, in real milliseconds. The session below ends
/// after two heartbeats, so the child starts about three seconds after the end. A plain sleep: the
/// session does not scale durations, so it is not shortened.
const CHILD_AFTER_MS: u64 = 5_000;

/// The session's moment, far enough ahead that no real clock reading can be mistaken for it.
const SESSION_AT: &str = "2077-01-01T00:00:00";

/// Seconds between the Unix epoch and 2070-01-01, a line every reading on the session's clock is past
/// and no real reading this century reaches.
const SESSION_LINE: u64 = 3_155_760_000;

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
    let line = format!("{role} {} {}\n", wall_seconds(), u8::from(hooked()));
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(file).expect("the probe opens its file");
    out.write_all(line.as_bytes()).expect("the probe writes its line");
}

/// Writes `parent-start`, waits past the session, starts itself as the child, which writes `child`,
/// then writes `parent-after`. Each line holds the wall seconds read and 1 when the hook is loaded.
#[test]
#[ignore = "the target of `a_child_started_after_its_session_ended_runs_on_the_real_clock`, not a test on its own"]
fn probe_starts_a_child_after_its_session_ends() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let out = PathBuf::from(out);
    if std::env::var_os(PROBE_CHILD).is_some() {
        append(&out, "child");
        return;
    }
    append(&out, "parent-start");
    std::thread::sleep(Duration::from_millis(CHILD_AFTER_MS));
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

/// The line the probe wrote for `role`: the seconds read and whether the hook was loaded.
fn line(text: &str, role: &str) -> Option<(u64, bool)> {
    let mut parts = text.lines().find(|l| l.split_whitespace().next() == Some(role))?.split_whitespace().skip(1);
    Some((parts.next()?.parse().ok()?, parts.next()? == "1"))
}

/// An application that outlives its session starts its children as it would without the tool: on the
/// real clock and without the hook, while its own start was on the session's clock.
#[test]
fn a_child_started_after_its_session_ended_runs_on_the_real_clock() {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this probe drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let dir = std::env::temp_dir().join(format!("chrono-session-identity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join("probe.txt");

    // The probe inherits the tool's error stream, so `output()` returns once the probe and its child are
    // done as well - which is what the result file needs.
    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &me.display().to_string(),
            "--at",
            SESSION_AT,
            "--zone",
            "+00:00",
            "--ticks",
            "2",
            "--json",
            "--args",
            &format!("--ignored --exact {PROBE} --test-threads 1"),
        ])
        .env(PROBE_OUT, &file)
        .output()
        .expect("the tool must run");
    let real_now = wall_seconds();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let context = || format!("probe: {text:?} stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));

    // The session took effect on the application, or nothing below says anything about it.
    let Some((start, start_hooked)) = line(&text, "parent-start") else {
        panic!("the probe wrote nothing under the session. {}", context());
    };
    assert!(start > SESSION_LINE && start_hooked, "the application did not start on the session's clock. {}", context());
    // The session ended with the application still running, so the child really came after the end.
    assert!(
        stdout.lines().any(|l| l.contains("\"session_verdict\"") && l.contains("\"session.left_running\"")),
        "the session did not end before the application did, so this proves nothing. {}",
        context()
    );
    let Some((child, child_hooked)) = line(&text, "child") else {
        panic!("the application started no child after the session. {}", context());
    };
    assert!(
        child <= real_now && real_now - child < 600 && !child_hooked,
        "a child started after the session ended read {child} (real {real_now}) with the hook {} - it \
         joined a session that was over (R4-W2). {}",
        if child_hooked { "loaded" } else { "not loaded" },
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
