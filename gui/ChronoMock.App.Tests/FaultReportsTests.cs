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
        // Two instances thrown from one place: the same signature, so the rule is about what failed where,
        // not about one object seen twice - a check by identity would let the second box through.
        var first = Thrown(() => new InvalidOperationException("one"));
        var second = Thrown(() => new InvalidOperationException("two"));

        Assert.True(reports.ShouldShow(first));
        Assert.False(reports.ShouldShow(second), "the same fault a second time is not new information");
        Assert.True(reports.ShouldShow(Thrown(() => new FormatException("three"))), "a different fault is");
    }

    /// <summary>A task that failed once stands for its one fault, and one that failed several ways keeps all
    /// of them rather than showing the first.</summary>
    [Fact]
    public void A_failed_task_is_its_one_fault_and_several_faults_stay_together()
    {
        var only = new InvalidOperationException("only");
        Assert.Same(only, FaultReports.Unwrap(new AggregateException(only)));

        var several = new AggregateException(new InvalidOperationException("a"), new FormatException("b"));
        Assert.Same(several, FaultReports.Unwrap(several));
    }

    /// <summary>The box says what happened and what to do first and the exception's own message last, and
    /// names the saved file only when there is one.</summary>
    [Fact]
    public void The_box_leads_with_what_to_do_and_names_the_file_only_when_it_was_written()
    {
        var fault = new InvalidOperationException("the message");

        var saved = FaultReports.DialogText(k => k, fault, @"C:\logs\diagnostics-1.log");
        Assert.StartsWith("fault.unexpected", saved, StringComparison.Ordinal);
        Assert.Contains("\n\nfault.saved C:\\logs\\diagnostics-1.log", saved, StringComparison.Ordinal);
        Assert.EndsWith("\n\nfault.detail the message", saved, StringComparison.Ordinal);

        var unsaved = FaultReports.DialogText(k => k, fault, savedPath: null);
        Assert.DoesNotContain("fault.saved", unsaved, StringComparison.Ordinal);
        Assert.Equal("fault.unexpected\n\nfault.detail the message", unsaved);
    }

    /// <summary>The saved record keeps what the box leaves out: the whole exception, with its stack.</summary>
    [Fact]
    public void The_record_keeps_the_whole_exception()
    {
        var fault = Thrown(() => new InvalidOperationException("recorded"));

        var record = FaultReports.Record(fault, "unobserved task");

        Assert.Contains("source: unobserved task", record, StringComparison.Ordinal);
        Assert.Contains(fault.ToString(), record, StringComparison.Ordinal);
        Assert.Contains(nameof(Thrown), record, StringComparison.Ordinal);
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
