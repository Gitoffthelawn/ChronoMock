using System.Diagnostics;

namespace ChronoMock.Protocol;

/// <summary>
/// The way out of a one-shot child process that did not finish on its own terms, shared by the two clients
/// that run one (<see cref="CalcClient"/>, <see cref="LicenseClient"/>). Each of them carried its own copy of
/// the kill, and the second copy of the pipe observation was about to be written - so it lives here once.
/// </summary>
internal static class ChildProcesses
{
    /// <summary>How long the way out may wait for the pipe readers after the kill. A killed child closes its
    /// pipes at once, so this is spent only when the kill did not take - and the call must still return.</summary>
    internal static readonly TimeSpan CleanupBound = TimeSpan.FromSeconds(2);

    /// <summary>
    /// Kill the child and its tree. True when it is gone - killed now, or never started, or already exited -
    /// and false when the system refused, so the caller can say the child may still be running rather than
    /// report it stopped (review of #87).
    /// </summary>
    internal static bool KillQuietly(Process process)
    {
        try
        {
            process.Kill(entireProcessTree: true);
            return true;
        }
        catch (InvalidOperationException)
        {
            return true; // no process was ever started - there is nothing to stop
        }
        catch (Exception e) when (e is NotSupportedException or System.ComponentModel.Win32Exception)
        {
            return false;
        }
    }

    /// <summary>
    /// Await the pipe readers so none becomes an unobserved faulted task, for at most
    /// <see cref="CleanupBound"/>. Their outcome is worthless here (the call already failed), so every result
    /// and fault is discarded.
    /// <para>
    /// 🔴 Bounded, because a child the kill did not reach keeps its pipes open, and an unbounded wait on its
    /// readers held the caller past the very limit that started this cleanup (review of #87). Readers still
    /// running at the bound are observed when they end, so a late fault is not reported as an unobserved one.
    /// </para>
    /// </summary>
    internal static async Task ObserveQuietly(params Task[] readers)
    {
        var all = Task.WhenAll(readers);
        try
        {
            await all.WaitAsync(CleanupBound).ConfigureAwait(false);
        }
        catch
        {
            // Cancelled, faulted with the killed process, or still running at the bound - immaterial here.
        }

        if (!all.IsCompleted)
        {
            _ = all.ContinueWith(
                finished => _ = finished.Exception,
                CancellationToken.None,
                TaskContinuationOptions.OnlyOnFaulted | TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
        }
    }
}
