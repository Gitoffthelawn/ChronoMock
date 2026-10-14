using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// The links in the title bar, measured on the laid-out window rather than read off the markup: where they
/// stand, whether the window's narrowest width still holds them, and whether the mark beside a word is a
/// glyph its face can actually draw.
/// </summary>
/// <remarks>
/// The strip had no test of its own until the repository link joined it, and three controls beside the
/// window buttons are exactly where a later change would push one under the caption buttons without
/// anybody seeing it - a render at the default width hides that, the floor does not.
/// </remarks>
public class TitleBarActionTests
{
    private static readonly string[] InOrder = ["SupportButton", "RepositoryButton", "AboutButton"];

    [Fact]
    public void The_links_stand_in_order_and_all_of_them_fit_at_the_narrowest_window()
    {
        var (width, placed) = WpfTestHost.InvokeSettled(() =>
        {
            var (window, root, floor) = LaidOutAtTheFloor();
            var bounds = InOrder
                .Select(name => (Button)window.FindName(name))
                .Select(button => (button.Name, Bounds: button.TransformToAncestor(root).TransformBounds(new Rect(button.RenderSize))))
                .ToList();
            return (floor, bounds);
        });

        // Canary: a strip that never laid out has every button at zero size, and every assertion below
        // would pass over nothing.
        Assert.All(placed, p => Assert.True(p.Bounds.Width > 0 && p.Bounds.Height > 0, $"{p.Name} has no size"));

        for (int i = 1; i < placed.Count; i++)
        {
            Assert.True(
                placed[i - 1].Bounds.Right <= placed[i].Bounds.Left,
                $"{placed[i - 1].Name} ends at {placed[i - 1].Bounds.Right:F1} and {placed[i].Name} starts at {placed[i].Bounds.Left:F1}");
        }

        Assert.All(placed, p => Assert.True(
            p.Bounds.Left >= 0 && p.Bounds.Right <= width,
            $"{p.Name} spans {p.Bounds.Left:F1} to {p.Bounds.Right:F1} in a window {width:F0} wide"));
    }

    [Fact]
    public void The_repository_mark_is_a_glyph_its_face_can_draw()
    {
        var (family, text, drawn, known) = WpfTestHost.InvokeSettled(() =>
        {
            var (window, _, _) = LaidOutAtTheFloor();
            var mark = Marks((Button)window.FindName("RepositoryButton")).Single();
            var typeface = new Typeface(mark.FontFamily, mark.FontStyle, mark.FontWeight, mark.FontStretch);
            var covered = typeface.TryGetGlyphTypeface(out var glyphs)
                && mark.Text.Length > 0
                && mark.Text.All(c => glyphs.CharacterToGlyphMap.ContainsKey(c));
            return (mark.FontFamily.Source, mark.Text, mark.ActualWidth, covered);
        });

        // A code point the face does not carry draws as an empty box, or as nothing at all when WPF falls
        // back to a face without it, and either one still has a width. So the face is asked directly.
        Assert.True(known, $"the face {family} has no glyph for U+{(text.Length > 0 ? (int)text[0] : 0):X4}");
        Assert.True(drawn > 0, "the mark took no room in the laid-out strip");
    }

    [Fact]
    public void Both_links_keep_the_same_gap_between_the_mark_and_the_word()
    {
        var gaps = WpfTestHost.InvokeSettled(() =>
        {
            var (window, root, _) = LaidOutAtTheFloor();
            return new[] { "SupportButton", "RepositoryButton" }.Select(name =>
            {
                var button = (Button)window.FindName(name);
                var blocks = Descendants(button).OfType<TextBlock>().ToList();
                var mark = blocks[0].TransformToAncestor(root).TransformBounds(new Rect(blocks[0].RenderSize));
                var word = blocks[1].TransformToAncestor(root).TransformBounds(new Rect(blocks[1].RenderSize));
                return (Name: name, Blocks: blocks.Count, Gap: word.Left - mark.Right);
            }).ToList();
        });

        Assert.All(gaps, g => Assert.Equal(2, g.Blocks));
        Assert.All(gaps, g => Assert.True(g.Gap > 0, $"{g.Name} sets its word against its mark ({g.Gap:F1})"));
        Assert.Equal(gaps[0].Gap, gaps[1].Gap, precision: 3);
    }

    [Fact]
    public void A_disabled_action_is_drawn_as_disabled()
    {
        var (enabled, disabled, token) = WpfTestHost.InvokeSettled(() =>
        {
            Button Laid(bool on)
            {
                var button = new Button
                {
                    Style = (Style)Application.Current.Resources["TitleBarActionButton"],
                    Content = "About",
                    IsEnabled = on,
                };
                button.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
                button.Arrange(new Rect(button.DesiredSize));
                button.UpdateLayout();
                return button;
            }

            return (Laid(true).Opacity, Laid(false).Opacity, (double)Application.Current.Resources["OpacityDisabled"]);
        });

        // Read off the styled control after its triggers ran, not off the style: a trigger that never
        // fires leaves the setter in the file and the button looking clickable.
        Assert.Equal(1.0, enabled, precision: 3);
        Assert.Equal(token, disabled, precision: 3);
        Assert.True(disabled < enabled, "the disabled action is drawn exactly like the enabled one");
    }

    /// <summary>The window's content laid out at its declared floor, the narrowest a user can drag it to.</summary>
    private static (MainWindow Window, FrameworkElement Root, double Width) LaidOutAtTheFloor()
    {
        // 🔴 An unshown Window has no HWND, so what is laid out is the CONTENT, measured and arranged by
        // hand - the same reason BackgroundGuardTests gives.
        var window = new MainWindow();
        var root = (FrameworkElement)window.Content;
        var width = (double)Application.Current.Resources["WindowMinWidth"];
        var height = (double)Application.Current.Resources["WindowDefaultHeight"];
        root.Measure(new Size(width, height));
        root.Arrange(new Rect(0, 0, width, height));
        root.UpdateLayout();
        return (window, root, width);
    }

    /// <summary>The marks inside a link: the text blocks drawn in the icon face.</summary>
    private static IEnumerable<TextBlock> Marks(Button button)
    {
        var icons = (FontFamily)Application.Current.Resources["FontFamilyIcons"];
        return Descendants(button).OfType<TextBlock>().Where(t => t.FontFamily.Source == icons.Source);
    }

    private static IEnumerable<DependencyObject> Descendants(DependencyObject root)
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            yield return child;
            foreach (var deeper in Descendants(child))
            {
                yield return deeper;
            }
        }
    }
}
