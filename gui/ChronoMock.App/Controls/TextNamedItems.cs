using System.Windows;
using System.Windows.Controls;

namespace ChronoMock.App.Controls;

/// <summary>
/// <see cref="TextNamedList"/> for a list with no header: a row of buttons, a column of readings. A type of
/// its own only because the base control is.
/// </summary>
public class TextNamedItems : ItemsControl
{
    // Never a stop on the keyboard path itself, for the reason given on TextNamedList. The buttons a row
    // holds stay stops of their own.
    static TextNamedItems() => FocusableProperty.OverrideMetadata(typeof(TextNamedItems), new FrameworkPropertyMetadata(false));

    protected override DependencyObject GetContainerForItemOverride() => new TextNamedRow();
}
