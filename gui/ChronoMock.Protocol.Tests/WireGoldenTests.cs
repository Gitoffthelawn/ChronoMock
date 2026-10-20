using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace ChronoMock.Protocol.Tests;

/// <summary>
/// The window reads every field the core sends (R5-D64). <c>tests/wire/events.ndjson</c> is written by the
/// core's own test (<c>crates/proto/tests/wire_golden.rs</c>): one event of each kind, every field present.
/// Read here with unknown members refused, so a field or an event the core starts sending and nobody taught
/// the window turns this red - the window's parser ignores what it does not know (docs/08 section 2), which
/// is the right thing for a running session and the reason a forgotten field was invisible until now.
/// </summary>
public class WireGoldenTests
{
    /// <summary>
    /// What the window knowingly does not read today, each with why and what makes it go. Checked from both
    /// ends: a gap the golden file no longer shows has to come out. Only shrinks.
    /// </summary>
    private static readonly (string Where, string Why)[] KnownUnread = [];

    private static string[] GoldenLines()
        => File.ReadAllLines(Path.Combine(RepoPaths.RepoRoot(), "tests", "wire", "events.ndjson"));

    [Fact]
    public void Every_field_of_every_event_the_core_sends_has_a_place_in_the_window()
    {
        var strict = new JsonSerializerOptions(ProtocolJson.Options)
        {
            UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
        };
        var lines = GoldenLines();
        // The canary: a scan over an empty or misplaced file would pass for every event it never read.
        Assert.True(lines.Length >= 10, $"read {lines.Length} events - is tests/wire/events.ndjson the core's file?");

        var unread = new List<string>();
        foreach (var line in lines)
        {
            var node = JsonNode.Parse(line)!.AsObject();
            var type = (string)node["type"]!;
            if (EventParser.Parse(line) is not { } parsed)
            {
                unread.Add(type);
                continue;
            }

            node.Remove("type"); // the discriminator, read by the parser rather than by the record
            foreach (var (where, _) in KnownUnread.Where(k => k.Where.StartsWith(type + ".", StringComparison.Ordinal)))
            {
                if (node.Remove(where[(type.Length + 1)..]))
                {
                    unread.Add(where);
                }
            }

            var refused = Record.Exception(() => JsonSerializer.Deserialize(node.ToJsonString(), parsed.GetType(), strict));
            Assert.True(
                refused is null,
                $"the window has no field for part of a '{type}' event - teach its record, or list it in KnownUnread "
                + $"with why: {refused?.Message}");
        }

        Assert.Equal(KnownUnread.Select(k => k.Where).Order(StringComparer.Ordinal), unread.Order(StringComparer.Ordinal));
    }
}
