//! Reading a shipped calendar catalogue off disk and turning it into the core's types.
//!
//! The wire shape (`CalendarDto` and friends) is separate from the engine types on purpose: a
//! catalogue is user-facing data that must be validated before the engine ever sees it, and a bad
//! rule has to name its own field rather than fail somewhere downstream.
//!
//! The catalogue lookup lives here too, including the identifier check that refuses path traversal -
//! an id names a shipped file, never a path.
//!
//! The consumer owns the I/O and serde - the core engine works over already-parsed rules. The JSON
//! schema is the contract (docs/04 section 5) and this is one reader of it. An unknown major schema
//! version is refused, and so is an unknown FIELD, at every level of the file (R4-N38, the owner's
//! decision R4-D11). A calendar is the catalogue people outside this project write by hand, and a
//! typo in an optional field - `valid_form` for `valid_from` - used to be dropped without a word, so
//! the holiday it was meant to limit counted in every year. Presets keep ignoring fields they do not
//! read, because a preset carries fields for the window that this reader has no use for.


use serde::Deserialize;

/// The schema field on its own, read before the rest of the file. A file written to a later version
/// has to be refused as that - read strictly first, it would be refused over whichever of its new
/// fields came first, and the reader would chase a typo that is not there.
#[derive(Deserialize)]
struct SchemaDto {
    schema: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CalendarDto {
    // Read by `SchemaDto` before this struct is. Named here so the field itself is not unknown.
    #[serde(rename = "schema")]
    _schema: String,
    // Contract fields this reader has no use for (docs/04 section 5), named so that refusing unknown
    // fields does not refuse the shipped calendars: `stability` marks the format as not yet frozen,
    // `subregion` waits for states and regions, and `name` is the calendar's own title.
    #[serde(default, rename = "stability")]
    _stability: Option<String>,
    #[serde(default, rename = "subregion")]
    _subregion: Option<String>,
    #[serde(default, rename = "name")]
    _name: Option<NameDto>,
    id: String,
    country: String,
    weekend: Vec<String>,
    observed: String,
    // The years the calendar answers for and the day its holidays were checked against the law, both
    // required (R4-S20, docs/04 section 5): a file without them would answer for every year the
    // calculator computes, which is how Poland's Constitution Day came out a day off in 1985.
    valid_from: i64,
    law_as_of: String,
    // Where the range, the weekend and the observance come from. Each holiday carries its own.
    #[serde(rename = "source")]
    _source: String,
    holidays: Vec<HolidayDto>,
}

/// The earliest `valid_from` a calendar may declare: the first full year of the Gregorian calendar.
/// Every rule here is Gregorian - Easter by Meeus included - so a year before it would be answered
/// with a calendar nobody kept.
pub(crate) const FIRST_GREGORIAN_YEAR: i64 = 1583;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HolidayDto {
    id: String,
    name: NameDto,
    rule: RuleDto,
    #[serde(default)]
    valid_from: Option<i64>,
    #[serde(default)]
    valid_to: Option<i64>,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NameDto {
    en: String,
    local: String,
}

// `deny_unknown_fields` on an internally tagged enum was measured with this build's serde before it
// was relied on: a stray field in any rule is refused, the `type` tag itself is not, and it does not
// matter where in the object the tag stands.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RuleDto {
    Fixed { month: u32, day: u32 },
    NthWeekday { month: u32, weekday: String, order: i32 },
    EasterOffset { offset: i32 },
}

/// Map a weekday name to a Sunday-based index 0..=6.
pub(crate) fn weekday_index(name: &str) -> Result<u32, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "sunday" => 0,
        "monday" => 1,
        "tuesday" => 2,
        "wednesday" => 3,
        "thursday" => 4,
        "friday" => 5,
        "saturday" => 6,
        other => return Err(format!("unknown weekday '{other}'")),
    })
}

pub(crate) fn observed_from(s: &str) -> Result<chrono_core::calendar::Observed, String> {
    use chrono_core::calendar::Observed;
    Ok(match s {
        "none" => Observed::None,
        "sat_to_fri_sun_to_mon" => Observed::SatToFriSunToMon,
        "sun_to_mon" => Observed::SunToMon,
        "weekend_to_mon" => Observed::WeekendToMon,
        other => return Err(format!("unknown observed rule '{other}'")),
    })
}

/// Validate and map one holiday rule. The engine trusts its inputs, and a calendar is a data file from
/// outside the build - the one documented extension point of this tool - so an out-of-range field must be
/// refused HERE, naming the field. Left unchecked it does not fail, which is worse: `days_from_civil`
/// happily rolls month 13 into the next January and day 40 into the following month, so the calendar
/// silently marks the wrong dates as holidays and every business-day answer built on it is quietly wrong.
pub(crate) fn rule_from(id: &str, dto: RuleDto) -> Result<chrono_core::calendar::HolidayRule, String> {
    use chrono_core::calendar::HolidayRule;
    Ok(match dto {
        RuleDto::Fixed { month, day } => {
            check_month(id, month)?;
            // The month's longest length in ANY year, not a flat 1..=31. Two different things look
            // alike here and only one of them is legal. February 29 is a rule a calendar may
            // legitimately name - the engine answers "not this year" in a common year (R2-N7) - so
            // the bound has to let it through. April 31 is a day that exists in no year at all, so
            // it is a typo, and the engine would answer "not this year" for EVERY year: a holiday
            // silently absent forever. Refused here, naming the month's real length, because a
            // calendar is a data file from outside the build.
            let longest = chrono_core::max_day_in_month(month);
            if !(1..=longest).contains(&day) {
                return Err(format!(
                    "holiday '{id}': day {day} out of range for month {month} (1..={longest})"
                ));
            }

            HolidayRule::Fixed { month, day }
        }
        RuleDto::NthWeekday { month, weekday, order } => {
            check_month(id, month)?;
            // -1 = "the last such weekday in the month" - 1..=5 counts from the start. A fifth exists
            // only in some months, and the engine now answers "this holiday does not fall in that
            // year" rather than borrowing a day from the next month - which is what it actually did
            // while this comment claimed otherwise (R2-N4). Anything outside the range would walk
            // past the end for every month, so it stays a load error.
            if order != -1 && !(1..=5).contains(&order) {
                return Err(format!(
                    "holiday '{id}': order {order} out of range (-1 for last, or 1..=5)"
                ));
            }

            HolidayRule::NthWeekday { month, weekday: weekday_index(&weekday)?, order }
        }
        RuleDto::EasterOffset { offset } => {
            // Kept inside the year of its own Easter (R4-N39, the owner's decision R4-D12). Easter
            // falls between 22 March, day 81 of a common year, and 25 April, day 115 (116 in a leap
            // year), so -80 is 1 January at the earliest and +250 is 31 December at the latest, in
            // every year. The bound used to be a year either side, and a holiday pushed into the
            // neighbouring year was a day the engine described two ways at once: `holiday_on` looks
            // for it in the date's own year and finds nothing, while the day-off cache looks a year
            // either side and finds it, so a report called one day "not a holiday" and "a holiday
            // shifted here from a weekend". Real observances sit well inside (Corpus Christi is +60).
            if !EASTER_OFFSET_RANGE.contains(&offset) {
                return Err(format!(
                    "holiday '{id}': easter offset {offset} out of range ({}..={} days, which keeps the holiday in the year of its Easter)",
                    EASTER_OFFSET_RANGE.start(),
                    EASTER_OFFSET_RANGE.end()
                ));
            }

            HolidayRule::EasterOffset { offset }
        }
    })
}

/// The Easter offsets a calendar may name - see where `rule_from` checks it.
const EASTER_OFFSET_RANGE: std::ops::RangeInclusive<i32> = -80..=250;

pub(crate) fn check_month(id: &str, month: u32) -> Result<(), String> {
    if !(1..=12).contains(&month) {
        return Err(format!("holiday '{id}': month {month} out of range (1..=12)"));
    }

    Ok(())
}

/// Locate a calendar file: next to the executable (portable layout), else in ./calendars.
/// Whether a catalogue id (calendar / preset) is safe to turn into a file name: non-empty and made
/// only of letters, digits, '-' and '_'. This rejects any path separator, '..', drive letter, or
/// ADS colon BEFORE the id becomes a path, so `--preset ../../secret` cannot read a file outside the
/// catalogue directory (docs/04 4.1 - a shared catalogue entry can never smuggle a path).
pub(crate) fn is_valid_catalogue_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Locate `<kind>/<id>.json` - the shared lookup behind calendars and presets.
///
/// Next to the executable first, then the working directory. The second is what makes a dev
/// checkout work: `chrono.exe` is built into `target/<triple>/release/`, which has no `calendars/`
/// beside it, so every `cargo run -- calc --calendar` and all 133 harness scenarios resolve through
/// the working directory. It cannot simply be dropped.
///
/// What it must NOT do is rescue an INSTALLED layout. If the folder beside the executable exists but
/// does not hold the file, the answer is "missing", not "here is one from wherever you happened to
/// be standing" - otherwise `chrono calc --calendar us-banking`, run from a directory someone else
/// can write to, silently answers business-day questions from THEIR holidays, in a report a tester
/// then quotes as evidence (untouchable rule 4). Directories are taken as given so all three cases
/// are testable without touching the process's real working directory.
pub(crate) fn find_catalogue_in(exe_dir: Option<&std::path::Path>, cwd: &std::path::Path, kind: &str, id: &str) -> Option<std::path::PathBuf> {
    let name = format!("{id}.json");
    if let Some(dir) = exe_dir {
        let installed = dir.join(kind);
        let candidate = installed.join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if installed.is_dir() {
            return None; // an installed catalogue answers for itself, including "not here"
        }
    }
    let local = cwd.join(kind).join(&name);
    local.is_file().then_some(local)
}

/// The `<kind>` directory a lookup by id would read from, by the same rule as [`find_catalogue_in`]:
/// the one beside the executable when it exists, which then answers for itself, else the working
/// directory's. `None` when neither exists. The preset catalogue lists this directory, so a file it
/// lists is one `--preset` finds.
pub(crate) fn find_catalogue_dir_in(exe_dir: Option<&std::path::Path>, cwd: &std::path::Path, kind: &str) -> Option<std::path::PathBuf> {
    if let Some(installed) = exe_dir.map(|dir| dir.join(kind)).filter(|dir| dir.is_dir()) {
        return Some(installed);
    }
    let local = cwd.join(kind);
    local.is_dir().then_some(local)
}

/// [`find_catalogue_dir_in`] against this process's real executable and working directory.
pub(crate) fn find_catalogue_dir(kind: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_ref().and_then(|e| e.parent());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    find_catalogue_dir_in(exe_dir, &cwd, kind)
}

/// [`find_catalogue_in`] against this process's real executable and working directory.
pub(crate) fn find_catalogue_file(kind: &str, id: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_ref().and_then(|e| e.parent());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    find_catalogue_in(exe_dir, &cwd, kind, id)
}

/// Where the lookup actually looked, for the "not found" message. The two layouts differ, and naming
/// `./<kind>` for an installed one that never consulted it would send the reader to fix the wrong
/// folder - in the single message they have to act on (rule 6).
pub(crate) fn catalogue_search_places(kind: &str) -> String {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(kind)))
        .is_some_and(|d| d.is_dir());
    if installed {
        format!("looked in <exe>/{kind}")
    } else {
        format!("looked in <exe>/{kind} and ./{kind}")
    }
}

pub(crate) fn find_calendar_file(id: &str) -> Result<std::path::PathBuf, String> {
    if !is_valid_catalogue_id(id) {
        return Err(format!("invalid calendar id '{id}' (use letters, digits, '-' or '_')"));
    }
    find_catalogue_file("calendars", id)
        .ok_or_else(|| format!("calendar '{id}' not found ({})", catalogue_search_places("calendars")))
}

/// Load and validate a calendar by id, mapping the JSON schema to the core engine's types.
pub(crate) fn load_calendar(id: &str) -> Result<chrono_core::calendar::Calendar, String> {
    let path = find_calendar_file(id)?;
    let text = read_catalogue_file(&path)?;
    let calendar = calendar_from_text(&text).map_err(|e| format!("{e} (in {})", path.display()))?;
    check_declared_id("calendar", &path, &calendar.id, id)?;
    Ok(calendar)
}

/// Refuse a catalogue file whose `id` is not the name it was found by (R4-N37, the owner's decision
/// R4-D14).
///
/// The lookup goes by file name and the report quotes the id inside, so `other.json` declaring `pl`
/// was loaded as `--calendar other` and signed its answers "(pl)" - a business-day result attributed
/// to a calendar nobody asked for. The window drops such a preset from its list the same way it drops
/// any file it cannot use, so both surfaces show one catalogue. Compared without case, because that
/// is how Windows matches the file name: `--calendar PL` finds `pl.json`, and refusing it as a mismatch
/// would name a file name that is not on disk.
pub(crate) fn check_declared_id(kind: &str, path: &std::path::Path, declared: &str, requested: &str) -> Result<(), String> {
    if declared.eq_ignore_ascii_case(requested) {
        return Ok(());
    }

    Err(format!(
        "{} declares the {kind} id '{declared}', but it is found as '{requested}' - a {kind}'s id has to match its file name",
        path.display()
    ))
}

/// The largest catalogue file (calendar or preset) this build will read.
///
/// Two hundred times the largest shipped one, and a bound rather than a judgement about what a file
/// should contain: `read_to_string` on a file with no ceiling is the one unbounded input left in this
/// tool, and calendars are the one catalogue outsiders are invited to write. A generated file, or a
/// path that turns out to point at something that is not a catalogue at all, should be refused by
/// name rather than pulled into memory whole.
pub(crate) const MAX_CATALOGUE_BYTES: u64 = 4 * 1024 * 1024;

/// The largest number of holidays a calendar may declare - see the note where it is enforced. The
/// shipped calendars hold about fourteen, so this is two orders of magnitude of room.
pub(crate) const MAX_HOLIDAYS: usize = 2000;

/// Read a catalogue file, refusing one too large to be a catalogue.
///
/// The size is checked before the read, not after, which is the whole point - a file that cannot be a
/// catalogue never has to fit in memory to be rejected.
pub(crate) fn read_catalogue_file(path: &std::path::Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    if size > MAX_CATALOGUE_BYTES {
        return Err(format!(
            "{} is {size} bytes, past the {MAX_CATALOGUE_BYTES}-byte limit for a catalogue file",
            path.display()
        ));
    }

    let mut bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // A UTF-8 byte order mark is skipped (R4-N37). Windows PowerShell 5.1 writes one for
    // `-Encoding UTF8`, and JSON parsers may ignore it (RFC 8259 section 8.1). serde does not, and read
    // it as a stray character before the first brace: "expected value at line 1 column 1", of a file
    // that looks perfect in any editor. The window's reader already skipped it, so the same preset
    // worked there and was refused here.
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }

    String::from_utf8(bytes).map_err(|e| format!("cannot read {}: it is not UTF-8 text ({e})", path.display()))
}

/// Parse and validate a calendar from its JSON text, mapping the `chronomock.calendar/1` schema to the
/// engine's types. Separated from the on-disk lookup (symmetry with `parse_preset`) so the shipped
/// calendars can be golden-tested against the real engine without the file-resolution step.
pub(crate) fn calendar_from_text(text: &str) -> Result<chrono_core::calendar::Calendar, String> {
    let schema: SchemaDto = serde_json::from_str(text).map_err(|e| format!("bad calendar JSON: {e}"))?;
    // An unknown major schema version is refused, not half-understood (docs/04 section 3.1).
    if schema.schema != "chronomock.calendar/1" {
        return Err(format!(
            "unsupported calendar schema '{}' (this build reads chronomock.calendar/1)",
            schema.schema
        ));
    }
    let dto: CalendarDto = serde_json::from_str(text).map_err(|e| format!("bad calendar JSON: {e}"))?;
    let mut weekend = dto.weekend.iter().map(|w| weekday_index(w)).collect::<Result<Vec<_>, _>>()?;
    // Duplicates are harmless to the engine (membership is a contains) but they hide a typo, and
    // they make the count below meaningless - so fold them away before counting.
    weekend.sort_unstable();
    weekend.dedup();
    // A holiday list has to stay a list of holidays. The cost of deciding "is this a business day" is
    // the number of holidays in force times three years, and a business-day WALK pays it again every
    // time it crosses a year - so the two multiply. Measured on this machine: 20 000 holidays and a
    // +200000bd walk took 4.6 s in release and 1 min 55 s in debug, against 0.47 s for the same walk
    // on a shipped calendar. The GUI's calc timeout would cut that off with a message, and the CLI has
    // no such ceiling. A number no real calendar approaches (the shipped ones hold about fourteen) is
    // cheap to refuse here, where the author can see it.
    if dto.holidays.len() > MAX_HOLIDAYS {
        return Err(format!(
            "calendar lists {} holidays, past the limit of {MAX_HOLIDAYS} this build reads",
            dto.holidays.len()
        ));
    }
    // A week with no working day leaves "+1 business day" with nothing to land on. The engine now
    // bounds its walk instead of hanging (S-1), but a file this broken should never reach it: say
    // which field is wrong, here, where the author can fix it.
    if weekend.len() >= 7 {
        return Err(
            "calendar 'weekend' lists all seven days - no business day would ever exist".to_string()
        );
    }
    // `observed` and `weekend` have to describe the SAME weekend. The engine's observance rules match
    // Saturday and Sunday by name - their variant names say so (`sun_to_mon`) - while `weekend` is a
    // free list of days. A file combining a Friday-Saturday weekend with a shift rule therefore
    // loaded cleanly and then did two wrong things quietly: a Friday holiday stayed put (the rule
    // never saw it) and a Sunday holiday moved to Monday although Sunday is a working day there. Both
    // come out as a wrong payment date with no message anywhere (R2-N8).
    //
    // Refused here rather than generalised in the engine. Generalising means new variant names - the
    // existing ones are ABOUT Saturday and Sunday, and reusing `weekend_to_mon` for a weekend that
    // ends on Saturday would produce a Sunday while the name promises Monday, which misleads worse
    // than the missing feature does. That wants a real market to define it, not a guess. A calendar
    // with a different weekend can still ship today with `"observed": "none"`.
    let observed = observed_from(&dto.observed)?;
    if observed != chrono_core::calendar::Observed::None && weekend != [0, 6] {
        return Err(format!(
            "calendar 'observed' is '{}', which is defined for a Saturday-Sunday weekend, but 'weekend' is {:?} - use \"observed\": \"none\" until an observance rule exists for that weekend",
            dto.observed, dto.weekend
        ));
    }
    let law_as_of = law_as_of_from(&dto.law_as_of)?;
    // The range has to be one this engine can keep and one the check could have looked at: a first year
    // after the day the list was checked would claim rules nobody has read yet.
    if dto.valid_from < FIRST_GREGORIAN_YEAR || dto.valid_from > law_as_of.year {
        return Err(format!(
            "calendar 'valid_from' is {}, outside {FIRST_GREGORIAN_YEAR}..={} (the Gregorian calendar up to the year of 'law_as_of')",
            dto.valid_from, law_as_of.year
        ));
    }

    let mut seen_ids: Vec<String> = Vec::new();
    let holidays = dto
        .holidays
        .into_iter()
        .map(|h| {
            // A duplicate id makes the audit ambiguous - `holiday_on` names one of them and the reader
            // cannot tell which. Cheap to catch, impossible to diagnose later.
            if seen_ids.contains(&h.id) {
                return Err(format!("duplicate holiday id '{}'", h.id));
            }

            // An inverted window silently means "never a holiday", which reads as a missing entry
            // rather than as the mistake it is.
            if let (Some(from), Some(to)) = (h.valid_from, h.valid_to)
                && from > to {
                    return Err(format!(
                        "holiday '{}': valid_from {from} is after valid_to {to}",
                        h.id
                    ));
                }
            // A holiday that ended before the calendar's first year can never apply - a typo in a year,
            // or a range that was meant to reach further back and does not.
            if let Some(to) = h.valid_to
                && to < dto.valid_from {
                    return Err(format!(
                        "holiday '{}': valid_to {to} is before the calendar's valid_from {}, so it never applies",
                        h.id, dto.valid_from
                    ));
                }

            seen_ids.push(h.id.clone());
            let rule = rule_from(&h.id, h.rule)?;
            Ok(chrono_core::calendar::Holiday {
                id: h.id,
                name_en: h.name.en,
                name_local: h.name.local,
                rule,
                valid_from: h.valid_from,
                valid_to: h.valid_to,
                source: h.source,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(chrono_core::calendar::Calendar {
        id: dto.id,
        country: dto.country,
        weekend,
        observed,
        valid_from: Some(dto.valid_from),
        law_as_of: Some(law_as_of),
        holidays,
    })
}

/// The calendar's `law_as_of`, a plain `YYYY-MM-DD` that names a real day, read through the same
/// civil-date parser as every other date here.
fn law_as_of_from(text: &str) -> Result<chrono_core::calc::CivilDateTime, String> {
    let refuse = || format!("calendar 'law_as_of' is '{text}', not a date written YYYY-MM-DD");
    if text.len() != 10 {
        return Err(refuse());
    }
    chrono_core::calc::parse_civil_datetime(&format!("{text}T00:00:00")).map_err(|_| refuse())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::read_data;

    /// S-1, first line. Calendars are the one catalogue outsiders are invited to write, so a file
    /// that makes "next business day" unanswerable has to be refused where its author can see it -
    /// not walked into by the engine. Duplicated weekend days are folded first, so the check counts
    /// distinct days and a repeated "saturday" is not mistaken for a full week.
    /// A holiday list has to stay a list of holidays.
    ///
    /// The cost of "is this a business day" is the number of holidays in force times three years, and a
    /// business-day walk pays it again on every year boundary it crosses - so the two multiply.
    /// Measured on this machine: 20 000 holidays and `+200000bd` took 4.6 s in release and 1 min 55 s
    /// in debug, against 0.47 s for the same walk on a shipped calendar. The GUI's calc timeout cuts
    /// that off with a message and the CLI has no such ceiling, so the refusal belongs here, where the
    /// file's author can see it. The shipped calendars hold about fourteen.
    #[test]
    fn a_calendar_with_an_implausible_number_of_holidays_is_refused() {
        let holiday = |i: usize| {
            format!(
                r#"{{"id":"h{i}","name":{{"en":"H","local":"H"}},"rule":{{"type":"easter_offset","offset":1}},"source":"probe"}}"#
            )
        };
        let with_holidays = |n: usize| {
            let list: Vec<String> = (0..n).map(holiday).collect();
            format!(
                r#"{{"schema":"chronomock.calendar/1","id":"probe","country":"XX","weekend":["saturday","sunday"],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[{}]}}"#,
                list.join(",")
            )
        };

        let err = calendar_from_text(&with_holidays(MAX_HOLIDAYS + 1))
            .expect_err("past the limit this build reads");
        assert!(err.contains("past the limit"), "the message names the limit, got: {err}");
        assert!(err.contains(&MAX_HOLIDAYS.to_string()), "and the number, got: {err}");

        // At the limit is fine - the bound is a ceiling on the implausible, not on the large.
        assert!(calendar_from_text(&with_holidays(MAX_HOLIDAYS)).is_ok());
    }

    /// A catalogue file is outside input, and `read_to_string` on it was the last unbounded read in
    /// this tool. The size is checked BEFORE the read, so a file that cannot be a catalogue never has
    /// to fit in memory to be rejected.
    #[test]
    fn a_catalogue_file_past_the_size_limit_is_refused_without_being_read() {
        let dir = std::env::temp_dir().join(format!("chrono-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("huge.json");
        // One byte past the ceiling, written as zeros - never parsed, so the content is immaterial.
        std::fs::write(&path, vec![b'0'; MAX_CATALOGUE_BYTES as usize + 1]).expect("write");

        let err = read_catalogue_file(&path).expect_err("past the size limit");
        assert!(err.contains("past the"), "got: {err}");

        std::fs::write(&path, b"{}").expect("write");
        assert_eq!(read_catalogue_file(&path).expect("within the limit"), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-N8: `weekend` and `observed` must describe the same weekend. A Friday-Saturday weekend with
    /// a shift rule used to load and then be wrong twice over, silently - which is the one thing a
    /// calendar file must never do, since calendars are written by people outside this build.
    #[test]
    fn an_observance_rule_with_a_foreign_weekend_is_refused() {
        let cal = |weekend: &str, observed: &str| {
            format!(
                r#"{{"schema":"chronomock.calendar/1","id":"x","country":"XX","weekend":{weekend},
                "observed":"{observed}","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[]}}"#
            )
        };

        for observed in ["sat_to_fri_sun_to_mon", "sun_to_mon", "weekend_to_mon"] {
            let err = calendar_from_text(&cal(r#"["friday","saturday"]"#, observed))
                .expect_err("a foreign weekend with an observance rule must be refused");
            assert!(err.contains("observed"), "the message must name the field: {err}");
            assert!(err.contains(observed), "the message must name the rule: {err}");
            assert!(err.contains("none"), "the message must say what to do instead: {err}");
        }

        // The same weekend with no observance rule is fine - this refuses a COMBINATION, and does not
        // ban weekends the project has not shipped a calendar for.
        calendar_from_text(&cal(r#"["friday","saturday"]"#, "none")).expect("a foreign weekend alone loads");
        // And the Saturday-Sunday weekend keeps every rule, in any listed order.
        for observed in ["sat_to_fri_sun_to_mon", "sun_to_mon", "weekend_to_mon", "none"] {
            calendar_from_text(&cal(r#"["sunday","saturday"]"#, observed))
                .unwrap_or_else(|e| panic!("{observed} on a Sat-Sun weekend must load: {e}"));
        }
    }

    #[test]
    fn calendar_with_every_day_as_weekend_is_refused() {
        let all_week = r#"{"schema":"chronomock.calendar/1","id":"x","country":"XX",
            "weekend":["monday","tuesday","wednesday","thursday","friday","saturday","sunday"],
            "observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[]}"#;
        let err = calendar_from_text(all_week).expect_err("a week with no working day is refused");
        assert!(err.contains("weekend"), "the message must name the field: {err}");

        // Duplicates alone are not an error - they fold, and the calendar still works.
        let dupes = r#"{"schema":"chronomock.calendar/1","id":"x","country":"XX",
            "weekend":["saturday","saturday","sunday"],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[]}"#;
        let cal = calendar_from_text(dupes).expect("duplicate weekend days fold");
        assert_eq!(cal.weekend.len(), 2);
    }

    /// S-17. Every rule field is refused out of range, naming the holiday and the field. Unchecked,
    /// none of these fail loudly - the civil-date maths rolls month 13 into January and day 40 into the
    /// next month, so the calendar quietly marks the WRONG dates and every business-day answer built on
    /// it is wrong with it. Calendars are the documented third-party extension point, so this is the
    /// place where a broken file has to stop.
    #[test]
    fn calendar_rules_out_of_range_are_refused_with_the_field_named() {
        let cal = |rule: &str| {
            format!(
                r#"{{"schema":"chronomock.calendar/1","id":"x","country":"XX","weekend":["saturday","sunday"],
                "observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[{{"id":"bad","name":{{"en":"B","local":"B"}},
                "rule":{rule},"source":"test"}}]}}"#
            )
        };

        for (rule, needle) in [
            (r#"{"type":"fixed","month":13,"day":1}"#, "month 13"),
            (r#"{"type":"fixed","month":1,"day":40}"#, "day 40"),
            // A day that exists in NO year is a typo, not a leap-year rule (R2-N7). The engine would
            // answer "not this year" for every year, so the holiday would be silently absent forever -
            // refused here instead, and the message names the month's real length.
            (r#"{"type":"fixed","month":4,"day":31}"#, "day 31"),
            (r#"{"type":"fixed","month":2,"day":30}"#, "day 30"),
            (r#"{"type":"nth_weekday","month":1,"weekday":"monday","order":9}"#, "order 9"),
            (r#"{"type":"nth_weekday","month":0,"weekday":"monday","order":1}"#, "month 0"),
            (r#"{"type":"easter_offset","offset":5000}"#, "5000"),
            // R4-N39: one day past either end already leaves the year of its Easter in some year.
            (r#"{"type":"easter_offset","offset":-81}"#, "-81"),
            (r#"{"type":"easter_offset","offset":251}"#, "251"),
        ] {
            let err = calendar_from_text(&cal(rule)).expect_err("out of range must be refused");
            assert!(err.contains("bad"), "the message must name the holiday: {err}");
            assert!(err.contains(needle), "the message must name the value: {err}");
        }

        // The legitimate neighbours of those bounds still parse. February 29 is the one that matters:
        // it is impossible in most years and legal in the schema, so the bound is the month's longest
        // length in any year, never the length of some particular year.
        for rule in [
            r#"{"type":"fixed","month":2,"day":29}"#,
            r#"{"type":"fixed","month":4,"day":30}"#,
            r#"{"type":"fixed","month":1,"day":31}"#,
            r#"{"type":"nth_weekday","month":12,"weekday":"monday","order":-1}"#,
            r#"{"type":"nth_weekday","month":5,"weekday":"monday","order":5}"#,
            r#"{"type":"easter_offset","offset":60}"#,
            r#"{"type":"easter_offset","offset":-80}"#,
            r#"{"type":"easter_offset","offset":250}"#,
        ] {
            calendar_from_text(&cal(rule)).unwrap_or_else(|e| panic!("{rule} should parse: {e}"));
        }
    }

    /// R4-N38, the owner's decision R4-D11: a field this reader does not know is refused, at every
    /// level of the file, and the message names it. `valid_form` for `valid_from` used to load, and the
    /// holiday meant to start in 2030 counted in every year.
    #[test]
    fn an_unknown_field_is_refused_at_every_level_and_named() {
        let cal = |root: &str, holiday: &str, name: &str, rule: &str| {
            format!(
                r#"{{"schema":"chronomock.calendar/1","id":"x","country":"XX","weekend":["saturday","sunday"],
                "observed":"none"{root},"valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[{{"id":"h","name":{{"en":"H","local":"H"{name}}},
                "rule":{{"type":"fixed","month":7,"day":1{rule}}},"source":"t"{holiday}}}]}}"#
            )
        };
        for (text, field) in [
            (cal(r#","obsreved":"none""#, "", "", ""), "obsreved"),
            (cal("", r#","valid_form":2030"#, "", ""), "valid_form"),
            (cal("", "", r#","pl":"H""#, ""), "pl"),
            (cal("", "", "", r#","dya":2"#), "dya"),
        ] {
            let err = calendar_from_text(&text).expect_err("an unknown field is refused");
            assert!(err.contains("unknown field"), "the message says what is wrong: {err}");
            assert!(err.contains(&format!("`{field}`")), "and names the field: {err}");
        }
        // The calendar's own title is checked too - a typo there is as much a typo as anywhere else.
        let titled = cal(r#","name":{"en":"X","local":"X","loacl":"X"}"#, "", "", "");
        assert!(calendar_from_text(&titled).expect_err("a typo in the title").contains("`loacl`"));

        // The contract's own fields are not unknown, even though this reader does not use them.
        let contract = cal(r#","stability":"unstable","subregion":null,"name":{"en":"X","local":"X"}"#, "", "", "");
        calendar_from_text(&contract).expect("the contract's fields load");
        calendar_from_text(&cal("", "", "", "")).expect("and a file without them loads as before");
    }

    /// A file written to a later schema is refused as that, not over its first new field. Read
    /// strictly in one pass, `chronomock.calendar/2` with an added field would be reported as a typo,
    /// and its author would go looking for one.
    #[test]
    fn a_later_schema_is_refused_as_a_schema_not_as_an_unknown_field() {
        let v2 = r#"{"schema":"chronomock.calendar/2","id":"x","country":"XX","weekend":[],
            "observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[],"range":[1990,2100]}"#;
        let err = calendar_from_text(v2).expect_err("a later schema");
        assert!(err.contains("unsupported calendar schema 'chronomock.calendar/2'"), "got: {err}");
        assert!(!err.contains("unknown field"), "got: {err}");
    }

    /// R4-N37. A catalogue file saved with a UTF-8 byte order mark - Windows PowerShell 5.1 does that
    /// for `-Encoding UTF8` - was refused with "expected value at line 1 column 1", while the window
    /// read the same file. A file that is not UTF-8 at all is still refused, and says so.
    #[test]
    fn a_byte_order_mark_is_skipped_and_other_bytes_are_still_refused() {
        let dir = crate::testutil::unique_temp_dir("chrono-bom");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("pl.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(read_data("calendars/pl.json").as_bytes());
        std::fs::write(&path, &bytes).expect("write");
        let text = read_catalogue_file(&path).expect("a file with a byte order mark reads");
        assert!(text.starts_with('{'), "the mark is gone: {:?}", text.chars().next());
        calendar_from_text(&text).expect("and the calendar in it loads");

        std::fs::write(&path, [0xFF, 0xFE, b'{', 0, b'}', 0]).expect("write");
        let err = read_catalogue_file(&path).expect_err("UTF-16 is not UTF-8");
        assert!(err.contains("not UTF-8"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R4-N37, the owner's decision R4-D14: a catalogue file's id is the name it is found by. The
    /// comparison ignores case, because the lookup does: `--calendar PL` finds `pl.json`.
    #[test]
    fn a_declared_id_other_than_the_file_name_is_refused() {
        let path = std::path::Path::new("calendars").join("other.json");
        let err = check_declared_id("calendar", &path, "pl", "other").expect_err("pl is not other");
        assert!(err.contains("'pl'") && err.contains("'other'"), "both ids are named: {err}");
        assert!(err.contains("other.json"), "and the file: {err}");
        check_declared_id("calendar", &path, "pl", "PL").expect("case is how Windows matches a file name");
    }

    /// Every calendar in `calendars/` loads the way `--calendar` loads it: read strictly and found by
    /// its own id. CONTRIBUTING.md tells an author that the suite parses and validates a new calendar,
    /// and until this test only the three files named in the golden test below were read - a fourth
    /// was checked by nothing (rule 12).
    #[test]
    fn every_shipped_calendar_loads_under_its_own_name() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../calendars");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).expect("the calendars folder") {
            let path = entry.expect("an entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).expect("a file name").to_string();
            let text = read_catalogue_file(&path).unwrap_or_else(|e| panic!("{stem}: {e}"));
            let calendar = calendar_from_text(&text).unwrap_or_else(|e| panic!("{stem}: {e}"));
            check_declared_id("calendar", &path, &calendar.id, &stem).unwrap_or_else(|e| panic!("{e}"));
            seen += 1;
        }
        assert!(seen >= 3, "the three shipped calendars at least, found {seen}");
    }

    /// S-17, the whole-file checks: a repeated id makes the audit ambiguous (holiday_on names one of
    /// them and the reader cannot tell which), and an inverted validity window means "never a holiday",
    /// which reads as a missing entry instead of the mistake it is.
    #[test]
    fn duplicate_holiday_ids_and_inverted_validity_windows_are_refused() {
        let dupes = r#"{"schema":"chronomock.calendar/1","id":"x","country":"XX",
            "weekend":["saturday","sunday"],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[
            {"id":"same","name":{"en":"A","local":"A"},"rule":{"type":"fixed","month":1,"day":1},"source":"t"},
            {"id":"same","name":{"en":"B","local":"B"},"rule":{"type":"fixed","month":2,"day":2},"source":"t"}]}"#;
        assert!(calendar_from_text(dupes).expect_err("duplicate id").contains("same"));

        let inverted = r#"{"schema":"chronomock.calendar/1","id":"x","country":"XX",
            "weekend":["saturday","sunday"],"observed":"none","valid_from":2000,"law_as_of":"2026-01-01","source":"t","holidays":[
            {"id":"w","name":{"en":"A","local":"A"},"rule":{"type":"fixed","month":1,"day":1},
             "valid_from":2030,"valid_to":2020,"source":"t"}]}"#;
        assert!(calendar_from_text(inverted).expect_err("inverted window").contains("valid_from"));
    }

    /// An installed layout answers for its own catalogue, "not here" included. Without that, running
    /// `chrono calc --calendar us-banking` from a directory someone else can write to answered
    /// business-day questions out of THEIR file whenever the shipped one was missing - different
    /// holidays, different answers, no warning, in output a tester quotes as evidence.
    ///
    /// The working-directory fallback itself has to stay: `chrono.exe` is built into
    /// `target/<triple>/release/`, which has no `calendars/` beside it, so a dev checkout and all
    /// 133 harness scenarios resolve that way.
    #[test]
    fn an_installed_catalogue_is_not_rescued_from_the_working_directory() {
        let root = std::env::temp_dir().join(format!("chrono-catalogue-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let exe_dir = root.join("app");
        let cwd = root.join("elsewhere");
        std::fs::create_dir_all(exe_dir.join("calendars")).unwrap();
        std::fs::create_dir_all(cwd.join("calendars")).unwrap();
        std::fs::write(cwd.join("calendars").join("us-banking.json"), "{}").unwrap();

        // Installed layout, file absent from it: the one in the working directory is NOT used.
        assert_eq!(find_catalogue_in(Some(&exe_dir), &cwd, "calendars", "us-banking"), None);

        // Installed layout that does have it: that copy wins.
        let shipped = exe_dir.join("calendars").join("us-banking.json");
        std::fs::write(&shipped, "{}").unwrap();
        assert_eq!(find_catalogue_in(Some(&exe_dir), &cwd, "calendars", "us-banking"), Some(shipped));

        // No catalogue beside the executable at all - a dev checkout. The fallback still works, and
        // this is the case the harness runs in.
        let bare = root.join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(
            find_catalogue_in(Some(&bare), &cwd, "calendars", "us-banking"),
            Some(cwd.join("calendars").join("us-banking.json"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The folder the preset catalogue lists is the folder a lookup by id reads, by the same rule - so
    /// `chrono presets` never lists a file that `--preset` would not find, and never leaves out one it
    /// would.
    #[test]
    fn the_catalogue_folder_is_the_one_a_lookup_by_id_reads() {
        let root = std::env::temp_dir().join(format!("chrono-catalogue-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let exe_dir = root.join("app");
        let cwd = root.join("elsewhere");
        let bare = root.join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(cwd.join("presets")).unwrap();
        std::fs::write(cwd.join("presets").join("month-end.json"), "{}").unwrap();

        // No folder beside the executable: the working directory's, the one a lookup falls back to.
        assert_eq!(find_catalogue_dir_in(Some(&bare), &cwd, "presets"), Some(cwd.join("presets")));
        assert_eq!(
            find_catalogue_in(Some(&bare), &cwd, "presets", "month-end"),
            Some(cwd.join("presets").join("month-end.json"))
        );

        // An installed folder, even an empty one, answers for itself - the lookup does not fall back.
        std::fs::create_dir_all(exe_dir.join("presets")).unwrap();
        assert_eq!(find_catalogue_dir_in(Some(&exe_dir), &cwd, "presets"), Some(exe_dir.join("presets")));
        assert_eq!(find_catalogue_in(Some(&exe_dir), &cwd, "presets", "month-end"), None);

        // Neither: no folder at all, not an empty catalogue.
        assert_eq!(find_catalogue_dir_in(Some(&bare), &bare, "presets"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shipped_calendars_parse_and_hit_golden_dates() {
        use chrono_core::calc::CivilDateTime;
        use chrono_core::calendar::{holiday_on, is_business_day};
        let d = |y: i64, m: u32, day: u32| CivilDateTime {
            year: y,
            month: m,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        };

        let pl = calendar_from_text(&read_data("calendars/pl.json")).expect("pl parses");
        // Easter Monday 2026 = 2026-04-06 (Easter Sunday 2026-04-05): the easter_offset computus.
        assert_eq!(holiday_on(&d(2026, 4, 6), &pl).map(|h| h.id.as_str()), Some("easter_monday"));
        // Corpus Christi 2026 = Easter + 60 days = 2026-06-04: a larger easter_offset.
        assert_eq!(holiday_on(&d(2026, 6, 4), &pl).map(|h| h.id.as_str()), Some("corpus_christi"));
        // Epiphany was restored as a Polish public holiday from 2011 (valid_from:2011 - law of 24 Sept
        // 2010, abolished 1960): a holiday in 2026, NOT in 2010.
        assert_eq!(holiday_on(&d(2026, 1, 6), &pl).map(|h| h.id.as_str()), Some("epiphany"));
        assert!(holiday_on(&d(2010, 1, 6), &pl).is_none());
        // Christmas Eve became a non-working day from 2025 (valid_from:2025): a holiday in 2025, not 2024.
        assert_eq!(holiday_on(&d(2025, 12, 24), &pl).map(|h| h.id.as_str()), Some("christmas_eve"));
        assert!(holiday_on(&d(2024, 12, 24), &pl).is_none());

        let fed = calendar_from_text(&read_data("calendars/us-federal.json")).expect("us-federal parses");
        let bank = calendar_from_text(&read_data("calendars/us-banking.json")).expect("us-banking parses");
        // One country, two calendars: Independence Day 2026 falls on Saturday (Jul 4), so it is observed
        // on Friday Jul 3 federally (sat->fri) - not a business day - while banking (sun->mon only) does
        // not shift a Saturday holiday, so the same Friday IS a business day.
        assert!(!is_business_day(&d(2026, 7, 3), &fed));
        assert!(is_business_day(&d(2026, 7, 3), &bank));
        // Juneteenth is a federal holiday from 2021 (valid_from:2021): a holiday in 2021, not in 2020.
        assert_eq!(holiday_on(&d(2021, 6, 19), &fed).map(|h| h.id.as_str()), Some("juneteenth"));
        assert!(holiday_on(&d(2020, 6, 19), &fed).is_none());
        // MLK Day 2026 = the third Monday of January = 2026-01-19: an nth_weekday rule.
        assert_eq!(holiday_on(&d(2026, 1, 19), &fed).map(|h| h.id.as_str()), Some("mlk_day"));
        // And it is a holiday from 1986 (valid_from:1986). Public Law 98-144 was signed in 1983 with
        // effect from the first 1 January falling after a two-year period, so the third Monday of
        // January 1986 (the 20th) is the first observance and the third Monday of January 1985 (the
        // 21st) is an ordinary working day. Both calendars carry it, so both are asserted - the
        // entry read "always" until 2026-09-07, which made every date back to year 1 wrong.
        assert_eq!(holiday_on(&d(1986, 1, 20), &fed).map(|h| h.id.as_str()), Some("mlk_day"));
        assert!(holiday_on(&d(1985, 1, 21), &fed).is_none());
        // The banking calendar starts in 2008, so it carries the rule but judges neither year.
        assert!(!bank.judges_year(1986));
        // Banking observes a Sunday holiday on the Monday: New Year 2023-01-01 (Sun) -> Mon 2023-01-02.
        assert!(!is_business_day(&d(2023, 1, 2), &bank));
    }

    /// Four United States holidays have a history rather than one rule, and until 2026-09-07 both
    /// calendars answered with today's rule for every year back to year one. That put Washington's
    /// Birthday on the wrong February day for every date before 1971, invented a Columbus Day that
    /// did not exist, and missed the seven years Veterans Day spent in October.
    ///
    /// Public Law 90-363 (the Uniform Monday Holiday Act) took effect on 1971-01-01, and Public Law
    /// 94-97 returned Veterans Day to 11 November from 1978-01-01. Since R4-S20 only the federal
    /// calendar reaches back that far (from 1960) - the banking one starts in 2008, the oldest Federal
    /// Reserve schedule read, and judges none of these years.
    #[test]
    fn us_calendars_follow_the_uniform_monday_holiday_act() {
        use chrono_core::calc::CivilDateTime;
        use chrono_core::calendar::holiday_on;
        let d = |y: i64, m: u32, day: u32| CivilDateTime {
            year: y,
            month: m,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        };

        let fed = calendar_from_text(&read_data("calendars/us-federal.json")).expect("us-federal parses");
        let bank = calendar_from_text(&read_data("calendars/us-banking.json")).expect("us-banking parses");
        assert!(fed.judges_year(1960) && !fed.judges_year(1959), "us-federal answers from 1960");
        assert!(bank.judges_year(2008) && !bank.judges_year(2007), "us-banking answers from 2008");

        {
            let (which, cal) = ("us-federal", &fed);
            let id_on = |date: CivilDateTime| {
                holiday_on(&date, cal).map(|h| h.id.clone()).unwrap_or_default()
            };

            // Washington's Birthday: 22 February through 1970, the third Monday from 1971. In 1971
            // the 22nd was itself a Monday, which is why it is asserted - a rule that had merely
            // slipped a week would still pass on the 15th alone.
            assert_eq!(id_on(d(1970, 2, 22)), "washingtons_birthday_pre_1971", "{which}");
            assert_eq!(id_on(d(1970, 2, 16)), "", "{which}");
            assert_eq!(id_on(d(1971, 2, 15)), "washingtons_birthday", "{which}");
            assert_eq!(id_on(d(1971, 2, 22)), "", "{which}");

            // Memorial Day: 30 May through 1970, the last Monday from 1971.
            assert_eq!(id_on(d(1970, 5, 30)), "memorial_day_pre_1971", "{which}");
            assert_eq!(id_on(d(1971, 5, 31)), "memorial_day", "{which}");

            // Columbus Day did not exist as a federal holiday before the Act created it.
            assert_eq!(id_on(d(1970, 10, 12)), "", "{which}");
            assert_eq!(id_on(d(1971, 10, 11)), "columbus_day", "{which}");

            // Veterans Day has three periods, and the middle one is the whole reason this test is
            // worth writing: for seven years 11 November was an ordinary working day.
            assert_eq!(id_on(d(1965, 11, 11)), "veterans_day_pre_1971", "{which}");
            assert_eq!(id_on(d(1975, 11, 11)), "", "{which}");
            assert_eq!(id_on(d(1975, 10, 27)), "veterans_day_october_monday", "{which}");
            assert_eq!(id_on(d(1978, 11, 11)), "veterans_day", "{which}");
            assert_eq!(id_on(d(2026, 11, 11)), "veterans_day", "{which}");

            // Both of its boundaries, on the exact years. The assertions above sit years away from
            // the switch, so a window off by one would pass every one of them - and this holiday has
            // TWO switches, which is twice the chance of that. 1970 is the last November before the
            // Act and 1971 the first October Monday under it, then 1977 is the last October Monday
            // and 1978 the return to 11 November.
            assert_eq!(id_on(d(1970, 11, 11)), "veterans_day_pre_1971", "{which}");
            assert_eq!(id_on(d(1971, 11, 11)), "", "{which}");
            assert_eq!(id_on(d(1971, 10, 25)), "veterans_day_october_monday", "{which}");
            assert_eq!(id_on(d(1977, 10, 24)), "veterans_day_october_monday", "{which}");
            assert_eq!(id_on(d(1977, 11, 11)), "", "{which}");
            assert_eq!(id_on(d(1978, 10, 23)), "", "{which}");
        }
    }

    /// Every weekday a calendar closes in a year, the way a holiday table lists them - weekends left out,
    /// because a table does not list them either.
    fn weekdays_off(cal: &chrono_core::calendar::Calendar, year: i64) -> Vec<(u32, u32)> {
        use chrono_core::calc::CivilDateTime;
        let mut out = Vec::new();
        for month in 1..=12u32 {
            for day in 1..=31u32 {
                let date = CivilDateTime { year, month, day, hour: 0, minute: 0, second: 0 };
                if chrono_core::calc::parse_civil_datetime(&date.to_iso()).is_err() {
                    continue; // 30 February and its kind
                }
                let weekend = chrono_core::calc::metadata(&date, &date).weekday == "Saturday"
                    || chrono_core::calc::metadata(&date, &date).weekday == "Sunday";
                if !weekend && !chrono_core::calendar::is_business_day(&date, cal) {
                    out.push((month, day));
                }
            }
        }
        out
    }

    /// R4-S20: the banking calendar against its source - the Federal Reserve's K.8 table for 2008-2012,
    /// the oldest schedule read and the first year the calendar answers for. Every weekday the Banks
    /// closed, and no other: 4 July 2009, 25 December 2010 and 1 January 2011 fell on a Saturday and
    /// moved nowhere, while a Sunday holiday closed the Monday (5 July 2010, 26 December 2011, 2 January
    /// and 12 November 2012).
    #[test]
    fn us_banking_matches_the_federal_reserve_table_from_its_first_year() {
        let bank = calendar_from_text(&read_data("calendars/us-banking.json")).expect("us-banking parses");
        let table: [(i64, &[(u32, u32)]); 5] = [
            (2008, &[(1, 1), (1, 21), (2, 18), (5, 26), (7, 4), (9, 1), (10, 13), (11, 11), (11, 27), (12, 25)]),
            (2009, &[(1, 1), (1, 19), (2, 16), (5, 25), (9, 7), (10, 12), (11, 11), (11, 26), (12, 25)]),
            (2010, &[(1, 1), (1, 18), (2, 15), (5, 31), (7, 5), (9, 6), (10, 11), (11, 11), (11, 25)]),
            (2011, &[(1, 17), (2, 21), (5, 30), (7, 4), (9, 5), (10, 10), (11, 11), (11, 24), (12, 26)]),
            (2012, &[(1, 2), (1, 16), (2, 20), (5, 28), (7, 4), (9, 3), (10, 8), (11, 12), (11, 22), (12, 25)]),
        ];
        for (year, closed) in table {
            assert_eq!(weekdays_off(&bank, year), closed.to_vec(), "{year}");
        }
    }

    /// R4-S20 and Z1 on the Polish calendar: it answers from 2002, the first full year of the five-day
    /// week in the Labour Code, and it knows 12 November 2018, a day off by its own act (Dz.U. 2018 poz.
    /// 2117) that the file used to count as a working day. The 2018 list is every weekday off that year.
    #[test]
    fn pl_answers_from_2002_and_knows_the_centenary_day_off() {
        let pl = calendar_from_text(&read_data("calendars/pl.json")).expect("pl parses");
        assert!(pl.judges_year(2002) && !pl.judges_year(2001));
        assert_eq!(
            weekdays_off(&pl, 2018),
            vec![(1, 1), (4, 2), (5, 1), (5, 3), (5, 31), (8, 15), (11, 1), (11, 12), (12, 25), (12, 26)]
        );
        // One day, one year: 12 November 2019 is a Tuesday like any other.
        assert!(!weekdays_off(&pl, 2019).contains(&(11, 12)));
    }

    /// R4-S20: the three fields that say which years a calendar answers for are required, and each is
    /// refused when it cannot be true - named, so the author of a calendar knows what to fix.
    #[test]
    fn a_calendar_says_which_years_it_answers_for() {
        let cal = |root: &str, holiday: &str| {
            format!(
                r#"{{"schema":"chronomock.calendar/1","id":"x","country":"XX","weekend":["saturday","sunday"],
                "observed":"none"{root},"holidays":[{{"id":"h","name":{{"en":"H","local":"H"}},
                "rule":{{"type":"fixed","month":7,"day":1}},"source":"t"{holiday}}}]}}"#
            )
        };
        let full = r#","valid_from":2002,"law_as_of":"2026-10-06","source":"s""#;
        let loaded = calendar_from_text(&cal(full, "")).expect("a calendar with all three loads");
        assert_eq!(loaded.valid_from, Some(2002));
        assert_eq!(loaded.law_as_of.map(|d| (d.year, d.month, d.day)), Some((2026, 10, 6)));

        for (root, field) in [
            (r#","law_as_of":"2026-10-06","source":"s""#, "valid_from"),
            (r#","valid_from":2002,"source":"s""#, "law_as_of"),
            (r#","valid_from":2002,"law_as_of":"2026-10-06""#, "source"),
        ] {
            let err = calendar_from_text(&cal(root, "")).expect_err("a required field is missing");
            assert!(err.contains(&format!("`{field}`")), "names {field}: {err}");
        }
        for (root, needle) in [
            (r#","valid_from":1582,"law_as_of":"2026-10-06","source":"s""#, "1582"),
            (r#","valid_from":2027,"law_as_of":"2026-10-06","source":"s""#, "2027"),
            (r#","valid_from":2002,"law_as_of":"2026-02-30","source":"s""#, "2026-02-30"),
            (r#","valid_from":2002,"law_as_of":"6 October 2026","source":"s""#, "6 October 2026"),
            (r#","valid_from":2002,"law_as_of":"2026-10-06T00:00:00","source":"s""#, "YYYY-MM-DD"),
            // The civil parser reads a year with a leading zero, so the length is what keeps it out.
            (r#","valid_from":2002,"law_as_of":"02026-10-06","source":"s""#, "02026-10-06"),
        ] {
            let err = calendar_from_text(&cal(root, "")).expect_err("an impossible range or date");
            assert!(err.contains(needle), "names the value: {err}");
        }
        // A holiday that ended before the calendar's first year can never apply.
        let err = calendar_from_text(&cal(full, r#","valid_to":2001"#)).expect_err("ends before the range");
        assert!(err.contains("valid_to 2001") && err.contains("valid_from 2002"), "got: {err}");
        calendar_from_text(&cal(full, r#","valid_to":2002"#)).expect("one that ends inside it loads");
    }

    #[test]
    fn catalogue_id_rejects_path_traversal() {
        // A catalogue id must never become a path escape: separators, '..', a drive colon, or empty
        // are refused before the id is turned into a file name (docs/04 4.1).
        assert!(is_valid_catalogue_id("month-end"));
        assert!(is_valid_catalogue_id("us_banking"));
        assert!(is_valid_catalogue_id("year-2038"));
        assert!(!is_valid_catalogue_id(""));
        assert!(!is_valid_catalogue_id(".."));
        assert!(!is_valid_catalogue_id("../secret"));
        assert!(!is_valid_catalogue_id("a/b"));
        assert!(!is_valid_catalogue_id("a\\b"));
        assert!(!is_valid_catalogue_id("c:evil"));
    }

    #[test]
    fn calendar_loader_maps_weekdays_and_observed() {
        assert_eq!(weekday_index("Monday").unwrap(), 1);
        assert_eq!(weekday_index("sunday").unwrap(), 0);
        assert!(weekday_index("funday").is_err());
        assert!(matches!(observed_from("sun_to_mon").unwrap(), chrono_core::calendar::Observed::SunToMon));
        assert!(observed_from("whenever").is_err());
    }
}
