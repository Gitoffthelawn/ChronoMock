//! What the injected library must survive in an application that does nothing wrong (R4/7).
//!
//! Two ordinary things an application may do used to take it down with the hook in it. It may hand a
//! clock function a buffer at an address that is not a multiple of the value's alignment - a packed
//! structure does that, and x86 and x64 read and write there without complaint, so the real function
//! answers. The hook stored through such a pointer as if it were aligned, which Rust calls undefined
//! behaviour, and its debug build checks it and aborts the application (R4-N3). And it may call
//! `FreeLibrary` on a library it finds loaded in itself. The hook's detours stay written into the
//! system's code, so once the library unmapped, the next clock read jumped into freed memory (R4-N2).
//!
//! The target is this test binary itself, as in `session_identity.rs`: the ignored probe below does its
//! work only when the variable names a file. In one mode it calls every detour that reads or writes
//! through a pointer the application passed, each through an odd address, and writes a line after each
//! call. In the other it frees the hook and reads the clock once more. A line that is missing is where
//! the application died.
//!
//! The same probe runs first without a session, which is the control: the functions themselves accept
//! every buffer here, so the session demands nothing of the application that Windows does not.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where the probe writes what it read. Unset, the probe returns at once.
const PROBE_OUT: &str = "CHRONO_HOOK_INTEGRITY_PROBE_OUT";

/// Which half of the probe runs: `odd` for the buffers at odd addresses, `free` for the freed library.
const PROBE_MODE: &str = "CHRONO_HOOK_INTEGRITY_PROBE_MODE";

/// The probe's own name, which is how the binary is asked to run it and nothing else.
const PROBE: &str = "probe_reads_through_odd_addresses_and_frees_the_hook";

/// The child the probe starts through `CreateProcessW`, which only writes one line.
const CHILD: &str = "probe_child_writes_one_line";

/// The session's moment, far enough ahead that no real clock reading can be mistaken for it.
const SESSION_AT: &str = "2077-01-01T00:00:00";

/// The session's zone. Not the zone of any machine this runs on in summer or winter, so the zone the
/// application reads can only be the session's.
const SESSION_ZONE: &str = "+05:00";

/// The bias `GetTimeZoneInformation` reports for `SESSION_ZONE`, in minutes (UTC = local + bias).
const SESSION_BIAS: i32 = -300;

/// Seconds between the Unix epoch and 2070-01-01, a line every reading on the session's clock is past
/// and no real reading this century reaches.
const SESSION_LINE: u64 = 3_155_760_000;

/// The core runs one session at a time, and the tests below each run one.
static ONE_SESSION: Mutex<()> = Mutex::new(());

type Handle = *mut c_void;

#[cfg_attr(target_arch = "x86", link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(ft: *mut u8);
    fn GetSystemTimePreciseAsFileTime(ft: *mut u8);
    fn GetSystemTime(st: *mut u8);
    fn GetLocalTime(st: *mut u8);
    fn GetTimeZoneInformation(tzi: *mut u8) -> u32;
    fn GetDynamicTimeZoneInformation(dtzi: *mut u8) -> u32;
    fn FileTimeToLocalFileTime(utc: *const u8, local: *mut u8) -> i32;
    fn LocalFileTimeToFileTime(local: *const u8, utc: *mut u8) -> i32;
    fn SystemTimeToTzSpecificLocalTime(zone: *const c_void, utc: *const u8, local: *mut u8) -> i32;
    fn SystemTimeToTzSpecificLocalTimeEx(zone: *const c_void, utc: *const u8, local: *mut u8) -> i32;
    fn TzSpecificLocalTimeToSystemTime(zone: *const c_void, local: *const u8, utc: *mut u8) -> i32;
    fn TzSpecificLocalTimeToSystemTimeEx(zone: *const c_void, local: *const u8, utc: *mut u8) -> i32;
    fn QueryUnbiasedInterruptTime(time: *mut u8) -> i32;
    fn QueryPerformanceCounter(count: *mut u8) -> i32;
    fn CreateWaitableTimerW(attributes: *const c_void, manual_reset: i32, name: *const u16) -> Handle;
    fn SetWaitableTimer(timer: Handle, due: *const u8, period: i32, routine: *const c_void, arg: *const c_void, resume: i32) -> i32;
    fn SetWaitableTimerEx(
        timer: Handle,
        due: *const u8,
        period: i32,
        routine: *const c_void,
        arg: *const c_void,
        wake_context: *const c_void,
        tolerable_delay: u32,
    ) -> i32;
    fn CancelWaitableTimer(timer: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn CreateThreadpoolTimer(callback: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void), context: *mut c_void, environment: *const c_void) -> Handle;
    fn SetThreadpoolTimer(timer: Handle, due: *const u8, period: u32, window: u32);
    fn SetThreadpoolTimerEx(timer: Handle, due: *const u8, period: u32, window: u32) -> i32;
    fn WaitForThreadpoolTimerCallbacks(timer: Handle, cancel_pending: i32);
    fn CloseThreadpoolTimer(timer: Handle);
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit_handles: i32,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const c_void,
        information: *mut u8,
    ) -> i32;
    fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
    fn GetModuleHandleA(name: *const core::ffi::c_char) -> Handle;
    fn FreeLibrary(module: Handle) -> i32;
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: u32, information: *const c_void, length: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
}

#[cfg_attr(target_arch = "x86", link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated"))]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtQuerySystemTime(time: *mut u8) -> i32;
    fn NtDelayExecution(alertable: u8, interval: *const u8) -> i32;
    /// The reference clock. The hook never touches it (ADR-2), so it stays real under any session.
    fn NtQueryPerformanceCounter(counter: *mut i64, frequency: *mut i64) -> i32;
}

/// A buffer whose first byte stands one past an 8-byte boundary, so every value written at `odd()` is
/// misaligned for any type wider than a byte.
#[repr(C, align(8))]
struct Odd([u8; 1032]);

impl Odd {
    fn new() -> Self {
        Self([0; 1032])
    }
    fn odd(&mut self) -> *mut u8 {
        self.0[1..].as_mut_ptr()
    }
    /// The value at the odd address, read as `T` without assuming its alignment.
    fn read<T: Copy>(&self) -> T {
        // SAFETY: the buffer is 1032 bytes, every `T` read here is far smaller, and the read is unaligned.
        unsafe { std::ptr::read_unaligned(self.0[1..].as_ptr().cast::<T>()) }
    }
    /// Write `value` at the odd address without assuming its alignment.
    fn write<T: Copy>(&mut self, value: T) {
        // SAFETY: as in `read`.
        unsafe { std::ptr::write_unaligned(self.odd().cast::<T>(), value) }
    }
}

/// Real milliseconds by the unhooked reference clock.
fn real_ms() -> f64 {
    let (mut counter, mut frequency) = (0i64, 0i64);
    // SAFETY: both pointers are to live locals.
    unsafe { NtQueryPerformanceCounter(&mut counter, &mut frequency) };
    counter as f64 * 1000.0 / frequency as f64
}

/// Spin on the unhooked clock, so no hooked wait is involved.
fn spin(ms: f64) {
    let start = real_ms();
    while real_ms() - start < ms {}
}

/// Unix seconds from FILETIME ticks.
fn unix_seconds(ticks: u64) -> u64 {
    (ticks / 10_000_000).saturating_sub(11_644_473_600)
}

/// Whether the hook library is loaded in this process.
fn hooked() -> bool {
    // SAFETY: a lookup by name with a NUL-terminated literal, no other state.
    !unsafe { GetModuleHandleA(c"chrono_hook.dll".as_ptr()) }.is_null()
}

fn append(file: &Path, line: &str) {
    use std::io::Write;
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(file).expect("the probe opens its file");
    out.write_all(format!("{line}\n").as_bytes()).expect("the probe writes its line");
}

/// `JobObjectExtendedLimitInformation`.
const JOB_EXTENDED_LIMITS: u32 = 9;

/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
const KILL_ON_JOB_CLOSE: u32 = 0x2000;

/// Put this process, and every process it starts from here on, in a job that ends them all when this
/// process ends. Answers whether that worked.
///
/// Why the probe needs it: the hook starts every child suspended and resumes it after injecting it, so
/// a probe that dies inside `CreateProcessW` - which is what a revert of R4-N3 in `inherit_into_child`
/// does - leaves its child suspended for good, holding the output of the session. The test would then
/// wait on nothing forever instead of failing. The job handle is never closed on purpose: closing the
/// last one ends every process in the job, this one included, so it closes when this process ends.
fn contain_family() -> bool {
    // JOBOBJECT_EXTENDED_LIMIT_INFORMATION: 144 bytes on 64-bit, 112 on 32-bit, with LimitFlags at byte
    // 16 on both (two LARGE_INTEGER time limits come first).
    let mut limits = [0u32; 36];
    limits[4] = KILL_ON_JOB_CLOSE;
    let size: u32 = if cfg!(target_pointer_width = "64") { 144 } else { 112 };
    // SAFETY: a new unnamed job, a buffer larger than the structure with its size given, and the
    // pseudo-handle of this process.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        !job.is_null()
            && SetInformationJobObject(job, JOB_EXTENDED_LIMITS, limits.as_ptr().cast(), size) != 0
            && AssignProcessToJobObject(job, GetCurrentProcess()) != 0
    }
}

/// A thread-pool timer callback that is never meant to run: the timer is due a minute out and is
/// cancelled at once.
unsafe extern "system" fn never_due(_instance: *mut c_void, _context: *mut c_void, _timer: *mut c_void) {}

/// A relative due time a minute out, in the signed 100 ns units the timer functions take.
const A_MINUTE_OUT: i64 = -600_000_000;

/// The child the probe starts through `CreateProcessW`. Writes the wall seconds it read and whether the
/// hook is loaded in it.
#[test]
#[ignore = "the child of the probe in this file, not a test on its own"]
fn probe_child_writes_one_line() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let mut ft = Odd::new();
    // SAFETY: a writable buffer of 1032 bytes, of which the function writes 8.
    unsafe { GetSystemTimeAsFileTime(ft.odd()) };
    append(Path::new(&out), &format!("child {} {}", unix_seconds(ft.read::<u64>()), u8::from(hooked())));
}

/// Calls every detour that reads or writes through a pointer the application passed, each through an
/// odd address, and writes `<name> <value>` after each call and `done` at the end. In `free` mode it
/// frees the hook instead and reads the clock once more.
#[test]
#[ignore = "the target of the session tests in this file, not a test on its own"]
fn probe_reads_through_odd_addresses_and_frees_the_hook() {
    let Some(out) = std::env::var_os(PROBE_OUT) else {
        return;
    };
    let out = PathBuf::from(out);
    append(&out, &format!("job {}", u8::from(contain_family())));
    match std::env::var(PROBE_MODE).as_deref() {
        Ok("free") => free_the_hook(&out),
        _ => read_through_odd_addresses(&out),
    }
    append(&out, "done");
    // Alive past the session's opening guard window (ADR-4), spun on the unhooked clock.
    spin(800.0);
}

/// The `free` half: frees the hook the way an application would free any library it found loaded in
/// itself, then reads the clock.
fn free_the_hook(out: &Path) {
    let mut ft = Odd::new();
    // SAFETY: a writable buffer, of which the function writes 8 bytes.
    unsafe { GetSystemTimeAsFileTime(ft.odd()) };
    append(out, &format!("before {} {}", unix_seconds(ft.read::<u64>()), u8::from(hooked())));
    // SAFETY: a NUL-terminated name, and a null answer is not freed.
    let freed = unsafe {
        let module = GetModuleHandleA(c"chrono_hook.dll".as_ptr());
        if module.is_null() { -1 } else { FreeLibrary(module) }
    };
    append(out, &format!("freed {freed}"));
    // SAFETY: as above.
    unsafe { GetSystemTimeAsFileTime(ft.odd()) };
    append(out, &format!("after {} {}", unix_seconds(ft.read::<u64>()), u8::from(hooked())));
}

/// The `odd` half.
fn read_through_odd_addresses(out: &Path) {
    let mut a = Odd::new();
    let mut b = Odd::new();
    let line = |name: &str, value: String| append(out, &format!("{name} {value}"));

    // SAFETY, for every call below: each pointer is into a live 1032-byte buffer, which holds any of the
    // structures these functions read or write, and a null zone pointer is documented as "the current
    // zone". Nothing here is freed or kept past the call.
    unsafe {
        GetSystemTimeAsFileTime(a.odd());
        line("gstaft", unix_seconds(a.read::<u64>()).to_string());
        GetSystemTimePreciseAsFileTime(a.odd());
        line("gstpaft", unix_seconds(a.read::<u64>()).to_string());
        GetSystemTime(a.odd());
        line("gst", a.read::<u16>().to_string());
        GetLocalTime(a.odd());
        line("glt", a.read::<u16>().to_string());
        NtQuerySystemTime(a.odd());
        line("ntqst", unix_seconds(a.read::<u64>()).to_string());
        GetTimeZoneInformation(a.odd());
        line("gtzi", a.read::<i32>().to_string());
        GetDynamicTimeZoneInformation(a.odd());
        line("gdtzi", a.read::<i32>().to_string());

        // The conversions: UTC midnight of the session's day in, the local value out, and back.
        a.write::<u64>(0x01DC_0000_0000_0000);
        let ok = FileTimeToLocalFileTime(a.odd(), b.odd());
        line("ftlft", format!("{ok} {}", (b.read::<u64>() as i64 - a.read::<u64>() as i64) / 600_000_000));
        let ok = LocalFileTimeToFileTime(b.odd(), a.odd());
        line("lftft", format!("{ok} {}", (b.read::<u64>() as i64 - a.read::<u64>() as i64) / 600_000_000));
        let noon: [u16; 8] = [2030, 6, 0, 15, 12, 0, 0, 0];
        a.write(noon);
        let ok = SystemTimeToTzSpecificLocalTime(std::ptr::null(), a.odd(), b.odd());
        line("stsl", format!("{ok} {}", b.read::<[u16; 8]>()[4]));
        let ok = SystemTimeToTzSpecificLocalTimeEx(std::ptr::null(), a.odd(), b.odd());
        line("stslex", format!("{ok} {}", b.read::<[u16; 8]>()[4]));
        let ok = TzSpecificLocalTimeToSystemTime(std::ptr::null(), a.odd(), b.odd());
        line("tltst", format!("{ok} {}", b.read::<[u16; 8]>()[4]));
        let ok = TzSpecificLocalTimeToSystemTimeEx(std::ptr::null(), a.odd(), b.odd());
        line("tltstex", format!("{ok} {}", b.read::<[u16; 8]>()[4]));

        // The duration axis and QPC, hooked only under --scale-duration and --scale-qpc.
        let ok = QueryUnbiasedInterruptTime(a.odd());
        line("quit", format!("{ok} {}", a.read::<u64>()));
        let ok = QueryPerformanceCounter(a.odd());
        line("qpc", format!("{ok} {}", a.read::<i64>()));
        a.write::<i64>(-10_000);
        line("ntdelay", NtDelayExecution(0, a.odd()).to_string());

        // Timers: armed a minute out through an odd due time, then cancelled.
        let timer = CreateWaitableTimerW(std::ptr::null(), 1, std::ptr::null());
        a.write::<i64>(A_MINUTE_OUT);
        line("swt", SetWaitableTimer(timer, a.odd(), 0, std::ptr::null(), std::ptr::null(), 0).to_string());
        let ok = SetWaitableTimerEx(timer, a.odd(), 0, std::ptr::null(), std::ptr::null(), std::ptr::null(), 0);
        line("swtex", ok.to_string());
        CancelWaitableTimer(timer);
        CloseHandle(timer);
        let pool = CreateThreadpoolTimer(never_due, std::ptr::null_mut(), std::ptr::null());
        SetThreadpoolTimer(pool, a.odd(), 0, 0);
        line("tptimer", u8::from(!pool.is_null()).to_string());
        line("tptimerex", SetThreadpoolTimerEx(pool, a.odd(), 0, 0).to_string());
        SetThreadpoolTimer(pool, std::ptr::null(), 0, 0);
        WaitForThreadpoolTimerCallbacks(pool, 1);
        CloseThreadpoolTimer(pool);

        // A child, with its PROCESS_INFORMATION at an odd address.
        line("cpw", start_child(&mut b).to_string());
    }
}

/// Starts `probe_child_writes_one_line` through `CreateProcessW` with the process information at an odd
/// address, waits for it, and answers its exit code, or -1 when it did not start.
///
/// # Safety
/// `info` is only written by the call and read back unaligned.
unsafe fn start_child(info: &mut Odd) -> i64 {
    let me = std::env::current_exe().expect("the probe knows its own path");
    let mut command: Vec<u16> =
        format!("\"{}\" --ignored --exact {CHILD} --test-threads 1", me.display()).encode_utf16().chain([0]).collect();
    // STARTUPINFOW: its size first, everything else zero.
    let mut startup = [0u64; 16];
    startup[0] = if cfg!(target_pointer_width = "64") { 104 } else { 68 };
    // SAFETY: the command line is mutable and NUL-terminated, the startup block is zeroed with its size
    // set, and the information buffer holds a PROCESS_INFORMATION at its odd address.
    unsafe {
        let started = CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            startup.as_ptr().cast(),
            info.odd(),
        );
        if started == 0 {
            return -1;
        }
        let handles = info.read::<[Handle; 2]>();
        WaitForSingleObject(handles[0], 30_000);
        let mut code = 0u32;
        GetExitCodeProcess(handles[0], &mut code);
        CloseHandle(handles[1]);
        CloseHandle(handles[0]);
        i64::from(code)
    }
}

/// The injected library, which `cargo test` does not build (`dry_run.rs` has the whole story).
fn injected_library() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chrono"))
        .parent()
        .expect("the binary under test lives in a directory")
        .join("chrono_hook.dll")
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

/// The probe's arguments to the test harness.
const PROBE_ARGS: [&str; 5] = ["--ignored", "--exact", PROBE, "--test-threads", "1"];

/// How long one run may take. A run of the probe takes a few seconds, so reaching this means something
/// was left waiting, and the test says so instead of waiting with it.
const RUN_LIMIT: Duration = Duration::from_secs(120);

/// What one bounded run left behind.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run `command` with its output in files beside `file`, and wait for the PROCESS, not for the end of
/// its streams: a process it leaves behind may hold a copy of them, and waiting for their end is
/// waiting for that process. Killed and reported at `RUN_LIMIT`.
fn bounded(mut command: Command, file: &Path) -> Run {
    let out_path = file.with_extension("stdout");
    let err_path = file.with_extension("stderr");
    let stdout = std::fs::File::create(&out_path).expect("a file for the output");
    let stderr = std::fs::File::create(&err_path).expect("a file for the errors");
    let mut child = command.stdin(Stdio::null()).stdout(stdout).stderr(stderr).spawn().expect("the run starts");
    let deadline = Instant::now() + RUN_LIMIT;
    let code = loop {
        if let Some(status) = child.try_wait().expect("the run can be asked about") {
            break status.code();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the run did not end within {RUN_LIMIT:?} and was killed - a process it started was left waiting. \
                 probe: {:?}",
                std::fs::read_to_string(file).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Run {
        code,
        stdout: std::fs::read_to_string(&out_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&err_path).unwrap_or_default(),
    }
}

/// The probe in `mode`, run alone.
fn alone(mode: &str, file: &Path) -> Run {
    let mut command = Command::new(std::env::current_exe().expect("the test binary knows its own path"));
    command.args(PROBE_ARGS).env(PROBE_OUT, file).env(PROBE_MODE, mode);
    bounded(command, file)
}

/// The probe in `mode` under a session scaling the duration axis and QPC, so every detour is installed.
fn under_session(mode: &str, file: &Path) -> Run {
    let me = std::env::current_exe().expect("the test binary knows its own path");
    let mut command = Command::new(env!("CARGO_BIN_EXE_chrono"));
    command
        .args([
            "run",
            &me.display().to_string(),
            "--at",
            SESSION_AT,
            "--zone",
            SESSION_ZONE,
            "--scale-duration",
            "--scale-qpc",
            "--json",
            "--args",
            &PROBE_ARGS.join(" "),
        ])
        .env(PROBE_OUT, file)
        .env(PROBE_MODE, mode);
    bounded(command, file)
}

/// What the probe wrote, by the first word of each line.
fn lines(file: &Path) -> HashMap<String, Vec<String>> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut words = l.split_whitespace().map(str::to_owned);
            Some((words.next()?, words.collect()))
        })
        .collect()
}

/// Every name the `odd` half writes, in order. The one after the last line present is where it died.
const ODD_NAMES: [&str; 23] = [
    "gstaft", "gstpaft", "gst", "glt", "ntqst", "gtzi", "gdtzi", "ftlft", "lftft", "stsl", "stslex", "tltst",
    "tltstex", "quit", "qpc", "ntdelay", "swt", "swtex", "tptimer", "tptimerex", "child", "cpw", "done",
];

/// The first name the probe did not write, if any.
fn first_missing(seen: &HashMap<String, Vec<String>>) -> Option<&'static str> {
    ODD_NAMES.into_iter().find(|name| !seen.contains_key(*name))
}

/// The first value of `name` as a number.
fn number<T: std::str::FromStr>(seen: &HashMap<String, Vec<String>>, name: &str, at: usize) -> Option<T> {
    seen.get(name)?.get(at)?.parse().ok()
}

/// Every clock and timer function a session hooks answers a buffer at an odd address as Windows does,
/// and on the session's clock, instead of taking the application down (R4-N3).
#[test]
fn every_clock_read_through_an_odd_address_answers_on_the_session_clock() {
    let _one = ONE_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let dir = prepare("hook-odd");

    // The control: Windows itself answers every one of these calls, so the probe asks nothing unusual.
    let control_file = dir.join("control.txt");
    let control = alone("odd", &control_file);
    let seen = lines(&control_file);
    assert_eq!(
        first_missing(&seen),
        None,
        "without a session the probe stopped before this call, so Windows itself refuses it and the test \
         proves nothing: {seen:?} {}",
        control.stdout
    );
    assert!(number::<u64>(&seen, "gstaft", 0).is_some_and(|s| s < SESSION_LINE), "the control read a session date: {seen:?}");

    let file = dir.join("session.txt");
    let out = under_session("odd", &file);
    let seen = lines(&file);
    let context = || {
        format!(
            "probe: {seen:?} exit: {:?} stdout: {} stderr: {}",
            out.code,
            out.stdout,
            out.stderr
        )
    };
    assert!(seen.contains_key("gstaft"), "the probe wrote nothing under the session. {}", context());
    assert_eq!(
        first_missing(&seen),
        None,
        "the application died in the call before the first missing line - the hook dereferenced a buffer \
         at an odd address the way it may only dereference an aligned one (R4-N3). {}",
        context()
    );
    for name in ["gstaft", "gstpaft", "ntqst", "child"] {
        assert!(
            number::<u64>(&seen, name, 0).is_some_and(|s| s > SESSION_LINE),
            "{name} did not read the session's clock. {}",
            context()
        );
    }
    // The UTC year too: the session starts at midnight five hours east, which is still 2076 in UTC.
    for name in ["gst", "glt"] {
        assert!(number::<u16>(&seen, name, 0).is_some_and(|y| y >= 2070), "{name} did not read the session's year. {}", context());
    }
    for name in ["gtzi", "gdtzi"] {
        assert_eq!(number::<i32>(&seen, name, 0), Some(SESSION_BIAS), "{name} did not report the session's zone. {}", context());
    }
    // Five hours east: local is UTC plus 300 minutes, and the way back is the same distance.
    assert_eq!(number::<i64>(&seen, "ftlft", 1), Some(300), "the UTC to local conversion is not the session's. {}", context());
    assert_eq!(number::<i64>(&seen, "lftft", 1), Some(300), "the local to UTC conversion is not the session's. {}", context());
    assert_eq!(number::<u16>(&seen, "stsl", 1), Some(17), "noon UTC is not 17:00 in the session's zone. {}", context());
    assert_eq!(number::<u16>(&seen, "tltst", 1), Some(7), "noon in the session's zone is not 07:00 UTC. {}", context());
    assert_eq!(number::<i64>(&seen, "cpw", 0), Some(0), "the child did not run to its end. {}", context());
    let _ = std::fs::remove_dir_all(&dir);
}

/// An application that frees the hook library it finds loaded in itself keeps running, and keeps the
/// session's clock: the library is pinned, because its detours stay written into the system's code
/// (R4-N2).
#[test]
fn an_application_that_frees_the_hook_keeps_running_on_the_session_clock() {
    let _one = ONE_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let dir = prepare("hook-free");

    // The control: alone, there is no hook to free, and the probe reads the real clock before and after.
    let control_file = dir.join("control.txt");
    let control = alone("free", &control_file);
    let seen = lines(&control_file);
    assert!(
        number::<u64>(&seen, "after", 0).is_some_and(|s| s < SESSION_LINE) && seen.contains_key("done"),
        "the probe did not run to its end without a session: {seen:?} {}",
        control.stdout
    );

    let file = dir.join("session.txt");
    let out = under_session("free", &file);
    let seen = lines(&file);
    let context = || {
        format!(
            "probe: {seen:?} exit: {:?} stdout: {} stderr: {}",
            out.code,
            out.stdout,
            out.stderr
        )
    };
    assert!(
        number::<u64>(&seen, "before", 0).is_some_and(|s| s > SESSION_LINE) && number::<u8>(&seen, "before", 1) == Some(1),
        "the application did not start on the session's clock with the hook loaded, so this proves nothing. {}",
        context()
    );
    assert!(seen.contains_key("freed"), "the application did not get as far as freeing the hook. {}", context());
    assert!(
        number::<u64>(&seen, "after", 0).is_some_and(|s| s > SESSION_LINE) && seen.contains_key("done"),
        "after it freed the hook library the application died or left the session's clock - the library \
         unmapped under its own detours (R4-N2). {}",
        context()
    );
    assert_eq!(number::<u8>(&seen, "after", 1), Some(1), "the hook library is no longer loaded. {}", context());
    let _ = std::fs::remove_dir_all(&dir);
}
