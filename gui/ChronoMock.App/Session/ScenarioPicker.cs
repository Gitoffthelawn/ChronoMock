using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App;

/// <summary>
/// The scenarios a screen offers, and the text the tester narrows them with.
/// </summary>
/// <remarks>
/// 🔴 WHY THIS IS A TYPE RATHER THAN TWO MORE MEMBERS ON THE SESSION. The catalogue and the text that
/// narrows it are one idea, and they were about to be added to <see cref="SessionViewModel"/>, which
/// stood EXACTLY on its coupling ceiling of 82 - measured with tools/margin.ps1, not remembered. Moving
/// the catalogue out takes two types with it (the catalogue record and its reader) and brings one back,
/// so the class comes down instead of the ceiling going up. That is the door gui/CodeMetricsConfig.txt
/// asks callers to walk through, and the same one <see cref="RelativeMomentViewModel"/> walked through.
///
/// 🔴 IT HOLDS NO SELECTION, AND THAT IS THE WHOLE SEAM. Choosing a scenario computes a moment through
/// the engine and fills the date, which needs the calc client, the session zone and the moment field -
/// every one of them the session's, none of them the list's. Splitting the LIST from the CHOICE is what
/// lets this type know about neither, and what keeps it testable without a window or a process.
///
/// A filter that hides the CHOSEN scenario leaves the choice standing, which is deliberate: the moment
/// was already computed from it, and dropping the selection because of a search box would silently
/// change what the target is about to see. The screen keeps saying what the chosen scenario tests, in
/// the line under the list, so the choice never becomes invisible.
/// </remarks>
public sealed class ScenarioPicker : ObservableObject
{
    private readonly PresetLibrary _library;
    private ScenarioCatalogue _catalogue = ScenarioCatalogue.Empty;
    private IReadOnlyList<ScenarioItem> _visible = [];
    private string _filter = string.Empty;
    private bool _chosen;

    /// <summary>A picker over the given catalogue - the window's, shared with the calculator.</summary>
    public ScenarioPicker(PresetLibrary library) => _library = library ?? throw new ArgumentNullException(nameof(library));

    /// <summary>A picker with no catalogue, which reads as an empty one. Its own constructor rather than a
    /// default argument, so a caller that wants none does not carry the library type in its signature - the
    /// session's coupling ceiling counts it (gui/CodeMetricsConfig.txt).</summary>
    public ScenarioPicker() => _library = new PresetLibrary(null);

    /// <summary>Where the list stands - reading, failed, empty, or read with files left out. Said in the
    /// section's header and in its body, so a list that is not there yet never looks like an empty one.</summary>
    public CatalogueStatus Status { get; } = new();

    /// <summary>
    /// Read the substitution side of the shared preset catalogue, through the engine (R4/18).
    /// </summary>
    /// <remarks>
    /// Asked for by the window once it is shown, never in a constructor: reading spawns the engine, and a
    /// window built in a test must start nothing. Shares one read with the calculator (<see cref="PresetLibrary"/>).
    /// Here rather than on the session, whose coupling ceiling the library, the translation and the readiness
    /// would each have raised - the picker already owns the list, and now how it arrives.
    /// </remarks>
    public async Task LoadAsync()
    {
        Status.Reading();
        RaisePropertyChanged(nameof(ShowsCount));
        PresetCatalogue catalogue;
        try
        {
            catalogue = await _library.ReadAsync();
        }
        catch (OperationCanceledException)
        {
            return; // the window is closing
        }
        catch (CalcException e)
        {
            Status.Failed(e.Message, TranslationKeyConverter.Resolve);
            RaisePropertyChanged(nameof(ShowsCount));
            return;
        }

        _catalogue = ScenarioCatalog.From(catalogue.Presets.Select(PresetInfo.From));
        Status.Ready(_catalogue.Ready.Count, catalogue.Refused);
        Narrow();
        RaisePropertyChanged(nameof(ShowsCount));
        RaisePropertyChanged(nameof(HasScenarios));
        RaisePropertyChanged(nameof(Available));
        RaisePropertyChanged(nameof(NeedingParameters));
        RaisePropertyChanged(nameof(HasNeedingParameters));
        RaisePropertyChanged(nameof(ShowsParametricNote));
    }

    /// <summary>What the tester typed to narrow the list. Empty, or only spaces, shows everything.</summary>
    public string Filter
    {
        get => _filter;
        set
        {
            if (Set(ref _filter, value))
            {
                Narrow();
            }
        }
    }

    /// <summary>The scenarios to show right now: the whole catalogue, or what the filter matched.</summary>
    public IReadOnlyList<ScenarioItem> Visible => _visible;

    /// <summary>
    /// Whether the folded section's header shows the number on offer: the list read, and nothing chosen.
    /// </summary>
    /// <remarks>
    /// The header shows the chosen scenario, the number on offer, or why there is no list yet - one at a
    /// time, in one cell. The picker holds no selection (see the class remarks), so the session tells it
    /// only THAT one exists (<see cref="SetChosen"/>), which is all this line needs.
    /// </remarks>
    public bool ShowsCount => Status.IsReady && !_chosen;

    /// <summary>The session's word that a scenario is or is no longer chosen - for the header alone.</summary>
    public void SetChosen(bool chosen)
    {
        if (_chosen != chosen)
        {
            _chosen = chosen;
            RaisePropertyChanged(nameof(ShowsCount));
        }
    }

    /// <summary>True when the catalogue offered at least one scenario. The section shows either way since
    /// R4/18 - it used to vanish without a word when there were none, and with the list read after the
    /// window is shown it would have appeared under the reader's eyes and pushed everything below it down.</summary>
    public bool HasScenarios => _catalogue.Ready.Count > 0;

    /// <summary>
    /// How many scenarios the list offers, unfiltered.
    /// </summary>
    /// <remarks>
    /// 🔴 It exists so a FOLDED catalogue can advertise itself. The section holding it is closed on a
    /// first run, and a closed section saying only "Scenarios" gives a reader no reason to open it - while
    /// one saying there are sixteen ready-made dates inside is the reason. Unfiltered on purpose: the
    /// number is about what is installed, not about what a search left standing.
    /// </remarks>
    public int Available => _catalogue.Ready.Count;

    /// <summary>
    /// True when there ARE scenarios and the filter hid all of them.
    /// </summary>
    /// <remarks>
    /// Its own state rather than "the list is empty", because the two mean opposite things to a reader:
    /// nothing installed is a setup problem, nothing matched is a word they can delete. A screen that
    /// showed the same blank box for both would be answering neither.
    /// </remarks>
    public bool HasNoMatches => HasScenarios && _visible.Count == 0;

    /// <summary>How many substitution presets the list does NOT offer because they take parameters. Said
    /// out loud rather than hidden (rule 6) - the calculator can build those and hand the moment back.</summary>
    public int NeedingParameters => _catalogue.NeedingParameters;

    /// <summary>True when at least one preset was left out for taking parameters.</summary>
    public bool HasNeedingParameters => _catalogue.NeedingParameters > 0;

    /// <summary>
    /// Whether to print the "N more need a value you choose" line under the well.
    /// </summary>
    /// <remarks>
    /// 🔴 NOT WHILE A SEARCH FOUND NOTHING. The count is about the whole catalogue, but the line sits
    /// directly under the well - so with an empty result it read as the explanation of that result, as
    /// if three scenarios HAD matched and were waiting for a value. Measured on the setup-no-matches
    /// render, where it stood one line below "No scenario matches that". The sentence is about what is
    /// installed, and it says so only when the list beside it is showing what is installed.
    /// </remarks>
    public bool ShowsParametricNote => HasNeedingParameters && !HasNoMatches;

    private void Narrow()
    {
        var needle = _filter.Trim();

        _visible = needle.Length == 0
            ? _catalogue.Ready
            : [.. _catalogue.Ready.Where(s => Matches(s, needle))];

        RaisePropertyChanged(nameof(Visible));
        RaisePropertyChanged(nameof(HasNoMatches));
        RaisePropertyChanged(nameof(ShowsParametricNote));
    }

    /// <summary>
    /// Whether one scenario answers what was typed.
    /// </summary>
    /// <remarks>
    /// 🔴 CURRENT culture here, while the catalogue is ORDERED invariantly (see ScenarioCatalog.Load), and
    /// the difference is on purpose rather than an oversight to tidy up. Ordering has to be identical on
    /// every machine or "the third scenario" stops naming the same one. Matching is a person typing, and
    /// a person typing expects their own alphabet's idea of case.
    ///
    /// The name only, never the explanation. A row that matched on text the list does not show would be
    /// a result the reader cannot account for, and an unexplainable result is a fault here even when it
    /// is arguably useful.
    /// </remarks>
    private static bool Matches(ScenarioItem scenario, string needle)
        => scenario.DisplayName.Contains(needle, StringComparison.CurrentCultureIgnoreCase);
}
