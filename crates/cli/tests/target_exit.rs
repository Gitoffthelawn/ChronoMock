//! The application's own exit code reaches the report also when it is 259 (R4-N19).
//!
//! 259 is `STILL_ACTIVE`, the value `GetExitCodeProcess` gives for a process that is still running, and
//! also a code a program may end with. The session asked the code alone whether the launched process had
//! ended, so an application that ended with 259 had no exit code on the report, in `ended` or in the
//! window. Whether it ended is now the signalled process object's to say.
//!
//! The target is this test binary itself, as in `session_end.rs`: the ignored probe below reads the clock,
//! outlives the guard window and the first look at the family, and ends with 259.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Set for the probe, so it ends with the code. Unset, the probe returns at once.
const PROBE_ON: &str = "CHRONO_TARGET_EXIT_PROBE";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_ends_with_the_code_that_reads_as_still_running";

/// The code `GetExitCodeProcess` also gives for a process that is still running.
const STILL_ACTIVE: i32 = 259;

/// Reads the clock, lives past the guard window and the first heartbeat, and ends with 259.
#[test]
#[ignore = "the target of the session test in this file, not a test on its own"]
fn probe_ends_with_the_code_that_reads_as_still_running() {
    if std::env::var_os(PROBE_ON).is_none() {
        return;
    }
    let _ = SystemTime::now();
    std::thread::sleep(Duration::from_millis(1_500));
    std::process::exit(STILL_ACTIVE);
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

#[test]
fn an_application_that_ends_with_259_has_that_code_on_the_report() {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this test drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &me.display().to_string(),
            "--at",
            "2077-01-01T00:00:00",
            "--zone",
            "+00:00",
            "--args",
            &format!("--ignored --exact {PROBE} --test-threads 1"),
        ])
        .env(PROBE_ON, "1")
        .output()
        .expect("the tool must run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let context = || format!("stdout:\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));

    // The probe ran under the session and lived past the guard window, or the line below proves nothing.
    assert!(stdout.contains("verdict:  WORKS"), "the probe did not run under the session. {}", context());
    assert!(
        stdout.contains(&format!("the target closed itself with code {STILL_ACTIVE}")),
        "the application's own exit code is missing from the report. {}",
        context()
    );
}
