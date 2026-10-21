using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Controls;

namespace ChronoMock.App.Controls;

/// <summary>
/// A list to choose from whose rows are named for assistive tech by the text they show, as
/// <see cref="TextNamedRow"/> names the rows of a list that is only read (R5-U2).
/// </summary>
/// <remarks>
/// The history list named each row by its application alone, so two sessions of one program were two rows
/// called "sample-app.exe" - while the row itself shows the verdict, the moment and when it ended, the very
/// facts its template says are there to tell the rows apart. A name written in the container style would
/// restate the row's template, and a second statement drifts, so this reads the row instead. A name set
/// explicitly with AutomationProperties.Name on the container still wins.
/// </remarks>
public class TextNamedListBox : ListBox
{
    protected override DependencyObject GetContainerForItemOverride() => new TextNamedListBoxItem();

    protected override bool IsItemItsOwnContainerOverride(object item) => item is TextNamedListBoxItem;
}

/// <summary>The row container of a <see cref="TextNamedListBox"/>.</summary>
public sealed class TextNamedListBoxItem : ListBoxItem
{
    protected override AutomationPeer OnCreateAutomationPeer() => new TextNamedListBoxItemAutomationPeer(this);

    private sealed class TextNamedListBoxItemAutomationPeer(TextNamedListBoxItem owner)
        : ListBoxItemWrapperAutomationPeer(owner)
    {
        protected override string GetNameCore()
        {
            string set = AutomationProperties.GetName(Owner);
            return string.IsNullOrEmpty(set) ? TextNamedRow.ShownText(Owner) : set;
        }
    }
}
