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

    /// <summary>
    /// How many DISTINCT faults are written to the diagnostics folder. More than are shown (R5/6b, D1): the
    /// ninth fault used to be neither shown nor written, so the file a support request asks for stopped at
    /// exactly the point the app was most broken. Still a cap, because the folder keeps its newest fifty files
    /// and a storm of faults would otherwise push out the session logs it is there for.
    /// </summary>
    internal const int MaxRecorded = 32;

    /// <summary>Fault signatures already heard this run (<see cref="Signature"/>), so a repeating fault is
    /// one box and one file, and a genuinely different one is still heard.</summary>
    private readonly HashSet<string> _heard = new(StringComparer.Ordinal);

    private readonly Lock _gate = new();

    /// <summary>
    /// What to do about <paramref name="fault"/>: nothing when it was heard before or past
    /// <see cref="MaxRecorded"/>, write it and show it while under <see cref="MaxShown"/>, write it only after
    /// that - and say so in the last file written. Remembers it. Safe from any thread.
    /// </summary>
    internal FaultNotice Hear(Exception fault) => Hear(Signature(fault));

    /// <summary><see cref="Hear(Exception)"/> for a signature already read - separate so the caps are tested
    /// without throwing forty different exceptions.</summary>
    internal FaultNotice Hear(string signature)
    {
        lock (_gate)
        {
            if (_heard.Count >= MaxRecorded || !_heard.Add(signature))
            {
                return FaultNotice.None;
            }

            return _heard.Count <= MaxShown ? FaultNotice.Show
                : _heard.Count < MaxRecorded ? FaultNotice.Record
                : FaultNotice.RecordLast;
        }
    }

    /// <summary>
    /// What tells one fault from another: its type and the innermost frame of THIS application's code it was
    /// thrown through (R5/6b, D1). The innermost frame of all is often inside .NET - a helper that throws for
    /// every argument check - so two unrelated faults thrown that way read as one, and the second was never
    /// shown or written. A fault with no frame of ours keeps the innermost one.
    /// </summary>
    internal static string Signature(Exception fault)
    {
        ArgumentNullException.ThrowIfNull(fault);
        var frames = (fault.StackTrace ?? string.Empty).Split('\n').Select(f => f.Trim()).ToList();
        var ours = frames.FirstOrDefault(f => f.StartsWith("at ChronoMock.", StringComparison.Ordinal));
        return fault.GetType().FullName + "|" + (ours ?? frames[0]);
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
        => Compose(translate, "fault.unexpected", fault, savedPath);

    /// <summary>
    /// The text of the box a failed start shows (R5/6b, D1): that it could not start, where the whole exception
    /// was written, and its message. It used to be the message alone, with nothing written anywhere. The
    /// strings dictionary may be the very thing that failed, so a key that resolves to itself means the window
    /// has no texts, and the box speaks the English it would have shown.
    /// </summary>
    internal static string StartupText(Func<string, string> translate, Exception fault, string? savedPath)
    {
        ArgumentNullException.ThrowIfNull(translate);
        var speaks = translate("fault.startup") != "fault.startup";
        return Compose(speaks ? translate : key => EnglishFallback[key], "fault.startup", fault, savedPath);
    }

    /// <summary>The startup box's words when the window has none of its own - what Strings.en.json says, held
    /// to it by a test.</summary>
    internal static readonly IReadOnlyDictionary<string, string> EnglishFallback = new Dictionary<string, string>(StringComparer.Ordinal)
    {
        ["fault.startup"] = "Chrono Mock could not start.",
        ["fault.saved"] = "The details were saved to:",
        ["fault.detail"] = "Technical detail:",
    };

    private static string Compose(Func<string, string> translate, string leadKey, Exception fault, string? savedPath)
    {
        ArgumentNullException.ThrowIfNull(translate);
        ArgumentNullException.ThrowIfNull(fault);
        var text = translate(leadKey);
        if (savedPath is { Length: > 0 })
        {
            text += "\n\n" + translate("fault.saved") + " " + savedPath;
        }

        return text + "\n\n" + translate("fault.detail") + " " + fault.Message;
    }

    /// <summary>The block written to the diagnostics folder: the whole exception, stack and inner faults
    /// included, which the box leaves out. English and stable, like every diagnostics block (data, not
    /// interface text). The last one a run writes says that later ones will not be.</summary>
    internal static string Record(Exception fault, string source, bool last = false)
    {
        ArgumentNullException.ThrowIfNull(fault);
        return "Chrono Mock unexpected fault\n"
            + "  when:   " + DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ", System.Globalization.CultureInfo.InvariantCulture) + "\n"
            + "  source: " + source + "\n"
            + (last ? $"  note:   the last of the {MaxRecorded} different faults a run writes - later ones are not written\n" : string.Empty)
            + "\n"
            + fault + "\n";
    }
}

/// <summary>What <see cref="FaultReports.Hear"/> says to do about a fault.</summary>
internal enum FaultNotice
{
    /// <summary>Heard before, or past the cap on what is written - nothing.</summary>
    None,

    /// <summary>Write it and show it.</summary>
    Show,

    /// <summary>Write it, past the cap on boxes.</summary>
    Record,

    /// <summary>Write it, and say it is the last one written.</summary>
    RecordLast,
}
