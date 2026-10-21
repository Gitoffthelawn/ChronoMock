namespace ChronoMock.App;

/// <summary>What the last copy to the clipboard from one button did.</summary>
public enum CopyState
{
    /// <summary>Nothing to say: the button reads as itself.</summary>
    Idle,

    /// <summary>The text is on the clipboard.</summary>
    Copied,

    /// <summary>Another program held the clipboard, so nothing was copied (rule 6 - said, never swallowed).</summary>
    Failed,
}

/// <summary>
/// What one copy button's last copy did, said on that button for a moment and then gone (R5-D81). One for
/// each of the four copy actions - the result's summary and diagnostics, the calculator's formats and its
/// command - which said it four different ways: two buttons sharing one line that never cleared, so a second
/// copy changed nothing on the screen, and a calculator that flashed "Copied" and said nothing at all when
/// the clipboard was busy.
/// </summary>
/// <remarks>
/// A model, not a timer in a view, so that the session's commands - which never hold a control (GUI rule
/// 15) - can report what they did, and a test can watch the moment end without a window.
///
/// The clipboard holds one text, so outcomes made with <see cref="Sibling"/> share a screen: when one says
/// what it did, any other still saying it stops at once, and the screen never claims two values copied.
/// </remarks>
public sealed class CopyOutcome : ObservableObject
{
    /// <summary>How long a button says what its copy did - the calculator's "Copied" moment, kept when the
    /// four became one.</summary>
    public static readonly TimeSpan ShownFor = TimeSpan.FromSeconds(1.5);

    private readonly Screen _screen;
    private readonly Func<TimeSpan, CancellationToken, Task> _wait;
    private CancellationTokenSource? _shown;
    private CopyState _state;

    public CopyOutcome()
        : this(new Screen(), Task.Delay)
    {
    }

    /// <summary>For a test, which decides when the moment ends.</summary>
    internal CopyOutcome(Func<TimeSpan, CancellationToken, Task> wait)
        : this(new Screen(), wait)
    {
    }

    private CopyOutcome(Screen screen, Func<TimeSpan, CancellationToken, Task> wait)
    {
        _screen = screen;
        _wait = wait;
    }

    /// <summary>What the button says now.</summary>
    public CopyState State
    {
        get => _state;
        private set => Set(ref _state, value);
    }

    /// <summary>An outcome for another copy button on the same screen (see the remarks).</summary>
    public CopyOutcome Sibling() => new(_screen, _wait);

    /// <summary>The copy happened, or the clipboard refused it. Said until <see cref="ShownFor"/> has passed,
    /// or until another copy on the screen speaks - a second copy from the same button starts the moment
    /// again rather than ending early on the first one's clock.</summary>
    public void Report(bool copied)
    {
        _screen.Speaks(this);
        StopShowing();
        var shown = new CancellationTokenSource();
        _shown = shown;
        State = copied ? CopyState.Copied : CopyState.Failed;
        _ = ClearAfterAsync(shown.Token);
    }

    /// <summary>Back to saying nothing.</summary>
    public void Clear()
    {
        StopShowing();
        State = CopyState.Idle;
    }

    // The source is cancelled and disposed by whoever replaces or ends the moment, and by nobody else - the
    // rule LatestAnswer follows, so nothing touches a disposed source and nothing leaks.
    private void StopShowing()
    {
        _shown?.Cancel();
        _shown?.Dispose();
        _shown = null;
    }

    private async Task ClearAfterAsync(CancellationToken token)
    {
        try
        {
            await _wait(ShownFor, token);
        }
        catch (OperationCanceledException)
        {
            return; // a newer copy, or Clear, ended this moment already
        }

        if (!token.IsCancellationRequested)
        {
            Clear();
        }
    }

    /// <summary>The copy buttons of one screen. Only the one that spoke last says anything.</summary>
    private sealed class Screen
    {
        private CopyOutcome? _speaking;

        public void Speaks(CopyOutcome outcome)
        {
            if (_speaking is { } other && !ReferenceEquals(other, outcome))
            {
                other.Clear();
            }

            _speaking = outcome;
        }
    }
}
