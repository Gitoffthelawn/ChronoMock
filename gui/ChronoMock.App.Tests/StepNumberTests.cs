using ChronoMock.App.Calc;

namespace ChronoMock.App.Tests;

/// <summary>
/// The number a calculator step carries for a screen reader ("Step 2", "Remove step 2", R5-D82): from 1, in
/// list order, kept true as steps come and go, and never taken for an edit of the question.
/// </summary>
public sealed class StepNumberTests
{
    private static readonly TimeSpan AnswerDeadline = TimeSpan.FromSeconds(5);

    [Fact]
    public void Steps_are_numbered_from_one_and_renumbered_when_one_goes()
    {
        var numbers = WpfTestHost.Invoke(() =>
        {
            var vm = new CalculatorViewModel(new FakeCalcEngine(CalculatorSheetEngine.Answer));
            vm.AddStep();
            vm.AddStep();
            vm.AddStep();
            var added = vm.Steps.Select(s => s.Number).ToList();
            vm.RemoveStep(vm.Steps[0]);
            return (added, after: vm.Steps.Select(s => s.Number).ToList());
        });

        Assert.Equal([1, 2, 3], numbers.added);
        Assert.Equal([1, 2], numbers.after);
    }

    [Fact]
    public async Task A_new_number_asks_the_engine_nothing()
    {
        var stale = await WpfTestHost.RunAsync(async () =>
        {
            var vm = new CalculatorViewModel(new FakeCalcEngine(CalculatorSheetEngine.Answer));
            await vm.EnsureComputedAsync();
            vm.AddStep();
            var deadline = DateTime.UtcNow + AnswerDeadline;
            while (vm.IsResultStale && DateTime.UtcNow < deadline)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(25), TestContext.Current.CancellationToken);
            }

            Assert.False(vm.IsResultStale, "the step's own answer never landed, so the next line proves nothing");
            vm.Steps[0].Number = 7;
            return vm.IsResultStale;
        });

        Assert.False(stale, "renumbering a step was taken for an edit and asked the engine again");
    }
}
