using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App;

/// <summary>
/// Where this application's own files go - the one answer the history store and the diagnostics log both
/// take, so the two cannot end up in different places.
/// <para>
/// 🔴 Always the per-user folder, in every copy (R5-D4, R5-D72). A portable copy used to keep its history and
/// logs beside the executable whenever that folder took a write, and a copy on a shared folder or a stick
/// handed round then kept everybody's sessions - the applications they tested, with the arguments they
/// started them with - in one file anyone with the folder could read. The per-user folder is readable by
/// that user only. An installed copy already kept them there, because the installer owns its folder.
/// </para>
/// <para>
/// The question "can this folder take a write" went with the choice: it wrote a probe file to ask, and two
/// instances asking at once could disagree (R4-N49).
/// </para>
/// </summary>
internal static class WritableFolder
{
    /// <summary>The folder the running window keeps one kind of its own files in (<c>history</c>, <c>logs</c>).</summary>
    internal static string ForApp(string name)
        => In(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), name);

    /// <summary>The folder for one kind of the window's own files under a per-user root - separate so a test
    /// can ask it of a root it made.</summary>
    internal static string In(string perUserRoot, string name) => Path.Combine(perUserRoot, "ChronoMock", name);
}
