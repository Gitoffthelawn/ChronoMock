using System.Windows;
using Wpf.Ui.Controls;
using ChronoMock.App.Calc;

namespace ChronoMock.App;

public partial class MainWindow : FluentWindow
{
    // The panel is a second consumer of the calculator engine and of the shared preset catalogue: the
    // scenario list turns a named preset into a date (7.1 pt 2). Same layout seam as everything else,
    // resolved in AppPaths.
    private readonly SessionViewModel _session = new(
        FileSessionHistoryStore.ForApp(), FileDiagnosticsLog.ForApp(), AppPaths.CalcClient, AppPaths.PresetsDir,
        ProcessElevation.IsElevated);
    private readonly CalculatorViewModel _calculator = CreateCalculator();


    // The calculator is a client of the same engine (ADR-6) - it reads the shared preset catalogue and the
    // calendars from the portable install beside the exe, or from the cargo outputs in a dev checkout - the
    // layout seam lives in AppPaths, not here.
    private static CalculatorViewModel CreateCalculator()
        => new(AppPaths.CalcClient, AppPaths.PresetsDir);

    public MainWindow()
    {
        InitializeComponent();
        DataContext = _session;
        CalculatorContainer.DataContext = _calculator;

        // Hand the session's window-dependent commands (the pickers, the clipboard, the confirm dialog) a
        // window to act against. This is the ONE place a UI type meets those commands: the view model never
        // holds it (GUI rules 15 and 16), and a command whose shell is not attached is a quiet no-op rather
        // than a crash, which is how a phase drawn for the state sheet stays harmless.
        _session.Commands.AttachShell(new WindowShellInteraction(this));

        // Bridge: the calculator asks to send its result to substitution - this window fills the panel.
        _calculator.UseInSubstitutionRequested += OnUseInSubstitution;

        // Dev convenience: pre-select the bundled sample target so the panel is usable at once. The user
        // can pick any executable instead - this default is dev scaffolding (DemoSession.DefaultTargetPath).
        var sample = SessionPlan.DefaultTargetPath();
        if (sample is not null)
        {
            _session.SetTarget(sample);
        }

        // Closing the window ends the session: the core is stopped, and the hook self-detaches so the
        // target reverts to real time on its own (slice 10) - we never kill it.
        Closing += OnWindowClosing;
    }

    /// <summary>How long a window closed during a session waits for it to end before it closes anyway
    /// (R4-D1 of R4/13). The longest ordinary end is the core's: two seconds of grace before it is stopped,
    /// then the reads drained - 6.5 s at the outside - and the history written after it. Typically all of
    /// it is well under half a second.</summary>
    private static readonly TimeSpan CloseWait = TimeSpan.FromSeconds(10);

    /// <summary>Where closing stands: the window is open, ending its session before it closes, or done.</summary>
    private CloseStage _closeStage;

    private enum CloseStage
    {
        Open,
        Finishing,
        Done,
    }

    /// <summary>
    /// 🔴 The close waits for the session WITHOUT blocking this thread (R4-S24). It used to block here for up
    /// to three seconds on a dispose whose end needed this very thread, so the wait always ran out - measured,
    /// every close during a session took 3.05 s - and the application then exited before the session reached
    /// the history: zero of three were recorded. Now a close with a session running is put off, the session
    /// ends as a Stop ends it (the panel says "Stopping"), and the window closes itself once it is over or
    /// once <see cref="CloseWait"/> has passed. A second close in the meantime changes nothing. A close with
    /// no session to end is not delayed at all.
    /// <para>
    /// Not raised when Windows ends the user's session: the documentation for Closing says logging off
    /// skips it, and an application shutdown ignores a cancel. A session cut off that way ends as before - the
    /// core sees its input close, the hook lets go - and is not in the history (R4-D2).
    /// </para>
    /// </summary>
    private void OnWindowClosing(object? sender, System.ComponentModel.CancelEventArgs e)
    {
        if (_closeStage == CloseStage.Done)
        {
            return;
        }

        _calculator.Abandon();
        if (_closeStage == CloseStage.Finishing)
        {
            e.Cancel = true;
            return;
        }

        var finishing = _session.FinishForCloseAsync(CloseWait);
        if (finishing.IsCompleted)
        {
            _closeStage = CloseStage.Done;
            return;
        }

        e.Cancel = true;
        _closeStage = CloseStage.Finishing;
        _ = CloseOnceFinishedAsync(finishing);
    }

    private async Task CloseOnceFinishedAsync(Task<bool> finishing)
    {
        try
        {
            await finishing;
        }
        finally
        {
            _closeStage = CloseStage.Done;
            // Queued, never called from here: Close() while the window is still inside Closing throws, and
            // nothing guarantees the wait above did not complete before Closing returned.
            _ = Dispatcher.BeginInvoke(new Action(Close));
        }
    }

    // Swap the visible module: substitution (the three phases) vs calculator view. The default radio's
    // Checked fires during InitializeComponent, before the content elements exist, so both are null-checked
    // here. The substitution container holds the phases - which phase shows inside it is the view model's
    // ShowsXPhase decision, not this switch's.
    private void OnModeChanged(object sender, RoutedEventArgs e)
    {
        if (SubstitutionContainer is null || CalculatorContainer is null)
        {
            return;
        }

        bool calculator = ModeCalculator.IsChecked == true;
        SubstitutionContainer.Visibility = calculator ? Visibility.Collapsed : Visibility.Visible;
        CalculatorContainer.Visibility = calculator ? Visibility.Visible : Visibility.Collapsed;
        if (calculator)
        {
            // Compute on first reveal (not at construction, so building the window in a test spawns nothing).
            _ = _calculator.EnsureComputedAsync();
        }
    }

    // Bridge from the calculator (chrono-mock 6.3): hand the moment and its zone to the session (rule 2 -
    // the moment travels with its zone, never a bare date), then show the substitution module. What the
    // session does with it depends on its phase and is decided there, not here.
    private void OnUseInSubstitution(string momentLocal, int zoneBias)
    {
        // Shown only when a field took the moment: landing on a screen that shows nothing of the press
        // is exactly what "the button does nothing" looked like before.
        if (_session.AdoptMoment(momentLocal, zoneBias))
        {
            ModeSubstitution.IsChecked = true; // OnModeChanged swaps the visible module
        }
    }

    // Dropping an application on the window (chrono-mock 7.1 pt 1). Only fills the target - it never
    // starts a session (rule 7), exactly like picking one from the dialog or the recent list.
    //
    // The window accepts the drop, not one small panel: aiming at a strip is the part people miss, and
    // the whole window is the obvious target for "run this app". Refused while a session runs, because
    // the target is start-only and a silently ignored drop would look like the drop failed.
    private void OnWindowDragOver(object sender, DragEventArgs e)
    {
        e.Effects = DroppedExecutable(e) is null ? DragDropEffects.None : DragDropEffects.Copy;
        e.Handled = true;
    }

    private async void OnWindowDrop(object sender, DragEventArgs e)
    {
        e.Handled = true;
        if (!_session.IsIdle)
        {
            return;
        }

        // Read the drop NOW: its data lives only as long as this event, and the check below awaits.
        var path = DroppedExecutable(e);
        var files = e.Data.GetDataPresent(DataFormats.FileDrop);
        if (path is not null && await _session.DropTargetAsync(path))
        {
            return;
        }

        // Say why nothing happened rather than swallowing the drop (rule 6). A dropped .lnk or .bat could
        // only fail later, and failing at the drop is the honest place for it.
        if (files)
        {
            Views.MessageDialog.Tell(this, Text("target.drop_rejected_title"), Text("target.drop_rejected"));
        }
    }

    /// <summary>The single .exe in a drop, or null - judged by its name only (see <see cref="DroppedTarget"/>).</summary>
    private string? DroppedExecutable(DragEventArgs e)
        => _session.IsIdle && e.Data.GetDataPresent(DataFormats.FileDrop)
            ? DroppedTarget.Candidate(e.Data.GetData(DataFormats.FileDrop))
            : null;

    // The support link. The destination is chosen inside ExternalLinks and never here, so this method is
    // not a place where a string could turn into something the shell runs.
    //
    // A machine with nothing associated with https is rare and real, and on one the shell simply refuses.
    // Refusing quietly would leave a button that does nothing at all, so the failure is said out loud and
    // the address goes into the message beneath it - a notice with no way out is half an answer, and the
    // way out here is being able to type the address in by hand.
    private void OnSupportClick(object sender, RoutedEventArgs e)
    {
        if (!ExternalLinks.TryOpenSupport())
        {
            Views.MessageDialog.Tell(
                this,
                Text("support.failed_title"),
                Text("support.failed") + Environment.NewLine + Environment.NewLine + ExternalLinks.Support);
        }
    }

    // The About window, which is where this application states its licence, its version and what somebody
    // else wrote inside it. The command line has answered that since it existed (`chrono license`), and
    // until now the window had no answer at all - the asymmetry, not a licence requirement, is the reason
    // it is here.
    private void OnAboutClick(object sender, RoutedEventArgs e)
        => Views.AboutDialog.Show(this, AppPaths.LicenceClient);

    // Resolve a translation key to text for a native dialog (rule 15) - falls back to the raw key if missing.
    private static string Text(string key) => Application.Current?.TryFindResource(key) as string ?? key;
}
