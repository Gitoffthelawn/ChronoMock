using System.Runtime.InteropServices;
using System.Windows;

namespace ChronoMock.App;

/// <summary>
/// Putting text on the clipboard, the one place that does it. The session's copies retried a held clipboard
/// and said when it failed, the calculator's tried once and said nothing (R5-D81), so a busy clipboard was a
/// failure on one screen and a silent click on the other.
/// </summary>
internal static class ClipboardText
{
    /// <summary>The text on the clipboard: true when it is there. Another process can hold the clipboard
    /// lock, so a refusal is tried once more before it is reported - never swallowed (rule 6).</summary>
    public static bool TrySet(string text)
    {
        for (int attempt = 0; attempt < 2; attempt++)
        {
            try
            {
                Clipboard.SetText(text);
                return true;
            }
            catch (ExternalException)
            {
                // The lock was held by another process - one more try, then false.
            }
        }

        return false;
    }
}
