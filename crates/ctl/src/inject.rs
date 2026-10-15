//! How a remote load of the hook ended, read one way by both sides that inject it (R5-A7).
//!
//! The mechanism loads the hook into the target it launches, and the hook loads itself into each child
//! that target starts (ADR-3). Both write the library's path into the process, run `LoadLibraryW` there
//! on a remote thread and wait for it, and both have to answer the same three questions afterwards: did
//! the library load, did the process end while it was being loaded, and may the page holding the path
//! be freed. They used to answer them in two copies that had drifted apart. The mechanism read the
//! target's own exit code first (R4-S6) and would not take a loader status for a module, while the hook
//! took any non-zero thread code for a loaded library - so a child its loader ended, a missing import
//! say, counted as a process that ran on the real clock although it never ran an instruction of its
//! own. The reading lives here once, with no Windows types, so both sides and their tests share it.
//!
//! The target is created suspended, so the remote thread is the first thread that runs in it, and the
//! first thread of a process does that process's own loading before its start routine: the static
//! imports and their start-up code, then `LoadLibraryW` of the hook. A process whose static import is
//! missing, built for the other bitness, lacking a function or refusing to initialise ends right
//! there, with the loader's status as its exit code (measured on x64 and x86, `tools/probes/r4-9`).

/// `STILL_ACTIVE` (259): the exit code Windows reports for a thread or a process that has not finished.
/// Read as "not known yet", never as a loaded module.
pub const STILL_ACTIVE_CODE: u32 = 259;

/// How long either side waits for the remote `LoadLibraryW` thread. The hook's `DllMain` does no heavy
/// work (it defers the watcher off the loader lock, ADR-3), so a load is quick - measured at about 68 ms
/// for a child - and a thread still running past this means the process's loader is stuck (a library's
/// start-up code waiting, or a loader lock). Finite, so neither the core's start nor the parent's
/// `CreateProcess` can hang for good (M-1, RELEASE-009).
pub const INJECT_TIMEOUT_MS: u32 = 10_000;

/// How the wait for the remote `LoadLibraryW` thread ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The thread finished - on its own, or because the process ended and took it along.
    ThreadDone,
    /// It did not, within the time allowed.
    TimedOut,
    /// The wait itself failed, so nothing is known about either.
    Failed,
}

/// The process as it stood right after that wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessAfter {
    /// Whether the process object was signalled, which is what says it has ended.
    pub signalled: bool,
    /// Its exit code as `GetExitCodeProcess` read it, `None` when the read failed.
    pub code: Option<u32>,
}

/// What the injection came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// `LoadLibraryW` returned a module.
    Loaded,
    /// `LoadLibraryW` returned NULL in a process that is still there.
    LibraryNotLoaded,
    /// The process ended while it was being loaded, with this exit code.
    TargetEnded(u32),
    /// The process ended and its exit code could not be read.
    TargetEndedUnread,
    /// The thread did not finish in time.
    TimedOut,
    /// The wait failed.
    WaitFailed,
}

/// Read the end of the injection wait.
///
/// The process's exit code comes first. Measured: when the loader ends a process, the remote thread is
/// signalled and the process's exit code already holds the loader's status, while the process object is
/// not signalled yet - so "is the process signalled" alone misses the very case this exists for, and the
/// thread's exit code (the same status) would read as a loaded module. A code other than `STILL_ACTIVE`
/// is an ending whatever the object says. A signalled object is an ending whatever the code is, because a
/// process may really end with 259 (R4-N19).
pub fn outcome(wait: Wait, process: ProcessAfter, thread_code: Option<u32>) -> Outcome {
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
/// still running may be inside `LoadLibraryW` with the path in hand, and freeing the page under it is a
/// use-after-free inside somebody else's application.
pub fn page_released_safely(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Loaded | Outcome::LibraryNotLoaded | Outcome::TargetEnded(_) | Outcome::TargetEndedUnread
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: ProcessAfter = ProcessAfter { signalled: false, code: Some(STILL_ACTIVE_CODE) };

    /// The case the process-first reading exists for, as measured: the thread is done with the loader's
    /// status as its code, the process code holds the same status, and the process object is not
    /// signalled yet. Read the old way it was a loaded module (a non-zero thread code).
    #[test]
    fn a_process_the_loader_ends_is_named_by_its_status_before_its_object_is_signalled() {
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
    fn a_signalled_process_has_ended_whatever_its_code_says() {
        let ended_259 = ProcessAfter { signalled: true, code: Some(STILL_ACTIVE_CODE) };
        assert_eq!(outcome(Wait::ThreadDone, ended_259, Some(0x7FF0_0000)), Outcome::TargetEnded(STILL_ACTIVE_CODE));
        let unread = ProcessAfter { signalled: true, code: None };
        assert_eq!(outcome(Wait::ThreadDone, unread, Some(0x7FF0_0000)), Outcome::TargetEndedUnread);
    }

    /// A live process: the thread's code decides, and only a finished thread's code counts.
    #[test]
    fn a_live_process_is_read_from_its_finished_thread() {
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
