using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using System.Windows.Threading;
using ChronoMock.App.Calc;
using ChronoMock.App.Views;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The reported failure: the custom-format field did nothing. It was bound with
/// <c>UpdateSourceTrigger=LostFocus</c>, and it is the last control in the result column, so a tester who
/// typed a mask and then looked at the result never moved focus and never saw one. Measured on the running
/// window before the fix: the box held "yyyy-MM-dd HH:mm:ss" and the result row was absent, then appeared
/// the moment focus went elsewhere.
/// <para>
/// This asserts the half a view-model test cannot reach. <see cref="CalculatorViewModel.CustomFormatMask"/>
/// already had its own tests and all of them passed over the dead field, because they set the property
/// directly - which is exactly what the view was failing to do.
/// </para>
/// <para>
/// <b>What this does not prove.</b> Nothing here computes a result. The view model is built on a client
/// that would launch a binary that does not exist, and the first-reveal gate is never opened, so the mask
/// setter schedules nothing. The question is only whether typing reaches the view model at all.
/// </para>
/// </summary>
public class CalculatorViewBindingTests
{
    [Fact]
    public void Typing_a_custom_format_mask_reaches_the_view_model_without_leaving_the_field()
    {
        var mask = WpfTestHost.InvokeSettled(() =>
        {
            var (view, vm) = NewCalculatorView();
            var box = (TextBox)view.FindName("CustomFormatBox");

            // Not vacuous: a box that never found its data context would take the text and report an
            // empty mask below for a reason that has nothing to do with the trigger.
            Assert.Same(vm, box.DataContext);

            // What typing does. Focus is never moved, which is the whole point.
            box.Text = "yyyy-MM-dd HH:mm:ss";
            return vm.CustomFormatMask;
        });

        Assert.Equal("yyyy-MM-dd HH:mm:ss", mask);
    }

    [Fact]
    public void Clearing_the_custom_format_mask_reaches_the_view_model_too()
    {
        // The other direction of the same wire, and the one the clear button on the wpfui box uses. A
        // mask that can be typed but not withdrawn would leave the result row stuck on screen.
        //
        // The first draft of this test asserted only the empty end state and was VACUOUS: put the old
        // LostFocus trigger back and it stayed green, because a mask that never arrives is also a mask
        // that never has to be withdrawn. It therefore reads the box twice, and the first read is what
        // makes the second one mean anything.
        var (typed, cleared) = WpfTestHost.InvokeSettled(() =>
        {
            var (view, vm) = NewCalculatorView();
            var box = (TextBox)view.FindName("CustomFormatBox");
            box.Text = "yyyy";
            var afterTyping = vm.CustomFormatMask;
            box.Text = string.Empty;
            return (afterTyping, vm.CustomFormatMask);
        });

        Assert.Equal("yyyy", typed);
        Assert.Equal(string.Empty, cleared);
    }

    [Fact]
    public void A_date_pasted_into_the_analysis_box_reaches_the_view_model_without_leaving_it()
    {
        // The same fault the mask had, in the analysis strip (R4-N44): the box is the last field in its
        // column, and on LostFocus a pasted date was analysed only once focus happened to move.
        var pasted = WpfTestHost.InvokeSettled(() =>
        {
            var (view, vm) = NewCalculatorView();
            var box = (TextBox)view.FindName("AnalyzeBox");
            Assert.Same(vm, box.DataContext);

            box.Text = "12/31/1999";
            return vm.AnalyzeText;
        });

        Assert.Equal("12/31/1999", pasted);
    }

    [Fact]
    public void The_result_fades_while_a_newer_result_is_computed_and_only_then()
    {
        // Measured on the laid-out view, not read off the markup (GUI rule 10): the block holding the
        // formats takes the stale opacity from the shared style when the view model says the result is
        // behind its input, and full opacity otherwise.
        var (stale, current, token) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new CalculatorView { DataContext = new StaleStub { IsResultStale = true } };
            Layout(view);
            var staleOpacity = ResultBlock(view).Opacity;

            view.DataContext = new StaleStub { IsResultStale = false };
            Layout(view);
            return (staleOpacity, ResultBlock(view).Opacity, (double)view.FindResource("OpacityStale"));
        });

        Assert.Equal(token, stale);
        Assert.True(token < 1);
        Assert.Equal(1, current);
    }

    [Fact]
    public async Task A_failure_behind_the_Use_press_reaches_the_dispatchers_fault_net()
    {
        // The click handlers keep no catch of their own, on the claim that a fault escaping an awaited async
        // void handler lands on the dispatcher, where the application records and shows it once and stays up
        // (App.OnDispatcherUnhandledException). Measured here rather than trusted: the host window's handler
        // throws, the button is pressed, and the dispatcher's net must be what catches it.
        var caught = await WpfTestHost.RunAsync(async () =>
        {
            Exception? seen = null;
            void Net(object sender, DispatcherUnhandledExceptionEventArgs e)
            {
                seen = e.Exception;
                e.Handled = true;
            }

            var dispatcher = Dispatcher.CurrentDispatcher;
            dispatcher.UnhandledException += Net;
            try
            {
                var vm = new CalculatorViewModel(new FakeCalcEngine(args => args.Contains("--analyze")
                    ? CalcResults.Analysis("2008-04-08T00:00:00")
                    : CalcResults.Moment("2026-01-01T00:00:00")));
                var view = new CalculatorView { DataContext = vm };
                Layout(view);
                await vm.EnsureComputedAsync();
                vm.UseInSubstitutionRequested += (_, _) => throw new InvalidOperationException("the host failed");

                ((Button)view.FindName("UseInSubstitutionButton")).RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent));
                await Dispatcher.Yield(DispatcherPriority.ApplicationIdle);
                await Dispatcher.Yield(DispatcherPriority.ApplicationIdle);
                return seen;
            }
            finally
            {
                dispatcher.UnhandledException -= Net;
            }
        });

        Assert.Equal("the host failed", Assert.IsType<InvalidOperationException>(caught).Message);
    }

    [Fact]
    public void A_format_row_with_no_value_has_its_Copy_off()
    {
        // R4-Z3: the row shows the out-of-range marker, a sentence about the value rather than the value.
        var (withValue, withoutValue) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new CalculatorView
            {
                DataContext = new FormatsStub
                {
                    Formats = [new FormatRow("Epoch (s)", "0", hasValue: true), new FormatRow("FILETIME", "out of range", hasValue: false)],
                },
            };
            Layout(view);
            return (CopyButton(view, 0).IsEnabled, CopyButton(view, 1).IsEnabled);
        });

        Assert.True(withValue);
        Assert.False(withoutValue);
    }

    /// <summary>The Copy button of the format row at <paramref name="index"/>, found on the laid-out list.</summary>
    private static Button CopyButton(CalculatorView view, int index)
    {
        var list = (ItemsControl)view.FindName("FormatList");
        var container = (DependencyObject)list.ItemContainerGenerator.ContainerFromIndex(index);
        return Descendants(container).OfType<Button>().Single();
    }

    private static IEnumerable<DependencyObject> Descendants(DependencyObject root)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            yield return child;
            foreach (var deeper in Descendants(child))
            {
                yield return deeper;
            }
        }
    }

    /// <summary>Stands in for the view model's format rows.</summary>
    private sealed class FormatsStub
    {
        public IReadOnlyList<FormatRow> Formats { get; init; } = [];
    }

    /// <summary>The block of answers the format list sits in - the container the stale state fades.</summary>
    private static StackPanel ResultBlock(CalculatorView view)
        => (StackPanel)LogicalTreeHelper.GetParent((DependencyObject)view.FindName("FormatList"));

    /// <summary>Stands in for the view model's stale flag, tied to the real name below.</summary>
    private sealed class StaleStub
    {
        public bool IsResultStale { get; init; }
    }

    [Fact]
    public void The_fade_is_driven_by_the_view_models_own_property_name()
    {
        Assert.Equal(nameof(CalculatorViewModel.IsResultStale), nameof(StaleStub.IsResultStale));
    }

    [Fact]
    public void The_calendar_picker_is_marked_when_the_engine_refuses_for_want_of_a_calendar()
    {
        // The other half of the missing-calendar fix. CalculatorErrorTests proves the sentence, this
        // proves the mark, and the two are bound to the same name below so neither can drift alone.
        // The mark is the drop-down's own edge (the shared error state, the one a text field shows), so
        // the measurement is the rendered border of the template's face, not a wrapper around it.
        var (quiet, marked, error) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new CalculatorView { DataContext = new CalendarMissingStub { CalendarMissing = false } };
            Layout(view);
            var quietBrush = CalendarEdge(view);

            view.DataContext = new CalendarMissingStub { CalendarMissing = true };
            Layout(view);
            var markedBrush = CalendarEdge(view);

            return (quietBrush, markedBrush, view.TryFindResource("BrushError"));
        });

        Assert.NotNull(error);
        Assert.Same(error, marked);
        Assert.NotSame(error, quiet);
    }

    /// <summary>The border brush the calendar drop-down is actually drawn with: the "Face" element of the
    /// shared ComboBox template, found through the template on the laid-out control.</summary>
    private static Brush CalendarEdge(CalculatorView view)
    {
        var box = (ComboBox)view.FindName("CalendarBox");
        box.ApplyTemplate();
        return ((Border)box.Template.FindName("Face", box)).BorderBrush;
    }

    /// <summary>Stands in for the view model so the trigger can be driven without running the engine. The
    /// property name is tied to the real one by <c>nameof</c> in the assertion below, so renaming the view
    /// model's flag breaks the build here rather than leaving a green test over a dead trigger.</summary>
    private sealed class CalendarMissingStub
    {
        public bool CalendarMissing { get; init; }
    }

    [Fact]
    public void The_mark_is_driven_by_the_view_models_own_property_name()
    {
        Assert.Equal(nameof(CalculatorViewModel.CalendarMissing), nameof(CalendarMissingStub.CalendarMissing));
    }

    /// <summary>A calculator screen wired to a view model that can never start a process: the path callback
    /// hands out a name that does not exist, and nothing here opens the first-reveal gate that would make
    /// the mask setter schedule a recompute.</summary>
    private static (CalculatorView View, CalculatorViewModel ViewModel) NewCalculatorView()
    {
        var client = new CalcClient(() => Path.Combine(Path.GetTempPath(), "chrono-does-not-exist-here.exe"));
        var vm = new CalculatorViewModel(client);
        var view = new CalculatorView { DataContext = vm };
        Layout(view);
        return (view, vm);
    }

    /// <summary>Lay the control out so its template is applied and every binding is attached. An unmeasured
    /// control can leave bindings unattached, which would make an assertion pass or fail for the wrong
    /// reason (the same trap TargetBoxTests documents for an unshown Window).</summary>
    private static void Layout(FrameworkElement view)
    {
        view.Measure(new Size(1600, 1400));
        view.Arrange(new Rect(0, 0, 1600, 1400));
        view.UpdateLayout();
    }
}
