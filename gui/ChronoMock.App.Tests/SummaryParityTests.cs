using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The copied summary says everything the CLI report says (R5-D64). The core's test records one native
/// session in <c>tests/wire/session.ndjson</c> and the CLI report of it in <c>tests/wire/session-report.txt</c>
/// (<c>crates/cli/src/run/collect.rs</c>). Here the same events go into the session panel, and every section
/// heading of that report needs its line in the summary - a section added to the report and never to the
/// window turns this red, where before it was found by a reader comparing the two by hand, or not at all.
/// </summary>
public class SummaryParityTests
{
    /// <summary>Each CLI report section (its heading up to the colon or the bracket) and the summary key that
    /// says the same thing.</summary>
    private static readonly Dictionary<string, string> Counterpart = new(StringComparer.Ordinal)
    {
        ["target"] = "report.target",
        ["verdict"] = "report.verdict",
        ["session"] = "report.session",
        ["exited"] = "report.target_exit",
        ["followed"] = "report.followed",
        ["covered channels"] = "coverage.covered",
        ["observed channels"] = "coverage.observed",
        ["uncovered channels"] = "coverage.uncovered",
        ["channels we meant to watch and could not hook"] = "coverage.unobserved",
        ["channels hooked only once their module loaded"] = "coverage.installed_late",
        ["processes this app spawned that the hook could not follow"] = "report.processes_uncovered",
        ["web engines reached inside the app"] = "report.engines_reached",
        ["warnings"] = "coverage.warnings",
        ["not fully cleaned up"] = "report.cleanup",
    };

    /// <summary>
    /// Sections the CLI report has and the summary knowingly does not, each with why and what makes it go.
    /// Checked from both ends: an entry the recorded report no longer has comes out. Only shrinks.
    /// </summary>
    private static readonly (string Section, string Why)[] KnownCliOnly =
    [
        ("errors",
            "a command the core refused while the session ran. The window shows it on the session screen while "
            + "it is live (InFlightErrorKey) and the copied summary leaves it out - goes when the summary lists "
            + "the refused commands under the verdict, as the report does"),
    ];

    private static string Wire(string name) => Path.Combine(TestPaths.RepoRoot(), "tests", "wire", name);

    /// <summary>The section headings of a CLI report: its lines that start two spaces in with a word, up to the
    /// colon, or the bracket where the heading explains itself.</summary>
    private static List<string> Sections(IEnumerable<string> report) =>
    [
        .. report
            .Where(line => line.Length > 2 && line.StartsWith("  ", StringComparison.Ordinal) && char.IsLetter(line[2]))
            .Select(line => line[2..line.IndexOf(':', StringComparison.Ordinal)])
            .Select(heading => heading.Split(" (")[0].Trim()),
    ];

    [Fact]
    public void Every_section_of_the_cli_report_has_its_line_in_the_copied_summary()
    {
        var sections = Sections(File.ReadAllLines(Wire("session-report.txt")));
        // The canary: a heading rule that matched nothing would pass for a window that says nothing.
        Assert.True(sections.Count >= 10, $"read {sections.Count} sections - is session-report.txt the core's file?");

        var vm = new SessionViewModel();
        vm.SetTarget(@"C:\apps\app.exe");
        foreach (var line in File.ReadAllLines(Wire("session.ndjson")))
        {
            vm.Apply(EventParser.Parse(line) ?? throw new InvalidOperationException("an event the window does not know: " + line));
        }

        var summary = vm.BuildSummary(key => key).Split('\n');
        var missing = new List<string>();
        foreach (var section in sections.Where(s => KnownCliOnly.All(k => k.Section != s)))
        {
            Assert.True(
                Counterpart.TryGetValue(section, out var key),
                $"the CLI report has a section '{section}' this test does not pair with the summary - give the window "
                + "its line and pair them here, or list it in KnownCliOnly with why");
            // The key as the line's own heading: a colon, a count in brackets or a value after a space follows it.
            if (!summary.Any(l => l.StartsWith("  " + key, StringComparison.Ordinal)
                                  && (l.Length == key.Length + 2 || l[key.Length + 2] is ':' or ' ')))
            {
                missing.Add($"{section} ({key})");
            }
        }

        Assert.True(missing.Count == 0, "the copied summary leaves out what the CLI report says: " + string.Join("; ", missing));
        // From both ends: a pair or an exception the recorded report no longer needs has to come out.
        Assert.Equal(Counterpart.Keys.Order(StringComparer.Ordinal), sections.Except(KnownCliOnly.Select(k => k.Section)).Order(StringComparer.Ordinal));
        Assert.All(KnownCliOnly, k => Assert.Contains(k.Section, sections));
    }
}
