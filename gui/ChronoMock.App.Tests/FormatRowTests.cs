using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using ChronoMock.App.Calc;
using ChronoMock.App.Localization;
using ChronoMock.App.Views;
using ChronoMock.Protocol;

namespace ChronoMock.App.Tests;

/// <summary>
/// PR B finding (d): a value to copy never reads as two.
/// </summary>
/// <remarks>
/// 🔴 The format list put each label in a column of its own, which left a value 92 px - at the calculator's
/// floor AND at the window's default size, because the result column stays at its floor until the window is
/// wider than that. ISO datetime needs 165, FILETIME 119 and RFC 1123 191, so "2026-01-01T00" sat over
/// ":00:00+00:00" and a FILETIME broke between two digits, while a comment on the column's floor said it
/// held "a FILETIME without breaking mid digit". The label stands over its value now.
/// Reversal probe: put the label back in a column beside the value and this reddens on three formats.
/// </remarks>
public sealed class FormatRowTests
{
    /// <summary>Eight formats today.</summary>
    private const int FormatsAtLeast = 8;

    [Fact]
    public async Task Every_format_value_stays_on_one_line_at_the_calculators_narrowest_in_every_language()
    {
        var (broken, values) = await WpfTestHost.RunAsync(async () =>
        {
            var found = new List<string>();
            var measured = int.MaxValue;
            try
            {
                foreach (var culture in LocalizationService.AvailableCultures())
                {
                    LocalizationService.Apply(Application.Current, culture);
                    var vm = new CalculatorViewModel(new FakeCalcEngine(args =>
                        args.Contains("--analyze") ? CalculatorSheetEngine.Answer(args) : Longest()));
                    var view = new CalculatorView { DataContext = vm };
                    await vm.EnsureComputedAsync();
                    var floor = (int)(double)view.FindResource("CalculatorMinWidth");
                    LayoutProbe.Settle(view, floor, Tall);

                    var list = (FrameworkElement)LayoutProbe.FindNamed(view, "FormatList")!;
                    var values = vm.Formats.Select(f => f.Value).ToHashSet(StringComparer.Ordinal);
                    var shown = TextBlocks(list).Where(t => values.Contains(t.Text)).ToList();
                    measured = Math.Min(measured, shown.Count);
                    found.AddRange(shown
                        .Where(v => v.ActualHeight > OneLine(v) + Tolerance)
                        .Select(v => $"{culture}: \"{v.Text}\" is {v.ActualWidth:F0} px wide and {v.ActualHeight:F0} tall, one line is {OneLine(v):F0}"));
                }
            }
            finally
            {
                LocalizationService.Apply(Application.Current, LocalizationService.DefaultCulture);
            }

            return (found, measured);
        });

        Assert.True(values >= FormatsAtLeast, $"only {values} format values were laid out");
        Assert.True(broken.Count == 0, string.Join("\n", broken));
    }

    /// <summary>Tall enough that nothing scrolls, so a complaint is about width alone.</summary>
    private const int Tall = 1600;

    /// <summary>A layout rounding step, not a second line.</summary>
    private const double Tolerance = 0.5;

    /// <summary>
    /// The longest value each format can carry: RFC 1123 is always 29 characters and ISO datetime 25, a
    /// FILETIME reaches 19 digits in the year 9999 and epoch milliseconds 15 characters before 1970.
    /// </summary>
    private static CalcResult Longest() => new(
        "chronomock.calc/1",
        new CalcMoment(
            "9999-12-31T23:59:59",
            0,
            "today",
            [],
            new CalcFormats(
                "9999-12-31",
                "9999-12-31T23:59:59-12:00",
                "12/31/9999",
                "31.12.9999",
                -62_135_596_800,
                -62_135_596_800_000,
                2_650_467_743_999_999_999,
                "Wed, 31 Dec 9999 23:59:59 GMT"),
            new CalcMetadata("Friday", 9999, 52, 52, 365, 4, false, 0, null, null),
            [],
            null,
            null,
            null,
            null),
        null);

    private static IEnumerable<TextBlock> TextBlocks(DependencyObject node)
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(node); i++)
        {
            var child = VisualTreeHelper.GetChild(node, i);
            if (child is TextBlock text)
            {
                yield return text;
            }

            foreach (var deeper in TextBlocks(child))
            {
                yield return deeper;
            }
        }
    }

    /// <summary>The height of one line of this text block's own face and size.</summary>
    private static double OneLine(TextBlock text)
    {
        var probe = new TextBlock
        {
            Text = text.Text,
            FontFamily = text.FontFamily,
            FontSize = text.FontSize,
            FontWeight = text.FontWeight,
            TextWrapping = TextWrapping.NoWrap,
        };
        probe.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
        return probe.DesiredSize.Height;
    }
}
