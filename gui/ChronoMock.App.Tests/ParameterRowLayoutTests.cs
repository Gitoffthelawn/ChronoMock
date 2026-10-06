using System.Windows;
using System.Windows.Controls;
using ChronoMock.App.Calc;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// R4/19 (Z1): a scenario's parameter rows at the calculator's narrowest.
/// </summary>
/// <remarks>
/// 🔴 Every size sweep measured the calculator with no view model, so no scenario was ever chosen and the
/// parameter rows were never on the surface. At the window's minimum width the row was clipped at the
/// result card's edge - the unit list lost its chevron and the date its calendar button, measured on the
/// live window - while every layout guard stayed green. And a sweep over a chosen scenario would not have
/// caught it either: the general rules exempt anything under a scrolling ancestor, and this column scrolls
/// only up and down, so a row pushed out sideways is exempt and still unreachable. So the measure here is
/// direct: at the calculator's own floor (CalculatorMinWidth, the width the window gives it at its minimum),
/// with each shipped scenario that has parameters, every visible part of the parameter block stays inside
/// the card's inner edge.
/// </remarks>
public sealed class ParameterRowLayoutTests
{
    /// <summary>Tall enough that nothing scrolls, so a complaint is about width alone.</summary>
    private const int Tall = 1600;

    /// <summary>Six shipped scenarios have parameters today - a canary against measuring none.</summary>
    private const int ScenariosAtLeast = 5;

    /// <summary>A layout rounding step, not a visible overlap.</summary>
    private const double Tolerance = 0.5;

    [Fact]
    public async Task Every_shipped_scenarios_parameters_stay_inside_the_card_at_the_calculators_narrowest()
    {
        var (complaints, measured) = await WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
            var view = new CalculatorView { DataContext = vm };
            await vm.EnsureComputedAsync();
            var floor = (int)(double)view.FindResource("CalculatorMinWidth");

            var found = new List<string>();
            var withParameters = 0;
            foreach (var preset in vm.Presets.Where(p => p.Info.IsParametric).ToList())
            {
                vm.SelectedPreset = preset;
                LayoutProbe.Settle(view, floor, Tall);
                var parts = OutsideTheCard(view);

                // Every parameter has at least one editor - a date box, an amount box, a choice - and the
                // block's heading is not one: counting the heading let this pass with no input laid out.
                Assert.True(
                    parts.Editors >= vm.ParamInputs.Count,
                    $"{preset.Info.Id}: {parts.Editors} editors laid out for {vm.ParamInputs.Count} parameters");
                withParameters++;
                found.AddRange(parts.Outside.Select(o => $"{preset.Info.Id}: {o}"));
            }

            return (found, withParameters);
        });

        Assert.True(measured >= ScenariosAtLeast, $"only {measured} scenarios with parameters were laid out");
        Assert.True(complaints.Count == 0, string.Join("\n", complaints));
    }

    /// <summary>
    /// The same screens held to the GENERAL layout rules, which since PR A finding (a) know which axis a
    /// scroll viewer scrolls and see a control cut in part. The direct measure above stays as well: it is
    /// the one that names the card's inner edge, and the general rules are the ones every other screen gets.
    /// Reversal probe: put the row from before R4/19 back (its label beside the field) and this reddens on
    /// the unit list cut at the card's edge - the defect the rules were blind to when it shipped.
    /// </summary>
    [Fact]
    public async Task The_general_layout_rules_find_nothing_in_any_scenarios_parameters_at_the_calculators_narrowest()
    {
        var (complaints, measured) = await WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
            var view = new CalculatorView { DataContext = vm };
            await vm.EnsureComputedAsync();
            var floor = (int)(double)view.FindResource("CalculatorMinWidth");

            var found = new List<string>();
            var laidOut = 0;
            foreach (var preset in vm.Presets.Where(p => p.Info.IsParametric).ToList())
            {
                vm.SelectedPreset = preset;
                LayoutProbe.Settle(view, floor, Tall);
                var elements = LayoutProbe.Walk(view);
                found.AddRange(LayoutRules.OutsideTheSurface(elements, new Size(floor, Tall))
                    .Concat(LayoutRules.PastAHardClip(elements))
                    .Concat(LayoutRules.PartlyPastAHardClip(elements))
                    .Concat(LayoutRules.SpillsOutOfItsParent(elements))
                    .Select(c => $"{preset.Info.Id}: {c}"));
                laidOut++;
            }

            return (found, laidOut);
        });

        Assert.True(measured >= ScenariosAtLeast, $"only {measured} scenarios with parameters were laid out");
        var unknown = complaints.Where(c => !KnownDefects.Any(k => IsKnown(c, k))).ToList();
        Assert.True(unknown.Count == 0, string.Join("\n", unknown));
        foreach (var defect in KnownDefects)
        {
            Assert.True(
                complaints.Any(c => IsKnown(c, defect)),
                $"{defect.Scenario}: the known defect no longer shows - take it off the list ({defect.Reason})");
        }
    }

    /// <summary>
    /// What the general rules found here the first time they could see it, each with the reason and what
    /// makes it go. Listed for the owner to decide rather than fixed inside the instrument's own change, and
    /// checked from both ends: a defect gone from the screen has to come off the list. Only shrinks.
    /// </summary>
    private static readonly (string Scenario, string Shows, string Reason)[] KnownDefects =
    [
        ("payment-due-business-days", "TextBlock \"business days\"",
         "the unit list is ShiftUnitWidth wide with 78 px for its text, and \"business days\" needs 85, so the "
         + "window reads \"90 business day\" - in the builder and in the parameter row, at every window width. "
         + "Goes with the amount and unit component (PR B, finding (c))"),
    ];

    private static bool IsKnown(string complaint, (string Scenario, string Shows, string Reason) defect)
        => complaint.StartsWith($"{defect.Scenario}: {defect.Shows} at ", StringComparison.Ordinal)
           && complaint.Contains(" spills out of ", StringComparison.Ordinal);

    /// <summary>The kinds a parameter is edited with: the date input's box, the amount, the unit and the choice.</summary>
    private static readonly HashSet<string> EditorKinds = new(StringComparer.Ordinal) { "TextBox", "ComboBox" };

    /// <summary>The visible parts of the parameter block that reach past the card's inner edge, and how
    /// many visible editors it laid out.</summary>
    private static (List<string> Outside, int Editors) OutsideTheCard(FrameworkElement view)
    {
        var elements = LayoutProbe.Walk(view);
        var block = IndexOf(elements, "PresetParameters");
        var cardElement = elements[IndexOf(elements, "ActivePresetCard")];
        var card = (Border)LayoutProbe.FindNamed(view, "ActivePresetCard")!;
        var edge = cardElement.Bounds;
        var inner = new Rect(
            edge.Left + card.Padding.Left + card.BorderThickness.Left,
            edge.Top,
            Math.Max(0, edge.Width - card.Padding.Left - card.Padding.Right - card.BorderThickness.Left - card.BorderThickness.Right),
            edge.Height);

        var outside = new List<string>();
        var editors = 0;
        for (int i = 0; i < elements.Count; i++)
        {
            var e = elements[i];
            if (!e.IsVisible || e.Bounds.Width <= Tolerance || !Within(elements, i, block))
            {
                continue;
            }

            if (EditorKinds.Contains(e.Kind))
            {
                editors++;
            }

            if (e.Bounds.Left < inner.Left - Tolerance || e.Bounds.Right > inner.Right + Tolerance)
            {
                outside.Add($"{e.Kind} '{e.Name}' at x {e.Bounds.Left:F0}..{e.Bounds.Right:F0}, card inner edge {inner.Left:F0}..{inner.Right:F0}");
            }
        }

        return (outside, editors);
    }

    private static int IndexOf(IReadOnlyList<LaidOutElement> elements, string name)
    {
        for (int i = 0; i < elements.Count; i++)
        {
            if (elements[i].Name == name)
            {
                return i;
            }
        }

        throw new InvalidOperationException($"'{name}' is not in the laid-out calculator");
    }

    private static bool Within(IReadOnlyList<LaidOutElement> elements, int index, int ancestor)
    {
        for (var at = elements[index].ParentIndex; at >= 0; at = elements[at].ParentIndex)
        {
            if (at == ancestor)
            {
                return true;
            }
        }

        return false;
    }
}
