using System.Globalization;

namespace ChronoMock.App.Localization;

/// <summary>
/// Filling a translated template, the one place that does it (R4-N46).
/// <para>
/// Translation files are loose files beside the window that anyone may edit, so a template can arrive
/// naming an argument that does not exist ({5} of three) or with a brace left open, and
/// <see cref="string.Format(IFormatProvider, string, object[])"/> throws on both. Thrown in the middle of
/// building a result, that took the result with it. A template with no placeholder at all is fine as it
/// is, because extra arguments are ignored when there is no hole to fill.
/// </para>
/// <para>
/// A translation that cannot be filled falls back to the DEFAULT language's template for the same key,
/// which says the same thing with the values in it. Only when that cannot be filled either does the
/// translation come back as written - a broken template on screen is ugly and honest, an exception is
/// neither.
/// </para>
/// <para>
/// Three places used to guard this on their own or not at all - the window title, the session summary,
/// and none of the three templates in the calculator's result. One rule written three ways drifts, so
/// they all come here now.
/// </para>
/// </summary>
internal static class TextFormat
{
    /// <summary>The template with its placeholders filled, or the template unchanged when it cannot be
    /// filled. For a template that has no key to fall back by. Invariant culture, like every other number
    /// this interface prints.</summary>
    public static string Fill(string template, params object?[] args)
    {
        ArgumentNullException.ThrowIfNull(template);
        return TryFill(template, args) ?? template;
    }

    /// <summary>The key translated by <paramref name="translate"/> and filled, falling back to the default
    /// language's template when the translation cannot be filled, and to the translation as written when
    /// neither can.</summary>
    public static string Translate(Func<string, string> translate, string key, params object?[] args)
    {
        ArgumentNullException.ThrowIfNull(translate);
        ArgumentNullException.ThrowIfNull(key);

        var template = translate(key);
        return TryFill(template, args)
            ?? (LocalizationService.DefaultTemplate(key) is { } fallback ? TryFill(fallback, args) : null)
            ?? template;
    }

    /// <summary>The same, translated into the interface's current language.</summary>
    public static string Translate(string key, params object?[] args)
        => Translate(TranslationKeyConverter.Resolve, key, args);

    private static string? TryFill(string template, object?[] args)
    {
        try
        {
            return string.Format(CultureInfo.InvariantCulture, template, args);
        }
        catch (FormatException)
        {
            return null;
        }
    }
}
