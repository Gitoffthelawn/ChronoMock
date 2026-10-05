using System.Globalization;
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// The scenario list and the text that narrows it. Everything here is pure - no window, no process, no
/// engine - which is the point of the list living apart from the choice.
/// </summary>
public class ScenarioPickerTests
{
    /// <summary>A picker over the engine's answer for the shipped catalogue, read.</summary>
    private static async Task<ScenarioPicker> Loaded()
    {
        var picker = TestCatalogues.Picker(TestCatalogues.Shipped());
        await picker.LoadAsync();
        return picker;
    }

    [Fact]
    public void An_unloaded_picker_offers_nothing_and_says_so_without_reading_a_file()
    {
        var picker = new ScenarioPicker();

        Assert.Empty(picker.Visible);
        Assert.False(picker.HasScenarios);
        Assert.False(picker.HasNoMatches);
        Assert.False(picker.HasNeedingParameters);
    }

    /// <summary>R4/18: a catalogue the engine could not give is said in the list's place, and the header
    /// counts nothing over it. Before, a list that could not be read was an empty list, and the section
    /// hid itself without a word.</summary>
    [Fact]
    public async Task A_catalogue_that_could_not_be_read_is_said_and_counts_nothing()
    {
        var picker = new ScenarioPicker(new Calc.PresetLibrary(
            FakePresetSource.Failing("cannot launch 'C:\\x\\chrono.exe': not found (calc.launch_failed)")));

        await picker.LoadAsync();

        Assert.True(picker.Status.HasFailed);
        Assert.False(picker.Status.ShowsList);
        Assert.False(picker.ShowsCount);
        Assert.False(picker.HasScenarios);
        Assert.Contains("chrono.exe", picker.Status.FailureReason + picker.Status.FailureDetail, StringComparison.Ordinal);
    }

    [Fact]
    public async Task An_empty_filter_shows_the_whole_catalogue()
    {
        var picker = await Loaded();
        var all = picker.Visible.Count;

        Assert.True(all > 0, "the shipped presets folder has substitution scenarios in it");

        picker.Filter = "   ";

        Assert.Equal(all, picker.Visible.Count);
        Assert.False(picker.HasNoMatches);
    }

    [Fact]
    public async Task A_filter_keeps_only_the_names_that_contain_it()
    {
        var picker = await Loaded();
        var wanted = picker.Visible[0].DisplayName;

        picker.Filter = wanted;

        Assert.Contains(picker.Visible, s => s.DisplayName == wanted);
        Assert.All(
            picker.Visible,
            s => Assert.Contains(wanted, s.DisplayName, StringComparison.CurrentCultureIgnoreCase));
    }

    [Fact]
    public async Task Case_does_not_matter_to_the_person_typing()
    {
        var picker = await Loaded();
        var wanted = picker.Visible[0].DisplayName;

        picker.Filter = wanted.ToUpper(CultureInfo.CurrentCulture);
        var upper = picker.Visible.Count;

        picker.Filter = wanted.ToLower(CultureInfo.CurrentCulture);

        Assert.Equal(upper, picker.Visible.Count);
        Assert.NotEmpty(picker.Visible);
    }

    /// <summary>
    /// The two blank states mean opposite things and must not look alike to a caller.
    /// </summary>
    [Fact]
    public async Task Nothing_matched_is_a_different_state_from_nothing_installed()
    {
        var installed = await Loaded();
        installed.Filter = "no scenario is called this";

        Assert.Empty(installed.Visible);
        Assert.True(installed.HasScenarios);
        Assert.True(installed.HasNoMatches);

        var bare = new ScenarioPicker();

        Assert.Empty(bare.Visible);
        Assert.False(bare.HasScenarios);
        Assert.False(bare.HasNoMatches);
    }

    /// <summary>
    /// The "N more need a value you choose" line is about what is INSTALLED, and it sits directly under
    /// the well - so with a search showing nothing it read as the explanation of that emptiness, as if
    /// those N had matched and were waiting for a value. Measured on the setup-no-matches render.
    /// </summary>
    [Fact]
    public async Task The_parametric_note_goes_quiet_while_a_search_is_showing_nothing()
    {
        var picker = await Loaded();

        Assert.True(picker.HasNeedingParameters, "the shipped presets folder has parametric ones in it");
        Assert.True(picker.ShowsParametricNote);

        picker.Filter = "no scenario is called this";

        Assert.True(picker.HasNeedingParameters, "the count itself does not depend on the filter");
        Assert.False(picker.ShowsParametricNote);

        picker.Filter = string.Empty;

        Assert.True(picker.ShowsParametricNote);
    }

    /// <summary>
    /// The folded catalogue's header advertises how many dates are inside. A number that followed the
    /// filter would say "ready-made dates: 0" while the reason was a word in a box the reader cannot see
    /// with the section shut.
    /// </summary>
    [Fact]
    public async Task The_count_the_header_shows_is_the_whole_catalogue_and_not_the_filtered_view()
    {
        var picker = await Loaded();
        var all = picker.Available;

        Assert.Equal(picker.Visible.Count, all);

        picker.Filter = "no scenario is called this";

        Assert.Empty(picker.Visible);
        Assert.Equal(all, picker.Available);
    }

    [Fact]
    public async Task Clearing_the_filter_brings_the_whole_catalogue_back()
    {
        var picker = await Loaded();
        var all = picker.Visible.Count;

        picker.Filter = "no scenario is called this";
        picker.Filter = string.Empty;

        Assert.Equal(all, picker.Visible.Count);
    }

    /// <summary>
    /// A list that changed without saying so leaves the screen showing the previous one.
    /// </summary>
    [Fact]
    public async Task Narrowing_the_list_announces_the_list_and_the_empty_state()
    {
        var picker = await Loaded();
        var announced = new List<string>();
        picker.PropertyChanged += (_, e) => announced.Add(e.PropertyName ?? string.Empty);

        picker.Filter = "no scenario is called this";

        Assert.Contains(nameof(ScenarioPicker.Filter), announced);
        Assert.Contains(nameof(ScenarioPicker.Visible), announced);
        Assert.Contains(nameof(ScenarioPicker.HasNoMatches), announced);
        Assert.Contains(nameof(ScenarioPicker.ShowsParametricNote), announced);
    }

    [Fact]
    public async Task Loading_announces_everything_a_screen_binds_before_the_catalogue_is_there()
    {
        var picker = TestCatalogues.Picker(TestCatalogues.Shipped());
        var announced = new List<string>();
        picker.PropertyChanged += (_, e) => announced.Add(e.PropertyName ?? string.Empty);

        await picker.LoadAsync();

        Assert.Contains(nameof(ScenarioPicker.Visible), announced);
        Assert.Contains(nameof(ScenarioPicker.HasScenarios), announced);
        Assert.Contains(nameof(ScenarioPicker.Available), announced);
        Assert.Contains(nameof(ScenarioPicker.NeedingParameters), announced);
        Assert.Contains(nameof(ScenarioPicker.HasNeedingParameters), announced);
        Assert.Contains(nameof(ScenarioPicker.ShowsParametricNote), announced);
        Assert.Contains(nameof(ScenarioPicker.ShowsCount), announced);
    }

    /// <summary>
    /// A row that matched on text the reader cannot see is a result they cannot account for.
    /// </summary>
    [Fact]
    public async Task The_filter_reads_the_name_and_never_the_explanation()
    {
        var picker = await Loaded();
        var withExplanation = picker.Visible.First(s => s.DisplayExplains.Length > 0);
        var wordFromTheExplanationAlone = withExplanation.DisplayExplains
            .Split(' ')
            .FirstOrDefault(w =>
                w.Length > 5
                && !withExplanation.DisplayName.Contains(w, StringComparison.CurrentCultureIgnoreCase));

        Assert.SkipWhen(
            wordFromTheExplanationAlone is null,
            "no shipped scenario has a long word in its explanation that is absent from its name");

        picker.Filter = wordFromTheExplanationAlone!;

        Assert.DoesNotContain(picker.Visible, s => s.Id == withExplanation.Id);
    }
}
