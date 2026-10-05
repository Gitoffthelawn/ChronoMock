using System.Diagnostics;
using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App.Tests;

/// <summary>
/// The window closing while a session is in hand (R4-S24, R4/13). The close used to block the UI thread for
/// three seconds on a dispose whose end needed that thread: measured, every close during a session took
/// 3.05 s and recorded nothing. The window now asks the session to finish and waits for it without blocking -
/// these tests hold the view model's half of that, the live measurement holds the window's
/// (<c>tools/probes/r4-13/close-during-session.ps1</c>).
/// </summary>
public class SessionCloseTests
{
    private static readonly TimeSpan Limit = TimeSpan.FromSeconds(10);

    [Fact]
    public async Task A_close_with_no_session_is_not_held_up_at_all()
    {
        var vm = new SessionViewModel();

        var finishing = vm.FinishForCloseAsync(Limit);

        Assert.True(finishing.IsCompleted, "a window with nothing running must close at once");
        Assert.True(await finishing);
    }

    [Fact]
    public async Task A_start_reached_after_the_window_began_to_close_starts_nothing()
    {
        // A Start waiting for its date when the window closes resumes afterwards, and must not go on.
        var vm = new SessionViewModel();
        vm.SetTarget(Path.Combine(Path.GetTempPath(), $"chrono-missing-{Guid.NewGuid():N}.exe"));

        await vm.FinishForCloseAsync(Limit);
        await vm.StartAsync();

        Assert.Equal("status.idle", vm.StatusKey); // never got as far as reading the target
        Assert.True(vm.IsIdle);
    }

    /// <summary>
    /// 🔴 The one a close must never get wrong: closed while the plan is built or the core shakes hands, the
    /// session would otherwise go on to LAUNCH the application after the window had gone. The start below
    /// returns while the plan is still being built off this thread, and the close lands then.
    /// </summary>
    [Fact]
    public async Task A_close_while_the_start_is_still_preparing_launches_nothing()
    {
        var target = SessionPlan.DefaultTargetPath();
        Assert.True(target is not null, "the sample target is not built - build the solution first");
        var store = new InMemorySessionHistoryStore();
        var vm = new SessionViewModel(store);
        vm.SetTarget(target);

        var start = vm.StartAsync();
        var finished = await vm.FinishForCloseAsync(Limit);
        await start;

        Assert.True(finished);
        Assert.True(vm.IsIdle);
        Assert.Empty(store.Load()); // nothing ran, so nothing is recorded
        Assert.NotEqual(SessionStatusKind.Running, vm.StatusKind);
    }

    /// <summary>
    /// The real thing, with a real core: closed during a running session, the session is stopped, recorded
    /// and let go before the close completes - the three things the blocking close lost.
    /// </summary>
    [Fact]
    [Trait("Category", "Integration")]
    public async Task A_close_during_a_running_session_waits_for_it_and_records_it()
    {
        var target = SessionPlan.DefaultTargetPath();
        Assert.True(target is not null, "the sample target is not built - build the solution first");
        var store = new InMemorySessionHistoryStore();

        var (finished, idle, recordedAtClose, ranFor) = await WpfTestHost.RunAsync(async () =>
        {
            var vm = new SessionViewModel(store);
            vm.SetTarget(target);
            var start = vm.StartAsync();
            var clock = Stopwatch.StartNew();
            while (!vm.IsRunning && clock.Elapsed < TimeSpan.FromSeconds(20))
            {
                await Task.Delay(50, TestContext.Current.CancellationToken);
            }

            Assert.True(vm.IsRunning, $"the session never ran ({vm.StatusKey}: {vm.LastError})");
            clock.Restart();
            var done = await vm.FinishForCloseAsync(Limit);
            // Read BEFORE the start is awaited - the window closes when the finish returns, not later.
            var result = (done, vm.IsIdle, store.Load().Count, clock.Elapsed);
            await start;
            return result;
        });

        Assert.True(finished, $"the session did not end within {Limit.TotalSeconds} s");
        Assert.True(idle, "the session must be over when the close goes ahead");
        Assert.Equal(1, recordedAtClose);
        // A Stop ends a session in well under a second. The sample target lives five, so a close that merely
        // waited for the application to end on its own would take that long - three leaves room for a loaded
        // machine and still tells the two apart.
        Assert.True(ranFor < TimeSpan.FromSeconds(3), $"the close took {ranFor.TotalMilliseconds:0} ms");
    }
}
