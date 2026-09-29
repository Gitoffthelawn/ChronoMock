using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// One box per fault signature, up to a cap, shared by the dispatcher and by tasks nobody awaited (R4-N52).
/// </summary>
public class FaultReportsTests
{
    /// <summary>A thrown exception, so it carries the stack trace its signature is read from.</summary>
    private static Exception Thrown(Func<Exception> make)
    {
        try
        {
            throw make();
        }
        catch (Exception e)
        {
            return e;
        }
    }

    [Fact]
    public void A_repeating_fault_is_shown_once_and_a_new_one_is_still_heard()
    {
        var reports = new FaultReports();
        var first = Thrown(() => new InvalidOperationException("one"));

        Assert.True(reports.ShouldShow(first));
        Assert.False(reports.ShouldShow(first), "the same fault a second time is not new information");
        Assert.True(reports.ShouldShow(Thrown(() => new FormatException("two"))), "a different fault is");
    }

    [Fact]
    public void Past_the_cap_nothing_more_is_shown()
    {
        var reports = new FaultReports();
        var types = new Func<Exception>[]
        {
            () => new InvalidOperationException(), () => new FormatException(), () => new ArgumentException(),
            () => new NotSupportedException(), () => new TimeoutException(), () => new OverflowException(),
            () => new KeyNotFoundException(), () => new ArithmeticException(), () => new RankException(),
        };

        var shown = types.Count(make => reports.ShouldShow(Thrown(make)));

        Assert.Equal(FaultReports.MaxShown, shown);
    }
}
