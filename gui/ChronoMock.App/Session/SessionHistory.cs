using System.Globalization;
using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Text.Json;
using System.Text.Json.Serialization;

namespace ChronoMock.App;

/// <summary>Limits on the local history - it is a convenience log, not an archive.</summary>
public static class SessionHistoryLimits
{
    /// <summary>The most recent sessions kept - older ones are dropped on append.</summary>
    public const int Max = 50;
}

/// <summary>
/// Reads, appends, and prunes the local session history (docs/04 section 6). Local-only, never exported.
/// Injected into <see cref="SessionViewModel"/> so unit tests use an in-memory store and touch no files.
/// </summary>
public interface ISessionHistoryStore
{
    /// <summary>Load the recorded sessions in the order they were written (oldest first). Returns empty for
    /// a missing or corrupt file - a broken history must never crash the app or be silently deleted.</summary>
    IReadOnlyList<SessionRecord> Load();

    /// <summary>Append one session and persist, keeping only the most recent <see cref="SessionHistoryLimits.Max"/>.
    /// Throws <see cref="IOException"/> or <see cref="UnauthorizedAccessException"/> when the location cannot
    /// be written (e.g. a read-only drive) so the caller can say so out loud, never swallow it (rule 6).
    /// <para>
    /// An existing store this build cannot read is SET ASIDE, never written over: <see cref="Load"/>
    /// promises not to delete a broken history, and an append that overwrote it would have broken that
    /// promise one step later.
    /// </para></summary>
    void Append(SessionRecord record);

    /// <summary>Remove one recorded session (matched by value). Same write-failure contract as Append.</summary>
    void Remove(SessionRecord record);

    /// <summary>Remove every recorded session. Same write-failure contract as Append.</summary>
    void Clear();

    /// <summary>
    /// <see cref="Append"/> off the calling thread, answering with the empty string when it was written and
    /// the reason when it was not - the panel shows that reason rather than a dialog (rule 6).
    /// <para>
    /// 🔴 The three changes go to the pool, not only the append. Removing a row and clearing the log wrote the
    /// file on the UI thread, where the replace sleeps between retries while another instance holds the file,
    /// and where the gate between instances (R4-N49) would wait (R4/13). The panel's own list is still changed
    /// by the caller, on its thread - WPF refuses a bound collection changed from anywhere else.
    /// </para>
    /// </summary>
    Task<string> AppendAsync(SessionRecord record) => InBackground(() => Append(record));

    /// <summary><see cref="Remove"/> off the calling thread, answered like <see cref="AppendAsync"/>.</summary>
    Task<string> RemoveAsync(SessionRecord record) => InBackground(() => Remove(record));

    /// <summary><see cref="Clear"/> off the calling thread, answered like <see cref="AppendAsync"/>.</summary>
    Task<string> ClearAsync() => InBackground(Clear);

    /// <summary>
    /// The exception is carried back as a value rather than rethrown: the message belongs on the panel, and
    /// awaiting a faulted Task.Run would wrap it in an AggregateException-shaped rethrow whose type filter is
    /// easy to get subtly wrong.
    /// </summary>
    private static Task<string> InBackground(Action change) => Task.Run(() =>
    {
        try
        {
            change();
            return string.Empty;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return ex.Message;
        }
    });
}

/// <summary>In-memory store: the default for a bare view-model and for unit tests, so they touch no files.</summary>
public sealed class InMemorySessionHistoryStore : ISessionHistoryStore
{
    private readonly List<SessionRecord> _records = [];

    public IReadOnlyList<SessionRecord> Load() => [.. _records];

    public void Append(SessionRecord record)
    {
        _records.Add(record);
        if (_records.Count > SessionHistoryLimits.Max)
        {
            _records.RemoveAt(0); // drop the oldest
        }
    }

    public void Remove(SessionRecord record) => _records.Remove(record);

    public void Clear() => _records.Clear();
}

/// <summary>
/// File store: one JSON file per the portable layout (history/sessions.json next to the executable,
/// docs/04 section 7). The file wraps the records with a schema and an "unstable" marker while the shape
/// is not frozen - history is local-only, so its shape is not an exchange contract (docs/04 row 27).
/// </summary>
public sealed class FileSessionHistoryStore : ISessionHistoryStore
{
    private const int Schema = 1;
    private const string FileName = "sessions.json";

    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    private readonly string _directory;

    public FileSessionHistoryStore(string directory) => _directory = directory;

    /// <summary>The store for the running app: a history folder next to the executable (portable). When that
    /// location is read-only - a USB stick, or Program Files without admin - fall back to a per-user
    /// writable folder so the log still saves instead of every session reporting a write error.</summary>
    public static FileSessionHistoryStore ForApp()
    {
        var exeHistory = Path.Combine(AppContext.BaseDirectory, "history");
        var perUser = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ChronoMock", "history");
        return new FileSessionHistoryStore(ChooseWritableDir(exeHistory, perUser, WritableFolder.IsWritable));
    }

    /// <summary>Pick <paramref name="preferred"/> when it is writable, else <paramref name="fallback"/>. The
    /// writability check is injected so the choice is unit-tested without a real read-only medium.</summary>
    internal static string ChooseWritableDir(string preferred, string fallback, Func<string, bool> isWritable)
        => isWritable(preferred) ? preferred : fallback;

    private string FilePath => Path.Combine(_directory, FileName);

    /// <summary>Distinguishes concurrent writes from within one process, as the process id does between
    /// instances - together they make every scratch file name unique.</summary>
    private static int _tempCounter;

    /// <summary>How many times a read is tried while the file is held by somebody else - the same short
    /// budget the replace below gives the destination (20 + 40 + 60 + 80 + 100 ms between tries).</summary>
    private const int ReadAttempts = 6;

    /// <summary>
    /// The records on disk, or none. A file that cannot be opened even after retrying reads as empty and is
    /// left exactly where it is - unlike an unreadable one, nothing here sets it aside, so the next append
    /// simply tries again.
    /// </summary>
    public IReadOnlyList<SessionRecord> Load()
    {
        try
        {
            return ReadFile(out _);
        }
        catch (IOException)
        {
            return [];
        }
    }

    /// <summary>
    /// The records on disk, plus whether there IS a file here that this build could not read.
    ///
    /// Load answers only the first half, because a reader has nothing to do with the second. A WRITER does:
    /// an unreadable file is still somebody's history, and overwriting it is a deletion however quietly it
    /// happens. That is precisely what used to occur - Load returned empty, Append built its new list from
    /// that empty, and the first session after a downgrade wiped the whole log the newer build had written.
    /// The schema gate stopped this build from MISREADING the file and did nothing to stop it destroying it.
    /// <para>
    /// 🔴 A file that is there and cannot be OPENED right now is neither, and throws once the retries are
    /// spent. It used to count as unreadable, so a scanner or a second instance holding it for a moment made
    /// the next append set the whole history aside and start a new one with a single row (R4-N49).
    /// </para>
    /// </summary>
    private IReadOnlyList<SessionRecord> ReadFile(out bool unreadable)
    {
        for (var attempt = 1; ; attempt++)
        {
            try
            {
                return ReadOnce(out unreadable);
            }
            catch (IOException) when (attempt < ReadAttempts)
            {
                Thread.Sleep(20 * attempt);
            }
        }
    }

    private IReadOnlyList<SessionRecord> ReadOnce(out bool unreadable)
    {
        unreadable = false;
        string text;
        try
        {
            text = File.ReadAllText(FilePath);
        }
        catch (Exception e) when (e is FileNotFoundException or DirectoryNotFoundException)
        {
            // No history yet - asked directly rather than checked first, so a file deleted between a check
            // and the read is not mistaken for one that cannot be read.
            return [];
        }
        catch (UnauthorizedAccessException)
        {
            // A read-denied file is somebody's history this account will never read - unreadable, and so
            // set aside by a writer rather than written over.
            unreadable = true;
            return [];
        }

        try
        {
            var file = JsonSerializer.Deserialize<HistoryFile>(text, Options);
            // The schema gates the file, like the calendar and preset readers (R2-N10). The shape is marked
            // unstable, so a file written by a later build is not a history this one can read - starting
            // empty and leaving the file alone beats showing rows misread through an older shape.
            if (file is { Schema: Schema } && file.IsWhole)
            {
                return file.Sessions;
            }

            unreadable = true; // a real file, in a shape this build does not speak
            return [];
        }
        catch (JsonException)
        {
            // A corrupt history must not crash the app or be deleted - start empty and leave the file be
            // (the interface contract, rule 6).
            unreadable = true;
            return [];
        }
    }

    /// <summary>
    /// Serialises the read-modify-write below ACROSS PROCESSES.
    ///
    /// The write itself was already safe - a uniquely named scratch file and an atomic move, so the
    /// file on disk is never half-written. What was not safe is the sequence: two portable instances
    /// finishing a session at the same moment both read the same list, both appended their own record,
    /// and the second move won. Nothing failed, nothing retried, and one session was simply not in the
    /// history. The retry loop cannot see that, because both writes succeed.
    ///
    /// A named mutex is the same tool the mechanism uses for the session itself, and for the same
    /// reason: the kernel releases it when its owner dies, so a killed instance cannot wedge the log
    /// for the next one. Local\ scopes it to the session, which is where the file it protects lives.
    /// </summary>
    private const string WriteMutexName = @"Local\ChronoMock.History";

    /// <summary>How long to wait for the other instance to finish its append. History is written once
    /// at the end of a session and the whole cycle is milliseconds, so anything longer than this means
    /// something is wrong rather than busy - and the write then goes ahead anyway, because losing this
    /// record to be tidy would be the very failure this guards against.</summary>
    private static readonly TimeSpan WriteMutexWait = TimeSpan.FromSeconds(5);

    public void Append(SessionRecord record) => UnderGate(() => AppendUnderGate(record));

    /// <summary>
    /// Run one read-modify-write of the file with the other instances kept out.
    /// <para>
    /// 🔴 Removing a row and clearing the log are read-modify-writes too, and they ran outside the gate: a
    /// removal read the list, another instance appended its session, and the removal wrote its list back
    /// over that session (R4-N49). Every change to the file goes through here now.
    /// </para>
    /// </summary>
    private static void UnderGate(Action change)
    {
        using var gate = new Mutex(initiallyOwned: false, WriteMutexName);
        var held = false;
        try
        {
            // AbandonedMutexException means the previous holder died without releasing - we now hold it,
            // and the file it left behind is a complete one either way (the move is atomic).
            try
            {
                held = gate.WaitOne(WriteMutexWait);
            }
            catch (AbandonedMutexException)
            {
                held = true;
            }

            change();
        }
        finally
        {
            if (held)
            {
                gate.ReleaseMutex();
            }
        }
    }

    private void AppendUnderGate(SessionRecord record)
    {
        var existing = ReadFile(out var unreadable);
        if (unreadable)
        {
            // Set it aside under a dated name instead of writing over it (rule 7 - nothing deletes itself).
            // The ordinary way to get here is a DOWNGRADE: a newer build wrote a schema this one does not
            // read, and the tester came back to compare. Their log survives, they can see where it went,
            // and this session still gets recorded.
            SetAsideUnreadableFile();
            existing = [];
        }

        var sessions = new List<SessionRecord>(existing) { record };
        if (sessions.Count > SessionHistoryLimits.Max)
        {
            sessions.RemoveRange(0, sessions.Count - SessionHistoryLimits.Max); // drop the oldest
        }

        Write(sessions);
    }

    /// <summary>Rename the unreadable history out of the way, to a name that says what it is and cannot
    /// collide with an earlier one. A move, never a copy-then-delete: on one volume it is atomic, so there
    /// is no window in which both or neither exists.</summary>
    private void SetAsideUnreadableFile()
    {
        var stamp = DateTime.UtcNow.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture);
        var target = $"{FilePath}.unreadable-{stamp}";
        // Two sessions ending in the same second would land on one name, so the counter that already keeps
        // scratch names apart does the same job here.
        if (File.Exists(target))
        {
            target = $"{target}.{Interlocked.Increment(ref _tempCounter)}";
        }

        File.Move(FilePath, target);
    }

    public void Remove(SessionRecord record) => UnderGate(() =>
    {
        // A history this build cannot read has nothing in it this build could have shown, so there is
        // nothing to remove - and writing the remainder would write over it.
        var sessions = new List<SessionRecord>(ReadFile(out var unreadable));
        if (!unreadable && sessions.Remove(record))
        {
            Write(sessions);
        }
    });

    public void Clear() => UnderGate(() =>
    {
        if (File.Exists(FilePath))
        {
            File.Delete(FilePath);
        }
    });

    private void Write(IReadOnlyList<SessionRecord> sessions)
    {
        Directory.CreateDirectory(_directory); // no-op when it exists; throws only on a real failure
        var file = new HistoryFile { Schema = Schema, Stability = "unstable", Sessions = sessions };
        var json = JsonSerializer.Serialize(file, Options);

        // Write to a sibling temp file, then move it into place (L-13). A crash mid-write then leaves the
        // PREVIOUS history intact rather than a truncated file that the next Load reads as empty and the
        // next Append overwrites - losing the whole log, not just the in-flight entry. File.Move(overwrite)
        // is atomic on one volume, and the temp is beside the target so it always is.
        // The scratch name carries this process's id and a counter, because a fixed one collides between
        // two portable instances writing at once: both open the same path and one fails, losing its entry.
        var temp = $"{FilePath}.{Environment.ProcessId}.{Interlocked.Increment(ref _tempCounter)}.tmp";
        try
        {
            File.WriteAllText(temp, json);
            MoveWithRetry(temp, FilePath);
        }
        catch
        {
            // Never leave the scratch file behind on a failed write - the caller surfaces the error.
            try
            {
                File.Delete(temp);
            }
            catch (IOException)
            {
                // Nothing more to do: it is a temp file in a directory we may not be able to write.
            }

            throw;
        }
    }

    /// <summary>Replace the history file, retrying briefly while the destination is momentarily locked.
    /// A unique scratch name is not enough on its own: the replace itself contends, and Windows answers a
    /// simultaneous move onto the same destination with a sharing violation. History is written once at the
    /// end of a session, so a few short retries cover the overlap - if it still will not go, the error is
    /// reported rather than swallowed.</summary>
    private static void MoveWithRetry(string temp, string destination)
    {
        const int attempts = 6;
        for (var i = 1; ; i++)
        {
            try
            {
                File.Move(temp, destination, overwrite: true);
                return;
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException && i < attempts)
            {
                Thread.Sleep(20 * i);
            }
        }
    }

    private sealed record HistoryFile
    {
        [JsonPropertyName("schema")] public int Schema { get; init; }

        [JsonPropertyName("stability")] public string Stability { get; init; } = "unstable";

        [JsonPropertyName("sessions")] public IReadOnlyList<SessionRecord> Sessions { get; init; } = [];

        /// <summary>
        /// Whether the list and every row in it are actually there. JSON can say <c>null</c> where the type
        /// says a value always is, and the reader takes it: <c>"sessions": null</c> stopped the application
        /// at start, and <c>[null]</c> or a row with a null text failed later, where that row was drawn or
        /// repeated (R4-N47). Such a file is treated like any other this build cannot read.
        /// </summary>
        [JsonIgnore] public bool IsWhole => Sessions is not null && Sessions.All(row => row is { IsWhole: true });
    }
}
