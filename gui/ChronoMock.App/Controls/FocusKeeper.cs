using System.Collections.Specialized;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace ChronoMock.App.Controls;

/// <summary>
/// Puts the keyboard focus back on the screen when the element holding it goes away (R5-U4, R5-D85): a step
/// removed with its own button, format rows an error took off, a button an error switched off. Set
/// <see cref="FallbackProperty"/> on the region - a list or a single element - and a focus that fell lands
/// on the row now standing where the lost one stood, else on an earlier row, else on the fallback.
/// </summary>
/// <remarks>
/// WPF does not lose such a focus, it drops it: the element that can no longer hold it hands it to its first
/// focusable ancestor, and with every layout host off the keyboard path (R5/7a) that is the column's scroll
/// viewer or the window itself - a stop with no name, measured, from which the next Tab starts over at the
/// top of the column.
///
/// A focus the reader moved is left alone: this acts only when the element that held it can no longer hold
/// it (gone from the screen, hidden or switched off) and only while the focus still lies where WPF dropped
/// it. A window that is not active has no keyboard focus at all, and nothing is taken there - taking it
/// would pull the window to the front.
///
/// The innermost region wins, so a list inside a region with its own fallback still gets its row first: WPF
/// tells the regions that the focus left from the inside out, and an outer region finds it back inside.
/// </remarks>
public static class FocusKeeper
{
    /// <summary>Where the focus goes when nothing in this region can take it back.</summary>
    public static readonly DependencyProperty FallbackProperty = DependencyProperty.RegisterAttached(
        "Fallback", typeof(UIElement), typeof(FocusKeeper), new PropertyMetadata(null, OnFallbackChanged));

    public static UIElement? GetFallback(DependencyObject element) => (UIElement?)element.GetValue(FallbackProperty);

    public static void SetFallback(DependencyObject element, UIElement? value) => element.SetValue(FallbackProperty, value);

    private static readonly DependencyProperty WatchProperty = DependencyProperty.RegisterAttached(
        "Watch", typeof(Watch), typeof(FocusKeeper), new PropertyMetadata(null));

    private static void OnFallbackChanged(DependencyObject element, DependencyPropertyChangedEventArgs e)
    {
        if (element is UIElement region && region.GetValue(WatchProperty) is null)
        {
            region.SetValue(WatchProperty, new Watch(region));
        }
    }

    /// <summary>What one region remembers of the element holding the focus inside it.</summary>
    private sealed class Watch
    {
        private readonly UIElement _region;
        private UIElement? _holder;
        private object? _row;
        private int _position = -1;

        public Watch(UIElement region)
        {
            _region = region;
            region.AddHandler(Keyboard.GotKeyboardFocusEvent, new KeyboardFocusChangedEventHandler(OnGotFocus), handledEventsToo: true);
            region.IsKeyboardFocusWithinChanged += (_, e) => OnFocusWithinChanged((bool)e.NewValue);
            if (region is ItemsControl list)
            {
                ((INotifyCollectionChanged)list.Items).CollectionChanged += OnRowsChanged;
            }
        }

        // The place a removed row had is the one it had when it went: rows above it may have gone since the
        // focus came to it (the mouse removes a step while the keyboard stays on another).
        private void OnRowsChanged(object? sender, NotifyCollectionChangedEventArgs e)
        {
            if (e.Action == NotifyCollectionChangedAction.Remove && _row is not null && e.OldItems?.Contains(_row) == true)
            {
                _position = e.OldStartingIndex;
            }
        }

        private void OnGotFocus(object sender, KeyboardFocusChangedEventArgs e)
        {
            _holder = e.NewFocus as UIElement;
            _row = null;
            _position = -1;
            if (_region is ItemsControl list && _holder is not null
                && ItemsControl.ContainerFromElement(list, _holder) is { } container)
            {
                _position = list.ItemContainerGenerator.IndexFromContainer(container);
                _row = list.ItemContainerGenerator.ItemFromContainer(container);
            }
        }

        private void OnFocusWithinChanged(bool within)
        {
            if (within || _holder is not { } holder || CanHold(holder) || Keyboard.FocusedElement is null)
            {
                return;
            }

            _region.Dispatcher.BeginInvoke(DispatcherPriority.Input, Land);
        }

        private static bool CanHold(UIElement element)
            => element.IsEnabled && element.IsVisible && element.Focusable && PresentationSource.FromVisual(element) is not null;

        private void Land()
        {
            // Only while the focus still lies where WPF dropped it - on something that holds this region.
            if (!_region.IsVisible || _region.IsKeyboardFocusWithin
                || Keyboard.FocusedElement is not Visual dropped || !dropped.IsAncestorOf(_region))
            {
                return;
            }

            if (_region is ItemsControl list && OntoRow(list))
            {
                return;
            }

            GetFallback(_region)?.Focus();
        }

        /// <summary>The first stop of the row now at the lost row's place, else of an earlier row.</summary>
        private bool OntoRow(ItemsControl list)
        {
            var at = _row is null ? -1 : list.Items.IndexOf(_row);
            var start = at >= 0 ? at : Math.Min(_position, list.Items.Count - 1);
            for (var i = start; i >= 0; i--)
            {
                if (list.ItemContainerGenerator.ContainerFromIndex(i) is UIElement row
                    && row.MoveFocus(new TraversalRequest(FocusNavigationDirection.First)))
                {
                    return true;
                }
            }

            return false;
        }
    }
}
