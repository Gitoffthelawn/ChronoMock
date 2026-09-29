using System.Collections.Concurrent;

namespace ChronoMock.Protocol;

/// <summary>
/// What a core session leaves for the diagnostics block: the core's stderr (and the client's own notes
/// about its readers), and the lines on the core's stdout that were not events. Each sits in a queue of
/// its own with its own cap, so a stream of the second kind cannot push the first out of the block
/// (R4-N52). They shared one queue, and every line of a target writing into the protocol took the place of
/// a line the core wrote about the session.
/// <para>
/// Nothing but the core writes on its stdout now (R4-W1), so a line there that is not an event is a fault
/// somewhere, and the first few are what a reader needs to find it. That queue keeps its first lines and
/// counts the rest, and its cap is small for that reason.
/// </para>
/// </summary>
internal sealed class CoreDiagnostics
{
    /// <summary>How many diagnostic lines are kept. The core's stderr is unbounded in principle (a long
    /// session), and the whole queue is later joined into one string for the diagnostics block, so it is
    /// capped and the OLDEST lines go - a failure is explained by what happened last.</summary>
    internal const int MaxLines = 2000;

    /// <summary>How many lines from the core's stdout that were not events are kept.</summary>
    internal const int MaxNoiseLines = 20;

    private readonly ConcurrentQueue<string> _lines = new();
    private int _linesDropped;
    private readonly ConcurrentQueue<string> _noise = new();
    private int _noiseDropped;

    /// <summary>Append one diagnostic line, dropping the oldest past the cap and counting what was dropped.
    /// <para>
    /// 🔴 The marker used to be a LINE, enqueued once when the first drop happened. Enqueue appends and
    /// the trimming dequeues from the front, so after another <see cref="MaxLines"/> lines the marker
    /// reached the front and was dropped itself - and the flag that guarded it was already set, so it
    /// never came back. The block then read as complete again, which is exactly what it was written to
    /// prevent. A counter cannot fall out of the queue, and it can say HOW MANY lines went, which the
    /// marker never could.
    /// </para>
    /// </summary>
    internal void Add(string line)
    {
        _lines.Enqueue(line);
        // Atomic, because two threads add here: the reader of the core's stderr, and dispose, which notes a
        // reader that did not finish in time - that is, while that reader still runs. A plain += between them
        // could lose a count, and the block would then claim to be less of a tail than it is (R4-N52).
        Interlocked.Add(ref _linesDropped, TrimToCap(_lines, MaxLines));
    }

    /// <summary>Keep one line about the core's stdout that was not an event, while there is room, and count it
    /// when there is not. The FIRST lines stay, unlike the diagnostic lines: they are the ones closest to
    /// whatever started writing there. Only the read loop writes here, so the room check is not a race.</summary>
    internal void AddNoise(string line)
    {
        if (_noise.Count < MaxNoiseLines)
        {
            _noise.Enqueue(line);
        }
        else
        {
            Interlocked.Increment(ref _noiseDropped);
        }
    }

    /// <summary>How many diagnostic lines were dropped to stay under <see cref="MaxLines"/>. Lines that were
    /// not events are not counted here - their own count is in <see cref="Snapshot"/>.</summary>
    internal int Dropped => Volatile.Read(ref _linesDropped);

    /// <summary>
    /// Everything kept, in the order the block lists it: the lines that were not events first, with a line
    /// saying how many later ones were not kept, then the diagnostic lines. The diagnostic lines come last
    /// because the block closes with its own note about how many of THOSE went, and that note has to follow
    /// them.
    /// </summary>
    internal IReadOnlyCollection<string> Snapshot()
    {
        var lines = new List<string>(_noise);
        var noiseDropped = Volatile.Read(ref _noiseDropped);
        if (noiseDropped > 0)
        {
            lines.Add($"core stdout: {noiseDropped} later line(s) that were not events are not kept");
        }

        lines.AddRange(_lines);
        return lines;
    }

    /// <summary>Drop the oldest entries until the queue is within <paramref name="cap"/>, and say how many
    /// went. Pure over the queue, so the rule this replaced - a marker line that fell out of its own queue
    /// - is testable without a core process.</summary>
    internal static int TrimToCap(ConcurrentQueue<string> queue, int cap)
    {
        var dropped = 0;
        while (queue.Count > cap && queue.TryDequeue(out _))
        {
            dropped++;
        }

        return dropped;
    }
}
