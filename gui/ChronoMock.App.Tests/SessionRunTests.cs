using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A session from Start to its record, over a core that answers in the order the test chooses (R5-D63). Until
/// the connection became an interface this path - plan, handshake, start, the event pump, the history record,
/// the diagnostics kept when it went wrong, letting the core go - ran only against a real core, in integration
/// tests the CI does not run.
/// </summary>
public class SessionRunTests
{
    private static readonly TimeSpan Limit = TimeSpan.FromSeconds(10);

    /// <summary>A real executable for the plan to read the header of - this test run's own, so its bitness is
    /// the machine's and the fake core's <c>ready</c> matches it.</summary>
    private static string Target => Environment.ProcessPath!;

    private static StateEvent Heartbeat => new()
    {
        V = ProtocolJson.ProtocolVersion,
        Fake = new Clock { Wall = "2038-01-19T03:14:08" },
        Real = new Clock { Wall = "2026-10-20T10:00:01" },
        Multiplier = 1,
        ElapsedRealMs = 1000,
        ElapsedFakeMs = 1000,
    };

    private static VerdictEvent Works => new()
    {
        V = ProtocolJson.ProtocolVersion,
        Verdict = "works",
        ReasonKey = "verdict.reason.works",
    };

    private static SessionViewModel ViewModel(ISessionHistoryStore store, FakeCoreConnection core)
    {
        var vm = new SessionViewModel(store, connect: core.Connector);
        vm.SetTarget(Target);
        return vm;
    }

    [Fact]
    public async Task A_session_runs_from_its_start_to_its_record_and_lets_the_core_go()
    {
        var store = new InMemorySessionHistoryStore();
        var core = new FakeCoreConnection(
            [FakeCoreConnection.Ready],
            command => command is StartCommand
                ? [Heartbeat, Works, new EndedEvent { V = ProtocolJson.ProtocolVersion, Clean = true }]
                : []);
        var vm = ViewModel(store, core);

        // The fake ends its stream once the session has its `ended`, as a core that exits does.
        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (vm.StatusKind != SessionStatusKind.Ended && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Equal(SessionStatusKind.Ended, vm.StatusKind);
        Assert.Equal(AppPaths.SubstitutionCores.CoreForTarget(Target), core.ConnectedTo);
        var start = Assert.IsType<StartCommand>(Assert.Single(core.Sent));
        Assert.Equal(Target, start.Target.Path);
        var record = Assert.Single(store.Load());
        Assert.Equal(("works", Target), (record.Verdict, record.TargetPath));
        Assert.True(core.LetGo);
        Assert.True(vm.IsIdle);
    }

    [Fact]
    public async Task A_core_that_refuses_the_handshake_launches_nothing_and_its_words_are_kept()
    {
        var store = new InMemorySessionHistoryStore();
        var core = new FakeCoreConnection(
            [new ErrorEvent { V = ProtocolJson.ProtocolVersion, Key = "hook.dll_missing", Origin = "core" }],
            stderr: "chrono: the hook library is not next to the core");
        var vm = ViewModel(store, core);

        await vm.StartAsync().WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Equal((SessionStatusKind.Error, "hook.dll_missing"), (vm.StatusKind, vm.StatusKey));
        Assert.Empty(core.Sent); // nothing was launched
        Assert.Empty(store.Load()); // nothing ran, so nothing is recorded
        Assert.True(core.LetGo);
        Assert.Contains("chrono: the hook library is not next to the core", vm.DiagnosticsText, StringComparison.Ordinal);
    }

    /// <summary>
    /// The close of R4-S24 without a real core: a window closed during a running session stops it, records it
    /// and lets the core go before the close completes. The core here never sends <c>ended</c>, so the record
    /// says what a session cut short is (R4-W3).
    /// </summary>
    [Fact]
    public async Task A_close_during_a_running_session_stops_it_records_it_and_lets_the_core_go()
    {
        var store = new InMemorySessionHistoryStore();
        var core = new FakeCoreConnection([FakeCoreConnection.Ready], command => command is StartCommand ? [Heartbeat, Works] : []);
        var vm = ViewModel(store, core);

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (!vm.IsRunning && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        Assert.True(vm.IsRunning, $"the session never ran ({vm.StatusKey}: {vm.LastError})");
        var finished = await vm.FinishForCloseAsync(Limit);
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.True(finished);
        Assert.Equal(SessionStatusKind.Stopped, vm.StatusKind);
        Assert.Equal("undetermined", Assert.Single(store.Load()).Verdict);
        Assert.True(core.LetGo);
    }
}
