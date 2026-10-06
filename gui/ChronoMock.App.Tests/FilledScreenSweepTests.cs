using System.Windows;
using System.Windows.Controls;
using ChronoMock.App.Calc;
using ChronoMock.App.Localization;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// PR B finding (b): both screens FILLED, at every window size the user can drag them to, in every language
/// the window speaks.
/// </summary>
/// <remarks>
/// 🔴 THE SIZE SWEEP MEASURED THE CALCULATOR EMPTY. <see cref="LayoutGuardTests"/> lays out a
/// <c>new CalculatorView()</c> with no view model, so no scenario list, no parameters, no answer and no
/// formats were ever on the surface - and neither the trimmed scenario name nor the cut "business days"
/// was ever measured by it. The panel was swept in its startup state only. Here the calculator has the
/// shipped catalogue, every scenario chosen in turn with its parameters filled and its answer computed, and
/// the panel shows each phase with the section that holds the state opened.
///
/// 🔴 EACH STATE GETS A FRESH VIEW, AND THE SIZES ARE WALKED NARROWEST FIRST. A view laid out wide and then
/// narrow keeps its scenario rows at the wide width (the list's virtualising panel arranges them by the
/// widest it has seen, measured 501 px rows in a 202 px list) - invisible on the screen, because the text is
/// measured at the right width, but visible to UI Automation. That is a separate fault, kept on the open
/// list with its measurement, and this sweep does not cover the path "narrowed after widened" - saying so
/// is the point of this paragraph. Widening is safe, which is what lets one view serve every size (a fresh
/// window per size cost 55 s for the two sweeps, measured).
///
/// The empty states stay in <see cref="LayoutGuardTests"/>: this class adds the filled ones, and a new class
/// rather than new tests there, because that class stands on its coupling ceiling.
/// </remarks>
public sealed class FilledScreenSweepTests
{
    /// <summary>Eleven shipped scenarios today, and the state with none chosen - a canary against a
    /// catalogue that stopped loading.</summary>
    private const int CalculatorStatesAtLeast = 10;

    /// <summary>Fourteen phase states today - a canary against a list that stopped being swept.</summary>
    private const int PanelStatesAtLeast = 12;

    /// <summary>The swept sizes, never narrower than the one before (see the remarks on this class).</summary>
    private static readonly IReadOnlyList<SizeSweep.Step> NarrowestFirst =
        [.. SizeSweep.Sizes.OrderBy(s => s.Width).ThenBy(s => s.Height)];

    /// <summary>How long an answer to a chosen scenario may take to land through the edit quiet period.</summary>
    private static readonly TimeSpan AnswerDeadline = TimeSpan.FromSeconds(5);

    [Fact]
    public async Task The_calculator_gives_way_nowhere_in_any_scenario_at_any_size_in_any_language()
    {
        var (complaints, states, unanswered) = await WpfTestHost.RunAsync(async () =>
        {
            var found = new List<string>();
            var measured = 0;
            var noAnswer = new List<string>();
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    var vm = new CalculatorViewModel(
                        new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
                    await vm.EnsureComputedAsync();
                    var stateNames = new List<string> { "no scenario" };
                    stateNames.AddRange(vm.Presets.Select(p => p.Info.Id));
                    foreach (var state in stateNames)
                    {
                        if (state != "no scenario")
                        {
                            vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == state);
                            FillEmptyParameters(vm);
                            if (!await AnsweredAsync(vm))
                            {
                                noAnswer.Add($"{culture} {state}");
                            }
                        }

                        var view = new CalculatorView { DataContext = vm };
                        foreach (var size in NarrowestFirst)
                        {
                            found.AddRange(SizeSweep.Inspect(view, size)
                                .Select(c => $"{culture} {state} at {SizeSweep.Describe(size)}: {c}"));
                        }

                        measured++;
                    }
                }
            }
            finally
            {
                LocalizationService.Apply(Application.Current, LocalizationService.DefaultCulture);
            }

            return (found, measured, noAnswer);
        });

        Assert.True(states >= CalculatorStatesAtLeast * 2, $"only {states} calculator states were swept");
        Assert.True(unanswered.Count == 0, "measured without an answer on the screen: " + string.Join(", ", unanswered));
        Assert.True(complaints.Count == 0, string.Join("\n", complaints));
    }

    [Fact]
    public void The_panel_gives_way_nowhere_in_any_phase_at_any_size_in_any_language()
    {
        var (complaints, states) = WpfTestHost.InvokeSettled(() =>
        {
            var found = new List<string>();
            var measured = 0;
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    foreach (var (name, make, open) in PanelStates())
                    {
                        var root = (FrameworkElement)new MainWindow().Content;
                        root.DataContext = make();
                        Open(root, open);
                        foreach (var size in NarrowestFirst)
                        {
                            found.AddRange(SizeSweep.Inspect(root, size)
                                .Select(c => $"{culture} {name} at {SizeSweep.Describe(size)}: {c}"));
                        }

                        measured++;
                    }
                }
            }
            finally
            {
                LocalizationService.Apply(Application.Current, LocalizationService.DefaultCulture);
            }

            return (found, measured);
        });

        Assert.True(states >= PanelStatesAtLeast * 2, $"only {states} panel states were swept");
        Assert.True(complaints.Count == 0, string.Join("\n", complaints));
    }

    /// <summary>
    /// The text and spacing rules over the FILLED calculator: every line reaches its contrast floor, every
    /// size is on the type scale, every gap of ours is on the spacing scale - in every scenario, with its
    /// answer, in every language. The same rules ran over the empty calculator only.
    /// </summary>
    [Fact]
    public async Task Every_text_and_gap_of_the_filled_calculator_keeps_to_the_scales_in_every_scenario()
    {
        var (findings, readings) = await WpfTestHost.RunAsync(async () =>
        {
            var found = new List<string>();
            var lowest = int.MaxValue;
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    var vm = new CalculatorViewModel(
                        new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
                    await vm.EnsureComputedAsync();
                    foreach (var preset in vm.Presets.ToList())
                    {
                        vm.SelectedPreset = preset;
                        FillEmptyParameters(vm);
                        await AnsweredAsync(vm);
                        var view = new CalculatorView { DataContext = vm };
                        LayoutProbe.Settle(view);
                        var walk = LayoutProbe.Walk(view);
                        var texts = ContrastReport.Measure(view, walk, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
                        lowest = Math.Min(lowest, texts.Count);
                        var where = $"{culture} {preset.Info.Id}";
                        found.AddRange(texts.Where(r => r.TooFaint).Select(r =>
                            $"{where}: {r.Label} \"{r.Text}\" at {r.FontSize}px reads {r.Ratio} against {r.Paper}, needs {r.Required}"));
                        found.AddRange(texts.Where(r => r.OffScale).Select(r =>
                            $"{where}: {r.Label} \"{r.Text}\" uses {r.FontSize}px, which is not on the scale"));
                        found.AddRange(SpacingReport.Measure(walk).Where(g => g.OffScale && g.Ours).Select(g =>
                            $"{where}: {g.Size}px under {g.Where} between {g.Before} and {g.After}"));
                    }
                }
            }
            finally
            {
                LocalizationService.Apply(Application.Current, LocalizationService.DefaultCulture);
            }

            return (found, lowest);
        });

        // Measured: 50 pieces of text in the sparsest filled scenario, 28 on the empty calculator.
        Assert.True(readings >= TextReadingsAtLeast, $"a filled scenario yielded only {readings} pieces of text");
        Assert.True(findings.Count == 0, string.Join("\n", findings));
    }

    /// <summary>Well under the 50 measured in the sparsest filled scenario - a canary against a screen
    /// that stopped being read.</summary>
    private const int TextReadingsAtLeast = 35;

    /// <summary>
    /// Every zone either zone field offers, in every language, reads whole in the closed box at the
    /// narrowest the screen gets. The sweeps above see only the zone a state happens to have chosen.
    /// </summary>
    /// <remarks>
    /// Found by the filled sweep the first time it read the window in Polish: "UTC+00:00 · Uniwersalny czas
    /// koordynowany" needed 285 px and the box left 270, in the setup at every window size and in the
    /// calculator at its floor. The Polish gloss is "Czas uniwersalny" now (owner's decision, 2026-10-06).
    /// Reversal probe: put the long gloss back in Strings.pl.json and this reddens on UTC in both fields.
    /// </remarks>
    [Fact]
    public void Every_zone_reads_whole_in_both_zone_fields_in_every_language()
    {
        var (complaints, zones) = WpfTestHost.InvokeSettled(() =>
        {
            var found = new List<string>();
            var measured = 0;
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    var session = PhaseStates.SetupStartup();
                    var setup = new SetupPhaseView { DataContext = session };
                    foreach (var zone in session.Zones.ToList())
                    {
                        session.SelectedZone = zone;
                        LayoutProbe.Settle(setup, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
                        found.AddRange(Cut(setup).Select(c => $"{culture} setup, {zone.Label}: {c}"));
                        measured++;
                    }

                    var calculator = new CalculatorViewModel(new FakeCalcEngine(CalculatorSheetEngine.Answer));
                    var view = new CalculatorView { DataContext = calculator };
                    var floor = (int)(double)view.FindResource("CalculatorMinWidth");
                    foreach (var zone in calculator.BaseZones.ToList())
                    {
                        calculator.SelectedBaseZone = zone;
                        LayoutProbe.Settle(view, floor, LayoutProbe.WindowHeight);
                        found.AddRange(Cut(view).Select(c => $"{culture} calculator, {zone.Label}: {c}"));
                        measured++;
                    }
                }
            }
            finally
            {
                LocalizationService.Apply(Application.Current, LocalizationService.DefaultCulture);
            }

            return (found, measured);
        });

        Assert.True(zones >= ZoneChoicesAtLeast, $"only {zones} zone choices were laid out");
        Assert.True(complaints.Count == 0, string.Join("\n", complaints));
    }

    /// <summary>Eight zones in each of two fields in each of two languages today, 32 in all.</summary>
    private const int ZoneChoicesAtLeast = 24;

    private static IEnumerable<string> Cut(FrameworkElement root)
    {
        var walk = LayoutProbe.Walk(root);
        return LayoutRules.SpillsOutOfItsParent(walk).Concat(LayoutRules.TrimmedAway(walk));
    }

    /// <summary>The phases in the states the state sheet draws, each with the section that holds the state
    /// opened, as the window shows them.</summary>
    private static IEnumerable<(string Name, Func<SessionViewModel> Make, string? Open)> PanelStates()
    {
        yield return ("setup at startup", PhaseStates.SetupStartup, null);
        yield return ("setup with the scenarios open", PhaseStates.SetupStartup, "ScenarioSection");
        yield return ("setup with every option", PhaseStates.SetupWithEveryOption, "SpeedSection");
        yield return ("setup with a bad date", PhaseStates.SetupWithBadDate, null);
        yield return ("setup configured", PhaseStates.SetupConfigured, null);
        yield return ("session running", () => PhaseStates.WithTarget(SessionStates.Running()), null);
        yield return ("session with the audit open", () => PhaseStates.WithTarget(SessionStates.RunningWithCoverageWarnings()), "AuditSection");
        yield return ("session with a failed command", () => PhaseStates.WithTarget(SessionStates.InFlightError()), null);
        yield return ("session ended", () => PhaseStates.WithTarget(SessionStates.Ended()), null);
        yield return ("result that worked", PhaseStates.ResultWorks, null);
        yield return ("result with processes on the real clock", PhaseStates.ResultPartialWithUncoveredProcesses, null);
        yield return ("result refused, processes left running", PhaseStates.ResultRefusedLeftRunning, null);
        yield return ("result that did not take effect", PhaseStates.ResultVanished, null);
        yield return ("result with the history open", PhaseStates.ResultWithHistoryChosen, "HistorySection");
    }

    /// <summary>Opens the named section, and fails loudly when it is not there - a state that stayed folded
    /// would be swept as the startup state under another name.</summary>
    private static void Open(FrameworkElement root, string? section)
    {
        if (section is null)
        {
            return;
        }

        var expander = LayoutProbe.FindNamed(root, section) as Expander
            ?? throw new InvalidOperationException($"the window has no section called {section}");
        expander.IsExpanded = true;
    }

    /// <summary>A sample value in every parameter the scenario leaves empty, so the scenario is answered and
    /// the answer is on the screen being measured - a scenario waiting for its parameters shows no formats.</summary>
    private static void FillEmptyParameters(CalculatorViewModel vm)
    {
        foreach (var input in vm.ParamInputs)
        {
            if (input.IsDate && string.IsNullOrEmpty(input.DateText))
            {
                input.DateText = "2026-03-01";
            }

            if (input.IsDuration && string.IsNullOrEmpty(input.Amount))
            {
                input.Amount = "30";
            }

            if (input.IsVariant && input.Variant is null)
            {
                input.Variant = input.VariantOptions[0];
            }
        }
    }

    /// <summary>Wait for the answer to the current input, polling rather than sleeping a fixed time.</summary>
    private static async Task<bool> AnsweredAsync(CalculatorViewModel vm)
    {
        var deadline = DateTime.UtcNow + AnswerDeadline;
        while (DateTime.UtcNow < deadline)
        {
            if (!vm.IsResultStale && vm.HasResult && vm.Formats.Count > 0)
            {
                return true;
            }

            await Task.Delay(TimeSpan.FromMilliseconds(25), TestContext.Current.CancellationToken);
        }

        return false;
    }
}
