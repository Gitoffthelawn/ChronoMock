using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>
/// Where this application's own files go - the one answer the history store and the diagnostics log both
/// take, so the two cannot end up in different places.
/// <para>
/// 🔴 Always the per-user folder, in every copy (R5-D4, R5-D72). A portable copy used to keep its history and
/// logs beside the executable whenever that folder took a write, and a copy on a shared folder or a stick
/// handed round then kept everybody's sessions - the applications they tested, with the arguments they
/// started them with - in one file anyone with the folder could read. The per-user folder is readable by
/// that user only. An installed copy already kept them there, because the installer owns its folder. Only
/// <see cref="Variable"/>, set by whoever starts the window, puts them anywhere else.
/// </para>
/// <para>
/// The question "can this folder take a write" went with the choice: it wrote a probe file to ask, and two
/// instances asking at once could disagree (R4-N49).
/// </para>
/// </summary>
internal static class WritableFolder
{
    /// <summary>
    /// The environment variable that moves the window's own folder. A full path there holds <c>history</c> and
    /// <c>logs</c> in place of <c>%LOCALAPPDATA%\ChronoMock</c>, for a run that must not touch the history of the
    /// account it runs under - a release check, the window's own tests. Windows answers the per-user folder from
    /// the account's settings, not from <c>LOCALAPPDATA</c>, so that variable cannot move it.
    /// </summary>
    internal const string Variable = "CHRONOMOCK_DATA_DIR";

    /// <summary>The folder the running window keeps one kind of its own files in (<c>history</c>, <c>logs</c>),
    /// read from <see cref="Variable"/> at every call.</summary>
    internal static string ForApp(string name) => ForApp(name, Environment.GetEnvironmentVariable(Variable));

    /// <summary>
    /// The folder for one kind of the window's own files when <see cref="Variable"/> holds
    /// <paramref name="fromVariable"/> - separate so a test can ask it without changing the environment the
    /// whole test run shares. Unset, it is the per-user folder, which Windows is asked to create when the
    /// account has none yet.
    /// <para>
    /// 🔴 A value that is not a full path is refused, never read against the current folder: a relative one
    /// would put every history and log in whatever folder the window was started from. The startup box says
    /// which value it was.
    /// </para>
    /// <para>
    /// 🔴 So is a full path with a character no folder name can have (review round on #145). .NET has not checked
    /// path characters since Core 2.1, so <c>Y:\data|bad</c> passes both calls here, the window comes up, its
    /// logs fail in silence and the first history save fails without a word about the variable.
    /// </para>
    /// </summary>
    internal static string ForApp(string name, string? fromVariable)
    {
        if (string.IsNullOrEmpty(fromVariable))
        {
            var perUserRoot = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData, Environment.SpecialFolderOption.Create);
            return In(perUserRoot, name);
        }

        if (!Path.IsPathFullyQualified(fromVariable))
        {
            throw Refused(fromVariable, "is not a full path");
        }

        var full = Path.GetFullPath(fromVariable);
        var root = Path.GetPathRoot(full) ?? string.Empty;
        if (full[root.Length..].Split(Path.DirectorySeparatorChar).Any(folder => folder.IndexOfAny(NotInAFolderName) >= 0)
            || root.IndexOfAny(NotInARoot) >= 0)
        {
            throw Refused(fromVariable, "holds a character no folder name can have");
        }

        return Path.Combine(full, name);
    }

    private static readonly char[] NotInAFolderName = Path.GetInvalidFileNameChars();

    /// <summary>What may not stand in a drive or share either - the separators and the drive's colon may.</summary>
    private static readonly char[] NotInARoot = ['"', '<', '>', '|', '*', '?'];

    private static InvalidOperationException Refused(string value, string why) => new(
        $"{Variable} is set to \"{value}\", which {why}, so Chrono Mock does not know where to keep its history and logs.");

    /// <summary>
    /// The folder for one kind of the window's own files under a per-user root - separate so a test can ask it
    /// of a root it made.
    /// <para>
    /// 🔴 An empty root is refused. Windows answers an empty path for an account it gives no local application
    /// data folder, and a path built on it is relative - every history and log would then be written into
    /// whatever folder the window was started from, outside both places README and SECURITY name.
    /// </para>
    /// </summary>
    internal static string In(string perUserRoot, string name)
    {
        if (string.IsNullOrWhiteSpace(perUserRoot))
        {
            throw new InvalidOperationException(
                "Windows gives this account no local application data folder, so Chrono Mock has nowhere of its own to keep its history and logs.");
        }

        return Path.Combine(perUserRoot, "ChronoMock", name);
    }
}
