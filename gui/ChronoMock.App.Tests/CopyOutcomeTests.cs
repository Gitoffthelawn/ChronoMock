namespace ChronoMock.App.Tests;

/// <summary>
/// What a copy button says after a copy (R5-D81): the outcome for a moment, then nothing, and on one screen
/// only the button that copied last.
/// </summary>
public sealed class CopyOutcomeTests
{
    [Fact]
    public void An_outcome_says_nothing_until_a_copy_is_reported()
        => Assert.Equal(CopyState.Idle, new CopyOutcome().State);

    [Fact]
    public async Task A_copy_is_said_until_its_moment_passes_and_then_nothing_is()
    {
        var clock = new ManualWait();
        var outcome = new CopyOutcome(clock.Wait);

        outcome.Report(copied: true);
        Assert.Equal(CopyState.Copied, outcome.State);
        Assert.Equal(CopyOutcome.ShownFor, clock.Asked);

        await clock.PassAsync();
        Assert.Equal(CopyState.Idle, outcome.State);
    }

    [Fact]
    public async Task A_busy_clipboard_is_said_as_a_failure_and_goes_the_same_way()
    {
        var clock = new ManualWait();
        var outcome = new CopyOutcome(clock.Wait);

        outcome.Report(copied: false);
        Assert.Equal(CopyState.Failed, outcome.State);

        await clock.PassAsync();
        Assert.Equal(CopyState.Idle, outcome.State);
    }

    [Fact]
    public async Task A_second_copy_starts_the_moment_again_rather_than_ending_on_the_first_ones_clock()
    {
        var clock = new ManualWait();
        var outcome = new CopyOutcome(clock.Wait);

        outcome.Report(copied: true);
        outcome.Report(copied: false);

        await clock.PassAsync(); // the first moment's wait, cancelled by the second copy
        Assert.Equal(CopyState.Failed, outcome.State);

        await clock.PassAsync();
        Assert.Equal(CopyState.Idle, outcome.State);
    }

    [Fact]
    public void A_copy_from_another_button_on_the_screen_silences_this_one()
    {
        var clock = new ManualWait();
        var summary = new CopyOutcome(clock.Wait);
        var diagnostics = summary.Sibling();

        summary.Report(copied: true);
        diagnostics.Report(copied: true);

        Assert.Equal(CopyState.Idle, summary.State);
        Assert.Equal(CopyState.Copied, diagnostics.State);
    }

    [Fact]
    public void Outcomes_made_apart_are_on_different_screens()
    {
        var clock = new ManualWait();
        var one = new CopyOutcome(clock.Wait);
        var other = new CopyOutcome(clock.Wait);

        one.Report(copied: true);
        other.Report(copied: true);

        Assert.Equal(CopyState.Copied, one.State);
        Assert.Equal(CopyState.Copied, other.State);
    }

    /// <summary>A wait the test ends, one at a time and in the order they were asked for. A cancelled wait
    /// throws, as Task.Delay does.</summary>
    private sealed class ManualWait
    {
        private readonly Queue<(TaskCompletionSource Done, CancellationToken Token)> _waits = new();

        public TimeSpan Asked { get; private set; }

        public Task Wait(TimeSpan duration, CancellationToken token)
        {
            Asked = duration;
            var done = new TaskCompletionSource(); // continuations run in place, so a pass is seen at once
            _waits.Enqueue((done, token));
            return done.Task;
        }

        public async Task PassAsync()
        {
            var (done, token) = _waits.Dequeue();
            if (token.IsCancellationRequested)
            {
                done.SetCanceled(token);
            }
            else
            {
                done.SetResult();
            }

            // A margin for a test host that posts continuations rather than running them in place.
            await Task.Delay(TimeSpan.FromMilliseconds(50), TestContext.Current.CancellationToken);
        }
    }
}
