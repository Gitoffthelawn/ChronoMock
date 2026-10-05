using System.Diagnostics;

namespace ChronoMock.App;

/// <summary>
/// The answer to the latest input, and only to it (R4-W4, R4-S21, R4-S25).
/// <para>
/// A screen that asks the engine something gets the answer later, and in that gap the input can move on:
/// a field is edited, another scenario is chosen, a zone changes. Every one of those faults had the same
/// shape - an answer was applied to an input it was never asked for, or an action used the answer on
/// screen while the input beside it already said something else. "Use this date" sent the date from
/// before the edit, a late scenario overwrote the one chosen after it, a session started with the old date.
/// </para>
/// <para>
/// So each input carries a generation. Every change of the input takes a new one (<see cref="Ask"/> when an
/// answer will be computed for it, <see cref="Settle"/> when the input is its own answer, a date typed by
/// hand). A run takes a <see cref="Ticket"/> for the newest generation, and its answer is accepted only while
/// that generation is still the newest. <see cref="IsCurrent"/> says whether the answer on screen belongs to
/// the input on screen, and <see cref="WhenCurrentAsync"/> lets an action wait for that rather than act on
/// whatever happens to be shown.
/// </para>
/// <para>
/// The cancellation sources follow the rule <see cref="Calc.Debounce"/> follows, for the same reason: exactly
/// one party disposes a source. A ticket that is superseded is cancelled AND disposed by whatever superseded
/// it, and a ticket that finishes while still the running one releases itself. Nothing touches a source
/// after it is disposed, and nothing is leaked.
/// </para>
/// <para>
/// Used from the dispatcher in the application, and from pool threads in tests, so its state sits behind a
/// lock. Nothing outside this class runs under that lock: waiters resume asynchronously and the event is
/// raised after it is released.
/// </para>
/// </summary>
internal sealed class LatestAnswer
{
    private readonly object _gate = new();
    private long _asked;
    private long _answered;
    private CancellationTokenSource? _running;
    private TaskCompletionSource<bool>? _waiting;

    /// <summary>Raised when <see cref="IsCurrent"/> flips, in either direction.</summary>
    public event EventHandler? CurrentChanged;

    /// <summary>Whether the answer on screen belongs to the newest input. False from an edit until the
    /// answer to it lands.</summary>
    public bool IsCurrent
    {
        get
        {
            lock (_gate)
            {
                return _answered == _asked;
            }
        }
    }

    /// <summary>The input changed and an answer will be computed for it. Whatever is computing an answer to
    /// an older input is cancelled, since that answer would be thrown away when it came.</summary>
    public void Ask() => Change(answered: false);

    /// <summary>The input changed and is its own answer - a date typed by hand, or a state that has no
    /// answer to compute. Whatever is computing an answer to an older input is cancelled and will be
    /// refused, so it can never overwrite this.</summary>
    public void Settle() => Change(answered: true);

    /// <summary>
    /// Start computing the answer to the newest input. Null when there is nothing to compute, because the
    /// newest input already has its answer - a run that was scheduled before a <see cref="Settle"/> finds
    /// nothing to do rather than landing an answer to an input that has since been answered.
    /// </summary>
    public Ticket? Begin()
    {
        CancellationTokenSource? previous;
        Ticket ticket;
        lock (_gate)
        {
            if (_answered == _asked)
            {
                return null;
            }

            previous = _running;
            var source = new CancellationTokenSource();
            _running = source;
            ticket = new Ticket(_asked, source);
        }

        Put(previous);
        return ticket;
    }

    /// <summary>
    /// Whether this ticket's answer may be applied: its generation is still the newest and nothing has
    /// cancelled it. Call it immediately before applying, on the thread that applies, and apply only on
    /// true. Asking again for the same ticket is harmless, so a failure while applying can still report
    /// itself.
    /// <para>
    /// 🔴 Accepting does NOT make the input current yet - <see cref="Release"/> does, after the answer is
    /// on screen. Waking a waiting action here let it read the screen before the answer was applied:
    /// measured as a Copy pressed after an edit that copied the value from before it, because the copy ran
    /// on one thread while the result was still being written on another.
    /// </para>
    /// </summary>
    public bool Accept(Ticket ticket)
    {
        ArgumentNullException.ThrowIfNull(ticket);
        lock (_gate)
        {
            if (ticket.Generation != _asked || ticket.Token.IsCancellationRequested)
            {
                return false;
            }

            ticket.Accepted = true;
            return true;
        }
    }

    /// <summary>
    /// The run is over. When its answer was accepted and its input is still the newest, the input is
    /// answered from here on - the screen shows the answer, and whatever waited for it may read it now.
    /// <para>
    /// It also lets go of the run's source, but only while that is still the running one. A ticket that
    /// was superseded finds another source in the slot and leaves it, because whatever superseded it
    /// already disposed its own. Call it exactly once per ticket, from a finally.
    /// </para>
    /// </summary>
    public void Release(Ticket ticket)
    {
        ArgumentNullException.ThrowIfNull(ticket);
        var flipped = false;
        var dispose = false;
        TaskCompletionSource<bool>? waiting = null;
        lock (_gate)
        {
            if (ticket.Accepted && ticket.Generation == _asked && _answered != _asked)
            {
                _answered = _asked;
                waiting = _waiting;
                _waiting = null;
                flipped = true;
            }

            if (ReferenceEquals(_running, ticket.Source))
            {
                _running = null;
                dispose = true;
            }
        }

        if (dispose)
        {
            ticket.Source.Dispose();
        }

        waiting?.TrySetResult(true);
        if (flipped)
        {
            CurrentChanged?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>
    /// Wait until the answer on screen belongs to the newest input, at most <paramref name="limit"/>. True
    /// when it does. Completes at once, without yielding, when nothing is being computed - so an action
    /// that waits behaves exactly as it did before whenever there is nothing to wait for.
    /// <para>
    /// The newest input is read again after every wake, so an edit made while waiting is waited for too:
    /// the action then works on the answer to what the screen shows, not to what it showed when the wait
    /// began.
    /// </para>
    /// </summary>
    public async Task<bool> WhenCurrentAsync(TimeSpan limit)
    {
        var clock = Stopwatch.StartNew();
        while (true)
        {
            Task<bool> next;
            lock (_gate)
            {
                if (_answered == _asked)
                {
                    return true;
                }

                _waiting ??= new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
                next = _waiting.Task;
            }

            var left = limit - clock.Elapsed;
            if (left <= TimeSpan.Zero)
            {
                return false;
            }

            try
            {
                await next.WaitAsync(left);
            }
            catch (TimeoutException)
            {
                return IsCurrent;
            }
        }
    }

    /// <summary>
    /// The screen is going away: whatever is being computed is cancelled now rather than left to finish for
    /// nobody - the engine client kills its process on cancellation (R4/13, from the review of #86). Nothing
    /// else changes: the generation stays, so an answer already on its way is refused like any other answer
    /// to a cancelled run, and the cancelled run finds its source gone from the slot and leaves it, as a
    /// superseded run does.
    /// </summary>
    public void Abandon()
    {
        CancellationTokenSource? running;
        lock (_gate)
        {
            running = _running;
            _running = null;
        }

        Put(running);
    }

    private void Change(bool answered)
    {
        CancellationTokenSource? previous;
        TaskCompletionSource<bool>? waiting = null;
        bool flipped;
        lock (_gate)
        {
            var wasCurrent = _answered == _asked;
            _asked++;
            if (answered)
            {
                _answered = _asked;
                waiting = _waiting;
                _waiting = null;
            }

            previous = _running;
            _running = null;
            flipped = wasCurrent != answered;
        }

        Put(previous);
        waiting?.TrySetResult(true);
        if (flipped)
        {
            CurrentChanged?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>Put down a superseded run's source: cancel it THEN dispose it, both here. Cancellation
    /// callbacks run synchronously, so by the time Cancel returns the run is already on its way out and
    /// nothing will touch the source again. A callback that throws makes Cancel throw, and the source is
    /// disposed all the same - the throw still reaches the edit that caused it, loudly.</summary>
    private static void Put(CancellationTokenSource? previous)
    {
        if (previous is null)
        {
            return;
        }

        try
        {
            previous.Cancel();
        }
        finally
        {
            previous.Dispose();
        }
    }

    /// <summary>One run's claim to answer one generation of the input.</summary>
    internal sealed class Ticket
    {
        internal Ticket(long generation, CancellationTokenSource source)
        {
            Generation = generation;
            Source = source;
            Token = source.Token;
        }

        /// <summary>The generation of the input this run answers.</summary>
        public long Generation { get; }

        /// <summary>Cancelled when a newer input makes this run's answer worthless. Read once, when the
        /// ticket is made, so it stays usable after the source is disposed.</summary>
        public CancellationToken Token { get; }

        internal CancellationTokenSource Source { get; }

        /// <summary>Whether the run's answer was accepted. Read and written under the owner's lock.</summary>
        internal bool Accepted { get; set; }
    }
}
