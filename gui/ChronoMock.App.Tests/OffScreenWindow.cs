using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace ChronoMock.App.Tests;

/// <summary>
/// A real window for a test that needs keyboard focus, which only an element in a shown window can hold.
/// Off the screen and never activated, so a test run takes neither the screen nor the keyboard from whoever
/// sits at the machine - and WPF still moves and reports the focus in it (measured: an element focused with
/// the window inactive reports IsKeyboardFocused). Call on the UI thread (<see cref="WpfTestHost"/>).
/// </summary>
internal static class OffScreenWindow
{
    public static Window Show(FrameworkElement content, double width = LayoutProbe.WindowWidth, double height = LayoutProbe.WindowHeight)
    {
        var window = new Window
        {
            Content = content,
            ShowActivated = false,
            ShowInTaskbar = false,
            WindowStartupLocation = WindowStartupLocation.Manual,
            Left = -32000,
            Top = -32000,
            Width = width,
            Height = height,
        };
        window.Show();
        Settle();
        return window;
    }

    /// <summary>Run the queue down to idle: bindings, layout and the focus WPF re-evaluates at input
    /// priority after an element holding it goes away.</summary>
    public static void Settle() => Dispatcher.CurrentDispatcher.Invoke(() => { }, DispatcherPriority.ContextIdle);

    /// <summary>A key pressed on whatever holds the focus, routed as the keyboard routes it: the preview
    /// event through the tree first, then the key event, which a handled preview stops.</summary>
    public static void Press(Key key)
    {
        var focused = (Visual)Keyboard.FocusedElement;
        InputManager.Current.ProcessInput(
            new KeyEventArgs(Keyboard.PrimaryDevice, PresentationSource.FromVisual(focused), 0, key)
            {
                RoutedEvent = Keyboard.PreviewKeyDownEvent,
            });
        Settle();
    }

    /// <summary>Every element of a type under a root, in the visual tree.</summary>
    public static IEnumerable<T> All<T>(DependencyObject root)
        where T : DependencyObject
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is T found)
            {
                yield return found;
            }

            foreach (var deeper in All<T>(child))
            {
                yield return deeper;
            }
        }
    }
}
