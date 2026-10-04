using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>
/// Whether a folder can take this application's files - the one question the history store and the
/// diagnostics log both ask before choosing between the folder beside the executable and the per-user one.
/// Each of them carried its own copy of the probe, and both copies had the same fault, so it lives here once.
/// </summary>
internal static class WritableFolder
{
    /// <summary>Keeps two probes of one process apart, as the process id keeps two instances apart.</summary>
    private static int _probeCounter;

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
