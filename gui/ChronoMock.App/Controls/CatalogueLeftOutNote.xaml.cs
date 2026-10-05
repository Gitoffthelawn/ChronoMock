using System.Windows.Controls;

namespace ChronoMock.App.Controls;

/// <summary>
/// The line under a scenario list that counts the preset files the engine left out, with the files and
/// reasons on its tooltip. Bound to a <see cref="Calc.CatalogueStatus"/>, under both lists.
/// </summary>
public partial class CatalogueLeftOutNote : UserControl
{
    public CatalogueLeftOutNote() => InitializeComponent();
}
