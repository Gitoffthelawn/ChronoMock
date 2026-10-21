using System.Windows;
using Microsoft.Win32;
using ChronoMock.App.Views;

namespace ChronoMock.App;

/// <summary>
/// The real <see cref="IShellInteraction"/>: it wraps the live window so the session's window-dependent
/// commands can open a picker, touch the clipboard, or ask a yes/no question. The window hands one to
/// <see cref="SessionCommands.AttachShell"/> once, so the view model never holds a UI type (GUI rules 15
/// and 16). The behaviour here is the released panel's, lifted from the code-behind handlers it replaces so
/// the pickers, the clipboard retry and the confirm dialog act exactly as they did.
/// </summary>
internal sealed class WindowShellInteraction : IShellInteraction
{
    private readonly Window _window;

    internal WindowShellInteraction(Window window) => _window = window;

    public string? PickExecutable()
    {
        // DereferenceLinks off (R5-D80): the dialog's default resolves a picked shortcut to its program and
        // drops the arguments and working folder on the floor without a word - the source of WPF's
        // CommonItemDialog never sets FOS_NODEREFERENCELINKS, whatever its documentation says. The shortcut
        // itself comes back, and TargetIntake reads all three.
        var dialog = new OpenFileDialog
        {
            Title = Text("target.dialog_title"),
            Filter = Text("target.dialog_filter"),
            CheckFileExists = true,
            DereferenceLinks = false,
        };

        return dialog.ShowDialog(_window) == true ? dialog.FileName : null;
    }

    public string? PickFolder(string? initialDirectory)
    {
        // Seeded with whatever is already typed, so browsing from a filled field starts where the tester was
        // rather than at the shell root. No Directory.Exists probe: that is filesystem I/O on the UI thread
        // and can block on a disconnected path, and the picker already falls back to its default location
        // when the seed does not resolve.
        var dialog = new OpenFolderDialog { Title = Text("launch.cwd_label") };
        if (!string.IsNullOrWhiteSpace(initialDirectory))
        {
            dialog.InitialDirectory = initialDirectory;
        }

        return dialog.ShowDialog(_window) == true ? dialog.FolderName : null;
    }

    public bool CopyToClipboard(string text) => ClipboardText.TrySet(text);

    public bool Confirm(string headingKey, string messageKey, string affirmativeKey)
        => MessageDialog.Ask(_window, Text(headingKey), Text(messageKey), Text(affirmativeKey));

    public void Tell(string headingKey, string messageKey) => MessageDialog.Tell(_window, Text(headingKey), Text(messageKey));

    public string Text(string key) => Application.Current?.TryFindResource(key) as string ?? key;
}
