namespace ChronoMock.App.Tests;

/// <summary>
/// A history store that has just moved an unreadable history aside (R5-D73): it says where, once, and answers
/// every change at once, so a test can record a session on the window's own thread without waiting for a
/// pool thread to come back to it.
/// </summary>
internal sealed class SetAsideHistoryStore(string setAsidePath) : ISessionHistoryStore
{
    private readonly InMemorySessionHistoryStore _records = new();
    private string _setAside = setAsidePath;

    public IReadOnlyList<SessionRecord> Load() => _records.Load();

    public void Append(SessionRecord record) => _records.Append(record);

    public void Remove(SessionRecord record) => _records.Remove(record);

    public void Clear() => _records.Clear();

    public Task<string> AppendAsync(SessionRecord record)
    {
        Append(record);
        return Task.FromResult(string.Empty);
    }

    public string TakeSetAside()
    {
        var path = _setAside;
        _setAside = string.Empty;
        return path;
    }
}
