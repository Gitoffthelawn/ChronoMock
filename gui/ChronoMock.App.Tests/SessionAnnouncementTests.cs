using System.Windows.Input;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// What the session panel announces, and only that (R5-WY4). Every command used to be told to look at its gate
/// again on every property change, a heartbeat re-announced the whole result, and every coverage report put
/// new lists on the audit whether or not they said anything new - measured as 59 356 CanExecuteChanged and
/// 3 124 property changes for a family of 256 processes. Each announcement is now made when the thing it
/// announces changed, and the tests below hold both halves: nothing said for nothing, and nothing changed
/// without being said.
/// </summary>
public class SessionAnnouncementTests
{
    private static readonly TimeSpan Limit = TimeSpan.FromSeconds(10);

    private static StateEvent Heartbeat(long second) => new()
    {
        V = ProtocolJson.ProtocolVersion,
        Fake = new Clock { Wall = "2038-01-19T03:14:07", ZoneBiasMin = 0 },
        Real = new Clock { Wall = "2026-10-21T10:00:00", ZoneBiasMin = 0 },
        Multiplier = 1,
        ElapsedFakeMs = second * 1000,
        ElapsedRealMs = second * 1000,
    };

    private static CoverageEvent Report(uint pid) => new()
    {
        V = ProtocolJson.ProtocolVersion,
        Pid = pid,
        Covered = [new CoveredChannel { Channel = "GetSystemTimeAsFileTime", Calls = pid }],
        Uncovered = ["QueryPerformanceCounter"],
        Unobserved = ["NtQuerySystemTime"],
        InstalledLate = ["timeGetTime"],
        WarningKeys = ["wait.timeout_collapsed"],
    };

    /// <summary>The panel's commands by name, read off the type so a command added later is counted too.</summary>
    private static Dictionary<string, ICommand> CommandsOf(SessionViewModel vm) => typeof(SessionCommands)
        .GetProperties()
        .Where(p => p.PropertyType == typeof(ICommand))
        .ToDictionary(p => p.Name, p => (ICommand)p.GetValue(vm.Commands)!);

    /// <summary>A command's answer for a parameter it takes - the three typed commands refuse a null one
    /// whatever their gate says, which would hide the gate.</summary>
    private static bool CanRun(string name, ICommand command) => command.CanExecute(name switch
    {
        "Speed" => 60L,
        "Jump" => "+1d",
        "SetCustomSpeed" => "x2",
        _ => null,
    });

    [Fact]
    public void A_heartbeat_that_changes_no_gate_tells_no_command()
    {
        var vm = new SessionViewModel();
        vm.Apply(Heartbeat(1));
        var told = 0;
        foreach (var command in CommandsOf(vm).Values)
        {
            command.CanExecuteChanged += (_, _) => told++;
        }

        for (var second = 2; second <= 31; second++)
        {
            vm.Apply(Heartbeat(second));
        }

        Assert.Equal(0, told); // it was nineteen a property change, fifteen property changes a heartbeat
    }

    [Fact]
    public void A_heartbeat_after_the_first_announces_only_where_the_fake_clock_got_to()
    {
        var vm = new SessionViewModel();
        var first = Announced(vm, () => vm.Apply(Heartbeat(1)));

        var later = Announced(vm, () => vm.Apply(Heartbeat(2)));

        Assert.Contains(nameof(SessionViewModel.HasTiming), first); // the first one says there is timing
        Assert.Contains(nameof(SessionViewModel.ResultHeadlineKey), first);
        Assert.Equal([nameof(SessionViewModel.FakeEndPreview)], later);
        Assert.Equal("0:00:02", vm.Fake.Elapsed); // the elapsed clocks still move, on their own objects
    }

    [Fact]
    public void A_report_that_adds_nothing_to_the_audit_announces_nothing()
    {
        var vm = new SessionViewModel();
        vm.Apply(Heartbeat(1));
        var first = Announced(vm, () => vm.Apply(Report(1000)));

        // A child reporting the same channels and warnings as its parent: the audit already says all of it.
        var child = Announced(vm, () => vm.Apply(Report(1001)));

        Assert.Contains(nameof(SessionViewModel.Uncovered), first); // not vacuous: the first report is said
        Assert.Empty(child);
        Assert.Equal(["QueryPerformanceCounter"], vm.Uncovered);
    }

    [Fact]
    public void A_report_that_adds_a_channel_announces_that_list_and_not_the_others()
    {
        var vm = new SessionViewModel();
        vm.Apply(Report(1000));

        var child = Announced(vm, () => vm.Apply(Report(1001) with { Uncovered = ["QueryPerformanceCounter", "GetTickCount"] }));

        Assert.Contains(nameof(SessionViewModel.Uncovered), child);
        Assert.DoesNotContain(nameof(SessionViewModel.Unobserved), child);
        Assert.DoesNotContain(nameof(SessionViewModel.Warnings), child);
        Assert.Equal(["QueryPerformanceCounter", "GetTickCount"], vm.Uncovered);
    }

    /// <summary>
    /// The other half, over a whole session through a core that answers at once: set up, connecting, running,
    /// ended, recorded, a history row chosen. Every command whose answer changed was told so by the time the
    /// panel's next change was announced - a gate left off the watch keeps its button as it was, which no test
    /// of CanExecute alone would see.
    /// </summary>
    [Fact]
    public async Task Every_command_whose_answer_changed_during_a_session_was_told_so()
    {
        var target = SessionPlan.DefaultTargetPath()
            ?? throw new InvalidOperationException("the sample target is not built - build the solution first");
        var core = new FakeCoreConnection(
            [FakeCoreConnection.ReadyFor(target)],
            command => command is StartCommand
                ? [Heartbeat(1), new VerdictEvent { V = ProtocolJson.ProtocolVersion, Verdict = "works", ReasonKey = "verdict.reason.works" }]
                : []);
        var vm = new SessionViewModel(new InMemorySessionHistoryStore(), connect: core.Connector);
        var commands = CommandsOf(vm);
        var gate = new Lock();
        var told = new HashSet<string>();
        var changed = new HashSet<string>();
        var missed = new List<string>();
        var last = commands.ToDictionary(c => c.Key, c => CanRun(c.Key, c.Value));
        foreach (var (name, command) in commands)
        {
            command.CanExecuteChanged += (_, _) =>
            {
                lock (gate)
                {
                    told.Add(name);
                }
            };
        }

        // After the commands' own handler, which the view model's constructor attached first.
        void Look(string after)
        {
            lock (gate)
            {
                foreach (var (name, command) in commands)
                {
                    var now = CanRun(name, command);
                    if (now != last[name])
                    {
                        changed.Add(name);
                        if (!told.Contains(name))
                        {
                            missed.Add($"{name} went {last[name]} -> {now} at {after} untold");
                        }

                        last[name] = now;
                    }
                }

                told.Clear();
            }
        }

        vm.PropertyChanged += (_, e) => Look(e.PropertyName ?? "?");
        vm.JumpMoment.PropertyChanged += (_, e) => Look("JumpMoment." + e.PropertyName);

        vm.SetTarget(target);
        var run = vm.StartAsync();
        await CalculatorAnswerTests.Until(() => vm.IsRunning, "the session running");
        vm.JumpMoment.DateText = "2038-01-20";
        vm.RequestStop();
        core.End();
        await run.WaitAsync(Limit, TestContext.Current.CancellationToken);
        vm.SelectedRecord = vm.History[0];

        Assert.Empty(missed);
        // Not vacuous: most of the panel's commands changed their answer on the way.
        Assert.True(changed.Count >= 15, $"only {changed.Count} changed: {string.Join(", ", changed)}");
    }

    /// <summary>The names the view model announced while <paramref name="change"/> ran, in order.</summary>
    private static List<string> Announced(SessionViewModel vm, Action change)
    {
        var names = new List<string>();
        void Note(object? sender, System.ComponentModel.PropertyChangedEventArgs e) => names.Add(e.PropertyName ?? "?");
        vm.PropertyChanged += Note;
        change();
        vm.PropertyChanged -= Note;
        return names;
    }
}
