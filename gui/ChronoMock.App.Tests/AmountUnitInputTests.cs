using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;
using ChronoMock.App.Calc;
using ChronoMock.App.Controls;
using ChronoMock.App.Localization;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// PR B findings (c) and (h): the shared amount-and-unit input.
/// </summary>
public sealed class AmountUnitInputTests
{
    /// <summary>Nine units in each of two languages today.</summary>
    private const int UnitChoicesAtLeast = 16;

    /// <summary>
    /// 🔴 Every unit, chosen, reads whole in the closed box, in every language the window speaks - not only
    /// the unit a screen happened to show.
    /// </summary>
    /// <remarks>
    /// The list was 128 wide, a number picked for "the longest word plus its chevron", and the longest word
    /// grew past it: "business days" needed 85 px and got 78, so the window read "90 business day" in the
    /// builder and in a scenario's parameter. The list sizes itself from its labels now, and with only a
    /// floor it grows to whatever is chosen - so what this catches is a fixed width coming back.
    /// Reversal probe: give the unit list Width="128" in Controls/AmountUnitInput.xaml and this reddens on
    /// "business days". A chrome too narrow does not cut the chosen label, it lets the width move - that is
    /// the next test's to catch.
    /// </remarks>
    [Fact]
    public void Every_unit_reads_whole_when_chosen_in_every_language()
    {
        var (complaints, choices) = WpfTestHost.InvokeSettled(() =>
        {
            var found = new List<string>();
            var measured = 0;
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    var sample = new AmountUnitSample("90", "d");
                    var input = new AmountUnitInput { HasSign = true, DataContext = sample };
                    foreach (var unit in StepViewModel.AllUnits)
                    {
                        sample.Unit = unit;
                        input.DataContext = null;
                        input.DataContext = sample;
                        LayoutProbe.Settle(input, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
                        var walk = LayoutProbe.Walk(input);
                        var shown = walk.Any(e => e.Kind == "TextBlock" && e.IsVisible && e.Text == TranslationKeyConverter.Resolve(unit.LabelKey));
                        if (!shown)
                        {
                            found.Add($"{culture} {unit.Token}: the chosen unit is not on the closed box");
                        }

                        found.AddRange(LayoutRules.SpillsOutOfItsParent(walk)
                            .Concat(LayoutRules.TrimmedAway(walk))
                            .Select(c => $"{culture} {unit.Token}: {c}"));
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

        Assert.True(choices >= UnitChoicesAtLeast, $"only {choices} unit choices were laid out");
        Assert.True(complaints.Count == 0, string.Join("\n", complaints));
    }

    /// <summary>
    /// The list keeps its width whatever unit is chosen: it is as wide as the longest label, not the chosen
    /// one, so the row does not shift under the pointer when the choice changes.
    /// </summary>
    /// <remarks>
    /// Reversal probes, both measured: size the list from nothing (the input's MeasureOverride) and it
    /// follows the chosen label, or take 10 off the right of DropDownChrome in Themes/Values.xaml and it is
    /// wider only while "business days" is chosen.
    /// </remarks>
    [Fact]
    public void The_unit_list_keeps_its_width_whatever_is_chosen()
    {
        var widths = WpfTestHost.InvokeSettled(() =>
        {
            var sample = new AmountUnitSample("90", "d");
            var input = new AmountUnitInput { DataContext = sample };
            var seen = new HashSet<double>();
            foreach (var unit in StepViewModel.AllUnits)
            {
                sample.Unit = unit;
                input.DataContext = null;
                input.DataContext = sample;
                LayoutProbe.Settle(input, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
                seen.Add(Math.Round(UnitList(input).ActualWidth));
            }

            return seen;
        });

        Assert.True(widths.Count == 1, "the unit list changed width with the choice: " + string.Join(", ", widths));
    }

    /// <summary>
    /// Every part of the input has a name for a screen reader, led by the input's own name when it has one.
    /// The builder's shift step had none at all, and a scenario parameter's unit list had none.
    /// </summary>
    [Fact]
    public void Every_part_is_named_and_led_by_the_inputs_name()
    {
        var (plain, named) = WpfTestHost.InvokeSettled(() =>
        {
            static IReadOnlyList<string> Names(AmountUnitInput input)
            {
                LayoutProbe.Settle(input, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
                return [.. Parts(input).Select(AutomationProperties.GetName)];
            }

            return (
                Names(new AmountUnitInput { HasSign = true, DataContext = new AmountUnitSample("1", "d") }),
                Names(new AmountUnitInput { HasSign = true, AccessibleName = "Payment term", DataContext = new AmountUnitSample("1", "d") }));
        });

        Assert.Equal(["Direction", "Amount", "Unit"], plain);
        Assert.Equal(["Payment term, Direction", "Payment term, Amount", "Payment term, Unit"], named);
    }

    /// <summary>
    /// Every amount input the screens draw is named after the field it fills, so a screen reader hears whose
    /// amount and whose unit it is on: the run panel's "From now", the builder's shift step and a scenario's
    /// parameter. Two of the three had no name and read "Direction, Amount, Unit" alone (CodeRabbit on #95).
    /// Reversal probe: drop AccessibleName from the input in SetupPhaseView.xaml and this reddens.
    /// </summary>
    [Fact]
    public async Task Every_amount_input_on_the_screens_is_named_after_its_field()
    {
        var names = await WpfTestHost.RunAsync(async () =>
        {
            var setup = new SetupPhaseView { DataContext = PhaseStates.SetupStartup() };
            LayoutProbe.Settle(setup);
            var found = Shown(setup).Select(i => ("setup", i.AccessibleName)).ToList();

            var vm = new CalculatorViewModel(
                new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
            await vm.EnsureComputedAsync();
            vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == "payment-due-business-days");
            var calculator = new CalculatorView { DataContext = vm };
            LayoutProbe.Settle(calculator);
            found.AddRange(Shown(calculator).Select(i => ("calculator", i.AccessibleName)));
            return found;
        });

        // The setup's shift, the scenario's shift step in the builder and its duration parameter.
        Assert.True(names.Count >= 3, $"only {names.Count} amount inputs were on the screens");
        Assert.All(names, n => Assert.False(string.IsNullOrWhiteSpace(n.AccessibleName), $"an amount input in the {n.Item1} has no name"));
    }

    /// <summary>The amount inputs a screen shows - its own visibility, since nothing is visible without a window.</summary>
    private static IEnumerable<AmountUnitInput> Shown(DependencyObject node)
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(node); i++)
        {
            var child = VisualTreeHelper.GetChild(node, i);
            if (child is AmountUnitInput { Visibility: Visibility.Visible } input)
            {
                yield return input;
            }

            foreach (var deeper in Shown(child))
            {
                yield return deeper;
            }
        }
    }

    /// <summary>
    /// 🔴 The amount reaches the view model when the field is left, in every place the input stands (owner's
    /// decision, 2026-10-06). The setup committed on every keystroke and the calculator on leaving the field,
    /// and a calculator commit is a question to the engine - one per half-typed number.
    /// </summary>
    [Fact]
    public void The_amount_commits_when_the_field_is_left()
    {
        var (trigger, before, after) = WpfTestHost.InvokeSettled(() =>
        {
            var sample = new AmountUnitSample("1", "d");
            var input = new AmountUnitInput { DataContext = sample };
            LayoutProbe.Settle(input, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);
            var amount = (TextBox)Parts(input)[1];
            amount.Text = "45";
            var typed = sample.Amount;
            BindingOperations.GetBindingExpression(amount, TextBox.TextProperty)!.UpdateSource();
            return (BindingOperations.GetBinding(amount, TextBox.TextProperty)!.UpdateSourceTrigger, typed, sample.Amount);
        });

        Assert.Equal(UpdateSourceTrigger.LostFocus, trigger);
        Assert.Equal("1", before);
        Assert.Equal("45", after);
    }

    /// <summary>The sign, the amount and the unit list, in that order, whether or not the sign is shown.</summary>
    private static IReadOnlyList<Control> Parts(DependencyObject node)
    {
        var controls = new List<Control>();
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(node); i++)
        {
            var child = VisualTreeHelper.GetChild(node, i);
            if (child is ComboBox or TextBox)
            {
                // A part, and never the inside of one: the drop-down's own template holds no second box.
                controls.Add((Control)child);
            }
            else
            {
                controls.AddRange(Parts(child));
            }
        }

        return controls;
    }

    private static ComboBox UnitList(AmountUnitInput input) => (ComboBox)Parts(input)[^1];
}
