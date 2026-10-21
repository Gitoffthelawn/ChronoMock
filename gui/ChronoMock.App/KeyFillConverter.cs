using System.Globalization;
using System.Windows.Data;
using ChronoMock.App.Localization;

namespace ChronoMock.App;

/// <summary>
/// A translated template, named by the converter parameter, filled with the bound value: "Remove step {0}"
/// with a step's number, "Copy {0}" with a format's name. The view's way to a name that carries a value,
/// so a view model does not compose display text for a screen reader. Filling goes through
/// <see cref="TextFormat"/>, so a broken translation falls back rather than throwing in a binding (R4-N46).
/// </summary>
/// <remarks>Read when the binding updates, not when the language changes - like every composed text on
/// this interface, which is rebuilt rather than switched in flight (R5-U8).</remarks>
public sealed class KeyFillConverter : IValueConverter
{
    public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => parameter is string key ? TextFormat.Translate(key, value) : string.Empty;

    public object ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
