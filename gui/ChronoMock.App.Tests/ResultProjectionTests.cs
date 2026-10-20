using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The calculator's result column as data (R5-D9): what one computed moment shows, and the empty column a
/// refusal or an unreadable base leaves. Pure over the moment and a translator, so read here without a view
/// model, a window or an engine.
/// </summary>
public class ResultProjectionTests
{
    /// <summary>Keys resolve to themselves, so a line is found by the key that produced it.</summary>
    private static string Key(string key) => key;

    private static readonly CalendarOption[] NoCalendars = [];

    private static CalcMoment Moment(
        string? custom = null, IReadOnlyList<string>? unknown = null, IReadOnlyList<CalcClampedStep>? clamps = null)
        => new(
            "2031-02-28T09:30:00", -120, "today", [],
            new CalcFormats("2031-02-28", "2031-02-28T09:30:00", "02/28/2031", "28.02.2031", 1930000000, null, null, null),
            new CalcMetadata("Friday", 2031, 9, 9, 59, 1, false, 12, BusinessDay: null, Holiday: null),
            [], custom, unknown, clamps, null);

    [Fact]
    public void The_empty_column_shows_a_dash_and_nothing_else()
    {
        var empty = ResultLines.Empty;

        Assert.False(empty.HasResult);
        Assert.Equal("-", empty.Date);
        Assert.Empty(empty.Formats);
        Assert.Empty(empty.SignificanceKeys);
        Assert.False(empty.CanUseInSubstitution);
        Assert.Equal(
            (false, string.Empty, false, string.Empty),
            (empty.HasCustomFormatWarning, empty.CustomFormatWarning, empty.HasClampNotice, empty.ClampNotice));
    }

    [Fact]
    public void A_moment_splits_into_date_time_and_zone_and_keeps_every_format_in_order()
    {
        var lines = ResultProjection.From(Moment(), NoCalendars, Key);

        Assert.True(lines.HasResult);
        Assert.Equal(("2031-02-28", "09:30:00", "UTC+02:00"), (lines.Date, lines.Time, lines.Zone));
        Assert.Equal("calc.weekday.friday", lines.Weekday);
        Assert.Equal(
            ["calc.fmt.iso_date", "calc.fmt.iso_datetime", "calc.fmt.us", "calc.fmt.pl", "calc.fmt.epoch_s",
                "calc.fmt.epoch_ms", "calc.fmt.filetime", "calc.fmt.rfc1123"],
            lines.Formats.Select(f => f.LabelKey));
        // A null value is a moment outside what that format holds - the view says so, it does not blank it.
        Assert.Equal("1930000000", lines.Formats[4].Value);
        Assert.Null(lines.Formats[5].Value);
        // The moment travels to substitution with its zone (rule 2).
        Assert.Equal(("2031-02-28T09:30:00", -120), (lines.MomentLocal, lines.ZoneBias));
    }

    [Fact]
    public void The_mask_warning_and_the_clamp_note_stand_only_with_what_they_explain()
    {
        // Unknown letters with no custom line have nothing to qualify, and a clamp note is one line per step.
        var noMask = ResultProjection.From(Moment(unknown: ["Q"]), NoCalendars, Key);
        var withMask = ResultProjection.From(Moment(custom: "28 Q", unknown: ["Q"]), NoCalendars, Key);
        var clamped = ResultProjection.From(
            Moment(clamps: [new CalcClampedStep(1, 31, 28), new CalcClampedStep(2, 30, 28)]), NoCalendars, Key);

        Assert.False(noMask.HasCustomFormatWarning);
        Assert.True(withMask.HasCustomFormatWarning);
        Assert.Equal("calc.fmt.unknown_tokens", withMask.CustomFormatWarning);
        Assert.True(clamped.HasClampNotice);
        Assert.Equal(2, clamped.ClampNotice.Split(Environment.NewLine).Length);
    }
}
