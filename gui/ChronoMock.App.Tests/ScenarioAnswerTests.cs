using System.IO;
using ChronoMock.App.Localization;
using ChronoMock.App.Views;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The session's moment field takes an engine answer only while it answers the newest request, and a date
/// the tester puts there is never overwritten by one (R4-S25). Start goes with the chosen date, never the
/// one before the choice, and refuses a choice that gave no date (R4-Z2). A new zone asks again, because
/// the zone is part of the question (R4-Z1).
/// <para>
/// The engine answers when each test says so. Where it ignores cancellation, the old answer really does
/// arrive last, so what keeps it out is the request it belonged to, not the luck of it never coming.
/// </para>
/// </summary>
public class ScenarioAnswerTests
{
    private static readonly TimeSpan RoomForAWrongApply = TimeSpan.FromMilliseconds(300);

    [Fact]
    public async Task A_late_answer_for_an_earlier_scenario_never_lands_over_the_later_choice()
    {
        var engine = new FakeCalcEngine(honoursCancellation: false);
        var vm = NewSession(engine);
        var (a, b) = (vm.ScenarioPicker.Visible[0], vm.ScenarioPicker.Visible[1]);

        // Arrowing down the list. Choosing A starts exactly this request (the selection's setter runs it),
        // and it is started here so the test can wait for it to finish rather than guess how long that takes.
        var choiceOfA = vm.ApplyScenarioAsync(a);
        var forA = await engine.QuestionAsync(0);
        vm.SelectedScenario = b;
        var forB = await engine.QuestionAsync(1);

        forB.Answer(CalcResults.Moment("2031-02-02T00:00:00"));
        await CalculatorAnswerTests.Until(() => vm.Moment.Canonical == "2031-02-02T00:00:00", "B's date");
        forA.Answer(CalcResults.Moment("2030-01-01T00:00:00"));
        await choiceOfA; // A's late answer has now been applied or refused - nothing of it is still on its way

        Assert.Equal("2031-02-02T00:00:00", vm.Moment.Canonical);
        Assert.Same(b, vm.SelectedScenario);
    }

    [Fact]
    public async Task A_date_typed_while_a_scenario_is_computed_stays_as_typed()
    {
        var engine = new FakeCalcEngine(honoursCancellation: false);
        var vm = NewSession(engine);

        var choice = vm.ApplyScenarioAsync(vm.ScenarioPicker.Visible[0]);
        var forScenario = await engine.QuestionAsync(0);
        vm.Moment.DateText = "2040-06-15";
        var typed = vm.Moment.Canonical;

        forScenario.Answer(CalcResults.Moment("2030-01-01T00:00:00"));
        await choice;

        Assert.Equal(typed, vm.Moment.Canonical);
        Assert.False(vm.HasScenarioError); // refused for being old, which is no failure of the scenario's
    }

    [Fact]
    public async Task A_relative_answer_that_comes_after_a_typed_date_fills_nothing()
    {
        var engine = new FakeCalcEngine(honoursCancellation: false);
        var vm = NewSession(engine);

        var apply = vm.Relative.ApplyAsync();
        var question = await engine.QuestionAsync(0);
        vm.Moment.DateText = "2040-06-15";
        var typed = vm.Moment.Canonical;

        question.Answer(CalcResults.Moment("2030-01-01T00:00:00"));
        await apply;

        Assert.Equal(typed, vm.Moment.Canonical);
        Assert.False(vm.Relative.HasError); // refused for being old, which is no failure of the line's
    }

    [Fact]
    public async Task Start_pressed_while_a_scenario_is_computed_waits_for_its_date()
    {
        var engine = new FakeCalcEngine();
        var vm = NewSession(engine);
        var target = NotAnExecutable();
        try
        {
            vm.SetTarget(target);
            vm.SelectedScenario = vm.ScenarioPicker.Visible[0];
            var forScenario = await engine.QuestionAsync(0);

            var start = vm.StartAsync();
            await Task.Delay(RoomForAWrongApply, TestContext.Current.CancellationToken);

            // Still waiting: nothing about the session has moved yet.
            Assert.False(start.IsCompleted);
            Assert.Equal(SessionStatusKind.Idle, vm.StatusKind);

            forScenario.Answer(CalcResults.Moment("2030-01-01T00:00:00"));
            await start;

            // It went ahead with the chosen date. The target is a text file, so the plan refuses it before
            // any core is spawned - which is what proves Start got past the wait.
            Assert.Equal("2030-01-01T00:00:00", vm.Moment.Canonical);
            Assert.Equal("status.target_unsupported", vm.StatusKey);
        }
        finally
        {
            File.Delete(target);
        }
    }

    [Fact]
    public async Task Start_refuses_when_the_chosen_date_is_still_being_computed_after_the_wait()
    {
        // A launch held up long enough (a scanner on the engine, say) outlasts Start's wait. Going ahead then
        // would start with the date from before the choice - the fault the wait is for - and say nothing.
        var engine = new FakeCalcEngine();
        var vm = TestCatalogues.WithShippedScenarios(scenarios => new SessionViewModel(
            new InMemorySessionHistoryStore(),
            calcClient: engine,
            scenarios: scenarios,
            momentWait: TimeSpan.FromMilliseconds(200)));
        var target = NotAnExecutable();
        try
        {
            vm.SetTarget(target);
            var choice = vm.ApplyScenarioAsync(vm.ScenarioPicker.Visible[0]);
            var forScenario = await engine.QuestionAsync(0);

            await vm.StartAsync(); // gives up after the short wait

            Assert.Equal(SessionStatusKind.Idle, vm.StatusKind); // nothing was started
            Assert.False(vm.CanStart);
            Assert.Equal("setup.moment_pending", vm.StartRefusalKey);
            Assert.False(vm.HasContract); // and no promise of the old date beside the refusal

            forScenario.Answer(CalcResults.Moment("2030-01-01T00:00:00"));
            await choice;

            // The date landed, so Start is back, with nothing left to say against it.
            Assert.True(vm.CanStart);
            Assert.False(vm.HasStartRefusal);
            Assert.Equal("2030-01-01T00:00:00", vm.Moment.Canonical);
        }
        finally
        {
            File.Delete(target);
        }
    }

    [Fact]
    public async Task A_scenario_that_gave_no_date_refuses_Start_until_a_date_is_typed()
    {
        var engine = new FakeCalcEngine(_ => throw new CalcException("chrono calc: the scenario overflows (calc.overflow)", 1));
        var vm = NewSession(engine);
        var target = NotAnExecutable();
        try
        {
            vm.SetTarget(target);
            Assert.True(vm.CanStart); // the shipped date is valid - the refusal below is the scenario's

            vm.SelectedScenario = vm.ScenarioPicker.Visible[0];
            await CalculatorAnswerTests.Until(() => vm.HasScenarioError, "the scenario's refusal");

            // R4-Z2: the field still holds the date from before the choice, and starting with it would
            // start a session the tester did not choose.
            Assert.False(vm.CanStart);
            Assert.Equal("setup.scenario_failed", vm.StartRefusalKey);
            Assert.True(vm.HasStartRefusal);

            // Nor does the footer go on promising that old date beside the refusal (owner, 2026-10-04).
            Assert.True(vm.HasMomentPreview); // the field still holds a moment...
            Assert.False(vm.HasContract); // ...that is not offered as the session's

            vm.Moment.DateText = "2040-06-15";

            Assert.True(vm.CanStart);
            Assert.False(vm.HasStartRefusal);
            Assert.True(vm.HasContract);
        }
        finally
        {
            File.Delete(target);
        }
    }

    [Fact]
    public void The_footer_stops_promising_the_old_date_while_Start_refuses_it()
    {
        // Measured on the laid-out phase, not read off the binding (GUI rule 10): the contract's opening
        // words are on screen for a configured form and absent beside the refusal.
        var (configured, refused) = WpfTestHost.InvokeSettled(() =>
        {
            var promise = TranslationKeyConverter.Resolve("moment.preview_label");
            return (Promises(PhaseStates.SetupConfigured(), promise), Promises(PhaseStates.SetupWithFailedScenario(), promise));
        });

        Assert.True(configured); // not vacuous: the line is found where it should be
        Assert.False(refused);
    }

    /// <summary>Whether the laid-out setup phase shows a visible line reading <paramref name="text"/>.</summary>
    private static bool Promises(SessionViewModel model, string text)
    {
        var view = new SetupPhaseView { DataContext = model };
        LayoutProbe.Settle(view);
        return LayoutProbe.Walk(view).Any(e => e.IsVisible && e.Text == text);
    }

    [Fact]
    public async Task Changing_the_zone_asks_the_chosen_scenario_again_in_the_new_zone()
    {
        // The engine answers in the zone it is asked in, the way a UTC instant does: 03:14:07 in UTC is a
        // different wall clock anywhere else.
        var engine = new FakeCalcEngine(args => CalcResults.Moment(
            new FakeCalcEngine.Question(args).ValueOf("--zone") == "+00:00" ? "2038-01-19T03:14:07" : "2038-01-19T04:14:07"));
        var vm = NewSession(engine);
        var scenario = vm.ScenarioPicker.Visible[0];

        vm.SelectedScenario = scenario;
        await CalculatorAnswerTests.Until(() => vm.Moment.Canonical == "2038-01-19T03:14:07", "the UTC answer");
        var zone = vm.Zones.First(z => z.BiasMinutes != 0);
        vm.SelectedZone = zone;
        await CalculatorAnswerTests.Until(() => vm.Moment.Canonical == "2038-01-19T04:14:07", "the answer in the new zone");

        // R4-Z1: before, the field kept the UTC wall clock, which in the new zone names another instant,
        // under a selection that still claimed the scenario.
        Assert.Same(scenario, vm.SelectedScenario);
        Assert.Equal(ZoneLabel.OffsetFromBiasMinutes(zone.BiasMinutes), engine.Last.ValueOf("--zone"));
    }

    [Fact]
    public async Task A_relative_request_in_flight_when_the_zone_changes_is_asked_again_in_the_new_zone()
    {
        // "Now plus one day" is a different civil date in another zone, so an answer computed for the zone
        // the panel no longer shows must not fill the field - it is asked again instead.
        var engine = new FakeCalcEngine();
        var vm = NewSession(engine);

        var apply = vm.Relative.ApplyAsync();
        var inUtc = await engine.QuestionAsync(0);
        var zone = vm.Zones.First(z => z.BiasMinutes != 0);
        vm.SelectedZone = zone;
        var inNewZone = await engine.QuestionAsync(1);

        inNewZone.Answer(CalcResults.Moment("2026-10-05T01:00:00"));
        inUtc.Answer(CalcResults.Moment("2026-10-05T00:00:00"));
        await apply;
        await CalculatorAnswerTests.Until(() => vm.Moment.Canonical == "2026-10-05T01:00:00", "the answer in the new zone");

        Assert.Equal("+00:00", inUtc.ValueOf("--zone"));
        Assert.Equal(ZoneLabel.OffsetFromBiasMinutes(zone.BiasMinutes), inNewZone.ValueOf("--zone"));
    }

    private static SessionViewModel NewSession(ICalcEngine engine)
        => TestCatalogues.WithShippedScenarios(scenarios => new(new InMemorySessionHistoryStore(), calcClient: engine, scenarios: scenarios));

    /// <summary>A target the plan refuses before any core is spawned, so Start can be driven without one.</summary>
    private static string NotAnExecutable()
    {
        var path = Path.Combine(Path.GetTempPath(), $"chrono-{Guid.NewGuid():N}.txt");
        File.WriteAllText(path, "not a PE");
        return path;
    }
}
