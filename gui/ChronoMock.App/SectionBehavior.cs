using System.Windows;
using System.Windows.Controls;

namespace ChronoMock.App;

/// <summary>
/// Opens a folding section each time a bound flag turns true, for something inside it the reader has to see.
/// </summary>
/// <remarks>
/// The history section is folded, and its summary says only how many sessions it holds, so a history the
/// store could not write stayed behind the fold - for the eye and for a screen reader alike, whose
/// announcement of a line waits until the line shows (found in review on #136). Opening is a value the view
/// sets, like <see cref="ScrollBehavior"/>, so the section keeps no code-behind (GUI rule 11). Only the turn to
/// true opens it, so the reader may fold it again.
/// </remarks>
public static class SectionBehavior
{
    /// <summary>Open the section whenever this turns true. Staying true, or turning false, moves nothing.</summary>
    public static readonly DependencyProperty OpenWhenProperty = DependencyProperty.RegisterAttached(
        "OpenWhen",
        typeof(bool),
        typeof(SectionBehavior),
        new PropertyMetadata(false, OnOpenWhenChanged));

    public static bool GetOpenWhen(DependencyObject element)
        => (bool)element.GetValue(OpenWhenProperty);

    public static void SetOpenWhen(DependencyObject element, bool value)
        => element.SetValue(OpenWhenProperty, value);

    private static void OnOpenWhenChanged(DependencyObject element, DependencyPropertyChangedEventArgs e)
    {
        // SetCurrentValue, not a local value, so a binding or style setter the section has for IsExpanded stays.
        if (element is Expander section && e.NewValue is true)
        {
            section.SetCurrentValue(Expander.IsExpandedProperty, true);
        }
    }
}
