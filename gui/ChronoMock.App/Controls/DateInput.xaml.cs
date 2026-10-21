using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;

namespace ChronoMock.App.Controls;

/// <summary>
/// The shared date input: an ISO date field with a calendar popup. Bound to any DataContext that exposes
/// <c>DateText</c> and <c>SelectedDate</c> (<see cref="ChronoMock.App.MomentField"/>, the scenario
/// parameter inputs), so every date on the screen is typed and picked the same way. <see cref="MomentInput"/>
/// is this control plus a time field.
/// </summary>
/// <remarks>
/// The calendar follows the toolkit's date picker (dotnet/wpf DatePicker.cs), which this control cannot be
/// (its text follows the OS locale). Two of the picker's hooks into the calendar are internal, so the same
/// moments are caught from outside: the release of the mouse on a day, and a key on a day.
/// </remarks>
public partial class DateInput : UserControl
{
    private string? _textBeforeOpen;

    public DateInput()
    {
        InitializeComponent();

        // Handled or not, as the calendar itself listens: the day button takes the release for its click.
        PickCalendar.AddHandler(MouseLeftButtonUpEvent, new MouseButtonEventHandler(OnCalendarMouseUp), handledEventsToo: true);
    }

    /// <summary>The name a screen reader announces for the field, for a consumer whose label is not a
    /// control the field can point at (the scenario parameters name theirs after the parameter). Unset,
    /// the field keeps the plain text box's own accessible name.</summary>
    public static readonly DependencyProperty AccessibleNameProperty = DependencyProperty.Register(
        nameof(AccessibleName), typeof(string), typeof(DateInput), new PropertyMetadata(null));

    public string? AccessibleName
    {
        get => (string?)GetValue(AccessibleNameProperty);
        set => SetValue(AccessibleNameProperty, value);
    }

    /// <summary>Open on the month of the date in the field and put the focus on its day. A calendar does not
    /// follow a date set from outside it - it opened on the current month whatever the field held, and its
    /// focus stood on today (R5-N6, measured). The text is kept for Escape.</summary>
    /// <remarks>The day the arrows move from is the calendar's own, apart from the month it shows: its
    /// template sets it to the month shown at the first opening, today, and a field with no date never
    /// moves it - measured, an arrow from the 15th of a January picked a day in the current October. The
    /// one public way to set it is coming back to the month view, which takes the date shown
    /// (Calendar.OnDisplayModeChanged), so the calendar goes there through the year view.</remarks>
    private void OnCalendarOpened(object? sender, EventArgs e)
    {
        _textBeforeOpen = DateField.Text;
        PickCalendar.ApplyTemplate();
        PickCalendar.SetCurrentValue(Calendar.DisplayModeProperty, CalendarMode.Year);
        PickCalendar.SetCurrentValue(Calendar.DisplayDateProperty, PickCalendar.SelectedDate ?? DateTime.Today);
        PickCalendar.SetCurrentValue(Calendar.DisplayModeProperty, CalendarMode.Month);
        PickCalendar.Focus();
    }

    /// <summary>A calendar closed with the focus in it gives the focus back to the button that opened it.</summary>
    private void OnCalendarClosed(object? sender, EventArgs e)
    {
        if (PickCalendar.IsKeyboardFocusWithin)
        {
            CalendarToggle.Focus();
        }
    }

    /// <summary>Escape puts back the TEXT the field held - not a date, since the field may have held a date
    /// half typed, which no date stands for and the date setter would leave overwritten. Enter or Space on a
    /// day takes that day - days are on screen only in the month view, and in the year view the key opens
    /// the month under it, as the calendar does. That key goes on to the day it was sent to, as the date
    /// picker's does, and takes the same day again (measured: nothing is left pressed and nothing opens).
    /// Escape is the calendar's own and stops here, so nothing behind it acts on the same key.</summary>
    private void OnCalendarKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape)
        {
            DateField.SetCurrentValue(TextBox.TextProperty, _textBeforeOpen);
            Close();
            e.Handled = true;
        }
        else if (e.Key is Key.Enter or Key.Space && e.OriginalSource is CalendarDayButton { DataContext: DateTime day })
        {
            PickCalendar.SetCurrentValue(Calendar.SelectedDateProperty, day);
            Close();
        }
    }

    private void OnCalendarMouseUp(object sender, MouseButtonEventArgs e)
    {
        for (var at = e.OriginalSource as DependencyObject; at is not null && at != PickCalendar; at = Up(at))
        {
            if (at is CalendarDayButton)
            {
                Close();
                return;
            }
        }
    }

    // A text run under the pointer is not a visual, and the visual tree refuses to walk it.
    private static DependencyObject? Up(DependencyObject at)
        => at is Visual ? VisualTreeHelper.GetParent(at) : LogicalTreeHelper.GetParent(at);

    private void Close() => CalendarToggle.IsChecked = false;
}
