using System.Windows;
using System.Windows.Automation.Peers;
using System.Windows.Input;
using System.Windows.Media;
using ChronoMock.App.Calc;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// What a keyboard user hears on every place Tab can land (R5-U2), measured on the automation peers of
/// rendered screens rather than read from the markup.
/// </summary>
/// <remarks>
/// A Tab stop fails in two ways. It has no automation element at all, so the focus lands on something a
/// screen reader can only call the window - a container that takes the focus for nothing. Or its element
/// has no name, so the reader says "combo box" or "edit" and never which one. A third check reads the
/// stops of one screen together: the same name on two of them ("Copy", "Copy") tells the listener nothing
/// about which is which.
///
/// <see cref="ItemPeerNameTests"/> asks the same question of list ROWS. This asks it of the stops between
/// them, where the rows' answer does not reach.
/// </remarks>
public sealed class KeyboardStopNameTests
{
    /// <summary>A literal floor, so a walk that stopped reaching the controls cannot pass over nothing.</summary>
    private const int StopsAtLeast = 150;

    private const int CatalogueWidth = 900;

    private const int CatalogueHeight = 7168;

    /// <summary>How long an answer may take to land through the calculator's edit quiet period.</summary>
    private static readonly TimeSpan AnswerDeadline = TimeSpan.FromSeconds(5);

    [Fact]
    public async Task Every_place_tab_can_land_is_named_and_no_two_on_a_screen_share_a_name()
    {
        var (stops, offenders) = await WpfTestHost.RunAsync(async () =>
        {
            int stops = 0;
            var offenders = new List<string>();
            await foreach (var (screen, view, width, height, isCatalogue) in Screens())
            {
                LayoutProbe.Settle(view, width, height);
                var named = new List<(string Name, string Type)>();
                foreach (var stop in TabStops(view))
                {
                    stops++;
                    string what = Describe(stop);
                    var peer = UIElementAutomationPeer.CreatePeerForElement(stop);
                    if (peer is null)
                    {
                        offenders.Add($"  {screen} / {what}: takes the focus but has no automation element");
                        continue;
                    }

                    // Read as the row guard reads a row: empty, a translation key, a record's printout or a
                    // type name is no name. A history row named by its record read unique and non-empty.
                    string name = peer.GetName() ?? string.Empty;
                    if ((!isCatalogue || IsOurs(stop, view)) && ItemPeerNameTests.Unreadable(name) is { } why)
                    {
                        offenders.Add($"  {screen} / {what}: \"{name}\" ({why})");
                        continue;
                    }

                    named.Add((name, peer.GetAutomationControlType().ToString()));
                }

                if (!isCatalogue)
                {
                    offenders.AddRange(named
                        .GroupBy(n => n)
                        .Where(g => g.Count() > 1)
                        .Select(g => $"  {screen}: {g.Count()} stops are all called \"{g.Key.Name}\" ({g.Key.Type})"));
                }
            }

            return (stops, offenders);
        });

        Assert.True(
            stops >= StopsAtLeast,
            $"only {stops} tab stops were reached, expected at least {StopsAtLeast} - the screens stopped "
                + "rendering their controls, so this guard was about to pass over nothing");
        Assert.True(
            offenders.Count == 0,
            "a keyboard user lands on these and cannot tell what they are:" + Environment.NewLine
                + string.Join(Environment.NewLine, offenders.Distinct()));
    }

    /// <summary>Every element under the root that Tab can land on: shown all the way up, focusable and a
    /// tab stop. A container that keeps Tab out (KeyboardNavigation None) keeps its children out too.</summary>
    private static IEnumerable<UIElement> TabStops(DependencyObject node)
    {
        int count = VisualTreeHelper.GetChildrenCount(node);
        for (int i = 0; i < count; i++)
        {
            if (VisualTreeHelper.GetChild(node, i) is not UIElement child || child.Visibility != Visibility.Visible)
            {
                continue;
            }

            if (child.Focusable && KeyboardNavigation.GetIsTabStop(child))
            {
                yield return child;
            }

            if (KeyboardNavigation.GetTabNavigation(child) == KeyboardNavigationMode.None)
            {
                continue;
            }

            foreach (var nested in TabStops(child))
            {
                yield return nested;
            }
        }
    }

    /// <summary>Whether the stop is one of this interface's own components, or a part of one, rather than a
    /// toolkit control the catalogue lays out bare. A bare text box or drop-down is named where a screen uses
    /// it, so the catalogue's demonstrations of them are judged on the screens instead - but a component that
    /// names itself (the search field, a section's toggle) is judged wherever it stands.</summary>
    private static bool IsOurs(UIElement stop, DependencyObject screen)
    {
        if (stop is FrameworkElement { TemplatedParent: not null } || IsOwnType(stop))
        {
            return true;
        }

        for (var up = VisualTreeHelper.GetParent(stop); up is not null && up != screen; up = VisualTreeHelper.GetParent(up))
        {
            if (IsOwnType(up))
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsOwnType(DependencyObject element)
        => element.GetType().Namespace == typeof(Controls.SearchBox).Namespace;

    /// <summary>The stop by its x:Name when it has one, else by its type and the nearest named ancestor.</summary>
    private static string Describe(UIElement stop)
    {
        string type = stop.GetType().Name;
        if (stop is FrameworkElement { Name.Length: > 0 } named)
        {
            return $"{named.Name} ({type})";
        }

        for (var up = VisualTreeHelper.GetParent(stop); up is not null; up = VisualTreeHelper.GetParent(up))
        {
            if (up is FrameworkElement { Name.Length: > 0 } holder)
            {
                return $"{type} under {holder.Name}";
            }
        }

        return type;
    }

    /// <summary>The screens, each with the sections that hold its controls opened. The catalogue shows one
    /// component in many states side by side, so its names repeat by design and are not read together, and
    /// it lays toolkit controls out bare (see <see cref="IsOurs"/>).</summary>
    private static async IAsyncEnumerable<(string Name, FrameworkElement View, int Width, int Height, bool IsCatalogue)> Screens()
    {
        yield return ("catalogue", new ComponentCatalogue(), CatalogueWidth, CatalogueHeight, true);
        yield return ("setup", Opened(new SetupPhaseView { DataContext = PhaseStates.SetupStartup() }, "ScenarioSection", "SpeedSection"), LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);
        yield return ("setup with every option", Opened(new SetupPhaseView { DataContext = PhaseStates.SetupWithEveryOption() }, "ScenarioSection", "SpeedSection"), LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);
        yield return ("session with the audit open", Opened(new SessionPhaseView { DataContext = PhaseStates.WithTarget(SessionStates.RunningWithCoverageWarnings()) }, "AuditSection"), LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);
        yield return ("result with processes", Opened(new ResultPhaseView { DataContext = PhaseStates.ResultPartialWithUncoveredProcesses() }, "AuditSection"), LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);
        yield return ("result with the history open", Opened(new ResultPhaseView { DataContext = PhaseStates.ResultWithHistoryChosen() }, "HistorySection"), LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);

        // The calculator with a specific base and one step of every kind, so every editor is on the screen.
        var builder = new CalculatorViewModel(
            new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
        await builder.EnsureComputedAsync();
        builder.SelectedBase = builder.BaseKinds.Single(b => b.Kind == BaseKind.Specific);
        foreach (var kind in Enum.GetValues<StepKind>())
        {
            builder.AddStep();
            builder.Steps[^1].SelectedKind = builder.Steps[^1].Kinds.Single(k => k.Kind == kind);
        }

        await AnsweredAsync(builder);
        Assert.True(
            builder.HasResult && builder.Formats.Count > 0,
            "the builder screen shows no format rows, so no format's Copy would be read");
        yield return ("calculator with every step", new CalculatorView { DataContext = builder }, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);

        // Every shipped scenario, for the parameter inputs each one brings.
        var scenarios = new CalculatorViewModel(
            new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
        await scenarios.EnsureComputedAsync();
        foreach (var preset in scenarios.Presets.ToList())
        {
            scenarios.SelectedPreset = preset;
            await AnsweredAsync(scenarios);
            yield return ($"calculator with {preset.Info.Id}", new CalculatorView { DataContext = scenarios }, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight, false);
        }
    }

    private static FrameworkElement Opened(FrameworkElement view, params string[] sections)
    {
        foreach (var section in sections)
        {
            var expander = LayoutProbe.FindNamed(view, section) as System.Windows.Controls.Expander
                ?? throw new InvalidOperationException($"{view.GetType().Name} has no section called {section}");
            expander.IsExpanded = true;
        }

        return view;
    }

    /// <summary>Wait until the result for the builder as it stands is on the screen, so the format rows and
    /// their copy buttons are there to be read. A scenario that answers with an error leaves no rows, which
    /// is a state of its own and still read - but an answer that never lands fails here rather than leaving
    /// the screen read without its rows (found in review: the wait used to end in silence).</summary>
    private static async Task AnsweredAsync(CalculatorViewModel vm)
    {
        var deadline = DateTime.UtcNow + AnswerDeadline;
        while (DateTime.UtcNow < deadline && (vm.IsResultStale || !vm.HasResult))
        {
            await Task.Delay(TimeSpan.FromMilliseconds(25), TestContext.Current.CancellationToken);
        }

        Assert.False(vm.IsResultStale, "the calculator never answered, so its copy buttons would not be read");
    }
}
