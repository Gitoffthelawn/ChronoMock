namespace ChronoMock.App;

/// <summary>
/// What a drop on the window offers as a target, judged by its SHAPE alone: exactly one file, named
/// <c>.exe</c>, or a shortcut, <c>.lnk</c>, whose program is read when it is dropped (R5-D80). Several
/// files, a folder or another format is not a target.
/// <para>
/// 🔴 By the name only, never by asking the disk (R4-N48). The window asks this on every mouse move of a
/// drag, and asking whether the file exists blocked for as long as an unreachable share takes to fail -
/// measured 21 s. Whether the file is really there is asked once, off the UI thread, when it is dropped
/// (<see cref="TargetIntake"/>).
/// </para>
/// </summary>
internal static class DroppedTarget
{
    /// <summary>The single program or shortcut a drop names, or null when it names anything else.</summary>
    internal static string? Candidate(object? dropped)
        => dropped is string[] { Length: 1 } paths
           && (paths[0].EndsWith(".exe", StringComparison.OrdinalIgnoreCase) || TargetIntake.IsShortcut(paths[0]))
            ? paths[0]
            : null;
}
