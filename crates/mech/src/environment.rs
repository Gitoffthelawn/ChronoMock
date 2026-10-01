//! The environment block a launched target inherits, with the session's own variables added.
//!
//! `CreateProcessW` with a null `lpEnvironment` hands the child a copy of this process's environment,
//! and that is what every session did until the embedded-engine channel (docs/09) needed to put two
//! variables in front of the target. Once a caller supplies a block, the system stops doing three
//! things for it, and this module does them instead:
//!
//! - the drive-directory entries (`=C:=C:\...`) are not propagated - so the block is built from
//!   `GetEnvironmentStringsW`, which carries them, never from `std::env`, which may not,
//! - the block must be sorted by name, case-insensitively in Unicode order,
//! - a Unicode block ends with four zero bytes.
//!
//! All three are documented for `CreateProcessW` and "Changing Environment Variables" on Microsoft
//! Learn. The entries travel as the UTF-16 units the system holds them in (R4-N17): Windows does not
//! require a variable to be valid UTF-16, and a value with an unpaired surrogate used to reach the
//! target with that unit replaced, which no session without the block ever did. The merge and the
//! encoding are pure functions over those entries, so they are tested without launching anything - and
//! a live launch with a block is what `launch_plain` proves.

use windows::Win32::System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW};

/// `=` as a UTF-16 unit. It is never half of a surrogate pair, so splitting at it by unit is safe.
const EQUALS: u16 = 0x3D;

/// Build the Unicode environment block for a child: this process's environment with `extra` merged
/// in (a name already present is replaced), sorted as the system requires, doubly terminated.
pub fn environment_block(extra: &[(String, String)]) -> Vec<u16> {
    encode_block(&merge_entries(raw_environment(), extra))
}

/// This process's environment as text, drive-directory entries included - for READING a variable, as
/// the embedded-engine channel reads its two. A name is everything before the first `=` past the
/// first character, so an entry like `=C:=C:\work` keeps its leading `=` as part of the name, exactly
/// as the system stores it. A unit that is not valid UTF-16 reads as U+FFFD here, which is why the
/// block a child gets is never built from this view.
pub fn current_environment() -> Vec<(String, String)> {
    raw_environment().iter().map(|entry| split_entry(&String::from_utf16_lossy(entry))).collect()
}

/// This process's environment exactly as the system holds it: one `name=value` entry each, in UTF-16
/// units, without its terminating zero.
fn raw_environment() -> Vec<Vec<u16>> {
    // SAFETY: GetEnvironmentStringsW returns a block owned by the system that stays valid until the
    // matching FreeEnvironmentStringsW, which runs after the last read below. A null pointer (the
    // documented failure) yields an empty environment rather than a read through null.
    unsafe {
        let block = GetEnvironmentStringsW();
        if block.is_null() {
            return Vec::new();
        }
        let mut entries = Vec::new();
        let mut cursor = block.0;
        loop {
            let len = (0..).take_while(|&i| *cursor.add(i) != 0).count();
            if len == 0 {
                break;
            }
            entries.push(std::slice::from_raw_parts(cursor, len).to_vec());
            cursor = cursor.add(len + 1);
        }
        let _ = FreeEnvironmentStringsW(block);
        entries
    }
}

/// `name=value` into its two halves. The split is at the first `=` AFTER the first character, so a
/// drive-directory entry's leading `=` stays with the name. By character, not by byte: a name may
/// start with a character wider than one byte, and a byte slice at offset one would panic on it.
fn split_entry(text: &str) -> (String, String) {
    match text.char_indices().skip(1).find(|(_, c)| *c == '=') {
        Some((at, _)) => (text[..at].to_string(), text[at + 1..].to_string()),
        None => (text.to_string(), String::new()),
    }
}

/// The name of a raw entry: its units before the first `=` past the first unit, the same split as
/// `split_entry`.
fn name_of(entry: &[u16]) -> &[u16] {
    match entry.iter().skip(1).position(|&unit| unit == EQUALS) {
        Some(at) => &entry[..=at],
        None => entry,
    }
}

/// The current entries with `extra` merged in: a name already present is replaced in place (names
/// compare without regard to case, as the system compares them), a new one is appended. Then the
/// whole list is sorted the way the system requires, so the caller never has to know that a block
/// is sorted at all. Entries `extra` does not name are kept unit for unit.
pub(crate) fn merge_entries(mut entries: Vec<Vec<u16>>, extra: &[(String, String)]) -> Vec<Vec<u16>> {
    for (name, value) in extra {
        let wanted = sort_key(&name.encode_utf16().collect::<Vec<u16>>());
        let entry: Vec<u16> = format!("{name}={value}").encode_utf16().collect();
        match entries.iter_mut().find(|present| sort_key(name_of(present)) == wanted) {
            Some(slot) => *slot = entry,
            None => entries.push(entry),
        }
    }
    entries.sort_by_key(|entry| sort_key(name_of(entry)));
    entries
}

/// The order the system sorts an environment block in: by name, case-insensitively, in Unicode
/// order without regard to locale. Upper-casing per character is that comparison for every script
/// with a simple case mapping, and the names in a Windows environment are ASCII in practice. A unit
/// that is half of no pair sorts as itself.
fn sort_key(name: &[u16]) -> Vec<u32> {
    char::decode_utf16(name.iter().copied())
        .flat_map(|decoded| match decoded {
            Ok(c) => c.to_uppercase().map(u32::from).collect::<Vec<u32>>(),
            Err(unpaired) => vec![u32::from(unpaired.unpaired_surrogate())],
        })
        .collect()
}

/// The block as `CreateProcessW` reads it with `CREATE_UNICODE_ENVIRONMENT`: each `name=value`
/// zero-terminated, then one more zero after the last - four zero bytes at the end, as documented.
/// An empty list still yields a valid (empty) block of one terminating zero pair, because a block
/// that ends in two zero bytes is the one shape the caller can hand over unconditionally.
pub(crate) fn encode_block(entries: &[Vec<u16>]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for entry in entries {
        out.extend_from_slice(entry);
        out.push(0);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(n: &str, v: &str) -> (String, String) {
        (n.to_string(), v.to_string())
    }

    fn entry(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn texts(entries: &[Vec<u16>]) -> Vec<String> {
        entries.iter().map(|e| String::from_utf16_lossy(e)).collect()
    }

    #[test]
    fn a_present_name_is_replaced_without_regard_to_case_and_a_new_one_is_appended() {
        let merged = merge_entries(
            vec![entry("Path=C:\\bin"), entry("TEMP=C:\\t")],
            &[pair("path", "D:\\bin"), pair("CHRONO_X", "1")],
        );
        assert_eq!(texts(&merged), ["CHRONO_X=1", "path=D:\\bin", "TEMP=C:\\t"]);
    }

    #[test]
    fn the_block_is_sorted_by_name_case_insensitively_with_drive_entries_first() {
        // `=` sorts below every letter, which is why the system keeps the drive-directory entries at
        // the front - and this is the order the system requires, not a preference.
        let merged =
            merge_entries(vec![entry("zeta=1"), entry("Alpha=2"), entry("=C:=C:\\work"), entry("beta=3")], &[]);
        assert_eq!(texts(&merged), ["=C:=C:\\work", "Alpha=2", "beta=3", "zeta=1"]);
    }

    #[test]
    fn a_drive_directory_entry_keeps_its_leading_equals_sign_in_the_name() {
        assert_eq!(split_entry("=C:=C:\\work"), pair("=C:", "C:\\work"));
        assert_eq!(split_entry("PATH=C:\\bin;D:\\bin"), pair("PATH", "C:\\bin;D:\\bin"));
        assert_eq!(split_entry("EMPTY="), pair("EMPTY", ""));
        assert_eq!(split_entry("NOEQUALS"), pair("NOEQUALS", ""));
        // A name that starts with a character wider than one byte, and a name that IS one.
        assert_eq!(split_entry("\u{c4}_NAME=v"), pair("\u{c4}_NAME", "v"));
        assert_eq!(split_entry("\u{c4}=v"), pair("\u{c4}", "v"));
        assert_eq!(split_entry("\u{c4}"), pair("\u{c4}", ""));
        // The raw split agrees, a name made of a surrogate pair included.
        assert_eq!(name_of(&entry("=C:=C:\\work")), entry("=C:").as_slice());
        assert_eq!(name_of(&entry("\u{1F600}=v")), entry("\u{1F600}").as_slice());
        assert_eq!(name_of(&entry("NOEQUALS")), entry("NOEQUALS").as_slice());
    }

    /// A value Windows holds with an unpaired surrogate reaches the child unit for unit (R4-N17), and so
    /// does a name with one. Read as text first, both came out with U+FFFD in its place.
    #[test]
    fn an_entry_that_is_not_valid_utf16_reaches_the_child_unit_for_unit() {
        let odd_value: Vec<u16> = vec![u16::from(b'A'), EQUALS, u16::from(b'x'), 0xD800, u16::from(b'y')];
        let odd_name: Vec<u16> = vec![u16::from(b'B'), 0xDC00, EQUALS, u16::from(b'1')];
        let merged = merge_entries(vec![odd_name.clone(), odd_value.clone()], &[pair("CHRONO_X", "1")]);
        assert!(merged.contains(&odd_value) && merged.contains(&odd_name), "{merged:?}");
        let block = encode_block(&merged);
        assert!(block.windows(odd_value.len()).any(|w| w == odd_value.as_slice()), "{block:?}");
        assert!(!block.contains(&0xFFFD), "a unit was replaced on the way: {block:?}");
    }

    /// The whole road, from the live environment: a variable this process holds with an unpaired
    /// surrogate is in the block a child gets, unit for unit (R4-N17).
    #[test]
    fn a_live_variable_that_is_not_valid_utf16_reaches_the_block_unit_for_unit() {
        use windows::core::PCWSTR;
        use windows::Win32::System::Environment::SetEnvironmentVariableW;
        let name: Vec<u16> = "CHRONO_N17_PROBE".encode_utf16().chain([0]).collect();
        let value: Vec<u16> = vec![u16::from(b'x'), 0xD800, u16::from(b'y'), 0];
        // SAFETY: both strings are zero-terminated and outlive the call. The name is this test's own.
        unsafe { SetEnvironmentVariableW(PCWSTR(name.as_ptr()), PCWSTR(value.as_ptr())) }.expect("the variable is set");
        let block = environment_block(&[]);
        let wanted: Vec<u16> = "CHRONO_N17_PROBE=x".encode_utf16().chain([0xD800, u16::from(b'y'), 0]).collect();
        assert!(block.windows(wanted.len()).any(|w| w == wanted.as_slice()), "the variable did not reach the block unit for unit");
    }

    #[test]
    fn the_encoded_block_ends_in_four_zero_bytes_and_separates_entries_with_one() {
        let block = encode_block(&[entry("A=1"), entry("B=")]);
        let expected: Vec<u16> = "A=1\0B=\0\0".encode_utf16().collect();
        assert_eq!(block, expected);
        assert_eq!(encode_block(&[]), vec![0]);
    }

    #[test]
    fn the_live_environment_reads_back_with_the_names_this_process_can_see() {
        // Not a fixed list - the machine decides its environment - but a process always has a
        // PATH-like set, and the entries come back split at the right place.
        let live = current_environment();
        assert!(!live.is_empty());
        assert!(live.iter().all(|(n, _)| !n.is_empty()));
        assert!(live.iter().any(|(n, _)| n.eq_ignore_ascii_case("SystemRoot") || n.eq_ignore_ascii_case("Path")));
    }
}
