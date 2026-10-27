using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Runtime.CompilerServices;
using ChronoMock.App;

namespace ChronoMock.App.Tests;

/// <summary>
/// The window's own folder for the whole test run: a fresh folder under the temp path, named by
/// <see cref="WritableFolder.Variable"/> before the first test runs. Without it every window a test builds reads
/// the history of the account that runs the tests, and every fault a test raises on purpose leaves a diagnostics
/// log in that account's folder, where keeping only the newest logs can push out a real one.
/// </summary>
internal static class TestDataFolder
{
    internal static readonly string Root =
        Path.Combine(Path.GetTempPath(), "chrono-test-data", Guid.NewGuid().ToString("N"));

    [ModuleInitializer]
    internal static void NameItForTheRun()
    {
        Environment.SetEnvironmentVariable(WritableFolder.Variable, Root);
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Remove();
    }

    private static void Remove()
    {
        try
        {
            if (Directory.Exists(Root))
            {
                Directory.Delete(Root, recursive: true);
            }
        }
        catch (IOException)
        {
            // A file still open as the run ends - the folder stays in the temp path, outside the account's own.
        }
        catch (UnauthorizedAccessException)
        {
            // The same, for a file the system still holds.
        }
    }
}
