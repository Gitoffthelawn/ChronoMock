using System.Globalization;
using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>
/// The calculator's result column as data: everything one computed moment shows, or the honest empty column
/// (<see cref="Empty"/>). The view model puts either on screen through one method, so a line the result
/// writes cannot be one the clear forgets - the clamp note and the custom-format warning once stood under a
/// result that was no longer there (R4-N43).
/// </summary>
internal sealed record ResultLines(
    bool HasResult,
    string Weekday,
    string Date,
    string Time,
    string Zone,
    string MetadataLine,
    IReadOnlyList<string> SignificanceKeys,
    string SignificanceCalendar,
    IReadOnlyList<FormatValue> Formats,
    string CustomFormat,
    bool HasCustomFormatWarning,
    string CustomFormatWarning,
    bool HasClampNotice,
    string ClampNotice,
    string MomentLocal,
    int ZoneBias,
    bool CanUseInSubstitution)
{
    /// <summary>No result: an unreadable base (the moment input already says why), an engine refusal, or a
    /// preset still waiting for a parameter. The date shows a dash, every other line is absent.</summary>
    internal static ResultLines Empty { get; } = new(
        false, string.Empty, "-", string.Empty, string.Empty, string.Empty, [], string.Empty, [], string.Empty,
        false, string.Empty, false, string.Empty, string.Empty, 0, false);
}

/// <summary>One output format of the moment: the translation key of its label and its value, null for a
/// moment outside what that format can hold.</summary>
internal sealed record FormatValue(string LabelKey, string? Value);

/// <summary>
/// How one computed moment becomes the lines of the result column (R5-D9). Pure over the moment and the
/// translator, so what the column says is tested without a view model or a window.
/// </summary>
internal static class ResultProjection
{
    /// <summary>The result column for <paramref name="moment"/>. The caller has checked that its formats and
    /// metadata are present - System.Text.Json does not enforce non-nullability (M-11).</summary>
    internal static ResultLines From(
        CalcMoment moment, IReadOnlyList<CalendarOption> calendars, Func<string, string> tr)
    {
        var t = moment.Iso.IndexOf('T', StringComparison.Ordinal);
        var significance = moment.Significance.Select(key => $"calc.sig.{key}").ToList();
        var custom = moment.CustomFormat ?? string.Empty;
        // The engine names the letter runs it did not recognise. They ARE in the custom line, verbatim, so
        // the row without this warning reads as a rendered date that happens to contain letters.
        var unknownTokens = custom.Length > 0 && moment.CustomFormatUnknown is { Count: > 0 };
        // One line per clamped step, so a two-clamp expression does not hide the second one. Through
        // TextFormat, like every translated template: a broken translation file used to throw here,
        // halfway through applying a result (R4-N46).
        var clamps = moment.ClampedSteps;
        var clamped = clamps is { Count: > 0 };

        return new ResultLines(
            HasResult: true,
            // The engine sends the weekday in English - render it in the current language (rule 15). Resolved
            // here (not exposed as a key) because it is one part of the result column the view binds as text.
            Weekday: tr(WeekdayKey(moment.Metadata.Weekday)),
            Date: t >= 0 ? moment.Iso[..t] : moment.Iso,
            Time: t >= 0 ? moment.Iso[(t + 1)..] : string.Empty,
            Zone: OffsetLabel(moment.ZoneBiasMin),
            MetadataLine: MetadataLine(moment.Metadata, tr),
            SignificanceKeys: significance,
            SignificanceCalendar: significance.Count > 0
                ? CalendarNote(
                    calendars,
                    tr,
                    moment.Metadata.Calendar,
                    moment.Significance.Contains("before_calendar") ? moment.Metadata.CalendarValidFrom : null,
                    moment.Significance.Contains("calendar_outdated") ? moment.Metadata.CalendarLawAsOf : null)
                : string.Empty,
            Formats: Formats(moment.Formats),
            CustomFormat: custom,
            HasCustomFormatWarning: unknownTokens,
            CustomFormatWarning: unknownTokens
                ? TextFormat.Translate(tr, "calc.fmt.unknown_tokens", string.Join(", ", moment.CustomFormatUnknown!))
                : string.Empty,
            HasClampNotice: clamped,
            ClampNotice: clamped
                ? string.Join(
                    Environment.NewLine,
                    clamps!.Select(c => TextFormat.Translate(tr, "calc.clamped_step", c.Step, c.RequestedDay, c.ClampedTo)))
                : string.Empty,
            // The moment with its zone, for the bridge to substitution (rule 2 - never a bare date).
            MomentLocal: moment.Iso,
            ZoneBias: moment.ZoneBiasMin,
            CanUseInSubstitution: CalculatorViewModel.CanTransferZone(moment.ZoneBiasMin));
    }

    /// <summary>
    /// Every output format of the moment, in the order the column lists them. Labels are translated
    /// (rule 15) by the view, the values are data, and format NAMES (US, PL, FILETIME, RFC 1123) are proper
    /// nouns that the Polish text keeps as they are.
    /// </summary>
    private static IReadOnlyList<FormatValue> Formats(CalcFormats f) =>
    [
        new("calc.fmt.iso_date", f.IsoDate),
        new("calc.fmt.iso_datetime", f.IsoDatetime),
        new("calc.fmt.us", f.Us),
        new("calc.fmt.pl", f.Pl),
        new("calc.fmt.epoch_s", Number(f.EpochSeconds)),
        new("calc.fmt.epoch_ms", Number(f.EpochMillis)),
        new("calc.fmt.filetime", Number(f.Filetime)),
        new("calc.fmt.rfc1123", f.Rfc1123),
    ];

    /// <summary>The sentence naming the calendar that judged the weekend and holiday marks, or empty when
    /// the engine applied none. The id comes back from the engine (e.g. "us-banking") and is shown through
    /// the picker's own label, so the note names the calendar the way the tester picked it (rule 15). An id
    /// the picker does not know is shown as itself rather than dropped - an honest raw id beats a sentence
    /// that quietly omits which calendar it means.
    ///
    /// Two results say more about the calendar than its name (R4-S20, R4/20). <paramref name="startsIn"/> is
    /// the calendar's first year when the date lies before it: the calendar judged nothing, and "marks
    /// follow" would claim it did. <paramref name="lawAsOf"/> is the day its holidays were checked against
    /// the law when that check is more than a year old for what was computed: the note says from when, so
    /// the tester can tell how old.</summary>
    internal static string CalendarNote(
        IReadOnlyList<CalendarOption> calendars, Func<string, string> tr, string? calendarId,
        long? startsIn = null, string? lawAsOf = null)
    {
        if (calendarId is null)
        {
            return string.Empty;
        }

        var label = calendars.FirstOrDefault(c => c.Id == calendarId) is { } known
            ? tr(known.LabelKey)
            : calendarId;
        if (startsIn is { } year)
        {
            return TextFormat.Translate(tr, "calc.sig_calendar_before", label, year);
        }

        return lawAsOf is { } day
            ? TextFormat.Translate(tr, "calc.sig_calendar_outdated", label, day)
            : TextFormat.Translate(tr, "calc.sig_calendar", label);
    }

    /// <summary>Translation key for a weekday name the engine emits in English (e.g. "Monday" ->
    /// "calc.weekday.monday"). An unrecognised name falls through to KeyToText's honest key-as-text.</summary>
    internal static string WeekdayKey(string engineWeekday)
        => $"calc.weekday.{engineWeekday.ToLowerInvariant()}";

    private static string MetadataLine(CalcMetadata m, Func<string, string> tr)
    {
        // Labels are translated (rule 15) - ISO and Q are universal notation, and the numbers and holiday
        // name are data. The weekday comes from the engine in English, mapped to a key and resolved.
        var parts = new List<string>
        {
            tr(WeekdayKey(m.Weekday)),
            $"ISO {m.IsoWeekYear}-W{m.IsoWeek:D2}",
            $"{tr("calc.md.us_week")} {m.UsWeek}",
            $"Q{m.Quarter}",
            $"{tr("calc.md.day")} {m.DayOfYear}",
            tr(m.IsLeapYear ? "calc.md.leap_year" : "calc.md.common_year"),
        };
        if (m.BusinessDay is { } business)
        {
            parts.Add(tr(business ? "calc.md.business_day" : "calc.md.not_business_day"));
        }

        if (m.Holiday is { } holiday)
        {
            parts.Add(holiday); // the holiday name follows the DATA locale (the calendar), not the UI (rule 15)
        }

        return string.Join(" · ", parts);
    }

    private static string? Number(long? value) => value?.ToString(CultureInfo.InvariantCulture);

    /// <summary>A "+HH:MM" / "-HH:MM" offset for a session bias (UTC = local + bias, so the offset is -bias).</summary>
    private static string OffsetLabel(int biasMin)
    {
        var off = -biasMin;
        var sign = off < 0 ? '-' : '+';
        return $"UTC{sign}{Math.Abs(off) / 60:D2}:{Math.Abs(off) % 60:D2}";
    }
}
