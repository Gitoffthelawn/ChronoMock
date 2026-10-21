using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using ChronoMock.App.Controls;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// The window builds its session and result phases the first time they show, and keeps them (R5/8, WY6).
/// Building both before the first frame cost 81 ms of a 910 ms start, measured on the published window.
/// </summary>
public class DeferredViewTests
{
    /// <summary>
    /// Reversal probe: build the view in DeferredView's constructor, or put the two phases back in the window
    /// as they were, and this reddens on both counts.
    /// </summary>
    [Fact]
    public void A_window_that_starts_has_built_neither_later_phase()
    {
        var (setup, session, result) = WpfTestHost.InvokeSettled(() =>
        {
            var root = (FrameworkElement)new MainWindow().Content;
            LayoutProbe.Settle(root);
            return (Count<SetupPhaseView>(root), Count<SessionPhaseView>(root), Count<ResultPhaseView>(root));
        });

        Assert.Equal(1, setup); // the canary: a walk that reaches no phase would pass the two below
        Assert.Equal(0, session);
        Assert.Equal(0, result);
    }

    /// <summary>
    /// A phase that shows is built, and the one that comes back is the one that was built - a rebuilt phase
    /// would forget what the tester left in it (GUI rule 3).
    /// Reversal probe: drop the Child check in DeferredView.MeasureOverride, so every measure builds anew, and
    /// this reddens on the session phase that comes back.
    /// </summary>
    [Fact]
    public void A_phase_is_built_when_it_first_shows_and_the_same_one_comes_back()
    {
        var (sessionFirst, resultWhileSession, resultLater, sessionKept, sessionBack) = WpfTestHost.InvokeSettled(() =>
        {
            var window = new MainWindow();
            var root = (FrameworkElement)window.Content;

            window.DataContext = PhaseStates.WithTarget(SessionStates.Running());
            LayoutProbe.Settle(root);
            var first = OffScreenWindow.All<SessionPhaseView>(root).Single();
            var resultsThen = Count<ResultPhaseView>(root);

            window.DataContext = PhaseStates.ResultWorks();
            LayoutProbe.Settle(root);
            var results = Count<ResultPhaseView>(root);
            var kept = OffScreenWindow.All<SessionPhaseView>(root).Single();

            window.DataContext = PhaseStates.WithTarget(SessionStates.Running());
            LayoutProbe.Settle(root);
            var back = OffScreenWindow.All<SessionPhaseView>(root).Single();
            return (first, resultsThen, results, ReferenceEquals(first, kept), ReferenceEquals(first, back));
        });

        Assert.NotNull(sessionFirst);
        Assert.Equal(0, resultWhileSession);
        Assert.Equal(1, resultLater);
        Assert.True(sessionKept, "hiding the session phase threw it away");
        Assert.True(sessionBack, "the session phase that came back is not the one that was built");
    }

    /// <summary>
    /// The view stands where the host stands: it takes the host's data and is laid out in its place.
    /// Reversal probe: return the built view without setting it as the host's child, and this reddens.
    /// </summary>
    [Fact]
    public void The_built_view_takes_the_data_of_the_place_it_stands_in()
    {
        var (text, width) = WpfTestHost.InvokeSettled(() =>
        {
            var factory = new FrameworkElementFactory(typeof(TextBlock));
            factory.SetBinding(TextBlock.TextProperty, new Binding());
            var host = new DeferredView { View = new DataTemplate { VisualTree = factory }, DataContext = "news", Visibility = Visibility.Visible };
            LayoutProbe.Settle(host, 200, 100);
            return ((host.Child as TextBlock)?.Text, host.Child?.RenderSize.Width ?? 0);
        });

        Assert.Equal("news", text);
        Assert.Equal(200, width);
    }

    private static int Count<T>(DependencyObject root)
        where T : DependencyObject => OffScreenWindow.All<T>(root).Count();
}
