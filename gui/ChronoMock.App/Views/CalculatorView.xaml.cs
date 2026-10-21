using System.Windows;
using System.Windows.Controls;
using ChronoMock.App.Calc;

namespace ChronoMock.App.Views;

/// <summary>
/// Date-calculator screen (chrono-mock 7.3). Slice G3b: the builder and result bind to a
/// <see cref="CalculatorViewModel"/> (set by the host window). These handlers cover the actions that
/// are not plain bindings - adding and removing a step, and copying one output format.
/// </summary>
public partial class CalculatorView : UserControl
{
    public CalculatorView()
    {
        InitializeComponent();
    }

    private CalculatorViewModel? ViewModel => DataContext as CalculatorViewModel;

    private void OnAddStepClick(object sender, RoutedEventArgs e) => ViewModel?.AddStep();

    private void OnRemoveStepClick(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: StepViewModel step })
        {
            ViewModel?.RemoveStep(step);
        }
    }

    // Send the result to the substitution panel (6.3): the view model raises an event the host window
    // handles (it knows both modules), so the moment travels with its zone (rule 2). It waits for a result
    // still being computed and sends that one, never the one on screen before it (R4-W4), and a second
    // press during that wait is ignored by the view model.
    //
    // No catch here, on purpose, in this handler and the one below. What either awaits returns its failures
    // as values (an error result, a null to copy), and anything else that escapes an awaited async void
    // handler is posted to the dispatcher, where App.OnDispatcherUnhandledException records and shows it
    // once per signature and keeps the window alive. A catch here would be a second route for the same
    // fault, or a place to lose it.
    private async void OnUseInSubstitutionClick(object sender, RoutedEventArgs e)
    {
        if (ViewModel is { } vm)
        {
            await vm.RequestUseInSubstitutionAsync();
        }
    }

    // Copy one output format to the clipboard: the value its row holds in the result for the builder as it
    // stands, which the view model waits for when it is still being computed (R4-W4). The row survives the
    // refresh, so its button says what the copy did - "Copied", or "Clipboard busy" when another program
    // held it, which this screen used to swallow (R5-D81).
    private async void OnCopyFormatClick(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { Tag: FormatRow row } || ViewModel is not { } vm
            || await vm.CopyValueAsync(row) is not { Length: > 0 } value)
        {
            return;
        }

        row.Copy.Report(ClipboardText.TrySet(value));
    }

    // Copy the result's question as a command line for a terminal (R5-D54), once the result for the builder as
    // it stands is in.
    private async void OnCopyCommandClick(object sender, RoutedEventArgs e)
    {
        if (ViewModel is { } vm && await vm.CopyCommandAsync() is { Length: > 0 } command)
        {
            vm.CommandCopy.Report(ClipboardText.TrySet(command));
        }
    }
}
