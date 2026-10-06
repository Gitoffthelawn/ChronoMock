using ChronoMock.App.Calc;
using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// R4-S20 and R4/20 in the window: the note under the marks says more about the calendar than its name
/// when the engine says so. A date before the calendar's first year was not judged at all, so "marks
/// follow the calendar" would claim a judgement that never happened. A calendar checked against the law
/// more than a year before what was computed says from when, so the tester can tell how old it is.
///
/// <para>The switch is the engine's mark, never the metadata alone: every result computed with a calendar
/// carries its first year and its check date, and only the mark says the date fell outside or the check
/// was old.</para>
/// </summary>
public class CalendarRangeNoteTests
{
    private const string Schema = "chronomock.calc/1";

    private static CalcResult Judged(string iso, CalcMetadata metadata, params string[] significance)
        => new(
            Schema,
            new CalcMoment(
                iso,
                0,
                "base",
                [],
                new CalcFormats(iso[..10], iso, iso, iso, 0, 0, 0, iso),
                metadata,
                significance,
                null,
                null,
                null,
                null),
            null);

    private static CalcMetadata Polish(bool? businessDay)
        => new(
            "Friday", 1985, 18, 18, 123, 2, false, 0, businessDay, null,
            Calendar: "pl", CalendarValidFrom: 2002, CalendarLawAsOf: "2030-06-01");

    [Fact]
    public async Task The_engine_marks_switch_the_note_and_the_fields_alone_do_not()
    {
        var seen = await WpfTestHost.RunAsync(async () =>
        {
            var answer = Judged("2026-05-04T00:00:00", Polish(true), "start_of_month");
            var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
                args.Contains("--analyze") ? CalcResults.Analysis("2008-04-08T00:00:00") : answer));
            await vm.EnsureComputedAsync();
            await CalculatorAnswerTests.Until(() => vm.HasSignificanceCalendar, "the plain note to show");
            var plain = vm.SignificanceCalendar;

            answer = Judged("1985-05-03T00:00:00", Polish(null), "before_calendar");
            vm.AddStep();
            await CalculatorAnswerTests.Until(() => vm.ResultDate == "1985-05-03" && !vm.IsResultStale, "the old date to land");
            var before = vm.SignificanceCalendar;

            answer = Judged("2032-03-10T00:00:00", Polish(true), "calendar_outdated");
            vm.AddStep();
            await CalculatorAnswerTests.Until(() => vm.ResultDate == "2032-03-10" && !vm.IsResultStale, "the late date to land");
            var outdated = vm.SignificanceCalendar;

            var label = TranslationKeyConverter.Resolve(vm.Calendars.Single(c => c.Id == "pl").LabelKey);
            return new
            {
                plain,
                before,
                outdated,
                Plain = TextFormat.Translate("calc.sig_calendar", label),
                Before = TextFormat.Translate("calc.sig_calendar_before", label, 2002L),
                Outdated = TextFormat.Translate("calc.sig_calendar_outdated", label, "2030-06-01"),
            };
        });

        Assert.Equal(seen.Plain, seen.plain);
        Assert.Equal(seen.Before, seen.before);
        Assert.Equal(seen.Outdated, seen.outdated);
        // Not vacuous: three different sentences, and each carries what it is about.
        Assert.NotEqual(seen.plain, seen.before);
        Assert.NotEqual(seen.plain, seen.outdated);
        Assert.Contains("2002", seen.before, StringComparison.Ordinal);
        Assert.Contains("2030-06-01", seen.outdated, StringComparison.Ordinal);
        Assert.DoesNotContain("{1}", seen.before + seen.outdated, StringComparison.Ordinal);
    }
}
