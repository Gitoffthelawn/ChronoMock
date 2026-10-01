//! A target that ends while Windows loads it is named for what failed, at once and without an error
//! window, and an application that loads fine runs with the error mode it inherited (R4-S6).
//!
//! The target starts suspended, and the remote thread that loads the hook is the first to run in it, so
//! that thread does the target's own loading first. A missing static import ended the process right
//! there, and the session reported a successful injection followed by a vanished target - or, started
//! from Explorer, with Windows's error window up, a loader lock ten seconds later. Measured on x64 and x86
//! in tools/probes/r4-9. The core now switches that window off while the target loads and back before its
//! first instruction.
//!
//! Both sessions here run with the error mode a program started from Explorer has (the default, which
//! shows the window), whatever mode the test runner itself was given. The broken target is a copy of
//! this test binary with one imported library renamed, so nothing has to be built for it.

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

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetErrorMode() -> u32;
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
