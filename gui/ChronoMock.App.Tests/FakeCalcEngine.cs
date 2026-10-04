using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// An engine that answers when the test says so, in the order the test chooses (R4-W4, R4-S25).
/// <para>
/// Every question is recorded with its arguments and waits for its own reply, unless a responder answers
/// it at once. A real chrono.exe answers in whatever order the machine schedules it, which is exactly what
/// a test of "a late answer must not land" cannot use: it would pass or fail by luck.
/// </para>
/// <para>
/// <c>honoursCancellation: false</c> makes the late answer arrive even after the question was cancelled,
/// so a test can prove the answer is REFUSED for being old, not merely never delivered. The real engine
/// does stop on cancellation, so with the fake ignoring it the generation check is the only thing standing.
/// </para>
/// </summary>
internal sealed class FakeCalcEngine : ICalcEngine
{
    private static readonly TimeSpan WaitForQuestion = TimeSpan.FromSeconds(5);

    private readonly object _gate = new();
    private readonly List<Question> _questions = [];
    private readonly Func<IReadOnlyList<string>, CalcResult>? _responder;
    private readonly bool _honoursCancellation;

    public FakeCalcEngine(Func<IReadOnlyList<string>, CalcResult>? responder = null, bool honoursCancellation = true)
    {
        _responder = responder;
        _honoursCancellation = honoursCancellation;
    }

    /// <summary>How many questions were asked so far.</summary>
    public int Count
    {
        get
        {
            lock (_gate)
            {
                return _questions.Count;
            }
        }
    }

    public Task<CalcResult> EvaluateAsync(IReadOnlyList<string> calcArgs, CancellationToken ct = default)
    {
        var question = new Question(calcArgs);
        lock (_gate)
        {
            _questions.Add(question);
        }

        if (_honoursCancellation)
        {
            ct.Register(() => question.Reply.TrySetCanceled(ct));
        }

        if (_responder is not null)
        {
            try
            {
                question.Reply.TrySetResult(_responder(calcArgs));
            }
            catch (Exception e)
            {
                question.Reply.TrySetException(e);
            }
        }

        return question.Reply.Task;
    }

    /// <summary>The question asked <paramref name="index"/>-th, waiting for it to be asked. A question that
    /// never comes fails the test loudly rather than hang it.</summary>
    public async Task<Question> QuestionAsync(int index)
    {
        var deadline = DateTime.UtcNow + WaitForQuestion;
        while (true)
        {
            lock (_gate)
            {
                if (_questions.Count > index)
                {
                    return _questions[index];
                }
            }

            if (DateTime.UtcNow > deadline)
            {
                throw new TimeoutException($"question {index} was never asked - {Count} were");
            }

            await Task.Delay(10, TestContext.Current.CancellationToken);
        }
    }

    /// <summary>The newest question asked.</summary>
    public Question Last
    {
        get
        {
            lock (_gate)
            {
                return _questions[^1];
            }
        }
    }

    /// <summary>One question and the reply it waits on.</summary>
    internal sealed class Question(IReadOnlyList<string> args)
    {
        public IReadOnlyList<string> Args { get; } = args;

        public TaskCompletionSource<CalcResult> Reply { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        /// <summary>The value after <paramref name="flag"/>, or null when the flag was not passed.</summary>
        public string? ValueOf(string flag)
        {
            for (var i = 0; i + 1 < Args.Count; i++)
            {
                if (Args[i] == flag)
                {
                    return Args[i + 1];
                }
            }

            return null;
        }

        public void Answer(CalcResult result) => Reply.TrySetResult(result);

        public void Refuse(string message) => Reply.TrySetException(new CalcException(message, 1));
    }
}

/// <summary>Engine answers small enough to write in a test.</summary>
internal static class CalcResults
{
    private const string Schema = "chronomock.calc/1";

    private static readonly CalcMetadata Metadata = new("Monday", 2026, 1, 1, 1, 1, false, 0, null, null);

    /// <summary>A moment whose formats repeat its date and time, so a test reads which answer is on screen
    /// off any row. A null <paramref name="epochSeconds"/> is a moment outside the epoch formats.</summary>
    public static CalcResult Moment(
        string iso,
        int zoneBias = 0,
        long? epochSeconds = 0,
        string? custom = null,
        IReadOnlyList<string>? unknown = null,
        IReadOnlyList<CalcClampedStep>? clamps = null)
        => new(
            Schema,
            new CalcMoment(
                iso,
                zoneBias,
                "base",
                [],
                new CalcFormats(iso[..10], iso, iso, iso, epochSeconds, epochSeconds * 1000, epochSeconds, iso),
                Metadata,
                [],
                custom,
                unknown,
                clamps,
                null),
            null);

    /// <summary>The engine's reading ids, in the order it lists them for an ambiguous numeric date.</summary>
    private static readonly string[] ReadingIds = ["us_month_day", "pl_day_month", "iso"];

    /// <summary>An analysis with one reading per date (at most three), ambiguous when there is more than one.</summary>
    public static CalcResult Analysis(params string[] isos)
        => new(
            Schema,
            null,
            new CalcAnalysis("pasted", isos.Length > 1, [.. isos.Select((iso, i) => new CalcReading(ReadingIds[i], iso, [], Metadata))]));

    /// <summary>An answer that carries neither a moment nor an analysis.</summary>
    public static CalcResult Nothing() => new(Schema, null, null);
}
