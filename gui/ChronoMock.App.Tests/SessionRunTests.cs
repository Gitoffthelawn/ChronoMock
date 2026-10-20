using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
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

    /// <summary>
    /// A real executable for the plan to read the header of: the sample target this repository builds for its
    /// own sessions, which lives a few seconds and exits.
    /// </summary>
    /// <remarks>
    /// 🔴 NOT this test run's own exe, which it was at first. A session that missed the fake core - the
    /// mutation entry for R5-D63 makes one - launched the real core with the test runner as its target, and
    /// the runner ran the whole suite again as the application under test, holding its own binaries locked.
    /// </remarks>
    private static string Target => SessionPlan.DefaultTargetPath()
        ?? throw new InvalidOperationException("the sample target is not built - build the solution first");

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
            [FakeCoreConnection.ReadyFor(Target)],
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

    /// <summary>
    /// R5-P10, R5-D51: the new form comes back at the mode the last session STARTED with. The live speed moves
    /// the Mode list while the session runs, and the form used to open at the speed it was left at - a run
    /// started in flow came back at x60. Reversal probe: drop the restore and the last assertion fails on x60.
    /// </summary>
    [Fact]
    public async Task A_new_session_comes_back_at_the_mode_the_last_one_started_with()
    {
        var atSixty = new StateEvent
        {
            V = ProtocolJson.ProtocolVersion,
            Fake = new Clock { Wall = "2038-01-19T03:14:08" },
            Real = new Clock { Wall = "2026-10-20T10:00:01" },
            Multiplier = 60,
            ElapsedRealMs = 1000,
            ElapsedFakeMs = 60000,
        };
        var core = new FakeCoreConnection(
            [FakeCoreConnection.ReadyFor(Target)],
            command => command is StartCommand
                ? [atSixty, Works, new EndedEvent { V = ProtocolJson.ProtocolVersion, Clean = true }]
                : []);
        var vm = ViewModel(new InMemorySessionHistoryStore(), core);
        var flow = vm.Modes.Single(m => m.Mode == "flow");
        vm.SelectedMode = flow;

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (vm.StatusKind != SessionStatusKind.Ended && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);
        Assert.Equal(60, vm.SelectedMode.Multiplier); // the live speed moved the list - not vacuous

        vm.BeginNewSession();

        Assert.Same(flow, vm.SelectedMode);
        Assert.DoesNotContain(core.Sent, c => c is SetMultiplierCommand); // the restore sends nothing
    }

    /// <summary>
    /// R5-P11: the jump target typed in one session is not waiting in the next. It is read in the zone of the
    /// session it is sent to, so a leftover would jump the new session somewhere nobody asked for - and it is
    /// cleared to the untouched state, not to an empty date, which would open the new session's field on
    /// "the date is empty". Reversal probe: drop the clear and the field still holds the old moment.
    /// </summary>
    [Fact]
    public async Task A_new_session_starts_with_an_untouched_jump_field()
    {
        var core = new FakeCoreConnection(
            [FakeCoreConnection.ReadyFor(Target)],
            command => command is StartCommand ? [Heartbeat, Works] : []);
        var vm = ViewModel(new InMemorySessionHistoryStore(), core);

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (!vm.IsRunning && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        vm.JumpMoment.DateText = "2040-06-15";
        vm.JumpMoment.TimeText = "13:30";
        Assert.True(vm.JumpMoment.IsValid); // typed during the session - not vacuous
        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        vm.BeginNewSession();

        Assert.Equal((string.Empty, string.Empty), (vm.JumpMoment.DateText, vm.JumpMoment.TimeText));
        Assert.Equal(string.Empty, vm.JumpMoment.ActiveErrorKey);
        Assert.False(vm.JumpMoment.HasDateError);
    }

    /// <summary>
    /// R5-D73. A session whose record moved an unreadable history aside says where it went, and the next
    /// session does not say it again. Reversal probe: drop the read of the store and the note stays empty.
    /// </summary>
    [Fact]
    public async Task A_session_whose_record_set_the_old_history_aside_says_where_it_went()
    {
        const string movedTo = @"C:\Users\tester\AppData\Local\ChronoMock\history\sessions.json.unreadable-20261020-180000";
        var core = new FakeCoreConnection(
            [FakeCoreConnection.ReadyFor(Target)],
            command => command is StartCommand
                ? [Heartbeat, Works, new EndedEvent { V = ProtocolJson.ProtocolVersion, Clean = true }]
                : []);
        var vm = ViewModel(new SetAsideHistoryStore(movedTo), core);

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (vm.StatusKind != SessionStatusKind.Ended && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Equal((movedTo, true), (vm.HistorySetAside, vm.HasHistorySetAside));

        vm.BeginNewSession();

        Assert.Equal((string.Empty, false), (vm.HistorySetAside, vm.HasHistorySetAside));
    }

    /// <summary>
    /// R5-D73, review round on #133. A clear that found the history unreadable moves it aside as an append
    /// does, and says where at once - with the section kept on screen for it although the clear left no rows.
    /// Read only by the next record, the move was silent and then claimed by a record that moved nothing.
    /// Reversal probe: drop the read after the clear and the note stays empty.
    /// </summary>
    [Fact]
    public async Task A_clear_that_set_the_old_history_aside_says_where_it_went()
    {
        const string movedTo = @"C:\Users\tester\AppData\Local\ChronoMock\history\sessions.json.unreadable-20261020-190000";
        var vm = new SessionViewModel(new SetAsideHistoryStore(movedTo));

        await vm.ClearHistoryAsync();

        Assert.Equal((movedTo, false, true), (vm.HistorySetAside, vm.HasHistory, vm.ShowsHistorySection));
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
        // The core ran and never said ready, and the block says which core that was (R5/6b, D2).
        Assert.Contains(
            "  core:      no handshake, " + AppPaths.SubstitutionCores.CoreForTarget(Target) + "\n",
            vm.DiagnosticsText,
            StringComparison.Ordinal);
    }

    /// <summary>
    /// R5/6b, D2. A core that could not even be started leaves a diagnostics block: which core was asked for,
    /// that it did not start, and the exception whole, with its stack. The session loop closed nothing in that
    /// case and composed no block at all, so the one failure a user asks for help with left nothing to send.
    /// Reversal probes: return early again for a missing session and the block is empty, keep only the message
    /// and the type and the stack are gone.
    /// </summary>
    [Fact]
    public async Task A_core_that_does_not_start_leaves_a_block_with_its_path_and_the_whole_exception()
    {
        var corePath = AppPaths.SubstitutionCores.CoreForTarget(Target);
        var vm = new SessionViewModel(
            new InMemorySessionHistoryStore(),
            connect: path => throw new FileNotFoundException("the core is not where the layout says", path));
        vm.SetTarget(Target);

        await vm.StartAsync().WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Equal(SessionStatusKind.Error, vm.StatusKind);
        var block = vm.DiagnosticsText;
        Assert.Contains("  core:      did not start, " + corePath + "\n", block, StringComparison.Ordinal);
        Assert.Contains(
            "  error:     System.IO.FileNotFoundException: the core is not where the layout says",
            block,
            StringComparison.Ordinal);
        Assert.Contains("\n                at ", block, StringComparison.Ordinal);
        Assert.Contains("  mode:      native\n", block, StringComparison.Ordinal);
    }

    /// <summary>
    /// R5-WY1: the core's process is created on a pool thread, never on the caller's - the window's, which
    /// creating a process holds for as long as the loader and an antivirus take. The connection here holds
    /// until the test lets it go, so a connection made on the caller's thread keeps the call from coming back
    /// (for five seconds, so that is a red assertion rather than a hang).
    /// </summary>
    [Fact]
    public async Task The_core_is_connected_off_the_callers_thread()
    {
        using var release = new ManualResetEventSlim();
        var connectedBeforeTheCallCameBack = 0;
        var core = new FakeCoreConnection([FakeCoreConnection.ReadyFor(Target)]);
        var plan = SessionPlan.Build(Target, new TimeSpec
        {
            Moment = new MomentSpec { Kind = "absolute", Local = "2038-01-19T03:14:07", TzBiasMin = 0 },
            Mode = "flow",
        });

        var open = CoreSession.OpenAsync(plan, Limit, path =>
        {
            release.Wait(TimeSpan.FromSeconds(5));
            Interlocked.Exchange(ref connectedBeforeTheCallCameBack, 1);
            return core.Connector(path);
        });
        var connectedFirst = Interlocked.CompareExchange(ref connectedBeforeTheCallCameBack, 0, 0);
        release.Set();
        var opened = await open.WaitAsync(Limit, TestContext.Current.CancellationToken);
        await opened.Session.DisposeAsync();

        Assert.Equal(0, connectedFirst);
        Assert.Null(opened.RefusalKey); // and once let go, the handshake passes as before
        Assert.Equal(plan.CorePath, core.ConnectedTo);
    }

    /// <summary>R5/6b, D2. A core that shook hands is named in the block by what it said: its version and
    /// bitness, then its path.</summary>
    [Fact]
    public async Task A_core_that_shook_hands_is_named_by_its_version_and_bitness()
    {
        var ready = FakeCoreConnection.ReadyFor(Target);
        var core = new FakeCoreConnection(
            [ready],
            command => command is StartCommand
                ? [new VerdictEvent { V = ProtocolJson.ProtocolVersion, Verdict = "fails", RefuseStart = true, ReasonKey = "verdict.reason.fails" }]
                : []);
        var vm = ViewModel(new InMemorySessionHistoryStore(), core);

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (vm.StatusKind != SessionStatusKind.Refused && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Contains(
            $"  core:      {ready.CoreVersion}, {ready.Bitness}, {AppPaths.SubstitutionCores.CoreForTarget(Target)}\n",
            vm.DiagnosticsText,
            StringComparison.Ordinal);
        Assert.DoesNotContain("  error:", vm.DiagnosticsText, StringComparison.Ordinal);
    }

    /// <summary>
    /// A speed change pressed just as the core went away - a Stop disposing it, or the core's own exit - is let
    /// go quietly: the session's end is what the event stream says next, not a failure of the button
    /// (<see cref="CoreSession.IsGoneAway"/>). This is the race that once popped a message box after an
    /// ordinary Stop, reachable here because the fake core throws on a send the way the real one does.
    /// </summary>
    [Fact]
    public async Task A_speed_change_sent_to_a_core_that_is_already_gone_is_let_go_quietly()
    {
        var store = new InMemorySessionHistoryStore();
        var core = new FakeCoreConnection(
            [FakeCoreConnection.ReadyFor(Target)], command => command is StartCommand ? [Heartbeat, Works] : []);
        var vm = ViewModel(store, core);

        var run = vm.StartAsync();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        while (!vm.IsRunning && clock.Elapsed < Limit)
        {
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }

        Assert.True(vm.IsRunning, $"the session never ran ({vm.StatusKey}: {vm.LastError})");
        core.GoAway();
        var pressed = Record.Exception(() => vm.SendMultiplier(60));
        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);

        Assert.Null(pressed);
        Assert.IsType<StartCommand>(Assert.Single(core.Sent)); // the speed change never reached a core
        Assert.Equal(SessionStatusKind.CoreStopped, vm.StatusKind); // the stream, not the button, ended it
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
        var core = new FakeCoreConnection([FakeCoreConnection.ReadyFor(Target)], command => command is StartCommand ? [Heartbeat, Works] : []);
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
