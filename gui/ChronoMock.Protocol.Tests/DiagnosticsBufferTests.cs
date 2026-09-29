using System.Collections.Concurrent;
using ChronoMock.Protocol;

namespace ChronoMock.Protocol.Tests;

/// <summary>
/// The diagnostics buffer is capped, so a chatty target cannot grow it without bound - and a capped
/// buffer has to be able to say it is a TAIL.
///
/// 🔴 It used to say so with a marker LINE, enqueued once when the first drop happened. Enqueue appends
/// and the trimming dequeues from the front, so after another cap's worth of lines the marker reached
/// the front and was dropped itself - and the flag guarding it was already set, so it never came back.
/// The block then read as complete again, which is exactly what the marker existed to prevent. A count
/// cannot fall out of the queue, and it says HOW MANY lines went, which the marker never could.
/// </summary>
public sealed class DiagnosticsBufferTests
{
    private static ConcurrentQueue<string> QueueOf(int count)
    {
        var q = new ConcurrentQueue<string>();
        for (var i = 0; i < count; i++)
        {
            q.Enqueue($"line {i}");
        }

        return q;
    }

    [Fact]
    public void Nothing_is_dropped_while_the_buffer_is_within_its_cap()
    {
        var q = QueueOf(5);

        Assert.Equal(0, CoreDiagnostics.TrimToCap(q, 10));
        Assert.Equal(5, q.Count);
    }

    [Fact]
    public void The_oldest_lines_go_first_and_the_count_says_how_many()
    {
        var q = QueueOf(12);

        Assert.Equal(2, CoreDiagnostics.TrimToCap(q, 10));
        Assert.Equal(10, q.Count);
        Assert.True(q.TryPeek(out var oldest));
        Assert.Equal("line 2", oldest); // 0 and 1 went, so what is left really is the tail
    }

    /// <summary>
    /// The regression itself: keep pushing past the cap and the record of what was lost has to survive.
    /// A marker line does not - it ages out like any other entry. A running total does.
    /// </summary>
    [Fact]
    public void The_record_of_what_was_dropped_survives_far_past_the_cap()
    {
        var q = new ConcurrentQueue<string>();
        var dropped = 0;
        for (var i = 0; i < 500; i++)
        {
            q.Enqueue($"line {i}");
            dropped += CoreDiagnostics.TrimToCap(q, 10);
        }

        Assert.Equal(10, q.Count);
        Assert.Equal(490, dropped);
    }

    /// <summary>
    /// R4-N52. Lines on the core's stdout that were not events used to go into the same capped queue as the
    /// core's stderr, so a target writing into the protocol pushed out exactly the lines that said what the
    /// session did. Each has its own queue now: a full stderr buffer survives any number of the other kind.
    /// </summary>
    [Fact]
    public void Lines_that_were_not_events_do_not_push_the_stderr_lines_out()
    {
        var log = new CoreDiagnostics();
        for (var i = 0; i < CoreDiagnostics.MaxLines; i++)
        {
            log.Add($"core stderr: {i}");
        }

        for (var i = 0; i < 10_000; i++)
        {
            log.AddNoise($"core stdout: noise {i}");
        }

        var lines = log.Snapshot();
        Assert.Equal(0, log.Dropped);
        Assert.Equal(CoreDiagnostics.MaxLines, lines.Count(l => l.StartsWith("core stderr: ", StringComparison.Ordinal)));
        Assert.Contains("core stderr: 0", lines);
    }

    /// <summary>The other queue keeps its FIRST lines, the ones closest to whatever started writing there, and
    /// says how many later ones it did not keep - listed ahead of the stderr lines, so the block's own closing
    /// note about dropped stderr lines still follows the lines it is about.</summary>
    [Fact]
    public void The_first_lines_that_were_not_events_stay_and_the_rest_are_counted()
    {
        var log = new CoreDiagnostics();
        log.Add("core stderr: first");
        for (var i = 0; i < CoreDiagnostics.MaxNoiseLines + 7; i++)
        {
            log.AddNoise($"noise {i}");
        }

        var lines = log.Snapshot().ToList();
        Assert.Equal("noise 0", lines[0]);
        Assert.Equal($"noise {CoreDiagnostics.MaxNoiseLines - 1}", lines[CoreDiagnostics.MaxNoiseLines - 1]);
        Assert.Equal("core stdout: 7 later line(s) that were not events are not kept", lines[CoreDiagnostics.MaxNoiseLines]);
        Assert.Equal("core stderr: first", lines[^1]);
        Assert.Equal(CoreDiagnostics.MaxNoiseLines + 2, lines.Count);
    }

    /// <summary>Nothing about the other queue appears while it is empty - no count line saying zero.</summary>
    [Fact]
    public void Without_lines_that_were_not_events_the_block_is_the_stderr_alone()
    {
        var log = new CoreDiagnostics();
        log.Add("core stderr: only");

        Assert.Equal(new[] { "core stderr: only" }, log.Snapshot());
    }
}
