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

    /// <summary>
    /// Two drops whose checks overlap are answered in the order they were MADE: the older one, whose answer
    /// comes last, must not become the target once a newer drop has replaced it.
    /// <para>
    /// The answers are handed in, so their order is the test's. The first version raced two real checks and
    /// went red on a CI runner: with no dispatcher to wait for, the older check finished and set the target
    /// before the test had made the second drop at all - an order the window itself never produces.
    /// </para>
    /// </summary>
    [Fact]
    public async Task Only_the_newest_drop_decides_the_target()
    {
        var vm = new SessionViewModel();
        var olderAnswer = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        var newerAnswer = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);

        var older = vm.DropTargetAsync(@"C:\apps\older.exe", olderAnswer.Task);
        var newer = vm.DropTargetAsync(@"C:\apps\newer.exe", newerAnswer.Task);
        newerAnswer.SetResult(false); // the newest names no file, which the window says out loud
        Assert.False(await newer);
        olderAnswer.SetResult(true); // the older one's file is there, and its answer comes last
        Assert.True(await older); // moot, so nothing to say about it

        Assert.Null(vm.TargetPath);
    }
}
