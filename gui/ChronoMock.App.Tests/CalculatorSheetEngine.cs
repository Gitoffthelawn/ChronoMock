using System.Globalization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The engine behind the calculator renders in the state sheet: New Year's Day 2026 plus the day shift
/// asked for, with every format filled the way the real engine fills it, a refusal past a hundred thousand
/// days, and a fixed ambiguous analysis that refuses a date with a thirty-first month.
/// <para>
/// A type of its own rather than a method on the sheet, because answering in full takes most of the
/// protocol's result types, and the sheet's own coupling is measured like any other class's.
/// </para>
/// </summary>
internal static class CalculatorSheetEngine
{
    public static CalcResult Answer(IReadOnlyList<string> args)
    {
        var question = new FakeCalcEngine.Question(args);
        if (question.ValueOf("--analyze") is { } pasted)
        {
            if (pasted == "1740607200")
            {
                // The engine's own answer for this number with this machine at +02:00 (measured with
                // `chrono calc --analyze 1740607200 --json`, R4/18): two instants, each with its time.
                var metadata = new CalcMetadata("Thursday", 2025, 9, 9, 58, 1, false, -585, null, null);
                return new CalcResult(
                    "chronomock.calc/1",
                    null,
                    new CalcAnalysis(
                        pasted,
                        true,
                        [
                            new CalcReading("epoch_seconds", "2025-02-27T00:00:00", [], metadata, Instant: true, Time: "00:00:00"),
                            new CalcReading("epoch_millis", "1970-01-21T05:30:07", [], metadata with { Weekday = "Wednesday" }, Instant: true, Time: "05:30:07"),
                        ],
                        ZoneBiasMin: -120));
            }

            return pasted.StartsWith("31/31", StringComparison.Ordinal)
                ? throw new CalcException($"chrono calc: '{pasted}' is not a date this analyser reads (calc.analyze_unrecognized)", 1)
                : CalcResults.Analysis("2008-04-08T00:00:00", "2008-08-04T00:00:00");
        }

        var shift = question.ValueOf("--shift");
        var days = shift is null ? 0 : ShiftDays(shift);
        if (days > 100_000)
        {
            throw new CalcException("chrono calc: step 1 overflows the representable range (calc.overflow)", 1);
        }

        var moment = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc).AddDays(days);
        var epoch = new DateTimeOffset(moment).ToUnixTimeSeconds();
        var mask = question.ValueOf("--format");
        return new CalcResult(
            "chronomock.calc/1",
            new CalcMoment(
                moment.ToString("yyyy-MM-dd'T'HH:mm:ss", CultureInfo.InvariantCulture),
                0,
                "today",
                [shift ?? string.Empty],
                new CalcFormats(
                    moment.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture),
                    moment.ToString("yyyy-MM-dd'T'HH:mm:ss'+00:00'", CultureInfo.InvariantCulture),
                    moment.ToString("MM/dd/yyyy", CultureInfo.InvariantCulture),
                    moment.ToString("dd.MM.yyyy", CultureInfo.InvariantCulture),
                    epoch,
                    epoch * 1000,
                    moment.ToFileTimeUtc(),
                    moment.ToString("R", CultureInfo.InvariantCulture)),
                new CalcMetadata(moment.DayOfWeek.ToString(), 2026, 5, 5, moment.DayOfYear, 1, false, days, null, null),
                [],
                mask is null ? null : moment.ToString(mask, CultureInfo.InvariantCulture),
                null,
                null,
                null),
            null);
    }

    /// <summary>
    /// The shift's number, as days whatever its unit, so a sheet answers every step the builder can send.
    /// It read a one-letter unit only, so "+90bd" from a business-day scenario came back as a parse failure
    /// in place of a result, and every screen measured over such a scenario was measured without one.
    /// </summary>
    private static long ShiftDays(string shift)
    {
        var digits = new string([.. shift.Skip(1).TakeWhile(char.IsAsciiDigit)]);
        var amount = long.Parse(digits, CultureInfo.InvariantCulture);
        return shift[0] == '-' ? -amount : amount;
    }
}
