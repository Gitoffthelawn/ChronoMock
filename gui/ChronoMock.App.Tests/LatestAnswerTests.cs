namespace ChronoMock.App.Tests;

/// <summary>
/// The one mechanism behind "an answer applies only to the input it was asked for" (R4-W4, R4-S25): the
/// calculator's result, its reverse analysis and the session's moment field all stand on it, so its rules
/// are asserted here once, without a screen or an engine.
/// </summary>
public class LatestAnswerTests
{
    private static readonly TimeSpan Short = TimeSpan.FromMilliseconds(200);

    [Fact]
    public void An_answer_to_the_newest_input_is_accepted_and_makes_it_current_once_its_run_is_over()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        Assert.False(answer.IsCurrent);

        var ticket = Assert.IsType<LatestAnswer.Ticket>(answer.Begin());

        Assert.True(answer.Accept(ticket));
        Assert.False(answer.IsCurrent); // accepted, not yet on screen
        answer.Release(ticket);
        Assert.True(answer.IsCurrent);
    }

    [Fact]
    public async Task A_waiting_action_wakes_only_after_the_answer_is_applied()
    {
        // Measured as a Copy pressed after an edit that copied the value from before it: the waiter woke at
        // Accept and read the screen while the result was still being written on another thread.
        var answer = new LatestAnswer();
        answer.Ask();
        var ticket = answer.Begin()!;
        var wait = answer.WhenCurrentAsync(TimeSpan.FromSeconds(5));

        Assert.True(answer.Accept(ticket));
        await Task.Delay(Short, TestContext.Current.CancellationToken); // room for a wrong wake
        Assert.False(wait.IsCompleted);

        answer.Release(ticket);
        Assert.True(await wait);
    }

    /// <summary>
    /// R4/13 (from the review of #86): a window that closes cancels what is being computed, so the engine
    /// process is stopped rather than left to finish for nobody. The run then releases as a superseded one
    /// does - it finds its source gone from the slot and leaves it - and a new question still works.
    /// </summary>
    [Fact]
    public void Abandoning_cancels_the_run_in_flight_and_refuses_its_answer()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        var ticket = answer.Begin()!;

        answer.Abandon();

        Assert.True(ticket.Token.IsCancellationRequested);
        Assert.False(answer.Accept(ticket));
        answer.Release(ticket); // must not touch the source Abandon already put down
        Assert.False(answer.IsCurrent);

        answer.Ask();
        var next = answer.Begin()!;
        Assert.True(answer.Accept(next));
        answer.Release(next);
        Assert.True(answer.IsCurrent);
    }

    [Fact]
    public void An_answer_to_an_input_that_has_moved_on_is_refused_and_its_run_cancelled()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        var old = answer.Begin()!;

        answer.Ask();

        Assert.True(old.Token.IsCancellationRequested);
        Assert.False(answer.Accept(old));
        Assert.False(answer.IsCurrent);
    }

    [Fact]
    public void A_run_that_ends_without_an_accepted_answer_leaves_the_input_unanswered()
    {
        // A cancelled or failed run releases its ticket from a finally like any other - only an answer
        // that was accepted may count as the answer on screen.
        var answer = new LatestAnswer();
        answer.Ask();
        var ticket = answer.Begin()!;

        answer.Release(ticket);

        Assert.False(answer.IsCurrent);
    }

    [Fact]
    public void A_ticket_from_a_finished_run_is_refused_once_the_input_moves_on()
    {
        // The finished run's source is disposed, not cancelled, so its token says nothing - the generation
        // is what refuses it.
        var answer = new LatestAnswer();
        answer.Ask();
        var finished = answer.Begin()!;
        answer.Accept(finished);
        answer.Release(finished);

        answer.Ask();

        Assert.False(finished.Token.IsCancellationRequested);
        Assert.False(answer.Accept(finished));
    }

    [Fact]
    public void An_input_that_is_its_own_answer_refuses_whatever_was_still_being_computed()
    {
        // A date typed by hand while a scenario is computed: the scenario must never overwrite it.
        var answer = new LatestAnswer();
        answer.Ask();
        var scenario = answer.Begin()!;

        answer.Settle();

        Assert.True(answer.IsCurrent);
        Assert.True(scenario.Token.IsCancellationRequested);
        Assert.False(answer.Accept(scenario));
    }

    [Fact]
    public void A_run_scheduled_before_a_settle_finds_nothing_to_do()
    {
        // The calculator's debounced run that fires after a preset settled the result (R4-S21).
        var answer = new LatestAnswer();
        answer.Ask();
        answer.Settle();

        Assert.Null(answer.Begin());
    }

    [Fact]
    public void A_second_run_for_the_same_input_supersedes_the_first()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        var first = answer.Begin()!;
        var second = answer.Begin()!;

        Assert.True(first.Token.IsCancellationRequested);
        Assert.False(answer.Accept(first));
        Assert.True(answer.Accept(second));
    }

    [Fact]
    public void Accepting_the_same_ticket_twice_is_harmless()
    {
        // The calculator accepts once to apply and again to report a failure while applying.
        var answer = new LatestAnswer();
        answer.Ask();
        var ticket = answer.Begin()!;

        Assert.True(answer.Accept(ticket));
        Assert.True(answer.Accept(ticket));
    }

    [Fact]
    public async Task Waiting_with_nothing_being_computed_completes_at_once()
    {
        // So Start, Use and Copy behave exactly as before whenever there is nothing to wait for: the task
        // is already complete when it is returned, so awaiting it does not even yield.
        var answer = new LatestAnswer();

        var wait = answer.WhenCurrentAsync(Short);

        Assert.True(wait.IsCompletedSuccessfully);
        Assert.True(await wait);
    }

    [Fact]
    public async Task Waiting_follows_an_edit_made_during_the_wait_to_its_own_answer()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        var first = answer.Begin()!;
        var wait = answer.WhenCurrentAsync(TimeSpan.FromSeconds(5));

        answer.Ask(); // edited while waiting
        Assert.False(answer.Accept(first));
        var second = answer.Begin()!;
        Assert.False(wait.IsCompleted);

        Assert.True(answer.Accept(second));
        answer.Release(second);
        Assert.True(await wait);
    }

    [Fact]
    public async Task A_wait_that_runs_out_says_so()
    {
        var answer = new LatestAnswer();
        answer.Ask();

        Assert.False(await answer.WhenCurrentAsync(Short));
    }

    [Fact]
    public void The_change_event_fires_on_flips_only()
    {
        var answer = new LatestAnswer();
        var flips = 0;
        answer.CurrentChanged += (_, _) => flips++;

        answer.Ask();
        answer.Ask(); // still not current - no flip
        var ticket = answer.Begin()!;
        answer.Accept(ticket);
        answer.Release(ticket);
        answer.Settle(); // current to current - no flip

        Assert.Equal(2, flips);
    }

    [Fact]
    public void Releasing_a_superseded_ticket_leaves_the_running_one_alone()
    {
        var answer = new LatestAnswer();
        answer.Ask();
        var old = answer.Begin()!;
        answer.Ask();
        var running = answer.Begin()!;

        answer.Release(old);

        // Still usable and still the one that answers: the old ticket's release did not touch its source.
        Assert.False(running.Token.IsCancellationRequested);
        Assert.True(answer.Accept(running));
        answer.Release(running);
    }

    [Fact]
    public void A_superseded_source_is_disposed_even_when_a_cancellation_callback_throws()
    {
        // Cancel runs the callbacks synchronously and throws when one of them does. The throw must still
        // reach the edit that caused it - and must not leave the superseded source undisposed behind it.
        var answer = new LatestAnswer();
        answer.Ask();
        var ticket = answer.Begin()!;
        ticket.Token.Register(() => throw new InvalidOperationException("a callback failed"));

        Assert.Throws<AggregateException>(answer.Ask);
        Assert.Throws<ObjectDisposedException>(() => ticket.Source.Token.WaitHandle);
    }

    [Fact]
    public async Task A_storm_of_edits_and_runs_never_touches_a_disposed_source()
    {
        // The ownership rule Debounce follows, under load: whatever supersedes a run disposes its source,
        // and a run that finishes while current disposes its own. Cancel on a disposed source is the one
        // call that throws, so a broken rule shows up here as an ObjectDisposedException.
        var answer = new LatestAnswer();
        var runs = Enumerable.Range(0, 8).Select(_ => Task.Run(() =>
        {
            for (var i = 0; i < 2_000; i++)
            {
                answer.Ask();
                if (answer.Begin() is { } ticket)
                {
                    answer.Accept(ticket);
                    answer.Release(ticket);
                }
            }
        }, TestContext.Current.CancellationToken));

        await Task.WhenAll(runs);
        answer.Settle();
        Assert.True(answer.IsCurrent);
    }
}
