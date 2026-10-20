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
        Assert.Equal(
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ChronoMock", name),
            WritableFolder.ForApp(name));
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
