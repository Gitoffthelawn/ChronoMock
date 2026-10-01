using System.Security;
using System.Security.Principal;

namespace ChronoMock.App;

/// <summary>
/// Whether this window runs as administrator (docs/09 section 12.19). An application started from an
/// elevated window is elevated too, and WebView2 then ignores the setting the session reaches its pages
/// through, so the one option that answers that - writing a WebView2 value to the machine registry - can
/// only be used from such a window. The core reads its own token for the same question and is the one
/// that decides. This is what lets the window grey the option out and say why, before the tester ticks it.
/// <para>
/// The built-in group, by the enumeration and never by its name: a name is looked up as an ACCOUNT, which
/// answers false in an elevated session on a Windows that is not in English.
/// </para>
/// </summary>
internal static class ProcessElevation
{
    /// <summary>Read once - a process cannot change its token. A token that cannot be read is not
    /// elevated, which greys the option out rather than offering what the core would then refuse.</summary>
    public static bool IsElevated { get; } = Read();

    private static bool Read()
    {
        try
        {
            using var identity = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
        }
        catch (Exception ex) when (ex is SecurityException or UnauthorizedAccessException)
        {
            return false;
        }
    }
}
