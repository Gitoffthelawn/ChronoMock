//! A target that ends while Windows loads it is named for what failed, at once and without an error
//! window, and an application that loads fine runs with the error mode it inherited (R4-S6).
//!
//! The target starts suspended, and the remote thread that loads the hook is the first to run in it, so
//! that thread does the target's own loading first. A missing static import ended the process right
//! there, and the session reported a successful injection followed by a vanished target - or, started
//! from Explorer, with Windows's error window up, a loader lock ten seconds later. Measured on x64 and x86
//! in tools/probes/r4-9. The core now switches that window off while the target loads and back before its
//! entry point.
//!
//! Both sessions here run with the error mode a program started from Explorer has (the default, which
//! shows the window), whatever mode the test runner itself was given. The broken target is a copy of
//! this test binary with one imported library renamed, so nothing has to be built for it.
//!
//! A third session runs the same broken copy as a CHILD of the application under test, which the hook
//! loads itself into and reads the end of the way the core reads its own target's (R5-A7, R5-F8).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// One real session at a time: the core allows one, and the two tests here would otherwise race for it.
static REAL_SESSION: Mutex<()> = Mutex::new(());

fn one_session_at_a_time() -> MutexGuard<'static, ()> {
    REAL_SESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `CREATE_DEFAULT_ERROR_MODE`: the child gets the default error mode instead of this process's (MS
/// Learn, process creation flags). The default shows Windows's error window - the case that hung.
const CREATE_DEFAULT_ERROR_MODE: u32 = 0x0400_0000;

/// Where the mode probe writes the error mode it runs with. Unset, the probe returns at once.
const MODE_OUT: &str = "CHRONO_LOADER_MODE_PROBE_OUT";

/// The mode probe's own name, which is how the binary is asked to run it and nothing else.
const MODE_PROBE: &str = "probe_reports_the_error_mode_it_runs_with";

/// The name the renamed import gets. No such library exists anywhere Windows looks.
const MISSING_LIBRARY: &str = "cmzz9.dll";

/// Well under the ten seconds the remote thread is given, which is how long the window used to hold it.
const AT_ONCE: Duration = Duration::from_secs(6);

/// Where the child probe writes how the child it started ended. Unset, the probe returns at once.
const CHILD_OUT: &str = "CHRONO_LOADER_CHILD_PROBE_OUT";

/// The program the child probe starts.
const CHILD_TARGET: &str = "CHRONO_LOADER_CHILD_PROBE_TARGET";

/// The child probe's own name.
const CHILD_PROBE: &str = "probe_starts_a_child_and_writes_how_it_ended";

/// `SEM_FAILCRITICALERRORS`: no error window for a failure Windows meets on this process's behalf, and
/// a child inherits the mode (MS Learn, `SetErrorMode`).
const SEM_FAILCRITICALERRORS: u32 = 0x0001;

/// `STATUS_DLL_NOT_FOUND`, the code the loader ends a process with when a library it imports is missing.
const STATUS_DLL_NOT_FOUND: &str = "0xC0000135";

/// Well past the session's opening guard window of 300 ms (ADR-4).
const PAST_THE_GUARD: Duration = Duration::from_secs(1);

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetErrorMode() -> u32;
    fn SetErrorMode(mode: u32) -> u32;
}

/// Writes the error mode this process runs with, as a number.
#[test]
#[ignore = "the target of the session test in this file, not a test on its own"]
fn probe_reports_the_error_mode_it_runs_with() {
    let Some(out) = std::env::var_os(MODE_OUT) else {
        return;
    };
    // SAFETY: no arguments, no state - it reads this process's mode.
    let mode = unsafe { GetErrorMode() };
    std::fs::write(PathBuf::from(out), mode.to_string()).expect("the probe writes its mode");
}

/// Starts the program `CHILD_TARGET` names, waits for it, and writes its exit code in hexadecimal.
#[test]
#[ignore = "the target of the child session test in this file, not a test on its own"]
fn probe_starts_a_child_and_writes_how_it_ended() {
    let (Some(out), Some(target)) = (std::env::var_os(CHILD_OUT), std::env::var_os(CHILD_TARGET)) else {
        return;
    };
    // The child inherits this mode, so its missing library ends it at once instead of holding it on
    // Windows's error window, which nobody in a test run can close.
    // SAFETY: no pointers, it sets this process's mode.
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS) };
    let status = Command::new(target).status().expect("the child is created");
    let code = status.code().map_or_else(|| "none".to_string(), |c| format!("{:#X}", c as u32));
    std::fs::write(PathBuf::from(out), code).expect("the probe writes how the child ended");
    // A target gone inside the session's first 300 ms reads as one that handed its work on (ADR-4), and
    // the session then gives no verdict on its family at all.
    std::thread::sleep(PAST_THE_GUARD);
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

fn scratch(name: &str) -> PathBuf {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this test drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let dir = std::env::temp_dir().join(format!("chrono-loader-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// `chrono run` on `target`, started with the default error mode.
fn run_with_default_error_mode(target: &Path, args: &str, env: Option<(&str, &Path)>) -> (std::process::Output, Duration) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chrono"));
    command
        .args(["run", &target.display().to_string(), "--at", "2077-01-01T00:00:00", "--zone", "+00:00"])
        .creation_flags(CREATE_DEFAULT_ERROR_MODE);
    if !args.is_empty() {
        command.args(["--args", args]);
    }
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let started = Instant::now();
    let out = command.output().expect("the tool must run");
    (out, started.elapsed())
}

fn u16_at(image: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn u32_at(image: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// The file offset of an address relative to the image, through the section table.
fn offset_of(image: &[u8], rva: usize) -> Option<usize> {
    let header = u32_at(image, 0x3C)?;
    let sections = u16_at(image, header + 6)?;
    let optional_size = u16_at(image, header + 20)?;
    let table = header + 24 + optional_size;
    (0..sections).find_map(|i| {
        let entry = table + i * 40;
        let size = u32_at(image, entry + 8)?.max(u32_at(image, entry + 16)?);
        let start = u32_at(image, entry + 12)?;
        (start..start + size).contains(&rva).then(|| u32_at(image, entry + 20).map(|raw| raw + rva - start))?
    })
}

/// Rename the first imported library whose name is at least as long as `to`, in place, and return its old
/// name. `None` when the image has no import table this can read.
fn rename_an_import(image: &mut [u8], to: &str) -> Option<String> {
    let header = u32_at(image, 0x3C)?;
    let optional = header + 24;
    let directories = match u16_at(image, optional)? {
        0x10B => optional + 96,
        0x20B => optional + 112,
        _ => return None,
    };
    let imports = offset_of(image, u32_at(image, directories + 8)?)?;
    for descriptor in (imports..).step_by(20) {
        let name_rva = u32_at(image, descriptor + 12)?;
        if name_rva == 0 {
            return None;
        }
        let at = offset_of(image, name_rva)?;
        let len = image.get(at..)?.iter().position(|&b| b == 0)?;
        if len >= to.len() {
            let old = String::from_utf8_lossy(&image[at..at + len]).into_owned();
            image[at..at + len].fill(0);
            image[at..at + to.len()].copy_from_slice(to.as_bytes());
            return Some(old);
        }
    }
    None
}

#[test]
fn a_target_whose_library_is_missing_is_named_at_once_without_a_window() {
    let _session = one_session_at_a_time();
    let dir = scratch("missing");
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let mut image = std::fs::read(&me).expect("the test binary is readable");
    let renamed = rename_an_import(&mut image, MISSING_LIBRARY).expect("the test binary has an import to rename");
    let broken = dir.join("broken.exe");
    std::fs::write(&broken, &image).expect("the broken copy is written");

    let (out, took) = run_with_default_error_mode(&broken, "", None);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let context = || format!("renamed {renamed} to {MISSING_LIBRARY}, took {took:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");

    assert!(
        stdout.contains("[target.loader_dll_not_found]"),
        "the missing library was not named. {}",
        context()
    );
    assert!(stderr.contains("0xC0000135"), "the loader's status is not on the detail line. {}", context());
    assert_eq!(out.status.code(), Some(2), "a target that could not start is exit 2. {}", context());
    assert!(took < AT_ONCE, "the start waited, as it did on Windows's error window. {}", context());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_application_runs_with_the_error_mode_it_inherited() {
    let _session = one_session_at_a_time();
    let dir = scratch("mode");
    let file = dir.join("mode.txt");
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let (out, _) = run_with_default_error_mode(
        &me,
        &format!("--ignored --exact {MODE_PROBE} --test-threads 1"),
        Some((MODE_OUT, &file)),
    );
    let seen = std::fs::read_to_string(&file).unwrap_or_default();
    let context = || {
        format!(
            "probe: {seen:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    // The probe ran under the session, or the line below proves nothing.
    assert!(!seen.is_empty(), "the probe did not run. {}", context());
    // Started with the default mode, it must run with the default mode: the window switched off for its
    // loading has to be back on before its own code reads the mode.
    assert_eq!(seen, "0", "the application runs with a mode it did not inherit. {}", context());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child the application starts and Windows ends while loading it never ran a line of its own, so the
/// session does not name it among the processes on the real clock (R5-F8). The hook used to take the
/// loader's status for a loaded library, find no sign-in from the child and count it uncovered, which
/// turned a `works` session into `partial` over a program that never ran.
#[test]
fn a_child_windows_ends_while_loading_it_is_not_reported_as_running_on_the_real_clock() {
    let _session = one_session_at_a_time();
    let dir = scratch("child");
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let mut image = std::fs::read(&me).expect("the test binary is readable");
    let renamed = rename_an_import(&mut image, MISSING_LIBRARY).expect("the test binary has an import to rename");
    let broken = dir.join("broken.exe");
    std::fs::write(&broken, &image).expect("the broken copy is written");
    let file = dir.join("child.txt");

    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args(["run", &me.display().to_string(), "--at", "2077-01-01T00:00:00", "--zone", "+00:00", "--json"])
        .args(["--args", &format!("--ignored --exact {CHILD_PROBE} --test-threads 1")])
        .env(CHILD_OUT, &file)
        .env(CHILD_TARGET, &broken)
        .output()
        .expect("the tool must run");
    let ended = std::fs::read_to_string(&file).unwrap_or_default();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let context = || {
        format!(
            "renamed {renamed} to {MISSING_LIBRARY}, the child ended with {ended:?}\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    };

    // The child was started and its loader ended it, or nothing below is about such a child.
    assert_eq!(ended, STATUS_DLL_NOT_FOUND, "the child did not end the way a missing library ends it. {}", context());
    let verdict = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|event| event["type"] == "session_verdict")
        .unwrap_or_else(|| panic!("the session gave no verdict. {}", context()));
    assert_eq!(
        verdict["uncovered_children_total"], 0,
        "a child that never ran was counted as a process on the real clock. {}",
        context()
    );
    assert_eq!(verdict["verdict"], "works", "the session is not a works session. {}", context());
    assert_eq!(out.status.code(), Some(0), "a works session is exit 0. {}", context());
    let _ = std::fs::remove_dir_all(&dir);
}
