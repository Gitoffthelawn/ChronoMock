//! The step grammar: one parser for `--shift`, `--snap`, `--nearest`, `--set-time` and `--base`.
//!
//! Shared on purpose between the calculator flags and the preset reader, and the code said so long
//! before it had a file: "so there is one grammar, not two". Keeping it here is what makes that
//! sentence structural instead of a promise - the two callers cannot drift apart without meeting
//! here first.

use chrono_core::calc::{Base, NearestTarget, Sign, SnapTarget, Step, Unit};

/// Parse a `--base` value: the keywords `today`/`now`, or an absolute civil date-time.
pub(crate) fn parse_base(raw: &str) -> Result<Base, String> {
    match raw {
        "today" => Ok(Base::Today),
        "now" => Ok(Base::Now),
        _ => Ok(Base::Absolute(chrono_core::calc::parse_civil_datetime(raw)?)),
    }
}

/// Parse a `--shift` value `±N<unit>` into a shift step. The sign is mandatory - the
/// unit accepts short codes and full names. Minute stays `m` - month is `mo`, never `m`.
pub(crate) fn parse_shift(raw: &str) -> Result<Step, String> {
    let sign = match raw.as_bytes().first() {
        Some(b'+') => Sign::Plus,
        Some(b'-') => Sign::Minus,
        _ => return Err(format!("shift must start with + or -, got '{raw}'")),
    };
    let rest = &raw[1..];
    let split = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let (num, unit_str) = rest.split_at(split);
    if num.is_empty() {
        return Err(format!("shift needs a number, got '{raw}'"));
    }
    let amount: i64 = num.parse().map_err(|_| format!("bad number in shift '{raw}'"))?;
    let unit = parse_unit(unit_str).ok_or_else(|| format!("unknown unit '{unit_str}' in shift '{raw}'"))?;
    Ok(Step::Shift { sign, amount, unit })
}

/// Map a unit token (short code or full name) to a canonical unit. 🔴 `m` is minutes,
/// `mo` is months - never conflate them (the substitution `--at` delta already uses `m`
/// for minutes, so calc keeps the same convention).
pub(crate) fn parse_unit(s: &str) -> Option<Unit> {
    Some(match s {
        "s" | "sec" | "secs" | "seconds" => Unit::Seconds,
        "m" | "min" | "mins" | "minutes" => Unit::Minutes,
        "h" | "hr" | "hrs" | "hours" => Unit::Hours,
        "d" | "day" | "days" => Unit::Days,
        "w" | "week" | "weeks" => Unit::Weeks,
        "mo" | "month" | "months" => Unit::Months,
        "q" | "quarter" | "quarters" => Unit::Quarters,
        "y" | "yr" | "yrs" | "year" | "years" => Unit::Years,
        "bd" | "business_days" | "businessdays" => Unit::BusinessDays,
        _ => return None,
    })
}

/// The sign a shift is written with - the inverse of the first character [`parse_shift`] reads.
pub(crate) fn sign_code(sign: Sign) -> &'static str {
    match sign {
        Sign::Plus => "+",
        Sign::Minus => "-",
    }
}

/// The short code a unit is written with in this grammar - the inverse of [`parse_unit`], and the
/// token the window's step builder offers. The preset catalogue writes every unit this way, so the
/// window never has to know that `month`, `months` and `mo` are one unit (R4-S22). No catch-all arm:
/// a new unit cannot be added without its code.
pub(crate) fn unit_code(unit: Unit) -> &'static str {
    match unit {
        Unit::Seconds => "s",
        Unit::Minutes => "m",
        Unit::Hours => "h",
        Unit::Days => "d",
        Unit::Weeks => "w",
        Unit::Months => "mo",
        Unit::Quarters => "q",
        Unit::Years => "y",
        Unit::BusinessDays => "bd",
    }
}

/// The code a snap target is written with - the inverse of [`parse_snap`], for the same reason as
/// [`unit_code`].
pub(crate) fn snap_code(target: SnapTarget) -> &'static str {
    match target {
        SnapTarget::StartOfMonth => "som",
        SnapTarget::EndOfMonth => "eom",
        SnapTarget::StartOfQuarter => "soq",
        SnapTarget::EndOfQuarter => "eoq",
        SnapTarget::StartOfYear => "soy",
        SnapTarget::EndOfYear => "eoy",
    }
}

/// The code a nearest target is written with - the inverse of [`parse_nearest`]. The leap day keeps
/// its long form, which is the one the window's builder has always sent.
pub(crate) fn nearest_code(target: NearestTarget) -> &'static str {
    match target {
        NearestTarget::NextBusinessDay => "nbd",
        NearestTarget::PrevBusinessDay => "pbd",
        NearestTarget::NextLeapDay => "next-leap-day",
    }
}

/// Parse a `--snap` target token (full or short form) into a typed target.
pub(crate) fn parse_snap(raw: &str) -> Result<SnapTarget, String> {
    Ok(match raw {
        "start-of-month" | "som" => SnapTarget::StartOfMonth,
        "end-of-month" | "eom" => SnapTarget::EndOfMonth,
        "start-of-quarter" | "soq" => SnapTarget::StartOfQuarter,
        "end-of-quarter" | "eoq" => SnapTarget::EndOfQuarter,
        "start-of-year" | "soy" => SnapTarget::StartOfYear,
        "end-of-year" | "eoy" => SnapTarget::EndOfYear,
        other => {
            return Err(format!("unknown snap target '{other}' (use start-of/end-of month|quarter|year)"))
        }
    })
}

/// Parse a `--nearest` target token into a typed target.
pub(crate) fn parse_nearest(raw: &str) -> Result<NearestTarget, String> {
    Ok(match raw {
        "next-business-day" | "nbd" => NearestTarget::NextBusinessDay,
        "prev-business-day" | "previous-business-day" | "pbd" => NearestTarget::PrevBusinessDay,
        "next-leap-day" | "next-feb-29" | "nld" => NearestTarget::NextLeapDay,
        other => {
            return Err(format!(
                "unknown nearest target '{other}' (next-business-day, prev-business-day, next-leap-day)"
            ))
        }
    })
}

/// Parse a `--set-time` value `HH:MM:SS`. Field ranges are validated in the core
/// evaluator (BadSetTime), so parsing only checks the shape and numeric form here.
pub(crate) fn parse_set_time(raw: &str) -> Result<Step, String> {
    let p: Vec<&str> = raw.split(':').collect();
    if p.len() != 3 {
        return Err(format!("set-time must be HH:MM:SS, got '{raw}'"));
    }
    // Digits only, for the reason `chrono_core::is_ascii_digits` gives: the parse alone took `+23` as
    // an hour (R4-N34).
    if !p.iter().all(|f| chrono_core::is_ascii_digits(f)) {
        return Err(format!("set-time must be HH:MM:SS in digits, got '{raw}'"));
    }
    let hour = p[0].parse().map_err(|_| format!("bad hour in set-time '{raw}'"))?;
    let minute = p[1].parse().map_err(|_| format!("bad minute in set-time '{raw}'"))?;
    let second = p[2].parse().map_err(|_| format!("bad second in set-time '{raw}'"))?;
    Ok(Step::SetTime { hour, minute, second })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shift_reads_sign_amount_and_unit() {
        assert_eq!(parse_shift("+18years").unwrap(), Step::Shift { sign: Sign::Plus, amount: 18, unit: Unit::Years });
        assert_eq!(parse_shift("-1d").unwrap(), Step::Shift { sign: Sign::Minus, amount: 1, unit: Unit::Days });
        assert_eq!(parse_shift("+2q").unwrap(), Step::Shift { sign: Sign::Plus, amount: 2, unit: Unit::Quarters });
    }

    #[test]
    fn shift_unit_m_is_minutes_mo_is_months() {
        // The collision that would silently corrupt every month calc if conflated.
        assert_eq!(parse_shift("+5m").unwrap(), Step::Shift { sign: Sign::Plus, amount: 5, unit: Unit::Minutes });
        assert_eq!(parse_shift("+5mo").unwrap(), Step::Shift { sign: Sign::Plus, amount: 5, unit: Unit::Months });
    }

    #[test]
    fn parse_shift_rejects_bad_shapes() {
        assert!(parse_shift("18years").is_err()); // no sign
        assert!(parse_shift("+years").is_err()); // no number
        assert!(parse_shift("+18zz").is_err()); // unknown unit
        assert!(parse_shift("+").is_err()); // sign only
    }

    #[test]
    fn parse_base_reads_keywords_and_absolute() {
        assert_eq!(parse_base("today").unwrap(), Base::Today);
        assert_eq!(parse_base("now").unwrap(), Base::Now);
        assert!(matches!(parse_base("2025-01-31T12:00:00").unwrap(), Base::Absolute(_)));
        assert!(parse_base("2025-02-31T00:00:00").is_err()); // impossible day rejected, not normalized
    }

    #[test]
    fn parse_set_time_reads_hms() {
        assert_eq!(parse_set_time("23:59:59").unwrap(), Step::SetTime { hour: 23, minute: 59, second: 59 });
        assert!(parse_set_time("23:59").is_err()); // wrong shape
        assert!(parse_set_time("aa:bb:cc").is_err()); // non-numeric
        assert!(parse_set_time("+23:59:59").is_err()); // a sign the integer parse used to accept (R4-N34)
        assert!(parse_set_time("23:+5:00").is_err());
    }

    #[test]
    fn parse_snap_reads_targets_and_rejects_unknown() {
        assert_eq!(parse_snap("end-of-quarter").unwrap(), SnapTarget::EndOfQuarter);
        assert_eq!(parse_snap("eom").unwrap(), SnapTarget::EndOfMonth);
        assert_eq!(parse_snap("start-of-year").unwrap(), SnapTarget::StartOfYear);
        assert!(parse_snap("end-of-week").is_err());
    }

    /// Every code reads back as the value it was written from. The catalogue hands the window these
    /// codes and the window sends them straight back as flags, so a code the parser does not read
    /// would turn a listed preset into a refused calculation.
    #[test]
    fn every_code_reads_back_as_the_value_it_names() {
        use Unit::*;
        for unit in [Seconds, Minutes, Hours, Days, Weeks, Months, Quarters, Years, BusinessDays] {
            assert_eq!(parse_unit(unit_code(unit)), Some(unit), "{unit:?}");
        }
        use SnapTarget::*;
        for target in [StartOfMonth, EndOfMonth, StartOfQuarter, EndOfQuarter, StartOfYear, EndOfYear] {
            assert_eq!(parse_snap(snap_code(target)), Ok(target), "{target:?}");
        }
        use NearestTarget::*;
        for target in [NextBusinessDay, PrevBusinessDay, NextLeapDay] {
            assert_eq!(parse_nearest(nearest_code(target)), Ok(target), "{target:?}");
        }
    }

    #[test]
    fn parse_nearest_reads_targets_and_rejects_unknown() {
        assert_eq!(parse_nearest("next-business-day").unwrap(), NearestTarget::NextBusinessDay);
        assert_eq!(parse_nearest("pbd").unwrap(), NearestTarget::PrevBusinessDay);
        assert!(parse_nearest("next-full-moon").is_err());
    }
}
