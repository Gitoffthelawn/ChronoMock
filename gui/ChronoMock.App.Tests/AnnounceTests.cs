using System.Diagnostics;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Data;
using ChronoMock.App.Calc;
using ChronoMock.App.Controls;
using ChronoMock.App.Views;

namespace ChronoMock.App.Tests;

/// <summary>
/// What a screen reader is told without looking (R5-U1, R5-D83): before this, nothing on the interface was
/// a live region, so a status, a verdict, a calculator answer or an error that changed where the reader was
/// not reached nobody. The announcements are read through the internal seam, since a test process has no
/// UI Automation client and the raise to one is skipped there - the live listener measures that part.
/// </summary>
public sealed class AnnounceTests
{
    private static readonly TimeSpan Deadline = TimeSpan.FromSeconds(5);

    [Fact]
    public Task A_changed_value_is_read_once_as_the_text_on_the_screen_after_the_quiet_moment()
        => OnText(async (text, heard) =>
        {
            var clock = Stopwatch.StartNew();
            text.Text = "Running";
            Announce.SetPolite(text, "running");

            var said = await NextAsync(heard);
            Assert.Equal("Running", said);
            Assert.True(clock.Elapsed >= Announce.Quiet * 0.9, $"read after {clock.Elapsed.TotalMilliseconds} ms, before the quiet moment");
            Assert.Equal(AutomationLiveSetting.Polite, AutomationProperties.GetLiveSetting(text));
            await NothingMoreAsync(heard);
        });

    /// <summary>A number typed key by key is one announcement, of where it ended.</summary>
    [Fact]
    public Task A_run_of_changes_is_one_announcement_of_the_last()
        => OnText(async (text, heard) =>
        {
            foreach (var step in new[] { "1", "15", "150" })
            {
                text.Text = $"{step} days";
                Announce.SetPolite(text, step);
                await Task.Delay(Announce.Quiet / 5, TestContext.Current.CancellationToken);
            }

            Assert.Equal("150 days", await NextAsync(heard));
            await NothingMoreAsync(heard);
        });

    [Fact]
    public Task A_change_while_hidden_is_read_when_it_shows()
        => OnText(async (text, heard) =>
        {
            text.Visibility = Visibility.Collapsed;
            text.Text = "Works";
            Announce.SetPolite(text, true);
            await NothingMoreAsync(heard);

            text.Visibility = Visibility.Visible;

            Assert.Equal("Works", await NextAsync(heard));
        });

    /// <summary>Showing alone is no news: the phases are switched by visibility, and coming back from the
    /// calculator must not read the verdict again.</summary>
    [Fact]
    public Task Showing_again_with_nothing_new_says_nothing()
        => OnText(async (text, heard) =>
        {
            text.Text = "Works";
            Announce.SetPolite(text, true);
            Assert.Equal("Works", await NextAsync(heard));

            text.Visibility = Visibility.Collapsed;
            OffScreenWindow.Settle();
            text.Visibility = Visibility.Visible;

            await NothingMoreAsync(heard);
        });

    [Fact]
    public Task A_value_whose_element_shows_no_text_is_not_read()
        => OnText(async (text, heard) =>
        {
            text.Text = string.Empty;
            Announce.SetAssertive(text, "cleared");

            await NothingMoreAsync(heard);
            Assert.Equal(AutomationLiveSetting.Assertive, AutomationProperties.GetLiveSetting(text));
        });

    [Fact]
    public Task A_copy_button_says_what_the_press_did_and_not_that_it_went_back()
        => WpfTestHost.RunAsync(async () =>
        {
            var outcome = new CopyOutcome((_, _) => new TaskCompletionSource().Task);
            var button = new CopyButton { Label = "Copy", Outcome = outcome };
            var window = OffScreenWindow.Show(new StackPanel { Children = { button } });
            var heard = Listen(button);
            try
            {
                outcome.Report(copied: true);
                Assert.Equal(TranslationKeyConverter.Resolve("copy.copied"), await NextAsync(heard));

                outcome.Clear();
                await NothingMoreAsync(heard);

                outcome.Report(copied: false);
                Assert.Equal(TranslationKeyConverter.Resolve("copy.busy"), await NextAsync(heard));
            }
            finally
            {
                heard.Stop();
                window.Close();
            }

            return 0;
        });

    /// <summary>The calculator's answer is read as the screen shows it - the weekday and the date, which
    /// the line builds from runs - when a step moves it.</summary>
    [Fact]
    public Task The_calculator_reads_its_answer_when_a_step_moves_it()
        => WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(CalculatorSheetEngine.Answer), TestCatalogues.Library(TestCatalogues.Shipped()));
            await vm.EnsureComputedAsync();
            vm.SelectedBase = vm.BaseKinds.Single(b => b.Kind == BaseKind.Specific);
            var view = new CalculatorView { DataContext = vm };
            var window = OffScreenWindow.Show(view);
            var heard = Listen(null);
            try
            {
                await Task.Delay(Announce.Quiet * 3, TestContext.Current.CancellationToken);
                heard.Clear();
                var before = vm.ResultDate;

                vm.AddStep();
                vm.Steps[0].Amount = "3";

                var said = await NextAsync(heard, s => s.Contains(vm.ResultDate, StringComparison.Ordinal) && vm.ResultDate != before);
                Assert.Contains(vm.ResultWeekday, said, StringComparison.Ordinal);
            }
            finally
            {
                heard.Stop();
                window.Close();
            }

            return 0;
        });

    /// <summary>Which line of each screen is announced, in which tone, and what news it follows. A line
    /// taken off or added here changes what a reader hears, so it changes this list.</summary>
    [Fact]
    public void Every_screen_announces_the_lines_it_says_it_does()
    {
        var found = WpfTestHost.Invoke(() => new Dictionary<string, string[]>
        {
            ["setup"] = Announced(new SetupPhaseView()),
            ["session"] = Announced(new SessionPhaseView()),
            ["result"] = Announced(new ResultPhaseView()),
            ["calculator"] = Announced(new CalculatorView()),
        });

        Assert.Equal(["Polite TargetNoteKey"], found["setup"]);
        Assert.Equal(["Assertive InFlightErrorKey", "Polite StatusKey", "Polite VerdictLabelKey"], found["session"]);
        Assert.Equal(["Polite HistoryError", "Polite ShowsResultPhase"], found["result"]);
        Assert.Equal(["Assertive Error", "Polite ResultDate", "Polite ResultTime"], found["calculator"]);
    }

    private static string[] Announced(FrameworkElement view)
    {
        var lines = new List<string>();
        Walk(view);
        lines.Sort(StringComparer.Ordinal);
        return [.. lines];

        void Walk(DependencyObject node)
        {
            foreach (var (tone, property) in new[] { ("Polite", Announce.PoliteProperty), ("Assertive", Announce.AssertiveProperty) })
            {
                if (BindingOperations.GetBinding(node, property) is { } binding)
                {
                    lines.Add($"{tone} {binding.Path.Path}");
                }
            }

            foreach (var child in LogicalTreeHelper.GetChildren(node).OfType<DependencyObject>())
            {
                Walk(child);
            }
        }
    }

    /// <summary>The announcements of one element (or of any, for null), as they come.</summary>
    private sealed class Heard
    {
        private readonly object? _from;
        private readonly Queue<string> _said = new();

        public Heard(object? from)
        {
            _from = from;
            Announce.Announced += OnAnnounced;
        }

        public int Count => _said.Count;

        public bool TryTake(out string said) => _said.TryDequeue(out said!);

        public void Clear() => _said.Clear();

        public void Stop() => Announce.Announced -= OnAnnounced;

        private void OnAnnounced(object? sender, string said)
        {
            if (_from is null || ReferenceEquals(sender, _from))
            {
                _said.Enqueue(said);
            }
        }
    }

    private static Heard Listen(object? from) => new(from);

    private static async Task<string> NextAsync(Heard heard, Func<string, bool>? wanted = null)
    {
        var deadline = DateTime.UtcNow + Deadline;
        while (DateTime.UtcNow < deadline)
        {
            while (heard.TryTake(out var said))
            {
                if (wanted is null || wanted(said))
                {
                    return said;
                }
            }

            await Task.Delay(TimeSpan.FromMilliseconds(20), TestContext.Current.CancellationToken);
        }

        Assert.Fail("nothing was announced");
        return string.Empty;
    }

    /// <summary>Three quiet moments with nothing announced.</summary>
    private static async Task NothingMoreAsync(Heard heard)
    {
        await Task.Delay(Announce.Quiet * 3, TestContext.Current.CancellationToken);
        Assert.Equal(0, heard.Count);
    }

    private static Task OnText(Func<TextBlock, Heard, Task> check)
        => WpfTestHost.RunAsync(async () =>
        {
            var text = new TextBlock();
            var window = OffScreenWindow.Show(new StackPanel { Children = { text } });
            var heard = Listen(text);
            try
            {
                await check(text, heard);
            }
            finally
            {
                heard.Stop();
                window.Close();
            }

            return 0;
        });
}
