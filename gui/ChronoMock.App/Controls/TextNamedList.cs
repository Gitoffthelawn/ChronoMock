using System.Windows;
using System.Windows.Controls;

namespace ChronoMock.App.Controls;

/// <summary>
/// A HeaderedItemsControl whose rows are named for assistive tech by the text they show. Every list and
/// table with a header uses it (PartTable, PartNoteList), and a view never uses the base control, which
/// ItemNameGuardTests enforces - see <see cref="TextNamedRow"/> for why the name has to come from the row.
/// </summary>
public class TextNamedList : HeaderedItemsControl
{
    // Never a stop on the keyboard path itself (R5-U2). An ItemsControl is a Control, so WPF makes it
    // focusable, and Tab stopped on a list of readings, formats or steps where there is nothing to do - five
    // such stops on the calculator, each read as an unnamed list. Switched off here rather than in each list
    // style, where the table had it and the fact and note lists did not. A button a row holds is a stop of
    // its own all the same.
    static TextNamedList() => FocusableProperty.OverrideMetadata(typeof(TextNamedList), new FrameworkPropertyMetadata(false));

    protected override DependencyObject GetContainerForItemOverride() => new TextNamedRow();
}
