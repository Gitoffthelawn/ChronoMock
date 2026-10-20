using System.Globalization;
using System.Runtime.CompilerServices;
using ChronoMock.App;
using ChronoMock.App.Localization;

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

        Assert.Equal(FaultNotice.Show, reports.Hear(first));
        Assert.Equal(FaultNotice.None, reports.Hear(second)); // the same fault a second time is not new information
        Assert.Equal(FaultNotice.Show, reports.Hear(Thrown(() => new FormatException("three")))); // a different one is
    }

    // Two places in this code that hand .NET the same bad text: the fault is thrown INSIDE .NET in both, from
    // one frame there, and only the frame of ours below it tells them apart.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int ReadsTheCount() => int.Parse("not a number", CultureInfo.InvariantCulture);

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int ReadsTheLimit() => int.Parse("not a number", CultureInfo.InvariantCulture);

    /// <summary>
    /// R5/6b, D1. Two faults of one type thrown from one frame inside .NET, reached from two different places in
    /// this code, are two faults. The signature took the innermost frame of all, which was the .NET one for
    /// both, so the second was neither shown nor written. Reversal probe: sign by the innermost frame again and
    /// the second is heard as the first.
    /// </summary>
    [Fact]
    public void Two_faults_thrown_inside_dotnet_from_two_places_of_ours_are_two_faults()
    {
        var first = Record.Exception(() => ReadsTheCount())!;
        var second = Record.Exception(() => ReadsTheLimit())!;
        Assert.Equal( // the innermost frame is the same one inside .NET - not vacuous
            first.StackTrace!.Split('\n')[0].Trim(), second.StackTrace!.Split('\n')[0].Trim());

        var reports = new FaultReports();

        Assert.Equal(FaultNotice.Show, reports.Hear(first));
        Assert.Equal(FaultNotice.Show, reports.Hear(second));
        Assert.Contains(nameof(ReadsTheLimit), FaultReports.Signature(second), StringComparison.Ordinal);
    }

    /// <summary>
    /// R5/6b, D1. Past the cap on boxes a fault is still written, up to the cap on files, and the last file says
    /// that later faults are not written. The ninth fault used to leave nothing at all. Reversal probe: return
    /// before writing past the box cap and the ninth notice is None.
    /// </summary>
    [Fact]
    public void Past_the_cap_on_boxes_faults_are_still_written_up_to_their_own_cap()
    {
        var reports = new FaultReports();
        var notices = Enumerable.Range(0, FaultReports.MaxRecorded + 3)
            .Select(i => reports.Hear($"System.InvalidOperationException|at ChronoMock.Place{i}()"))
            .ToList();

        Assert.Equal(FaultReports.MaxShown, notices.Count(n => n == FaultNotice.Show));
        Assert.Equal(FaultNotice.Record, notices[FaultReports.MaxShown]);
        Assert.Equal(FaultReports.MaxRecorded - FaultReports.MaxShown - 1, notices.Count(n => n == FaultNotice.Record));
        Assert.Equal(FaultNotice.RecordLast, notices[FaultReports.MaxRecorded - 1]);
        Assert.All(notices.Skip(FaultReports.MaxRecorded), n => Assert.Equal(FaultNotice.None, n));
        Assert.Contains("later ones are not written", FaultReports.Record(new InvalidOperationException(), "dispatcher", last: true), StringComparison.Ordinal);
        Assert.DoesNotContain("note:", FaultReports.Record(new InvalidOperationException(), "dispatcher"), StringComparison.Ordinal);
    }

    /// <summary>
    /// R5/6b, D1. The box of a failed start says it could not start, names the file the whole exception went
    /// to, and ends with the message - in the window's words, or in the English it would have shown when the
    /// window has no words because the strings are what failed.
    /// </summary>
    [Fact]
    public void A_failed_start_says_so_names_the_file_and_falls_back_to_english_without_strings()
    {
        var fault = new InvalidOperationException("the strings file is missing");

        var withStrings = FaultReports.StartupText(k => "<" + k + ">", fault, @"C:\logs\diagnostics-1.log");
        Assert.Equal(
            "<fault.startup>\n\n<fault.saved> C:\\logs\\diagnostics-1.log\n\n<fault.detail> the strings file is missing",
            withStrings);

        var without = FaultReports.StartupText(k => k, fault, savedPath: null);
        Assert.Equal("Chrono Mock could not start.\n\nTechnical detail: the strings file is missing", without);
    }

    /// <summary>The English the startup box falls back to is the English the window speaks, word for word.</summary>
    [Fact]
    public void The_english_fallback_says_what_the_english_strings_say()
    {
        var english = LocalizationService.Load(LocalizationService.DefaultCulture);

        Assert.All(FaultReports.EnglishFallback, pair => Assert.Equal(english[pair.Key], pair.Value));
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

        var shown = types.Count(make => reports.Hear(Thrown(make)) == FaultNotice.Show);

        Assert.Equal(FaultReports.MaxShown, shown);
    }
}
