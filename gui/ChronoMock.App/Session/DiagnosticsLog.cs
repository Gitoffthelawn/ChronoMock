using System.Globalization;
using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>Limit on the diagnostics logs kept beside the app - a support aid, not an archive.</summary>
public static class DiagnosticsLogLimits
{
    /// <summary>The most recent diagnostics files kept - older ones are pruned on write.</summary>
    public const int Max = 50;
}

/// <summary>
/// Persists a diagnostics block when a session ends in anything other than a clean success (RELEASE-012),
/// so a QA report has a file to attach when an injection is blocked (Defender/AV), a hook is missing, or a
/// target vanishes. Injected into <see cref="SessionViewModel"/> so unit tests use a no-op and touch no
/// files. The block is composed by the view model (BuildDiagnosticsBlock) - this only writes it.
/// </summary>
public interface IDiagnosticsLog
{
    /// <summary>Persist one diagnostics block and return the file path written, or null when it could not be
    /// saved (a read-only medium). Never throws - the in-memory copy behind the Copy-diagnostics button is
    /// the reliable path, and the file is a best-effort convenience.</summary>
    string? Save(string content);
}

/// <summary>No-op log: the default for a bare view-model and for unit tests, so they write no files.</summary>
public sealed class NoOpDiagnosticsLog : IDiagnosticsLog
{
    public string? Save(string content) => null;
}

/// <summary>
/// File log: writes one timestamped file per problem session into a logs/ folder beside the executable
/// (portable layout), falling back to a per-user writable folder when the exe folder is read-only, and
/// always in the per-user folder when the installer put the copy there - the same choice the history store
/// makes. Best-effort: a write
/// failure returns null rather than throwing, because the diagnostics are also kept in memory for the
/// Copy-diagnostics button.
/// </summary>
public sealed class FileDiagnosticsLog : IDiagnosticsLog
{
    private readonly string _directory;

    internal FileDiagnosticsLog(string directory) => _directory = directory;

    /// <summary>The log for the running app: a logs/ folder next to the executable of a portable copy, or a
    /// per-user folder when that is read-only and always for an installed copy (the same choice as
    /// <see cref="FileSessionHistoryStore.ForApp"/>, made in <see cref="WritableFolder.Choose"/>).</summary>
    public static FileDiagnosticsLog ForApp() => new(WritableFolder.ForApp("logs"));

    /// <summary>How many names one save tries before it gives up - the stamp, then the stamp with a suffix.</summary>
    private const int NameAttempts = 10;

    public string? Save(string content) => Save(content, DateTime.UtcNow);

    /// <summary>The save at a given moment - the clock is a parameter only so a test can make two saves
    /// land on one stamp, which is the collision two instances produce.</summary>
    internal string? Save(string content, DateTime utcNow)
    {
        try
        {
            Directory.CreateDirectory(_directory);
            // Sortable, filename-safe timestamp (no ':') - milliseconds keep two fast failures from colliding.
            var stamp = utcNow.ToString("yyyyMMddTHHmmssfffZ", CultureInfo.InvariantCulture);
            var path = WriteNew(stamp, content);
            Prune();
            return path;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return null; // best-effort: the in-memory copy behind the button still stands (rule 6 met there)
        }
    }

    /// <summary>
    /// Write the block under a name nobody else holds. The stamp alone kept two failures of ONE instance
    /// apart, and two instances failing in the same millisecond wrote the same file, the second over the
    /// first (R4/13). Created new, never opened over - a name already taken gets a suffix, which sorts beside
    /// its own stamp and among no other, so pruning still drops the oldest first.
    /// </summary>
    private string WriteNew(string stamp, string content)
    {
        for (var attempt = 0; ; attempt++)
        {
            var name = attempt == 0 ? $"diagnostics-{stamp}.log" : $"diagnostics-{stamp}-{attempt}.log";
            var path = Path.Combine(_directory, name);
            try
            {
                using var file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
                using var writer = new StreamWriter(file);
                writer.Write(content);
                return path;
            }
            catch (IOException) when (File.Exists(path) && attempt < NameAttempts)
            {
                // Taken - by the other instance, or a moment ago by this one. Try the next name.
            }
        }
    }

    // Keep only the most recent files so the folder does not grow without bound. Housekeeping only - a
    // failure here must not undo the save that already succeeded.
    private void Prune()
    {
        try
        {
            var files = Directory.GetFiles(_directory, "diagnostics-*.log");
            if (files.Length <= DiagnosticsLogLimits.Max)
            {
                return;
            }

            Array.Sort(files, StringComparer.Ordinal); // the ISO stamp sorts oldest first
            foreach (var stale in files.Take(files.Length - DiagnosticsLogLimits.Max))
            {
                File.Delete(stale);
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            // A prune failure is ignored on purpose - the diagnostics file was already written.
        }
    }
}
