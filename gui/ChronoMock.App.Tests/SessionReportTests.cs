using ChronoMock.App;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The reports a session leaves behind, composed from its facts and its start setup alone - no view model,
/// no window, no core (R5-D62). The view model's own tests drive the same functions through Apply, so these
/// hold what only the seam makes reachable: a report from facts written by hand, and a clock the caller owns.
/// </summary>
public class SessionReportTests
{
    /// <summary>Keys resolve to themselves, so a test can find a line by the key that produced it.</summary>
    private static string Key(string key) => key;

    private static readonly DateTime Noon = new(2031, 5, 6, 12, 0, 0, DateTimeKind.Utc);

    /// <summary>Every field set to something no other field holds, so a field read from the wrong place
    /// shows up as the wrong value rather than as a coincidence.</summary>
    private static SessionSetup Distinct() => new(
        TargetPath: @"C:\apps\ledger.exe",
        Moment: "2031-02-03T04:05:06",
        Zone: TimeInputs.Zones.First(z => z.BiasMinutes != 0),
        Mode: TimeInputs.Modes.First(m => m.Multiplier is > 1),
        TargetArgs: "--profile two",
        WorkingFolder: @"C:\work",
        ScaleDuration: true,
        ScaleQpc: false,
        KeepBackgroundTimers: true,
        Force: false,
        ReachEmbedded: false,
        ReachElevated: true);

    [Fact]
    public void The_record_carries_every_field_of_the_setup_it_was_started_with()
    {
        var setup = Distinct();
        var result = new SessionResult { VerdictKnown = true, VerdictKind = VerdictKind.Partial };

        var record = SessionReport.Record(result, setup, Noon);

        Assert.Equal(setup.TargetPath, record.TargetPath);
        Assert.Equal(setup.Moment, record.MomentLocal);
        Assert.Equal(setup.Zone.BiasMinutes, record.TzBiasMin);
        Assert.Equal(setup.Mode.Mode, record.Mode);
        Assert.Equal(setup.Mode.Multiplier, record.Multiplier);
        Assert.Equal(setup.TargetArgs, record.TargetArgs);
        Assert.Equal(setup.WorkingFolder, record.WorkingFolder);
        Assert.Equal(
            (true, false, true, false, false, true),
            (record.ScaleDuration, record.ScaleQpc, record.KeepBackgroundTimers, record.Force, record.Embedded,
                record.ElevatedEmbedded));
        Assert.Equal("partial", record.Verdict);
        Assert.Equal("2031-05-06T12:00:00Z", record.EndedAtUtc);
    }

    [Fact]
    public void The_diagnostics_block_is_stamped_with_the_clock_it_is_given()
    {
        var block = SessionReport.Diagnostics(new SessionResult(), Distinct(), ["core said this"], 0, null, Noon);

        Assert.Contains("  when:      2031-05-06T12:00:00Z\n", block, StringComparison.Ordinal);
        Assert.Contains(@"  target:    C:\apps\ledger.exe", block, StringComparison.Ordinal);
        Assert.Contains("    core said this\n", block, StringComparison.Ordinal);
    }

    [Fact]
    public void A_summary_names_the_start_setup_and_the_facts_it_is_given()
    {
        var setup = Distinct();
        var result = new SessionResult
        {
            StatusKind = SessionStatusKind.Ended,
            EndedSeen = true,
            VerdictKnown = true,
            VerdictKind = VerdictKind.Works,
            VerdictLabelKey = "verdict.works",
            Warnings = ["coverage.session_clock_never_read"],
        };

        var summary = SessionReport.Summary(result, setup, "-", Key);

        // A works verdict whose warnings undo it as evidence still leads with the banner (R5-D3).
        Assert.StartsWith("report.unreliable_banner", summary, StringComparison.Ordinal);
        Assert.Contains("  report.target: ledger.exe\n", summary, StringComparison.Ordinal);
        Assert.Contains("  report.verdict: verdict.works\n", summary, StringComparison.Ordinal);
        Assert.Contains("    - coverage.session_clock_never_read\n", summary, StringComparison.Ordinal);
    }
}
