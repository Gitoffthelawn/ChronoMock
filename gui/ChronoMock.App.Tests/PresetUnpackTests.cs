using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A preset's canonical moment (from the engine's catalogue) into builder inputs, so a click fills the
/// constructor (7.3). Pure. The moments are the engine's own answers - the shipped catalogue - and, for
/// shapes no shipped preset has, moments written the way the engine writes them.
/// </summary>
public class PresetUnpackTests
{
    private static PresetInfo Preset(string id) => TestCatalogues.ShippedPreset(id);

    private static UnpackedMoment Unpack(PresetInfo preset, Dictionary<string, ParamValue>? values = null)
        => PresetUnpack.UnpackMoment(preset.Moment, preset.Parameters, values);

    private static CatalogueMoment Moment(CatalogueBase b, params CatalogueStep[] steps) => new(b, steps);

    private static CatalogueStep Step(string kind, string? sign = null, long? amount = null, string? unit = null,
        string? parameter = null, string? target = null, string? time = null, string? offset = null)
        => new(kind, sign, amount, unit, parameter, target, time, offset);

    [Fact]
    public void A_snap_preset_unpacks_to_today_plus_a_snap_step()
    {
        var moment = Unpack(Preset("month-end"));

        Assert.Equal(BaseKind.Today, moment.Base);
        var step = Assert.Single(moment.Steps);
        Assert.Equal(StepKind.Snap, step.Kind);
        Assert.Equal("eom", step.SnapToken);
    }

    [Fact]
    public void An_absolute_utc_base_preset_unpacks_to_a_utc_instant_with_no_steps()
    {
        var moment = Unpack(Preset("epoch-zero"));

        Assert.Equal(BaseKind.SpecificUtc, moment.Base);
        Assert.Equal("1970-01-01T00:00:00", moment.BaseText);
        Assert.Empty(moment.Steps);
    }

    [Fact]
    public void The_2038_preset_carries_its_absolute_moment_as_a_utc_instant()
    {
        var moment = Unpack(Preset("year-2038"));

        Assert.Equal(BaseKind.SpecificUtc, moment.Base);
        Assert.Equal("2038-01-19T03:14:07", moment.BaseText);
    }

    [Fact]
    public void A_plain_absolute_base_stays_a_session_zone_moment()
    {
        var moment = PresetUnpack.UnpackMoment(
            Moment(new CatalogueBase("absolute", "2030-01-01T07:05:00", null), Step("set_time", time: "07:05:00")), []);

        Assert.Equal(BaseKind.Specific, moment.Base);
        Assert.Equal("2030-01-01T07:05:00", moment.BaseText);
        Assert.Equal("07:05:00", Assert.Single(moment.Steps).SetTime);
    }

    [Fact]
    public void A_shift_step_carries_the_unit_code_the_catalogue_gives()
    {
        // The file says "years", the catalogue says "y" - the builder's own option, read as given.
        var step = Unpack(Preset("license-expired-year-ago")).Steps.Single(s => s.Kind == StepKind.Shift);

        Assert.Equal(("-", "1", "y"), (step.Sign, step.Amount, step.UnitToken));
    }

    [Fact]
    public void The_feb_29_preset_unpacks_to_today_plus_a_next_leap_day_nearest_step()
    {
        var moment = Unpack(Preset("feb-29"));

        Assert.Equal(BaseKind.Today, moment.Base);
        var step = Assert.Single(moment.Steps);
        Assert.Equal(StepKind.Nearest, step.Kind);
        Assert.Equal("next-leap-day", step.NearestToken);
    }

    [Fact]
    public void The_age_of_majority_preset_unpacks_a_variant_into_a_signless_shift()
    {
        // birth_date base, then +18y, then the boundary variant (day_before -> "- 1 day", its own sign).
        var values = new Dictionary<string, ParamValue>
        {
            ["birth_date"] = new DateValue("2008-03-15"),
            ["boundary"] = new VariantValue("day_before"),
        };

        var moment = Unpack(Preset("age-of-majority"), values);

        Assert.Equal(BaseKind.Specific, moment.Base);
        Assert.Equal("2008-03-15T00:00:00", moment.BaseText);
        Assert.Equal(("+", "18", "y"), (moment.Steps[0].Sign, moment.Steps[0].Amount, moment.Steps[0].UnitToken));
        Assert.Equal(("-", "1", "d"), (moment.Steps[1].Sign, moment.Steps[1].Amount, moment.Steps[1].UnitToken));
    }

    /// <summary>The day offset of each choice comes from the catalogue - the window keeps no table of its own.</summary>
    [Theory]
    [InlineData("on_day", "+", "0")]
    [InlineData("day_after", "+", "1")]
    public void A_variant_resolves_to_its_signed_day_offset(string label, string sign, string amount)
    {
        var values = new Dictionary<string, ParamValue>
        {
            ["birth_date"] = new DateValue("2008-03-15"),
            ["boundary"] = new VariantValue(label),
        };

        var step = Unpack(Preset("age-of-majority"), values).Steps[1];

        Assert.Equal((sign, amount, "d"), (step.Sign, step.Amount, step.UnitToken));
    }

    /// <summary>R4/18 (D1 = B): the day offset is the catalogue's word, not a table of the window's own. The
    /// engine's three choices are -1, 0 and 1 today, so only an offset no engine gives yet tells the two apart.
    /// </summary>
    [Fact]
    public void A_variant_moves_by_the_days_the_catalogue_gives()
    {
        var boundary = System.Text.Json.JsonSerializer.Deserialize<CatalogueParameter>("""
            { "id": "boundary", "type": "variant", "default": null, "choices": [ { "label": "day_after", "days": 2 } ] }
            """)!;
        var moment = Moment(new CatalogueBase("today", null, null), Step("shift", parameter: "boundary"));

        var step = PresetUnpack.UnpackMoment(
            moment, [boundary], new Dictionary<string, ParamValue> { ["boundary"] = new VariantValue("day_after") }).Steps[0];

        Assert.Equal(("+", "2", "d"), (step.Sign, step.Amount, step.UnitToken));
    }

    [Fact]
    public void A_variant_label_the_catalogue_does_not_list_is_not_unpackable()
    {
        var values = new Dictionary<string, ParamValue>
        {
            ["birth_date"] = new DateValue("2008-03-15"),
            ["boundary"] = new VariantValue("week_before"),
        };

        Assert.Throws<NotSupportedException>(() => Unpack(Preset("age-of-majority"), values));
    }

    [Fact]
    public void A_parametric_base_without_its_value_is_not_unpackable()
        => Assert.Throws<NotSupportedException>(() => Unpack(Preset("age-of-majority")));

    [Fact]
    public void A_duration_parameter_resolves_a_shift_from_its_value()
    {
        var values = new Dictionary<string, ParamValue> { ["days"] = new DurationValue("90", "bd") };

        var step = Assert.Single(Unpack(Preset("payment-due-business-days"), values).Steps);

        Assert.Equal(("+", "90", "bd"), (step.Sign, step.Amount, step.UnitToken));
    }

    [Fact]
    public void A_date_parameter_resolves_an_absolute_base_with_its_shift_and_time()
    {
        var values = new Dictionary<string, ParamValue>
        {
            ["start_date"] = new DateValue("2026-01-01"),
            ["trial_length"] = new DurationValue("30", "d"),
        };

        var moment = Unpack(Preset("trial-last-day"), values);

        Assert.Equal(BaseKind.Specific, moment.Base);
        Assert.Equal("2026-01-01T00:00:00", moment.BaseText);
        Assert.Contains(moment.Steps, s => s.Kind == StepKind.Shift && s.Amount == "30" && s.UnitToken == "d");
        Assert.Contains(moment.Steps, s => s.Kind == StepKind.SetTime);
    }

    /// <summary>Shapes this window cannot represent - an engine newer than its window would be the only way
    /// to meet one - leave through the one failure type the callers catch, never through another.</summary>
    [Fact]
    public void A_shape_this_window_does_not_know_is_one_honest_failure()
    {
        var today = new CatalogueBase("today", null, null);
        CatalogueMoment[] shapes =
        [
            Moment(new CatalogueBase("tomorrow", null, null)),
            Moment(new CatalogueBase("absolute", null, null)),
            Moment(new CatalogueBase("parameter", null, null)),
            Moment(today, Step("jump")),
            Moment(today, Step("shift", unit: "d", amount: 1)),
            Moment(today, Step("shift", sign: "+", unit: "d")),
            Moment(today, Step("shift", sign: "+", amount: 1)),
            Moment(today, Step("snap")),
            Moment(today, Step("nearest")),
            Moment(today, Step("set_time")),
            Moment(today, Step("zone")),
            Moment(today, Step("shift", sign: "+", parameter: "missing")),
        ];

        foreach (var shape in shapes)
        {
            var ex = Record.Exception(() => PresetUnpack.UnpackMoment(shape, []));
            Assert.IsType<NotSupportedException>(ex);
        }
    }
}
