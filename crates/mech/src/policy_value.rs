//! The one WebView2 option a session may put in the machine registry (docs/09 section 12.19).
//!
//! An elevated WebView2 host ignores the environment variable a session reaches an engine by, and
//! honours the machine registry instead (Microsoft Learn, "Develop secure WebView2 apps"). So, for an
//! application that runs as administrator and only when the tester asked for it, the session puts one
//! value under `HKLM\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments`, named after
//! the file the application was started from, that opens a debugging port on loopback - and takes it
//! away again. This is the only module that writes the registry, which is why everything it leaves
//! behind is pinned down here:
//!
//! - **It writes nothing that is not its own.** A value already under the application's name is left as
//!   it is, whatever its type, and the `*` value (every WebView2 host) is copied into ours rather than
//!   shadowed, because a value under the file name REPLACES `*` for that host instead of joining it.
//! - **Every value it writes carries a marker**, `--chrono-mock-session=<pid>.<creation time>` of the
//!   core that wrote it. A core that dies takes its `Drop` with it, so the next session finds the
//!   value by the marker, sees that its owner is gone, and removes it - with no log file to lose.
//! - **Only keys it created are removed, and only while empty.**
//!
//! The registry is reached through a [`Location`], so the tests run the same Win32 calls against a
//! scratch key under `HKCU` and never touch the machine's policies.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
    NO_ERROR, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegEnumValueW, RegGetValueW, RegOpenKeyExW,
    RegQueryInfoKeyW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_CREATE_SUB_KEY, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_CREATED_NEW_KEY, REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
    RRF_NOEXPAND, RRF_RT_ANY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

/// The switch that opens the debugging port. `=0` lets the engine pick a free one, and discovery finds
/// it (docs/09 section 12.9).
const PORT_SWITCH: &str = "--remote-debugging-port";
/// The marker switch: a switch the engine does not know (measured harmless, `wv2-policy`), whose value
/// names the core that wrote the value. Also what the scan looks for.
const MARKER_SWITCH: &str = "--chrono-mock-session=";

/// Where the values live: a root and the path below it, key by key. Production is `HKLM` and the
/// policy path, and a test uses a scratch key under `HKCU`.
#[derive(Debug, Clone)]
struct Location {
    root: HKEY,
    /// The path one key at a time. Index 0 exists on every machine and is never created or removed.
    path: Vec<String>,
}

impl Location {
    fn machine() -> Location {
        Location {
            root: HKEY_LOCAL_MACHINE,
            path: ["SOFTWARE", "Policies", "Microsoft", "Edge", "WebView2", "AdditionalBrowserArguments"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }

    /// The path of the first `count` keys, as one backslash-separated string.
    fn prefix(&self, count: usize) -> String {
        self.path[..count].join("\\")
    }
}

/// What a value under the key turned out to be. Any type other than a plain string is `Other`: the
/// session cannot copy it, and must not write over it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Absent,
    Text(String),
    Other,
}

/// What `set_for_session` did.
#[derive(Debug)]
pub enum PolicySet {
    /// The value is written. Dropping the guard (or `remove`) takes it away again.
    Written(PolicyValue),
    /// The tester's own `*` value already opens a debugging port, so nothing was written: discovery
    /// finds the port that opens. Said to no one - the port being reached is the whole report.
    StarHasPort,
    /// A value that is not this session's already stands under the application's name. Left as it is.
    Foreign,
    /// Nothing written, and the detail is for stderr: access denied, a `*` value that is not a string,
    /// a registry that would not answer.
    Failed(String),
}

/// What taking the value away came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRemoval {
    /// Gone, or no longer ours (a policy refresh or the tester replaced it during the session).
    Removed,
    /// The registry refused. The detail is for stderr, and the marker lets the next session finish it.
    Left(String),
}

/// A value this session wrote and is answerable for. The guard removes it on `Drop` as a last resort,
/// but a session that has something to say about how it went calls `remove` first.
#[derive(Debug)]
pub struct PolicyValue {
    location: Location,
    name: String,
    /// The core that wrote it, as its marker names it: a pid and the time that process was created.
    owner: (u32, u64),
    /// Indexes into the location's path of the keys this session created, shallowest first.
    created: Vec<usize>,
    removal: Option<PolicyRemoval>,
}

impl PolicyValue {
    /// The value's name: the file name the application was started from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Take the value away, and the empty keys that were created for it. Safe to call twice: the
    /// second call says what the first one did.
    pub fn remove(&mut self) -> PolicyRemoval {
        if let Some(done) = &self.removal {
            return done.clone();
        }
        let outcome = remove_value(&self.location, &self.name, self.owner);
        if outcome == PolicyRemoval::Removed {
            // Keys go only after the value is certainly gone: a key that still holds our value is not
            // empty, and one that is not empty is never deleted.
            for &level in self.created.iter().rev() {
                let _ = delete_key_if_empty(&self.location, level + 1);
            }
        }
        self.removal = Some(outcome.clone());
        outcome
    }
}

impl Drop for PolicyValue {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

/// What a scan for values left behind by sessions that are gone found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recovery {
    pub removed: u32,
    pub failed: u32,
}

/// Put the value for the application started from `exe_name` (a file name, exactly as the start path
/// names it - no canonicalisation, the runtime matches the name it is started under) into the machine
/// registry. Needs administrator rights, which the caller has checked: without them this is `Failed`.
pub fn set_for_session(exe_name: &str) -> PolicySet {
    // SAFETY: the pseudo-handle of the current process needs no closing and always allows the question.
    let created = unsafe { crate::process_created(GetCurrentProcess()) };
    let Some(created) = created else {
        return PolicySet::Failed("this process's identity could not be read, so the value could not be marked".into());
    };
    set_in(&Location::machine(), exe_name, (std::process::id(), created))
}

/// Remove the values that sessions which no longer exist left behind. Only a value carrying the
/// marker of a core that is gone is touched.
pub fn recover_stale() -> Recovery {
    recover_in(&Location::machine(), owner_alive)
}

/// Whether the machine registry holds a value left behind by a session that is gone - the read-only
/// question a session that cannot (or was not asked to) remove it still owes the tester.
pub fn stale_value_present() -> bool {
    stale_in(&Location::machine(), owner_alive)
}

fn set_in(location: &Location, exe_name: &str, owner: (u32, u64)) -> PolicySet {
    if let Some(why) = unusable_name(exe_name) {
        return PolicySet::Failed(why.into());
    }
    let (star, own) = match read_both(location, exe_name) {
        Ok(pair) => pair,
        Err(e) => return PolicySet::Failed(format!("the existing values could not be read ({})", describe(e))),
    };
    let data = match decide(&star, &own, &marker_text(owner.0, owner.1)) {
        Decision::Write(data) => data,
        Decision::StarHasPort => return PolicySet::StarHasPort,
        Decision::Foreign => return PolicySet::Foreign,
        Decision::Refuse(why) => return PolicySet::Failed(why.into()),
    };
    match write_value(location, exe_name, &data) {
        Ok(created) => PolicySet::Written(PolicyValue {
            location: location.clone(),
            name: exe_name.to_string(),
            owner,
            created,
            removal: None,
        }),
        Err(e) => PolicySet::Failed(format!("the value could not be written ({})", describe(e))),
    }
}

/// Why a file name cannot be a value name, or `None` when it can. An empty name is the key's DEFAULT
/// value, `*` is the value every host reads, a backslash is not part of a file name, and a zero ends
/// the name early - each would write somewhere other than under the application's name.
fn unusable_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.contains(['\\', '*', '\0']) {
        Some("the application's file name cannot be used as the name of a registry value")
    } else {
        None
    }
}

/// What to do about the values that are there. Pure, so each combination is tested without a registry.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Write(String),
    StarHasPort,
    Foreign,
    Refuse(&'static str),
}

fn decide(star: &Found, own: &Found, marker: &str) -> Decision {
    if *own != Found::Absent {
        return Decision::Foreign;
    }
    match star {
        Found::Absent => Decision::Write(format!("{PORT_SWITCH}=0 {marker}")),
        Found::Text(text) if text.trim().is_empty() => Decision::Write(format!("{PORT_SWITCH}=0 {marker}")),
        Found::Text(text) if has_port_switch(text) => Decision::StarHasPort,
        Found::Text(text) => Decision::Write(format!("{} {PORT_SWITCH}=0 {marker}", text.trim())),
        Found::Other => Decision::Refuse("the * value is not a plain string, so it could not be carried into ours"),
    }
}

/// Whether the switches already ask for a debugging port. A switch is a whole whitespace-separated
/// token that starts with the name - `--remote-debugging-port=9222` and `--remote-debugging-port=0`
/// both, `--remote-debugging-portal` is not a thing and is not matched by accident because the name is
/// compared up to an `=` or the end.
fn has_port_switch(switches: &str) -> bool {
    switches.split_whitespace().any(|token| token.split('=').next() == Some(PORT_SWITCH))
}

/// The marker text for a core: its pid and the time it was created, which together name one process
/// where a pid alone names whichever one the system handed it to last.
fn marker_text(pid: u32, created: u64) -> String {
    format!("{MARKER_SWITCH}{pid}.{created}")
}

/// The pid and creation time a value's switches name, if they carry a marker at all.
fn parse_marker(switches: &str) -> Option<(u32, u64)> {
    let token = switches.split_whitespace().find(|t| t.starts_with(MARKER_SWITCH))?;
    let (pid, created) = token[MARKER_SWITCH.len()..].split_once('.')?;
    Some((pid.parse().ok()?, created.parse().ok()?))
}

/// Whether the process a marker names is still running. A pid the system recycled is told apart by the
/// creation time. A process that exists and will not answer is alive: the question here decides whether
/// to delete something, and a guess must never destroy what a live session owns.
fn owner_alive(pid: u32, created: u64) -> bool {
    // SAFETY: the handle is closed on every path out, and nothing is borrowed from the caller.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => {
                let same = crate::process_created(handle) == Some(created);
                let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
                let _ = CloseHandle(handle);
                same && running
            }
            Err(e) => open_error_means_alive(e.code()),
        }
    }
}

/// What a process that cannot be opened is taken for. "Access denied" means it exists and will not
/// answer, so it is alive: the question decides whether to delete something, and a guess must never
/// destroy what a live session owns. Any other failure is a pid nothing answers to.
fn open_error_means_alive(code: windows::core::HRESULT) -> bool {
    code == ERROR_ACCESS_DENIED.to_hresult()
}

fn recover_in(location: &Location, alive: impl Fn(u32, u64) -> bool) -> Recovery {
    let mut recovery = Recovery::default();
    for name in stale_names(location, alive) {
        match open_deepest(location, KEY_SET_VALUE) {
            Ok(Some(key)) => {
                // SAFETY: `key` is open and the name is NUL-terminated, and the key is closed right after.
                let status = unsafe { RegDeleteValueW(key.0, PCWSTR(wide(&name).as_ptr())) };
                if status == NO_ERROR || status == ERROR_FILE_NOT_FOUND {
                    recovery.removed += 1;
                } else {
                    recovery.failed += 1;
                }
            }
            _ => recovery.failed += 1,
        }
    }
    recovery
}

fn stale_in(location: &Location, alive: impl Fn(u32, u64) -> bool) -> bool {
    !stale_names(location, alive).is_empty()
}

/// The names of the values whose marker names a core that is gone. A value that is not a string, has
/// no marker, or cannot be read in full is not ours and is not listed.
fn stale_names(location: &Location, alive: impl Fn(u32, u64) -> bool) -> Vec<String> {
    let Ok(Some(key)) = open_deepest(location, KEY_QUERY_VALUE) else {
        return Vec::new();
    };
    let Ok(values) = enumerate(&key) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter(|(_, switches)| parse_marker(switches).is_some_and(|(pid, created)| !alive(pid, created)))
        .map(|(name, _)| name)
        .collect()
}

fn read_both(location: &Location, exe_name: &str) -> Result<(Found, Found), WIN32_ERROR> {
    let Some(key) = open_deepest(location, KEY_QUERY_VALUE)? else {
        return Ok((Found::Absent, Found::Absent));
    };
    Ok((read_value(&key, "*")?, read_value(&key, exe_name)?))
}

fn remove_value(location: &Location, name: &str, owner: (u32, u64)) -> PolicyRemoval {
    let key = match open_deepest(location, KEY_QUERY_VALUE | KEY_SET_VALUE) {
        Ok(Some(key)) => key,
        // The key itself is gone: so is the value.
        Ok(None) => return PolicyRemoval::Removed,
        Err(e) => return PolicyRemoval::Left(format!("the key could not be opened ({})", describe(e))),
    };
    match read_value(&key, name) {
        Ok(Found::Text(switches)) if parse_marker(&switches) == Some(owner) => {
            // SAFETY: `key` is open and the name is NUL-terminated.
            let status = unsafe { RegDeleteValueW(key.0, PCWSTR(wide(name).as_ptr())) };
            if status == NO_ERROR || status == ERROR_FILE_NOT_FOUND {
                PolicyRemoval::Removed
            } else {
                PolicyRemoval::Left(format!("the value could not be deleted ({})", describe(status)))
            }
        }
        // Not ours any more, or not there: nothing of this session stands in the registry.
        Ok(_) => PolicyRemoval::Removed,
        Err(e) => PolicyRemoval::Left(format!("the value could not be read back ({})", describe(e))),
    }
}

/// An open registry key, closed when it goes out of scope.
struct Open(HKEY);

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open or create and is closed exactly once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// The deepest key of the location with the given access, or `None` when it does not exist (a missing
/// key is an answer, not an error - any level of the path may be absent).
fn open_deepest(location: &Location, access: REG_SAM_FLAGS) -> Result<Option<Open>, WIN32_ERROR> {
    let path = wide(&location.prefix(location.path.len()));
    let mut key = HKEY::default();
    // SAFETY: the path is NUL-terminated and outlives the call, and the out-parameter is a local.
    let status = unsafe { RegOpenKeyExW(location.root, PCWSTR(path.as_ptr()), None, access, &mut key) };
    match status {
        NO_ERROR => Ok(Some(Open(key))),
        s if s == ERROR_FILE_NOT_FOUND || s == ERROR_PATH_NOT_FOUND => Ok(None),
        s => Err(s),
    }
}

/// Create the path down to the deepest key and write the value, returning which levels were created
/// (the disposition of each create says whether the key was there before - no separate look, no race
/// between looking and creating). A failure part-way removes what was created.
fn write_value(location: &Location, name: &str, data: &str) -> Result<Vec<usize>, WIN32_ERROR> {
    let mut created = Vec::new();
    let mut deepest: Option<Open> = None;
    for level in 1..=location.path.len() {
        let path = wide(&location.prefix(level));
        let mut key = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION(0);
        // SAFETY: the path is NUL-terminated and outlives the call, the out-parameters are locals and
        // the class is left null.
        let status = unsafe {
            RegCreateKeyExW(
                location.root,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_CREATE_SUB_KEY | KEY_QUERY_VALUE | KEY_SET_VALUE,
                None,
                &mut key,
                Some(&mut disposition),
            )
        };
        if status != NO_ERROR {
            undo_created(location, &created);
            return Err(status);
        }
        if disposition == REG_CREATED_NEW_KEY {
            created.push(level - 1);
        }
        // Only the deepest handle is used, and the ones above it closed as the walk went on.
        deepest = Some(Open(key));
    }
    let Some(deepest) = deepest else {
        return Ok(created);
    };
    let bytes: Vec<u8> = wide(data).iter().flat_map(|u| u.to_le_bytes()).collect();
    // SAFETY: the key is open with `KEY_SET_VALUE`, the name is NUL-terminated and the data is the
    // NUL-terminated UTF-16 text a `REG_SZ` holds, its byte length included.
    let status = unsafe { RegSetValueExW(deepest.0, PCWSTR(wide(name).as_ptr()), None, REG_SZ, Some(&bytes)) };
    if status != NO_ERROR {
        drop(deepest);
        undo_created(location, &created);
        return Err(status);
    }
    Ok(created)
}

fn undo_created(location: &Location, created: &[usize]) {
    for &level in created.iter().rev() {
        let _ = delete_key_if_empty(location, level + 1);
    }
}

/// Delete the key of the first `count` path entries when nothing is under it. There is no registry
/// call that deletes only an empty key, so this looks and then deletes. A value written into the key
/// in the microseconds between the two goes with it, and that window is named in the README.
fn delete_key_if_empty(location: &Location, count: usize) -> Result<bool, WIN32_ERROR> {
    let path = wide(&location.prefix(count));
    let mut key = HKEY::default();
    // SAFETY: the path is NUL-terminated and outlives the call, and the out-parameter is a local.
    let status = unsafe { RegOpenKeyExW(location.root, PCWSTR(path.as_ptr()), None, KEY_QUERY_VALUE, &mut key) };
    if status != NO_ERROR {
        return Err(status);
    }
    let key = Open(key);
    let (mut subkeys, mut values) = (0u32, 0u32);
    // SAFETY: the key is open, and every out-parameter is either a local or left null.
    let status = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    if status != NO_ERROR {
        return Err(status);
    }
    drop(key);
    if subkeys != 0 || values != 0 {
        return Ok(false);
    }
    // SAFETY: the path is NUL-terminated and outlives the call.
    let status = unsafe { RegDeleteKeyW(location.root, PCWSTR(path.as_ptr())) };
    if status == NO_ERROR {
        Ok(true)
    } else {
        Err(status)
    }
}

/// One value, of any type, under an open key. Two calls - the size first, then the data - because the
/// size is what the first one is for, and a value that changed between them asks for one more round.
fn read_value(key: &Open, name: &str) -> Result<Found, WIN32_ERROR> {
    let name = wide(name);
    for _ in 0..3 {
        let (mut kind, mut size) = (REG_VALUE_TYPE(0), 0u32);
        // SAFETY: the name is NUL-terminated and no buffer is given, so only the type and the size come back.
        let status = unsafe {
            RegGetValueW(key.0, PCWSTR::null(), PCWSTR(name.as_ptr()), RRF_RT_ANY | RRF_NOEXPAND, Some(&mut kind), None, Some(&mut size))
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(Found::Absent);
        }
        if status != NO_ERROR {
            return Err(status);
        }
        if kind != REG_SZ {
            return Ok(Found::Other);
        }
        // Room for one more unit, so the text is always ended even when the stored one was not.
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut got = (buf.len() * 2) as u32;
        // SAFETY: the buffer is `got` bytes long and the name is NUL-terminated.
        let status = unsafe {
            RegGetValueW(
                key.0,
                PCWSTR::null(),
                PCWSTR(name.as_ptr()),
                RRF_RT_ANY | RRF_NOEXPAND,
                Some(&mut kind),
                Some(buf.as_mut_ptr() as *mut c_void),
                Some(&mut got),
            )
        };
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(Found::Absent);
        }
        if status != NO_ERROR {
            return Err(status);
        }
        return Ok(if kind == REG_SZ { Found::Text(text_of(&buf)) } else { Found::Other });
    }
    Err(ERROR_MORE_DATA)
}

/// Every string value under an open key, as (name, data). Values of another type are skipped, and so
/// is one that changed size between the question and the read: neither is something this session wrote.
fn enumerate(key: &Open) -> Result<Vec<(String, String)>, WIN32_ERROR> {
    let (mut count, mut max_name, mut max_data) = (0u32, 0u32, 0u32);
    // SAFETY: the key is open, and every out-parameter is either a local or left null.
    let status = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut count),
            Some(&mut max_name),
            Some(&mut max_data),
            None,
            None,
        )
    };
    if status != NO_ERROR {
        return Err(status);
    }
    let mut out = Vec::new();
    for index in 0..count {
        let mut name = vec![0u16; max_name as usize + 2];
        let mut name_len = name.len() as u32;
        let mut data = vec![0u16; (max_data as usize).div_ceil(2) + 2];
        let mut data_len = (data.len() * 2) as u32;
        let mut kind = 0u32;
        // SAFETY: both buffers are as long as the lengths passed say, and `kind` is a local.
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr() as *mut u8),
                Some(&mut data_len),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != NO_ERROR || kind != REG_SZ.0 {
            continue;
        }
        out.push((String::from_utf16_lossy(&name[..name_len as usize]), text_of(&data)));
    }
    Ok(out)
}

/// A UTF-16 buffer as the text before its first zero.
fn text_of(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn describe(status: WIN32_ERROR) -> String {
    format!("error {}", status.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A scratch location under `HKCU`: the same Win32 calls as production against a key nobody else
    /// owns, removed whole when the test ends - whatever the test did to it.
    struct Scratch {
        location: Location,
    }

    impl Scratch {
        fn new() -> Scratch {
            let unique = format!("ChronoMockPolicyTest-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
            Scratch {
                location: Location {
                    root: HKEY_CURRENT_USER,
                    path: vec!["Software".into(), unique, "Edge".into(), "WebView2".into(), "Args".into()],
                },
            }
        }

        /// A value put in by hand, the way a tester would: creating the keys as needed.
        fn plant(&self, name: &str, kind: REG_VALUE_TYPE, bytes: &[u8]) {
            write_value(&self.location, "unused-so-keys-exist", "x").unwrap();
            let key = open_deepest(&self.location, KEY_SET_VALUE).unwrap().unwrap();
            let status = unsafe { RegSetValueExW(key.0, PCWSTR(wide(name).as_ptr()), None, kind, Some(bytes)) };
            assert_eq!(status, NO_ERROR);
            let status = unsafe { RegDeleteValueW(key.0, PCWSTR(wide("unused-so-keys-exist").as_ptr())) };
            assert_eq!(status, NO_ERROR);
        }

        fn plant_text(&self, name: &str, text: &str) {
            let bytes: Vec<u8> = wide(text).iter().flat_map(|u| u.to_le_bytes()).collect();
            self.plant(name, REG_SZ, &bytes);
        }

        fn value(&self, name: &str) -> Found {
            match open_deepest(&self.location, KEY_QUERY_VALUE).unwrap() {
                Some(key) => read_value(&key, name).unwrap(),
                None => Found::Absent,
            }
        }

        fn key_exists(&self, count: usize) -> bool {
            let path = wide(&self.location.prefix(count));
            let mut key = HKEY::default();
            let status =
                unsafe { RegOpenKeyExW(self.location.root, PCWSTR(path.as_ptr()), None, KEY_QUERY_VALUE, &mut key) };
            if status == NO_ERROR {
                drop(Open(key));
            }
            status == NO_ERROR
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // Deepest first, each only if present: the test may have removed some of it already.
            for count in (2..=self.location.path.len()).rev() {
                let path = wide(&self.location.prefix(count));
                let _ = unsafe { RegDeleteKeyW(self.location.root, PCWSTR(path.as_ptr())) };
            }
        }
    }

    const OWNER: (u32, u64) = (4242, 777);
    const MINE: &str = "--chrono-mock-session=4242.777";

    fn written(set: PolicySet) -> PolicyValue {
        match set {
            PolicySet::Written(value) => value,
            other => panic!("expected a written value, got {other:?}"),
        }
    }

    /// The round trip on a registry that has nothing: the value appears with the port and the marker,
    /// and taking it away removes the value and every key this session created.
    #[test]
    fn a_value_is_written_and_removed_with_the_keys_it_created() {
        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        assert_eq!(scratch.value("app.exe"), Found::Text(format!("--remote-debugging-port=0 {MINE}")));
        assert!(scratch.key_exists(scratch.location.path.len()));

        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(scratch.value("app.exe"), Found::Absent);
        assert!(!scratch.key_exists(2), "the keys this session created go with the value");
        assert!(scratch.key_exists(1), "the key that was always there stays");
    }

    /// Dropping the guard is the last line of defence for a core that unwinds.
    #[test]
    fn dropping_the_guard_removes_the_value() {
        let scratch = Scratch::new();
        drop(written(set_in(&scratch.location, "app.exe", OWNER)));
        assert_eq!(scratch.value("app.exe"), Found::Absent);
        assert!(!scratch.key_exists(2));
    }

    /// Keys that were there before the session are not the session's to remove, and neither are the
    /// values next to ours.
    #[test]
    fn keys_and_values_that_were_there_before_stay() {
        let scratch = Scratch::new();
        scratch.plant_text("other.exe", "--some-flag");
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(scratch.value("other.exe"), Found::Text("--some-flag".into()));
        assert_eq!(scratch.value("app.exe"), Found::Absent);
        assert!(scratch.key_exists(scratch.location.path.len()), "a key with a value of the tester's in it is not empty");
    }

    /// A value under the application's name that is not ours is never written over - of any type, which
    /// is the reason the existing read-only reader (strings only) is not the check.
    #[test]
    fn a_value_under_the_same_name_is_left_alone_whatever_its_type() {
        let text_bytes = |text: &str| -> Vec<u8> { wide(text).iter().flat_map(|u| u.to_le_bytes()).collect() };
        let planted = [
            (REG_SZ, text_bytes("--mine")),
            (REG_VALUE_TYPE(4), vec![1, 0, 0, 0]),
            (REG_VALUE_TYPE(2), text_bytes("%TEMP%")),
        ];
        for (kind, bytes) in planted {
            let scratch = Scratch::new();
            scratch.plant("app.exe", kind, &bytes);
            assert!(matches!(set_in(&scratch.location, "app.exe", OWNER), PolicySet::Foreign), "type {}", kind.0);
            let key = open_deepest(&scratch.location, KEY_QUERY_VALUE).unwrap().unwrap();
            assert_ne!(read_value(&key, "app.exe").unwrap(), Found::Absent, "the planted value is still there");
        }
        // A name compares without regard to case, the way the registry does.
        let scratch = Scratch::new();
        scratch.plant_text("APP.EXE", "--mine");
        assert!(matches!(set_in(&scratch.location, "app.exe", OWNER), PolicySet::Foreign));
    }

    /// `*` is copied into ours, because a value under the file name replaces it for that host.
    #[test]
    fn the_star_value_is_carried_into_ours_and_left_where_it_was() {
        let scratch = Scratch::new();
        scratch.plant_text("*", "--enable-features=Foo");
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        assert_eq!(
            scratch.value("app.exe"),
            Found::Text(format!("--enable-features=Foo --remote-debugging-port=0 {MINE}"))
        );
        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(scratch.value("*"), Found::Text("--enable-features=Foo".into()), "the tester's value is untouched");
    }

    /// A `*` that already opens a port needs no value of ours: the port it opens is found anyway.
    #[test]
    fn a_star_value_with_its_own_port_means_nothing_is_written() {
        let scratch = Scratch::new();
        scratch.plant_text("*", "--remote-debugging-port=9222");
        assert!(matches!(set_in(&scratch.location, "app.exe", OWNER), PolicySet::StarHasPort));
        assert_eq!(scratch.value("app.exe"), Found::Absent);
    }

    /// A `*` that is not a string cannot be carried into ours, and is not overwritten either.
    #[test]
    fn a_star_value_that_is_not_a_string_is_refused_with_a_reason() {
        let scratch = Scratch::new();
        scratch.plant("*", REG_VALUE_TYPE(4), &[1, 0, 0, 0]);
        match set_in(&scratch.location, "app.exe", OWNER) {
            PolicySet::Failed(why) => assert!(why.contains("not a plain string"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(scratch.value("app.exe"), Found::Absent);
    }

    /// The decision over every pair, without a registry.
    #[test]
    fn the_decision_is_made_from_the_two_values() {
        use Found::{Absent, Other, Text};
        let port = format!("--remote-debugging-port=0 {MINE}");
        assert_eq!(decide(&Absent, &Absent, MINE), Decision::Write(port.clone()));
        assert_eq!(decide(&Text("--a".into()), &Absent, MINE), Decision::Write(format!("--a {port}")));
        assert_eq!(decide(&Text("  --a  ".into()), &Absent, MINE), Decision::Write(format!("--a {port}")), "trimmed");
        assert_eq!(decide(&Text("  ".into()), &Absent, MINE), Decision::Write(port.clone()), "an empty star adds nothing");
        assert_eq!(decide(&Text("--x --remote-debugging-port=1 --y".into()), &Absent, MINE), Decision::StarHasPort);
        assert_eq!(decide(&Text("--remote-debugging-port".into()), &Absent, MINE), Decision::StarHasPort);
        assert_eq!(decide(&Other, &Absent, MINE), Decision::Refuse("the * value is not a plain string, so it could not be carried into ours"));
        assert_eq!(decide(&Absent, &Text("x".into()), MINE), Decision::Foreign);
        assert_eq!(decide(&Text("--remote-debugging-port=1".into()), &Other, MINE), Decision::Foreign, "ours comes first");
    }

    #[test]
    fn a_port_switch_is_a_whole_token() {
        assert!(has_port_switch("--remote-debugging-port=9222"));
        assert!(has_port_switch("--a  --remote-debugging-port=0"));
        assert!(!has_port_switch("--remote-debugging-portal=1"));
        assert!(!has_port_switch("--remote-debugging-pipe"));
        assert!(!has_port_switch("--x=--remote-debugging-port=1"));
        assert!(!has_port_switch(""));
    }

    #[test]
    fn a_marker_round_trips_and_only_a_whole_one_parses() {
        assert_eq!(parse_marker(&format!("--a {}", marker_text(4242, 133_700_000_000_000_000))), Some((4242, 133_700_000_000_000_000)));
        assert_eq!(parse_marker("--chrono-mock-session=12.34 --b"), Some((12, 34)));
        assert_eq!(parse_marker("--chrono-mock-session=12"), None, "no creation time");
        assert_eq!(parse_marker("--chrono-mock-session=x.34"), None);
        assert_eq!(parse_marker("--chrono-mock-session=12.y"), None);
        assert_eq!(parse_marker("--chrono-mock-session=99999999999.1"), None, "a pid is 32 bits");
        assert_eq!(parse_marker("--remote-debugging-port=0"), None);
    }

    /// The scan removes the value of a core that is gone, and leaves every other value: one that is the
    /// tester's, one of a core that is alive, and one that only looks like ours.
    #[test]
    fn the_scan_removes_only_values_whose_owner_is_gone() {
        let scratch = Scratch::new();
        scratch.plant_text("dead.exe", "--remote-debugging-port=0 --chrono-mock-session=1.1");
        scratch.plant_text("alive.exe", "--remote-debugging-port=0 --chrono-mock-session=2.2");
        scratch.plant_text("tester.exe", "--remote-debugging-port=9222");
        scratch.plant_text("marker-less.exe", "--chrono-mock-session");
        scratch.plant("dword.exe", REG_VALUE_TYPE(4), &[1, 0, 0, 0]);

        let alive = |pid: u32, _created: u64| pid == 2;
        assert!(stale_in(&scratch.location, alive), "a value of a dead owner is there to be found");
        assert_eq!(recover_in(&scratch.location, alive), Recovery { removed: 1, failed: 0 });
        assert_eq!(scratch.value("dead.exe"), Found::Absent);
        for kept in ["alive.exe", "tester.exe", "marker-less.exe", "dword.exe"] {
            assert_ne!(scratch.value(kept), Found::Absent, "{kept} stays");
        }
        assert!(!stale_in(&scratch.location, alive), "and once removed it is not found again");
        assert_eq!(recover_in(&scratch.location, alive), Recovery::default(), "a second scan has nothing to do");
    }

    /// Nothing to scan is not an error: the key may not exist at all, which is the ordinary machine.
    #[test]
    fn a_scan_of_a_missing_key_finds_nothing() {
        let scratch = Scratch::new();
        assert!(!stale_in(&scratch.location, |_, _| false));
        assert_eq!(recover_in(&scratch.location, |_, _| false), Recovery::default());
    }

    /// A value that was replaced during the session, by a policy refresh or by the tester, is no longer
    /// ours: removal leaves theirs, and says the session left nothing of its own.
    #[test]
    fn a_replaced_value_is_left_and_a_missing_one_is_removed() {
        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        scratch.plant_text("app.exe", "--the-testers-own");
        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(scratch.value("app.exe"), Found::Text("--the-testers-own".into()));

        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        // Gone before the session got to it, together with its keys.
        let key = open_deepest(&scratch.location, KEY_SET_VALUE).unwrap().unwrap();
        assert_eq!(unsafe { RegDeleteValueW(key.0, PCWSTR(wide("app.exe").as_ptr())) }, NO_ERROR);
        drop(key);
        assert_eq!(value.remove(), PolicyRemoval::Removed);
    }

    /// Keys that were there before the session are not the session's to remove - not even when they are
    /// empty, which is exactly when removing them would look harmless. Only the key the session created
    /// goes, and the ones above it stay.
    #[test]
    fn keys_that_were_already_there_stay_even_when_empty() {
        let scratch = Scratch::new();
        // The first four levels exist and hold nothing, the way a policy tree left empty by a tester does.
        let above = Location { root: scratch.location.root, path: scratch.location.path[..4].to_vec() };
        write_value(&above, "unused", "x").unwrap();
        let key = open_deepest(&above, KEY_SET_VALUE).unwrap().unwrap();
        assert_eq!(unsafe { RegDeleteValueW(key.0, PCWSTR(wide("unused").as_ptr())) }, NO_ERROR);
        drop(key);
        assert!(scratch.key_exists(4) && !scratch.key_exists(5));

        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        assert!(scratch.key_exists(5), "the session created the last level");
        assert_eq!(value.remove(), PolicyRemoval::Removed);

        assert!(!scratch.key_exists(5), "the key it created is gone");
        assert!(scratch.key_exists(4), "the empty keys that were already there stay");
    }

    /// A key the session created that the tester then put a value of their own in is not empty any more,
    /// and a key that is not empty is never deleted: ours goes, theirs stays, and so does the key.
    #[test]
    fn a_key_the_session_created_but_the_tester_then_used_is_kept() {
        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        scratch.plant_text("other.exe", "--theirs");

        assert_eq!(value.remove(), PolicyRemoval::Removed);

        assert_eq!(scratch.value("app.exe"), Found::Absent, "ours is gone");
        assert_eq!(scratch.value("other.exe"), Found::Text("--theirs".into()), "theirs is not touched");
        assert!(scratch.key_exists(scratch.location.path.len()), "and the key that holds it stays");
    }

    /// A process that cannot be opened is alive only when access was denied: it exists and will not say.
    /// Any other failure is a pid nothing answers to.
    #[test]
    fn a_process_that_will_not_answer_is_alive_and_one_that_does_not_exist_is_not() {
        use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND};
        assert!(open_error_means_alive(ERROR_ACCESS_DENIED.to_hresult()));
        assert!(!open_error_means_alive(ERROR_INVALID_PARAMETER.to_hresult()), "what OpenProcess says for a pid nobody has");
        assert!(!open_error_means_alive(ERROR_NOT_FOUND.to_hresult()));
    }

    #[test]
    fn removing_twice_says_the_same_thing() {
        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(value.remove(), PolicyRemoval::Removed);
    }

    /// A process that is running is alive, a pid nothing answers to is not, and the creation time tells
    /// a recycled pid from the process that wrote the marker.
    #[test]
    fn the_owner_is_alive_only_when_the_same_process_is_running() {
        let created = unsafe { crate::process_created(GetCurrentProcess()) }.unwrap();
        assert!(owner_alive(std::process::id(), created));
        assert!(!owner_alive(std::process::id(), created + 1), "same pid, another creation time: a recycled pid");
        assert!(!owner_alive(u32::MAX - 5, created), "no such process");
    }

    /// A file name that would land somewhere other than under the application's name is refused before
    /// anything is written: the empty name is the key's default value, and `*` is every host's.
    #[test]
    fn a_name_that_is_not_a_value_name_is_refused_and_nothing_is_written() {
        for bad in ["", "*", "a\\b.exe", "a\0b.exe"] {
            let scratch = Scratch::new();
            match set_in(&scratch.location, bad, OWNER) {
                PolicySet::Failed(why) => assert!(why.contains("file name"), "{bad:?}: {why}"),
                other => panic!("{bad:?} should be refused, got {other:?}"),
            }
            assert!(!scratch.key_exists(2), "{bad:?}: not even the keys were created");
        }
        assert_eq!(unusable_name("app.exe"), None);
        assert_eq!(unusable_name("with space (1).exe"), None);
    }

    /// Removal compares the marker as a whole: a value of another session that shares the pid's
    /// leading digits, or the creation time's, is not taken for ours.
    #[test]
    fn removal_takes_only_a_value_whose_whole_marker_is_this_sessions() {
        let scratch = Scratch::new();
        let mut value = written(set_in(&scratch.location, "app.exe", OWNER));
        scratch.plant_text("app.exe", "--remote-debugging-port=0 --chrono-mock-session=4242.7770");
        assert_eq!(value.remove(), PolicyRemoval::Removed);
        assert_eq!(
            scratch.value("app.exe"),
            Found::Text("--remote-debugging-port=0 --chrono-mock-session=4242.7770".into()),
            "another session's value is not ours to delete"
        );
    }
}
