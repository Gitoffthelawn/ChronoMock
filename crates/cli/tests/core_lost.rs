//! A session whose core stopped before it closed it is not reported as its verdict (R4-W3).
//!
//! The core sends the parent's verdict inside the guard window, a fraction of a second into the session,
//! and `ended` only when the session closes. A core that died between the two left `chrono run` standing
//! on the first: a WORKS report over the call counts of that first blink, an evidence file with no
//! unreliable banner, and whatever exit code the killer chose. Measured with `taskkill /F`, which ends a
//! process with 1 - the usage-error code - and nothing was said on stderr at all.
//!
//! The target is this test binary itself, as in `session_identity.rs`: the ignored probe below reads the
//! clock, waits past the guard window, and ends its own parent - the core, which started it - with exit
//! code 1, the case that was measured. Then it exits, so the session has nothing left to follow.

use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Where the probe writes what it did. Unset, the probe returns at once.
const PROBE_OUT: &str = "CHRONO_CORE_LOST_PROBE_OUT";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_ends_the_core_that_started_it";

/// How long the probe waits before it ends the core, in real milliseconds. Past the guard window and the
/// first poll, which is when the parent's verdict goes out - the verdict the old report stood on.
const END_CORE_AFTER_MS: u64 = 1_500;

/// The code the probe ends the core with: what `taskkill /F` uses, and a code the contract gives to
/// something else entirely, so passing it through is the failure this file guards.
const CORE_KILLED_WITH: u32 = 1;

const PROCESS_TERMINATE: u32 = 0x0001;

/// `PROCESS_BASIC_INFORMATION`. The two 32-bit fields are pointer-wide here because the structure pads
/// them to that on 64-bit Windows, and on 32-bit a pointer is 32 bits anyway, so one layout serves both.
#[repr(C)]
#[derive(Default)]
struct BasicInformation {
    exit_status: isize,
    peb: usize,
    affinity: usize,
    base_priority: isize,
    pid: usize,
    parent_pid: usize,
}

#[cfg_attr(target_arch = "x86", link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: *mut c_void,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

/// The pid of the process that started this one, when Windows says it.
fn parent_pid() -> Option<u32> {
    let mut info = BasicInformation::default();
    // SAFETY: class 0 is ProcessBasicInformation, and the buffer is that structure at its own size.
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            0,
            (&raw mut info).cast(),
            size_of::<BasicInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return None;
    }
    u32::try_from(info.parent_pid).ok()
}

/// End the process `pid` with `code`, and say whether it was ended.
fn terminate(pid: u32, code: u32) -> bool {
    // SAFETY: a handle opened here, used once and closed here.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return false;
        }
        let ended = TerminateProcess(process, code) != 0;
        CloseHandle(process);
        ended
    }
}

/// Reads the clock once, so the session has a call to count, waits, ends the core that started it, and
/// writes the core's pid and 1 when it was ended.
#[test]
#[ignore = "the target of the session test in this file, not a test on its own"]
fn probe_ends_the_core_that_started_it() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let _ = SystemTime::now();
    std::thread::sleep(Duration::from_millis(END_CORE_AFTER_MS));
    let core = parent_pid();
    let ended = core.is_some_and(|pid| terminate(pid, CORE_KILLED_WITH));
    std::fs::write(PathBuf::from(out), format!("{} {}", core.unwrap_or(0), u8::from(ended))).expect("the probe writes its line");
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

#[test]
fn a_core_that_stopped_before_it_closed_the_session_is_no_verdict_and_exits_3() {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this test drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let dir = std::env::temp_dir().join(format!("chrono-core-lost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join("probe.txt");
    let evidence = dir.join("evidence.txt");

    let me = std::env::current_exe().expect("the test binary knows its own path");
    // The probe inherits the tool's error stream, so `output()` returns once the probe is done as well.
    let out = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &me.display().to_string(),
            "--at",
            "2077-01-01T00:00:00",
            "--zone",
            "+00:00",
            "--report",
            &evidence.display().to_string(),
            "--args",
            &format!("--ignored --exact {PROBE} --test-threads 1"),
        ])
        .env(PROBE_OUT, &file)
        .output()
        .expect("the tool must run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let probe = std::fs::read_to_string(&file).unwrap_or_default();
    let report = std::fs::read_to_string(&evidence).unwrap_or_default();
    let context = || format!("probe: {probe:?}\nstdout:\n{stdout}\nstderr:\n{stderr}\nevidence:\n{report}");

    // The case this file is about, or it proves nothing: the probe ran under the session, ended the core,
    // and the verdict the old report stood on had already arrived.
    assert!(probe.ends_with(" 1"), "the probe did not end the core. {}", context());
    assert!(stdout.contains("verdict:  WORKS"), "the parent's verdict had not arrived before the core stopped. {}", context());

    assert!(
        stdout.contains("CUT SHORT - the core stopped before it closed the session"),
        "the report stood on the verdict sent at the start. {}",
        context()
    );
    assert!(
        report.starts_with("!! UNRELIABLE EVIDENCE"),
        "the evidence file cites a session the core never closed as proof. {}",
        context()
    );
    assert!(
        stderr.contains(&format!("the core stopped before it closed the session (exit code {CORE_KILLED_WITH})")),
        "nothing said the core stopped, or with what. {}",
        context()
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "the core's own code was passed through as the session's result. {}",
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
