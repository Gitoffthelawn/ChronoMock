//! What Windows's loader did to the target while the hook was being injected (R4-S6, R4-N10).
//!
//! The target is created suspended, so the first thread that runs in it is the remote thread that loads
//! the hook, and the first thread of a process does that process's own loading before its start routine:
//! the static imports and their start-up code, then `LoadLibraryW` of the hook. A target whose static
//! import is missing, built for the other bitness, lacking a function or refusing to initialise ends right
//! there, with the loader's status as its exit code. That used to read as a successful injection followed
//! by a target that vanished (exit 12, "suspected single-instance app"), or, when Windows showed its error
//! window and waited for an answer, as a loader lock ten seconds later. Both were measured on x64 and x86
//! (`tools/probes/r4-9`), and neither was true.
//!
//! Two things fix it. The end of the wait is read in the order that tells the truth ([`outcome`]): the
//! target's own exit code first, then the thread's. And Windows's error window is switched off for the
//! target while it loads and switched back before its entry point ([`QuietLoader`]), so a failed
//! load ends in milliseconds with its status instead of waiting on a window nobody may be looking at, and
//! no window outlives the start that raised it.

use std::ffi::c_void;

use windows::core::s;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

use crate::STILL_ACTIVE_CODE;

/// How the wait for the remote `LoadLibraryW` thread ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// The thread finished - on its own, or because the target ended and took it along.
    ThreadDone,
    /// It did not, within the time allowed.
    TimedOut,
    /// The wait itself failed, so nothing is known about either.
    Failed,
}

/// The target process as it stood right after that wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessAfter {
    /// Whether the process object was signalled, which is what says it has ended.
    pub signalled: bool,
    /// Its exit code as `GetExitCodeProcess` read it, `None` when the read failed.
    pub code: Option<u32>,
}

/// What the injection came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// `LoadLibraryW` returned a module.
    Loaded,
    /// `LoadLibraryW` returned NULL in a target that is still there.
    LibraryNotLoaded,
    /// The target ended while it was being loaded, with this exit code.
    TargetEnded(u32),
    /// The target ended and its exit code could not be read.
    TargetEndedUnread,
    /// The thread did not finish in time.
    TimedOut,
    /// The wait failed.
    WaitFailed,
}

/// Read the end of the injection wait.
///
/// The target's exit code comes first. Measured: when the loader ends a process, the remote thread is
/// signalled and the process's exit code already holds the loader's status, while the process object is
/// not signalled yet - so "is the process signalled" alone misses the very case this exists for, and the
/// thread's exit code (the same status) would read as a loaded module. A code other than `STILL_ACTIVE`
/// is an ending whatever the object says. A signalled object is an ending whatever the code is, because a
/// process may really end with 259 (R4-N19).
pub(crate) fn outcome(wait: Wait, process: ProcessAfter, thread_code: Option<u32>) -> Outcome {
    match (process.signalled, process.code) {
        (true, Some(code)) => return Outcome::TargetEnded(code),
        (true, None) => return Outcome::TargetEndedUnread,
        (false, Some(code)) if code != STILL_ACTIVE_CODE => return Outcome::TargetEnded(code),
        _ => {}
    }
    match wait {
        // The thread's exit code is the low 32 bits of the module `LoadLibraryW` returned, and a module
        // is aligned to 64 KiB. So a code with any of its low 16 bits set is not a module - a loader
        // status such as 0xC0000135 always has some - and 0 is the NULL of a library that did not load.
        Wait::ThreadDone => match thread_code {
            Some(code) if is_module(code) => Outcome::Loaded,
            _ => Outcome::LibraryNotLoaded,
        },
        Wait::TimedOut => Outcome::TimedOut,
        Wait::Failed => Outcome::WaitFailed,
    }
}

/// Whether a remote thread's exit code can be the low half of a loaded module: not zero, and aligned to
/// the 64 KiB a module is mapped at. On x64 a module based exactly on a 4 GiB boundary reads as zero
/// too - astronomically rare, and refused in the safe direction, since a retry lands elsewhere.
fn is_module(thread_code: u32) -> bool {
    thread_code != 0 && thread_code & 0xFFFF == 0
}

/// Whether the remote page holding the library's path can be freed: only once nothing will read it
/// again (R4-N10). The thread that read it has finished, or the process that owns it has ended. A thread
/// still running may be inside `LoadLibraryW` with the path in hand, and the caller ends that target
/// anyway, which takes the page with it.
pub(crate) fn page_released_safely(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Loaded | Outcome::LibraryNotLoaded | Outcome::TargetEnded(_) | Outcome::TargetEndedUnread
    )
}

/// `ProcessDefaultHardErrorMode`: the information class that reads and sets whether Windows shows its
/// error window for a process's hard errors - what `SetErrorMode` changes for the calling process. Bit 0
/// set means the window is shown, the inverse of `SEM_FAILCRITICALERRORS`. Not on the documented list for
/// `NtQueryInformationProcess` or `NtSetInformationProcess`: the phnt / ReactOS number, an assessment
/// rather than a source (zasady/03 section 4), measured on Windows 11 for x64 and x86 targets from both
/// cores (`tools/probes/r4-9`). A wrong number is a status the kernel returns, never a fault, and reads
/// here as "leave the window as it is".
const PROCESS_DEFAULT_HARD_ERROR_MODE: u32 = 12;

/// The bit of that value that shows the window.
const SHOWS_THE_WINDOW: u32 = 1;

type NtSetInformationProcessFn = unsafe extern "system" fn(HANDLE, u32, *const c_void, u32) -> i32;

/// Windows's error window, switched off for a target while it loads (R4-S6).
///
/// A target inherits the error mode of the process that started it (MS Learn, "Inheritance" and
/// `SetErrorMode`), and a program started from Explorer - the window, in the ordinary case - has the
/// default, which shows the window. The loader then raises it for a missing library and waits for an
/// answer, so the remote thread stood until the ten-second limit and the session reported a loader lock,
/// with the window still on the desktop after the start was over. Off, the loader ends the process at
/// once with the status the window would have shown.
///
/// The mode goes back before the target's entry point, so the application runs with the mode it
/// inherited - measured with a program that reports its own mode. What runs under the switch is the
/// loading itself: the start-up code of the libraries it imports and its TLS callbacks (R4-N20).
pub(crate) struct QuietLoader {
    process: HANDLE,
    inherited: u32,
    quiet: u32,
}

impl QuietLoader {
    /// Switch the window off for `process`, which must not have run an instruction yet. `None` when it
    /// was off already, so there is nothing to give back, or when the system would not read or set the
    /// mode - the start then goes on exactly as it did before this existed, window and all.
    ///
    /// # Safety
    /// `process` must be an open process handle with query and set-information access.
    pub(crate) unsafe fn begin(process: HANDLE) -> Option<Self> { unsafe {
        let inherited = query_mode(process)?;
        if inherited & SHOWS_THE_WINDOW == 0 {
            return None;
        }
        let quiet = inherited & !SHOWS_THE_WINDOW;
        set_mode(process, quiet).ok()?;
        Some(Self { process, inherited, quiet })
    }}

    /// Give the target back the mode it inherited. Left alone when the value is no longer the one set
    /// here: a library the target loaded chose its own mode in its start-up code, and that choice belongs
    /// to the target (measured with a library that sets 0x8001). One choice cannot be told from this
    /// switch and is undone with it: a library that set exactly the value set here, which for a target
    /// that inherited the default is `SEM_FAILCRITICALERRORS` and nothing else. An `Err` means the
    /// application would run without the window it inherited, which the caller refuses rather than leave
    /// the change in place unsaid.
    ///
    /// # Safety
    /// The handle given to `begin` must still be open.
    pub(crate) unsafe fn end(self) -> Result<(), String> { unsafe {
        match query_mode(self.process) {
            Some(now) if now != self.quiet => Ok(()),
            _ => set_mode(self.process, self.inherited).map_err(|status| {
                format!(
                    "could not give the target back the error mode it inherited (status 0x{status:08X}), so it was not started"
                )
            }),
        }
    }}
}

/// The process's hard-error mode, or `None` when the system will not say.
///
/// # Safety
/// `process` must be an open process handle with query access.
unsafe fn query_mode(process: HANDLE) -> Option<u32> { unsafe {
    let ntdll = GetModuleHandleA(s!("ntdll.dll")).ok()?;
    let entry = GetProcAddress(ntdll, s!("NtQueryInformationProcess"))?;
    let query: crate::NtQueryInformationProcessFn = std::mem::transmute(entry);
    let mut mode: u32 = 0;
    let status = query(
        process,
        PROCESS_DEFAULT_HARD_ERROR_MODE,
        (&raw mut mode).cast(),
        size_of::<u32>() as u32,
        std::ptr::null_mut(),
    );
    (status >= 0).then_some(mode)
}}

/// Set the process's hard-error mode. `Err` carries the status, or `u32::MAX` when the entry point is not
/// there to call.
///
/// # Safety
/// `process` must be an open process handle with set-information access.
unsafe fn set_mode(process: HANDLE, mode: u32) -> Result<(), u32> { unsafe {
    let Ok(ntdll) = GetModuleHandleA(s!("ntdll.dll")) else {
        return Err(u32::MAX);
    };
    let Some(entry) = GetProcAddress(ntdll, s!("NtSetInformationProcess")) else {
        return Err(u32::MAX);
    };
    let set: NtSetInformationProcessFn = std::mem::transmute(entry);
    let status = set(process, PROCESS_DEFAULT_HARD_ERROR_MODE, (&raw const mode).cast(), size_of::<u32>() as u32);
    if status >= 0 { Ok(()) } else { Err(status as u32) }
}}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: ProcessAfter = ProcessAfter { signalled: false, code: Some(STILL_ACTIVE_CODE) };

    /// The case this module exists for, as measured: the thread is done with the loader's status as its
    /// code, the process code holds the same status, and the process object is not signalled yet. Read
    /// the old way it was a loaded module (a non-zero thread code), and the target "vanished" afterwards.
    #[test]
    fn a_target_the_loader_ends_is_named_by_its_status_before_its_object_is_signalled() {
        let ending = ProcessAfter { signalled: false, code: Some(0xC000_0135) };
        assert_eq!(outcome(Wait::ThreadDone, ending, Some(0xC000_0135)), Outcome::TargetEnded(0xC000_0135));
        let ended = ProcessAfter { signalled: true, code: Some(0xC000_0142) };
        assert_eq!(outcome(Wait::ThreadDone, ended, Some(0xC000_0142)), Outcome::TargetEnded(0xC000_0142));
        // An ending outranks whatever the wait said about the thread.
        assert_eq!(outcome(Wait::TimedOut, ending, None), Outcome::TargetEnded(0xC000_0135));
        assert_eq!(outcome(Wait::Failed, ended, None), Outcome::TargetEnded(0xC000_0142));
    }

    /// A signalled process has ended even when its code is 259 (R4-N19), and a process whose code cannot
    /// be read is not taken for a loaded one.
    #[test]
    fn a_signalled_target_has_ended_whatever_its_code_says() {
        let ended_259 = ProcessAfter { signalled: true, code: Some(STILL_ACTIVE_CODE) };
        assert_eq!(outcome(Wait::ThreadDone, ended_259, Some(0x7FF0_0000)), Outcome::TargetEnded(STILL_ACTIVE_CODE));
        let unread = ProcessAfter { signalled: true, code: None };
        assert_eq!(outcome(Wait::ThreadDone, unread, Some(0x7FF0_0000)), Outcome::TargetEndedUnread);
    }

    /// A live target: the thread's code decides, and only a finished thread's code counts.
    #[test]
    fn a_live_target_is_read_from_its_finished_thread() {
        assert_eq!(outcome(Wait::ThreadDone, LIVE, Some(0x7FF0_0000)), Outcome::Loaded);
        assert_eq!(outcome(Wait::ThreadDone, LIVE, Some(0)), Outcome::LibraryNotLoaded);
        assert_eq!(outcome(Wait::ThreadDone, LIVE, None), Outcome::LibraryNotLoaded);
        let unreadable = ProcessAfter { signalled: false, code: None };
        assert_eq!(outcome(Wait::ThreadDone, unreadable, Some(0x7FF0_0000)), Outcome::Loaded);
        // A thread that ended with a status while its process lives on did not load anything.
        assert_eq!(outcome(Wait::ThreadDone, LIVE, Some(0xC000_0135)), Outcome::LibraryNotLoaded);
        assert_eq!(outcome(Wait::ThreadDone, LIVE, Some(STILL_ACTIVE_CODE)), Outcome::LibraryNotLoaded);
    }

    /// A wait that failed or timed out says nothing about the thread (R4-N10): a failed wait used to
    /// read the thread's code, 259 for a thread still running, and took it for a module.
    #[test]
    fn a_wait_that_failed_or_ran_out_is_never_a_loaded_module() {
        assert_eq!(outcome(Wait::TimedOut, LIVE, Some(STILL_ACTIVE_CODE)), Outcome::TimedOut);
        assert_eq!(outcome(Wait::Failed, LIVE, None), Outcome::WaitFailed);
    }

    /// The remote page goes only when nothing will read it again (R4-N10).
    #[test]
    fn the_remote_page_is_freed_only_once_nothing_reads_it() {
        assert!(page_released_safely(Outcome::Loaded));
        assert!(page_released_safely(Outcome::LibraryNotLoaded));
        assert!(page_released_safely(Outcome::TargetEnded(0xC000_0135)));
        assert!(page_released_safely(Outcome::TargetEndedUnread));
        assert!(!page_released_safely(Outcome::TimedOut));
        assert!(!page_released_safely(Outcome::WaitFailed));
    }
}
