using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Runtime.InteropServices;

namespace ChronoMock.App.Tests;

/// <summary>
/// Reading a real shortcut through the shell (R5-D80). The shortcuts are made by Windows Script Host, a
/// different writer from the reader under test, so a reader that misplaced a field in its interface would not
/// agree with itself and pass.
/// </summary>
/// <remarks>Not covered here: an installer's advertised shortcut, which only an installer makes. The reader
/// refuses one by the SLDF_HAS_DARWINID flag, read through the same data list interface - unmeasured on a real
/// one.</remarks>
public sealed class ShellShortcutTests
{
    [Fact]
    public void A_shortcut_gives_its_program_its_arguments_and_its_folder()
    {
        var program = Path.Combine(Path.GetTempPath(), $"chrono-target-{Guid.NewGuid():N}.exe");
        File.WriteAllText(program, string.Empty);
        var lnk = MakeShortcut(program, "--level \"3 and 4\"", Path.GetTempPath());
        try
        {
            var read = WpfTestHost.Invoke(() => ShellShortcut.Read(lnk));

            Assert.NotNull(read);
            Assert.Equal(program, read.Program, ignoreCase: true);
            Assert.Equal("--level \"3 and 4\"", read.Arguments);
            Assert.Equal(Path.TrimEndingDirectorySeparator(Path.GetTempPath()), Path.TrimEndingDirectorySeparator(read.WorkingFolder), ignoreCase: true);
        }
        finally
        {
            File.Delete(lnk);
            File.Delete(program);
        }
    }

    /// <summary>The path the window takes: the intake reads the shortcut on a pool thread, outside the
    /// window's apartment, and fills the target, its arguments and its folder (found in review: the reads
    /// above all ran on the test's single-threaded apartment).</summary>
    [Fact]
    public async Task A_real_shortcut_offered_to_the_intake_fills_the_form()
    {
        var program = Path.Combine(Path.GetTempPath(), $"chrono-target-{Guid.NewGuid():N}.exe");
        File.WriteAllText(program, string.Empty);
        var lnk = MakeShortcut(program, "--fast", Path.GetTempPath());
        try
        {
            var vm = new SessionViewModel();

            Assert.Equal(TargetTaken.Taken, await new TargetIntake(vm).TakeAsync(lnk));

            Assert.Equal(program, vm.TargetPath, ignoreCase: true);
            Assert.Equal("--fast", vm.TargetArgs);
            Assert.True(vm.HasTargetNote);
        }
        finally
        {
            File.Delete(lnk);
            File.Delete(program);
        }
    }

    [Fact]
    public void A_file_that_is_no_shortcut_is_not_read()
    {
        var lnk = Path.Combine(Path.GetTempPath(), $"chrono-fake-{Guid.NewGuid():N}.lnk");
        File.WriteAllText(lnk, "not a shell link");
        try
        {
            Assert.Null(WpfTestHost.Invoke(() => ShellShortcut.Read(lnk)));
        }
        finally
        {
            File.Delete(lnk);
        }
    }

    [Fact]
    public void A_shortcut_that_is_not_there_is_not_read()
        => Assert.Null(WpfTestHost.Invoke(() => ShellShortcut.Read(Path.Combine(Path.GetTempPath(), $"chrono-gone-{Guid.NewGuid():N}.lnk"))));

    private static string MakeShortcut(string program, string arguments, string folder)
    {
        var lnk = Path.Combine(Path.GetTempPath(), $"chrono-{Guid.NewGuid():N}.lnk");
        WpfTestHost.Invoke(() =>
        {
            dynamic shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell", throwOnError: true)!)!;
            dynamic link = shell.CreateShortcut(lnk);
            link.TargetPath = program;
            link.Arguments = arguments;
            link.WorkingDirectory = folder;
            link.Save();
            Marshal.FinalReleaseComObject(link);
            Marshal.FinalReleaseComObject(shell);
            return 0;
        });
        return lnk;
    }
}
