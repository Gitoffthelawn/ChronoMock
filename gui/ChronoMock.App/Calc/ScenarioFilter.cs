namespace ChronoMock.App.Calc;

/// <summary>
/// What a search box over a list of scenarios matches - one rule for every such list (R5-D67).
/// </summary>
/// <remarks>
/// 🔴 The session's list and the calculator's draw their rows with one template (<c>ScenarioRow</c>), the
/// name over the "what this date tests" line, both in full. They used to match differently: the calculator
/// read both lines, the session only the name, on the grounds that a row matched on text the list does not
/// show cannot be accounted for - true when the session's row showed the name alone, and no longer since the
/// rows became one. Both lines are on the row, so a match on either is one the reader can see.
///
/// CURRENT culture, while the catalogue is ORDERED invariantly, on purpose: ordering has to be identical on
/// every machine or "the third scenario" stops naming the same one, and matching is a person typing, who
/// expects their own alphabet's idea of case.
/// </remarks>
public static class ScenarioFilter
{
    /// <summary>Whether a scenario with this name and "what it tests" line answers what was typed: an empty
    /// filter, or one of spaces, keeps everything, otherwise either line contains it, without case.</summary>
    public static bool Matches(string name, string explains, string filter)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(explains);
        var needle = (filter ?? string.Empty).Trim();
        return needle.Length == 0
            || name.Contains(needle, StringComparison.CurrentCultureIgnoreCase)
            || explains.Contains(needle, StringComparison.CurrentCultureIgnoreCase);
    }
}
