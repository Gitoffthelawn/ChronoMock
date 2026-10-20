using System.Globalization;

namespace ChronoMock.App.Calc;

/// <summary>
/// What the clock contributes to a calculator answer (R5-P13): the civil day in the base zone for a Today
/// base, the second for a Now base, and nothing for a specific date. Two answers asked under the same stamp
/// are the same answer, so an answer whose stamp is no longer the clock's has been overtaken by time.
/// </summary>
/// <remarks>
/// Its own type so the calculator's view model holds one clock rather than the time, zone and culture types
/// it takes to read one - it stands on its coupling ceiling (gui/CodeMetricsConfig.txt). The time comes in as a
/// <see cref="TimeProvider"/>, so a test moves it by hand.
/// </remarks>
public sealed class AnswerClock(TimeProvider time)
{
    /// <summary>The machine's own clock and zone.</summary>
    public static AnswerClock System { get; } = new(TimeProvider.System);

    /// <summary>
    /// The stamp for a base of <paramref name="kind"/> read in <paramref name="zone"/>. The machine's own zone
    /// goes through its rules, as the engine reads it when no zone is sent, so a change of daylight saving
    /// since the window opened does not move midnight. Any other zone is a fixed offset (rule 2).
    /// </summary>
    public string Stamp(BaseKind kind, ZoneOption zone)
    {
        ArgumentNullException.ThrowIfNull(zone);
        var now = time.GetUtcNow();
        return kind switch
        {
            BaseKind.Today => (zone.IsHost
                    ? TimeZoneInfo.ConvertTime(now, time.LocalTimeZone).DateTime
                    : now.UtcDateTime.AddMinutes(-zone.BiasMinutes))
                .ToString("yyyy-MM-dd", CultureInfo.InvariantCulture),
            BaseKind.Now => now.UtcDateTime.ToString("yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture),
            _ => string.Empty,
        };
    }
}
