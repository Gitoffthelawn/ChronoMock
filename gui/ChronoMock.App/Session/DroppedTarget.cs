using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>
/// What a drop on the window offers as a target, judged by its SHAPE alone: exactly one file, named
/// <c>.exe</c>, or a shortcut, <c>.lnk</c>, whose program is read when it is dropped (R5-D80). Several
/// files, a folder or another format is not a target.
/// <para>
/// 🔴 By the name only, never by asking the disk (R4-N48). The window asks this on every mouse move of a
/// drag, and asking whether the file exists blocked for as long as an unreachable share takes to fail -
/// measured 21 s. Whether the file is really there is asked once, off the UI thread, when it is dropped
/// (<see cref="TargetIntake"/>).
/// </para>
/// </summary>
internal static class DroppedTarget
{
    /// <summary>The single program or shortcut a drop names, or null when it names anything else.</summary>
    internal static string? Candidate(object? dropped)
        => dropped is string[] { Length: 1 } paths
           && (paths[0].EndsWith(".exe", StringComparison.OrdinalIgnoreCase) || TargetIntake.IsShortcut(paths[0]))
            ? paths[0]
            : null;

    /// <summary>
    /// The program or shortcut the window was started with - <c>ChronoMock.exe &lt;file&gt;</c>, which is what
    /// "Open with" and a file dropped on the program's icon hand it - by the rule a drop is judged by: exactly
    /// one file, named <c>.exe</c> or <c>.lnk</c>. A relative name is read from the folder the window was
    /// started in, as a command line means it, and a name no full path can be made of is not a target.
    /// </summary>
    internal static string? FromCommandLine(IReadOnlyList<string> args, string startFolder)
    {
        if (args.Count != 1)
        {
            return null;
        }

        try
        {
            return Candidate(new[] { Path.GetFullPath(args[0], startFolder) });
        }
        catch (ArgumentException)
        {
            return null;
        }
    }
}
