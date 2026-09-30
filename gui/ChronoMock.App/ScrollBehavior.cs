using System.Windows;
using System.Windows.Controls;

namespace ChronoMock.App;

/// <summary>
/// Scrolls a ScrollViewer back to its top each time a bound flag turns true.
/// </summary>
/// <remarks>
/// The three phases are fixed instances switched by visibility, so each keeps its scroll offset while it is
/// hidden. A second session therefore opened its session phase and its result wherever the tester had left
/// the first one - measured at 100 percent, with the verdict headline above the edge of the window.
///
/// Keyed to the phase's own flag and not to visibility, on purpose: the module switch hides and shows the
/// same phase as well, and coming back from the calculator has to find the result where the tester left it
/// (GUI rule 3). The flag turns true only when a session enters the phase. A value the view sets, like
/// <see cref="ComboBoxBehavior"/>, so the phase keeps no code-behind (GUI rule 11).
/// </remarks>
public static class ScrollBehavior
{
    /// <summary>Scroll to the top whenever this turns true. Staying true, or turning false, moves nothing.</summary>
    public static readonly DependencyProperty ToTopWhenProperty = DependencyProperty.RegisterAttached(
        "ToTopWhen",
        typeof(bool),
        typeof(ScrollBehavior),
        new PropertyMetadata(false, OnToTopWhenChanged));

    public static bool GetToTopWhen(DependencyObject element)
        => (bool)element.GetValue(ToTopWhenProperty);

    public static void SetToTopWhen(DependencyObject element, bool value)
        => element.SetValue(ToTopWhenProperty, value);

    private static void OnToTopWhenChanged(DependencyObject element, DependencyPropertyChangedEventArgs e)
    {
        // The flag and the phase's visibility follow the same change, in an order nobody promises, so this
        // can run while the phase is still collapsed. The scroll viewer queues the request and applies it on
        // its next layout pass, which is the one that shows the phase.
        if (element is ScrollViewer scroll && e.NewValue is true)
        {
            scroll.ScrollToTop();
        }
    }
}
