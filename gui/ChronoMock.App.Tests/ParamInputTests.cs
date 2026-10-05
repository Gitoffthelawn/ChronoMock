using System.Text.Json;
using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// A preset-parameter input (slice G4-2b), seeded from the parameter as the engine's catalogue gives it: a
/// date is null until entered, a duration and a variant seed their defaults - and stay EMPTY without one
/// (R4-S22) - and the label humanizes the parameter id. Pure - no UI thread.
/// </summary>
public class ParamInputTests
{
    private static IReadOnlyList<UnitOption> Units() =>
    [
        new("s", "calc.unit.seconds"), new("m", "calc.unit.minutes"), new("h", "calc.unit.hours"),
        new("d", "calc.unit.days"), new("w", "calc.unit.weeks"), new("mo", "calc.unit.months"),
        new("q", "calc.unit.quarters"), new("y", "calc.unit.years"), new("bd", "calc.unit.business_days"),
    ];

    /// <summary>A parameter written as the engine writes it in the catalogue.</summary>
    private static CatalogueParameter Param(string json) => JsonSerializer.Deserialize<CatalogueParameter>(json)!;

    private const string Boundary = """
        { "id": "boundary", "type": "variant", "default": "day_before", "default_hint": null,
          "choices": [ { "label": "day_before", "days": -1 }, { "label": "on_day", "days": 0 }, { "label": "day_after", "days": 1 } ] }
        """;

    [Fact]
    public void The_default_unit_is_found_by_token_not_by_position()
    {
        // R2-N11: the fallback unit was units[3], which happened to be days. Reordering the dropdown would
        // have silently changed what a parameter's unit means. Same list, reversed.
        var reversed = Units().Reverse().ToList();
        var input = new ParamInputViewModel(
            Param("""{ "id": "length", "type": "duration", "default": { "amount": 2, "unit": "mo" } }"""), reversed);

        Assert.Equal("mo", input.Unit.Token);
    }

    [Fact]
    public void A_parameter_type_this_build_does_not_resolve_yields_no_value()
    {
        // The catalogue lists only types the engine builds, so this is a defence: a type the window does not
        // know must not fall into another branch and compute from a value nobody entered (R2-S8).
        var input = new ParamInputViewModel(Param("""{ "id": "count", "type": "int", "default": null }"""), Units());

        Assert.False(input.IsDate);
        Assert.False(input.IsDuration);
        Assert.False(input.IsVariant);
        Assert.Null(input.ToValue());
    }

    [Fact]
    public void A_date_input_is_null_until_a_date_is_entered()
    {
        var input = new ParamInputViewModel(
            Param("""{ "id": "start_date", "type": "date", "default": null, "default_hint": "target_file_creation" }"""), Units());

        Assert.Null(input.ToValue());

        input.DateText = "2026-01-01";
        var value = Assert.IsType<DateValue>(input.ToValue());
        Assert.Equal("2026-01-01", value.DateTimeText);
    }

    [Fact]
    public void A_duration_input_seeds_its_default_and_yields_a_duration_value()
    {
        var input = new ParamInputViewModel(
            Param("""{ "id": "days", "type": "duration", "default": { "amount": 90, "unit": "bd" } }"""), Units());

        Assert.Equal("90", input.Amount);
        Assert.Equal("bd", input.Unit.Token);

        var value = Assert.IsType<DurationValue>(input.ToValue());
        Assert.Equal("90", value.Amount);
        Assert.Equal("bd", value.UnitToken);
    }

    /// <summary>R4-S22: it used to show "1" and compute "+1 day" while the engine said the parameter had no
    /// value. Reversal probe: seed the amount with "1" again and the first assertion fails.</summary>
    [Fact]
    public void A_duration_without_a_default_is_empty_until_an_amount_is_typed()
    {
        var input = new ParamInputViewModel(Param("""{ "id": "length", "type": "duration", "default": null }"""), Units());

        Assert.Equal(string.Empty, input.Amount);
        Assert.Null(input.ToValue());

        input.Amount = "   ";
        Assert.Null(input.ToValue());

        input.Amount = "7";
        Assert.Equal(new DurationValue("7", "d"), input.ToValue());
    }

    [Fact]
    public void A_variant_input_seeds_its_default_label_from_the_catalogues_choices()
    {
        var input = new ParamInputViewModel(Param(Boundary), Units());

        Assert.True(input.IsVariant);
        Assert.Equal(["day_before", "on_day", "day_after"], input.VariantOptions.Select(v => v.Token));
        Assert.Equal("calc.variant.on_day", input.VariantOptions[1].LabelKey);
        Assert.Equal("day_before", input.Variant!.Token);
        Assert.Equal("day_before", Assert.IsType<VariantValue>(input.ToValue()).Label);

        input.Variant = input.VariantOptions.First(v => v.Token == "day_after");
        Assert.Equal("day_after", Assert.IsType<VariantValue>(input.ToValue()!).Label);
    }

    /// <summary>R4/18 (D1 = B): the choices are the catalogue's, not a list this window keeps. The engine lists
    /// all three today, so a list of the window's own would look right until the two drifted apart.</summary>
    [Fact]
    public void A_variant_offers_exactly_the_choices_the_catalogue_lists()
    {
        var input = new ParamInputViewModel(Param("""
            { "id": "boundary", "type": "variant", "default": null,
              "choices": [ { "label": "on_day", "days": 0 }, { "label": "day_after", "days": 1 } ] }
            """), Units());

        Assert.Equal(["on_day", "day_after"], input.VariantOptions.Select(v => v.Token));
    }

    /// <summary>R4-S22: a variant without a default used to start on "day before" and compute with it.</summary>
    [Fact]
    public void A_variant_without_a_default_is_unchosen_until_the_tester_picks()
    {
        var input = new ParamInputViewModel(Param(Boundary.Replace("\"default\": \"day_before\"", "\"default\": null", StringComparison.Ordinal)), Units());

        Assert.Null(input.Variant);
        Assert.Null(input.ToValue());

        input.Variant = input.VariantOptions[0];
        Assert.Equal(new VariantValue("day_before"), input.ToValue());
    }

    [Fact]
    public void A_date_default_at_midnight_is_shown_as_the_bare_date_and_one_with_a_time_keeps_it()
    {
        var midnight = new ParamInputViewModel(Param("""{ "id": "d", "type": "date", "default": "2026-03-01T00:00:00" }"""), Units());
        var noon = new ParamInputViewModel(Param("""{ "id": "d", "type": "date", "default": "2026-03-01T12:30:00" }"""), Units());

        Assert.Equal("2026-03-01", midnight.DateText);
        Assert.Equal(new DateValue("2026-03-01"), midnight.ToValue());
        Assert.Equal("2026-03-01T12:30:00", noon.DateText);
        Assert.Equal(new DateValue("2026-03-01T12:30:00"), noon.ToValue());
    }

    [Fact]
    public void A_picked_calendar_day_writes_the_iso_date_text()
    {
        // The shared date input's calendar binds SelectedDate, the way it does on a MomentField. Culture
        // invariant: the text is ISO whatever the OS date format says (rule 2).
        var input = new ParamInputViewModel(Param("""{ "id": "start_date", "type": "date", "default": null }"""), Units());

        input.SelectedDate = new DateTime(2040, 6, 15);

        Assert.Equal("2040-06-15", input.DateText);
        Assert.Equal(new DateTime(2040, 6, 15), input.SelectedDate);
    }

    [Fact]
    public void A_date_still_being_typed_is_not_a_value_yet()
    {
        // The field updates on every keystroke, so "2040-0" must not reach the engine as a base and flash an
        // error mid-word. A full shape that is no date still goes through - the engine's own reason is the
        // honest message for it - and so do the shapes the engine writes: a time, a year before the era.
        var input = new ParamInputViewModel(Param("""{ "id": "start_date", "type": "date", "default": null }"""), Units());

        input.DateText = "2040-0";
        Assert.Null(input.ToValue());

        input.DateText = "2040-02-31";
        Assert.Equal(new DateValue("2040-02-31"), input.ToValue());

        input.DateText = "-0044-03-15";
        Assert.Equal(new DateValue("-0044-03-15"), input.ToValue());

        input.DateText = "2040-02-01T08:00";
        Assert.Null(input.ToValue());
    }

    [Fact]
    public void The_label_humanizes_the_parameter_id()
        => Assert.Equal(
            "install date",
            new ParamInputViewModel(Param("""{ "id": "install_date", "type": "date", "default": null }"""), Units()).Label);
}
