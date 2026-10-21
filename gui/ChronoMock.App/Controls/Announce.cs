using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Threading;

namespace ChronoMock.App.Controls;

/// <summary>
/// Tells a screen reader what changed where the reader is not looking: a session's status, a verdict, a
/// calculator result, an error (R5-U1, R5-D83). The bound value is the NEWS. When it changes while the
/// element shows, the element's own text is announced after a quiet moment, and a change while it is hidden
/// is announced when it shows. <see cref="PoliteProperty"/> waits for the reader to finish,
/// <see cref="AssertiveProperty"/> interrupts it and is kept for something that went wrong.
/// </summary>
/// <remarks>
/// Showing alone announces nothing, on purpose. The phases are fixed instances switched by visibility, so
/// coming back from the calculator would read the verdict a second time, and a verdict therefore binds the
/// flag that marks entering its phase - two results in a row can read the same, and the second is news too.
///
/// The quiet moment (<see cref="Quiet"/>) keeps the calculator from reading a result for every key of a
/// number being typed: a run of changes is one announcement, of where the run ended.
///
/// What a reader hears is the element's automation name - for a text block all of its text, runs included -
/// so the announcement is what is on the screen, never a second wording of it that could drift.
/// </remarks>
public static class Announce
{
    /// <summary>Read this element when the value changes, after whatever the reader is saying.</summary>
    public static readonly DependencyProperty PoliteProperty = DependencyProperty.RegisterAttached(
        "Polite", typeof(object), typeof(Announce),
        new PropertyMetadata(null, (element, _) => News(element, AutomationLiveSetting.Polite)));

    public static object? GetPolite(DependencyObject element) => element.GetValue(PoliteProperty);

    public static void SetPolite(DependencyObject element, object? value) => element.SetValue(PoliteProperty, value);

    /// <summary>Read this element when the value changes, cutting in - for what went wrong.</summary>
    public static readonly DependencyProperty AssertiveProperty = DependencyProperty.RegisterAttached(
        "Assertive", typeof(object), typeof(Announce),
        new PropertyMetadata(null, (element, _) => News(element, AutomationLiveSetting.Assertive)));

    public static object? GetAssertive(DependencyObject element) => element.GetValue(AssertiveProperty);

    public static void SetAssertive(DependencyObject element, object? value) => element.SetValue(AssertiveProperty, value);

    /// <summary>How long the news stays unchanged before it is read.</summary>
    internal static readonly TimeSpan Quiet = TimeSpan.FromMilliseconds(500);

    /// <summary>Raised with the text of every announcement, whether or not a reader listens. A test process
    /// has no UI Automation client, so the raise itself is skipped there, and this is what a test reads.</summary>
    internal static event EventHandler<string>? Announced;

    private static readonly DependencyProperty HeraldProperty = DependencyProperty.RegisterAttached(
        "Herald", typeof(Herald), typeof(Announce), new PropertyMetadata(null));

    /// <summary>The element has news in this tone - for a control that announces itself, as the copy button
    /// does when it says what it did.</summary>
    internal static void News(DependencyObject element, AutomationLiveSetting tone)
    {
        if (element is not FrameworkElement shown)
        {
            return;
        }

        AutomationProperties.SetLiveSetting(shown, tone);
        if (shown.GetValue(HeraldProperty) is not Herald herald)
        {
            herald = new Herald(shown);
            shown.SetValue(HeraldProperty, herald);
        }

        herald.News();
    }

    /// <summary>One element's news waiting to be read.</summary>
    private sealed class Herald
    {
        private readonly FrameworkElement _element;
        private readonly DispatcherTimer _quiet;
        private bool _waiting;

        public Herald(FrameworkElement element)
        {
            _element = element;
            _quiet = new DispatcherTimer(DispatcherPriority.Background, element.Dispatcher) { Interval = Quiet };
            _quiet.Tick += (_, _) => Read();
            element.IsVisibleChanged += (_, _) => OnVisibleChanged();
        }

        public void News()
        {
            _waiting = true;
            if (_element.IsVisible)
            {
                Restart();
            }
        }

        private void OnVisibleChanged()
        {
            if (!_element.IsVisible)
            {
                _quiet.Stop();
            }
            else if (_waiting)
            {
                Restart();
            }
        }

        private void Restart()
        {
            _quiet.Stop();
            _quiet.Start();
        }

        private void Read()
        {
            _quiet.Stop();
            if (!_element.IsVisible)
            {
                return;
            }

            _waiting = false;
            var peer = UIElementAutomationPeer.CreatePeerForElement(_element);
            var said = peer?.GetName();
            if (peer is null || string.IsNullOrWhiteSpace(said))
            {
                return;
            }

            Announced?.Invoke(_element, said);
            if (AutomationPeer.ListenerExists(AutomationEvents.LiveRegionChanged))
            {
                peer.RaiseAutomationEvent(AutomationEvents.LiveRegionChanged);
            }
        }
    }
}
