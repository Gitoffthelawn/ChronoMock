using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The window's end of the bridge to the engine's preset catalogue (R4/18). The engine's answer for a test
/// catalogue of odd files is recorded in the repository and held to the engine by
/// <c>the_test_catalogue_gives_its_recorded_answer</c> in <c>preset_catalogue.rs</c>. These tests read the SAME
/// file and check what the window makes of it - so a change to the engine's answer turns that test red, and a
/// change to how the window reads it turns these red. Plus the parts that are the window's own: one read
/// shared by both lists, the lines that say where a list stands, the filter.
/// </summary>
public class PresetCatalogueTests
{
    private static PresetInfo Listed(string id)
        => PresetInfo.From(TestCatalogues.TestCatalogue().Presets.Single(p => p.Id == id));

    private static IReadOnlyList<ParamInputViewModel> Inputs(PresetInfo preset)
        => [.. preset.Parameters.Select(p => new ParamInputViewModel(p, StepViewModel.AllUnits))];

    private static Dictionary<string, ParamValue> Values(PresetInfo preset)
        => Inputs(preset).Where(i => i.ToValue() is not null).ToDictionary(i => i.Id, i => i.ToValue()!);

    [Fact]
    public void The_engines_answer_for_the_test_catalogue_is_read_whole()
    {
        var catalogue = TestCatalogues.TestCatalogue();

        Assert.Equal(PresetCatalogue.SupportedSchema, catalogue.Schema);
        // Fifteen files the engine accepts, thirty it refuses (crates/cli/tests/data/preset-catalogue).
        Assert.Equal(15, catalogue.Presets.Count);
        Assert.Equal(30, catalogue.Refused.Count);
        Assert.All(catalogue.Refused, r => Assert.False(string.IsNullOrWhiteSpace(r.Reason)));
    }

    /// <summary>
    /// Every preset the engine lists - shipped and odd - fills the builder with words the builder has an
    /// option for, once its parameters have values. A word the builder did not know would leave a step on
    /// whatever option it started on, which is a wrong date with nothing said (R4-S22).
    /// </summary>
    [Fact]
    public void Every_listed_preset_fills_the_builder_with_words_it_has_options_for()
    {
        var vm = new CalculatorViewModel(new CalcClient(() => "chrono"));
        var units = vm.Units.Select(u => u.Token).ToHashSet();
        var snaps = vm.SnapTargets.Select(t => t.Token).ToHashSet();
        var nearest = vm.NearestTargets.Select(t => t.Token).ToHashSet();
        var unpacked = 0;

        foreach (var preset in TestCatalogues.TestCatalogue().Presets.Concat(TestCatalogues.Shipped().Presets).Select(PresetInfo.From))
        {
            var values = Values(preset);
            if (values.Count < preset.Parameters.Count)
            {
                continue; // waits for a value the tester types - the "fill in" note's case, covered below
            }

            var moment = PresetUnpack.UnpackMoment(preset.Moment, preset.Parameters, values);
            unpacked++;
            foreach (var step in moment.Steps)
            {
                switch (step.Kind)
                {
                    case StepKind.Shift:
                        Assert.Contains(step.UnitToken, units);
                        Assert.Contains(step.Sign, new[] { "+", "-" });
                        break;
                    case StepKind.Snap:
                        Assert.Contains(step.SnapToken, snaps);
                        break;
                    case StepKind.Nearest:
                        Assert.Contains(step.NearestToken, nearest);
                        break;
                }
            }
        }

        Assert.True(unpacked >= 20, $"only {unpacked} presets unpacked - the guard went blind");
    }

    /// <summary>R4-S22: <c>month</c>, <c>hrs</c> and <c>businessdays</c> are words the engine reads and the
    /// window did not, so a preset using them said "fill in the parameters" with none to fill. They arrive as
    /// the builder's own codes now.</summary>
    [Fact]
    public void Unit_aliases_arrive_as_the_builders_codes()
    {
        var moment = PresetUnpack.UnpackMoment(Listed("units-every-alias").Moment, []);

        Assert.Equal(
            "s s s s m m m m h h h h d d d w w w mo mo mo q q q y y y y y bd bd bd",
            string.Join(' ', moment.Steps.Select(s => s.UnitToken)));
    }

    /// <summary>R4-S22: a duration and a variant without a default were given "1 day" and "day before" -
    /// values nobody entered, while the engine said the parameter had no value. They are empty now, so the
    /// preset waits for the tester, as the CLI does.</summary>
    [Fact]
    public void A_parameter_without_a_default_is_left_empty_and_the_preset_waits()
    {
        var inputs = Inputs(Listed("params-without-defaults"));

        Assert.All(inputs, i => Assert.Null(i.ToValue()));
        Assert.Equal(string.Empty, inputs.Single(i => i.IsDuration).Amount);
        Assert.Null(inputs.Single(i => i.IsVariant).Variant);
    }

    /// <summary>R4-S22: a date parameter's default was ignored - the field started empty - and a duration
    /// default of one month became one day.</summary>
    [Fact]
    public void Defaults_arrive_whole_and_fill_the_builder()
    {
        var preset = Listed("params-every-type");

        var moment = PresetUnpack.UnpackMoment(preset.Moment, preset.Parameters, Values(preset));

        Assert.Equal(BaseKind.Specific, moment.Base);
        Assert.Equal("2026-03-01T00:00:00", moment.BaseText);
        Assert.Equal(("-", "2", "mo"), (moment.Steps[0].Sign, moment.Steps[0].Amount, moment.Steps[0].UnitToken));
        Assert.Equal(("+", "0", "d"), (moment.Steps[1].Sign, moment.Steps[1].Amount, moment.Steps[1].UnitToken)); // on_day
    }

    [Fact]
    public void A_date_default_with_a_time_or_an_old_year_is_shown_as_it_is()
    {
        Assert.Equal("2026-03-01T12:30:00", Inputs(Listed("date-default-with-time")).Single().DateText);
        Assert.Equal("-0044-03-15", Inputs(Listed("date-default-negative-year")).Single().DateText);
        Assert.Equal(string.Empty, Inputs(Listed("date-default-null")).Single().DateText);
    }

    [Fact]
    public void Every_language_the_file_wrote_its_texts_in_is_offered()
    {
        var preset = Listed("texts-many-languages");

        Assert.Equal("Viele Sprachen", preset.LocalizedName("de"));
        Assert.Equal("Many languages", preset.LocalizedName("xx")); // not text in the file - English
    }

    [Fact]
    public void The_calendar_of_a_market_comes_from_the_engine()
    {
        Assert.Equal("pl", Listed("market-pl").Calendar);
        Assert.Null(Listed("market-unknown").Calendar);
        Assert.Equal("us-banking", TestCatalogues.ShippedPreset("payment-due-business-days").Calendar);
    }

    [Fact]
    public void Substitution_only_presets_are_listed_but_not_offered_by_the_calculator()
    {
        var shipped = TestCatalogues.Shipped().Presets.Select(PresetInfo.From).ToList();

        var rollover = shipped.Single(p => p.Id == "year-rollover");
        Assert.False(rollover.ForCalculator);
        Assert.True(rollover.ForSubstitution);
        Assert.Contains(shipped, p => p.IsParametric);
    }

    [Fact]
    public void An_unknown_culture_falls_back_to_english()
        => Assert.Equal("Last day of month", TestCatalogues.ShippedPreset("month-end").LocalizedName("xx"));

    [Fact]
    public async Task The_calculator_and_the_panel_share_one_read()
    {
        var source = new FakePresetSource(TestCatalogues.Shipped());
        var library = new PresetLibrary(source);

        _ = await library.ReadAsync();
        _ = await library.ReadAsync();

        Assert.Equal(1, source.Reads);
    }

    /// <summary>An engine quarantined for a moment must not leave the lists empty for the life of the window.</summary>
    [Fact]
    public async Task A_failed_read_is_not_kept_and_the_next_ask_tries_again()
    {
        var source = FakePresetSource.Failing("cannot launch 'x.exe': not found (calc.launch_failed)");
        var library = new PresetLibrary(source);

        await Assert.ThrowsAsync<CalcException>(library.ReadAsync);
        await Assert.ThrowsAsync<CalcException>(library.ReadAsync);

        Assert.Equal(2, source.Reads);
    }

    [Fact]
    public async Task A_library_with_no_source_is_an_empty_catalogue()
    {
        var catalogue = await new PresetLibrary(null).ReadAsync();

        Assert.Empty(catalogue.Presets);
        Assert.Empty(catalogue.Refused);
    }

    [Fact]
    public async Task Closing_the_window_stops_a_read_in_flight()
    {
        var library = new PresetLibrary(new HangingPresetSource());
        var read = library.ReadAsync();

        library.Abandon();

        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => read);
    }

    /// <summary>D2: files the engine refused are counted beside the list, with each file and its reason on
    /// the tooltip - a tester who wrote one and does not see it learns it was read and refused.</summary>
    [Fact]
    public void The_calculators_list_says_how_many_files_were_left_out()
    {
        var vm = new CalculatorViewModel(new FakeCalcEngine(), TestCatalogues.Library(TestCatalogues.TestCatalogue()));

        // The list is read before the first result is asked for, and the fake catalogue answers at once -
        // the result itself waits on an engine that never answers here, and is not what is asserted.
        _ = vm.EnsureComputedAsync();

        Assert.True(vm.PresetStatus.IsReady);
        Assert.True(vm.PresetStatus.HasLeftOut);
        Assert.Equal(30, vm.PresetStatus.LeftOut);
        Assert.Contains("step-to-zone.json - ", vm.PresetStatus.LeftOutFiles, StringComparison.Ordinal);
        Assert.All(vm.Presets, p => Assert.True(p.Info.ForCalculator));
        Assert.DoesNotContain(vm.Presets, p => p.Info.Id == "time-mode-x60"); // substitution only
    }

    /// <summary>
    /// D2: the line counting the files left out is there only when some were, and when none were it is
    /// gone WHOLE. Measured on the part, not read off the binding (GUI rule 10): only the text inside used
    /// to collapse, so the part kept its margin under every list and a screen reader still found it,
    /// named "Preset files left out: 0" - read on the live window with the shipped catalogue.
    /// </summary>
    [Fact]
    public void The_left_out_line_is_gone_whole_when_no_file_was_left_out()
    {
        // Built in one pass and measured in the next, so every binding has its value before the measure.
        var (clean, refused) = WpfTestHost.InvokeSettled(() =>
        {
            var nothing = new CatalogueStatus();
            nothing.Ready(14, []);
            var something = new CatalogueStatus();
            something.Ready(14, [new RefusedPresetFile("hand-written.json", "parameter 'n': unknown unit 'fortnights'")]);
            return (Note(nothing), Note(something));
        });
        var (none, some) = WpfTestHost.InvokeSettled(() => (Laid(clean), Laid(refused)));

        Assert.Equal((System.Windows.Visibility.Collapsed, 0.0), none);
        Assert.Equal(System.Windows.Visibility.Visible, some.Visibility);
        Assert.True(some.Height > 0, "not vacuous: with a file left out the line takes room");

        // A margin like the views give it, so a part that stays while its text collapses shows as height.
        static Controls.CatalogueLeftOutNote Note(CatalogueStatus status)
            => new() { DataContext = status, Margin = new System.Windows.Thickness(4) };

        static (System.Windows.Visibility Visibility, double Height) Laid(Controls.CatalogueLeftOutNote note)
        {
            note.Measure(new System.Windows.Size(400, 400));
            return (note.Visibility, note.DesiredSize.Height);
        }
    }

    [Fact]
    public void A_list_that_could_not_be_read_says_why_and_shows_nothing()
    {
        const string EngineSaid = "cannot launch 'C:\\x\\chrono.exe': not found (calc.launch_failed)";

        // On the window's thread, where the strings are always loaded. Off it they are there only once some
        // other test has started the shared host, and test classes run side by side - so a host started in
        // the middle of this test gave the status the engine's sentence and the expectation the translation.
        var seen = WpfTestHost.Invoke(() =>
        {
            var library = new PresetLibrary(FakePresetSource.Failing(EngineSaid));
            var vm = new CalculatorViewModel(new FakeCalcEngine(), library);

            // The list is read before the first result is asked for, and the fake catalogue answers at once -
            // the result itself waits on an engine that never answers here, and is not what is asserted.
            _ = vm.EnsureComputedAsync();

            return new
            {
                vm.PresetStatus.HasFailed,
                vm.PresetStatus.Failure,
                vm.PresetStatus.FailureReason,
                vm.PresetStatus.FailureDetail,
                vm.PresetStatus.HasFailureDetail,
                vm.PresetStatus.HasLeftOut,
                Presets = vm.Presets.Count,
                Failed = TranslationKeyConverter.Resolve("scenario.list_failed"),
                Reason = CalcErrorText.Describe(EngineSaid, TranslationKeyConverter.Resolve),
            };
        });

        Assert.True(seen.HasFailed);
        Assert.NotEqual("scenario.list_failed", seen.Failed); // not vacuous: the strings are loaded
        Assert.Equal(seen.Failed, seen.Failure);
        // The reason the calculator itself would give, translated, on its own line - and under it the engine's
        // sentence with the path of the engine, which is what fixing it needs.
        Assert.NotEqual(CalcErrorText.Detail(EngineSaid), seen.Reason); // not vacuous: the key translated
        Assert.Equal(seen.Reason, seen.FailureReason);
        Assert.True(seen.HasFailureDetail);
        Assert.Equal(CalcErrorText.Detail(EngineSaid), seen.FailureDetail);
        Assert.Contains("chrono.exe", seen.FailureDetail, StringComparison.Ordinal);
        Assert.Equal(0, seen.Presets);
        Assert.False(seen.HasLeftOut);
    }

    [Fact]
    public void A_list_starts_as_reading_and_never_as_empty()
    {
        var status = new CatalogueStatus();

        Assert.True(status.IsReading);
        Assert.False(status.IsEmpty);
        Assert.False(status.HasFailed);
        Assert.False(status.HasLeftOut);
    }

    [Fact]
    public void A_list_read_with_nothing_to_show_says_it_is_empty()
    {
        var status = new CatalogueStatus();

        status.Ready(0, []);

        Assert.True(status.IsEmpty);
        Assert.False(status.HasLeftOut);
    }

    [Theory]
    [InlineData("Last day of quarter", "reporting on the last day", "quarter", true)]
    [InlineData("Unix epoch zero", "treats Unix epoch zero as a real date", "epoch", true)]
    [InlineData("2038 boundary", "survives the 32-bit boundary", "EPOCH", false)]
    [InlineData("Last day of month", "month-end closing", "", true)]
    [InlineData("Payment due", "N business days out", "  business  ", true)]
    [InlineData("Payment due", "N business days out", "quarter", false)]
    public void The_filter_matches_name_or_explains_case_insensitively(
        string name, string explains, string filter, bool expected)
        => Assert.Equal(expected, CalculatorViewModel.PresetMatchesFilter(name, explains, filter));
}
