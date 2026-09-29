namespace ChronoMock.App;

/// <summary>
/// Which unexpected faults the app still has to show, and what the box about one says. A repeating fault is
/// shown once and a genuinely new one is still heard, up to a cap.
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
/// share one memory of what was already shown. The second arrives on the finalizer thread, not the window's,
/// so the memory is locked.
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

    private readonly Lock _gate = new();

    /// <summary>Whether <paramref name="fault"/> is one to show: not shown before, and under the cap. Records
    /// it when it is. Safe from any thread.</summary>
    internal bool ShouldShow(Exception fault)
    {
        ArgumentNullException.ThrowIfNull(fault);
        var signature = $"{fault.GetType().FullName}|{fault.StackTrace?.Split('\n').FirstOrDefault()?.Trim()}";
        lock (_gate)
        {
            return _shown.Count < MaxShown && _shown.Add(signature);
        }
    }

    /// <summary>The fault a failed task stands for: the one inside it when there is exactly one, which is how
    /// a single awaited-nowhere failure arrives, and the aggregate itself when there are several, so none of
    /// them is dropped.</summary>
    internal static Exception Unwrap(AggregateException fault)
    {
        ArgumentNullException.ThrowIfNull(fault);
        return fault.InnerExceptions.Count == 1 ? fault.InnerExceptions[0] : fault;
    }

    /// <summary>
    /// The text of the box: what happened and what to do first, where the details went, and the exception's
    /// own message last, for whoever reads the saved file beside it. A raw exception message as the whole box
    /// told the reader neither what failed nor what to do next.
    /// </summary>
    /// <param name="translate">Resolves a translation key - the box is native, but the strings it shows are
    /// the interface's own.</param>
    /// <param name="fault">The fault being shown.</param>
    /// <param name="savedPath">Where the details were written, or null when they could not be - the line then
    /// goes, rather than naming a file that does not exist.</param>
    internal static string DialogText(Func<string, string> translate, Exception fault, string? savedPath)
    {
        ArgumentNullException.ThrowIfNull(translate);
        ArgumentNullException.ThrowIfNull(fault);
        var text = translate("fault.unexpected");
        if (savedPath is { Length: > 0 })
        {
            text += "\n\n" + translate("fault.saved") + " " + savedPath;
        }

        return text + "\n\n" + translate("fault.detail") + " " + fault.Message;
    }

    /// <summary>The block written to the diagnostics folder: the whole exception, stack and inner faults
    /// included, which the box leaves out. English and stable, like every diagnostics block (data, not
    /// interface text).</summary>
    internal static string Record(Exception fault, string source)
    {
        ArgumentNullException.ThrowIfNull(fault);
        return "Chrono Mock unexpected fault\n"
            + "  when:   " + DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ", System.Globalization.CultureInfo.InvariantCulture) + "\n"
            + "  source: " + source + "\n\n"
            + fault + "\n";
    }
}
