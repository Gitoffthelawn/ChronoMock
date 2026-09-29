//! A console target stays off the protocol (R4-W1, ADR-17).
//!
//! The core speaks to its client over its own stdin and stdout, and a console program started with
//! the defaults gets copies of both. Measured before the fix (`tools/probes/r4-5`): a byte that is not
//! UTF-8 in the target's output ended the driver's read, so the run printed "no verdict" beside exit
//! code 0. A program reading its input took the `end` that `--ticks` sent, so the session ran on
//! until the program closed. And `--json` carried the program's lines among the events.
//!
//! The target here is the command interpreter every Windows has. It prints a line, reads a line of
//! input the way a script does, and then keeps running on a loopback ping for longer than the
//! session, so an `end` that reaches the core leaves it running - which the report says
//! (`session.left_running`). The tool is started with no console window (`CREATE_NO_WINDOW`), so the
//! target's input is NUL and nothing waits for a keyboard, wherever the tests run.
//!
//! Whether the printed line is valid UTF-8 depends on the console's code page (it was UTF-8 here,
//! and `chcp` did not change what the interpreter wrote into a pipe - measured), so this does not
//! lean on a bad byte: none of the target's output may reach the protocol at all.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// `CREATE_NO_WINDOW` from the Windows API, spelled out so the test needs no bindings crate.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The command interpreter, by full path (see `dry_run.rs` for why a bare name will not do).
fn command_interpreter() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    format!(r"{root}\System32\cmd.exe")
}

/// The library the core injects, beside the binary under test. `cargo test` does not build it.
fn injected_library_is_built() {
    let library = PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll");
    assert!(
        library.is_file(),
        "this test starts the core and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI does that before the tests.",
        library.display()
    );
}

fn describe(out: &Output) -> String {
    format!(
        "exit {:?}, stdout: {} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_console_target_neither_reads_the_commands_nor_writes_into_the_events() {
    injected_library_is_built();
    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &command_interpreter(),
            "--args",
            "/c echo target-said-hello & set /p X= & ping -n 6 127.0.0.1 >nul",
            "--at",
            "2030-01-01T00:00:00",
            "--zone",
            "+00:00",
            "--ticks",
            "2",
            "--json",
        ])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("the tool must run");

    // Every line on stdout is an event: the target's line is not among them.
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with('{') && line.contains("\"type\":\""),
            "a line on --json that is not an event: {line:?} - {}",
            describe(&out)
        );
    }
    // The read went on past the target's first line, so the session's end arrived.
    assert!(stdout.contains("\"type\":\"session_verdict\""), "no session verdict: {}", describe(&out));
    assert!(stdout.contains("\"type\":\"ended\""), "no end of session: {}", describe(&out));
    // The `end` that --ticks sent reached the core whole: the session ended with the target still
    // pinging, and the core read no broken command.
    assert!(
        stdout.contains("session.left_running"),
        "the session did not end on --ticks, so the target took its end: {}",
        describe(&out)
    );
    assert!(!stdout.contains("protocol.bad_command_ignored"), "a command arrived broken: {}", describe(&out));
    // The target's own output went where the tool's diagnostics go (R4-D15).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("target-said-"), "the target's output did not reach stderr: {}", describe(&out));
    // And it still held that stream when the session ended, which this test feels: `output()` reads
    // stderr through a pipe to its end, so it returns when the ping does, not when the tool exits. The
    // run says so, where the wait would otherwise look like the tool hanging (R4/5 review round).
    assert!(
        stderr.contains("the application is still running and writes to this command's standard error"),
        "the run did not say the target holds its stderr: {}",
        describe(&out)
    );
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
}
