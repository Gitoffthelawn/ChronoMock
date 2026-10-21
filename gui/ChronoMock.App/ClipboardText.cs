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
    /// lock, and WPF already waits it out - ten tries 100 ms apart (ClipboardCore, shared with Windows
    /// Forms) - before it throws, so one call is enough. A second one doubled the time the window stood still,
    /// found in review. A refusal is reported, never swallowed (rule 6).</summary>
    public static bool TrySet(string text)
    {
        try
        {
            Clipboard.SetText(text);
            return true;
        }
        catch (ExternalException)
        {
            return false;
        }
    }
}
