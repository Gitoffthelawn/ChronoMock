using System.Diagnostics;
using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.Protocol.Tests;

/// <summary>
/// The one way the window runs the engine for a single question (R4/18): the calculator, the preset
/// catalogue and the licence query all come through here. Exercised on the system's command interpreter,
/// which is on every machine and can be told to exit with a code, write to either stream, or never finish.
/// </summary>
public class OneShotEngineTests
{
    private static readonly string Cmd = Path.Combine(Environment.SystemDirectory, "cmd.exe");

    private static Task<EngineRun> Run(string script, TimeSpan limit, CancellationToken ct)
        => OneShotEngine.RunAsync(Cmd, ["/d", "/c", script], null, limit, ct);

    [Fact(Timeout = 15_000)]
    public async Task Both_streams_and_the_exit_code_come_back()
    {
        var run = await Run("echo out& echo err 1>&2& exit /b 3", TimeSpan.FromSeconds(10), TestContext.Current.CancellationToken);

        Assert.Equal(3, run.ExitCode);
        Assert.Equal("out", run.Stdout.Trim());
        Assert.Equal("err", run.Stderr.Trim());
    }

    /// <summary>R4-N50: stderr redirected and never read held a child on its write once it passed a pipe
    /// buffer, until the limit killed it. Far more than a buffer goes there here, and the run still ends on
    /// its own.</summary>
    [Fact(Timeout = 30_000)]
    public async Task A_child_writing_far_more_than_a_pipe_buffer_to_either_stream_finishes()
    {
        const string Line = "0123456789012345678901234567890123456789";
        var run = await Run(
            $"for /l %i in (1,1,3000) do @echo {Line}& @echo {Line} 1>&2",
            TimeSpan.FromSeconds(20),
            TestContext.Current.CancellationToken);

        Assert.Equal(0, run.ExitCode);
        Assert.True(run.Stdout.Length > 100_000, $"stdout {run.Stdout.Length}");
        Assert.True(run.Stderr.Length > 100_000, $"stderr {run.Stderr.Length}");
    }

    [Fact(Timeout = 15_000)]
    public async Task A_child_past_its_limit_is_stopped_and_said_to_be()
    {
        var clock = Stopwatch.StartNew();

        var failure = await Assert.ThrowsAsync<EngineTimeoutException>(
            () => Run("for /l %i in (1,1,2000000000) do @rem", TimeSpan.FromMilliseconds(300), TestContext.Current.CancellationToken));

        Assert.True(failure.Stopped, "the child was killed");
        Assert.True(clock.Elapsed < TimeSpan.FromSeconds(8), $"took {clock.Elapsed.TotalMilliseconds:0} ms");
    }

    [Fact(Timeout = 15_000)]
    public async Task A_caller_that_lets_go_gets_a_cancellation_not_a_timeout()
    {
        using var caller = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);
        caller.CancelAfter(TimeSpan.FromMilliseconds(300));

        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => Run("for /l %i in (1,1,2000000000) do @rem", TimeSpan.FromSeconds(10), caller.Token));
    }

    /// <summary>
    /// R5-WY1: the engine is started on a pool thread, never on the caller's - the window's, which creating a
    /// process held for as long as the loader and an antivirus took, on every edit in the calculator. The start
    /// here holds until the test lets it go, so a start made on the caller's thread keeps the call from coming
    /// back - for five seconds, so that is a red assertion rather than a hang.
    /// </summary>
    [Fact(Timeout = 15_000)]
    public async Task The_engine_is_started_off_the_callers_thread()
    {
        using var release = new ManualResetEventSlim();
        var startedBeforeTheCallCameBack = 0;

        var run = OneShotEngine.RunAsync(
            Cmd,
            ["/d", "/c", "exit /b 0"],
            null,
            TimeSpan.FromSeconds(10),
            process =>
            {
                release.Wait(TimeSpan.FromSeconds(5));
                Interlocked.Exchange(ref startedBeforeTheCallCameBack, 1);
                return process.Start();
            },
            TestContext.Current.CancellationToken);
        var startedFirst = Interlocked.CompareExchange(ref startedBeforeTheCallCameBack, 0, 0);
        release.Set();

        Assert.Equal(0, (await run).ExitCode); // and once let go, it runs as before
        Assert.Equal(0, startedFirst);
    }

    [Fact]
    public async Task A_question_cancelled_before_it_is_asked_starts_no_engine()
    {
        // The start no longer happens before the first await, so a question superseded before the pool got to
        // it is never started - it used to be started and killed at once.
        var starts = 0;

        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => OneShotEngine.RunAsync(
            Cmd,
            ["/d", "/c", "exit /b 0"],
            null,
            TimeSpan.FromSeconds(10),
            process =>
            {
                Interlocked.Increment(ref starts);
                return process.Start();
            },
            new CancellationToken(canceled: true)));

        Assert.Equal(0, starts);
    }

    [Fact]
    public async Task An_engine_that_cannot_start_is_named_with_the_systems_reason()
    {
        var missing = Path.Combine(Path.GetTempPath(), $"chrono-no-such-engine-{Environment.ProcessId}.exe");

        var failure = await Assert.ThrowsAsync<EngineLaunchException>(
            () => OneShotEngine.RunAsync(missing, [], null, TimeSpan.FromSeconds(5), TestContext.Current.CancellationToken));

        Assert.Contains(missing, failure.Message, StringComparison.Ordinal);
    }
}
