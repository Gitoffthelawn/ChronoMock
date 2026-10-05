using System.Globalization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>A resolved value for a preset parameter, supplied to <see cref="PresetUnpack.UnpackMoment"/> to
/// fill a parametric base or shift.</summary>
public abstract record ParamValue;

/// <summary>A date value for a <c>date</c> parameter (becomes an absolute base). A bare date is midnight.</summary>
public sealed record DateValue(string DateTimeText) : ParamValue;

/// <summary>A magnitude and unit code for a <c>duration</c> parameter (becomes a shift - the sign is the step's).</summary>
public sealed record DurationValue(string Amount, string UnitToken) : ParamValue;

/// <summary>A choice label for a <c>variant</c> parameter (docs/05 3.6) - becomes a shift by the day offset
/// the catalogue gives that choice, carrying its own direction.</summary>
public sealed record VariantValue(string Label) : ParamValue;

/// <summary>A preset's moment expressed as builder inputs: the base and a list of steps ready to configure
/// <see cref="StepViewModel"/>s. Each step carries the value(s) its kind reads - the others keep the same
/// defaults a fresh step has.</summary>
public sealed record UnpackedStep(
    StepKind Kind,
    string Sign = "+",
    string Amount = "1",
    string UnitToken = "d",
    string SnapToken = "eom",
    string NearestToken = "nbd",
    string SetTime = "23:59:59",
    string ZoneOffset = "+00:00");

/// <summary>A preset's moment as builder inputs (base + steps), produced by <see cref="PresetUnpack"/>.</summary>
public sealed record UnpackedMoment(BaseKind Base, string BaseText, IReadOnlyList<UnpackedStep> Steps)
{
    /// <summary>
    /// The <c>chrono calc</c> flag pair for one step - the ONE place a step kind becomes flags, shared by
    /// the calculator's builder (<see cref="StepViewModel.ToArgs"/>) and by the substitution panel's
    /// scenario list. The panel deliberately evaluates a preset through these flags rather than through
    /// <c>calc --preset</c>: that path gates on <c>applies_to</c> and refuses a substitution-only preset,
    /// which would drop exactly the scenarios the panel exists to offer (year rollover, expired licence).
    /// </summary>
    public static IReadOnlyList<string> StepArgs(UnpackedStep step) => step.Kind switch
    {
        StepKind.Shift => ["--shift", $"{step.Sign}{step.Amount.Trim()}{step.UnitToken}"],
        StepKind.Snap => ["--snap", step.SnapToken],
        StepKind.Nearest => ["--nearest", step.NearestToken],
        StepKind.SetTime => ["--set-time", step.SetTime.Trim()],
        StepKind.Zone => ["--to-zone", step.ZoneOffset.Trim()],
        _ => [],
    };
}

/// <summary>
/// Turns a preset's canonical moment (<see cref="CatalogueMoment"/>, from <c>chrono presets</c>) into builder
/// inputs so a click fills the constructor (7.3). One source of truth - the builder - and the whole
/// live-recompute pipeline reused, with no separate preset compute path.
/// <para>
/// 🔴 IT INTERPRETS NOTHING. Every word arrives in the form the builder sends back as a flag (unit, target
/// and sign codes, ISO dates, <c>HH:MM:SS</c>, <c>±HH:MM</c>), written by the engine from its own types - so
/// there is no alias table here to drift from the engine's, which is how one <c>month</c> used to become one
/// day (R4-S22). What is left is filling the parameters: a date into the base, a duration's amount and unit
/// into its shift, a variant's day offset - taken from the choices the catalogue gives it - into its shift.
/// </para>
/// <para>
/// A parameter with no value, or a shape this build does not know (a step kind added to the engine and not
/// yet to the window), throws <see cref="NotSupportedException"/> - the one failure the callers catch, so the
/// builder is never filled from a moment it cannot represent honestly (rule 6).
/// </para>
/// </summary>
public static class PresetUnpack
{
    private static readonly IReadOnlyDictionary<string, ParamValue> NoValues = new Dictionary<string, ParamValue>();

    public static UnpackedMoment UnpackMoment(
        CatalogueMoment moment,
        IReadOnlyList<CatalogueParameter> parameters,
        IReadOnlyDictionary<string, ParamValue>? values = null)
    {
        ArgumentNullException.ThrowIfNull(moment);
        ArgumentNullException.ThrowIfNull(parameters);
        var given = values ?? NoValues;
        var (baseKind, baseText) = Base(moment.Base, given);
        var steps = moment.Steps.Select(step => Step(step, parameters, given)).ToList();
        return new UnpackedMoment(baseKind, baseText, steps);
    }

    private static (BaseKind, string) Base(CatalogueBase b, IReadOnlyDictionary<string, ParamValue> values) => b.Kind switch
    {
        "today" => (BaseKind.Today, string.Empty),
        "now" => (BaseKind.Now, string.Empty),
        "absolute" => (BaseKind.Specific, Required(b.At, "an absolute base")),
        "absolute_utc" => (BaseKind.SpecificUtc, Required(b.At, "a UTC base")),
        "parameter" => values.TryGetValue(Required(b.Parameter, "a parameter base"), out var value) && value is DateValue date
            ? (BaseKind.Specific, NormalizeDate(date.DateTimeText))
            : throw new NotSupportedException($"base parameter '{b.Parameter}' has no date value"),
        var other => throw new NotSupportedException($"unknown base kind '{other}'"),
    };

    private static UnpackedStep Step(
        CatalogueStep step, IReadOnlyList<CatalogueParameter> parameters, IReadOnlyDictionary<string, ParamValue> values)
        => step.Kind switch
        {
            "shift" when step.Parameter is { } id => ParametricShift(step, id, parameters, values),
            "shift" => new UnpackedStep(
                StepKind.Shift,
                Sign: Required(step.Sign, "a shift"),
                Amount: (step.Amount ?? throw new NotSupportedException("a shift has no amount")).ToString(CultureInfo.InvariantCulture),
                UnitToken: Required(step.Unit, "a shift")),
            "snap" => new UnpackedStep(StepKind.Snap, SnapToken: Required(step.Target, "a snap step")),
            "nearest" => new UnpackedStep(StepKind.Nearest, NearestToken: Required(step.Target, "a nearest step")),
            "set_time" => new UnpackedStep(StepKind.SetTime, SetTime: Required(step.Time, "a set_time step")),
            "zone" => new UnpackedStep(StepKind.Zone, ZoneOffset: Required(step.Offset, "a zone step")),
            var other => throw new NotSupportedException($"unknown step kind '{other}'"),
        };

    // A duration gives the amount and unit, with the step's sign. A variant gives the day offset of the chosen
    // label from the choices the catalogue lists for that parameter - its own sign, so the step has none.
    private static UnpackedStep ParametricShift(
        CatalogueStep step, string id, IReadOnlyList<CatalogueParameter> parameters, IReadOnlyDictionary<string, ParamValue> values)
    {
        switch (values.GetValueOrDefault(id))
        {
            case DurationValue duration:
                return new UnpackedStep(
                    StepKind.Shift, Sign: Required(step.Sign, "a duration shift"), Amount: duration.Amount, UnitToken: duration.UnitToken);
            case VariantValue variant:
                var choice = parameters.FirstOrDefault(p => p.Id == id)?.Choices?.FirstOrDefault(c => c.Label == variant.Label)
                    ?? throw new NotSupportedException($"parameter '{id}' has no choice '{variant.Label}'");
                return new UnpackedStep(
                    StepKind.Shift,
                    Sign: choice.Days < 0 ? "-" : "+",
                    Amount: Math.Abs(choice.Days).ToString(CultureInfo.InvariantCulture),
                    UnitToken: "d");
            default:
                throw new NotSupportedException($"shift parameter '{id}' has no duration or variant value");
        }
    }

    private static string Required(string? value, string what)
        => value ?? throw new NotSupportedException($"{what} is missing a field");

    // A bare date is midnight, matching the engine's parse_param_date so a date parameter resolves the same.
    private static string NormalizeDate(string value)
        => value.Contains('T', StringComparison.Ordinal) || value.Contains(' ', StringComparison.Ordinal)
            ? value
            : $"{value}T00:00:00";
}
