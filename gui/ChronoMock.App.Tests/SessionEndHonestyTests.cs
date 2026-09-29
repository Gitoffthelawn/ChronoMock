using ChronoMock.App;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A session the core never closed is not reported as its verdict (R4-W3, R4-S27).
/// </summary>
/// <remarks>
/// The core sends the parent's verdict a fraction of a second into the session and `ended` when it closes the
/// session. A core that died between the two left the panel leading with a green "Works" over the counts of
/// that first moment, beside a status that said the application's clock was frozen - which since ADR-14 it
/// is not. The summary carried no banner, no diagnostics were kept, and the history said "works".
/// </remarks>
public class SessionEndHonestyTests
{
    /// <summary>Keys resolve to themselves, so a test can find a line by the key that produced it.</summary>
    private static string Key(string key) => key;

    /// <summary>A session under way whose core sent the parent's works verdict and nothing after it.</summary>
    private static SessionViewModel WorksAtTheStart()
    {
        var vm = SessionStates.RunningWithCoverageWarnings();
        vm.Apply(new VerdictEvent { V = ProtocolJson.ProtocolVersion, Verdict = "works", ReasonKey = "coverage.time_channels_covered" });
        return vm;
    }

    [Fact]
    public void A_core_that_stopped_after_a_works_verdict_is_cut_short_and_never_evidence()
    {
        var vm = WorksAtTheStart();

        vm.OnStreamEnded(watchdogFired: false, stopRequested: false);

        Assert.Equal(SessionStatusKind.CoreStopped, vm.StatusKind);
        Assert.True(vm.IsCutShort);
        Assert.Equal("result.headline_cut_short", vm.ResultHeadlineKey);
        Assert.Equal(VerdictKind.Undetermined, vm.ResultKind);
        Assert.False(vm.ResultHasReason, "the verdict's reason explains a word that is no longer the headline");
        Assert.Equal("undetermined", vm.BuildRecord().Verdict);

        var summary = vm.BuildSummary(Key);
        Assert.StartsWith("report.unreliable_banner", summary, StringComparison.Ordinal);
        var cut = summary.IndexOf("report.cut_short", StringComparison.Ordinal);
        var verdict = summary.IndexOf("report.verdict", StringComparison.Ordinal);
        Assert.True(cut >= 0 && cut < verdict, $"the caveat stands above the verdict it qualifies:\n{summary}");

        vm.CaptureDiagnostics(["core stderr: panicked"], coreExit: -1073741819);
        Assert.Contains("core exit: -1073741819 (0xC0000005)", vm.DiagnosticsText, StringComparison.Ordinal);
    }

    [Fact]
    public void A_core_that_went_quiet_is_cut_short_too()
    {
        var vm = WorksAtTheStart();

        vm.OnStreamEnded(watchdogFired: true, stopRequested: false);

        Assert.Equal(SessionStatusKind.CoreUnresponsive, vm.StatusKind);
        Assert.True(vm.IsCutShort);
        Assert.Equal("result.headline_cut_short", vm.ResultHeadlineKey);
    }

    /// <summary>
    /// A Stop is the one ending that can go either way: a healthy core answers it with `ended` in milliseconds,
    /// and one that does not is killed after its grace period with nothing more said.
    /// </summary>
    [Fact]
    public void A_stop_the_core_closed_keeps_its_verdict_and_one_it_did_not_is_cut_short()
    {
        var closed = SessionStates.Ended();
        closed.OnStreamEnded(watchdogFired: false, stopRequested: true);
        Assert.Equal(SessionStatusKind.Stopped, closed.StatusKind);
        Assert.False(closed.IsCutShort);
        Assert.Equal("verdict.works", closed.ResultHeadlineKey);
        Assert.DoesNotContain("report.unreliable_banner", closed.BuildSummary(Key), StringComparison.Ordinal);

        var killed = WorksAtTheStart();
        killed.OnStreamEnded(watchdogFired: false, stopRequested: true);
        Assert.Equal(SessionStatusKind.Stopped, killed.StatusKind);
        Assert.True(killed.IsCutShort);
        Assert.StartsWith("report.unreliable_banner", killed.BuildSummary(Key), StringComparison.Ordinal);
    }

    /// <summary>The control: a session the core closed is what it always was.</summary>
    [Fact]
    public void A_session_the_core_closed_is_reliable_and_recorded_as_its_verdict()
    {
        var vm = SessionStates.Ended();
        vm.OnStreamEnded(watchdogFired: false, stopRequested: false);

        Assert.Equal(SessionStatusKind.Ended, vm.StatusKind);
        Assert.False(vm.IsCutShort);
        Assert.Equal("works", vm.BuildRecord().Verdict);
        Assert.DoesNotContain("report.unreliable_banner", vm.BuildSummary(Key), StringComparison.Ordinal);
        Assert.DoesNotContain("report.cut_short", vm.BuildSummary(Key), StringComparison.Ordinal);
        vm.CaptureDiagnostics(["core stderr: nothing"], coreExit: 0);
        Assert.Equal(string.Empty, vm.DiagnosticsText);
    }

    /// <summary>An error after the first heartbeat keeps the verdict on screen, because the substitution did
    /// work while the session lasted - but the session failed, so it is not evidence either.</summary>
    [Fact]
    public void An_error_during_the_session_keeps_the_verdict_but_is_not_evidence()
    {
        var vm = WorksAtTheStart();
        vm.Apply(new ErrorEvent { V = ProtocolJson.ProtocolVersion, Id = 1, Code = 3, Key = "target.inject_failed", Origin = "core" });

        Assert.Equal(SessionStatusKind.Error, vm.StatusKind);
        Assert.Equal("verdict.works", vm.ResultHeadlineKey);
        Assert.StartsWith("report.unreliable_banner", vm.BuildSummary(Key), StringComparison.Ordinal);
    }

    /// <summary>
    /// R4-S27: the status turns terminal as the event stream ends, and the start task then still records the
    /// session, disposes the core and captures diagnostics before it goes idle. A new form begun in between
    /// was written over by that tail, so New session waits for idle. The window is not reachable without a
    /// live core, so the state is set on the field the start task clears.
    /// </summary>
    [Fact]
    public void New_session_waits_until_the_session_is_closed_down()
    {
        var vm = SessionStates.Ended();
        Assert.True(vm.CanBeginNewSession);

        typeof(SessionViewModel)
            .GetField("_idle", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance)!
            .SetValue(vm, false);

        Assert.False(vm.CanBeginNewSession, "the result of a session still closing down is not one to leave");
    }

    [Fact]
    public void The_diagnostics_block_names_the_core_s_exit_code_only_when_it_is_known()
    {
        var vm = WorksAtTheStart();
        vm.OnStreamEnded(watchdogFired: false, stopRequested: false);

        Assert.Contains("  core exit: 1\n", vm.BuildDiagnosticsBlock([], 0, 1), StringComparison.Ordinal);
        Assert.DoesNotContain("core exit", vm.BuildDiagnosticsBlock([], 0, null), StringComparison.Ordinal);
    }
}
