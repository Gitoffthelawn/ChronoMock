using System.Globalization;
using System.IO;
using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The calculator acts on the result for the builder AS IT STANDS, never on the one still on screen from
/// before an edit (R4-W4), and a result that is gone takes everything it wrote with it (R4-S21, R4-N43,
/// R4-N44).
/// <para>
/// The reported shape of W4: a step reads "+1 d", the tester types 30 and presses "Use this date" at once.
/// The press takes focus from the field, which commits 30 and only SCHEDULES its result, so the press used
/// to send the date for +1 d. The engine here answers from the arguments it is asked with - New Year's Day
/// plus the shift - so which question an answer belongs to can be read straight off the date.
/// </para>
/// </summary>
public class CalculatorAnswerTests
{
    private const string BaseDay = "2026-01-01T00:00:00";

    [Fact]
    public async Task Use_pressed_straight_after_an_edit_sends_the_date_for_the_edit()
    {
        var vm = await CalculatorWithOneStepAsync(new FakeCalcEngine(NewYearShifted));
        string? sent = null;
        vm.UseInSubstitutionRequested += (moment, _) => sent = moment;

        vm.Steps[0].Amount = "30"; // what leaving the field commits, the moment the button takes focus

        Assert.True(vm.IsResultStale);
        await vm.RequestUseInSubstitutionAsync();

        Assert.Equal("2026-01-31T00:00:00", sent);
    }

    [Fact]
    public async Task Copy_pressed_straight_after_an_edit_copies_the_value_for_the_edit_from_the_same_row()
    {
        var vm = await CalculatorWithOneStepAsync(new FakeCalcEngine(NewYearShifted));
        var isoDate = vm.Formats[0];

        vm.Steps[0].Amount = "30";
        var copied = await vm.CopyValueAsync(isoDate);

        Assert.Equal("2026-01-31", copied);

        // Updated in place (R4-Z4): the pressed row is still the row on screen, so its button can answer.
        Assert.Same(isoDate, vm.Formats[0]);
    }

    [Fact]
    public async Task Copy_on_the_custom_format_row_straight_after_a_mask_edit_copies_the_new_rendering()
    {
        // The mask reformats the same moment, so it keeps the preset framing - but the row it renders still
        // shows the OLD mask until the new run lands, and a Copy pressed in between must wait for it.
        var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
            args.Contains("--analyze")
                ? CalcResults.Analysis("2008-04-08T00:00:00")
                : CalcResults.Moment(BaseDay, custom: new FakeCalcEngine.Question(args).ValueOf("--format"))));
        await vm.EnsureComputedAsync();
        vm.CustomFormatMask = "yyyy";
        await Until(() => vm.CustomFormatRow.Value == "yyyy" && !vm.IsResultStale, "the first mask to render");

        vm.CustomFormatMask = "yyyy-MM";
        var copied = await vm.CopyValueAsync(vm.CustomFormatRow);

        Assert.Equal("yyyy-MM", copied);
    }

    [Fact]
    public async Task An_answer_to_an_older_edit_never_lands_over_a_newer_one()
    {
        // The engine ignores cancellation here, so the old answer DOES arrive, and last. What keeps it off
        // the screen is the generation it was asked for, not the luck of it never coming.
        var engine = new FakeCalcEngine(honoursCancellation: false);
        var vm = new CalculatorViewModel(engine);
        var reveal = vm.EnsureComputedAsync();
        (await engine.QuestionAsync(0)).Answer(CalcResults.Analysis("2008-04-08T00:00:00"));
        (await engine.QuestionAsync(1)).Answer(CalcResults.Moment(BaseDay));
        await reveal;

        vm.AddStep();
        var older = await engine.QuestionAsync(2); // +1 d, held
        vm.Steps[0].Amount = "30";
        var newer = await engine.QuestionAsync(3); // +30 d

        newer.Answer(NewYearShifted(newer.Args));
        await Until(() => vm.ResultDate == "2026-01-31", "the newer answer to land");
        older.Answer(NewYearShifted(older.Args));
        await Task.Delay(300, TestContext.Current.CancellationToken); // room for the late answer to be applied, wrongly

        Assert.Equal("2026-01-31", vm.ResultDate);
        Assert.False(vm.IsResultStale);
    }

    [Fact]
    public async Task An_engine_refusal_leaves_no_trace_of_the_result_before_it()
    {
        var vm = await CalculatorWithOneStepAsync(new FakeCalcEngine(args =>
            Shift(args) == "+99d"
                ? throw new CalcException("chrono calc: step 1 overflows the representable range (calc.overflow)", 1)
                : NewYearShifted(args)));
        var isoDate = vm.Formats[0];

        vm.Steps[0].Amount = "99";
        await Until(() => vm.HasError, "the refusal to land");

        // R4-S21: the old date, formats and metadata stood under the error, and Copy copied them.
        Assert.Equal("-", vm.ResultDate);
        Assert.Empty(vm.Formats);
        Assert.Empty(vm.MetadataLine);
        Assert.False(vm.CanUseInSubstitution);
        Assert.Null(await vm.CopyValueAsync(isoDate));
    }

    [Fact]
    public async Task A_preset_waiting_for_a_parameter_shows_no_result_and_a_pending_edit_does_not_land_under_it()
    {
        var presets = Path.Combine(TestPaths.RepoRoot(), "presets");
        var engine = new FakeCalcEngine(NewYearShifted);
        var vm = await CalculatorWithOneStepAsync(engine, presets);

        vm.Steps[0].Amount = "30"; // schedules a run a quarter of a second from now
        vm.ApplyPreset(PresetCatalog.Load(presets).Single(p => p.Id == "age-of-majority"));
        var asked = engine.Count;

        Assert.True(vm.ActiveNeedsParameters);
        Assert.Equal("-", vm.ResultDate);
        Assert.False(vm.CanUseInSubstitution);
        Assert.False(vm.IsResultStale); // nothing is coming - the preset is waiting for the tester

        await Task.Delay(600, TestContext.Current.CancellationToken); // past the quiet period

        Assert.Equal(asked, engine.Count);
        Assert.Equal("-", vm.ResultDate);
    }

    [Fact]
    public async Task Clearing_the_result_clears_the_clamp_note_and_the_mask_warning_too()
    {
        var clamped = CalcResults.Moment(
            BaseDay, custom: "2026 qq", unknown: ["qq"], clamps: [new CalcClampedStep(1, 31, 28)]);
        var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
            args.Contains("--analyze") ? CalcResults.Analysis("2008-04-08T00:00:00") : clamped));
        await vm.EnsureComputedAsync();
        vm.CustomFormatMask = "yyyy qq";
        await Until(() => vm.HasClampNotice && vm.HasCustomFormatWarning, "the clamp and the warning to show");

        vm.SelectedBase = vm.BaseKinds.Single(b => b.Kind == BaseKind.Specific);
        vm.Base.DateText = "not a date";
        await Until(() => vm.ResultDate == "-", "the result to clear");

        // R4-N43: both stood under a result that was no longer there.
        Assert.False(vm.HasClampNotice);
        Assert.Empty(vm.ClampNotice);
        Assert.False(vm.HasCustomFormatWarning);
        Assert.Empty(vm.CustomFormatWarning);
    }

    [Fact]
    public async Task A_format_the_moment_falls_outside_of_has_nothing_to_copy()
    {
        var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
            args.Contains("--analyze") ? CalcResults.Analysis("2008-04-08T00:00:00") : CalcResults.Moment(BaseDay, epochSeconds: null)));
        await vm.EnsureComputedAsync();

        var epochSeconds = vm.Formats[4];

        // R4-Z3: the row shows the out-of-range marker, a sentence about the value and not one to paste.
        Assert.False(epochSeconds.HasValue);
        Assert.Null(await vm.CopyValueAsync(epochSeconds));
        Assert.True(vm.Formats[0].HasValue);
    }

    [Fact]
    public async Task An_analysis_refusal_keeps_the_engine_sentence_and_drops_the_old_readings()
    {
        // On the UI thread with the real strings, because whether the engine's sentence is detail at all
        // depends on the refusal having a translation of its own - without one the sentence IS the message,
        // and there is nothing to add under it.
        const string refusal = "chrono calc: step 3 overflows the representable range (calc.overflow)";
        var seen = await WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
                args.Contains("garbage")
                    ? throw new CalcException(refusal, 1)
                    : args.Contains("--analyze") ? CalcResults.Analysis("2008-04-08T00:00:00", "2008-08-04T00:00:00") : CalcResults.Moment(BaseDay)));
            await vm.EnsureComputedAsync();
            await Until(() => vm.Readings.Count == 2, "the first analysis to land");

            vm.AnalyzeText = "garbage";
            var staleAtOnce = vm.IsAnalysisStale;
            await Until(() => vm.AnalyzeHasError, "the refusal to land");

            return new
            {
                staleAtOnce,
                vm.Readings.Count,
                vm.AnalyzeAmbiguous,
                vm.HasAnalysis,
                vm.AnalyzeErrorDetail,
                vm.HasAnalyzeErrorDetail,
                vm.IsAnalysisStale,
                Expected = CalculatorViewModel.DetailForCalcError(refusal),
            };
        });

        Assert.True(seen.staleAtOnce);
        Assert.Equal(0, seen.Count);
        Assert.False(seen.AnalyzeAmbiguous);
        Assert.False(seen.HasAnalysis);
        Assert.NotEmpty(seen.Expected); // not vacuous: the sentence really is detail under a translation
        Assert.Equal(seen.Expected, seen.AnalyzeErrorDetail);
        Assert.True(seen.HasAnalyzeErrorDetail);
        Assert.False(seen.IsAnalysisStale);
    }

    [Fact]
    public async Task An_answer_with_no_analysis_drops_the_old_readings()
    {
        var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
            args.Contains("empty") ? CalcResults.Nothing()
            : args.Contains("--analyze") ? CalcResults.Analysis("2008-04-08T00:00:00", "2008-08-04T00:00:00")
            : CalcResults.Moment(BaseDay)));
        await vm.EnsureComputedAsync();
        await Until(() => vm.Readings.Count == 2, "the first analysis to land");

        vm.AnalyzeText = "empty";
        await Until(() => vm.AnalyzeHasError, "the empty answer to land");

        Assert.Empty(vm.Readings);
        Assert.False(vm.AnalyzeAmbiguous);
    }

    /// <summary>A calculator past its first reveal with one "+1 d" step and that step's result on screen.</summary>
    private static async Task<CalculatorViewModel> CalculatorWithOneStepAsync(FakeCalcEngine engine, string? presets = null)
    {
        var vm = new CalculatorViewModel(engine, presets);
        await vm.EnsureComputedAsync();
        vm.AddStep();
        await Until(() => vm.ResultDate == "2026-01-02" && !vm.IsResultStale, "the +1 d result to land");
        return vm;
    }

    /// <summary>The engine for these tests: New Year's Day 2026 plus the one day shift in the question, and
    /// a fixed analysis for the reverse strip.</summary>
    private static CalcResult NewYearShifted(IReadOnlyList<string> args)
    {
        if (args.Contains("--analyze"))
        {
            return CalcResults.Analysis("2008-04-08T00:00:00");
        }

        var shift = Shift(args);
        var days = shift is null ? 0 : int.Parse(shift[1..^1], CultureInfo.InvariantCulture);
        var moment = DateTime.ParseExact(BaseDay, "yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture).AddDays(days);
        return CalcResults.Moment(moment.ToString("yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture));
    }

    private static string? Shift(IReadOnlyList<string> args)
    {
        var at = args.ToList().IndexOf("--shift");
        return at >= 0 && at + 1 < args.Count ? args[at + 1] : null;
    }

    /// <summary>Wait for a condition the view model reaches on its own, at most five seconds. A condition
    /// that never comes fails with what was being waited for.</summary>
    internal static async Task Until(Func<bool> condition, string what)
    {
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(5);
        while (!condition())
        {
            Assert.True(DateTime.UtcNow < deadline, $"timed out waiting for {what}");
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }
    }
}
