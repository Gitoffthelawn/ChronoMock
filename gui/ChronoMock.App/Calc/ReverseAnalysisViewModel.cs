using System.Collections.ObjectModel;
using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>
/// The calculator's reverse-analysis strip: a pasted date and every way it can be read (R5-D9). It has its
/// own input, its own answer and its own failure, and shares nothing with the builder above it but the
/// zone a pasted number is read in - so it is its own view model, which the strip binds as its DataContext.
/// <para>
/// It lived inside <see cref="CalculatorViewModel"/>, which reached eighteen hundred lines with it, and the
/// answer to the pasted text went through the same hand-written ticket loop as the builder's result.
/// </para>
/// </summary>
public sealed class ReverseAnalysisViewModel : ObservableObject
{
    private readonly ICalcEngine _client;
    private readonly Func<ZoneOption> _zone;

    /// <summary>Which pasted text the readings on screen answer (R4-N44): every edit asks, and an analysis is
    /// shown only while it answers the newest text.</summary>
    private readonly LatestAnswer _analysis = new();
    private readonly Debounce _debounce;
    private bool _started;
    private string _analyzeText = "04/08/2008";
    private bool _hasAnalysis;
    private bool _analyzeAmbiguous;
    private bool _analyzeHasError;
    private string _analyzeError = string.Empty;
    private string _analyzeErrorDetail = string.Empty;
    private string _analysisZoneLine = string.Empty;

    /// <param name="zone">The zone the builder reads its start point in - a pasted number is an instant,
    /// read in this same zone, the one <c>--zone</c> names for <c>--analyze</c> in the CLI too (R4-S23,
    /// owner's decision D4).</param>
    /// <param name="quiet">How long typing stays quiet before it turns into an engine process.</param>
    internal ReverseAnalysisViewModel(ICalcEngine client, Func<ZoneOption> zone, TimeSpan quiet)
    {
        _client = client;
        _zone = zone;
        _debounce = new Debounce(quiet);
        _analysis.CurrentChanged += (_, _) => RaisePropertyChanged(nameof(IsAnalysisStale));
    }

    public ObservableCollection<ReadingRow> Readings { get; } = [];

    /// <summary>The pasted date to interpret (reverse analysis).</summary>
    public string AnalyzeText
    {
        get => _analyzeText;
        set
        {
            if (Set(ref _analyzeText, value))
            {
                Trigger();
            }
        }
    }

    public bool HasAnalysis { get => _hasAnalysis; private set => Set(ref _hasAnalysis, value); }
    public bool AnalyzeAmbiguous { get => _analyzeAmbiguous; private set => Set(ref _analyzeAmbiguous, value); }
    public bool AnalyzeHasError { get => _analyzeHasError; private set => Set(ref _analyzeHasError, value); }
    public string AnalyzeError { get => _analyzeError; private set => Set(ref _analyzeError, value); }

    /// <summary>The engine's own sentence under the analysis failure, the way the result column shows one
    /// under its own (R4-N44): it names what the engine could not read, which the translated line alone
    /// does not. Empty when it would only repeat that line.</summary>
    public string AnalyzeErrorDetail
    {
        get => _analyzeErrorDetail;
        private set { if (Set(ref _analyzeErrorDetail, value)) { RaisePropertyChanged(nameof(HasAnalyzeErrorDetail)); } }
    }

    public bool HasAnalyzeErrorDetail => _analyzeErrorDetail.Length > 0;

    /// <summary>Whether the readings on screen are for an earlier text than the one in the box - from an
    /// edit until the analysis of it lands. The strip shows that rather than pass old readings off as
    /// the new text's.</summary>
    public bool IsAnalysisStale => !_analysis.IsCurrent;

    /// <summary>The line above instant readings naming the zone they are shown in, empty when there is none.</summary>
    public string AnalysisZoneLine
    {
        get => _analysisZoneLine;
        private set
        {
            if (Set(ref _analysisZoneLine, value))
            {
                RaisePropertyChanged(nameof(HasAnalysisZoneLine));
            }
        }
    }

    public bool HasAnalysisZoneLine => _analysisZoneLine.Length > 0;

    /// <summary>Build the calc arguments for reverse analysis (pure - unit-tested). <paramref name="zoneOffset"/>
    /// is the calculator's zone, null for this machine's, which sends no flag - the zone a pasted number is
    /// read in, exactly as <c>--zone</c> does for <c>--analyze</c> in the CLI (R4-S23).</summary>
    public static IReadOnlyList<string> BuildAnalyzeArgs(string text, string? zoneOffset = null)
        => zoneOffset is null ? ["--analyze", text.Trim()] : ["--analyze", text.Trim(), "--zone", zoneOffset];

    /// <summary>The sentence above instant readings. A number is an instant because it carries no
    /// date, a date with an offset (<c>2026-01-01T10:00:00+02:00</c>, R5-D5 item 5) because it names its
    /// own zone - the line for the number said "a number" of a date, so the second one has its own.</summary>
    internal static string AnalysisZoneKey(IEnumerable<CalcReading> readings)
        => readings.Any(r => string.Equals(r.Reading, "iso_offset", StringComparison.Ordinal))
            ? "calc.analyze_zone_offset"
            : "calc.analyze_zone";

    /// <summary>The strip is shown for the first time: analyse the text it holds (its default example), at
    /// once rather than after the quiet period. Never from the constructor, so building the window in a
    /// test spawns no process.</summary>
    internal void Start()
    {
        _started = true;
        _analysis.Ask();
        _ = AnalyzeAsync();
    }

    /// <summary>The text or the zone it is read in changed: analyse again once typing is quiet.</summary>
    internal void Trigger()
    {
        if (_started)
        {
            _analysis.Ask();
            _ = _debounce.RunAsync(AnalyzeAsync);
        }
    }

    /// <summary>The window is closing: cancel the analysis being computed (R4/13).</summary>
    internal void Abandon() => _analysis.Abandon();

    // Null when the text in the box already has its analysis on screen - nothing to compute.
    private Task AnalyzeAsync() => _analysis.RunAsync(
        token => _client.EvaluateAsync(BuildAnalyzeArgs(_analyzeText, CalculatorViewModel.ZoneArgument(_zone())), token),
        ApplyAnalysis,
        Fail);

    private void Fail(Exception e)
    {
        if (e is CalcException calc)
        {
            ShowAnalysisError(CalculatorViewModel.DescribeCalcError(calc.Message), CalculatorViewModel.DetailForCalcError(calc.Message));
        }
        else
        {
            // Any other failure (an incomplete reading dereferenced in ApplyAnalysis / ReadingRow) is
            // surfaced honestly rather than swallowed by this fire-and-forget task (M-11, rule 6). Not a
            // translated key: this is a .NET fault with no key to map, and the message is all there is.
            ShowAnalysisError(e.Message, string.Empty);
        }
    }

    /// <summary>An analysis that failed shows the failure and nothing else (R4-N44): the readings of the
    /// text before it stayed on screen under the error, read as this text's.</summary>
    private void ShowAnalysisError(string message, string detail)
    {
        AnalyzeError = message;
        AnalyzeErrorDetail = detail;
        AnalyzeHasError = true;
        HasAnalysis = false;
        AnalyzeAmbiguous = false;
        AnalysisZoneLine = string.Empty;
        Readings.Clear();
    }

    private void ApplyAnalysis(CalcResult result)
    {
        if (result.Analysis is not { } analysis)
        {
            ShowAnalysisError(Tr("calc.err.no_readings"), string.Empty);
            return;
        }

        AnalyzeHasError = false;
        AnalyzeError = string.Empty;
        AnalyzeErrorDetail = string.Empty;
        AnalyzeAmbiguous = analysis.Ambiguous;
        // An instant has a wall clock only in some zone, and the readings showed it with no zone near them
        // (R4-S23) - the CLI prints the same line above them. A date written as fields has none to name.
        AnalysisZoneLine = analysis.Readings.Any(r => r.Instant)
            ? TextFormat.Translate(
                Tr, AnalysisZoneKey(analysis.Readings),
                ZoneLabel.FromBiasMinutes(analysis.ZoneBiasMin) + (_zone().IsHost ? $" ({Tr("zone.host")})" : string.Empty))
            : string.Empty;
        Readings.Clear();
        foreach (var reading in analysis.Readings)
        {
            Readings.Add(new ReadingRow(reading));
        }

        HasAnalysis = Readings.Count > 0;
    }

    private static string Tr(string key) => TranslationKeyConverter.Resolve(key);
}
