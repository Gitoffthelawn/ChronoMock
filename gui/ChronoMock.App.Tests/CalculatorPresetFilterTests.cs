using System.Windows.Controls;
using ChronoMock.App.Calc;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// R5-U7: the calculator's scenario list under its search box. The list was emptied and refilled row by row at
/// every keystroke, which took the chosen row with it though it still matched, and a row that came back unmarked
/// filled the parameters again from their defaults when pressed. Measured on the real list, not on the view
/// model alone: it is the list that lets go of a selection it can no longer show (GUI rule 10). Every step is
/// its own pass of the window's thread, because the list takes its rows from a binding the dispatcher runs.
/// </summary>
public class CalculatorPresetFilterTests
{
    private const string PaymentDue = "payment-due-business-days";

    private static (CalculatorViewModel Vm, ListBox List) Shown() => WpfTestHost.InvokeSettled(() =>
    {
        var vm = new CalculatorViewModel(new FakeCalcEngine(), TestCatalogues.Library(TestCatalogues.Shipped()));
        var view = new CalculatorView { DataContext = vm };
        _ = vm.EnsureComputedAsync();
        return (vm, (ListBox)view.FindName("PresetList"));
    });

    private static T Settled<T>(Func<T> step) => WpfTestHost.InvokeSettled(step);

    private static string? Marked(ListBox list) => Settled(() => (list.SelectedItem as PresetItemViewModel)?.Info.Id);

    /// <summary>Reversal probe: empty and refill the list at each keystroke, as it was, and the mark goes.</summary>
    [Fact]
    public void A_chosen_scenario_that_still_matches_stays_chosen_while_the_tester_types()
    {
        var (vm, list) = Shown();
        var chosen = Settled(() => vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == PaymentDue));
        var dropped = 0;
        Settled(() =>
        {
            list.SelectionChanged += (_, e) => dropped += e.RemovedItems.Count;
            return 0;
        });

        foreach (var typed in new[] { "P", "Pa", "Pay", "Paym" })
        {
            Settled(() => vm.PresetFilter = typed);
        }

        Assert.Same(chosen, Settled(() => vm.SelectedPreset));
        Assert.Equal(PaymentDue, Marked(list));
        Assert.Equal(0, dropped);
    }

    /// <summary>A search that hid the chosen scenario gives its row back marked, and the parameters typed for
    /// it stay as typed. Reversal probe: drop the re-choosing and the row comes back unmarked.</summary>
    [Fact]
    public void A_scenario_hidden_by_the_search_comes_back_chosen_with_its_parameters_as_typed()
    {
        var (vm, list) = Shown();
        Settled(() => vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == PaymentDue));
        Settled(() => vm.ParamInputs[0].Amount = "45");

        Settled(() => vm.PresetFilter = "no scenario is called this");
        Assert.True(Settled(() => vm.HasNoPresetMatches));
        Assert.Null(Marked(list)); // the list let go of a row it cannot show - not vacuous
        Assert.Equal(System.Windows.Visibility.Collapsed, Settled(() => list.Visibility)); // no empty strip

        Settled(() => vm.PresetFilter = string.Empty);

        Assert.Equal(PaymentDue, Settled(() => vm.SelectedPreset?.Info.Id));
        Assert.Equal(PaymentDue, Marked(list));
        Assert.Equal("45", Settled(() => vm.ParamInputs[0].Amount));
        Assert.True(Settled(() => vm.HasActivePreset));
        Assert.False(Settled(() => vm.HasNoPresetMatches));
    }

    /// <summary>Enter in the search box chooses the first scenario left standing, as in the session's list -
    /// matched on the line under the name as well (R5-D67).</summary>
    [Fact]
    public void Enter_chooses_the_first_scenario_the_search_left_standing()
    {
        var vm = new CalculatorViewModel(new FakeCalcEngine(), TestCatalogues.Library(TestCatalogues.Shipped()));
        _ = vm.EnsureComputedAsync();
        var payment = vm.Presets.Single(p => p.Info.Id == PaymentDue);
        var word = payment.DisplayExplains.Split(' ')
            .First(w => w.Length > 6 && !payment.DisplayName.Contains(w, StringComparison.CurrentCultureIgnoreCase));

        vm.PresetFilter = word;
        vm.ChooseFirstPreset.Execute(null);

        Assert.Contains(vm.Presets, p => p.Info.Id == PaymentDue);
        Assert.Same(vm.Presets[0], vm.SelectedPreset);
    }
}
