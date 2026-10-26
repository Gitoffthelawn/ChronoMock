using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.App.Tests;

/// <summary>
/// A file offered as the target: an application or a shortcut dropped on the window, or a shortcut picked
/// in the dialog (R4-N48, R5-D80). Whether the file exists used to be asked on the UI thread on every mouse
/// move of a drag, and on a share that has gone that question blocks for as long as the share takes to fail
/// - measured 21 s on an unreachable address, 1.1 s on a name that does not resolve. The drag now looks at
/// the name only, and the offer is looked at once, off the caller's thread.
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
        Assert.Equal(@"C:\apps\shortcut.lnk", DroppedTarget.Candidate(new[] { @"C:\apps\shortcut.lnk" }));
        Assert.Null(DroppedTarget.Candidate(new[] { @"C:\apps\a.exe", @"C:\apps\b.exe" }));
        Assert.Null(DroppedTarget.Candidate(new[] { @"C:\apps\run.bat" }));
        Assert.Null(DroppedTarget.Candidate("not a file list"));
    }

    [Fact]
    public async Task A_dropped_application_that_exists_becomes_the_target()
    {
        var vm = new SessionViewModel();
        var exe = TempProgram();
        try
        {
            Assert.Equal(TargetTaken.Taken, await new TargetIntake(vm).TakeAsync(exe));
            Assert.Equal(exe, vm.TargetPath);
            Assert.False(vm.HasTargetNote, "a program brings no arguments, so there is nothing to note");
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

        var taken = await new TargetIntake(vm).TakeAsync(Path.Combine(Path.GetTempPath(), $"chrono-gone-{Guid.NewGuid():N}.exe"));

        Assert.Equal(TargetTaken.Missing, taken);
        Assert.Null(vm.TargetPath);
    }

    [Fact]
    public async Task The_check_does_not_hold_up_the_caller()
    {
        var vm = new SessionViewModel();

        var drop = new TargetIntake(vm).TakeAsync(OnAMissingShare());

        Assert.False(drop.IsCompleted, "the existence check must not run on the caller's thread");
        Assert.Equal(TargetTaken.Missing, await drop);
    }

    /// <summary>
    /// Two drops whose looks overlap are answered in the order they were MADE: the older one, whose answer
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
        var intake = new TargetIntake(vm);
        var olderAnswer = new TaskCompletionSource<TargetIntake.Offer>(TaskCreationOptions.RunContinuationsAsynchronously);
        var newerAnswer = new TaskCompletionSource<TargetIntake.Offer>(TaskCreationOptions.RunContinuationsAsynchronously);

        var older = intake.TakeAsync(olderAnswer.Task);
        var newer = intake.TakeAsync(newerAnswer.Task);
        newerAnswer.SetResult(TargetIntake.Offer.Refused(TargetTaken.Missing)); // the newest names no file
        Assert.Equal(TargetTaken.Missing, await newer);
        olderAnswer.SetResult(TargetIntake.Offer.OfProgram(@"C:\apps\older.exe")); // there, and answered last
        Assert.Equal(TargetTaken.Taken, await older); // moot, so nothing to say about it

        Assert.Null(vm.TargetPath);
    }

    [Fact]
    public async Task A_shortcut_fills_the_target_its_arguments_and_its_folder_and_says_so()
    {
        var vm = new SessionViewModel();
        var exe = TempProgram();
        try
        {
            var intake = new TargetIntake(vm, _ => new ShortcutTarget(exe, "--level 3", @"C:\work"));

            Assert.Equal(TargetTaken.Taken, await intake.TakeAsync(@"C:\Users\tester\Desktop\app.lnk"));

            Assert.Equal(exe, vm.TargetPath);
            Assert.Equal("--level 3", vm.TargetArgs);
            Assert.Equal(@"C:\work", vm.WorkingFolder);
            Assert.Equal("setup.from_shortcut", vm.TargetNoteKey);
        }
        finally
        {
            File.Delete(exe);
        }
    }

    [Fact]
    public async Task The_note_goes_when_another_target_is_chosen()
    {
        var vm = new SessionViewModel();
        var exe = TempProgram();
        try
        {
            await new TargetIntake(vm, _ => new ShortcutTarget(exe, string.Empty, string.Empty)).TakeAsync(@"C:\app.lnk");
            Assert.True(vm.HasTargetNote);

            vm.SetTarget(@"C:\apps\other.exe");

            Assert.False(vm.HasTargetNote);
        }
        finally
        {
            File.Delete(exe);
        }
    }

    [Theory]
    [InlineData(@"C:\tools\run.bat")]
    [InlineData(@"C:\docs\notes.txt")]
    [InlineData("")]
    public async Task A_shortcut_to_anything_but_a_program_is_refused(string leadsTo)
    {
        var vm = new SessionViewModel();
        var intake = new TargetIntake(vm, _ => new ShortcutTarget(leadsTo, string.Empty, string.Empty));

        Assert.Equal(TargetTaken.NotAProgram, await intake.TakeAsync(@"C:\app.lnk"));
        Assert.Null(vm.TargetPath);
    }

    [Fact]
    public async Task A_shortcut_the_shell_cannot_read_is_refused()
    {
        var vm = new SessionViewModel();

        Assert.Equal(TargetTaken.NotAProgram, await new TargetIntake(vm, _ => null).TakeAsync(@"C:\app.lnk"));
        Assert.Null(vm.TargetPath);
    }

    [Fact]
    public async Task A_shortcut_whose_program_is_gone_is_said_to_be_missing()
    {
        var vm = new SessionViewModel();
        var gone = Path.Combine(Path.GetTempPath(), $"chrono-gone-{Guid.NewGuid():N}.exe");

        var taken = await new TargetIntake(vm, _ => new ShortcutTarget(gone, string.Empty, string.Empty)).TakeAsync(@"C:\app.lnk");

        Assert.Equal(TargetTaken.Missing, taken);
        Assert.Equal("target.program_missing", TargetIntake.RefusalKey(taken));
    }

    /// <summary>The command line (<c>ChronoMock.exe &lt;file&gt;</c>, "Open with") is judged by the rule a drop is:
    /// one file, a program or a shortcut. A relative name is read from the folder the window was started in,
    /// and a name no full path can be made of is no target rather than a crash at start.</summary>
    [Fact]
    public void The_command_line_offers_a_target_by_the_rule_a_drop_is_judged_by()
    {
        Assert.Equal(@"C:\work\app.exe", DroppedTarget.FromCommandLine(new[] { "app.exe" }, @"C:\work"));
        Assert.Equal(@"C:\apps\app.exe", DroppedTarget.FromCommandLine(new[] { @"..\apps\app.exe" }, @"C:\work"));
        Assert.Equal(@"D:\links\app.lnk", DroppedTarget.FromCommandLine(new[] { @"D:\links\app.lnk" }, @"C:\work"));
        Assert.Null(DroppedTarget.FromCommandLine(Array.Empty<string>(), @"C:\work"));
        Assert.Null(DroppedTarget.FromCommandLine(new[] { "a.exe", "b.exe" }, @"C:\work"));
        Assert.Null(DroppedTarget.FromCommandLine(new[] { "run.bat" }, @"C:\work"));
        Assert.Null(DroppedTarget.FromCommandLine(new[] { "app.exe" }, "not-a-full-path"));
        Assert.Null(DroppedTarget.FromCommandLine(new[] { "a\0b.exe" }, @"C:\work"));
    }

    /// <summary>The window's own way from its command line to the target: a program named there is taken, over
    /// the target the window opened with, and naming it starts nothing (rule 7).</summary>
    [Fact]
    public async Task A_program_named_on_the_command_line_becomes_the_target_and_nothing_starts()
    {
        var exe = TempProgram();
        try
        {
            var (target, idle) = await WpfTestHost.RunAsync(async () =>
            {
                var window = new MainWindow();
                await window.OfferStartupTargetAsync(new[] { exe });
                var session = (SessionViewModel)window.DataContext;
                return (session.TargetPath, session.IsIdle);
            });

            Assert.Equal(exe, target);
            Assert.True(idle, "naming a program fills the form, it never starts a session");
        }
        finally
        {
            File.Delete(exe);
        }
    }

    private static string TempProgram()
    {
        var exe = Path.Combine(Path.GetTempPath(), $"chrono-drop-{Guid.NewGuid():N}.exe");
        File.WriteAllText(exe, string.Empty);
        return exe;
    }
}
