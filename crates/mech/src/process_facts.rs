//! Two questions the session asks about a live process from outside it, entering nothing: whether a
//! library is loaded in it, and whether its token is elevated.
//!
//! Both feed one claim of the embedded-engine channel (docs/09 section 12.12): an application that
//! loaded WebView2 and whose web engine the session never reached ran its pages on the real clock,
//! and when its token is elevated the engine ignores the variable the session reaches it by
//! (Microsoft Learn, "Develop secure WebView2 apps"). The hook cannot answer either question: for an
//! elevated host the engine's browser process is started by a system service, so no spawn the hook
//! watches ever names it. The answers are read from outside instead - a snapshot of the process's
//! modules, and its token - at the cadence the session already polls its family.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::tree::text_up_to_nul;

/// What a look at the libraries loaded in a process established. `NotLoaded` is said only after the
/// whole list was read: a snapshot that failed, or that ended early, is `Unknown`. A list cut short
/// would read as "it is not there" and keep a claim from being made that the evidence supports, and
/// the audit does not stay quiet about what it could not look at (rule 4 cuts both ways).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleProbe {
    Loaded,
    NotLoaded,
    Unknown,
}

/// How many times the snapshot is taken when the module list changed under it. Microsoft Learn
/// (`CreateToolhelp32Snapshot`): on `ERROR_BAD_LENGTH` call the function again until it succeeds. The
/// retry is bounded here, so a process that keeps loading libraries cannot hold the session loop.
const SNAPSHOT_ATTEMPTS: u32 = 5;

/// Whether `module` (a file name such as `EmbeddedBrowserWebView.dll`, compared without regard to
/// case, the way the system compares names) is loaded in the process `pid`.
///
/// The core is the same bitness as the application it hooks - a mismatch is refused before the
/// launch - so the snapshot sees every module of it. Libraries mapped as data files are not listed
/// (Microsoft Learn), which is no loss: a library that is only mapped as data never ran.
pub fn process_has_module(pid: u32, module: &str) -> ModuleProbe {
    // SAFETY: the snapshot handle is closed on every path out of `probe_module`, and the entry
    // structure carries its own size as the API requires.
    unsafe { probe_module(pid, module) }
}

/// # Safety
/// Takes and closes its own snapshot handle, and nothing is borrowed from the caller.
unsafe fn probe_module(pid: u32, module: &str) -> ModuleProbe { unsafe {
    let mut snapshot = None;
    for _ in 0..SNAPSHOT_ATTEMPTS {
        match CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) {
            Ok(handle) => {
                snapshot = Some(handle);
                break;
            }
            Err(e) if e.code() == ERROR_BAD_LENGTH.to_hresult() => continue,
            Err(_) => return ModuleProbe::Unknown,
        }
    }
    let Some(snapshot) = snapshot else {
        return ModuleProbe::Unknown;
    };
    let mut entry = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
    let mut first = true;
    let probe = classify(
        || {
            let stepped =
                if first { Module32FirstW(snapshot, &mut entry) } else { Module32NextW(snapshot, &mut entry) };
            first = false;
            match stepped {
                Ok(()) => Step::Module(text_up_to_nul(&entry.szModule)),
                Err(e) if e.code() == ERROR_NO_MORE_FILES.to_hresult() => Step::End,
                Err(_) => Step::Failed,
            }
        },
        module,
    );
    let _ = CloseHandle(snapshot);
    probe
}}

/// One step of a walk over a module list, as the walk itself sees it.
enum Step {
    Module(String),
    /// The list ended the way the API says a list ends (`ERROR_NO_MORE_FILES`).
    End,
    /// The walk broke off for any other reason.
    Failed,
}

/// Judge a walk over a module list: the library is loaded the moment it is met, absent only when the
/// list ended cleanly AFTER it had entries, and unknown otherwise. A list that ended at its first
/// step is not the list of a live process (its own executable is a module), and a walk that failed
/// part-way has read only some of it. Names compare without regard to case, over ASCII, which is what
/// the file names of the libraries this looks for are made of. Pure over the steps, so every branch is
/// tested without a process.
fn classify(mut next: impl FnMut() -> Step, wanted: &str) -> ModuleProbe {
    let mut read_any = false;
    loop {
        match next() {
            Step::Module(name) => {
                read_any = true;
                if name.eq_ignore_ascii_case(wanted) {
                    return ModuleProbe::Loaded;
                }
            }
            Step::End if read_any => return ModuleProbe::NotLoaded,
            Step::End | Step::Failed => return ModuleProbe::Unknown,
        }
    }
}

/// Whether the token of the process `pid` is elevated, or `None` when it cannot be read (the process
/// is gone, or does not grant the question). Only `Some(true)` is ever said to a tester as a fact: a
/// token that could not be read is not a token that is not elevated.
pub fn process_elevated(pid: u32) -> Option<bool> {
    // SAFETY: both handles are closed on every path out. The output structure is a plain struct the
    // call fills, and the size passed is its own.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let answer = token_elevated(process);
        let _ = CloseHandle(process);
        answer
    }
}

/// # Safety
/// `process` must be an open process handle with at least limited query access. It is not closed here.
unsafe fn token_elevated(process: HANDLE) -> Option<bool> { unsafe {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
    let mut info = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    let read = GetTokenInformation(
        token,
        TokenElevation,
        Some(&mut info as *mut TOKEN_ELEVATION as *mut c_void),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut returned,
    );
    let _ = CloseHandle(token);
    read.ok()?;
    Some(info.TokenIsElevated != 0)
}}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process running this test stands in for any application: it has `kernel32.dll` loaded and
    /// no library of the name below, and the answer for the first is found through the same walk that
    /// would have answered for the second. A snapshot that stopped at the first entry would pass the
    /// negative alone, which is why both are asked.
    #[test]
    fn a_process_has_the_libraries_it_loaded_and_not_one_it_did_not() {
        let me = std::process::id();
        assert_eq!(process_has_module(me, "kernel32.dll"), ModuleProbe::Loaded);
        assert_eq!(process_has_module(me, "no-such-library-chrono-mock-3f9a.dll"), ModuleProbe::NotLoaded);
    }

    #[test]
    fn a_library_name_is_compared_without_regard_to_case() {
        assert_eq!(process_has_module(std::process::id(), "KERNEL32.DLL"), ModuleProbe::Loaded);
        let walk = |names: &'static [&'static str], wanted| {
            let mut left = names.iter();
            classify(move || left.next().map_or(Step::End, |n| Step::Module((*n).to_string())), wanted)
        };
        assert_eq!(walk(&["app.exe", "EmbeddedBrowserWebView.dll"], "embeddedbrowserwebview.DLL"), ModuleProbe::Loaded);
        assert_eq!(walk(&["app.exe", "EmbeddedBrowserWebView.dll"], "EmbeddedBrowserWebView"), ModuleProbe::NotLoaded, "a prefix is not the name");
    }

    /// The four ways a walk can go, each judged on its own: found (and the walk stops there), the list
    /// ended cleanly after entries, the list was empty from the first step, and the walk broke off
    /// part-way. Only the second says the library is absent.
    #[test]
    fn a_walk_is_judged_by_how_it_ended() {
        fn scripted(steps: Vec<Step>, wanted: &str) -> (ModuleProbe, usize) {
            let mut left = steps.into_iter();
            let mut taken = 0;
            let probe = classify(
                || {
                    taken += 1;
                    left.next().unwrap_or(Step::End)
                },
                wanted,
            );
            (probe, taken)
        }
        let m = |n: &str| Step::Module(n.to_string());

        let (found, taken) = scripted(vec![m("a.exe"), m("target.dll"), m("later.dll"), Step::End], "TARGET.DLL");
        assert_eq!((found, taken), (ModuleProbe::Loaded, 2), "the walk stops at the library");
        assert_eq!(scripted(vec![m("a.exe"), m("b.dll"), Step::End], "target.dll").0, ModuleProbe::NotLoaded);
        assert_eq!(scripted(vec![Step::End], "target.dll").0, ModuleProbe::Unknown, "an empty list says nothing");
        assert_eq!(scripted(vec![m("a.exe"), Step::Failed], "target.dll").0, ModuleProbe::Unknown, "a cut-short list says nothing");
        assert_eq!(scripted(vec![Step::Failed], "target.dll").0, ModuleProbe::Unknown);
    }

    /// A pid nothing answers to cannot be looked into, and that is `Unknown`: saying `NotLoaded` for a
    /// snapshot that failed would have a claim kept back on no evidence.
    #[test]
    fn a_process_that_cannot_be_looked_into_is_unknown_not_absent() {
        assert_eq!(process_has_module(u32::MAX - 5, "kernel32.dll"), ModuleProbe::Unknown);
    }

    #[test]
    fn a_token_is_read_for_a_live_process_and_not_for_a_missing_one() {
        assert!(process_elevated(std::process::id()).is_some(), "this process can read its own token");
        assert_eq!(process_elevated(u32::MAX - 5), None);
    }
}
