using ChronoMock.App;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A refusal names the processes of the application it could not end, on the result and in the copied
/// summary (R4-S5, R4/8b).
/// </summary>
/// <remarks>
/// When the substitution did not take effect the core ends the application and everything it started. What it
/// could not end - a process running with more rights, one that did not end in time, a child it could not
/// confirm as the application's - still runs on the real clock, and the core names it in
/// <c>verdict.left_running</c>. The status line says the application was ended, so without the list the window
/// would hide exactly the processes a tester has to close by hand.
/// </remarks>
public class RefusalLeftRunningTests
{
    /// <summary>Keys resolve to themselves, so a test can find a line by the key that produced it.</summary>
    private static string Key(string key) => key;

    /// <summary>A refusal with what it left running, one named and one the process list could not name.</summary>
    [Fact]
    public void Each_process_a_refusal_left_running_is_one_line_under_the_refused_status()
    {
        var vm = PhaseStates.ResultRefusedLeftRunning();

        Assert.Equal(SessionStatusKind.Refused, vm.StatusKind);
        Assert.Equal("status.refused", vm.StatusKey);
        Assert.True(vm.HasLeftRunning);
        Assert.Equal(["helper.exe (pid 5150)", "pid 5151"], vm.LeftRunningRows);
    }

    /// <summary>The summary names them where the CLI report does: under the verdict they follow from, before the
    /// audit lists that are about something else.</summary>
    [Fact]
    public void The_summary_names_them_under_the_verdict()
    {
        var summary = PhaseStates.ResultRefusedLeftRunning().BuildSummary(Key);

        var verdict = summary.IndexOf("report.verdict", StringComparison.Ordinal);
        var header = summary.IndexOf("  report.left_running (2):\n", StringComparison.Ordinal);
        var named = summary.IndexOf("    - helper.exe (pid 5150)\n", StringComparison.Ordinal);
        var nameless = summary.IndexOf("    - pid 5151\n", StringComparison.Ordinal);
        var audit = summary.IndexOf("coverage.uncovered", StringComparison.Ordinal);
        Assert.True(
            verdict >= 0 && verdict < header && header < named && named < nameless && nameless < audit,
            $"out of place:\n{summary}");
    }

    /// <summary>The control: a refusal that ended everything says so in its status and lists nothing, on the
    /// screen or in the summary.</summary>
    [Fact]
    public void A_refusal_that_ended_everything_lists_nothing()
    {
        var vm = PhaseStates.ResultRefused();

        Assert.Equal(SessionStatusKind.Refused, vm.StatusKind);
        Assert.False(vm.HasLeftRunning);
        Assert.Empty(vm.LeftRunningRows);
        Assert.DoesNotContain("report.left_running", vm.BuildSummary(Key), StringComparison.Ordinal);
    }

    /// <summary>The next session starts without the last refusal's list - a list that outlived its session would
    /// name processes this one never started.</summary>
    [Fact]
    public void A_new_session_does_not_inherit_them()
    {
        var vm = PhaseStates.ResultRefusedLeftRunning();

        vm.BeginNewSession();

        Assert.Equal(SessionStatusKind.Idle, vm.StatusKind);
        Assert.False(vm.HasLeftRunning);
        Assert.Empty(vm.LeftRunningRows);
    }

    /// <summary>The field as the core writes it reaches the event, and a verdict without it - every verdict that
    /// is not a refusal, and every core older than the field - reads as an empty list.</summary>
    [Fact]
    public void The_wire_field_is_read_and_its_absence_is_an_empty_list()
    {
        const string refused =
            """{"type":"verdict","v":1,"id":1,"verdict":"fails","refuse_start":true,"reason_key":"coverage.time_channels_uncovered","left_running":[{"pid":5150,"image":"helper.exe"},{"pid":5151}]}""";
        const string works =
            """{"type":"verdict","v":1,"id":1,"verdict":"works","refuse_start":false,"reason_key":"coverage.time_channels_covered"}""";

        var left = Assert.IsType<VerdictEvent>(EventParser.Parse(refused)).LeftRunning;
        Assert.Equal([new FollowedProcess { Pid = 5150, Image = "helper.exe" }, new FollowedProcess { Pid = 5151 }], left);
        Assert.Empty(Assert.IsType<VerdictEvent>(EventParser.Parse(works)).LeftRunning);
    }
}
