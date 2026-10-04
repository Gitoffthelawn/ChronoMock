using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// The local session history store (docs/04 section 6): append and load round-trip, a missing file is
/// empty, and a corrupt file is empty but never deleted (a broken history must not crash or be lost).
/// </summary>
public sealed class SessionHistoryStoreTests : IDisposable
{
    private readonly string _dir =
        Path.Combine(Path.GetTempPath(), "chrono-hist-tests", Guid.NewGuid().ToString("N"));

    public void Dispose()
    {
        if (Directory.Exists(_dir))
        {
            Directory.Delete(_dir, recursive: true);
        }
    }

    private static SessionRecord Record(string name, string verdict = "works") => new()
    {
        TargetPath = $@"C:\apps\{name}.exe",
        MomentLocal = "2038-01-19T03:14:07",
        TzBiasMin = -120,
        Mode = "multiplier",
        Multiplier = 60,
        Verdict = verdict,
        EndedAtUtc = "2026-08-25T09:00:00Z",
    };

    /// <summary>
    /// S-33 regression. The scratch file used one fixed name, so two portable instances writing at the
    /// same moment collided: one threw an IOException that surfaced as a history error and lost its entry.
    /// Concurrent appends must all complete, and leave no scratch files behind.
    /// </summary>
    [Fact]
    public async Task Concurrent_appends_do_not_collide_on_the_scratch_file()
    {
        Directory.CreateDirectory(_dir);
        var writers = Enumerable.Range(0, 8)
            .Select(i => Task.Run(() => new FileSessionHistoryStore(_dir).Append(Record($"app{i}"))))
            .ToArray();

        await Task.WhenAll(writers); // a collision would surface here as an IOException

        Assert.NotEmpty(new FileSessionHistoryStore(_dir).Load());
        Assert.Empty(Directory.GetFiles(_dir, "*.tmp"));
    }

    /// <summary>
    /// 🔴 Every concurrent append has to SURVIVE, not merely complete.
    ///
    /// The test above asserts that nothing throws and the file is not empty, and both were already
    /// true of the broken version: the write is atomic, so the file is never half-written, but the
    /// read-modify-write around it was not serialised. Two instances read the same list, both appended
    /// their own record, and the second move won - nothing failed, nothing retried, and one session
    /// was simply not in the history. A test that counts is what tells those two states apart.
    /// </summary>
    [Fact]
    public async Task Every_concurrent_append_survives_rather_than_the_last_one_winning()
    {
        Directory.CreateDirectory(_dir);
        const int writers = 8;
        var tasks = Enumerable.Range(0, writers)
            .Select(i => Task.Run(() => new FileSessionHistoryStore(_dir).Append(Record($"app{i}"))))
            .ToArray();

        await Task.WhenAll(tasks);

        var loaded = new FileSessionHistoryStore(_dir).Load();
        Assert.Equal(writers, loaded.Count);
        for (var i = 0; i < writers; i++)
        {
            Assert.Contains(loaded, r => r.TargetName == $"app{i}.exe");
        }
    }

    [Fact]
    public void Append_then_load_round_trips_the_records_in_order()
    {
        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("Alpha"));
        store.Append(Record("Beta", "partial"));

        var loaded = store.Load();

        Assert.Equal(2, loaded.Count);
        Assert.Equal("Alpha.exe", loaded[0].TargetName); // oldest first, as written
        Assert.Equal("Beta.exe", loaded[1].TargetName);
        Assert.Equal("partial", loaded[1].Verdict);
        Assert.Equal(60, loaded[1].Multiplier);
        Assert.Equal(-120, loaded[1].TzBiasMin);
    }

    [Fact]
    public void Load_is_empty_when_no_file_exists()
        => Assert.Empty(new FileSessionHistoryStore(_dir).Load());

    [Fact]
    public void Load_is_empty_and_keeps_the_file_when_it_is_corrupt()
    {
        Directory.CreateDirectory(_dir);
        var path = Path.Combine(_dir, "sessions.json");
        File.WriteAllText(path, "{ this is not valid json");

        var loaded = new FileSessionHistoryStore(_dir).Load();

        Assert.Empty(loaded);
        Assert.True(File.Exists(path)); // a broken history is never deleted
    }

    /// <summary>
    /// Load promises not to delete a broken history, and it kept that promise - Append then broke it one
    /// step later. It built its new list from Load's empty answer and wrote over the file, so the first
    /// session recorded after a DOWNGRADE (a newer build's schema this one will not read) wiped the whole
    /// log. The schema gate stopped this build from misreading the file and did nothing to stop it
    /// destroying it.
    /// </summary>
    [Fact]
    public void Append_sets_an_unreadable_history_aside_instead_of_writing_over_it()
    {
        Directory.CreateDirectory(_dir);
        var path = Path.Combine(_dir, "sessions.json");
        const string fromALaterBuild =
            "{\"schema\":2,\"stability\":\"unstable\",\"sessions\":[{\"target_path\":\"Ledger.exe\","
            + "\"moment_local\":\"2038-01-19T03:14:07\",\"tz_bias_min\":0,\"mode\":\"flow\","
            + "\"verdict\":\"works\",\"ended_at_utc\":\"2026-09-03T10:00:00Z\"}]}";
        File.WriteAllText(path, fromALaterBuild);

        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("Ledger"));

        // The new session is recorded...
        Assert.Equal("Ledger.exe", Assert.Single(store.Load()).TargetName);

        // ...and the log it could not read is still on disk, under a name that says what it is.
        var setAside = Directory.GetFiles(_dir, "sessions.json.unreadable-*");
        Assert.Single(setAside);
        Assert.Equal(fromALaterBuild, File.ReadAllText(setAside[0]));
    }

    /// <summary>The same for a file that is not JSON at all - the other way Load answers "not mine".</summary>
    [Fact]
    public void Append_sets_a_corrupt_history_aside_too()
    {
        Directory.CreateDirectory(_dir);
        File.WriteAllText(Path.Combine(_dir, "sessions.json"), "{ this is not valid json");

        new FileSessionHistoryStore(_dir).Append(Record("Ledger"));

        Assert.Single(Directory.GetFiles(_dir, "sessions.json.unreadable-*"));
    }

    /// <summary>Setting aside happens ONCE: the second append finds an ordinary file and simply extends it,
    /// rather than shuffling a fresh copy aside on every session for the rest of the install's life.</summary>
    [Fact]
    public void An_unreadable_history_is_set_aside_once_not_on_every_append()
    {
        Directory.CreateDirectory(_dir);
        File.WriteAllText(Path.Combine(_dir, "sessions.json"), "{ this is not valid json");

        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("First"));
        store.Append(Record("Second"));

        Assert.Single(Directory.GetFiles(_dir, "sessions.json.unreadable-*"));
        Assert.Equal(2, store.Load().Count); // both sessions are in the new log
    }

    [Fact]
    public void Load_is_empty_and_keeps_the_file_when_the_schema_is_not_this_one()
    {
        // R2-N10: the store writes a schema and an "unstable" marker and then never looked at either on
        // read, unlike the calendar and preset readers. A file from a later build would have been rendered
        // through this build's shape - whatever survived deserialization, shown as history.
        Directory.CreateDirectory(_dir);
        var path = Path.Combine(_dir, "sessions.json");
        File.WriteAllText(
            path,
            "{\"schema\":2,\"stability\":\"unstable\",\"sessions\":[{\"target_path\":\"Ledger.exe\","
            + "\"moment_local\":\"2038-01-19T03:14:07\",\"tz_bias_min\":0,\"mode\":\"flow\","
            + "\"verdict\":\"works\",\"ended_at_utc\":\"2026-09-03T10:00:00Z\"}]}");

        var loaded = new FileSessionHistoryStore(_dir).Load();

        Assert.Empty(loaded);
        Assert.True(File.Exists(path)); // a history this build cannot read is never deleted
    }

    [Fact]
    public void In_memory_store_appends_and_loads()
    {
        var store = new InMemorySessionHistoryStore();
        store.Append(Record("Gamma"));
        Assert.Equal("Gamma.exe", Assert.Single(store.Load()).TargetName);
    }

    [Fact]
    public void Clear_empties_the_file_store()
    {
        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("Alpha"));
        store.Append(Record("Beta"));

        store.Clear();

        Assert.Empty(store.Load());
    }

    [Fact]
    public void Remove_deletes_the_matching_record()
    {
        var store = new FileSessionHistoryStore(_dir);
        var alpha = Record("Alpha");
        store.Append(alpha);
        store.Append(Record("Beta"));

        store.Remove(alpha);

        Assert.Equal("Beta.exe", Assert.Single(store.Load()).TargetName);
    }

    [Fact]
    public void Chooses_the_preferred_history_directory_when_it_is_writable()
        => Assert.Equal(
            @"X:\exe\history",
            FileSessionHistoryStore.ChooseWritableDir(@"X:\exe\history", @"Y:\user\history", _ => true));

    [Fact]
    public void Falls_back_to_the_per_user_directory_when_the_preferred_is_read_only()
        // A read-only medium (a USB stick, Program Files without admin) cannot hold the log next to the exe,
        // so history saves to a per-user location instead of failing every session.
        => Assert.Equal(
            @"Y:\user\history",
            FileSessionHistoryStore.ChooseWritableDir(@"X:\exe\history", @"Y:\user\history", _ => false));

    [Fact]
    public void Append_keeps_only_the_most_recent_maximum()
    {
        var store = new FileSessionHistoryStore(_dir);
        for (int i = 0; i < SessionHistoryLimits.Max + 3; i++)
        {
            store.Append(Record($"App{i}"));
        }

        var loaded = store.Load();

        Assert.Equal(SessionHistoryLimits.Max, loaded.Count);
        Assert.Equal("App3.exe", loaded[0].TargetName); // App0..App2 dropped as oldest
        Assert.Equal($"App{SessionHistoryLimits.Max + 2}.exe", loaded[^1].TargetName); // newest kept
    }

    /// <summary>
    /// R4-N47: JSON can say null where the shape says a value always is, and the reader took it. A null list
    /// stopped the application at start ("startup failed"), a null row or a null text failed later, where the
    /// row was drawn or repeated. Each is now a file this build cannot read: empty on load, kept on disk, and
    /// set aside rather than written over by the next session.
    /// </summary>
    [Theory]
    [InlineData("null")]
    [InlineData("[null]")]
    [InlineData("[{\"target_path\":null,\"moment_local\":\"2038-01-19T03:14:07\",\"tz_bias_min\":0,"
                + "\"mode\":\"flow\",\"verdict\":\"works\",\"ended_at_utc\":\"2026-09-03T10:00:00Z\"}]")]
    [InlineData("[{\"target_path\":\"a.exe\",\"moment_local\":\"2038-01-19T03:14:07\",\"tz_bias_min\":0,"
                + "\"mode\":\"flow\",\"verdict\":\"works\",\"ended_at_utc\":\"2026-09-03T10:00:00Z\","
                + "\"target_args\":null}]")]
    public void A_history_with_a_null_where_a_value_belongs_is_unreadable_not_fatal(string sessions)
    {
        Directory.CreateDirectory(_dir);
        var path = Path.Combine(_dir, "sessions.json");
        var content = "{\"schema\":1,\"stability\":\"unstable\",\"sessions\":" + sessions + "}";
        File.WriteAllText(path, content);
        var store = new FileSessionHistoryStore(_dir);

        Assert.Empty(store.Load());
        Assert.Equal(content, File.ReadAllText(path));

        store.Append(Record("After"));

        Assert.Equal("After.exe", Assert.Single(store.Load()).TargetName);
        var setAside = Assert.Single(Directory.GetFiles(_dir, "sessions.json.unreadable-*"));
        Assert.Equal(content, File.ReadAllText(setAside));
    }

    /// <summary>
    /// R4-N49: a file held for a moment by somebody else - a scanner, the other instance's replace - read as
    /// unreadable, so the next session set the WHOLE history aside and started a new one with a single row.
    /// It is waited for now, briefly, like the replace already was.
    /// </summary>
    [Fact]
    public async Task A_history_held_for_a_moment_is_waited_for_not_set_aside()
    {
        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("First"));
        // Shared for delete and nothing else: a read fails while a rename goes through, which is exactly the
        // combination that let the old code set a perfectly good history aside.
        var held = new FileStream(Path.Combine(_dir, "sessions.json"), FileMode.Open, FileAccess.Read, FileShare.Delete);
        var release = Task.Run(
            async () =>
            {
                await Task.Delay(100, TestContext.Current.CancellationToken);
                await held.DisposeAsync();
            },
            TestContext.Current.CancellationToken);

        store.Append(Record("Second"));
        await release;

        Assert.Equal(["First.exe", "Second.exe"], store.Load().Select(r => r.TargetName));
        Assert.Empty(Directory.GetFiles(_dir, "sessions.json.unreadable-*"));
    }

    /// <summary>The other side of the same wait: held for longer than it lasts, the append fails out loud (the
    /// panel says the session was not recorded) and the history stays exactly where it was.</summary>
    [Fact]
    public void A_history_held_past_the_wait_is_reported_and_left_where_it_is()
    {
        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("First"));
        var path = Path.Combine(_dir, "sessions.json");

        using (new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Delete))
        {
            Assert.ThrowsAny<IOException>(() => store.Append(Record("Second")));
        }

        Assert.Equal("First.exe", Assert.Single(store.Load()).TargetName);
        Assert.Empty(Directory.GetFiles(_dir, "sessions.json.unreadable-*"));
    }

    /// <summary>
    /// R4-N49: removing a row is a read-modify-write, and it ran outside the gate that keeps instances apart -
    /// so it could write its list back over a session another instance had just appended. It waits for the
    /// gate now, like an append.
    /// </summary>
    [Fact]
    public async Task Removing_a_row_waits_for_another_instance_that_is_writing()
    {
        var store = new FileSessionHistoryStore(_dir);
        store.Append(Record("Kept"));
        store.Append(Record("Gone"));
        using var taken = new ManualResetEventSlim();
        using var letGo = new ManualResetEventSlim();
        var other = new Thread(() =>
        {
            using var gate = new Mutex(initiallyOwned: false, @"Local\ChronoMock.History");
            gate.WaitOne();
            taken.Set();
            letGo.Wait();
            gate.ReleaseMutex();
        });
        other.Start();
        taken.Wait(TestContext.Current.CancellationToken);

        var removal = Task.Run(() => store.Remove(Record("Gone")), TestContext.Current.CancellationToken);
        await Task.Delay(300, TestContext.Current.CancellationToken);
        var waited = !removal.IsCompleted;
        letGo.Set();
        await removal;
        other.Join();

        Assert.True(waited, "the removal must wait for the instance that holds the history");
        Assert.Equal("Kept.exe", Assert.Single(store.Load()).TargetName);
    }
}
