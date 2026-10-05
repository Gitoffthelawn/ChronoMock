using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).

namespace ChronoMock.Protocol;

/// <summary>
/// Asks the core what is inside this build, by running <c>chrono license --components</c> and taking its
/// text as it comes.
/// <para>
/// The window could have carried its own list. It does not, and that is the point: the register is
/// compiled INTO the core with <c>include_str!</c> from the same <c>packaging/components.json</c> that
/// generates the release SBOM and that <c>THIRD-PARTY-NOTICES.md</c> is checked against. A second copy in
/// C# would be a fourth thing to keep in step, and the day it drifted the window would be stating
/// somebody else's build with a straight face.
/// </para>
/// <para>
/// Separate from <see cref="CalcClient"/> rather than folded into it. That one adds <c>calc</c> and
/// <c>--json</c> to every invocation and parses a result document, none of which applies here, and a
/// class whose name says calc is the wrong place to hide a licence query.
/// </para>
/// </summary>
public sealed class LicenseClient
{
    /// <summary>
    /// How long the query may take before the child is killed.
    /// </summary>
    /// <remarks>
    /// Generous for what this is. The core prints a string it was compiled with and exits, touching no
    /// file and no clock, so the only ways to spend seconds here are a machine under heavy load or an
    /// executable that is not the one we think it is. Either way a window must not hang on it.
    /// </remarks>
    private static readonly TimeSpan QueryTimeout = TimeSpan.FromSeconds(10);

    /// <summary>
    /// The first group heading in the core's component listing, and where its register actually starts.
    /// </summary>
    /// <remarks>
    /// <c>chrono license --components</c> prints the whole notice and THEN the register, which is right
    /// for somebody reading it in a terminal and wrong for a window that has already stated the notice in
    /// its own words: measured on the first render, every line of it appeared twice. Rather than widen the
    /// command-line contract with a flag for one caller, the prose is cut here at the heading the core
    /// itself writes - and <c>LicenseClientTests</c> mirrors this string against the Rust source, so the
    /// day somebody rewords that heading the test says so instead of the window quietly going long again.
    /// </remarks>
    public const string FirstGroupHeading = "In both packages, compiled into the native tool:";

    private readonly Func<string> _chronoPath;

    public LicenseClient(Func<string> chronoPath)
        => _chronoPath = chronoPath ?? throw new ArgumentNullException(nameof(chronoPath));

    /// <summary>
    /// The register alone, with the notice the core prints above it removed.
    /// </summary>
    /// <remarks>
    /// Falls back to the whole text when the heading is not found. A reader seeing the notice twice is a
    /// blemish, and a reader seeing an empty components list would be a lie about what is in the build
    /// (rule 4) - so when this cannot find its landmark it shows too much rather than too little.
    /// </remarks>
    public static string RegisterOnly(string listing)
    {
        ArgumentNullException.ThrowIfNull(listing);

        var start = listing.IndexOf(FirstGroupHeading, StringComparison.Ordinal);
        return start >= 0 ? listing[start..] : listing;
    }

    /// <summary>Dev-checkout factory: the x64 core build, matching <see cref="CalcClient.ForRepo"/>.</summary>
    public static LicenseClient ForRepo(string repoRoot) => new(() => EnginePaths.Repo(repoRoot));

    /// <summary>Portable-install factory: the x64 core beside the window, matching <see cref="CalcClient.ForPortable"/>.</summary>
    public static LicenseClient ForPortable(string baseDir) => new(() => EnginePaths.Portable(baseDir));

    /// <summary>
    /// The component register as the core prints it, or <c>null</c> when the core could not be asked.
    /// </summary>
    /// <remarks>
    /// Null rather than an exception or an empty string, because the caller has to be able to tell the
    /// two apart and SAY which one happened. An empty list rendered as a heading with nothing under it
    /// reads as "there is nothing third-party in here", which is the one answer this must never give by
    /// accident (untouchable rules 4 and 6). Every failure lands here: a missing core in a checkout that
    /// was never built, a core that will not start, a non-zero exit.
    /// </remarks>
    public async Task<string?> TryReadComponentsAsync(CancellationToken ct = default)
    {
        try
        {
            // Both pipes are read to the end while the core runs (OneShotEngine): this output runs to a few
            // kilobytes and a pipe buffer is about four, and stderr was once redirected without being read,
            // so a core that wrote more than a buffer there stopped until the limit killed it (R4-N50).
            var run = await OneShotEngine.RunAsync(_chronoPath(), ["license", "--components"], null, QueryTimeout, ct)
                .ConfigureAwait(false);
            return run.ExitCode == 0 && run.Stdout.Trim().Length > 0 ? run.Stdout : null;
        }
        catch (Exception failure) when (failure is EngineLaunchException or EngineTimeoutException
                                            or OperationCanceledException or IOException)
        {
            // Whether the core could not start, ran out of time or was let go, the register could not be
            // read - and the caller says so rather than showing an empty list.
            return null;
        }
    }
}
