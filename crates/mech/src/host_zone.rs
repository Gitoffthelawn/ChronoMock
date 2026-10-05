//! The name this machine's time zone goes by, as the system's own ICU names it (`Europe/Warsaw`).
//!
//! For the pages of an application that outlives the session, which are put back on it (ADR-21). An
//! engine inside a hooked application keeps the last zone it was given, also once the connection that
//! gave it closes (measured on WebView2 154, 2026-10-05), so taking the session's zone away leaves a
//! page on it. A named zone puts the page back on this machine's offsets on either side of daylight
//! saving, which a fixed offset of today would not.
//!
//! The ICU in Windows is a system library from Windows 10 version 1903 on, and only its C functions
//! are exposed (Microsoft Learn, "International Components for Unicode (ICU)"). It is loaded from the
//! system folder when asked and let go again, so a system without it costs the answer and nothing else.

use windows::core::{s, w, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

/// Room for the answer, in UTF-16 units. The names in the zone database are far shorter, so an answer
/// that fills this is not taken as a name.
const CAPACITY: usize = 64;

/// ICU's `ucal_getHostTimeZone` and `ucal_getDefaultTimeZone`: the name into a buffer of the given
/// capacity, its length returned, and the status written to the last argument - above zero is an error,
/// below zero a warning (ICU's `UErrorCode`).
type ZoneNameFn = unsafe extern "C" fn(*mut u16, i32, *mut i32) -> i32;

/// This machine's time zone as ICU names it, or `None` when the system has no ICU, ICU gives no name,
/// or the name is not one an engine can take ([`zone_name`]).
///
/// `ucal_getHostTimeZone` (ICU 65) asks the system afresh, and turns a zone whose daylight saving the
/// user switched off into a fixed offset of its own (an assessment from ICU's detection, not measured
/// here, because measuring it means changing a setting of the machine).
/// `ucal_getDefaultTimeZone` is the way for an older ICU: the default of this process, which never sets
/// one, so it is the system's zone too.
pub fn host_zone_name() -> Option<String> {
    // SAFETY: the library is loaded from the system folder only, every entry point is checked before it
    // is called, and the library is let go on the one path out once the answers are read.
    unsafe {
        let library = LoadLibraryExW(w!("icu.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let name = [s!("ucal_getHostTimeZone"), s!("ucal_getDefaultTimeZone")]
            .into_iter()
            .find_map(|function| ask(library, function));
        let _ = FreeLibrary(library);
        name
    }
}

/// One ICU function's answer, checked.
///
/// # Safety
/// `library` must be the loaded ICU, and `function` the name of an entry point of the `ZoneNameFn` shape.
unsafe fn ask(library: HMODULE, function: PCSTR) -> Option<String> { unsafe {
    let entry = GetProcAddress(library, function)?;
    let call: ZoneNameFn = std::mem::transmute(entry);
    let mut units = [0u16; CAPACITY];
    let mut status = 0i32;
    let length = call(units.as_mut_ptr(), CAPACITY as i32, &raw mut status);
    answer(&units, length, status)
}}

/// What an ICU call wrote, read as a zone name: `None` for an error status, a length below zero, or an
/// answer that fills the buffer - ICU then left it without its end, or a longer name did not fit. Pure,
/// so the paths the system's ICU never takes here are tested too.
fn answer(units: &[u16; CAPACITY], length: i32, status: i32) -> Option<String> {
    if status > 0 {
        return None;
    }
    let length = usize::try_from(length).ok().filter(|&n| n < CAPACITY)?;
    zone_name(&units[..length])
}

/// The answer as a zone name an engine takes, or `None`: not empty, only ASCII letters, digits and
/// `/ _ + -`, and not ICU's `Etc/Unknown`, which it answers when it could not map the system's zone and
/// which names no zone at all. Pure, so the rule is tested without ICU.
fn zone_name(units: &[u16]) -> Option<String> {
    let name = String::from_utf16(units).ok()?;
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-');
    (!name.is_empty() && name.chars().all(allowed) && name != "Etc/Unknown").then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_zone_name_is_taken_as_it_is_and_nothing_else_is() {
        for name in ["Europe/Warsaw", "America/Argentina/Buenos_Aires", "Etc/GMT-1", "Etc/GMT+12", "UTC"] {
            assert_eq!(zone_name(&units(name)).as_deref(), Some(name));
        }
        assert_eq!(zone_name(&units("Etc/Unknown")), None, "ICU's answer for a zone it could not map");
        assert_eq!(zone_name(&[]), None);
        for not_a_name in ["Europe/Warsaw\"", "Europe Warsaw", "a\\b"] {
            assert_eq!(zone_name(&units(not_a_name)), None, "{not_a_name}");
        }
        let mut accented = units("Europe/Warsaw");
        accented.push(0x0105);
        assert_eq!(zone_name(&accented), None, "a letter outside ASCII");
        assert_eq!(zone_name(&[0xD800]), None, "not text at all");
    }

    /// An error status, a length below zero and an answer that fills the buffer are no name, whatever the
    /// buffer holds. A warning status (below zero, such as ICU's fallback warning) still carries one.
    #[test]
    fn an_icu_answer_is_read_only_when_it_is_whole() {
        let mut buffer = [0u16; CAPACITY];
        let name = units("Europe/Warsaw");
        buffer[..name.len()].copy_from_slice(&name);
        let len = i32::try_from(name.len()).unwrap();
        assert_eq!(answer(&buffer, len, 0).as_deref(), Some("Europe/Warsaw"));
        assert_eq!(answer(&buffer, len, -128).as_deref(), Some("Europe/Warsaw"), "a warning is no failure");
        assert_eq!(answer(&buffer, len, 1), None, "an error status");
        assert_eq!(answer(&buffer, -1, 0), None, "a length below zero");
        let full = [u16::from(b'A'); CAPACITY];
        let capacity = i32::try_from(CAPACITY).unwrap();
        assert_eq!(answer(&full, capacity, -124), None, "a buffer filled to the end has no end");
        assert_eq!(answer(&full, capacity - 1, 0).as_deref().map(str::len), Some(CAPACITY - 1), "one short of it is whole");
    }

    /// The system's ICU answers on every Windows this runs on (from 1903 on), here and on the CI runner,
    /// with a name an engine takes.
    #[test]
    fn this_machine_names_its_zone() {
        let name = host_zone_name().expect("the system's ICU names this machine's zone");
        assert!(zone_name(&units(&name)).is_some(), "{name}");
    }
}
