using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>
/// Where a list of scenarios stands, for the lines the screen shows in its place and under it: still being
/// read, could not be read (and why), read but empty, or read - with how many preset files the engine left out.
/// <para>
/// One type for both lists, the calculator's and the substitution panel's (GUI rules 2 and 5): they show
/// the same catalogue, so a difference in how they say "not yet", "could not" or "left out" would be a
/// difference with no reason behind it. Its views are <c>Controls/CatalogueStatusView</c> (in the list's
/// place) and <c>Controls/CatalogueLeftOutNote</c> (under the list).
/// </para>
/// <para>
/// 🔴 THE LEFT-OUT COUNT IS SAID, NOT HIDDEN (rule 6, owner's decision D2 of R4/18). A preset file the engine
/// refuses does not appear in the list - showing it would offer a scenario that cannot be computed - but a
/// tester who wrote one and does not see it needs to know it was read and refused, and where to ask why.
/// </para>
/// </summary>
public sealed class CatalogueStatus : ObservableObject
{
    private Stage _stage = Stage.Reading;
    private string _failure = string.Empty;
    private string _failureReason = string.Empty;
    private string _failureDetail = string.Empty;
    private int _shown;
    private int _leftOut;
    private string _leftOutFiles = string.Empty;

    private enum Stage
    {
        Reading,
        Failed,
        Ready,
    }

    /// <summary>True until the catalogue has been read or has failed - a list starts here, because a screen
    /// shown before the read asks for it is a screen waiting for it.</summary>
    public bool IsReading => _stage == Stage.Reading;

    /// <summary>True when the catalogue could not be read.</summary>
    public bool HasFailed => _stage == Stage.Failed;

    /// <summary>True when the catalogue was read - what the list shows is then what is installed.</summary>
    public bool IsReady => _stage == Stage.Ready;

    /// <summary>True when there is a list to show: read, and not empty. Until then its place says why not.</summary>
    public bool ShowsList => _stage == Stage.Ready && _shown > 0;

    /// <summary>What went wrong, in one line the reader can act on: the list could not be read.</summary>
    public string Failure => _failure;

    /// <summary>Why, in the interface's language when the engine named its failure with a key - the same
    /// sentence the calculator gives for it - else the engine's own sentence.</summary>
    public string FailureReason => _failureReason;

    /// <summary>The engine's own sentence under the reason, or empty when the reason already is that
    /// sentence. It carries what a key cannot, such as the path of an engine that is not there.</summary>
    public string FailureDetail => _failureDetail;

    public bool HasFailureDetail => HasFailed && _failureDetail.Length > 0;

    /// <summary>True when the catalogue was read and this list has nothing to show from it.</summary>
    public bool IsEmpty => _stage == Stage.Ready && _shown == 0;

    /// <summary>How many preset files the engine left out of the catalogue.</summary>
    public int LeftOut => _leftOut;

    public bool HasLeftOut => _stage == Stage.Ready && _leftOut > 0;

    /// <summary>The files left out, one per line with the engine's reason - for the tooltip of the count.</summary>
    public string LeftOutFiles => _leftOutFiles;

    /// <summary>A read has started (again).</summary>
    public void Reading() => Move(Stage.Reading);

    /// <summary>The read failed with the engine's <paramref name="engineMessage"/>, as
    /// <c>CalcException.Message</c> carries it.</summary>
    public void Failed(string engineMessage, Func<string, string> translate)
    {
        ArgumentNullException.ThrowIfNull(translate);
        var said = CalcErrorText.Describe(engineMessage, translate);
        var sentence = CalcErrorText.Detail(engineMessage);
        _failure = translate("scenario.list_failed");
        _failureReason = said;
        _failureDetail = string.Equals(said, sentence, StringComparison.Ordinal) ? string.Empty : sentence;
        Move(Stage.Failed);
    }

    /// <summary>The read succeeded: this list shows <paramref name="shown"/> scenarios, and the catalogue
    /// refused the files in <paramref name="refused"/>.</summary>
    public void Ready(int shown, IReadOnlyList<RefusedPresetFile> refused)
    {
        ArgumentNullException.ThrowIfNull(refused);
        _shown = shown;
        _leftOut = refused.Count;
        _leftOutFiles = string.Join(Environment.NewLine, refused.Select(r => $"{r.File} - {r.Reason}"));
        Move(Stage.Ready);
    }

    private void Move(Stage stage)
    {
        _stage = stage;
        RaisePropertyChanged(nameof(IsReading));
        RaisePropertyChanged(nameof(HasFailed));
        RaisePropertyChanged(nameof(IsReady));
        RaisePropertyChanged(nameof(ShowsList));
        RaisePropertyChanged(nameof(Failure));
        RaisePropertyChanged(nameof(FailureReason));
        RaisePropertyChanged(nameof(FailureDetail));
        RaisePropertyChanged(nameof(HasFailureDetail));
        RaisePropertyChanged(nameof(IsEmpty));
        RaisePropertyChanged(nameof(LeftOut));
        RaisePropertyChanged(nameof(HasLeftOut));
        RaisePropertyChanged(nameof(LeftOutFiles));
    }
}
