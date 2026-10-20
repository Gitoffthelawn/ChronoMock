using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// What the setup form asks of the core: the moment, the zone, the rate and the scaling options mapped to
/// the wire time. Moved here from SessionViewModelTests when the start setup became its own record (R5-D62)
/// - the session plan is built from that record, so the record is what these hold.
/// </summary>
public class SessionSetupTests
{
    [Fact]
    public void The_form_maps_the_inputs_to_the_wire()
    {
        var vm = new SessionViewModel();
        vm.Moment.DateText = "2040-06-15";
        vm.Moment.TimeText = "08:30";
        vm.SelectedZone = vm.Zones.First(z => z.BiasMinutes == 300); // UTC-05:00
        vm.SelectedMode = vm.Modes.First(m => m.Mode == "frozen");

        var time = vm.LiveSetup.Time();

        Assert.Equal("absolute", time.Moment.Kind);
        Assert.Equal("2040-06-15T08:30:00", time.Moment.Local); // canonicalised to the 'T' form
        Assert.Equal(300, time.Moment.TzBiasMin);
        Assert.Equal("frozen", time.Mode);
        Assert.Null(time.Multiplier);
        Assert.False(time.ScaleDuration); // off by default
        Assert.False(time.ScaleQpc); // off by default (ADR-2)
    }

    [Fact]
    public void The_form_carries_the_scale_duration_toggle()
    {
        // The toggle (chrono-mock 11.1 pt 4) flows to the wire so a duration-based target - a countdown,
        // an animation - can be sped up with the multiplier, not just the wall clock.
        var vm = new SessionViewModel { ScaleDuration = true };
        Assert.True(vm.LiveSetup.Time().ScaleDuration);

        vm.ScaleDuration = false;
        Assert.False(vm.LiveSetup.Time().ScaleDuration);
    }

    [Fact]
    public void The_form_carries_the_scale_qpc_toggle()
    {
        // The QPC toggle (A3, ADR-2 reversal) flows to the wire so a QPC-based timer (Python monotonic,
        // .NET Stopwatch, Java nanoTime) accelerates with the multiplier too. Separate from scale_duration.
        var vm = new SessionViewModel { ScaleQpc = true };
        Assert.True(vm.LiveSetup.Time().ScaleQpc);
        Assert.False(vm.LiveSetup.Time().ScaleDuration); // independent of scale_duration

        vm.ScaleQpc = false;
        Assert.False(vm.LiveSetup.Time().ScaleQpc);
    }
}
