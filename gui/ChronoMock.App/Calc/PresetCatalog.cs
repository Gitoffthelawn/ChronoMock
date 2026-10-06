using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>
/// One preset from the shared catalogue (docs/04 4.2), as the engine hands it to the window
/// (<c>chrono presets</c>, <see cref="PresetCatalogue"/>): identity, the localized framing, which module it
/// applies to, the calendar its market implies, its parameters and its moment - all in canonical form. Name
/// and explains are DATA locales ({en, pl, ...}), not interface keys.
/// <para>
/// 🔴 The window does not read preset files. It did, with its own copy of the step grammar, and the copy
/// drifted from the engine's: a default of one <c>month</c> became one day, an unknown variant became
/// <c>day_before</c>, a step the engine refuses was computed (R4-S22). The engine is the one reader now, so
/// a preset is here exactly when <c>calc --preset</c> and <c>run --preset</c> accept it.
/// </para>
/// </summary>
public sealed record PresetInfo(
    string Id,
    IReadOnlyDictionary<string, string> Name,
    IReadOnlyDictionary<string, string> Explains,
    string AppliesTo,
    string? Market,
    string? Calendar,
    IReadOnlyList<CatalogueParameter> Parameters,
    CatalogueMoment Moment)
{
    /// <summary>The preset as the catalogue gives it.</summary>
    public static PresetInfo From(CataloguePreset preset)
    {
        ArgumentNullException.ThrowIfNull(preset);
        return new PresetInfo(
            preset.Id, preset.Name, preset.Explains, preset.AppliesTo, preset.Market, preset.Calendar,
            preset.Parameters, preset.Moment);
    }

    /// <summary>Whether this preset is offered by the calculator module (<c>calculator</c> or <c>both</c>).</summary>
    public bool ForCalculator => AppliesTo is "calculator" or "both";

    /// <summary>Whether this preset is offered by the substitution panel (<c>substitution</c> or <c>both</c>),
    /// mirroring the gate <c>chrono run --preset</c> applies (docs/04 4.2).</summary>
    public bool ForSubstitution => AppliesTo is "substitution" or "both";

    /// <summary>Whether this preset takes parameters.</summary>
    public bool IsParametric => Parameters.Count > 0;

    /// <summary>The name in the given culture, falling back to the default culture (English).</summary>
    public string LocalizedName(string culture) => Localized(Name, culture);

    /// <summary>The "what this date tests" framing in the given culture, English fallback.</summary>
    public string LocalizedExplains(string culture) => Localized(Explains, culture);

    /// <summary>One text of a preset file in the given culture, falling back to English, empty when the
    /// file has neither - the one rule for every per-language text a preset carries. A blank text counts
    /// as missing, so a language written as " " falls back to English rather than showing nothing.</summary>
    internal static string Localized(IReadOnlyDictionary<string, string>? map, string culture)
        => map is null ? string.Empty
            : map.TryGetValue(culture, out var value) && !string.IsNullOrWhiteSpace(value) ? value
            : map.TryGetValue(LocalizationService.DefaultCulture, out var fallback) ? fallback
            : string.Empty;
}

/// <summary>
/// The preset catalogue for one window, read once through the engine and shared by the calculator and the
/// substitution panel - two readers of the same list used to read the folder twice, each on its own terms.
/// <para>
/// Asked for, never read in a constructor: reading spawns the engine, and a window built in a test must start
/// nothing. A read that failed is not kept, so the next screen that asks tries again - an engine quarantined
/// for a moment does not leave the lists empty for the life of the window. A read in flight is shared.
/// </para>
/// </summary>
public sealed class PresetLibrary(IPresetSource? source)
{
    private readonly CancellationTokenSource _life = new();
    private Task<PresetCatalogue>? _read;

    /// <summary>
    /// The catalogue, read on the first call and shared after that. With no source (a test, a build with no
    /// catalogue at all) it is the empty catalogue. Throws what <see cref="IPresetSource.ReadAsync"/> throws.
    /// </summary>
    public Task<PresetCatalogue> ReadAsync()
    {
        if (source is null)
        {
            return Task.FromResult(new PresetCatalogue(PresetCatalogue.SupportedSchema, [], []));
        }

        if (_read is null || _read.IsFaulted || _read.IsCanceled)
        {
            _read = source.ReadAsync(_life.Token);
        }

        return _read;
    }

    /// <summary>Stop a read still in flight because the window is closing - the engine behind it is stopped
    /// rather than left to finish after the window has gone (the pattern of R4/13).</summary>
    public void Abandon() => _life.Cancel();
}
