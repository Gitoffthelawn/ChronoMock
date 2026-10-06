using System.Collections;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using ChronoMock.App.Calc;

namespace ChronoMock.App.Controls;

/// <summary>
/// The shared amount-and-unit input: an optional sign, a whole number and a unit. Bound to any DataContext
/// that exposes <c>Amount</c>, <c>Unit</c> and <c>Units</c>, plus <c>Sign</c> and <c>Signs</c> when
/// <see cref="HasSign"/> is set - the setup's relative moment, a calculator shift step and a scenario's
/// duration parameter - so every amount on the screen is typed and its unit picked the same way.
/// </summary>
public partial class AmountUnitInput : UserControl
{
    public AmountUnitInput() => InitializeComponent();

    /// <summary>The translation key of the sign picker's own word, for its name to a screen reader.</summary>
    public const string SignWordKey = "moment.relative_sign";

    /// <summary>The translation key of the amount field's own word.</summary>
    public const string AmountWordKey = "moment.relative_amount";

    /// <summary>The translation key of the unit list's own word.</summary>
    public const string UnitWordKey = "moment.relative_unit";

    /// <summary>Whether the sign picker leads the input. A scenario's duration parameter has no sign: the
    /// scenario says which way the amount goes.</summary>
    public static readonly DependencyProperty HasSignProperty = DependencyProperty.Register(
        nameof(HasSign), typeof(bool), typeof(AmountUnitInput), new PropertyMetadata(false));

    public bool HasSign
    {
        get => (bool)GetValue(HasSignProperty);
        set => SetValue(HasSignProperty, value);
    }

    /// <summary>The input's name for a screen reader, leading each part's own word ("Payment term,
    /// Amount"). Unset, the parts are named by their words alone.</summary>
    public static readonly DependencyProperty AccessibleNameProperty = DependencyProperty.Register(
        nameof(AccessibleName), typeof(string), typeof(AmountUnitInput), new PropertyMetadata(null));

    public string? AccessibleName
    {
        get => (string?)GetValue(AccessibleNameProperty);
        set => SetValue(AccessibleNameProperty, value);
    }

    /// <summary>
    /// 🔴 The unit list as wide as its longest label plus the drop-down's own chrome, on every measure.
    /// </summary>
    /// <remarks>
    /// The list had a width of its own, 128, picked to hold "the longest word plus its chevron", and the
    /// longest word outgrew it: "business days" needed 85 px and got 78, so the window read "90 business
    /// day". Each label is measured the way the list draws it - a text block in the list's face, size and
    /// formatting mode - in the language the window shows when it is measured, so a translation or a new
    /// unit sizes the list without anyone choosing a number. A first version stood a hidden copy of every
    /// label beside the list to widen its cell, and the item-name guard caught the copies reaching a screen
    /// reader as a second, unnamed list.
    /// </remarks>
    protected override Size MeasureOverride(Size constraint)
    {
        var widest = WidestLabel(UnitList.ItemsSource);
        if (widest > 0)
        {
            var chrome = (Thickness)FindResource("DropDownChrome");
            var width = Math.Ceiling(widest + chrome.Left + chrome.Right);
            if (!UnitList.MinWidth.Equals(width))
            {
                UnitList.MinWidth = width;
            }
        }

        return base.MeasureOverride(constraint);
    }

    private double WidestLabel(IEnumerable? units)
    {
        if (units is null)
        {
            return 0;
        }

        double widest = 0;
        foreach (var unit in units)
        {
            var probe = new TextBlock
            {
                Text = unit is UnitOption option ? option.DisplayText : unit?.ToString() ?? string.Empty,
                FontFamily = UnitList.FontFamily,
                FontSize = UnitList.FontSize,
                FontStyle = UnitList.FontStyle,
                FontWeight = UnitList.FontWeight,
                FontStretch = UnitList.FontStretch,
            };
            TextOptions.SetTextFormattingMode(probe, TextOptions.GetTextFormattingMode(UnitList));
            probe.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
            widest = Math.Max(widest, probe.DesiredSize.Width);
        }

        return widest;
    }
}
