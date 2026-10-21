using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using ChronoMock.App.Controls;

namespace ChronoMock.App.Tests;

/// <summary>
/// The calendar of the shared date input from the keyboard (R5-U3, R5-D84), the way the toolkit's date
/// picker works. Measured before: opening left the focus on the button, the calendar showed the current
/// month whatever the field held (R5-N6), and the first arrow closed it and wrote a date the reader never
/// chose (R5-N5).
/// </summary>
public sealed class DateInputKeyboardTests
{
    private static readonly DateTime Today = DateTime.Today;

    [Fact]
    public void Opening_puts_the_focus_on_the_day_of_the_date_in_the_field_and_shows_its_month()
        => Open("2025-01-15", (calendar, _, _) =>
        {
            Assert.Equal(new DateTime(2025, 1, 15), FocusedDay());
            Assert.Equal((2025, 1), (calendar.DisplayDate.Year, calendar.DisplayDate.Month));
        });

    [Fact]
    public void An_arrow_moves_the_day_from_the_date_in_the_field_and_keeps_the_calendar_open()
        => Open("2025-01-15", (_, toggle, field) =>
        {
            OffScreenWindow.Press(Key.Right);

            Assert.True(toggle.IsChecked);
            Assert.Equal("2025-01-16", field.DateText);
            Assert.Equal(new DateTime(2025, 1, 16), FocusedDay());
        });

    /// <summary>A field with no date starts the calendar at today, also for the arrows - the calendar keeps
    /// the day they move from apart from the month it shows, and a date that never came does not move it.</summary>
    [Fact]
    public void An_arrow_from_a_field_with_no_date_moves_from_today()
        => Open(string.Empty, (_, toggle, field) =>
        {
            Assert.Equal(Today, FocusedDay());

            OffScreenWindow.Press(Key.Right);

            Assert.True(toggle.IsChecked);
            Assert.Equal(Today.AddDays(1).ToString("yyyy-MM-dd", System.Globalization.CultureInfo.InvariantCulture), field.DateText);
        });

    [Theory]
    [InlineData(Key.Enter)]
    [InlineData(Key.Space)]
    public void A_key_that_takes_a_day_closes_with_it_and_gives_the_focus_back_to_the_button(Key key)
        => Open("2025-01-15", (_, toggle, field) =>
        {
            OffScreenWindow.Press(Key.Right);
            OffScreenWindow.Press(key);

            Assert.False(toggle.IsChecked);
            Assert.Equal("2025-01-16", field.DateText);
            Assert.True(toggle.IsKeyboardFocused);
        });

    /// <summary>Taking a day without moving to it first takes the day the focus stands on - today, in a field
    /// with no date, where the calendar has nothing selected for the key to keep. Space in particular: the day
    /// button clicks on the key's release, which goes to the button the focus is back on.</summary>
    [Theory]
    [InlineData(Key.Enter)]
    [InlineData(Key.Space)]
    public void A_key_in_a_field_with_no_date_takes_the_day_the_focus_stands_on(Key key)
        => Open(string.Empty, (_, toggle, field) =>
        {
            OffScreenWindow.Press(key);

            Assert.False(toggle.IsChecked);
            Assert.Equal(Today.ToString("yyyy-MM-dd", System.Globalization.CultureInfo.InvariantCulture), field.DateText);
        });

    [Fact]
    public void Escape_closes_and_puts_back_the_date_the_field_held()
        => Open("2025-01-15", (_, toggle, field) =>
        {
            OffScreenWindow.Press(Key.Right);
            OffScreenWindow.Press(Key.Down);
            OffScreenWindow.Press(Key.Escape);

            Assert.False(toggle.IsChecked);
            Assert.Equal("2025-01-15", field.DateText);
            Assert.True(toggle.IsKeyboardFocused);
        });

    /// <summary>The text comes back, not a date: a field holding half a date has no date to restore, and the
    /// date the arrows wrote over it would stay.</summary>
    [Fact]
    public void Escape_puts_back_a_date_half_typed()
        => Open("2025-01", (_, _, field) =>
        {
            OffScreenWindow.Press(Key.Right);
            Assert.NotEqual("2025-01", field.DateText);

            OffScreenWindow.Press(Key.Escape);

            Assert.Equal("2025-01", field.DateText);
        });

    /// <summary>In the year view Enter opens the month under the focus, as the calendar does - it takes no
    /// day and closes nothing.</summary>
    [Fact]
    public void Enter_in_the_year_view_opens_the_month_and_keeps_the_calendar_open()
        => Open("2025-01-15", (calendar, toggle, field) =>
        {
            calendar.DisplayMode = CalendarMode.Year;
            Keyboard.Focus(OffScreenWindow.All<CalendarButton>(calendar).First(b => b.DataContext is DateTime { Month: 1 }));
            OffScreenWindow.Settle();

            OffScreenWindow.Press(Key.Enter);

            Assert.True(toggle.IsChecked);
            Assert.Equal(CalendarMode.Month, calendar.DisplayMode);
            Assert.Equal("2025-01-15", field.DateText);
        });

    [Fact]
    public void A_day_released_under_the_mouse_closes_the_calendar()
        => Open("2025-01-15", (calendar, toggle, _) =>
        {
            var day = OffScreenWindow.All<CalendarDayButton>(calendar).First(b => b.DataContext is DateTime d && d.Day == 20);

            ReleaseMouseOn(day);

            Assert.False(toggle.IsChecked);
        });

    [Fact]
    public void A_release_on_the_month_arrows_leaves_the_calendar_open()
        => Open("2025-01-15", (calendar, toggle, _) =>
        {
            var next = OffScreenWindow.All<Button>(calendar).Single(b => b.Name == "PART_NextButton");

            ReleaseMouseOn(next);

            Assert.True(toggle.IsChecked);
        });

    /// <summary>The release as the mouse sends it: the button-up event bubbling from the element, which WPF
    /// re-raises as the left button's release on every element on the way (handled ones too), so a handler
    /// on the calendar hears a release the day button has already taken for its click.</summary>
    private static void ReleaseMouseOn(UIElement element)
    {
        element.RaiseEvent(new MouseButtonEventArgs(Mouse.PrimaryDevice, 0, MouseButton.Left)
        {
            RoutedEvent = Mouse.MouseUpEvent,
        });
        OffScreenWindow.Settle();
    }

    private static DateTime? FocusedDay() => (Keyboard.FocusedElement as CalendarDayButton)?.DataContext as DateTime?;

    /// <summary>A date input over a field holding <paramref name="text"/>, its calendar opened from the
    /// keyboard: the focus on the button, then Enter (Space presses a button on its release, which this
    /// window's keys do not send).</summary>
    private static void Open(string text, Action<Calendar, ToggleButton, MomentField> check)
    {
        WpfTestHost.Invoke(() =>
        {
            var field = new MomentField { DateText = text };
            var input = new DateInput { DataContext = field };
            var window = OffScreenWindow.Show(new StackPanel { Children = { input } });
            try
            {
                var toggle = (ToggleButton)input.FindName("CalendarToggle");
                var calendar = (Calendar)input.FindName("PickCalendar");
                Keyboard.Focus(toggle);
                OffScreenWindow.Settle();
                OffScreenWindow.Press(Key.Enter);
                Assert.True(toggle.IsChecked, "the button did not open the calendar");

                check(calendar, toggle, field);
            }
            finally
            {
                window.Close();
            }

            return 0;
        });
    }
}
