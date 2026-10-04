using System.Diagnostics;

namespace ChronoMock.Protocol;

/// <summary>
/// The way out of a one-shot child process that did not finish on its own terms, shared by the two clients
/// that run one (<see cref="CalcClient"/>, <see cref="LicenseClient"/>). Each of them carried its own copy of
/// the kill, and the second copy of the pipe observation was about to be written - so it lives here once.
/// </summary>
internal static class ChildProcesses
{
    /// <summary>Kill the child and its tree, ignoring the races that make it moot (it exited on its own
    /// between the timeout and here, or was never started).</summary>
    internal static void KillQuietly(Process process)
    {
        try
        {
            process.Kill(entireProcessTree: true);
        }
        catch (Exception e) when (e is InvalidOperationException or NotSupportedException
                                      or System.ComponentModel.Win32Exception)
        {
            // Already gone, or the OS refused - either way there is nothing left to do about it.
        }
    }

    /// <summary>Await the pipe readers so none becomes an unobserved faulted task. Their outcome is worthless
    /// here (the call already failed), so every result and fault is discarded.</summary>
    internal static async Task ObserveQuietly(params Task[] readers)
    {
        try
        {
            await Task.WhenAll(readers).ConfigureAwait(false);
        }
        catch
        {
            // Cancelled or faulted with the killed process - immaterial, but must not go unobserved.
        }
    }
}
