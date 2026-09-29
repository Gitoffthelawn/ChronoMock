using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;

namespace ChronoMock.Protocol;

/// <summary>
/// Drives one core process over the machine protocol (ADR-6): spawns <c>chrono __core</c>, sends the
/// <c>start</c> command first, then relays the event stream.
/// <para>
/// The core emits <c>ready</c> FIRST and only then reads the <c>start</c> line, so this client gates on
/// <c>ready</c> before sending - see <see cref="Connect"/>. This paragraph used to claim the opposite and
/// call docs/08 a drift - the doc was right and the comment was stale, which is worse than no comment at
/// all because a reader trusts it over the code (R2-N1).
/// </para>
/// The GUI is a client of this protocol, not FFI, so it stays AnyCPU and lets the core match the target's bitness.
/// </summary>
public sealed class CoreClient : IAsyncDisposable
{
    private static readonly UTF8Encoding Utf8NoBom = new(encoderShouldEmitUTF8Identifier: false);

    /// <summary>How many characters of a line that was not an event go into the diagnostics block. The
    /// line can be up to <see cref="ProtocolJson.MaxProtocolLine"/> bytes, and the start of it is what
    /// says what wrote it.</summary>
    private const int NoiseSampleChars = 200;

    /// <summary>How many unread events the client will hold before it makes the reader block.
    ///
    /// Every other input channel in this tool is bounded - MAX_WS_BYTES, MAX_PROTOCOL_LINE,
    /// MAX_QUEUED_EVENTS, the seqlock read budget, the business-day walk, CoreDiagnostics.MaxLines - and this one
    /// was not, so a consumer that stopped draining (a UI thread parked on a modal dialog) grew it for
    /// as long as the session ran. The core emits about one event a second, so the cap is thousands of
    /// times what a healthy session queues, and BlockingFull is the right full-behaviour here: dropping
    /// an event would lose a verdict or an `ended`, and back-pressure onto a core we started ourselves
    /// costs nothing worse than a paused reader.</summary>
    private const int MaxQueuedEvents = 4096;

    private readonly Process _process;
    private readonly Channel<ChronoEvent> _events = Channel.CreateBounded<ChronoEvent>(
        new BoundedChannelOptions(MaxQueuedEvents)
        {
            SingleWriter = true,
            FullMode = BoundedChannelFullMode.Wait,
        });
    private readonly CoreDiagnostics _log = new();
    private readonly object _stdinLock = new();
    private readonly Task _readLoop;
    private readonly Task _stderrDrain;
    /// <summary>Set once the read loop has handed on the core's <c>ended</c>, the last line a core that
    /// ended cleanly writes. Dispose waits on it before it closes the event stream.</summary>
    private readonly TaskCompletionSource _endedRead = new(TaskCreationOptions.RunContinuationsAsynchronously);
    // The one shutdown every DisposeAsync caller awaits. See DisposeAsync. Declared above `_disposed` on
    // purpose: the unused-definition scan in crates/cli/tests/hygiene.rs reads the constructor below as part
    // of the field right above it, and that has to stay one the public guard keeps alive.
    private readonly object _disposalLock = new();
    private Task? _disposal;
    private int _disposed;

    private CoreClient(Process process)
    {
        _process = process;
        _readLoop = Task.Run(ReadEventsAsync);
        _stderrDrain = Task.Run(DrainStderrAsync);
    }

    /// <summary>The core's event stream. Completes when the core closes its stdout (it exited).</summary>
    public ChannelReader<ChronoEvent> Events => _events.Reader;

    /// <summary>
    /// Human-side diagnostics: the lines on the core's stdout that were not events, then the core's stderr
    /// lines (see <see cref="CoreDiagnostics.Snapshot"/>). Never on the protocol path - surfaced for logging,
    /// never parsed. Draining stderr on its own task also keeps a full stderr pipe buffer from deadlocking
    /// the core.
    /// </summary>
    public IReadOnlyCollection<string> Diagnostics => _log.Snapshot();

    /// <summary>
    /// Spawn the core WITHOUT sending a command, so the client can gate on <c>ready</c> - check the protocol
    /// version and bitness (<see cref="HandshakeGate"/>) - before it commits to launching the target. The core
    /// emits <c>ready</c> before it reads its first command (docs/08 section 3, fixed in 97eae17), so awaiting
    /// <c>ready</c> here cannot deadlock.
    /// </summary>
    public static CoreClient Connect(string coreExePath)
    {
        ArgumentException.ThrowIfNullOrEmpty(coreExePath);

        var psi = new ProcessStartInfo
        {
            FileName = coreExePath,
            ArgumentList = { "__core" },
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardInputEncoding = Utf8NoBom,
            // No encoding for stdout: its bytes are read and decoded by ProtocolLineReader, which refuses
            // what is not UTF-8 rather than replacing it.
            StandardErrorEncoding = Utf8NoBom,
        };

        var process = Process.Start(psi)
            ?? throw new InvalidOperationException($"failed to start core '{coreExePath}'");
        return new CoreClient(process);
    }

    /// <summary>
    /// Spawn the core and send <c>start</c> immediately (start-first). A convenience over <see cref="Connect"/>
    /// for callers that do not gate on <c>ready</c> first - the conformance tests use it.
    /// </summary>
    public static CoreClient Launch(string coreExePath, StartCommand start)
    {
        ArgumentNullException.ThrowIfNull(start);
        var client = Connect(coreExePath);
        try
        {
            client.Send(start);
        }
        catch
        {
            // The core died right after spawning, so Send threw. Dispose the client we just created before
            // the exception propagates - the reference never escapes this method, so otherwise its process,
            // handles, and read tasks would leak (L-12).
            client.DisposeAsync().AsTask().GetAwaiter().GetResult();
            throw;
        }

        return client;
    }

    /// <summary>Send a command as one NDJSON line. The core reads one line per command.</summary>
    public void Send(Command command)
    {
        ArgumentNullException.ThrowIfNull(command);
        // One documented exception after dispose, thrown here rather than surfacing from deep inside a
        // closed stream - callers that guard on a running session (the GUI's in-flight controls) can race
        // Dispose, and a predictable type is what lets them catch it.
        ObjectDisposedException.ThrowIf(Volatile.Read(ref _disposed) != 0, this);
        SendLine(command);
    }

    /// <summary>
    /// Write one command without the disposed guard, for the ONE caller that must still be able to
    /// speak after the guard has closed: <see cref="DisposeAsync"/>, whose whole job starts by asking
    /// the core to end.
    /// <para>
    /// This split is not decoration. <c>_disposed</c> carries two jobs - the idempotency latch for
    /// dispose, and the guard on the public API - and routing shutdown through the public
    /// <see cref="Send"/> made the second job veto the first: the latch was already set, so the `end`
    /// command threw <see cref="ObjectDisposedException"/> and took the whole graceful path with it
    /// (R2-K1). Keep the two jobs apart.
    /// </para>
    /// </summary>
    private void SendLine(Command command)
    {
        var line = command.ToNdjson();
        lock (_stdinLock)
        {
            var writer = _process.StandardInput;
            // Explicit '\n' (not Environment.NewLine) - the core trims line endings on its side either way.
            writer.Write(line);
            writer.Write('\n');
            writer.Flush();
        }
    }

    /// <summary>
    /// Wait for the core process to exit and return its exit code - the session verdict (docs/08 section 8).
    /// </summary>
    public async Task<int> WaitForExitAsync(CancellationToken cancellationToken = default)
    {
        await _process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
        return _process.ExitCode;
    }

    private async Task ReadEventsAsync()
    {
        try
        {
            var lines = new ProtocolLineReader(_process.StandardOutput.BaseStream);
            while (true)
            {
                var read = await lines.ReadAsync().ConfigureAwait(false);
                if (read.Kind is ProtocolLineKind.Eof)
                {
                    break;
                }

                var (evt, noise) = Interpret(read);
                if (noise is not null)
                {
                    _log.AddNoise(noise);
                }

                if (evt is not null)
                {
                    await _events.Writer.WriteAsync(evt).ConfigureAwait(false);
                    if (evt is EndedEvent)
                    {
                        _endedRead.TrySetResult();
                    }
                }
            }
        }
        finally
        {
            _events.Writer.TryComplete();
        }
    }

    /// <summary>
    /// What one read of the core's stdout is: an event to hand on, a line that was not one (described for the
    /// diagnostics block), or neither - an empty line, or an event type this build does not know, which is
    /// ignored for forward compatibility (docs/08 section 2).
    /// <para>
    /// An event that names another protocol version is not handed on (R4-W1): whatever wrote it, this
    /// client cannot know what its fields mean. Except <c>ready</c>, which is the version check itself -
    /// <see cref="HandshakeGate"/> refuses a mismatched one with <c>handshake.protocol_mismatch</c>, and
    /// dropping it here would turn that into "no handshake" after the whole wait, a diagnosis pointing away
    /// from the answer.
    /// </para>
    /// <para>
    /// Pure, so every kind of line is tested without a core process.
    /// </para>
    /// </summary>
    internal static (ChronoEvent? Event, string? Noise) Interpret(ProtocolLine read)
    {
        switch (read.Kind)
        {
            case ProtocolLineKind.NotText:
                return (null, "core stdout: a line that is not UTF-8, skipped");
            case ProtocolLineKind.TooLong:
                return (null, $"core stdout: a line of {ProtocolJson.MaxProtocolLine} bytes or more, skipped");
            case ProtocolLineKind.Eof:
                return (null, null);
        }

        if (read.Text.Length == 0)
        {
            return (null, null);
        }

        ChronoEvent? evt;
        try
        {
            evt = EventParser.Parse(read.Text);
        }
        catch (JsonException ex)
        {
            return (null, $"core stdout: not an event ({ex.Message}), skipped: {Sample(read.Text)}");
        }

        if (evt is null or ReadyEvent || evt.V == ProtocolJson.ProtocolVersion)
        {
            return (evt, null);
        }

        return (null, $"core stdout: an event of protocol version {evt.V}, where this client speaks "
            + $"{ProtocolJson.ProtocolVersion}, skipped: {Sample(read.Text)}");
    }

    /// <summary>The start of a line for the diagnostics block, cut at <see cref="NoiseSampleChars"/> and
    /// never through the middle of a surrogate pair.</summary>
    private static string Sample(string line)
    {
        if (line.Length <= NoiseSampleChars)
        {
            return line;
        }

        var cut = char.IsHighSurrogate(line[NoiseSampleChars - 1]) ? NoiseSampleChars - 1 : NoiseSampleChars;
        return string.Concat(line.AsSpan(0, cut), "...");
    }

    private async Task DrainStderrAsync()
    {
        var stderr = _process.StandardError;
        string? line;
        while ((line = await stderr.ReadLineAsync().ConfigureAwait(false)) is not null)
        {
            _log.Add($"core stderr: {line}");
        }
    }

    /// <summary>
    /// End the session and the core. Every caller awaits the SAME shutdown: a second call used to return at
    /// once while the first was still joining the readers, so the GUI's Stop - which starts one dispose in
    /// the background and awaits another on its way out - read the core's exit code and its stderr before
    /// either was there. The session the core did not close then had no exit code in its diagnostics, the
    /// one case that line exists for (R4-S27).
    /// </summary>
    public ValueTask DisposeAsync()
    {
        Task disposal;
        lock (_disposalLock)
        {
            if (_disposal is null)
            {
                Volatile.Write(ref _disposed, 1);
                // Started, not run, under the lock: the shutdown writes to the core's stdin before its first
                // await, and a second caller must not wait on that write just to get the task.
                _disposal = Task.Run(ShutDownAsync);
            }

            disposal = _disposal;
        }

        return new ValueTask(disposal);
    }

    private async Task ShutDownAsync()
    {
        try
        {
            if (!_process.HasExited)
            {
                // Graceful: end the session, then close stdin so the core sees EOF and shuts down.
                // SendLine, not Send: the disposed latch is already set above, and the public guard would
                // reject our own shutdown command (R2-K1).
                try
                {
                    SendLine(new EndCommand { Id = 0 });
                }
                catch (Exception ex) when (ex is IOException or ObjectDisposedException)
                {
                    // Core already gone, or its stdin stream is closed - nothing to end.
                }

                try
                {
                    _process.StandardInput.Close();
                }
                catch (Exception ex) when (ex is IOException or ObjectDisposedException)
                {
                    // Stream already closed.
                }

                // Give the core a moment to end cleanly, then take down just the core (not the target
                // tree). The hook self-detaches when the core dies (slice 10), so the target reverts
                // to real time on its own - we never kill the application under test.
                using var grace = new CancellationTokenSource(TimeSpan.FromSeconds(2));
                try
                {
                    await _process.WaitForExitAsync(grace.Token).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    // Did not end within the grace period: take down just the core, never the target tree.
                    // A kill that is refused is said in the diagnostics rather than thrown: every caller
                    // awaits this one shutdown now, and the one that captures the diagnostics must reach it.
                    try
                    {
                        _process.Kill(entireProcessTree: false);
                    }
                    catch (System.ComponentModel.Win32Exception ex)
                    {
                        _log.Add($"could not stop the core: {ex.Message}");
                    }
                }
            }
        }
        catch (InvalidOperationException)
        {
            // Process exited between the HasExited check and here - fine.
        }

        // The core is gone. Complete the event stream NOW instead of waiting for the read loop to see EOF
        // on stdout. That EOF did not arrive when the core died: the target held the write end of the
        // core's stdout pipe, so it landed only once the APPLICATION UNDER TEST exited - which after a Stop
        // is whenever the tester happens to close it. Measured on this path: the core exited 21 ms after
        // `end`, and the read loop stayed parked until the target was killed, to the millisecond. The
        // target no longer gets that pipe (R4-W1, ADR-17), and the rule stays: EOF says when whoever holds
        // the pipe lets go, which is no statement about the core.
        //
        // Consumers gate "the session is over" on this stream (the GUI keeps showing "stopping" until it
        // ends, then falls back to its 15 s idle watchdog), so the target's lifetime must not be what
        // decides when a stopped session looks stopped. Already-written events stay readable - completing
        // a channel closes it to WRITERS, not to a reader draining what is left.
        //
        // But only once the read loop has taken what the core wrote before it exited. Closing at once
        // lost the tail, and the tail is the part that matters: the core writes the session verdict and
        // `ended` last, and an event the read loop takes out of the pipe after the close is dropped.
        // Measured over this Stop path (2026-09-24, eight runs): one lost the verdict and everything after
        // it, another lost `ended`. `ended` is the last line of a clean end, so it is the exact signal, and
        // a core that never wrote it costs the bounded wait instead.
        await Task.WhenAny(_endedRead.Task, Task.Delay(DrainTimeout)).ConfigureAwait(false);
        _events.Writer.TryComplete();

        // Bounded join for the same reason: the read loop can be parked on a read for as long as anything
        // holds that pipe, and dispose must not inherit that lifetime. Disposing the process below closes
        // the stream underneath it either way.
        await AwaitQuietly(_readLoop, JoinTimeout).ConfigureAwait(false);
        await AwaitQuietly(_stderrDrain, JoinTimeout).ConfigureAwait(false);
        try
        {
            if (_process.HasExited)
            {
                ExitCode = _process.ExitCode;
            }
        }
        catch (InvalidOperationException)
        {
            // No process was ever associated, or it is gone from under us - there is no code to keep.
        }

        _process.Dispose();
    }

    /// <summary>The core's exit code, once dispose has seen the process exit, or null. It tells a crash from a
    /// kill in a diagnostics block, which the stream alone cannot: a core that died sent nothing about it
    /// (R4-S27).</summary>
    public int? ExitCode { get; private set; }

    /// <summary>How many diagnostic lines were dropped to stay under the cap, so a caller can say the
    /// block is a tail rather than the whole of it. Zero means nothing was lost.</summary>
    public int DiagnosticsDropped => _log.Dropped;

    /// <summary>How long dispose waits to join a background reader before giving up on it. The read loop
    /// can be parked on a pipe something else still holds, so this is a bound on OUR shutdown, not on the
    /// reader - the process dispose that follows closes the stream underneath it.</summary>
    private static readonly TimeSpan JoinTimeout = TimeSpan.FromSeconds(2);

    /// <summary>How long dispose lets the read loop hand on what the core wrote before it exited, when the
    /// core's <c>ended</c> has not been read yet. Only a core that was killed, or died, before writing
    /// <c>ended</c> waits this long - a core that ended cleanly releases the wait as soon as its last line
    /// is read, which is a matter of milliseconds because it is already in the pipe.</summary>
    private static readonly TimeSpan DrainTimeout = TimeSpan.FromMilliseconds(500);

    private async Task AwaitQuietly(Task task, TimeSpan? timeout = null)
    {
        try
        {
            if (timeout is { } limit)
            {
                var finished = await Task.WhenAny(task, Task.Delay(limit)).ConfigureAwait(false);
                if (!ReferenceEquals(finished, task))
                {
                    _log.Add($"background task did not finish within {limit.TotalSeconds:0.#}s");
                    return;
                }
            }

            await task.ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _log.Add($"background task: {ex.Message}");
        }
    }
}
