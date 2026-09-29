namespace ChronoMock.App;

/// <summary>
/// Which unexpected faults the app still has to show. A repeating fault is shown once and a genuinely new one
/// is still heard, up to a cap.
/// </summary>
/// <remarks>
/// A modal box per occurrence is not an option (R2-N12): a repeating exception - a binding that throws on
/// every heartbeat - opened one box per beat, and a window the user cannot out-click is worse than the fault
/// it reports. But the fix for that was a flag set once per RUN, which silenced every later exception
/// including UNRELATED ones. An early stumble in the preset list then muted a coverage failure an hour later,
/// and the app went on looking healthy while every operation threw. That is rule 6 spread over a session.
/// So the choice is per SIGNATURE, not per run.
/// <para>
/// Its own type, rather than two fields on the application, so the rule is tested without a window, and so
/// the two ways a fault reaches the app - the dispatcher, and a background task nobody awaited (R4-N52) -
/// share one memory of what was already shown.
/// </para>
/// </remarks>
internal sealed class FaultReports
{
    /// <summary>How many DISTINCT faults are shown before the app stops opening boxes. Past this many
    /// different failures the interface is not recoverable in any useful sense, and the boxes have stopped
    /// being information - one more would be noise on top of an app that is already broken.</summary>
    internal const int MaxShown = 8;

    /// <summary>Fault signatures already shown this run - type plus innermost frame, so a repeating fault is
    /// one box and a genuinely different one is still heard.</summary>
    private readonly HashSet<string> _shown = new(StringComparer.Ordinal);

    /// <summary>Whether <paramref name="fault"/> is one to show: not shown before, and under the cap. Records
    /// it when it is.</summary>
    internal bool ShouldShow(Exception fault)
    {
        ArgumentNullException.ThrowIfNull(fault);
        var signature = $"{fault.GetType().FullName}|{fault.StackTrace?.Split('\n').FirstOrDefault()?.Trim()}";
        return _shown.Count < MaxShown && _shown.Add(signature);
    }
}
