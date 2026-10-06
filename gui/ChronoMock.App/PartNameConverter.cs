using System.Globalization;
using System.Windows.Data;

namespace ChronoMock.App;

/// <summary>
/// The accessible name of one part of a composite input: the part's own word ("Amount", "Unit") from the
/// translation key in the converter parameter, led by the input's name when it has one ("Payment term,
/// Unit"). A screen reader moving through three controls of one input hears which input each belongs to,
/// and an input without a name of its own still names its parts rather than leaving them silent.
/// </summary>
public sealed class PartNameConverter : IValueConverter
{
    public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => Name(value as string, parameter as string ?? string.Empty);

    /// <summary>The part's word alone, or the input's name, a comma and the part's word.</summary>
    public static string Name(string? input, string partKey)
    {
        var part = TranslationKeyConverter.Resolve(partKey);
        return string.IsNullOrWhiteSpace(input) ? part : string.Create(CultureInfo.InvariantCulture, $"{input}, {part}");
    }

    public object ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
