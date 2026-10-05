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
//!
//! The name is given, never trusted: an engine that does not know it keeps the page where it was. So
//! the session reads back what each page shows for two instants of the year and compares it with this
//! machine's offsets at them ([`host_zone_sample`]), which come from the system's own zone settings, not
//! from ICU, and are there also when ICU names nothing.

use windows::core::{s, w, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, FILETIME, HMODULE, SYSTEMTIME};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::System::Time::{
    GetDynamicTimeZoneInformation, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
    DYNAMIC_TIME_ZONE_INFORMATION,
};

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

/// Two instants of the current year and this machine's offset at each, for a check that a page is back
/// on this machine's zone after the session (ADR-21). The instants are the 15th of January and of July,
/// at noon UTC, so a zone with daylight saving shows both of its offsets, in the northern and the
/// southern half of the world alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneSample {
    /// The two instants, in Unix-epoch milliseconds.
    pub at_ms: [i64; 2],
    /// This machine's offset at each, in minutes as `getTimezoneOffset()` and a Windows bias count it
    /// (UTC minus local time, so `+02:00` is -120). `None` when the system would not convert them.
    pub offsets: Option<[i32; 2]>,
}

/// The [`ZoneSample`] of this machine's zone now, read from the system's own zone settings with their
/// rules for the year (`GetDynamicTimeZoneInformation` and `SystemTimeToTzSpecificLocalTimeEx`, Microsoft
/// Learn). The process asking is never hooked, so this is the zone the machine runs in, not the session's.
pub fn host_zone_sample() -> ZoneSample {
    // SAFETY: every structure passed is a plain struct owned here and sized by its type, and nothing is
    // kept past the calls.
    unsafe {
        let year = GetSystemTime().wYear;
        let months = [1, 7];
        let instants = months.map(|month| noon_utc(year, month));
        // Arithmetic rather than a conversion of the system's, so there is no failure to stand in for -
        // a sentinel instant would send the read to a date the machine was never sampled at (CodeRabbit
        // on #90).
        let at_ms = months.map(|month| noon_utc_epoch_ms(year, month));
        let mut zone = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // TIME_ZONE_ID_INVALID is the one failure the call reports (Microsoft Learn).
        let read = GetDynamicTimeZoneInformation(&mut zone) != u32::MAX;
        let offsets = read
            .then(|| -> Option<[i32; 2]> { Some([offset_at(&zone, &instants[0])?, offset_at(&zone, &instants[1])?]) })
            .flatten();
        ZoneSample { at_ms, offsets }
    }
}

/// The 15th of `month` in `year`, at noon UTC.
fn noon_utc(year: u16, month: u16) -> SYSTEMTIME {
    SYSTEMTIME { wYear: year, wMonth: month, wDayOfWeek: 0, wDay: 15, wHour: 12, wMinute: 0, wSecond: 0, wMilliseconds: 0 }
}

/// The 15th of `month` in `year`, at noon UTC, as Unix-epoch milliseconds: the days since 1970 of the
/// proleptic Gregorian calendar, by the civil-date arithmetic of H. Hinnant's `days_from_civil`. Pure, and
/// checked against the system's own conversion for every year a session can be in.
fn noon_utc_epoch_ms(year: u16, month: u16) -> i64 {
    let (y, m, d) = (i64::from(year) - i64::from(month <= 2), i64::from(month), 15);
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let day_of_year = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    days * 86_400_000 + 12 * 3_600_000
}

/// The machine's offset at the UTC instant `utc`, in minutes, UTC minus local time.
///
/// # Safety
/// `zone` must be a zone the system filled in.
unsafe fn offset_at(zone: &DYNAMIC_TIME_ZONE_INFORMATION, utc: &SYSTEMTIME) -> Option<i32> { unsafe {
    let mut local = SYSTEMTIME::default();
    SystemTimeToTzSpecificLocalTimeEx(Some(zone), utc, &mut local).ok()?;
    let (mut utc_file, mut local_file) = (FILETIME::default(), FILETIME::default());
    SystemTimeToFileTime(utc, &mut utc_file).ok()?;
    SystemTimeToFileTime(&local, &mut local_file).ok()?;
    offset_minutes(ticks_of(utc_file), ticks_of(local_file))
}}

/// A file time as one number of 100-nanosecond ticks since 1601.
fn ticks_of(file: FILETIME) -> u64 {
    (u64::from(file.dwHighDateTime) << 32) | u64::from(file.dwLowDateTime)
}

/// The offset between the same instant written in UTC and in local time, in whole minutes, UTC minus
/// local. `None` for one that is not whole minutes or lies outside any zone the world uses (more than
/// a day), which no conversion of the system's should give. Pure.
fn offset_minutes(utc_ticks: u64, local_ticks: u64) -> Option<i32> {
    const TICKS_PER_MINUTE: i128 = 600_000_000;
    let diff = i128::from(utc_ticks) - i128::from(local_ticks);
    (diff % TICKS_PER_MINUTE == 0)
        .then_some(diff / TICKS_PER_MINUTE)
        .filter(|m| m.abs() <= 24 * 60)
        .and_then(|m| i32::try_from(m).ok())
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

    /// Known instants: the 15th of January and of July 2026 at noon UTC. And the arithmetic is the system's
    /// own conversion, for both months of every year a SYSTEMTIME can hold - leap years, the century rule
    /// and the four-century one included.
    #[test]
    fn the_sample_instants_are_the_fifteenth_of_january_and_july_at_noon_utc() {
        assert_eq!([noon_utc_epoch_ms(2026, 1), noon_utc_epoch_ms(2026, 7)], [1_768_478_400_000, 1_784_116_800_000]);
        const EPOCH_FROM_1601_MS: i64 = 11_644_473_600_000;
        for year in 1601..=30827u16 {
            for month in [1, 7] {
                let mut file = FILETIME::default();
                // SAFETY: both structures are owned here.
                unsafe { SystemTimeToFileTime(&noon_utc(year, month), &mut file) }.expect("the system converts it");
                let system = i64::try_from(ticks_of(file) / 10_000).expect("in range") - EPOCH_FROM_1601_MS;
                assert_eq!(noon_utc_epoch_ms(year, month), system, "{year}-{month:02}-15");
            }
        }
    }

    /// UTC minus local, in whole minutes: east of UTC is negative, as `getTimezoneOffset()` says it.
    #[test]
    fn an_offset_is_utc_minus_local_in_whole_minutes() {
        let minute = 600_000_000u64;
        let utc = 1_000_000 * minute;
        assert_eq!(offset_minutes(utc, utc + 120 * minute), Some(-120), "+02:00");
        assert_eq!(offset_minutes(utc, utc - 330 * minute), Some(330), "-05:30");
        assert_eq!(offset_minutes(utc, utc), Some(0));
        assert_eq!(offset_minutes(utc, utc + 1), None, "not whole minutes");
        assert_eq!(offset_minutes(utc + 1, utc), None, "not whole minutes, west of UTC");
        assert_eq!(offset_minutes(utc, utc + 25 * 60 * minute), None, "no zone is more than a day away");
    }

    /// This machine's offset now, read the way the sample reads it, is the bias the system reports for
    /// now - here and on the CI runner. Both halves of the sample are offsets some zone uses.
    #[test]
    fn the_sample_reads_this_machines_zone() {
        use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let sample = host_zone_sample();
        let offsets = sample.offsets.expect("the system converts both instants");
        let days = (sample.at_ms[1] - sample.at_ms[0]) / 86_400_000;
        assert!(days == 181 || days == 182, "the 15th of January to the 15th of July: {days} days");
        assert_eq!(sample.at_ms[0] % 86_400_000, 43_200_000, "at noon UTC");
        assert!(offsets.iter().all(|m| (-14 * 60..=12 * 60).contains(m)), "{offsets:?}");
        // SAFETY: the structures are owned here.
        let (now_offset, bias) = unsafe {
            let mut zone = DYNAMIC_TIME_ZONE_INFORMATION::default();
            assert_ne!(GetDynamicTimeZoneInformation(&mut zone), u32::MAX);
            let now = GetSystemTime();
            let mut tzi = TIME_ZONE_INFORMATION::default();
            let id = GetTimeZoneInformation(&mut tzi);
            let bias = tzi.Bias + if id == 2 { tzi.DaylightBias } else { tzi.StandardBias };
            (offset_at(&zone, &now), bias)
        };
        assert_eq!(now_offset, Some(bias));
    }
}
