using System.Text.Json;
using System.Text.Json.Serialization;

namespace ChronoMock.Protocol;

/// <summary>
/// The preset catalogue as the engine reads it (<c>chrono presets --json</c>, schema
/// <c>chronomock.presets/1</c>, docs/08 section 9c-ter). The window used to read the preset files itself,
/// with a copy of the step grammar that drifted from the engine's (R4-S22) - now the engine is the one
/// reader, and a preset is listed here exactly when <c>calc --preset</c> and <c>run --preset</c> accept it.
/// Every word in it is already canonical: unit, target and sign codes the step builder offers, ISO dates,
/// <c>HH:MM:SS</c> times and <c>±HH:MM</c> offsets, so the window maps them to its controls and interprets
/// none. Fields the window does not use are not modelled - unknown fields are ignored (additive evolution).
/// </summary>
public sealed record PresetCatalogue(
    [property: JsonPropertyName("schema")] string Schema,
    [property: JsonPropertyName("presets")] IReadOnlyList<CataloguePreset> Presets,
    [property: JsonPropertyName("refused")] IReadOnlyList<RefusedPresetFile> Refused)
{
    /// <summary>The only catalogue schema this build reads.</summary>
    public const string SupportedSchema = "chronomock.presets/1";
}

/// <summary>One listed preset: its texts in every language its file has, which module it is for, the
/// calendar its market implies, and its parameters and moment in canonical form.</summary>
public sealed record CataloguePreset(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("applies_to")] string AppliesTo,
    [property: JsonPropertyName("market")] string? Market,
    [property: JsonPropertyName("calendar")] string? Calendar,
    [property: JsonPropertyName("name")] IReadOnlyDictionary<string, string> Name,
    [property: JsonPropertyName("explains")] IReadOnlyDictionary<string, string> Explains,
    [property: JsonPropertyName("parameters")] IReadOnlyList<CatalogueParameter> Parameters,
    [property: JsonPropertyName("moment")] CatalogueMoment Moment);

/// <summary>A parameter: <c>date</c>, <c>duration</c> or <c>variant</c>. <see cref="Default"/> is shaped
/// by the type - a date as ISO text, a duration as <c>{ amount, unit }</c> with a unit code, a variant as its
/// label - and is a JSON null when the file gives none. A variant carries its choices and the day offset
/// each moves the boundary by, so the window keeps no copy of that table. <see cref="Label"/> is what a
/// reader calls the parameter, per language - empty when the file names none, absent from an engine
/// older than R4/19, and the window shows the id in both cases.</summary>
public sealed record CatalogueParameter(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("type")] string Type,
    [property: JsonPropertyName("default")] JsonElement Default,
    [property: JsonPropertyName("choices")] IReadOnlyList<CatalogueChoice>? Choices,
    [property: JsonPropertyName("label")] IReadOnlyDictionary<string, string>? Label = null);

/// <summary>One variant choice: its label (a translation key suffix) and its signed day offset.</summary>
public sealed record CatalogueChoice(
    [property: JsonPropertyName("label")] string Label,
    [property: JsonPropertyName("days")] long Days);

/// <summary>A moment in canonical form: where it starts and the steps after it.</summary>
public sealed record CatalogueMoment(
    [property: JsonPropertyName("base")] CatalogueBase Base,
    [property: JsonPropertyName("steps")] IReadOnlyList<CatalogueStep> Steps);

/// <summary>The base: <c>today</c>, <c>now</c>, <c>absolute</c> or <c>absolute_utc</c> (with <see cref="At"/>
/// as ISO), or <c>parameter</c> (with <see cref="Parameter"/> naming a date parameter).</summary>
public sealed record CatalogueBase(
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("at")] string? At,
    [property: JsonPropertyName("parameter")] string? Parameter);

/// <summary>One step, by <see cref="Kind"/>: <c>shift</c> (literal sign, amount and unit code, or filled by a
/// parameter - a duration with the step's sign, a variant without one), <c>snap</c> and <c>nearest</c>
/// (<see cref="Target"/> code), <c>set_time</c> (<see cref="Time"/>), <c>zone</c> (<see cref="Offset"/>).</summary>
public sealed record CatalogueStep(
    [property: JsonPropertyName("kind")] string Kind,
    [property: JsonPropertyName("sign")] string? Sign,
    [property: JsonPropertyName("amount")] long? Amount,
    [property: JsonPropertyName("unit")] string? Unit,
    [property: JsonPropertyName("parameter")] string? Parameter,
    [property: JsonPropertyName("target")] string? Target,
    [property: JsonPropertyName("time")] string? Time,
    [property: JsonPropertyName("offset")] string? Offset);

/// <summary>A file the engine left out of the catalogue, with the sentence <c>--preset</c> gives for it.</summary>
public sealed record RefusedPresetFile(
    [property: JsonPropertyName("file")] string File,
    [property: JsonPropertyName("reason")] string Reason);

/// <summary>Where the window gets the preset catalogue from - the engine in every build, an answer chosen by
/// the test in the tests (the pattern of <see cref="ICalcEngine"/>).</summary>
public interface IPresetSource
{
    /// <summary>Read the catalogue. Throws <see cref="CalcException"/> when the engine could not answer (its
    /// sentence, or a keyed one for a launch that failed or a limit reached), and
    /// <see cref="OperationCanceledException"/> when <paramref name="ct"/> is cancelled first.</summary>
    Task<PresetCatalogue> ReadAsync(CancellationToken ct = default);
}

/// <summary>
/// Reads the catalogue by running <c>chrono presets --dir &lt;folder&gt; --json</c>. The folder is always
/// passed, so the answer does not depend on where the engine happens to run from.
/// </summary>
public sealed class PresetCatalogueClient(Func<string> chronoPath, string presetsDir) : IPresetSource
{
    public async Task<PresetCatalogue> ReadAsync(CancellationToken ct = default)
    {
        var stdout = await CalcClient
            .RunKeyedAsync(chronoPath(), ["presets", "--dir", presetsDir, "--json"], null, ct)
            .ConfigureAwait(false);
        return Parse(stdout);
    }

    /// <summary>The catalogue in one answer of the engine's, refused when it is not one - a schema this build
    /// does not read is said as such rather than read as an empty list.</summary>
    public static PresetCatalogue Parse(string json)
    {
        PresetCatalogue? catalogue;
        try
        {
            catalogue = JsonSerializer.Deserialize<PresetCatalogue>(json, ProtocolJson.Options);
        }
        catch (JsonException e)
        {
            throw new CalcException($"the preset catalogue was not valid JSON: {e.Message}", 0);
        }

        if (catalogue is null)
        {
            throw new CalcException("the engine gave no preset catalogue", 0);
        }

        return catalogue.Schema == PresetCatalogue.SupportedSchema
            ? catalogue
            : throw new CalcException(
                $"the engine answered with catalogue schema '{catalogue.Schema}', and this window reads {PresetCatalogue.SupportedSchema}",
                0);
    }
}
