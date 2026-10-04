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
/// Three places used to guard this on their own or not at all - the window title, the session summary,
/// and none of the three templates in the calculator's result. One rule written three ways drifts, so
/// they all come here now.
/// </para>
/// </summary>
internal static class TextFormat
{
    /// <summary>The template with its placeholders filled, or the template unchanged when it cannot be
    /// filled. Invariant culture, like every other number this interface prints.</summary>
    public static string Fill(string template, params object?[] args)
    {
        ArgumentNullException.ThrowIfNull(template);

        try
        {
            return string.Format(CultureInfo.InvariantCulture, template, args);
        }
        catch (FormatException)
        {
            return template;
        }
    }
}
