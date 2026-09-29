//! The standard handles a target starts with (R4-W1, ADR-17): never this process's stdin or stdout,
//! which carry the protocol.
//!
//! A console program started with the defaults gets copies of the starting process's standard
//! handles, and it gets them whether or not those handles are inheritable - measured (x64 and x86,
//! `tools/probes/r4-5`): marking them non-inheritable changed nothing. For the core that meant the
//! target read the client's commands and wrote into the event stream. So a console program is given
//! its handles explicitly, with an attribute list that limits what it inherits to exactly those, or a
//! console of its own. Without the list the child also inherits every inheritable handle here, the
//! protocol pipe's ends among them, and the pipe then stays open until the target exits (measured).
//!
//! Marking this process's own handles non-inheritable does not replace any of it, and is not done:
//! a console target is given copies either way, and the one other child the core starts, the
//! Chromium mechanism's browser, runs in a job that ends with the core, so it cannot outlive it
//! holding the pipe (measured with `tools/probes/r4-5/cdp-eof.ps1`, the core ending and killed).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE};
use windows::Win32::System::Threading::{
    DeleteProcThreadAttributeList, GetCurrentProcess, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
    CREATE_NEW_CONSOLE, CREATE_NO_WINDOW, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROCESS_CREATION_FLAGS, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

/// How a target's standard handles are set up. None of the three hands it this process's stdin or
/// stdout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetStdio {
    /// A console program on this process's console, its handles given explicitly: stdin is the
    /// console's input (NUL when there is no console, or one without a window that nobody can type
    /// into), stdout and stderr are this process's stderr (NUL without one). With no console at all
    /// the program starts without a window, which Windows would otherwise open for it, empty
    /// (measured).
    Shared,
    /// A program with a window, started with the defaults, which give it no standard handles - the
    /// way a terminal starts it (measured). Explicit handles would give it some, and a program that
    /// logs to stderr when it has one would then behave differently under the tool (R4-D17).
    Windowed,
    /// A console window of its own (`CREATE_NEW_CONSOLE`). The flag gives a program with a window
    /// nothing (measured), so this one needs no subsystem.
    NewConsole,
}

impl TargetStdio {
    /// The handles for a target: its own console when the client asked for one, none for a program
    /// known to have a window, and this process's console and stderr otherwise. A program whose kind
    /// is not known goes with the last, because explicit handles are safe for any program - the worst
    /// they do is give a program with a window a stderr.
    pub fn choose(new_console: bool, windowed_program: Option<bool>) -> TargetStdio {
        if new_console {
            TargetStdio::NewConsole
        } else if windowed_program == Some(true) {
            TargetStdio::Windowed
        } else {
            TargetStdio::Shared
        }
    }
}

/// One of this process's standard handles, when it has one.
fn std_handle(which: STD_HANDLE) -> Option<HANDLE> {
    // SAFETY: reads this process's own parameter block.
    let h = unsafe { GetStdHandle(which) }.ok()?;
    (!h.is_invalid()).then_some(h)
}

/// The standard handles of one launch, kept alive until `CreateProcessW` has copied them into the
/// child: the startup information to pass, the creation flags and inherit switch that go with it, the
/// handles this process opened for the child, and the attribute list naming them.
pub(crate) struct LaunchStdio {
    pub(crate) si: STARTUPINFOEXW,
    pub(crate) flags: PROCESS_CREATION_FLAGS,
    pub(crate) inherit: bool,
    owned: Vec<HANDLE>,
    // The list the attribute points at, and the attribute list's own memory. Both are read by
    // `CreateProcessW`, not copied by `UpdateProcThreadAttribute`, so they live here until the drop.
    // `u64` for the list's memory, so it is aligned as the structure behind it expects.
    list: Vec<HANDLE>,
    attrs: Vec<u64>,
    attrs_ready: bool,
}

impl LaunchStdio {
    /// Startup information with nothing in it: the defaults, as every launch used to have.
    fn defaults(flags: PROCESS_CREATION_FLAGS) -> LaunchStdio {
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        LaunchStdio { si, flags, inherit: false, owned: Vec::new(), list: Vec::new(), attrs: Vec::new(), attrs_ready: false }
    }

    /// The startup information to hand `CreateProcessW`. With the extended flag set it reads the
    /// whole `STARTUPINFOEXW`, whose first field this is.
    pub(crate) fn startup_info(&self) -> *const STARTUPINFOW {
        &self.si.StartupInfo
    }
}

impl Drop for LaunchStdio {
    fn drop(&mut self) {
        // SAFETY: the list was initialised exactly when `attrs_ready` says so, and each handle in
        // `owned` was opened or duplicated for this launch and is closed once, here.
        unsafe {
            if self.attrs_ready {
                DeleteProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(self.attrs.as_mut_ptr().cast()));
            }
            for h in &self.owned {
                let _ = CloseHandle(*h);
            }
        }
    }
}

/// Everything a launch needs for `stdio`, or why it could not be set up.
pub(crate) fn launch_stdio(stdio: TargetStdio) -> Result<LaunchStdio, String> {
    match stdio {
        TargetStdio::Windowed => Ok(LaunchStdio::defaults(PROCESS_CREATION_FLAGS(0))),
        TargetStdio::NewConsole => Ok(LaunchStdio::defaults(CREATE_NEW_CONSOLE)),
        TargetStdio::Shared => shared_console(),
    }
}

/// `TargetStdio::Shared`: the console's input or NUL, this process's stderr or NUL, and an attribute
/// list holding exactly those two.
fn shared_console() -> Result<LaunchStdio, String> {
    let mut launch = LaunchStdio::defaults(PROCESS_CREATION_FLAGS(0));
    // The console's input, when someone can type into it. Opening it is the first question: without a
    // console it cannot be opened, and the program then starts without a window. The second is whether
    // the console has a window at all - a client started with no window (`CreateNoWindow`) gives this
    // process a console nobody sees, and a program reading from it would wait for ever, where the same
    // program started by that client without the tool reads the end of its input.
    let console_input = open_inheritable(w!("CONIN$"));
    if console_input.is_none() {
        launch.flags |= CREATE_NO_WINDOW;
    }
    let input = match console_input {
        Some(h) if console_has_window() => h,
        other => {
            if let Some(unseen) = other {
                // SAFETY: opened just above for this launch, closed once.
                unsafe {
                    let _ = CloseHandle(unseen);
                }
            }
            open_inheritable(w!("NUL")).ok_or_else(|| "cannot open NUL for the target's input".to_string())?
        }
    };
    launch.owned.push(input);
    let output = match std_handle(STD_ERROR_HANDLE).and_then(inheritable_copy) {
        Some(h) => h,
        None => open_inheritable(w!("NUL")).ok_or_else(|| "cannot open NUL for the target's output".to_string())?,
    };
    launch.owned.push(output);

    launch.si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    launch.si.StartupInfo.hStdInput = input;
    launch.si.StartupInfo.hStdOutput = output;
    launch.si.StartupInfo.hStdError = output;
    launch.list = vec![input, output];
    attach_handle_list(&mut launch)?;
    launch.inherit = true;
    launch.flags |= EXTENDED_STARTUPINFO_PRESENT;
    // With the extended flag the size has to be the extended structure's, or `CreateProcessW` fails
    // with "the parameter is incorrect" (0x80070057) - measured, the first build of this had it.
    launch.si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    Ok(launch)
}

/// Build the attribute list that limits inheritance to `launch.list` and point the startup
/// information at it.
fn attach_handle_list(launch: &mut LaunchStdio) -> Result<(), String> {
    let mut size: usize = 0;
    // SAFETY: the first call only asks for the size (it fails with "insufficient buffer" by design),
    // the second initialises memory of that size, and the attribute points at `launch.list`, which
    // stays where it is until the drop deletes the list.
    unsafe {
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        launch.attrs = vec![0u64; size.div_ceil(8)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(launch.attrs.as_mut_ptr().cast());
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)
            .map_err(|e| format!("InitializeProcThreadAttributeList failed: {e}"))?;
        launch.attrs_ready = true;
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(launch.list.as_ptr().cast()),
            launch.list.len() * std::mem::size_of::<HANDLE>(),
            None,
            None,
        )
        .map_err(|e| format!("UpdateProcThreadAttribute failed: {e}"))?;
        launch.si.lpAttributeList = list;
    }
    Ok(())
}

/// Whether this process's console has a window, visible or not - a terminal's does, a pseudo console's
/// (Windows Terminal) does, and one made for a process started with no window does not (measured:
/// `GetConsoleWindow` returns null there).
fn console_has_window() -> bool {
    // SAFETY: a query about this process's own console.
    !unsafe { GetConsoleWindow() }.is_invalid()
}

/// An inheritable handle to a device by name (`CONIN$`, `NUL`), or `None` when it cannot be opened.
fn open_inheritable(name: PCWSTR) -> Option<HANDLE> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: a device name and a security descriptor that live for the call.
    unsafe {
        CreateFileW(
            name,
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&sa),
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()
    .filter(|h| !h.is_invalid())
}

/// An inheritable copy of one of this process's handles, which stays this process's to close.
fn inheritable_copy(h: HANDLE) -> Option<HANDLE> {
    let mut copy = HANDLE::default();
    // SAFETY: duplicating a handle this process owns into itself.
    unsafe {
        let me = GetCurrentProcess();
        DuplicateHandle(me, h, me, &mut copy, 0, true, DUPLICATE_SAME_ACCESS).ok()?;
    }
    (!copy.is_invalid()).then_some(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Console::{STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    /// Its own console when asked for, none for a program known to have a window, and the shared
    /// console otherwise - a program of unknown kind included (R4-D17).
    #[test]
    fn the_handles_follow_the_request_and_then_the_program_kind() {
        for kind in [None, Some(false), Some(true)] {
            assert_eq!(TargetStdio::choose(true, kind), TargetStdio::NewConsole, "{kind:?}");
        }
        assert_eq!(TargetStdio::choose(false, Some(true)), TargetStdio::Windowed);
        assert_eq!(TargetStdio::choose(false, Some(false)), TargetStdio::Shared);
        assert_eq!(TargetStdio::choose(false, None), TargetStdio::Shared, "unknown is treated as a console program");
    }

    /// A shared launch names exactly two handles, input and output, and never this process's own
    /// stdin or stdout - which is the whole point.
    #[test]
    fn a_shared_launch_hands_over_its_own_handles_and_nothing_of_the_protocol() {
        let launch = launch_stdio(TargetStdio::Shared).expect("a shared launch");
        assert_eq!(launch.list.len(), 2);
        assert!(launch.inherit);
        assert!(launch.flags.contains(EXTENDED_STARTUPINFO_PRESENT));
        let si = &launch.si.StartupInfo;
        assert_eq!(si.cb as usize, std::mem::size_of::<STARTUPINFOEXW>(), "the extended flag needs the extended size");
        assert!(si.dwFlags.contains(STARTF_USESTDHANDLES));
        assert_eq!(si.hStdOutput, si.hStdError);
        for protocol in [std_handle(STD_INPUT_HANDLE), std_handle(STD_OUTPUT_HANDLE)].into_iter().flatten() {
            assert_ne!(si.hStdInput, protocol);
            assert_ne!(si.hStdOutput, protocol);
        }
        assert_eq!(launch.list, vec![si.hStdInput, si.hStdOutput]);
    }

    /// The other two leave the startup information at its defaults and inherit nothing.
    #[test]
    fn a_windowed_or_own_console_launch_inherits_nothing() {
        for (stdio, flags) in [(TargetStdio::Windowed, PROCESS_CREATION_FLAGS(0)), (TargetStdio::NewConsole, CREATE_NEW_CONSOLE)] {
            let launch = launch_stdio(stdio).expect("a launch");
            assert_eq!(launch.flags, flags, "{stdio:?}");
            assert!(!launch.inherit, "{stdio:?}");
            assert!(launch.list.is_empty() && launch.owned.is_empty(), "{stdio:?}");
            assert!(!launch.si.StartupInfo.dwFlags.contains(STARTF_USESTDHANDLES), "{stdio:?}");
            assert_eq!(launch.si.StartupInfo.cb as usize, std::mem::size_of::<STARTUPINFOW>(), "{stdio:?}");
        }
    }
}
