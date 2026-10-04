using System.Diagnostics;

namespace ChronoMock.Protocol.Tests;

/// <summary>
/// The way out of a child process that did not finish on its own terms (review of #87). A child the kill did
/// not reach keeps its pipes open, and the cleanup used to wait on their readers without a bound - holding
/// the caller past the very limit that started the cleanup.
/// </summary>
public class ChildProcessesTests
{
    [Fact(Timeout = 15_000)]
    public async Task The_cleanup_does_not_wait_past_its_bound_for_a_reader_that_never_ends()
    {
        var neverEnds = new TaskCompletionSource<string>().Task;
        var clock = Stopwatch.StartNew();

        // The test's own timeout ends an unbounded wait as a failure rather than a hang.
        await ChildProcesses.ObserveQuietly(neverEnds).WaitAsync(TestContext.Current.CancellationToken);

        Assert.True(
            clock.Elapsed < ChildProcesses.CleanupBound + TimeSpan.FromSeconds(3),
            $"the cleanup took {clock.Elapsed.TotalMilliseconds:0} ms");
    }

    [Fact]
    public void A_child_that_was_never_started_counts_as_gone()
    {
        using var never = new Process();

        Assert.True(ChildProcesses.KillQuietly(never));
    }
}
