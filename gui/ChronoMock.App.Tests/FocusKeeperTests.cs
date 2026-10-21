using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using ChronoMock.App.Calc;
using ChronoMock.App.Controls;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// The keyboard focus is put back on the screen when the element holding it goes away (R5-U4, R5-N7,
/// R5-D85). Measured before, on the real calculator: a step removed with its own button left the focus on
/// the window, and so did format rows taken off by an error, while a copy an error switched off dropped it
/// on the column's scroll viewer - stops with no name, from which Tab starts over.
/// </summary>
public sealed class FocusKeeperTests
{
    private static readonly TimeSpan AnswerDeadline = TimeSpan.FromSeconds(20);

    [Fact]
    public Task A_step_removed_with_its_own_button_hands_the_focus_to_the_step_now_in_its_place()
        => OnCalculator(steps: 3, async (view, vm) =>
        {
            Press(RemoveButtons(view)[1]);

            Assert.Equal(2, vm.Steps.Count);
            Assert.Same(KindBox(view, 1), Keyboard.FocusedElement);
            await Task.CompletedTask;
        });

    [Fact]
    public Task The_last_step_removed_hands_the_focus_to_the_step_before_it()
        => OnCalculator(steps: 3, async (view, vm) =>
        {
            Press(RemoveButtons(view)[2]);

            Assert.Same(KindBox(view, 1), Keyboard.FocusedElement);
            await Task.CompletedTask;
        });

    [Fact]
    public Task The_only_step_removed_hands_the_focus_to_add_step()
        => OnCalculator(steps: 1, async (view, vm) =>
        {
            Press(RemoveButtons(view)[0]);

            Assert.Empty(vm.Steps);
            Assert.Same(LayoutProbe.FindNamed(view, "AddStepButton"), Keyboard.FocusedElement);
            await Task.CompletedTask;
        });

    [Fact]
    public Task Format_rows_an_error_takes_off_hand_the_focus_to_the_custom_format()
        => OnCalculator(steps: 0, async (view, vm) =>
        {
            Keyboard.Focus(OffScreenWindow.All<CopyButton>(LayoutProbe.FindNamed(view, "FormatList")!).ElementAt(2));
            OffScreenWindow.Settle();

            await UnreadableBaseAsync(vm, () => vm.Formats.Count == 0);

            Assert.Same(LayoutProbe.FindNamed(view, "CustomFormatBox"), Keyboard.FocusedElement);
        });

    [Theory]
    [InlineData("CopyCommandButton")]
    [InlineData("UseInSubstitutionButton")]
    public Task A_button_an_error_switches_off_hands_the_focus_to_the_custom_format(string name)
        => OnCalculator(steps: 0, async (view, vm) =>
        {
            var button = (UIElement)LayoutProbe.FindNamed(view, name)!;
            Keyboard.Focus(button);
            OffScreenWindow.Settle();
            Assert.True(button.IsKeyboardFocused);

            await UnreadableBaseAsync(vm, () => !button.IsEnabled);

            Assert.Same(LayoutProbe.FindNamed(view, "CustomFormatBox"), Keyboard.FocusedElement);
        });

    /// <summary>A focus the reader moved away is the reader's: a step removed afterwards, while nothing in
    /// the list holds the focus, takes nothing back.</summary>
    [Fact]
    public Task A_focus_the_reader_moved_away_is_left_where_it_is()
        => OnCalculator(steps: 2, async (view, vm) =>
        {
            Keyboard.Focus(RemoveButtons(view)[0]);
            OffScreenWindow.Settle();
            var elsewhere = (UIElement)LayoutProbe.FindNamed(view, "CustomFormatBox")!;
            Keyboard.Focus(elsewhere);
            OffScreenWindow.Settle();

            vm.RemoveStep(vm.Steps[0]);
            OffScreenWindow.Settle();

            Assert.Same(elsewhere, Keyboard.FocusedElement);
            await Task.CompletedTask;
        });

    /// <summary>A click on the column's empty space gives the focus to the column's scroll viewer, which holds
    /// the list - a place the focus also falls to, but here the reader put it, and the step that held it can
    /// still hold it.</summary>
    [Fact]
    public Task A_focus_the_reader_put_on_the_column_itself_stays_there()
        => OnCalculator(steps: 2, async (view, vm) =>
        {
            var remove = RemoveButtons(view)[0];
            Keyboard.Focus(remove);
            OffScreenWindow.Settle();
            var column = System.Windows.Media.VisualTreeHelper.GetParent(remove);
            while (column is not ScrollViewer)
            {
                column = System.Windows.Media.VisualTreeHelper.GetParent(column);
            }

            Assert.True(((ScrollViewer)column).Focus());
            OffScreenWindow.Settle();

            Assert.Same(column, Keyboard.FocusedElement);
            await Task.CompletedTask;
        });

    /// <summary>A list inside a region with a fallback of its own still gets its row first.</summary>
    [Fact]
    public void The_innermost_region_puts_the_focus_back()
    {
        WpfTestHost.Invoke(() =>
        {
            var rows = new System.Collections.ObjectModel.ObservableCollection<string> { "first", "second", "third" };
            var outerFallback = new Button { Content = "outer" };
            var list = Rows(rows, new Button { Content = "inner" });
            var outer = new StackPanel { Children = { list } };
            FocusKeeper.SetFallback(outer, outerFallback);
            var window = OffScreenWindow.Show(new StackPanel { Children = { outer, outerFallback } });
            try
            {
                Keyboard.Focus(OffScreenWindow.All<Button>(list).ElementAt(1));
                OffScreenWindow.Settle();

                rows.RemoveAt(1);
                OffScreenWindow.Settle();

                Assert.Equal("third", (Keyboard.FocusedElement as Button)?.Content);
            }
            finally
            {
                window.Close();
            }

            return 0;
        });
    }

    /// <summary>Rows above the one holding the focus may go first (the mouse removes a step while the keyboard
    /// stays on another): the place is the one the row had when it went, not when the focus came.</summary>
    [Fact]
    public void A_row_removed_after_rows_above_it_went_hands_the_focus_to_the_row_now_in_its_place()
        => OnRows(["a", "b", "c", "d", "e"], (rows, buttons) =>
        {
            Keyboard.Focus(buttons()[2]);
            OffScreenWindow.Settle();
            rows.RemoveAt(0);
            OffScreenWindow.Settle();

            rows.Remove("c");
            OffScreenWindow.Settle();

            Assert.Equal("d", (Keyboard.FocusedElement as Button)?.Content);
        });

    /// <summary>The same for a row switched off where it stands: an earlier row from where it stands now.</summary>
    [Fact]
    public void A_row_switched_off_after_rows_above_it_went_hands_the_focus_to_the_row_above_it_now()
        => OnRows(["a", "b", "c", "d"], (rows, buttons) =>
        {
            Keyboard.Focus(buttons()[2]);
            OffScreenWindow.Settle();
            rows.RemoveAt(0);
            OffScreenWindow.Settle();

            buttons().Single(b => "c".Equals(b.Content)).IsEnabled = false;
            OffScreenWindow.Settle();

            Assert.Equal("b", (Keyboard.FocusedElement as Button)?.Content);
        });

    private static void OnRows(string[] start, Action<System.Collections.ObjectModel.ObservableCollection<string>, Func<List<Button>>> check)
    {
        WpfTestHost.Invoke(() =>
        {
            var rows = new System.Collections.ObjectModel.ObservableCollection<string>(start);
            var fallback = new Button { Content = "fallback" };
            var list = Rows(rows, fallback);
            var window = OffScreenWindow.Show(new StackPanel { Children = { list, fallback } });
            try
            {
                check(rows, () => OffScreenWindow.All<Button>(list).ToList());
            }
            finally
            {
                window.Close();
            }

            return 0;
        });
    }

    /// <summary>A list of buttons, one per row, in a region falling back to <paramref name="fallback"/>.</summary>
    private static ItemsControl Rows(System.Collections.IEnumerable rows, UIElement fallback)
    {
        var list = new ItemsControl { ItemsSource = rows, Focusable = false };
        list.ItemTemplate = new DataTemplate { VisualTree = new FrameworkElementFactory(typeof(Button)) };
        list.ItemTemplate.VisualTree.SetBinding(ContentControl.ContentProperty, new System.Windows.Data.Binding());
        FocusKeeper.SetFallback(list, fallback);
        return list;
    }

    /// <summary>A row whose stop is switched off in place sends the focus to an earlier row, not to the
    /// fallback past the rows still there - the format rows do it for a moment no format can show.</summary>
    [Fact]
    public void A_row_switched_off_in_place_hands_the_focus_to_an_earlier_row()
    {
        WpfTestHost.Invoke(() =>
        {
            var fallback = new Button { Content = "fallback" };
            var list = Rows(new[] { "first", "second", "third" }, fallback);
            var window = OffScreenWindow.Show(new StackPanel { Children = { list, fallback } });
            try
            {
                var buttons = OffScreenWindow.All<Button>(list).ToList();
                Keyboard.Focus(buttons[2]);
                OffScreenWindow.Settle();

                buttons[2].IsEnabled = false;
                OffScreenWindow.Settle();

                Assert.Same(buttons[1], Keyboard.FocusedElement);
            }
            finally
            {
                window.Close();
            }

            return 0;
        });
    }

    private static void Press(Button button)
    {
        Keyboard.Focus(button);
        OffScreenWindow.Settle();
        Assert.True(button.IsKeyboardFocused);
        button.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
        OffScreenWindow.Settle();
    }

    private static List<Button> RemoveButtons(CalculatorView view)
        => OffScreenWindow.All<Button>(view).Where(b => b.Tag is StepViewModel).ToList();

    /// <summary>The kind picker of the step at a position - the first stop of its row.</summary>
    private static ComboBox KindBox(CalculatorView view, int position)
        => OffScreenWindow.All<ComboBox>(view).Single(box => box.DataContext is StepViewModel step
            && step.Number == position + 1 && ReferenceEquals(box.ItemsSource, step.Kinds));

    private static async Task UnreadableBaseAsync(CalculatorViewModel vm, Func<bool> done)
    {
        vm.Base.DateText = "2026-13-45";
        var deadline = DateTime.UtcNow + AnswerDeadline;
        while (DateTime.UtcNow < deadline && !done())
        {
            await Task.Delay(TimeSpan.FromMilliseconds(25), TestContext.Current.CancellationToken);
        }

        Assert.True(done(), "the calculator never took the unreadable base");
        OffScreenWindow.Settle();
    }

    /// <summary>The calculator with a specific base, a number of shift steps and its answer on the screen, in
    /// a window that can hold the focus.</summary>
    private static Task OnCalculator(int steps, Func<CalculatorView, CalculatorViewModel, Task> check)
        => WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
            await vm.EnsureComputedAsync();
            vm.SelectedBase = vm.BaseKinds.Single(b => b.Kind == BaseKind.Specific);
            for (var i = 0; i < steps; i++)
            {
                vm.AddStep();
            }

            var deadline = DateTime.UtcNow + AnswerDeadline;
            while (DateTime.UtcNow < deadline && (vm.IsResultStale || !vm.HasResult))
            {
                await Task.Delay(TimeSpan.FromMilliseconds(25), TestContext.Current.CancellationToken);
            }

            Assert.True(vm.HasResult && vm.Formats.Count > 0, "the calculator never answered");
            var view = new CalculatorView { DataContext = vm };
            var window = OffScreenWindow.Show(view);
            try
            {
                await check(view, vm);
            }
            finally
            {
                window.Close();
            }

            return 0;
        });
}
