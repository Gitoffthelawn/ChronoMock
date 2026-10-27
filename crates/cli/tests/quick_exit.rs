//! A program that ends at once ends its session, unless a windowed program was in its family (ADR-4,
//! owner decision 2026-10-27).
//!
//! The guard window after the start reads an exit inside it as the second instance of a single-instance
//! application handing its work to the first. Until 2026-10-27 it read every such exit that way, so a
//! console tool that did its work in a few milliseconds - a date printed, a commit made, a file written,
//! each measured with the session's date in it - was reported as a session that never took effect, with
//! exit code 12. The suspicion now needs a windowed program in the family, which is what a hand-off is,
//! and the hook says which kind each process is (`Cov.subsystem`).
//!
//! Both targets come with every Windows installation, so this runs on a clean runner: the command
//! interpreter as the console program, and `rundll32.exe` with nothing to run as the windowed one, which
//! ends at once without showing anything (measured: 1 to 3 ms after the start, both bitnesses).

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

/// Held by every test here that starts the core. The core allows one session at a time, and the
/// tests of one file run on parallel threads.
static REAL_SESSION: Mutex<()> = Mutex::new(());

fn one_real_session_at_a_time() -> MutexGuard<'static, ()> {
    REAL_SESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

/// A program from the system folder, by full path, because the target is never searched for on PATH.
fn system_program(name: &str) -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    format!(r"{root}\System32\{name}")
}

/// One session over `program`, with the protocol on stdout: its exit code, stdout and stderr.
fn session(program: &str, args: Option<&str>) -> (Option<i32>, String, String) {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this test drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let _session = one_real_session_at_a_time();
    let mut command = Command::new(env!("CARGO_BIN_EXE_chrono"));
    command.args(["run", program, "--at", "2091-06-15T12:00:00", "--zone", "+00:00", "--json"]);
    if let Some(args) = args {
        command.args(["--args", args]);
    }
    let out = command.output().expect("the tool must run");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The command interpreter told to end at once is a console program that does its work inside the guard
/// window, and its session ends the way any other does: a verdict for the family, no vanish, exit 0.
#[test]
fn a_console_program_that_ends_at_once_ends_its_session_instead_of_vanishing() {
    let (code, stdout, stderr) = session(&system_program("cmd.exe"), Some("/c exit 0"));
    let context = format!("exit {code:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(!stdout.contains("\"vanished\""), "a console program that ended was called a vanish. {context}");
    assert!(stdout.contains("\"session_verdict\""), "the session ended without a family verdict. {context}");
    assert_eq!(code, Some(0), "the interpreter read the session's clock, so the session works. {context}");
}

/// A windowed program that ends at once is what the second instance of a single-instance application
/// looks like from outside, and the suspicion stays: exit 12, named.
#[test]
fn a_windowed_program_that_ends_at_once_is_still_a_suspected_hand_off() {
    let (code, stdout, stderr) = session(&system_program("rundll32.exe"), None);
    let context = format!("exit {code:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains("target.single_instance_suspected"),
        "a windowed program that ended at once was not suspected of handing off. {context}"
    );
    assert_eq!(code, Some(12), "{context}");
}
