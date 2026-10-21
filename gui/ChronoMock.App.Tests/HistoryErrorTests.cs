using System.ComponentModel;
using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using ChronoMock.App.Controls;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// A history the store could not write is on the screen (rule 6, found in review on #136). The section is
/// folded and its summary says only how many sessions it holds, so the reason stood behind the fold, seen by
/// nobody and, for a screen reader, announced only once someone opened it. A clear or the removal of the last
/// row that the store refused left no rows, and the section went with its reason.
/// </summary>
public sealed class HistoryErrorTests
{
    private const string Refusal = "The drive is read-only.";

    /// <summary>What a binding sees is the value at the LAST notification: emptying the rows tells the section
    /// it goes before the store has answered, so the refusal has to tell it again.</summary>
    [Fact]
    public async Task A_clear_the_store_refused_keeps_the_section_and_its_reason_on_the_screen()
    {
        var vm = new SessionViewModel(new RefusingHistoryStore());
        var told = new List<bool>();
        ((INotifyPropertyChanged)vm).PropertyChanged += (_, e) =>
        {
            if (e.PropertyName == nameof(SessionViewModel.ShowsHistorySection))
            {
                told.Add(vm.ShowsHistorySection);
            }
        };

        await vm.ClearHistoryAsync();

        Assert.Equal((false, Refusal, true), (vm.HasHistory, vm.HistoryError, vm.ShowsHistorySection));
        Assert.True(told.Count > 0 && told[^1], $"the section was last told it shows: {string.Join(", ", told)}");
    }

    [Fact]
    public Task A_history_write_that_failed_opens_the_section_and_cuts_in()
        => WpfTestHost.RunAsync(async () =>
        {
            var vm = new SessionViewModel(new RefusingHistoryStore());
            var view = new ResultPhaseView { DataContext = vm };
            var window = OffScreenWindow.Show(view);
            var heard = new List<(object? From, string Said)>();
            EventHandler<string> listen = (from, said) => heard.Add((from, said));
            Announce.Announced += listen;
            try
            {
                var section = (Expander)LayoutProbe.FindNamed(view, "HistorySection")!;
                var line = LayoutProbe.FindNamed(view, "HistoryError")!;
                Assert.False(section.IsExpanded);

                await vm.ClearHistoryAsync();
                OffScreenWindow.Settle();

                Assert.True(section.IsVisible && section.IsExpanded && line.IsVisible);
                var deadline = DateTime.UtcNow + Announce.Quiet * 10;
                while (DateTime.UtcNow < deadline && !heard.Exists(h => ReferenceEquals(h.From, line)))
                {
                    await Task.Delay(TimeSpan.FromMilliseconds(20), TestContext.Current.CancellationToken);
                }

                Assert.Contains(heard, h => ReferenceEquals(h.From, line) && h.Said.Contains(Refusal, StringComparison.Ordinal));
                Assert.Equal(AutomationLiveSetting.Assertive, AutomationProperties.GetLiveSetting(line));
            }
            finally
            {
                Announce.Announced -= listen;
                window.Close();
            }

            return 0;
        });

    /// <summary>A history store on a drive that takes no writes: every change answers with the reason.</summary>
    private sealed class RefusingHistoryStore : ISessionHistoryStore
    {
        public IReadOnlyList<SessionRecord> Load() => [];

        public void Append(SessionRecord record) => throw new IOException(Refusal);

        public void Remove(SessionRecord record) => throw new IOException(Refusal);

        public void Clear() => throw new IOException(Refusal);

        public Task<string> AppendAsync(SessionRecord record) => Task.FromResult(Refusal);

        public Task<string> RemoveAsync(SessionRecord record) => Task.FromResult(Refusal);

        public Task<string> ClearAsync() => Task.FromResult(Refusal);
    }
}
