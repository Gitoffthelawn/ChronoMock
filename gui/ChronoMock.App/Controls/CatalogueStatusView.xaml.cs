using System.Windows.Controls;

namespace ChronoMock.App.Controls;

/// <summary>
/// What stands in a scenario list's place until it has rows: reading, failed (and why), empty. Bound to a
/// <see cref="Calc.CatalogueStatus"/>, and shared by the calculator's list and the substitution panel's.
/// </summary>
public partial class CatalogueStatusView : UserControl
{
    public CatalogueStatusView() => InitializeComponent();
}
