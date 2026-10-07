using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>
/// Where this application's own files go, and whether a folder can take them - the one question the history
/// store and the diagnostics log both ask before choosing between the folder beside the executable and the
/// per-user one. Each of them carried its own copy of the probe and of the choice, and both copies had the
/// same fault, so they live here once.
/// </summary>
internal static class WritableFolder
{
    /// <summary>Keeps two probes of one process apart, as the process id keeps two instances apart.</summary>
    private static int _probeCounter;

    /// <summary>The folder the running window keeps one kind of its own files in (<c>history</c>, <c>logs</c>).</summary>
    internal static string ForApp(string name) => Choose(
        name,
        AppContext.BaseDirectory,
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        AppPaths.IsInstalled,
        IsWritable);

    /// <summary>
    /// Where one kind of the window's own files goes. A portable copy keeps them beside the executable when
    /// that folder takes a write (a USB stick or a read-only share does not), and in the per-user folder
    /// otherwise. An installed copy keeps them in the per-user folder, always.
    /// <para>
    /// 🔴 Installed means the installer owns the folder. Asked like a portable copy, an installed window
    /// running as administrator found Program Files writable and kept its history there - a second history
    /// beside the one the same person's ordinary window keeps, and a folder the uninstaller leaves behind,
    /// since it removes only what it installed. And the question itself writes: the probe creates the folder
    /// it asks about. So an installed copy never asks.
    /// </para>
    /// </summary>
    internal static string Choose(
        string name, string exeDir, string perUserRoot, bool installed, Func<string, bool> isWritable)
    {
        var perUser = Path.Combine(perUserRoot, "ChronoMock", name);
        if (installed)
        {
            return perUser;
        }

        var beside = Path.Combine(exeDir, name);
        return isWritable(beside) ? beside : perUser;
    }

    /// <summary>
    /// Whether the folder can be created and written to, by actually writing (a folder's read-only attribute
    /// does not stop file creation on Windows, so a real write is the only honest test).
    /// <para>
    /// 🔴 The probe used to have one fixed name. Two instances starting together could meet on it - one
    /// holding it open while the other tried to write it - and the loser decided the folder was read-only and
    /// took the per-user one, so the two kept their history in two places (R4-N49). The name is unique now,
    /// and the file deletes itself when it is closed, so even a process killed between the write and the
    /// delete leaves nothing behind.
    /// </para>
    /// </summary>
    internal static bool IsWritable(string dir)
    {
        try
        {
            Directory.CreateDirectory(dir);
            var probe = Path.Combine(
                dir, $".write-probe-{Environment.ProcessId}-{Interlocked.Increment(ref _probeCounter)}");
            using (new FileStream(probe, FileMode.CreateNew, FileAccess.Write, FileShare.None, 1,
                       FileOptions.DeleteOnClose))
            {
            }

            return true;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }
}
