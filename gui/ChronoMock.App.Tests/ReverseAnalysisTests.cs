using ChronoMock.App.Calc;
using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// Reverse analysis (slice G5): the analyze argument list and the mapping from an engine reading to a row.
/// Pure - no UI thread, no process - except where the zone line is read in the real strings.
/// </summary>
public class ReverseAnalysisTests
{
    [Fact]
    public void Analyze_args_wrap_the_trimmed_text()
        => Assert.Equal(new[] { "--analyze", "04/08/2008" }, CalculatorViewModel.BuildAnalyzeArgs("  04/08/2008 "));

    /// <summary>R4-S23, owner's decision D4: a pasted number is an instant, and the zone its wall clock is
    /// shown in is the calculator's, the one <c>--zone</c> names for <c>--analyze</c> in the CLI.</summary>
    [Fact]
    public void Analyze_args_carry_the_calculator_zone_when_one_is_picked()
        => Assert.Equal(
            new[] { "--analyze", "1700000000", "--zone", "+05:00" },
            CalculatorViewModel.BuildAnalyzeArgs(" 1700000000 ", "+05:00"));

    [Fact]
    public void A_reading_row_maps_the_key_date_and_significance()
    {
        var metadata = new CalcMetadata("Tuesday", 2008, 15, 15, 99, 2, true, -6714, null, null);
        var reading = new CalcReading("us_month_day", "2008-04-08T00:00:00", ["end_of_quarter"], metadata);

        var row = new ReadingRow(reading);

        Assert.Equal("calc.reading.us_month_day", row.ReadingLabelKey);
        // The weekday is a translation key (mapped from the engine's English name), rendered via KeyToText -
        // the date is data. Kept as a key so the row stays language-neutral and testable without WPF.
        Assert.Equal("calc.weekday.tuesday", row.WeekdayKey);
        Assert.Equal("2008-04-08", row.Date);
        Assert.Equal(new[] { "calc.sig.end_of_quarter" }, row.Significance);
        // Every reading's iso ends at midnight, so midnight is not a time to show unless the engine says so.
        Assert.Equal(string.Empty, row.Time);
    }

    /// <summary>R4-S23: an instant reading showed its date alone, the time cut off with the rest of the
    /// iso, so a number pasted from a log read as a day with no moment in it.</summary>
    [Fact]
    public void A_reading_row_shows_the_time_the_engine_gives_it()
    {
        var metadata = new CalcMetadata("Tuesday", 2023, 46, 318, 47, 4, false, 0, null, null);
        var reading = new CalcReading("epoch_seconds", "2023-11-14T22:13:20", [], metadata, Instant: true, Time: "22:13:20");

        var row = new ReadingRow(reading);

        Assert.Equal("2023-11-14", row.Date);
        Assert.Equal("22:13:20", row.Time);
    }

    /// <summary>R4-S23, D4: picking a zone moves the readings of a pasted number with it. Without the second
    /// question the readings stayed in the zone the analysis was first asked in, under a picker naming another.
    /// </summary>
    [Fact]
    public async Task Picking_a_zone_asks_the_analysis_again_in_that_zone()
    {
        var engine = new FakeCalcEngine(args => args.Contains("--analyze")
            ? CalcResults.Analysis("2008-04-08T00:00:00")
            : CalcResults.Moment("2026-01-01T00:00:00"));
        var vm = new CalculatorViewModel(engine);
        await vm.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => vm.Readings.Count == 1 && !vm.IsAnalysisStale, "the first analysis to land");

        // This machine's entry sends no zone - the engine reads the host's, as the CLI does without --zone.
        Assert.All(Analyses(engine.Asked), q => Assert.Null(q.ValueOf("--zone")));

        var picked = vm.BaseZones.First(z => !z.IsHost && z.BiasMinutes != 0);
        var before = engine.Count;
        vm.SelectedBaseZone = picked;
        await CalculatorAnswerTests.Until(() => Analyses(engine.Asked.Skip(before)).Any(), "the analysis to be asked again");

        Assert.Equal(
            ZoneLabel.OffsetFromBiasMinutes(picked.BiasMinutes),
            Analyses(engine.Asked.Skip(before)).Last().ValueOf("--zone"));
    }

    /// <summary>R4-S23: an instant has a wall clock only in some zone, and the readings showed one with no
    /// zone near them. A date written as fields has none, so it gets no line - and the line of the number
    /// before it does not stay above it.</summary>
    [Fact]
    public async Task The_zone_line_stands_above_an_instant_and_only_there()
    {
        var seen = await WpfTestHost.RunAsync(async () =>
        {
            ZoneOption? picked = null;
            var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
                !args.Contains("--analyze") ? CalcResults.Moment("2026-01-01T00:00:00")
                : args.Contains("1700000000") ? CalcResults.Instant("2023-11-14T22:13:20", "22:13:20", picked?.BiasMinutes ?? 0)
                : CalcResults.Analysis("2008-04-08T00:00:00")));
            await vm.EnsureComputedAsync();
            await CalculatorAnswerTests.Until(() => vm.Readings.Count == 1 && !vm.IsAnalysisStale, "the date's analysis to land");
            var aboveTheFirstDate = vm.HasAnalysisZoneLine;

            picked = vm.BaseZones.First(z => !z.IsHost && z.BiasMinutes != 0);
            vm.SelectedBaseZone = picked;
            vm.AnalyzeText = "1700000000";
            await CalculatorAnswerTests.Until(() => !vm.IsAnalysisStale && vm.Readings.Count == 1 && vm.Readings[0].Time.Length > 0, "the instant's analysis to land");
            var aboveTheInstant = vm.AnalysisZoneLine;

            vm.SelectedBaseZone = vm.BaseZones.First(z => z.IsHost);
            await CalculatorAnswerTests.Until(() => !vm.IsAnalysisStale, "the instant's analysis in this machine's zone to land");
            var aboveTheInstantOnThisMachine = vm.AnalysisZoneLine;

            vm.AnalyzeText = "2008-04-08";
            await CalculatorAnswerTests.Until(() => !vm.IsAnalysisStale && vm.Readings.Count == 1 && vm.Readings[0].Time.Length == 0, "the second date's analysis to land");

            return new
            {
                aboveTheFirstDate,
                aboveTheInstant,
                aboveTheInstantOnThisMachine,
                aboveTheSecondDate = vm.HasAnalysisZoneLine,
                Label = ZoneLabel.FromBiasMinutes(picked.BiasMinutes),
                Expected = TextFormat.Translate(TranslationKeyConverter.Resolve, "calc.analyze_zone", ZoneLabel.FromBiasMinutes(picked.BiasMinutes)),
                Host = TranslationKeyConverter.Resolve("zone.host"),
            };
        });

        Assert.False(seen.aboveTheFirstDate);
        Assert.Equal(seen.Expected, seen.aboveTheInstant);
        Assert.Contains(seen.Label, seen.aboveTheInstant, StringComparison.Ordinal); // not vacuous: the real strings are loaded
        Assert.DoesNotContain(seen.Host, seen.aboveTheInstant, StringComparison.Ordinal);
        // Rule 2: this machine's entry sends no zone, so the line says which zone that turned out to be.
        Assert.Contains($"({seen.Host})", seen.aboveTheInstantOnThisMachine, StringComparison.Ordinal);
        Assert.False(seen.aboveTheSecondDate);
    }

    private static IEnumerable<FakeCalcEngine.Question> Analyses(IEnumerable<FakeCalcEngine.Question> asked)
        => asked.Where(q => q.Args.Contains("--analyze"));
}
