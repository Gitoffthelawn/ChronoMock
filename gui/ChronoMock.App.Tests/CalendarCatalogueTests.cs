using ChronoMock.App.Calc;
using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// R5-P12: the calculator offers the calendars the engine lists (<c>chrono calendars</c>), and a scenario that
/// counts in a calendar the engine does not list is not computed. The window used to hold three calendars in
/// its own code and, for any other, kept the calendar picked before - a market with no calendar here counted
/// its business days in another country's, with no word anywhere.
/// </summary>
public class CalendarCatalogueTests
{
    private const string PaymentDue = "payment-due-business-days"; // market us, so it counts in us-banking

    private static CalendarCatalogue With(params CatalogueCalendar[] calendars)
        => new(CalendarCatalogue.SupportedSchema, calendars, []);

    private static CatalogueCalendar Shipped(string id) => TestCatalogues.ShippedCalendars().Calendars.Single(c => c.Id == id);

    [Fact]
    public void The_recorded_engine_answer_reads_as_the_shipped_calendars()
    {
        var catalogue = TestCatalogues.ShippedCalendars();

        Assert.Equal(["pl", "us-banking", "us-federal"], catalogue.Calendars.Select(c => c.Id));
        Assert.Equal(new CalendarName("Poland", "Polska"), catalogue.Calendars[0].Name);
        Assert.Empty(catalogue.Refused);
    }

    [Fact]
    public void An_answer_in_another_schema_is_refused_as_that()
    {
        var e = Assert.Throws<CalcException>(
            () => CalendarCatalogueClient.Parse("""{ "schema": "chronomock.calendars/2", "calendars": [], "refused": [] }"""));

        Assert.Contains("chronomock.calendars/2", e.Message, StringComparison.Ordinal);
    }

    /// <summary>An answer missing a list or holding an entry without an id is refused as unreadable, not read
    /// into nulls that fail later where nothing catches them (review of #132). Reversal probe: drop the check and
    /// every case parses.</summary>
    [Theory]
    [InlineData("""{ "schema": "chronomock.calendars/1" }""")]
    [InlineData("""{ "schema": "chronomock.calendars/1", "calendars": null, "refused": [] }""")]
    [InlineData("""{ "schema": "chronomock.calendars/1", "calendars": [], "refused": null }""")]
    [InlineData("""{ "schema": "chronomock.calendars/1", "calendars": [ null ], "refused": [] }""")]
    [InlineData("""{ "schema": "chronomock.calendars/1", "calendars": [ { "id": null, "country": "PL", "name": null } ], "refused": [] }""")]
    [InlineData("""{ "schema": "chronomock.calendars/1", "calendars": [ { "id": "pl", "name": null } ], "refused": [] }""")]
    public void An_incomplete_answer_is_refused_as_unreadable(string json)
    {
        var e = Assert.Throws<CalcException>(() => CalendarCatalogueClient.Parse(json));

        Assert.Contains("incomplete", e.Message, StringComparison.Ordinal);
    }

    /// <summary>R5-D66: a calendar the window ships keeps the window's own text, any other is labelled by its
    /// file's English name, and one whose file names none by its id.</summary>
    [Fact]
    public void The_calculator_offers_what_the_engine_lists_under_the_windows_texts_or_the_files_name()
    {
        var engine = With(
            Shipped("pl"),
            new CatalogueCalendar("de", "DE", new CalendarName("Germany", "Deutschland")),
            new CatalogueCalendar("xx", "XX", Name: null));
        var vm = new CalculatorViewModel(new FakeCalcEngine(), presets: null, new CalendarChoices(new FakeCalendarSource(engine)));

        _ = vm.EnsureComputedAsync();

        Assert.Equal([null, "pl", "de", "xx"], vm.Calendars.Select(c => c.Id));
        Assert.Equal(["calc.cal.none", "calc.cal.pl", "Germany", "xx"], vm.Calendars.Select(c => c.LabelKey));
        Assert.False(vm.HasCalendarError);
    }

    /// <summary>The calendar picked while the engine was still answering stays picked when the engine lists
    /// it, and gives way to "no calendar" when it does not.</summary>
    [Fact]
    public void A_calendar_picked_before_the_list_arrives_stays_picked_only_if_listed()
    {
        var listed = new FakeCalendarSource();
        var kept = new CalculatorViewModel(new FakeCalcEngine(), presets: null, new CalendarChoices(listed));
        _ = kept.EnsureComputedAsync();
        kept.SelectedCalendar = kept.Calendars.Single(c => c.Id == "pl");

        listed.Answer(TestCatalogues.ShippedCalendars());

        Assert.Equal("pl", kept.SelectedCalendar.Id);
        Assert.Contains(kept.SelectedCalendar, kept.Calendars);

        var unlisted = new FakeCalendarSource();
        var dropped = new CalculatorViewModel(new FakeCalcEngine(), presets: null, new CalendarChoices(unlisted));
        _ = dropped.EnsureComputedAsync();
        dropped.SelectedCalendar = dropped.Calendars.Single(c => c.Id == "pl");

        unlisted.Answer(With(Shipped("us-banking")));

        Assert.Null(dropped.SelectedCalendar.Id);
    }

    /// <summary>
    /// The scenario is named with the calendar it needs and left uncomputed, and the calendar picked before is
    /// left alone - nothing is counted in it. Reversal probe: take the check out and the builder reaches for a
    /// calendar that is not on the list, which the old code answered by keeping the one picked before.
    /// </summary>
    [Fact]
    public void A_scenario_counting_in_a_calendar_that_is_not_listed_is_not_computed()
    {
        var seen = WpfTestHost.Invoke(() =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(),
                TestCatalogues.Library(TestCatalogues.Shipped()),
                new CalendarChoices(new FakeCalendarSource(With(Shipped("pl")))));
            _ = vm.EnsureComputedAsync();
            vm.SelectedCalendar = vm.Calendars.Single(c => c.Id == "pl");

            vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == PaymentDue);

            return new
            {
                vm.ActiveNoteKey,
                vm.ActiveNoteText,
                Calendar = vm.SelectedCalendar.Id,
                vm.HasResult,
                Steps = vm.Steps.Count,
                Expected = TextFormat.Translate("calc.preset_calendar_missing", "us-banking"),
            };
        });

        Assert.Equal("calc.preset_calendar_missing", seen.ActiveNoteKey);
        Assert.Equal(seen.Expected, seen.ActiveNoteText);
        Assert.Contains("us-banking", seen.ActiveNoteText, StringComparison.Ordinal);
        Assert.Equal("pl", seen.Calendar);
        Assert.False(seen.HasResult);
        Assert.Equal(0, seen.Steps);
    }

    /// <summary>A list the engine could not give leaves "no calendar" alone, says why under the field in the
    /// interface language, and a scenario that needs a calendar says the list could not be read - not that
    /// the calendar is missing, which nobody checked.</summary>
    [Fact]
    public void A_list_that_could_not_be_read_leaves_no_calendar_and_says_why()
    {
        const string EngineSaid = "chrono calendars: no calendar folder (looked in C:\\x\\calendars) (calendars.folder_missing)";

        var seen = WpfTestHost.Invoke(() =>
        {
            var vm = new CalculatorViewModel(
                new FakeCalcEngine(),
                TestCatalogues.Library(TestCatalogues.Shipped()),
                new CalendarChoices(FakeCalendarSource.Failing(EngineSaid)));
            _ = vm.EnsureComputedAsync();
            vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == PaymentDue);

            return new
            {
                Calendars = vm.Calendars.Select(c => c.Id).ToList(),
                vm.CalendarError,
                vm.ActiveNoteKey,
                Expected = TextFormat.Translate(
                    "calc.cal.unreadable", TranslationKeyConverter.Resolve("calendars.err.folder_missing")),
            };
        });

        Assert.Equal([null], seen.Calendars);
        Assert.Equal(seen.Expected, seen.CalendarError);
        Assert.DoesNotContain("calendars.", seen.CalendarError, StringComparison.Ordinal);
        Assert.Equal("calc.preset_calendar_unchecked", seen.ActiveNoteKey);
    }

    /// <summary>A scenario whose calendar IS listed fills the builder and picks that calendar, matched without
    /// case the way the engine finds the file.</summary>
    [Fact]
    public void A_scenario_whose_calendar_is_listed_picks_it()
    {
        var vm = new CalculatorViewModel(
            new FakeCalcEngine(),
            TestCatalogues.Library(TestCatalogues.Shipped()),
            new CalendarChoices(new FakeCalendarSource(With(Shipped("pl"), Shipped("us-banking") with { Id = "US-Banking" }))));
        _ = vm.EnsureComputedAsync();

        vm.SelectedPreset = vm.Presets.Single(p => p.Info.Id == PaymentDue);

        Assert.Equal("US-Banking", vm.SelectedCalendar.Id);
        Assert.NotEqual("calc.preset_calendar_missing", vm.ActiveNoteKey);
        Assert.NotEmpty(vm.Steps);
    }
}
