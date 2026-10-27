using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// Where the window keeps its own files - the history and the diagnostics log take the same answer
/// (<see cref="WritableFolder.ForApp"/>) - and how it knows the installer put it where it is.
/// </summary>
public sealed class DataFolderTests : IDisposable
{
    private readonly string _dir =
        Path.Combine(Path.GetTempPath(), "chrono-data-folder-tests", Guid.NewGuid().ToString("N"));

    public void Dispose()
    {
        if (Directory.Exists(_dir))
        {
            Directory.Delete(_dir, recursive: true);
        }
    }

    /// <summary>
    /// R5-D4, R5-D72. Every copy keeps its history and logs in the per-user folder, the portable one too: beside
    /// the executable of a copy on a shared folder they were everybody's sessions, with the arguments each was
    /// started with, in one file anyone with the folder could read. Reversal probe: answer the folder beside
    /// the executable again and the running window's folder is not the per-user one.
    /// </summary>
    [Theory]
    [InlineData("history")]
    [InlineData("logs")]
    public void Every_copy_keeps_its_files_in_the_per_user_folder(string name)
    {
        Assert.Equal(Path.Combine(@"Y:\user", "ChronoMock", name), WritableFolder.In(@"Y:\user", name));
        var perUser = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ChronoMock", name);
        Assert.Equal(perUser, WritableFolder.ForApp(name, fromVariable: null));
        Assert.Equal(perUser, WritableFolder.ForApp(name, fromVariable: string.Empty));
    }

    /// <summary>
    /// A full path in CHRONOMOCK_DATA_DIR holds the files in place of the per-user ChronoMock folder, named as
    /// the window will use it. Reversal probe: take the value as written and the folder keeps its "..".
    /// </summary>
    [Theory]
    [InlineData(@"Y:\data", "history", @"Y:\data\history")]
    [InlineData(@"Y:\data\", "logs", @"Y:\data\logs")]
    [InlineData(@"Y:\runs\..\data", "history", @"Y:\data\history")]
    [InlineData(@"\\server\share\data", "logs", @"\\server\share\data\logs")]
    public void The_variable_moves_the_files_to_the_folder_it_names(string value, string name, string expected)
        => Assert.Equal(expected, WritableFolder.ForApp(name, value));

    /// <summary>
    /// A value that is not a full path would be read against the folder the window was started from, which is
    /// where the empty per-user root used to put the files. Refused, with the variable and the value named.
    /// Reversal probe: accept any value and these become folders under the current one.
    /// </summary>
    [Theory]
    [InlineData("data")]
    [InlineData(@"runs\data")]
    [InlineData(@"Y:data")]
    [InlineData(@"\data")]
    [InlineData("   ")]
    [InlineData("\"Y:\\data\"")]
    public void A_variable_that_is_not_a_full_path_is_refused(string value)
    {
        var refusal = Assert.Throws<InvalidOperationException>(() => WritableFolder.ForApp("history", value));
        Assert.Contains(WritableFolder.Variable, refusal.Message, StringComparison.Ordinal);
        Assert.Contains($"\"{value}\"", refusal.Message, StringComparison.Ordinal);
    }

    /// <summary>
    /// Review round on #145. A full path with a character no folder name can have passes .NET's path calls (they
    /// stopped checking characters in Core 2.1), so the window came up and its logs and history failed later
    /// without naming the variable. Refused at start instead, in a folder and in a share's host alike. Reversal
    /// probes: drop either check and its values pass.
    /// </summary>
    [Theory]
    [InlineData(@"Y:\data|bad")]
    [InlineData(@"Y:\da*ta")]
    [InlineData(@"Y:\data\what?")]
    [InlineData(@"Y:\data\a<b")]
    [InlineData(@"Y:\data\stream:x")]
    [InlineData("Y:\\da\"ta")]
    [InlineData(@"\\ser|ver\share\data")]
    public void A_variable_with_a_character_no_folder_name_can_have_is_refused(string value)
    {
        var refusal = Assert.Throws<InvalidOperationException>(() => WritableFolder.ForApp("logs", value));
        Assert.Contains(WritableFolder.Variable, refusal.Message, StringComparison.Ordinal);
        Assert.Contains($"\"{value}\"", refusal.Message, StringComparison.Ordinal);
        Assert.Contains("character", refusal.Message, StringComparison.Ordinal);
    }

    /// <summary>
    /// The test run names its own folder before the first test (<see cref="TestDataFolder"/>), so no window a test
    /// builds reads or writes the files of the account that runs the tests. Reversal probes: stop reading the
    /// variable, or stop naming it for the run, and the window's folder is the account's own again.
    /// </summary>
    [Fact]
    public void The_test_run_keeps_the_windows_files_out_of_the_accounts_folder()
    {
        var account = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ChronoMock");

        Assert.Equal(Path.Combine(TestDataFolder.Root, "logs"), WritableFolder.ForApp("logs"));
        Assert.Equal(Path.Combine(TestDataFolder.Root, "history"), WritableFolder.ForApp("history"));
        Assert.False(WritableFolder.ForApp("history").StartsWith(account, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>
    /// Review round on #133. Windows answers an empty path for an account it gives no local application data
    /// folder, and a folder built on it is relative - the history and logs would land in whatever folder the
    /// window was started from. Refused instead. Reversal probe: drop the check and the answer is relative.
    /// </summary>
    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    public void An_empty_per_user_root_is_refused_rather_than_made_relative(string root)
        => Assert.Throws<InvalidOperationException>(() => WritableFolder.In(root, "history"));

    [Fact]
    public void A_portable_copy_with_the_installers_marker_beside_it_is_installed()
    {
        Directory.CreateDirectory(_dir);
        File.WriteAllText(Path.Combine(_dir, AppPaths.InstalledMarker), "laid down by the installer");

        Assert.True(AppPaths.IsInstalledLayout(portable: true, _dir));
    }

    [Fact]
    public void A_portable_copy_without_the_marker_is_portable()
    {
        Directory.CreateDirectory(_dir);

        Assert.False(AppPaths.IsInstalledLayout(portable: true, _dir));
    }

    /// <summary>A development build has no installer and no package, whatever lies beside it.</summary>
    [Fact]
    public void A_development_build_is_never_installed_even_beside_the_marker()
    {
        Directory.CreateDirectory(_dir);
        File.WriteAllText(Path.Combine(_dir, AppPaths.InstalledMarker), "laid down by the installer");

        Assert.False(AppPaths.IsInstalledLayout(portable: false, _dir));
    }

    /// <summary>A folder whose name is the marker's is not the marker.</summary>
    [Fact]
    public void A_folder_named_like_the_marker_does_not_make_a_copy_installed()
    {
        Directory.CreateDirectory(Path.Combine(_dir, AppPaths.InstalledMarker));

        Assert.False(AppPaths.IsInstalledLayout(portable: true, _dir));
    }
}
