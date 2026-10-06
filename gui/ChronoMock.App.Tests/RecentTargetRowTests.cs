using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// PR B finding (i): the recent-application list says which file is gone, and where each one is.
/// </summary>
/// <remarks>
/// 🔴 Lost in the move to three phases (e0bbbae): the row kept only the file name, so the "(missing)" mark
/// and the folder beside the name went with the old panel while RecentTarget.IsMissing went on being
/// computed and tested, and the text "target.missing" stayed in both languages reached by nothing - the
/// comment over it promises the list "never goes quiet". Picking a file that was gone ended in a start
/// that failed without the list having said a word.
/// Reversal probe: drop the mark from RecentTargetRowTemplate in Themes/Parts.xaml and both tests redden.
/// </remarks>
public sealed class RecentTargetRowTests
{
    [Fact]
    public async Task The_setup_box_says_a_chosen_file_is_missing_and_says_nothing_of_one_that_is_there()
    {
        var present = Path.Combine(Path.GetTempPath(), $"chrono-recent-{Guid.NewGuid():N}.exe");
        await File.WriteAllTextAsync(present, string.Empty, TestContext.Current.CancellationToken);
        try
        {
            var gone = Path.Combine(Path.GetTempPath(), $"chrono-gone-{Guid.NewGuid():N}.exe");
            var (missingShown, presentShown) = await WpfTestHost.RunAsync(async () =>
            {
                var vm = new SessionViewModel();
                vm.SetTarget(present);
                vm.SetTarget(gone);
                await vm.RefreshRecentTargetsAsync();
                var view = new SetupPhaseView { DataContext = vm };
                LayoutProbe.Settle(view);
                var whileGone = Marked(view);

                vm.SetTarget(present);
                LayoutProbe.Settle(view);
                return (whileGone, Marked(view));
            });

            Assert.True(missingShown, "the box holds a file that is gone and does not say so");
            Assert.False(presentShown, "the box marks a file that is there as missing");
        }
        finally
        {
            File.Delete(present);
        }
    }

    [Fact]
    public void An_open_list_row_shows_the_folder_and_names_the_missing_file_for_a_screen_reader()
    {
        var (rows, names, statuses) = WpfTestHost.InvokeSettled(() =>
        {
            var there = new RecentTarget(@"C:\src\app\bin\x64\Release\app.exe");
            var gone = new RecentTarget(@"C:\src\app\bin\x86\Release\app.exe") { IsMissing = true };
            // From the setup's own merge of the parts, the copy the screen draws with: the application-wide
            // copy is read before App.xaml defines the converters its templates name.
            var setup = new SetupPhaseView();
            var panel = new StackPanel();
            var items = new[] { there, gone }.Select(t => new ComboBoxItem
            {
                Style = (Style)setup.FindResource("RecentTargetRow"),
                ContentTemplate = (DataTemplate)setup.FindResource("RecentTargetRowTemplate"),
                Content = t,
                DataContext = t, // what a list's own container gets from the list
            }).ToList();
            items.ForEach(i => panel.Children.Add(i));
            LayoutProbe.Settle(panel, LayoutProbe.WindowWidth, LayoutProbe.WindowHeight);

            var shown = items.Select(i => string.Join(" | ", LayoutProbe.Walk(i)
                .Where(e => e.Kind == "TextBlock" && e.IsVisible && e.Text.Length > 0)
                .OrderBy(e => e.Bounds.Left)
                .Select(e => e.Text))).ToList();
            return (shown,
                items.Select(AutomationProperties.GetName).ToList(),
                items.Select(AutomationProperties.GetItemStatus).ToList());
        });

        var mark = TranslationKeyConverter.Resolve("target.missing");
        Assert.Equal(@"app.exe | C:\src\app\bin\x64\Release", rows[0]);
        Assert.Equal($@"app.exe | C:\src\app\bin\x86\Release | {mark}", rows[1]);
        Assert.Equal(@"C:\src\app\bin\x64\Release\app.exe", names[0]);
        Assert.Equal($@"C:\src\app\bin\x86\Release\app.exe {mark}", names[1]);
        Assert.True(string.IsNullOrEmpty(statuses[0]), $"a file that is there has the status \"{statuses[0]}\"");
        Assert.Equal(mark, statuses[1]);
    }

    /// <summary>Whether the setup's recent-application box shows the missing mark.</summary>
    private static bool Marked(FrameworkElement view)
    {
        var mark = TranslationKeyConverter.Resolve("target.missing");
        return LayoutProbe.Walk(view).Any(e => e.Kind == "TextBlock" && e.IsVisible && e.Text == mark);
    }
}
