using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Text.Json;

using ChronoMock.App;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// The option that reaches the web pages of an application that runs as administrator by writing one
/// WebView2 value to the machine registry for the session (docs/09 section 12.19). It changes the machine
/// and opens a debugging port in an application with administrator rights, so every way it could be on
/// without the tester meaning it is held here: off by default, unusable from a window that is not elevated,
/// never ticked without the channel it rides on, and never sent to the core in any of those cases.
/// </summary>
/// <remarks>
/// A class of its own, not <c>SessionViewModelTests</c>: that one stands on its size ceiling.
/// </remarks>
public class ElevatedEmbeddedTests
{
    private static TimeSpec AnyTime() => new()
    {
        Moment = new MomentSpec { Kind = "absolute", Local = "2038-01-19T03:14:07", TzBiasMin = 0 },
        Mode = "flow",
    };

    /// <summary>A real PE to build a plan from: the app's own test host, which is certainly one.</summary>
    private static string APeFile() => System.Reflection.Assembly.GetExecutingAssembly().Location
        is { Length: > 0 } dll && File.Exists(Path.ChangeExtension(dll, ".exe"))
        ? Path.ChangeExtension(dll, ".exe")
        : Environment.ProcessPath!;

    private static SessionViewModel Elevated() => new(new InMemorySessionHistoryStore(), canReachElevated: true);

    [Fact]
    public void The_option_is_off_by_default_in_every_window()
    {
        Assert.False(new SessionViewModel().ReachElevatedEmbedded);
        Assert.False(Elevated().ReachElevatedEmbedded);
    }

    /// <summary>A window that is not elevated cannot use the option: it is greyed, and the line that says
    /// what it needs is shown in place of the cost - which only an elevated window has to weigh.</summary>
    [Fact]
    public void A_window_that_is_not_elevated_cannot_use_the_option_and_says_what_it_needs()
    {
        var vm = new SessionViewModel();

        Assert.False(vm.CanReachElevatedEmbedded);
        Assert.True(vm.ElevatedEmbeddedUnavailable);
        Assert.False(vm.ElevatedEmbeddedEnabled);
    }

    [Fact]
    public void An_elevated_window_can_use_it_while_idle_and_the_channel_is_on()
    {
        var vm = Elevated();

        Assert.True(vm.CanReachElevatedEmbedded);
        Assert.False(vm.ElevatedEmbeddedUnavailable);
        Assert.True(vm.ElevatedEmbeddedEnabled);
    }

    /// <summary>The option rides on the channel, so turning the channel off turns it off - unticked, not
    /// left ticked and greyed, because a box that looks chosen claims a session Start does not run - and the
    /// view is told, because it binds to the enabled state through a notification.</summary>
    [Fact]
    public void Turning_the_channel_off_unticks_the_option_and_announces_it()
    {
        var vm = Elevated();
        vm.ReachElevatedEmbedded = true;
        var announced = new List<string?>();
        vm.PropertyChanged += (_, e) => announced.Add(e.PropertyName);

        vm.ReachEmbedded = false;

        Assert.False(vm.ReachElevatedEmbedded);
        Assert.False(vm.ElevatedEmbeddedEnabled);
        Assert.Contains(nameof(SessionViewModel.ElevatedEmbeddedEnabled), announced);
        Assert.Contains(nameof(SessionViewModel.ReachElevatedEmbedded), announced);

        vm.ReachEmbedded = true;
        Assert.False(vm.ReachElevatedEmbedded, "turning the channel back on does not tick what was unticked");
        Assert.True(vm.ElevatedEmbeddedEnabled);
    }

    /// <summary>Greying out is enough only if the value is held too: a tick the form kept from before
    /// would be sent. What the form says and what Start sends are the same thing or the form lies.</summary>
    [Fact]
    public void A_tick_a_window_could_not_have_is_never_asked_of_the_core()
    {
        // What Start asks is what the record keeps, and the record is built from the same value.
        bool Asked(SessionViewModel vm)
        {
            vm.SetTarget(APeFile());
            return vm.BuildRecord().ElevatedEmbedded;
        }

        // A window that is not elevated, ticked from code (a binding, a replayed record): nothing is asked.
        var bare = new SessionViewModel();
        bare.ReachElevatedEmbedded = true;
        Assert.False(Asked(bare));

        // An elevated window with the box ticked asks.
        var elevated = Elevated();
        elevated.ReachElevatedEmbedded = true;
        Assert.True(Asked(elevated));

        // And not once the channel it rides on is off, even if the tick were put back by code.
        elevated.ReachEmbedded = false;
        elevated.ReachElevatedEmbedded = true;
        Assert.False(Asked(elevated));

        // Unticked, it is never asked.
        Assert.False(Asked(Elevated()));
    }

    /// <summary>The plan puts the option on the wire exactly when asked, and never beside a channel that
    /// is off: the core would ignore it, and a start command saying both says what the session will not do.</summary>
    [Fact]
    public void The_plan_carries_the_option_only_when_asked_and_only_with_the_channel()
    {
        Assert.False(SessionPlan.Build(APeFile(), AnyTime()).Start.Target.ElevatedEmbedded, "off unless asked");

        var on = SessionPlan.Build(APeFile(), AnyTime(), elevatedEmbedded: true);
        Assert.True(on.Start.Target.ElevatedEmbedded);
        Assert.Contains("\"elevated_embedded\":true", on.Start.ToNdjson(), StringComparison.Ordinal);

        var withoutChannel = SessionPlan.Build(APeFile(), AnyTime(), embedded: false, elevatedEmbedded: true);
        Assert.False(withoutChannel.Start.Target.ElevatedEmbedded);
        Assert.Contains("\"elevated_embedded\":false", withoutChannel.Start.ToNdjson(), StringComparison.Ordinal);
    }

    /// <summary>A record from before the field existed reads as off, which is what it did, and a record
    /// written now says what was asked of the core.</summary>
    [Fact]
    public void A_record_from_before_the_field_reads_as_off_and_the_field_round_trips()
    {
        const string old =
            "{\"target_path\":\"C:\\\\app.exe\",\"moment_local\":\"2038-01-19T03:14:07\",\"mode\":\"flow\"," +
            "\"verdict\":\"works\",\"ended_at_utc\":\"2026-10-01T00:00:00Z\"}";
        Assert.False(JsonSerializer.Deserialize<SessionRecord>(old)!.ElevatedEmbedded);

        var written = JsonSerializer.Serialize(new SessionRecord
        {
            TargetPath = @"C:\app.exe",
            MomentLocal = "2038-01-19T03:14:07",
            Mode = "flow",
            Verdict = "works",
            EndedAtUtc = "2026-10-01T00:00:00Z",
            ElevatedEmbedded = true,
        });
        Assert.Contains("\"elevated_embedded\":true", written, StringComparison.Ordinal);
        Assert.True(JsonSerializer.Deserialize<SessionRecord>(written)!.ElevatedEmbedded);
    }

    /// <summary>Repeating a session fills the form from what was asked of the core. An elevated window
    /// loads the tick, and a window that is not elevated does not: the box would claim what it cannot do.</summary>
    [Fact]
    public void Repeating_a_session_loads_the_tick_only_in_a_window_that_can_use_it()
    {
        var record = new SessionRecord
        {
            TargetPath = APeFile(),
            MomentLocal = "2038-01-19T03:14:07",
            Mode = "flow",
            Verdict = "partial",
            EndedAtUtc = "2026-10-01T00:00:00Z",
            ElevatedEmbedded = true,
        };

        foreach (var (elevated, expected) in new[] { (true, true), (false, false) })
        {
            var store = new InMemorySessionHistoryStore();
            store.Append(record);
            var vm = new SessionViewModel(store, canReachElevated: elevated);
            vm.SelectedRecord = vm.History[0];

            vm.Commands.Repeat.Execute(null);

            Assert.Equal(expected, vm.ReachElevatedEmbedded);
        }
    }
}
