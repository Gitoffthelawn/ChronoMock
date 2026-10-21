using System.Windows;
using System.Windows.Automation;
using ChronoMock.App.Controls;

namespace ChronoMock.App.Tests;

/// <summary>
/// The copy button (R5-D81): what it says in each state, what a screen reader hears, and that saying it
/// moves nothing.
/// </summary>
public sealed class CopyButtonTests
{
    /// <remarks>A fresh button for each state: one button measured again after its state changed answers
    /// from its cached measure, so the first version of this test passed over a button that did change
    /// width (found by the mutation entry that collapses the hidden texts).</remarks>
    [Fact]
    public void The_button_is_as_wide_in_every_state_as_in_the_widest()
    {
        var widths = WpfTestHost.Invoke(() => Enum.GetValues<CopyState>().Select(state =>
        {
            var button = new CopyButton { Label = "Copy", State = state };
            button.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
            return button.DesiredSize.Width;
        }).ToList());

        Assert.All(widths, width => Assert.Equal(widths[0], width));
        Assert.True(widths[0] > 0, "the button measured nothing, so equal widths prove nothing");
    }

    [Fact]
    public void A_reader_hears_the_accessible_label_and_then_what_the_copy_did()
    {
        var names = WpfTestHost.Invoke(() =>
        {
            var button = new CopyButton { Label = "Copy", AccessibleLabel = "Copy ISO 8601" };
            var idle = AutomationProperties.GetName(button);
            button.State = CopyState.Copied;
            var copied = AutomationProperties.GetName(button);
            button.State = CopyState.Failed;
            var failed = AutomationProperties.GetName(button);
            return (idle, copied, failed);
        });

        Assert.Equal("Copy ISO 8601", names.idle);
        Assert.Equal(WpfTestHost.Invoke(() => TranslationKeyConverter.Resolve("copy.copied")), names.copied);
        Assert.Equal(WpfTestHost.Invoke(() => TranslationKeyConverter.Resolve("copy.busy")), names.failed);
        Assert.NotEqual("copy.busy", names.failed); // resolved, not the key read out
    }

    [Fact]
    public void Without_an_accessible_label_a_reader_hears_the_label()
        => Assert.Equal(
            "Copy summary",
            WpfTestHost.Invoke(() => AutomationProperties.GetName(new CopyButton { Label = "Copy summary" })));

    [Fact]
    public void The_button_shows_what_its_outcome_reports()
    {
        var states = WpfTestHost.Invoke(() =>
        {
            var outcome = new CopyOutcome((_, _) => new TaskCompletionSource().Task);
            var button = new CopyButton { Label = "Copy", Outcome = outcome };
            var before = button.State;
            outcome.Report(copied: true);
            return (before, after: button.State);
        });

        Assert.Equal(CopyState.Idle, states.before);
        Assert.Equal(CopyState.Copied, states.after);
    }
}
