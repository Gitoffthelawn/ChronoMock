using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// R5-P13: an answer the clock has moved past is asked again before it is used. Nothing in the builder changes
/// when time passes, so a "Today + N" computed yesterday counted as current and went to the substitution panel
/// and the clipboard as today's. The engine here answers each question with the next day, so which answer was
/// used is read off the date.
/// </summary>
public class CalculatorClockTests
{
    private static readonly DateTimeOffset Morning = new(2026, 10, 20, 10, 0, 0, TimeSpan.Zero);

    /// <summary>A view model on <paramref name="clock"/> whose engine answers its n-th date question with day n
    /// of 2026-11 - the reverse-analysis strip asks too, and is answered apart.</summary>
    private static (CalculatorViewModel Vm, Func<int> Asked) Calculator(ManualClock clock)
    {
        var asked = 0;
        var engine = new FakeCalcEngine(args => args.Contains("--analyze")
            ? CalcResults.Analysis("2008-04-08T00:00:00")
            : CalcResults.Moment($"2026-11-{Interlocked.Increment(ref asked):00}T00:00:00"));
        var vm = new CalculatorViewModel(engine, clock: new AnswerClock(clock));
        return (vm, () => Volatile.Read(ref asked));
    }

    private static async Task<string?> Transferred(CalculatorViewModel vm)
    {
        string? sent = null;
        void Take(string moment, int bias) => sent = moment;
        vm.UseInSubstitutionRequested += Take;
        await vm.RequestUseInSubstitutionAsync();
        vm.UseInSubstitutionRequested -= Take;
        return sent;
    }

    /// <summary>The day the answer was asked on is the day it is used on - a later hour of the same day asks
    /// nothing, the next day asks again and sends the new answer. Reversal probe: take the check out of the
    /// press and the second press sends day 1.</summary>
    [Fact]
    public async Task A_today_answer_is_asked_again_once_the_day_has_turned()
    {
        var clock = new ManualClock(Morning);
        var (vm, asked) = Calculator(clock);
        await vm.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => vm.HasResult && !vm.IsResultStale, "the first answer");

        clock.Now = Morning.AddHours(13); // 23:00, the same day
        Assert.Equal("2026-11-01T00:00:00", await Transferred(vm));
        Assert.Equal(1, asked());

        clock.Now = Morning.AddHours(14.5); // 00:30 the next day
        Assert.Equal("2026-11-02T00:00:00", await Transferred(vm));
        Assert.Equal(2, asked());
    }

    /// <summary>"Today" in a fixed zone east of UTC turns at that zone's midnight, which is the evening before in
    /// UTC. Reversal probe: read the day in UTC and both presses fall on one UTC day, so nothing is asked again.</summary>
    [Fact]
    public async Task A_today_answer_in_a_fixed_zone_turns_at_that_zones_midnight()
    {
        var clock = new ManualClock(Morning);
        var (vm, asked) = Calculator(clock);
        var east = vm.BaseZones.First(z => !z.IsHost && z.BiasMinutes < 0);
        var midnight = new DateTimeOffset(2026, 10, 21, 0, 0, 0, TimeSpan.Zero).AddMinutes(east.BiasMinutes); // UTC = local + bias
        clock.Now = midnight.AddMinutes(-30);
        vm.SelectedBaseZone = east;
        await vm.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => vm.HasResult && !vm.IsResultStale, "the first answer");
        var before = asked();

        clock.Now = midnight.AddMinutes(30);
        await Transferred(vm);

        Assert.Equal(midnight.UtcDateTime.Date, midnight.AddMinutes(-30).UtcDateTime.Date); // one UTC day - not vacuous
        Assert.Equal(before + 1, asked());
    }

    /// <summary>A Now base is read to the second, so a copy a moment later copies the answer for the time of
    /// the press, and the screen shows that same answer.</summary>
    [Fact]
    public async Task A_now_answer_is_asked_again_before_it_is_copied()
    {
        var clock = new ManualClock(Morning);
        var (vm, asked) = Calculator(clock);
        vm.SelectedBase = vm.BaseKinds.Single(b => b.Kind == BaseKind.Now);
        await vm.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => vm.HasResult && !vm.IsResultStale && asked() >= 1, "the first answer");
        var before = asked();
        var iso = vm.Formats[0];

        clock.Now = Morning.AddSeconds(5);
        var copied = await vm.CopyValueAsync(iso);

        Assert.Equal(before + 1, asked());
        Assert.Equal(vm.Formats[0].Value, copied);
    }

    /// <summary>Showing the screen again asks again when the day has turned, and a specific date never does,
    /// however far the clock moves.</summary>
    [Fact]
    public async Task Showing_the_screen_again_asks_only_for_an_answer_the_clock_moved_past()
    {
        var clock = new ManualClock(Morning);
        var (today, todayAsked) = Calculator(clock);
        await today.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => today.HasResult && !today.IsResultStale, "the first answer");

        await today.EnsureComputedAsync();
        Assert.Equal(1, todayAsked());

        clock.Now = Morning.AddDays(1);
        await today.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => todayAsked() == 2 && !today.IsResultStale, "the answer for the new day");

        var (specific, specificAsked) = Calculator(clock);
        specific.SelectedBase = specific.BaseKinds.Single(b => b.Kind == BaseKind.Specific);
        await specific.EnsureComputedAsync();
        await CalculatorAnswerTests.Until(() => specific.HasResult && !specific.IsResultStale, "the specific answer");
        var settled = specificAsked();

        clock.Now = Morning.AddDays(30);
        await specific.EnsureComputedAsync();
        Assert.Equal($"2026-11-{settled:00}T00:00:00", await Transferred(specific));
        Assert.Equal(settled, specificAsked());
    }
}

/// <summary>A clock the test moves by hand, in UTC.</summary>
internal sealed class ManualClock(DateTimeOffset now) : TimeProvider
{
    public DateTimeOffset Now { get; set; } = now;

    public override DateTimeOffset GetUtcNow() => Now;

    public override TimeZoneInfo LocalTimeZone => TimeZoneInfo.Utc;
}
