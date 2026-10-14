//! The evidence file of `--report`, seen from the whole command (R5-D28, review round on PR #104).
//!
//! A session that reached a verdict whose evidence did not take the path the flag named ends with exit
//! code 8, not with the verdict. Held open by another program, the old file cannot be replaced and
//! stays where it was, so a pipeline that keeps that file as the proof of the run would have kept the
//! proof of an earlier one under exit code 0.

use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// The old evidence file is held with no sharing, the way an editor can hold it: the session runs,
/// the rename over the old file fails, the old file stays as it was, the new evidence stays in the
/// scratch file the run names, and the exit code says the evidence is not where it was asked for.
#[test]
fn a_session_whose_evidence_cannot_replace_the_old_file_ends_with_exit_code_8() {
    injected_library_is_built();
    let dir = std::env::temp_dir().join(format!("chrono-evidence-held-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let evidence = dir.join("evidence.txt");
    std::fs::write(&evidence, "an earlier run").expect("the old evidence");
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&evidence).expect("held");

    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &command_interpreter(),
            "--args",
            "/c ping -n 2 127.0.0.1 >nul",
            "--at",
            "2030-01-01T00:00:00",
            "--zone",
            "+00:00",
            "--report",
            &evidence.display().to_string(),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("the tool must run");
    drop(held);

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let old = std::fs::read_to_string(&evidence).unwrap_or_default();
    let scratch: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the scratch directory")
        .map(|e| e.expect("an entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "tmp"))
        .collect();
    let kept = scratch.first().map(|p| std::fs::read_to_string(p).unwrap_or_default()).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(out.status.code(), Some(8), "stderr: {stderr}");
    assert!(stderr.contains("cannot replace") && stderr.contains("exit code 8, not the verdict's"), "{stderr}");
    assert_eq!(old, "an earlier run", "the held file is untouched");
    assert_eq!(scratch.len(), 1, "one scratch file: {scratch:?}");
    assert!(kept.contains("verdict"), "the evidence is in the scratch file: {kept}");
}
