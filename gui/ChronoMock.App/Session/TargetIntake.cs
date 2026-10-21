using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>What became of a file offered as the target.</summary>
internal enum TargetTaken
{
    /// <summary>It is the target now - or a newer offer made this one moot, or a session started meanwhile.</summary>
    Taken,

    /// <summary>Nothing behind the name: the program, or the program a shortcut leads to, is not there.</summary>
    Missing,

    /// <summary>A shortcut that leads to no program file a session could start.</summary>
    NotAProgram,
}

/// <summary>
/// A file offered as the target - dropped on the window, or a shortcut picked in the dialog - looked at off
/// the window's thread and then taken (R4-N48, R5-D80). A program is taken when it is there. A shortcut is
/// read, and its program is taken with the arguments and working folder it starts it with.
/// </summary>
/// <remarks>
/// 🔴 THE LOOK RUNS OFF THE WINDOW'S THREAD. Asking whether a file exists on a share that has gone blocks for
/// as long as the share takes to fail - measured 21 s on an unreachable address - and a shortcut is a file
/// the shell opens. Two offers in that time are answered in the order they were MADE, not in the order their
/// looks return, the rule R4/17 gave every answer that arrives late.
///
/// Out of <see cref="SessionViewModel"/>, which stands on its coupling ceiling: it hands the model strings,
/// never a shortcut.
/// </remarks>
internal sealed class TargetIntake
{
    private readonly SessionViewModel _session;
    private readonly Func<string, ShortcutTarget?> _readShortcut;

    /// <summary>Counts offers, so only the newest one decides the target.</summary>
    private int _offers;

    public TargetIntake(SessionViewModel session)
        : this(session, ShellShortcut.Read)
    {
    }

    /// <summary>For a test, which decides what a shortcut says without making one.</summary>
    internal TargetIntake(SessionViewModel session, Func<string, ShortcutTarget?> readShortcut)
    {
        _session = session;
        _readShortcut = readShortcut;
    }

    /// <summary>Whether a file is a shortcut, by its name alone - the drag asks on every mouse move.</summary>
    public static bool IsShortcut(string path) => path.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase);

    /// <summary>The translation key that says why an offer was not taken, or empty when it was.</summary>
    public static string RefusalKey(TargetTaken taken) => taken switch
    {
        TargetTaken.Missing => "target.program_missing",
        TargetTaken.NotAProgram => "target.shortcut_rejected",
        _ => string.Empty,
    };

    /// <summary>Offer a program or a shortcut as the target.</summary>
    public Task<TargetTaken> TakeAsync(string path) => TakeAsync(Task.Run(() => Look(path)));

    /// <summary>The same, with the look handed in - so a test decides the ORDER in which two looks return,
    /// which a real check on two local files cannot promise.</summary>
    internal async Task<TargetTaken> TakeAsync(Task<Offer> look)
    {
        var mine = ++_offers;
        var offer = await look.ConfigureAwait(true);
        if (mine != _offers)
        {
            return TargetTaken.Taken;
        }

        if (offer.Answer != TargetTaken.Taken)
        {
            return offer.Answer;
        }

        // A session started while the look ran keeps the target it started with - the target is start-only.
        if (_session.IsIdle)
        {
            if (offer.Shortcut is { } link)
            {
                _session.SetTargetFromShortcut(link.Program, link.Arguments, link.WorkingFolder);
            }
            else
            {
                _session.SetTarget(offer.Program);
            }
        }

        return TargetTaken.Taken;
    }

    /// <summary>What the file is: a program that is there, a shortcut to one, or a refusal.</summary>
    internal Offer Look(string path)
    {
        if (!IsShortcut(path))
        {
            return File.Exists(path) ? Offer.OfProgram(path) : Offer.Refused(TargetTaken.Missing);
        }

        if (_readShortcut(path) is not { } link || !link.Program.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
        {
            return Offer.Refused(TargetTaken.NotAProgram);
        }

        return File.Exists(link.Program) ? Offer.OfShortcut(link) : Offer.Refused(TargetTaken.Missing);
    }

    /// <summary>The answer to one look.</summary>
    internal sealed record Offer(TargetTaken Answer, string Program, ShortcutTarget? Shortcut)
    {
        public static Offer OfProgram(string program) => new(TargetTaken.Taken, program, null);

        public static Offer OfShortcut(ShortcutTarget link) => new(TargetTaken.Taken, link.Program, link);

        public static Offer Refused(TargetTaken why) => new(why, string.Empty, null);
    }
}
