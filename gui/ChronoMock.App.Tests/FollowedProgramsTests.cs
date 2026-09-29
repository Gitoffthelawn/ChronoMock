using ChronoMock.App;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The programs a session went on for after the application closed are named on the result and in the copied
/// summary (R4-N53, ADR-16).
/// </summary>
/// <remarks>
/// A launcher that starts the real program and closes leaves the session running for that program, and the
/// CLI report names it under the exit line. The window said only "The application exited on its own with exit
/// code 0", and the one warning that explained the rest sat among the others, below the fold.
/// </remarks>
public class FollowedProgramsTests
{
    /// <summary>Keys resolve to themselves, so a test can find a line by the key that produced it.</summary>
    private static string Key(string key) => key;

    /// <summary>A session whose application closed on its own while the programs it started ran on.</summary>
    private static SessionViewModel EndedAfterHandOff(IReadOnlyList<FollowedProcess> followed)
    {
        var vm = SessionStates.RunningWithCoverageWarnings();
        vm.Apply(new SessionVerdictEvent
        {
            V = ProtocolJson.ProtocolVersion,
            Verdict = "works",
            ReasonKey = "session.family_covered",
            ProcessCount = 1 + followed.Count,
            WarningKeys = ["session.followed_family"],
            Followed = followed,
        });
        vm.Apply(new EndedEvent { V = ProtocolJson.ProtocolVersion, Clean = true, TargetExitCode = 0 });
        return vm;
    }

    /// <summary>One line each, in the CLI report's spelling: a nameless one is its pid alone, whether the wire
    /// left the name out or sent it empty, and the largest pid a Windows process can have prints whole.</summary>
    [Fact]
    public void Each_program_the_session_went_on_for_is_one_line_in_the_spelling_of_the_cli_report()
    {
        var vm = EndedAfterHandOff(
        [
            new FollowedProcess { Pid = 5150, Image = "app.exe" },
            new FollowedProcess { Pid = 5151 },
            new FollowedProcess { Pid = uint.MaxValue, Image = string.Empty },
        ]);

        Assert.True(vm.HasFollowed);
        Assert.Equal(["app.exe (pid 5150)", "pid 5151", "pid 4294967295"], vm.FollowedRows);
    }

    /// <summary>The summary names them where the CLI report does: under the line that says the application
    /// closed, and before the audit lists that are about something else.</summary>
    [Fact]
    public void The_summary_names_them_under_the_exit_code()
    {
        var summary = EndedAfterHandOff([new FollowedProcess { Pid = 5150, Image = "app.exe" }]).BuildSummary(Key);

        var exit = summary.IndexOf("report.target_exit 0", StringComparison.Ordinal);
        var header = summary.IndexOf("  report.followed (1):\n", StringComparison.Ordinal);
        var row = summary.IndexOf("    - app.exe (pid 5150)\n", StringComparison.Ordinal);
        var audit = summary.IndexOf("coverage.covered", StringComparison.Ordinal);
        Assert.True(exit >= 0 && exit < header && header < row && row < audit, $"out of place:\n{summary}");
    }

    /// <summary>The control: a session that ended with its application says nothing about programs it went
    /// on for, on the screen or in the summary.</summary>
    [Fact]
    public void A_session_that_went_on_for_nothing_says_nothing_about_it()
    {
        var vm = SessionStates.Ended();

        Assert.False(vm.HasFollowed);
        Assert.Empty(vm.FollowedRows);
        Assert.DoesNotContain("report.followed", vm.BuildSummary(Key), StringComparison.Ordinal);
    }

    /// <summary>The next session starts without the last one's programs - a list that outlived its session would
    /// name programs this one never ran.</summary>
    [Fact]
    public void A_new_session_does_not_inherit_them()
    {
        var vm = EndedAfterHandOff([new FollowedProcess { Pid = 5150, Image = "app.exe" }]);

        vm.BeginNewSession();

        Assert.Equal(SessionStatusKind.Idle, vm.StatusKind);
        Assert.False(vm.HasFollowed);
        Assert.Empty(vm.FollowedRows);
    }
}
