using ChronoMock.App.Localization;
using ChronoMock.Protocol;

namespace ChronoMock.App.Calc;

/// <summary>
/// The calendars the calculator offers, as the engine lists them (<c>chrono calendars</c>, R5-P12), and why
/// there are none when it could not say.
/// </summary>
/// <remarks>
/// Its own type for the reason <see cref="ScenarioPicker"/> is one: <see cref="CalculatorViewModel"/> stood on
/// its coupling ceiling (gui/CodeMetricsConfig.txt), and the reader, its cancellation and the catalogue's types
/// are one idea the builder only asks two questions of - which calendars there are, and whether one is listed.
/// The calendar CHOSEN stays the builder's, because choosing one recomputes the answer.
/// </remarks>
public sealed class CalendarChoices : ObservableObject
{
    private readonly ICalendarSource? _source;
    private readonly CancellationTokenSource _life = new();
    private IReadOnlyList<CalendarOption> _options = CalendarOption.Shipped;
    private string _error = string.Empty;

    /// <summary>Choices read from <paramref name="source"/>. With no source (a test, a window that asks no
    /// engine) the list stays the shipped one it starts as.</summary>
    public CalendarChoices(ICalendarSource? source) => _source = source;

    /// <summary>No calendar, then the engine's list - the shipped calendars until it has answered.</summary>
    public IReadOnlyList<CalendarOption> Options { get => _options; private set => Set(ref _options, value); }

    /// <summary>Why the list could not be read, in the interface language - empty when it was read.</summary>
    public string Error
    {
        get => _error;
        private set
        {
            if (Set(ref _error, value))
            {
                RaisePropertyChanged(nameof(HasError));
            }
        }
    }

    public bool HasError => _error.Length > 0;

    /// <summary>
    /// Read the list through the engine. Never throws: a list that could not be read leaves only "no
    /// calendar" and says why - offering a calendar nobody could vouch for would be the silent fallback this
    /// replaces. Asked for on the calculator's first reveal, never in a constructor, so a window built in a
    /// test starts no process.
    /// </summary>
    public async Task LoadAsync()
    {
        if (_source is null)
        {
            return;
        }

        try
        {
            var catalogue = await _source.ReadAsync(_life.Token);
            Options = [CalendarOption.None, .. catalogue.Calendars.Select(CalendarOption.From)];
        }
        catch (OperationCanceledException)
        {
            // the window is closing
        }
        catch (CalcException e)
        {
            Options = [CalendarOption.None];
            Error = TextFormat.Translate(
                TranslationKeyConverter.Resolve, "calc.cal.unreadable", CalcErrorText.Describe(e.Message, TranslationKeyConverter.Resolve));
        }
    }

    /// <summary>The listed option for <paramref name="id"/>, matched as the engine finds the file, or "no
    /// calendar" when it is not listed.</summary>
    public CalendarOption Find(string? id) => _options.FirstOrDefault(c => CalendarOption.SameId(c.Id, id)) ?? CalendarOption.None;

    /// <summary>Whether the engine lists a calendar of this id.</summary>
    public bool Lists(string id) => _options.Any(c => CalendarOption.SameId(c.Id, id));

    /// <summary>Stop a read still in flight because the window is closing.</summary>
    public void Abandon() => _life.Cancel();
}
