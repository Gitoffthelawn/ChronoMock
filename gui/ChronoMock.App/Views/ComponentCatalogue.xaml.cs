using System.Windows.Controls;

namespace ChronoMock.App.Views;

/// <summary>
/// The component catalogue: every part of the interface, in every state that can be shown without input.
/// </summary>
/// <remarks>
/// It carries no behaviour on purpose. A catalogue that computed anything would be a second screen to
/// keep working rather than a picture of the parts library, and the parts it shows are drawings, not
/// controls with logic behind them.
/// </remarks>
public partial class ComponentCatalogue : UserControl
{
    public ComponentCatalogue() => InitializeComponent();

    /// <summary>
    /// Sample clocks for the clock part, which cannot be written in the markup.
    /// </summary>
    /// <remarks>
    /// 🔴 DATA, NOT BEHAVIOUR - the distinction this class's remarks draw. ClockView takes its role in the
    /// constructor and exposes the time through one settable member, so a compiled XAML file cannot build
    /// one: there is no parameterless constructor for it to call. Leaving the clock out of the catalogue
    /// was the alternative, and a catalogue missing a part it has is the fault the catalogue exists to
    /// prevent.
    ///
    /// The pair is deliberate. Both clocks are ONE drawing with different data, and the only way to see
    /// that they have not drifted apart is to put them next to each other, which is also how the session
    /// phase shows them.
    /// </remarks>
    public ClockView SampleFakeClock { get; } = new("clock.fake")
    {
        Wall = "2038-01-19T03:14:07",
        Zone = "UTC+00:00",
    };

    /// <summary>Every status the audit fold can produce, in the order the table lists them, plus the extremes:
    /// a function name longer than any real one and a count past six digits. Rows, not lists, because the
    /// fold is the converter's and the catalogue shows the drawing.</summary>
    public IReadOnlyList<AuditRow> SampleAuditRows { get; } =
    [
        new("GetLocalTime", "audit.status_real", AuditStatus.Real, null),
        new("QueryPerformanceCounter", "audit.status_by_design", AuditStatus.ByDesign, 3),
        new("QueryPerformanceFrequency", "audit.status_by_design_late", AuditStatus.ByDesignLate, 2),
        new("NtQuerySystemTime", "audit.status_unwatched", AuditStatus.Unwatched, null),
        new("timeGetTime", "audit.status_late", AuditStatus.Late, 7),
        new("GetSystemTimeAsFileTime", "audit.status_fake", AuditStatus.Fake, 128004),
        new("GetSystemTimePreciseAsFileTimeWithAnImpossiblyLongName", "audit.status_fake", AuditStatus.Fake, 9876543210),
    ];

    /// <summary>Every kind of row the process fold can produce, in the order the table lists them: the
    /// renderer row in the failure ink, the other roles, a process with no role at all, the extremes of an
    /// executable name longer than any real one and a count past six digits, and last the row for the
    /// processes gone before they could be named - and the renderer in BOTH of its meanings, with and without
    /// its pages reached.</summary>
    public IReadOnlyList<UncoveredProcessRow> SampleProcessRows { get; } =
    [
        new("engine.exe", IsUnnamed: false, "renderer", IsRenderer: true, PagesReached: false, 1),
        // The same role with the opposite meaning, which is why the two are never counted into one row:
        // the session reached this renderer's engine, so its pages ran on the session clock and only its
        // own native reads did not. Partial ink and an annotation, not the failure ink.
        new("engine.exe", IsUnnamed: false, "renderer", IsRenderer: true, PagesReached: true, 1),
        new("engine.exe", IsUnnamed: false, "utility", IsRenderer: false, PagesReached: false, 2),
        new("engine.exe", IsUnnamed: false, "gpu-process", IsRenderer: false, PagesReached: false, 1),
        new("helper.exe", IsUnnamed: false, string.Empty, IsRenderer: false, PagesReached: false, 1),
        new("AnEmbeddedRuntimeWithAnImpossiblyLongExecutableName.exe", IsUnnamed: false, "crashpad-handler", IsRenderer: false, PagesReached: false, 1234567),
        new(string.Empty, IsUnnamed: true, string.Empty, IsRenderer: false, PagesReached: false, 2),
    ];

    /// <summary>Every kind of row the engine fold can produce: an engine that named itself, one that named
    /// nothing and takes the word instead, and the extremes - a name far longer than any engine reports and
    /// the highest port number there is.</summary>
    public IReadOnlyList<EngineRow> SampleEngineRows { get; } =
    [
        new("Engine/153.0", IsUnnamed: false, 61868),
        new("python/3.14", IsUnnamed: false, 5123),
        new(string.Empty, IsUnnamed: true, 9222),
        new("AnEmbeddedWebEngineThatReportsAnImpossiblyLongVersionString/153.0.3283.0", IsUnnamed: false, 65535),
    ];

    /// <summary>The date input with nothing typed yet, so the hint shows and the calendar has no selection.
    /// A MomentField rather than a stub, so the catalogue draws the part over the same object the
    /// screens bind it to.</summary>
    public MomentField SampleEmptyDate { get; } = new();

    /// <summary>The date input with the shipped default moment in it - and the moment input's time beside it
    /// in the second row, which is what the setup and the builder show.</summary>
    public MomentField SampleFilledDate { get; } = new()
    {
        DateText = "2038-01-19",
        TimeText = "03:14:07",
    };

    /// <summary>The amount input with a sign, as a shift step shows it, on the longest unit there is.</summary>
    public AmountUnitSample SampleSignedAmount { get; } = new("90", "bd");

    /// <summary>The amount input without a sign, as a scenario's duration parameter shows it.</summary>
    public AmountUnitSample SampleUnsignedAmount { get; } = new("30", "d");

    /// <summary>The recent-application rows: one there, its other build gone, and one with a long path.</summary>
    public IReadOnlyList<RecentTarget> SampleRecentTargets { get; } =
    [
        new(@"C:\src\app\bin\x64\Release\app.exe"),
        new(@"C:\src\app\bin\x86\Release\app.exe") { IsMissing = true },
        new(@"C:\Users\tester\source\repos\a-product-with-a-long-name\build\output\x64\Release\net10.0-windows\the-application.exe"),
    ];

    /// <summary>A scenario list still being read - where every list starts.</summary>
    public Calc.CatalogueStatus SampleListReading { get; } = new();

    /// <summary>A scenario list the engine could not give, with a reason past any real one's length.</summary>
    public Calc.CatalogueStatus SampleListFailed { get; } = Sample(s => s.Failed(
        "chrono presets: cannot read the preset folder C:\\An\\Install\\Folder\\Whose\\Path\\Runs\\Far\\Past\\The\\Width\\Of\\Any\\Column\\presets: Access is denied. (os error 5) (presets.folder_unreadable)",
        TranslationKeyConverter.Resolve));

    /// <summary>A scenario list read with nothing in it for this screen.</summary>
    public Calc.CatalogueStatus SampleListEmpty { get; } = Sample(s => s.Ready(0, []));

    /// <summary>A list read with files left out - more of them than a tooltip comfortably holds.</summary>
    public Calc.CatalogueStatus SampleListLeftOut { get; } = Sample(s => s.Ready(
        14,
        [.. Enumerable.Range(1, 40).Select(i => new Protocol.RefusedPresetFile(
            $"hand-written-{i}.json", "parameter 'n': unknown unit 'fortnights' (in presets\\hand-written.json)"))]));

    private static Calc.CatalogueStatus Sample(Action<Calc.CatalogueStatus> state)
    {
        var status = new Calc.CatalogueStatus();
        state(status);
        return status;
    }

    /// <summary>See <see cref="SampleFakeClock"/>.</summary>
    public ClockView SampleRealClock { get; } = new("clock.real")
    {
        Wall = "2026-09-12T20:30:00",
        Zone = "UTC+02:00",
    };

    /// <summary>The extreme the catalogue is for: a zone label far longer than any real one, and a time
    /// that has run past four digits of hours. If either overflows the card, it does it here rather than
    /// on somebody's screen.</summary>
    public ClockView SampleExtremeClock { get; } = new("clock.fake")
    {
        Wall = "2038-01-19T03:14:07",
        Zone = "UTC+14:00 - Line Islands, Kiritimati, the furthest offset there is",
    };
}

/// <summary>
/// The shape the amount input binds to, filled for the catalogue: the screens bind it to a shift step, the
/// setup's relative moment and a scenario's parameter, none of which can be built without the screen
/// behind it.
/// </summary>
public sealed class AmountUnitSample(string amount, string unitToken)
{
    public IReadOnlyList<string> Signs => Calc.StepViewModel.Signs;

    public string Sign { get; set; } = "+";

    public string Amount { get; set; } = amount;

    public IReadOnlyList<Calc.UnitOption> Units => Calc.StepViewModel.AllUnits;

    public Calc.UnitOption Unit { get; set; } = Calc.StepViewModel.AllUnits.Single(u => u.Token == unitToken);
}
