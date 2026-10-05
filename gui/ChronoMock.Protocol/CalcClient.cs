using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace ChronoMock.Protocol;

/// <summary>Raised when <c>chrono calc</c> exits non-zero: the calculator surface reports a usage or
/// build error on stderr and returns a result only on success (docs/08 section 9a). Carries the exit
/// code so a caller can tell a usage error (1) from a not-built-yet operation (5).</summary>
public sealed class CalcException : Exception
{
    public int ExitCode { get; }

    public CalcException(string message, int exitCode)
        : base(message) => ExitCode = exitCode;
}

/// <summary>
/// Runs the calculator engine as a one-shot child process (<c>chrono calc &lt;args&gt; --json</c>) and
/// parses its <c>chronomock.calc/1</c> output. The GUI is a thin client of the same engine the CLI and
/// the substitution core use (ADR-6), so the date logic lives in one place (chrono-core), never
/// re-implemented in C#. Unlike <see cref="CoreClient"/> there is no session: each call is independent.
/// </summary>
public sealed class CalcClient : ICalcEngine
{
    private static readonly UTF8Encoding Utf8NoBom = new(encoderShouldEmitUTF8Identifier: false);

    /// <summary>How long one calc invocation may take before it is killed. Calc is pure computation and
    /// finishes in well under a second, so this is a safety net, not a budget: without it a core that
    /// cannot finish (a degenerate calendar used to spin forever - see the business-day walk) left the
    /// calculator frozen with no explanation and a CPU-burning orphan behind it.</summary>
    private static readonly TimeSpan CalcTimeout = TimeSpan.FromSeconds(10);

    private readonly Func<string> _chronoPath;
    private readonly string? _workingDirectory;

    /// <param name="workingDirectory">Where calc looks for <c>calendars/</c> and <c>presets/</c> (it checks
    /// <c>./calendars</c> and <c>&lt;exe&gt;/calendars</c>). Set it so <c>--calendar</c> / <c>--preset</c> resolve.</param>
    public CalcClient(Func<string> chronoPath, string? workingDirectory = null)
    {
        _chronoPath = chronoPath ?? throw new ArgumentNullException(nameof(chronoPath));
        _workingDirectory = workingDirectory;
    }

    /// <summary>Dev-checkout factory: the x64 core build (calc is pure computation, so its bitness does not
    /// matter), run from the repo root so its <c>calendars/</c> and <c>presets/</c> resolve. Named by its
    /// explicit target triple, like <see cref="CoreLocator.ForRepo"/> - <c>target/release/</c> is only
    /// written by a build without <c>--target</c>, so it goes stale silently (R2-X3).</summary>
    public static CalcClient ForRepo(string repoRoot)
        => new(
            () => Path.Combine(repoRoot, "target", "x86_64-pc-windows-msvc", "release", "chrono.exe"),
            repoRoot);

    /// <summary>Portable-install factory (the shipped layout, Stage 5): the x64 core at
    /// <paramref name="baseDir"/>/core/x64/chrono.exe (calc is pure computation, bitness does not matter),
    /// run from <paramref name="baseDir"/> so its root-level <c>calendars/</c> and <c>presets/</c> resolve
    /// via the <c>./</c> lookup.</summary>
    public static CalcClient ForPortable(string baseDir)
        => new(() => Path.Combine(baseDir, "core", "x64", "chrono.exe"), baseDir);

    /// <summary>
    /// Evaluate a calc invocation. <paramref name="calcArgs"/> are the flags after <c>calc</c> (e.g.
    /// <c>--base</c>, <c>--shift</c>, <c>--calendar</c>, <c>--analyze</c>) - <c>calc</c> and <c>--json</c>
    /// are added here. Throws <see cref="CalcException"/> on a non-zero exit (stderr as the message).
    /// </summary>
    public async Task<CalcResult> EvaluateAsync(IReadOnlyList<string> calcArgs, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(calcArgs);

        var exe = _chronoPath();
        var psi = new ProcessStartInfo
        {
            FileName = exe,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = Utf8NoBom,
            StandardErrorEncoding = Utf8NoBom,
        };
        psi.ArgumentList.Add("calc");
        foreach (var arg in calcArgs)
        {
            psi.ArgumentList.Add(arg);
        }

        psi.ArgumentList.Add("--json");
        if (_workingDirectory is not null)
        {
            psi.WorkingDirectory = _workingDirectory;
        }

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception e) when (e is not OperationCanceledException)
        {
            // Keyed like the timeout below, so a missing or quarantined engine reads in the interface's
            // language - with the path, which is the part somebody fixing the install needs (R4-S26). The
            // system's own reason only: the process message repeats the path and adds a working directory,
            // which on the rendered panel made one sentence of four lines.
            var reason = e is System.ComponentModel.Win32Exception native
                ? new System.ComponentModel.Win32Exception(native.NativeErrorCode).Message
                : e.Message;
            throw new CalcException($"cannot launch '{exe}': {reason} (calc.launch_failed)", -1);
        }

        // Bounded wait, and the child is killed on the way out either way. `using var process` disposes
        // the managed wrapper, NOT the running process - a cancelled call (every keystroke supersedes the
        // previous one) used to leave chrono.exe running, so a burst of typing left a pile of orphans.
        using var attempt = CancellationTokenSource.CreateLinkedTokenSource(ct);
        attempt.CancelAfter(CalcTimeout);

        // Drain both pipes concurrently before awaiting exit, so a full pipe buffer cannot deadlock. On the
        // attempt's token rather than the caller's alone, so the readers stop with the limit too.
        var stdoutTask = process.StandardOutput.ReadToEndAsync(attempt.Token);
        var stderrTask = process.StandardError.ReadToEndAsync(attempt.Token);
        string stdout;
        string stderr;
        try
        {
            await process.WaitForExitAsync(attempt.Token).ConfigureAwait(false);
            stdout = await stdoutTask.ConfigureAwait(false);
            stderr = await stderrTask.ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            var stopped = ChildProcesses.KillQuietly(process);
            await ChildProcesses.ObserveQuietly(stdoutTask, stderrTask).ConfigureAwait(false);
            if (ct.IsCancellationRequested)
            {
                throw; // the caller superseded this call - its own concern, not an error
            }

            // Carries a stable key in the engine's own shape, so the interface can translate this the way
            // it translates the engine's refusals. This library knows nothing about languages and must not
            // - the key is the seam that keeps it that way (rule 15). The detail says whether the process is
            // really gone: a kill the system refused leaves it running, and "stopped" would not be true.
            var outcome = stopped ? "was stopped" : "could not be stopped";
            throw new CalcException(
                $"calc did not finish within {CalcTimeout.TotalSeconds:0} s and {outcome} (calc.timeout)",
                -1);
        }

        if (process.ExitCode != 0)
        {
            var message = stderr.Trim();
            throw new CalcException(
                message.Length > 0 ? message : $"calc exited with code {process.ExitCode}", process.ExitCode);
        }

        CalcResult? result;
        try
        {
            result = JsonSerializer.Deserialize<CalcResult>(stdout, ProtocolJson.Options);
        }
        catch (JsonException e)
        {
            throw new CalcException($"calc output was not valid JSON: {e.Message}", 0);
        }

        return result ?? throw new CalcException("calc produced no JSON output", 0);
    }
}
