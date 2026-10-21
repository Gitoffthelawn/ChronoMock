using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Data;

namespace ChronoMock.App.Controls;

/// <summary>
/// A button that copies something to the clipboard and says, on itself, what the copy did: its own label,
/// then "Copied" or "Clipboard busy" for <see cref="CopyOutcome.ShownFor"/>, then its label again (R5-D81).
/// Every copy action on this interface is one of these.
/// </summary>
/// <remarks>
/// 🔴 IT NEVER CHANGES WIDTH. The three texts stand in one cell and only one is shown, the others Hidden,
/// so the button is as wide as the widest of them from the start - a layout rule rather than a width
/// measured off today's words. The button flipping from "Copy summary" to "Copied" used to move the one
/// beside it, and a Polish "Skopiowano" is longer than "Kopiuj".
///
/// The label is <see cref="Label"/>, not Content, because the content is that cell. A screen reader hears
/// <see cref="AccessibleLabel"/> when one is given - "Copy ISO 8601" where eight buttons all read "Copy" -
/// and the outcome while it is shown.
/// </remarks>
public sealed class CopyButton : Button
{
    public static readonly DependencyProperty LabelProperty = DependencyProperty.Register(
        nameof(Label), typeof(string), typeof(CopyButton), new PropertyMetadata(string.Empty, OnShownChanged));

    public static readonly DependencyProperty AccessibleLabelProperty = DependencyProperty.Register(
        nameof(AccessibleLabel), typeof(string), typeof(CopyButton), new PropertyMetadata(null, OnShownChanged));

    public static readonly DependencyProperty OutcomeProperty = DependencyProperty.Register(
        nameof(Outcome), typeof(CopyOutcome), typeof(CopyButton), new PropertyMetadata(null, OnOutcomeChanged));

    /// <summary>What the button says. Bound to the <see cref="Outcome"/> when there is one, and set directly
    /// by the component catalogue, which shows every state side by side.</summary>
    public static readonly DependencyProperty StateProperty = DependencyProperty.Register(
        nameof(State), typeof(CopyState), typeof(CopyButton), new PropertyMetadata(CopyState.Idle, OnShownChanged));

    private readonly TextBlock _label = new();
    private readonly TextBlock _copied = new();
    private readonly TextBlock _failed = new();

    public CopyButton()
    {
        // Each text centred in the cell the widest of them sets, so the short label of a wide button does not
        // sit against its left edge (measured on the render: "Copy" in a button as wide as "Clipboard busy").
        var cell = new Grid();
        foreach (var text in new[] { _label, _copied, _failed })
        {
            text.HorizontalAlignment = HorizontalAlignment.Center;
            cell.Children.Add(text);
        }
        Content = cell;
        Show();
    }

    /// <summary>The button's own text, shown whenever there is no outcome to say.</summary>
    public string Label
    {
        get => (string)GetValue(LabelProperty);
        set => SetValue(LabelProperty, value);
    }

    /// <summary>The name a screen reader hears while the label is shown, when the label alone does not say
    /// which button this is. Unset, it hears the label.</summary>
    public string? AccessibleLabel
    {
        get => (string?)GetValue(AccessibleLabelProperty);
        set => SetValue(AccessibleLabelProperty, value);
    }

    /// <summary>The outcome this button reports to and shows.</summary>
    public CopyOutcome? Outcome
    {
        get => (CopyOutcome?)GetValue(OutcomeProperty);
        set => SetValue(OutcomeProperty, value);
    }

    public CopyState State
    {
        get => (CopyState)GetValue(StateProperty);
        set => SetValue(StateProperty, value);
    }

    private static void OnShownChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((CopyButton)d).Show();

    // A binding rather than an event handler, so a long-lived outcome never holds a button alive.
    private static void OnOutcomeChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        var button = (CopyButton)d;
        if (e.NewValue is CopyOutcome outcome)
        {
            button.SetBinding(StateProperty, new Binding(nameof(CopyOutcome.State)) { Source = outcome });
        }
        else
        {
            BindingOperations.ClearBinding(button, StateProperty);
        }
    }

    private void Show()
    {
        _label.Text = Label;
        _copied.Text = TranslationKeyConverter.Resolve("copy.copied");
        _failed.Text = TranslationKeyConverter.Resolve("copy.busy");

        var shown = State switch
        {
            CopyState.Copied => _copied,
            CopyState.Failed => _failed,
            _ => _label,
        };
        foreach (var text in new[] { _label, _copied, _failed })
        {
            text.Visibility = ReferenceEquals(text, shown) ? Visibility.Visible : Visibility.Hidden;
        }

        var name = State == CopyState.Idle && !string.IsNullOrEmpty(AccessibleLabel) ? AccessibleLabel : shown.Text;
        AutomationProperties.SetName(this, name);
    }
}
