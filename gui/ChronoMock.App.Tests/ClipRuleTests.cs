using System.Windows;
using System.Windows.Controls;

namespace ChronoMock.App.Tests;

/// <summary>
/// The layout instrument's own checks for the edges that cut content: which axis a scroll viewer scrolls,
/// and a control cut in part (PR A finding (a)). Apart from <see cref="LayoutGuardTests"/>, which sits at
/// the class coupling ceiling - these build their trees from controls that class has no other use for.
/// </summary>
public class ClipRuleTests
{
    /// <summary>A scroller as the calculator's columns build one: down only, sideways disabled.</summary>
    private static ScrollViewer ScrollsDownOnly(UIElement content) => new()
    {
        Content = content,
        HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
    };

    /// <summary>
    /// 🔴 PR A finding (a), the instrument's own check. The spill rule used to excuse EVERYTHING under a
    /// scroll viewer, so content wider than a column that scrolls down only - cut at its side edge, with no
    /// way to scroll to it - passed as content below the fold. Reversal probe: give the probe one flag for
    /// both axes again and this goes quiet.
    /// </summary>
    [Fact]
    public void The_spill_rule_fires_on_content_too_wide_for_a_scroller_that_only_scrolls_down()
    {
        var complaints = WpfTestHost.InvokeSettled(() =>
        {
            var viewer = ScrollsDownOnly(new Grid { Children = { new Border { Width = 400, Height = 40 } } });
            LayoutProbe.Settle(viewer, 200, 200);
            return LayoutRules.SpillsOutOfItsParent(LayoutProbe.Walk(viewer));
        });

        Assert.NotEmpty(complaints);
    }

    /// <summary>The off-surface rule had the same one flag: content past the SIDE of the surface, in a scroller
    /// that only scrolls down, is nowhere, not below the fold.</summary>
    [Fact]
    public void The_off_surface_rule_fires_on_content_past_the_side_of_a_scroller_that_only_scrolls_down()
    {
        var complaints = WpfTestHost.InvokeSettled(() =>
        {
            var canvas = new Canvas();
            var stray = new TextBlock { Text = "off the side", Width = 80, Height = 20 };
            Canvas.SetLeft(stray, 2000);
            canvas.Children.Add(stray);
            var viewer = ScrollsDownOnly(canvas);
            LayoutProbe.Settle(viewer, 200, 200);
            return LayoutRules.OutsideTheSurface(LayoutProbe.Walk(viewer), new Size(200, 200));
        });

        Assert.NotEmpty(complaints);
    }

    /// <summary>The exemption the rule keeps, in the axis it belongs to: content taller than a scroller that
    /// scrolls down is content to scroll to.</summary>
    [Fact]
    public void The_spill_rule_stays_quiet_on_content_too_tall_for_a_scroller_that_scrolls_down()
    {
        var complaints = WpfTestHost.InvokeSettled(() =>
        {
            var viewer = ScrollsDownOnly(new Grid { Children = { new Border { Width = 150, Height = 900 } } });
            LayoutProbe.Settle(viewer, 200, 200);
            return LayoutRules.SpillsOutOfItsParent(LayoutProbe.Walk(viewer));
        });

        Assert.Empty(complaints);
    }

    /// <summary>
    /// 🔴 A control cut in PART by an edge that does not scroll, the shape the parameter row had (PR A
    /// finding (a)): the hard-clip rule saw only a control gone entirely, so a list missing its chevron
    /// passed. One finding for the list, not one more for the drop-down button in its template.
    /// </summary>
    [Fact]
    public void The_partial_clip_rule_fires_on_a_control_cut_by_a_viewport_that_does_not_scroll_sideways()
    {
        var (partly, entirely) = WpfTestHost.InvokeSettled(() =>
        {
            var row = new StackPanel { Orientation = Orientation.Horizontal };
            row.Children.Add(new Button { Content = "Fill", Width = 150 });
            row.Children.Add(new ComboBox { Width = 150 });
            var viewer = ScrollsDownOnly(row);
            LayoutProbe.Settle(viewer, 200, 200);
            var walk = LayoutProbe.Walk(viewer);
            return (LayoutRules.PartlyPastAHardClip(walk), LayoutRules.PastAHardClip(walk));
        });

        var cut = Assert.Single(partly);
        Assert.Contains("ComboBox", cut, StringComparison.Ordinal);
        Assert.DoesNotContain(entirely, c => c.Contains("ComboBox", StringComparison.Ordinal));
    }

    /// <summary>
    /// The application's own date field is one control too, not its box and its calendar button apart -
    /// the calendar button is exactly the part a cut date field lost (R4/19 Z1). Found in review: each part
    /// used to be a finding of its own. Reversal probe: take DateInput out of the probe's list of controls
    /// and this finds the box instead.
    /// </summary>
    [Fact]
    public void The_partial_clip_rule_finds_a_cut_date_field_once_and_by_its_name()
    {
        var partly = WpfTestHost.InvokeSettled(() =>
        {
            var row = new StackPanel { Orientation = Orientation.Horizontal };
            row.Children.Add(new Button { Content = "Fill", Width = 100 });
            row.Children.Add(new ChronoMock.App.Controls.DateInput());
            var viewer = ScrollsDownOnly(row);
            LayoutProbe.Settle(viewer, 200, 200);
            return LayoutRules.PartlyPastAHardClip(LayoutProbe.Walk(viewer));
        });

        var cut = Assert.Single(partly);
        Assert.StartsWith("DateInput", cut, StringComparison.Ordinal);
    }

    /// <summary>The same row in a viewport that DOES scroll sideways is a row to scroll along, and a control
    /// wholly inside a clipping canvas is not cut at all.</summary>
    [Fact]
    public void The_partial_clip_rule_stays_quiet_where_the_rest_of_the_control_can_be_reached()
    {
        var complaints = WpfTestHost.InvokeSettled(() =>
        {
            var row = new StackPanel { Orientation = Orientation.Horizontal };
            row.Children.Add(new Button { Content = "Fill", Width = 150 });
            row.Children.Add(new ComboBox { Width = 150 });
            var viewer = new ScrollViewer { Content = row, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto };
            var canvas = new Canvas { Width = 100, Height = 100, ClipToBounds = true };
            var inside = new Button { Content = "In", Width = 40, Height = 20 };
            Canvas.SetLeft(inside, 10);
            canvas.Children.Add(inside);
            var host = new StackPanel { Children = { viewer, canvas } };
            LayoutProbe.Settle(host, 200, 400);
            return LayoutRules.PartlyPastAHardClip(LayoutProbe.Walk(host));
        });

        Assert.Empty(complaints);
    }

    /// <summary>And a clipping canvas that cuts a button in part is a finding too, not only a viewport.</summary>
    [Fact]
    public void The_partial_clip_rule_fires_on_a_control_cut_by_a_clipping_ancestor()
    {
        var complaints = WpfTestHost.InvokeSettled(() =>
        {
            var canvas = new Canvas { Width = 100, Height = 100, ClipToBounds = true };
            var button = new Button { Content = "Half", Width = 80, Height = 20 };
            Canvas.SetLeft(button, 60);
            canvas.Children.Add(button);
            var host = new Grid { Width = 400, Height = 400 };
            host.Children.Add(canvas);
            LayoutProbe.Settle(host, 400, 400);
            return LayoutRules.PartlyPastAHardClip(LayoutProbe.Walk(host));
        });

        Assert.Single(complaints);
    }
}
