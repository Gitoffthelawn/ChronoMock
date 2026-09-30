//! A target the core was still starting ends with the core (R4-S1).
//!
//! The core starts the target suspended, injects the hook, and only then resumes it. The injection can
//! take up to ten seconds (`INJECT_TIMEOUT_MS` in the mechanism), and a core that died inside that stretch
//! left the target suspended for good: a process nobody sees, holding its executable and the hook library
//! open, so the folder they sit in cannot be deleted. The window kills a core that does not answer Stop
//! within two seconds, and Ctrl+C, `taskkill` and a crash end it the same way, so nothing the core does on
//! its way out can be what saves the target.
//!
//! The target is this test binary. Its thread-local-storage callback runs while the process initialises,
//! and in a process started suspended and then injected that is the injection's own thread, while the main
//! thread still sleeps - inside the core's `prepare`. With the probe variable set, the callback writes its
//! pid and ends the core that started it. The target must then be gone within a few seconds.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where the callback writes its pid and the core's. Unset, the callback does nothing.
const PROBE_OUT: &str = "CHRONO_LAUNCH_JOB_PROBE_OUT";

/// The code the callback ends the core with, one the core never exits with on its own.
const CORE_KILLED_WITH: u32 = 0x5A;

/// How long the tool may take. It ends as soon as it sees its core gone, so this only bounds a failure.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// How long the target may outlive the core. The job ends it the moment the core's handles close.
const GONE_WITHIN_MS: u32 = 5_000;

const DLL_PROCESS_ATTACH: u32 = 1;
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_OBJECT_0: u32 = 0;

/// `PROCESS_BASIC_INFORMATION`, pointer-wide fields as in `core_lost.rs`.
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
    fn GetCurrentProcessId() -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

/// The callback the loader calls for this binary. Listed between the C runtime's own markers, so the
/// loader finds it in the image's thread-local-storage directory.
#[used]
#[unsafe(link_section = ".CRT$XLC")]
static ON_PROCESS_START: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) = on_process_start;

/// Writes this process's pid and its parent's, then ends the parent - the core, which is still inside
/// `prepare` when this runs under a session. The line goes out first: once the core is gone the target may
/// be ended at any instruction, and a line written after that would never land.
unsafe extern "system" fn on_process_start(_module: *mut c_void, reason: u32, _reserved: *mut c_void) {
    if reason != DLL_PROCESS_ATTACH {
        return;
    }
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    // SAFETY: a query about this process, no arguments.
    let me = unsafe { GetCurrentProcessId() };
    let core = parent_pid().unwrap_or(0);
    let _ = std::fs::write(PathBuf::from(out), format!("{me} {core}"));
    if core != 0 {
        // SAFETY: a handle opened here, used once and closed here.
        unsafe {
            let process = OpenProcess(PROCESS_TERMINATE, 0, core);
            if !process.is_null() {
                TerminateProcess(process, CORE_KILLED_WITH);
                CloseHandle(process);
            }
        }
    }
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

/// The full path of the executable behind a process handle.
fn image_of(process: *mut c_void) -> Option<String> {
    let mut name = [0u16; 1024];
    let mut size = name.len() as u32;
    // SAFETY: the buffer and its length in characters, for a handle with query rights.
    let read = unsafe { QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut size) } != 0;
    read.then(|| String::from_utf16_lossy(&name[..size as usize]))
}

/// Whether the process `pid`, started from `image`, is still there after `ms` milliseconds. A pid that
/// now belongs to another executable is somebody else's, so the target is gone. One still there is ended
/// here, so a failing run does not leave it behind on the machine that ran the test.
///
/// The executable is compared by file name, not by full path: the kernel and `current_exe` can spell one
/// path two ways (a `subst` drive, a junction, a `\\?\` prefix), and a mismatch would read as "somebody
/// else's" and pass over a target left suspended. A running process whose name cannot be read cannot be
/// told apart either way, so that fails loudly instead of passing.
fn outlives(pid: u32, image: &Path, ms: u32) -> bool {
    let file = |path: &Path| path.file_name().map(|f| f.to_string_lossy().to_ascii_lowercase());
    // SAFETY: a handle opened here and closed on every path out.
    unsafe {
        let process = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return false;
        }
        let ours = match image_of(process) {
            Some(name) => file(Path::new(&name)) == file(image),
            None if WaitForSingleObject(process, 0) == WAIT_OBJECT_0 => false,
            None => {
                // Not ended: a process that cannot be named cannot be told from somebody else's.
                CloseHandle(process);
                panic!("pid {pid} is running and its executable cannot be read, so the check would prove nothing");
            }
        };
        let alive = ours && WaitForSingleObject(process, ms) != WAIT_OBJECT_0;
        if alive {
            TerminateProcess(process, 1);
        }
        CloseHandle(process);
        alive
    }
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
}

#[test]
fn a_target_the_core_was_still_starting_ends_with_the_core() {
    let library = injected_library();
    assert!(
        library.is_file(),
        "this test drives a real session and needs {}, which `cargo test` does not build. \
         Run `cargo build --workspace` first - CI and tools/gates.ps1 both do that.",
        library.display()
    );
    let dir = std::env::temp_dir().join(format!("chrono-launch-job-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join("probe.txt");
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");

    let me = std::env::current_exe().expect("the test binary knows its own path");
    // To files, not pipes: a target left suspended holds a copy of the tool's error stream, and reading a
    // pipe to its end would then wait for ever instead of failing.
    let mut run = Command::new(env!("CARGO_BIN_EXE_chrono"))
        .args([
            "run",
            &me.display().to_string(),
            "--at",
            "2077-01-01T00:00:00",
            "--zone",
            "+00:00",
            // Should the main thread ever run, it lists the tests and leaves rather than run this one again.
            "--args",
            "--list",
        ])
        .env(PROBE_OUT, &file)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).expect("a file for the output"))
        .stderr(std::fs::File::create(&stderr_path).expect("a file for the errors"))
        .spawn()
        .expect("the tool must run");
    let deadline = Instant::now() + RUN_LIMIT;
    let code = loop {
        if let Some(status) = run.try_wait().expect("the run can be asked about") {
            break status.code();
        }
        if Instant::now() >= deadline {
            let _ = run.kill();
            let _ = run.wait();
            panic!("the tool did not end within {RUN_LIMIT:?} after its core was ended");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let probe = std::fs::read_to_string(&file).unwrap_or_default();
    let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    let context = || format!("exit: {code:?}\nprobe: {probe:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");

    // The case this file is about, or it proves nothing: the callback ran under the session and named the
    // core as its parent, which it then ended while the core was still starting it.
    let fields: Vec<u32> = probe.split_whitespace().filter_map(|f| f.parse().ok()).collect();
    let [target, core] = fields[..] else {
        panic!("the callback did not run under the session. {}", context());
    };
    assert_ne!(core, 0, "the callback could not name the core. {}", context());

    assert!(
        !outlives(target, &me, GONE_WITHIN_MS),
        "the target outlived, suspended, the core that was starting it (ended here instead). {}",
        context()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
