using ChronoMock.App.Calc;
using ChronoMock.Protocol;

namespace ChronoMock.App;

/// <summary>
/// Everything that fills the session's moment field by asking the engine - a scenario, "now, shifted" -
/// and the one rule they share: an answer fills the field only while it answers the newest request, and a
/// change made to the field by anything else is an answer of its own (R4-S25).
/// <para>
/// The engine answers in its own time, and the field used to take every answer as it came. Arrowing from
/// scenario A to B showed A's date beside B's name when A answered last. A date typed by hand was
/// overwritten by the scenario chosen just before it. Start pressed straight after a choice started with the
/// date from before it. All three are an answer applied to a request that was no longer the newest, which
/// is what <see cref="LatestAnswer"/> exists to refuse.
/// </para>
/// <para>
/// A type of its own beside the session view model rather than more of it (R4-D6): that class stands on its
/// coupling ceiling, and this takes the scenario and "now, shifted" resolution off it rather than adding to
/// it.
/// </para>
/// </summary>
internal sealed class MomentRequests
{
    /// <summary>How long Start waits for the answer to the newest request before going ahead with the field
    /// as it stands. Above the engine's own 10 s limit, so an honest request always answers inside it.</summary>
    private static readonly TimeSpan StartWait = TimeSpan.FromSeconds(15);

    private readonly MomentField _moment;
    private readonly ICalcEngine? _engine;
    private readonly Func<int> _zoneBias;
    private readonly LatestAnswer _answer = new();
    private bool _filling;
    private bool _relativeAsked;

    /// <param name="moment">The field the answers fill - the session's start moment.</param>
    /// <param name="engine">The calculator engine, or null when it could not be resolved.</param>
    /// <param name="zoneBias">The session zone's bias, read when a request is made rather than captured,
    /// because the zone is part of every question asked here (untouchable rule 2).</param>
    public MomentRequests(MomentField moment, ICalcEngine? engine, Func<int> zoneBias)
    {
        _moment = moment ?? throw new ArgumentNullException(nameof(moment));
        _engine = engine;
        _zoneBias = zoneBias ?? throw new ArgumentNullException(nameof(zoneBias));

        // A change this type did not make - typed, Today or Now, the calculator's date, a history row - is
        // the user's own answer. Whatever is still being computed is cancelled and will be refused, so it
        // can never overwrite what the user put there.
        _moment.Changed += (_, _) =>
        {
            if (!_filling)
            {
                _answer.Settle();
            }
        };
    }

    /// <summary>The session zone's bias right now, the one every request is asked in.</summary>
    public int ZoneBias => _zoneBias();

    /// <summary>True only while the field is being filled from a scenario's answer, so the field's change
    /// does not clear the scenario that caused it.</summary>
    public bool FillingFromScenario { get; private set; }

    /// <summary>Whether the newest request is a "now, shifted" one still being computed. A zone change asks
    /// it again, since the zone is part of the question.</summary>
    public bool IsRelativePending => _relativeAsked && !_answer.IsCurrent;

    /// <summary>
    /// Ask for a scenario's moment and fill the field with it, unless something newer has been asked or put
    /// in the field by the time it comes. Returns the translation key naming why the scenario gave no date,
    /// or null when it filled the field - or when its answer was refused for being out of date, which is no
    /// failure of the scenario's.
    /// </summary>
    public async Task<string?> ChooseScenarioAsync(ScenarioItem scenario)
    {
        ArgumentNullException.ThrowIfNull(scenario);
        _relativeAsked = false;
        _answer.Ask();
        if (_answer.Begin() is not { } ticket)
        {
            return null;
        }

        try
        {
            var resolved = await ScenarioMoment.ResolveAsync(_engine, scenario, _zoneBias(), ticket.Token);
            return Settle(ticket, resolved.Iso, resolved.ErrorKey, fromScenario: true);
        }
        catch (OperationCanceledException)
        {
            return null; // a newer request or a typed date answers the field now
        }
        finally
        {
            _answer.Release(ticket);
        }
    }

    /// <summary>The same for "now, shifted": <paramref name="args"/> is the calc invocation the relative
    /// line built. Returns the reason it gave no date, or null.</summary>
    public async Task<string?> FillRelativeAsync(IReadOnlyList<string> args)
    {
        ArgumentNullException.ThrowIfNull(args);
        _relativeAsked = true;
        _answer.Ask();
        if (_answer.Begin() is not { } ticket)
        {
            return null;
        }

        try
        {
            var resolved = await RelativeMoment.ResolveAsync(_engine, args, ticket.Token);
            return Settle(ticket, resolved.Iso, resolved.ErrorKey, fromScenario: false);
        }
        catch (OperationCanceledException)
        {
            return null;
        }
        finally
        {
            _answer.Release(ticket);
        }
    }

    /// <summary>
    /// Wait until the field holds the answer to the newest request - at once when nothing is being
    /// computed. Start waits on this, so a press straight after a choice starts with the chosen date rather
    /// than the one before it. True when the field is current, false when the wait ran out.
    /// </summary>
    public Task<bool> WhenCurrentAsync() => _answer.WhenCurrentAsync(StartWait);

    private string? Settle(LatestAnswer.Ticket ticket, string? iso, string? errorKey, bool fromScenario)
    {
        if (!_answer.Accept(ticket))
        {
            return null;
        }

        if (iso is null)
        {
            return errorKey;
        }

        _filling = true;
        FillingFromScenario = fromScenario;
        try
        {
            _moment.LoadCanonical(iso);
        }
        finally
        {
            _filling = false;
            FillingFromScenario = false;
        }

        return null;
    }
}
