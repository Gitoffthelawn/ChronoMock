namespace ChronoMock.Protocol;

/// <summary>
/// The calculator engine as the interface sees it: one question in, one answer out. <see cref="CalcClient"/>
/// is the engine every build ships, a one-shot <c>chrono calc</c> process per question.
/// <para>
/// It is an interface because the screens that ask it have to be proven against an engine that answers
/// in an order the test chooses. An answer that arrives after the input moved on is the whole class of
/// fault the screens guard against (R4-W4, R4-S25), and a real process answers in whatever order the
/// machine happens to schedule it, so a test over one could only ever pass by luck.
/// </para>
/// </summary>
public interface ICalcEngine
{
    /// <summary>
    /// Evaluate a calc invocation. <paramref name="calcArgs"/> are the flags after <c>calc</c>. Throws
    /// <see cref="CalcException"/> when the engine refuses, and <see cref="OperationCanceledException"/>
    /// when <paramref name="ct"/> is cancelled first.
    /// </summary>
    Task<CalcResult> EvaluateAsync(IReadOnlyList<string> calcArgs, CancellationToken ct = default);
}
