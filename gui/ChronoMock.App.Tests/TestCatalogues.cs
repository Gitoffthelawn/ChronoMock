using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The preset catalogue as the window's tests see it: the ENGINE's answers, recorded in the repository and
/// held to the engine by its own tests (<c>preset_catalogue.rs</c>) - the shipped catalogue
/// (<c>presets-shipped.json</c>) and the test catalogue of odd files (<c>preset-catalogue.json</c>). The
/// window has no reader of its own since R4/18, so a test that wrote preset files and read them back would
/// be testing nothing the window does - and a catalogue made up here could say what the engine never would.
/// </summary>
internal static class TestCatalogues
{
    private static string Recorded(string file)
        => Path.Combine(TestPaths.RepoRoot(), "crates", "cli", "tests", "data", file);

    /// <summary>What the engine answers for the shipped <c>presets/</c> folder.</summary>
    public static PresetCatalogue Shipped() => PresetCatalogueClient.Parse(File.ReadAllText(Recorded("presets-shipped.json")));

    /// <summary>What the engine answers for the test catalogue - the bridge the Rust side records.</summary>
    public static PresetCatalogue TestCatalogue() => PresetCatalogueClient.Parse(File.ReadAllText(Recorded("preset-catalogue.json")));

    /// <summary>One shipped preset, as the window holds it.</summary>
    public static PresetInfo ShippedPreset(string id) => PresetInfo.From(Shipped().Presets.Single(p => p.Id == id));

    /// <summary>A catalogue for one window, answering with <paramref name="catalogue"/>.</summary>
    public static PresetLibrary Library(PresetCatalogue catalogue) => new(new FakePresetSource(catalogue));

    /// <summary>A scenario picker over <paramref name="catalogue"/>, ready to hand to a session.</summary>
    public static ScenarioPicker Picker(PresetCatalogue catalogue) => new(Library(catalogue));

    /// <summary>
    /// A session over the shipped catalogue with its scenario list read, as the window reads it once shown.
    /// The fake catalogue answers at once, so the read is over when this returns - asserted, not assumed,
    /// for the state factories that cannot await.
    /// </summary>
    public static SessionViewModel WithShippedScenarios(Func<ScenarioPicker, SessionViewModel> build)
    {
        ArgumentNullException.ThrowIfNull(build);
        var vm = build(Picker(Shipped()));
        var read = vm.EnsureScenariosAsync();
        Assert.True(read.IsCompletedSuccessfully, "the fake catalogue answers at once");
        return vm;
    }

    /// <summary>The shipped presets with the test catalogue's refused files beside them - the list a tester
    /// sees after dropping a few broken files of their own into the folder.</summary>
    public static PresetCatalogue ShippedWithFilesLeftOut()
        => new(PresetCatalogue.SupportedSchema, Shipped().Presets, TestCatalogue().Refused);

    /// <summary>A catalogue holding exactly the given canonical presets (JSON objects as the engine writes
    /// them) - for a shape no shipped preset has, such as a zone step.</summary>
    public static PresetCatalogue With(params string[] presets)
        => PresetCatalogueClient.Parse(
            $$"""{ "schema": "chronomock.presets/1", "presets": [ {{string.Join(", ", presets)}} ], "refused": [] }""");
}

/// <summary>An engine that never answers the catalogue question until the read is let go - a list as it is
/// while the engine is still starting.</summary>
internal sealed class HangingPresetSource : IPresetSource
{
    public async Task<PresetCatalogue> ReadAsync(CancellationToken ct = default)
    {
        await Task.Delay(Timeout.Infinite, ct);
        throw new InvalidOperationException("a delay without end ended");
    }
}

/// <summary>An engine that answers the catalogue question with what the test chose - or fails as told.</summary>
internal sealed class FakePresetSource : IPresetSource
{
    private readonly PresetCatalogue? _catalogue;
    private readonly string? _failure;

    public FakePresetSource(PresetCatalogue catalogue) => _catalogue = catalogue;

    private FakePresetSource(string failure) => _failure = failure;

    /// <summary>A source whose engine fails with <paramref name="engineMessage"/>, as <c>CalcException</c> carries it.</summary>
    public static FakePresetSource Failing(string engineMessage) => new(engineMessage);

    /// <summary>How many times the catalogue was asked for.</summary>
    public int Reads { get; private set; }

    public Task<PresetCatalogue> ReadAsync(CancellationToken ct = default)
    {
        Reads++;
        return _failure is null
            ? Task.FromResult(_catalogue!)
            : Task.FromException<PresetCatalogue>(new CalcException(_failure, -1));
    }
}
