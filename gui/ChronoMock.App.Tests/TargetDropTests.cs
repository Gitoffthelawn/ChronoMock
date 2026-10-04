using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App.Tests;

/// <summary>
/// An application dropped on the window (R4-N48). Whether the file exists used to be asked on the UI thread on
/// every mouse move of a drag, and on a share that has gone that question blocks for as long as the share
/// takes to fail - measured 21 s on an unreachable address, 1.1 s on a name that does not resolve. The drag now
/// looks at the name only, and the drop asks once, off the caller's thread.
/// </summary>
public class TargetDropTests
{
    /// <summary>A share on a host name that does not resolve: slow to fail (about a second here), never there.</summary>
    private static string OnAMissingShare() => $@"\\chrono-no-such-host-{Guid.NewGuid():N}\share\app.exe";

    /// <summary>The drag judges by the name and never asks the disk: a name with nothing behind it is still
    /// a candidate here, which is only true of a check that does not look.</summary>
    [Fact]
    public void The_drag_judges_a_drop_by_its_name_without_asking_the_disk()
    {
        var nowhere = OnAMissingShare();

        Assert.Equal(nowhere, DroppedTarget.Candidate(new[] { nowhere }));
        Assert.Null(DroppedTarget.Candidate(new[] { @"C:\apps\a.exe", @"C:\apps\b.exe" }));
        Assert.Null(DroppedTarget.Candidate(new[] { @"C:\apps\shortcut.lnk" }));
        Assert.Null(DroppedTarget.Candidate("not a file list"));
    }

    [Fact]
    public async Task A_dropped_application_that_exists_becomes_the_target()
    {
        var vm = new SessionViewModel();
        var exe = Path.Combine(Path.GetTempPath(), $"chrono-drop-{Guid.NewGuid():N}.exe");
        File.WriteAllText(exe, string.Empty);
        try
        {
            Assert.True(await vm.DropTargetAsync(exe));
            Assert.Equal(exe, vm.TargetPath);
        }
        finally
        {
            File.Delete(exe);
        }
    }

    [Fact]
    public async Task A_dropped_name_with_no_file_behind_it_is_refused()
    {
        var vm = new SessionViewModel();

        Assert.False(await vm.DropTargetAsync(Path.Combine(Path.GetTempPath(), $"chrono-gone-{Guid.NewGuid():N}.exe")));
        Assert.Null(vm.TargetPath);
    }

    [Fact]
    public async Task The_check_does_not_hold_up_the_caller()
    {
        var vm = new SessionViewModel();

        var drop = vm.DropTargetAsync(OnAMissingShare());

        Assert.False(drop.IsCompleted, "the existence check must not run on the caller's thread");
        Assert.False(await drop);
    }

    /// <summary>Two drops while a slow check runs are answered in the order they were MADE: the first one,
    /// answered first, must not become the target once a second drop has replaced it.</summary>
    [Fact]
    public async Task Only_the_newest_drop_decides_the_target()
    {
        var vm = new SessionViewModel();
        var first = Path.Combine(Path.GetTempPath(), $"chrono-drop-{Guid.NewGuid():N}.exe");
        File.WriteAllText(first, string.Empty);
        try
        {
            var older = vm.DropTargetAsync(first);
            var newer = vm.DropTargetAsync(OnAMissingShare());

            Assert.True(await older); // moot, so nothing to say about it
            Assert.False(await newer); // the newest names no file, which the window says out loud
            Assert.Null(vm.TargetPath);
        }
        finally
        {
            File.Delete(first);
        }
    }
}
