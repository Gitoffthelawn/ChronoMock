using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Controls.Primitives;

namespace ChronoMock.App.Controls;

/// <summary>
/// The toggle that folds and unfolds a section, named for assistive tech by whatever labels it - the section
/// itself, in the section template (R5-U2).
/// </summary>
/// <remarks>
/// 🔴 WHY A TYPE AND NOT ONLY <see cref="AutomationProperties.LabeledByProperty"/>. WPF reads a label through
/// the label's EXISTING automation element (<c>UIElementAutomationPeer.GetLabeledByCore</c> calls
/// <c>GetAutomationPeer()</c>, which never creates one). A reader that walked the window from the top has
/// created the section's element on the way down, so the toggle has a name - but focus reaching the toggle
/// first left it nameless, measured: asked before the section, every toggle on the three phase screens read
/// "", asked after it, each read its section's header. This element creates the label's element itself.
/// </remarks>
public sealed class SectionToggle : ToggleButton
{
    protected override AutomationPeer OnCreateAutomationPeer() => new SectionToggleAutomationPeer(this);

    private sealed class SectionToggleAutomationPeer(SectionToggle owner) : ToggleButtonAutomationPeer(owner)
    {
        protected override string GetNameCore()
        {
            var name = base.GetNameCore();
            if (!string.IsNullOrEmpty(name))
            {
                return name;
            }

            return AutomationProperties.GetLabeledBy(Owner) is UIElement label
                && UIElementAutomationPeer.CreatePeerForElement(label) is { } labelPeer
                ? labelPeer.GetName() ?? string.Empty
                : string.Empty;
        }
    }
}
