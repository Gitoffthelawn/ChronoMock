using System.Windows;
using System.Windows.Controls;
using System.Windows.Markup;

namespace ChronoMock.App.Controls;

/// <summary>
/// A view built the first time it is shown and kept from then on (R5/8, WY6). The window holds its later
/// phases in these, so a start pays for the screen it shows and not for the two that only a session reaches.
/// </summary>
/// <remarks>
/// <para>
/// Built when layout first measures it, which is exactly when it is shown: a collapsed element is never
/// measured. Loaded would not do - it needs a window on the screen, which a test that lays the window out off
/// screen never has.
/// </para>
/// <para>
/// 🔴 Collapsed until told otherwise. A binding that has no data yet leaves an element at its default, and the
/// default of every element is Visible - so a host bound to a phase flag was laid out, and built, in the moment
/// before its binding said Collapsed (measured: both phases built in a window laid out as it was made).
/// </para>
/// <para>
/// Kept, never rebuilt: hiding it again, for the calculator or for a new session, keeps what the tester left
/// in it - a folded section, a scroll offset - as the phases have always kept it (GUI rule 3).
/// </para>
/// <para>
/// Measured on the published window, median of nine starts against nine: the session and result phases built
/// at start cost 81 ms of 910 before the first frame. Their first showing now pays it instead, once.
/// </para>
/// </remarks>
[ContentProperty(nameof(View))]
public sealed class DeferredView : Decorator
{
    static DeferredView() => VisibilityProperty.OverrideMetadata(
        typeof(DeferredView), new FrameworkPropertyMetadata(Visibility.Collapsed));

    /// <summary>What to build: a template whose root is the view.</summary>
    public static readonly DependencyProperty ViewProperty = DependencyProperty.Register(
        nameof(View), typeof(DataTemplate), typeof(DeferredView), new PropertyMetadata(null));

    public DataTemplate? View
    {
        get => (DataTemplate?)GetValue(ViewProperty);
        set => SetValue(ViewProperty, value);
    }

    protected override Size MeasureOverride(Size constraint)
    {
        if (Child is null && View is { } template)
        {
            // A template written in place, not in a dictionary, is not sealed by anything else, and one built
            // in code refuses to load until it is.
            if (!template.IsSealed)
            {
                template.Seal();
            }

            Child = template.LoadContent() as UIElement;
        }

        return base.MeasureOverride(constraint);
    }
}
