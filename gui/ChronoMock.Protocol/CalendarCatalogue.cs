using System.Text.Json;
using System.Text.Json.Serialization;

namespace ChronoMock.Protocol;

/// <summary>
/// The calendar catalogue as the engine reads it (<c>chrono calendars --json</c>, schema
/// <c>chronomock.calendars/1</c>, docs/08 section 9c-quater). The window used to offer three calendars written
/// into its own code, so a calendar added as a data file could not be picked, and a preset whose market named
/// one the window did not know left the previous calendar in place without a word (R5-P12). Now the engine is
/// the one reader, and a calendar is listed here exactly when <c>calc --calendar</c> counts in it. Fields the
/// window does not use are not modelled - unknown fields are ignored (additive evolution).
/// </summary>
public sealed record CalendarCatalogue(
    [property: JsonPropertyName("schema")] string Schema,
    [property: JsonPropertyName("calendars")] IReadOnlyList<CatalogueCalendar> Calendars,
    [property: JsonPropertyName("refused")] IReadOnlyList<RefusedCalendarFile> Refused)
{
    /// <summary>The only catalogue schema this build reads.</summary>
    public const string SupportedSchema = "chronomock.calendars/1";
}

/// <summary>One listed calendar: the id <c>--calendar</c> takes, the country it is for, and the file's own
/// name for it - absent when the file gives none.</summary>
public sealed record CatalogueCalendar(
    [property: JsonPropertyName("id")] string Id,
    [property: JsonPropertyName("country")] string Country,
    [property: JsonPropertyName("name")] CalendarName? Name);

/// <summary>A calendar's own name: in English, and in the language of the country it is for.</summary>
public sealed record CalendarName(
    [property: JsonPropertyName("en")] string En,
    [property: JsonPropertyName("local")] string Local);

/// <summary>A file the engine left out of the catalogue, with the sentence <c>--calendar</c> gives for it.</summary>
public sealed record RefusedCalendarFile(
    [property: JsonPropertyName("file")] string File,
    [property: JsonPropertyName("reason")] string Reason);

/// <summary>Where the window gets the calendar catalogue from - the engine in every build, an answer chosen by
/// the test in the tests (the pattern of <see cref="IPresetSource"/>).</summary>
public interface ICalendarSource
{
    /// <summary>Read the catalogue. Throws <see cref="CalcException"/> when the engine could not answer (its
    /// sentence, or a keyed one for a launch that failed or a limit reached), and
    /// <see cref="OperationCanceledException"/> when <paramref name="ct"/> is cancelled first.</summary>
    Task<CalendarCatalogue> ReadAsync(CancellationToken ct = default);
}

/// <summary>
/// Reads the catalogue by running <c>chrono calendars --dir &lt;folder&gt; --json</c>. The folder is always
/// passed, so the answer does not depend on where the engine happens to run from.
/// </summary>
public sealed class CalendarCatalogueClient(Func<string> chronoPath, string calendarsDir) : ICalendarSource
{
    public async Task<CalendarCatalogue> ReadAsync(CancellationToken ct = default)
    {
        var stdout = await CalcClient
            .RunKeyedAsync(chronoPath(), ["calendars", "--dir", calendarsDir, "--json"], null, ct)
            .ConfigureAwait(false);
        return Parse(stdout);
    }

    /// <summary>The catalogue in one answer of the engine's, refused when it is not one - a schema this build
    /// does not read is said as such rather than read as an empty list.</summary>
    public static CalendarCatalogue Parse(string json)
    {
        CalendarCatalogue? catalogue;
        try
        {
            catalogue = JsonSerializer.Deserialize<CalendarCatalogue>(json, ProtocolJson.Options);
        }
        catch (JsonException e)
        {
            throw new CalcException($"the calendar catalogue was not valid JSON: {e.Message}", 0);
        }

        if (catalogue is null)
        {
            throw new CalcException("the engine gave no calendar catalogue", 0);
        }

        if (catalogue.Schema != CalendarCatalogue.SupportedSchema)
        {
            throw new CalcException(
                $"the engine answered with catalogue schema '{catalogue.Schema}', and this window reads {CalendarCatalogue.SupportedSchema}",
                0);
        }

        // System.Text.Json does not hold a reference type to its nullability, so an answer missing a list, or
        // holding an entry with no id, would arrive here whole and fail later as a null dereference nobody
        // catches - the calculator's scenario list then stayed on "reading" with no reason. Refused here, it
        // takes the path every other unreadable answer takes, and the calendar field says why.
        if (catalogue.Calendars is null || catalogue.Refused is null
            || catalogue.Calendars.Any(c => c is null || string.IsNullOrEmpty(c.Id) || c.Country is null))
        {
            throw new CalcException("the engine's calendar catalogue is incomplete", 0);
        }

        return catalogue;
    }
}
