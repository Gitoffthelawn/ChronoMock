using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// A new session opens the session phase and the result at the top, and the module switch keeps the place.
/// </summary>
/// <remarks>
/// The phases are fixed instances switched by visibility, so each kept its scroll offset while hidden: a
/// second session opened both phases wherever the tester had left the first one, measured live at 100
/// percent with the verdict headline above the edge (tools/probes/r4-12/gui-phase-scroll.ps1). Measured
/// here on the real phase views, laid out at the window's floor height so they scroll, because the fault is
/// invisible in the markup.
///
/// A new session is played the way the window plays it: the phase collapses while its flag is false, the
/// flag turns true while the phase is still collapsed, and only then does it show. That order is the hard
/// one - a scroll asked for on a collapsed viewer has to survive until the pass that shows it.
///
/// Every test first asserts that the phase was scrolled away from the top, because "the offset is 0" is
/// also true of a view that never scrolled at all.
///
/// 🔴 THE PHASES ARE HOSTED IN A HIDDEN WINDOW. A view laid out on its own is never IsVisible - that needs a
/// presentation source - so the first version of the calculator test passed over a behaviour keyed to
/// IsVisibleChanged, the very design it exists to rule out (measured: its revert probe stayed green). An
/// HwndSource without WS_VISIBLE gives the view a real source without showing anything, and the test
/// asserts IsVisible before it relies on it.
/// </remarks>
public class ScrollBehaviorTests
{
    /// <summary>The window's minimum height, where every phase scrolls.</summary>
    private const int FloorHeight = 360;

    [Fact]
    public void A_new_session_opens_its_result_at_the_top()
    {
        // Reversal probe: drop app:ScrollBehavior.ToTopWhen from ResultPhaseView's FormScroll and the second
        // result opens where the first one was left.
        var (read, next) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new ResultPhaseView { DataContext = PhaseStates.ResultWorks() };
            using var host = Host(view);
            var scroll = ReadDown(view);
            var reading = scroll.VerticalOffset;
            NextSession(view, PhaseStates.ResultWorks());
            return (reading, scroll.VerticalOffset);
        });

        Assert.True(read > 0, "the first result never scrolled, so where the second opens proves nothing");
        Assert.Equal(0, next);
    }

    [Fact]
    public void A_new_session_opens_the_session_phase_at_the_top()
    {
        // Reversal probe: drop app:ScrollBehavior.ToTopWhen from SessionPhaseView's FormScroll.
        var (read, next) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new SessionPhaseView { DataContext = PhaseStates.WithTarget(SessionStates.RunningWithCoverageWarnings()) };
            using var host = Host(view);
            var scroll = ReadDown(view);
            var reading = scroll.VerticalOffset;
            NextSession(view, PhaseStates.WithTarget(SessionStates.RunningWithCoverageWarnings()));
            return (reading, scroll.VerticalOffset);
        });

        Assert.True(read > 0, "the first session never scrolled, so where the second opens proves nothing");
        Assert.Equal(0, next);
    }

    [Fact]
    public void Coming_back_from_the_calculator_keeps_the_result_where_it_was()
    {
        // Reversal probe: scroll to the top on IsVisibleChanged instead of on the phase flag, and the result
        // jumps back up every time the tester looks at the calculator.
        var (read, back, seen) = WpfTestHost.InvokeSettled(() =>
        {
            var view = new ResultPhaseView { DataContext = PhaseStates.ResultWorks() };
            using var host = Host(view);
            var scroll = ReadDown(view);
            var reading = scroll.VerticalOffset;
            var visible = scroll.IsVisible;
            view.Visibility = Visibility.Collapsed;
            LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
            view.Visibility = Visibility.Visible;
            LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
            return (reading, scroll.VerticalOffset, visible);
        });

        Assert.True(seen, "the viewer is not IsVisible, so a behaviour keyed to visibility would pass unseen");
        Assert.True(read > 0, "the result never scrolled, so keeping its place proves nothing");
        Assert.Equal(read, back);
    }

    [Fact]
    public void The_flag_staying_true_moves_nothing()
    {
        // A live phase refreshes its data all the time (a copy, a history write, a new count): only the flag
        // turning true may scroll, never it being set to what it already was.
        var (read, after) = WpfTestHost.InvokeSettled(() =>
        {
            var scroll = new ScrollViewer { Content = new Border { Height = 2000 } };
            ScrollBehavior.SetToTopWhen(scroll, true);
            LayoutProbe.Settle(scroll, LayoutProbe.WindowWidth, FloorHeight);
            scroll.ScrollToBottom();
            LayoutProbe.Settle(scroll, LayoutProbe.WindowWidth, FloorHeight);
            var reading = scroll.VerticalOffset;
            ScrollBehavior.SetToTopWhen(scroll, true);
            LayoutProbe.Settle(scroll, LayoutProbe.WindowWidth, FloorHeight);
            return (reading, scroll.VerticalOffset);
        });

        Assert.True(read > 0, "the viewer never scrolled");
        Assert.Equal(read, after);
    }

    /// <summary>
    /// A presentation source for the view, in a window that is never shown (no WS_VISIBLE), so IsVisible and
    /// IsVisibleChanged behave as they do in the application.
    /// </summary>
    private static HwndSource Host(FrameworkElement view)
        => new(new HwndSourceParameters(nameof(ScrollBehaviorTests), LayoutProbe.WindowWidth, FloorHeight) { WindowStyle = 0 })
        {
            RootVisual = view,
        };

    /// <summary>Lay the phase out at the floor height and scroll it to its end, as a tester reading down would.</summary>
    private static ScrollViewer ReadDown(FrameworkElement view)
    {
        LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
        var scroll = (ScrollViewer)view.FindName("FormScroll");
        scroll.ScrollToBottom();
        LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
        return scroll;
    }

    /// <summary>
    /// What the window does between one session and the next: the phase leaves (its flag false, collapsed),
    /// the next session's model arrives with the flag true while the phase is still collapsed, then it shows.
    /// </summary>
    private static void NextSession(FrameworkElement view, object nextModel)
    {
        view.Visibility = Visibility.Collapsed;
        view.DataContext = PhaseStates.SetupStartup();
        LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
        view.DataContext = nextModel;
        LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
        view.Visibility = Visibility.Visible;
        LayoutProbe.Settle(view, LayoutProbe.WindowWidth, FloorHeight);
    }
}
