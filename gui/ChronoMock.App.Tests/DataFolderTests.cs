using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// Where the window keeps its own files - the history and the diagnostics log ask the same question
/// (<see cref="WritableFolder.Choose"/>) - and how it knows the installer put it where it is.
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

    [Theory]
    [InlineData("history")]
    [InlineData("logs")]
    public void A_portable_copy_keeps_its_files_beside_the_executable_when_that_folder_takes_a_write(string name)
        => Assert.Equal(
            Path.Combine(@"X:\exe", name),
            WritableFolder.Choose(name, @"X:\exe", @"Y:\user", installed: false, _ => true));

    [Theory]
    [InlineData("history")]
    [InlineData("logs")]
    public void A_portable_copy_on_a_read_only_medium_keeps_them_in_the_per_user_folder(string name)
        // A USB stick or a read-only share cannot hold them next to the exe, so they go to a per-user location
        // instead of every session reporting a write error.
        => Assert.Equal(
            Path.Combine(@"Y:\user", "ChronoMock", name),
            WritableFolder.Choose(name, @"X:\exe", @"Y:\user", installed: false, _ => false));

    /// <summary>
    /// An installed copy running as administrator found Program Files writable and kept its history there,
    /// beside the one the same person's ordinary window keeps. It must not even ask: the probe creates the
    /// folder it asks about, and the uninstaller removes only what it installed.
    /// </summary>
    [Theory]
    [InlineData("history")]
    [InlineData("logs")]
    public void An_installed_copy_keeps_them_in_the_per_user_folder_without_asking_the_install_folder(string name)
    {
        var asked = new List<string>();

        var chosen = WritableFolder.Choose(name, @"X:\exe", @"Y:\user", installed: true, dir =>
        {
            asked.Add(dir);
            return true;
        });

        Assert.Equal(Path.Combine(@"Y:\user", "ChronoMock", name), chosen);
        Assert.Empty(asked);
    }

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
