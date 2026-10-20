using System.Threading.Channels;

namespace ChronoMock.Protocol;

/// <summary>
/// One connection to a running core as the interface sees it: the commands that go to it, the events that
/// come back, and what the core wrote to its stderr (R5-D63). <see cref="CoreClient"/> is the connection every
/// build ships, a core process speaking NDJSON over its standard streams.
/// <para>
/// It is an interface for the reason <see cref="ICalcEngine"/> is one. The window's whole session - plan,
/// handshake, start, the event pump, the record in the history, the diagnostics kept when it went wrong - ran
/// only against a real core, so the only tests of that path were integration tests the CI does not run, and
/// the order of events in them was whatever the machine produced. Over this, a test chooses the order.
/// </para>
/// </summary>
public interface ICoreConnection : IAsyncDisposable
{
    /// <summary>The core's event stream. Completes when the core closes its stdout.</summary>
    ChannelReader<ChronoEvent> Events { get; }

    /// <summary>The core's stderr and the lines it wrote that were not events, kept for support (RELEASE-012).
    /// Complete once the connection is disposed.</summary>
    IReadOnlyCollection<string> Diagnostics { get; }

    /// <summary>How many of those lines were dropped to stay under the cap, so a captured block can say it is a
    /// tail rather than the whole of it (rule 6).</summary>
    int DiagnosticsDropped { get; }

    /// <summary>The core's exit code once the connection is disposed, or null.</summary>
    int? ExitCode { get; }

    /// <summary>Write one command to the core. Throws when the core is already gone.</summary>
    void Send(Command command);
}
