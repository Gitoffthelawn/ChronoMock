using System.Threading.Channels;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A core that answers in the order a test chooses (R5-D63): it says what <c>greeting</c> holds as soon as it
/// is connected, answers each command with what <c>answer</c> returns for it, and ends its stream when told
/// to or when it is let go. What the session sent it and whether it was let go are kept for the test to read.
/// </summary>
internal sealed class FakeCoreConnection : ICoreConnection
{
    private readonly Channel<ChronoEvent> _events = Channel.CreateUnbounded<ChronoEvent>();
    private readonly Func<Command, IEnumerable<ChronoEvent>> _answer;
    private readonly Lock _gate = new();
    private readonly List<Command> _sent = [];

    internal FakeCoreConnection(
        IEnumerable<ChronoEvent> greeting, Func<Command, IEnumerable<ChronoEvent>>? answer = null, params string[] stderr)
    {
        _answer = answer ?? (_ => []);
        Diagnostics = stderr;
        foreach (var evt in greeting)
        {
            _events.Writer.TryWrite(evt);
        }
    }

    /// <summary>The <c>ready</c> a core built for this machine's own bitness sends first.</summary>
    internal static ReadyEvent Ready => new()
    {
        V = ProtocolJson.ProtocolVersion,
        Protocol = ProtocolJson.ProtocolVersion,
        CoreVersion = "0.0.0-test",
        Bitness = Environment.Is64BitProcess ? "x64" : "x86",
    };

    /// <summary>The path the session asked to connect to, once it did.</summary>
    internal string? ConnectedTo { get; private set; }

    internal bool LetGo { get; private set; }

    internal IReadOnlyList<Command> Sent
    {
        get
        {
            lock (_gate)
            {
                return [.. _sent];
            }
        }
    }

    public ChannelReader<ChronoEvent> Events => _events.Reader;

    public IReadOnlyCollection<string> Diagnostics { get; }

    public int DiagnosticsDropped => 0;

    public int? ExitCode => LetGo ? 0 : null;

    /// <summary>The factory a view model takes, recording which core it was asked to connect to.</summary>
    internal Func<string, ICoreConnection> Connector => path =>
    {
        ConnectedTo = path;
        return this;
    };

    public void Send(Command command)
    {
        lock (_gate)
        {
            _sent.Add(command);
        }

        foreach (var evt in _answer(command))
        {
            _events.Writer.TryWrite(evt);
        }
    }

    /// <summary>The core closes its stdout, as it does once it has exited.</summary>
    internal void End() => _events.Writer.TryComplete();

    public ValueTask DisposeAsync()
    {
        LetGo = true;
        End();
        return ValueTask.CompletedTask;
    }
}
