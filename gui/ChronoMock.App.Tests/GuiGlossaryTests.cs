using System.Text.RegularExpressions;
using ChronoMock.App.Localization;

namespace ChronoMock.App.Tests;

/// <summary>
/// R4/21: the words the window keeps out of what a tester reads. "Core", "target", "substitution" and
/// "moment" are how the code, the wire, the command line and the documents name things, and they stay
/// there. In the window they named a process the tester never sees ("the core received a malformed
/// command"), a role the window itself calls the application, and a mechanism the window calls the fake
/// clock - 46 texts in two languages said so before this guard, the same list growing by two between the
/// report that found it and the slice that fixed it. Without a guard it grows back.
/// </summary>
/// <remarks>
/// The match is on whole words, so a key name, a longer word that merely contains one of these ("targeted")
/// and the product name are not caught. A text that genuinely needs one of these words in its plain sense
/// is rephrased first ("at that moment" became "then") - the list has no allowance on purpose, because
/// every hit so far was jargon. The Polish list carries the inflected forms the language uses, the verb's
/// as well as the noun's: the first list knew "podmiana" and missed five texts saying "podmieniony zegar"
/// or "sesja podmienia", which a review found by reading them.
/// </remarks>
public sealed class GuiGlossaryTests
{
    private static readonly Dictionary<string, Regex> KeptOut = new(StringComparer.Ordinal)
    {
        ["en"] = new Regex(
            @"\b(cores?|targets?|substitut\w*|moments?)\b",
            RegexOptions.IgnoreCase | RegexOptions.CultureInvariant),
        ["pl"] = new Regex(
            @"\b(rdze[nń]\w*|rdzeni\w*|cel(u|em|e|i|ów|owi|ach|ami)?|podmi[ae]n\w*|moment\w*)\b",
            RegexOptions.IgnoreCase | RegexOptions.CultureInvariant),
    };

    [Fact]
    public void No_text_in_the_window_uses_a_word_the_window_keeps_out()
    {
        var cultures = LocalizationService.AvailableCultures();
        var hits = WpfTestHost.Invoke(() =>
        {
            var found = new List<string>();
            foreach (var culture in cultures)
            {
                if (!KeptOut.TryGetValue(culture, out var words))
                {
                    continue; // the next test fails on a language without a list
                }

                var strings = LocalizationService.Load(culture);
                foreach (var key in strings.Keys.Cast<object>().Select(k => k.ToString()!))
                {
                    if (strings[key] is string text && words.Match(text) is { Success: true } match)
                    {
                        found.Add($"{culture} {key}: \"{match.Value}\"");
                    }
                }
            }

            return found;
        });

        Assert.True(cultures.Count >= 2, "expected at least English and Polish, found: " + string.Join(", ", cultures));
        Assert.True(
            hits.Count == 0,
            "texts in the window that use a word it keeps out (say the application, the fake clock, Chrono Mock, "
            + "the date and time): " + string.Join(" | ", hits));
    }

    [Fact]
    public void Every_language_of_the_window_has_its_list_of_words()
    {
        // A language added without a list would pass the test above while checking nothing.
        var missing = LocalizationService.AvailableCultures().Where(c => !KeptOut.ContainsKey(c)).ToList();
        Assert.True(missing.Count == 0, "languages without a list of kept-out words: " + string.Join(", ", missing));
    }

    [Theory]
    [InlineData("en", "The core received a malformed command", true)]
    [InlineData("en", "Launch the TARGET", true)]
    [InlineData("en", "the substitution did not take effect", true)]
    [InlineData("en", "at that moment", true)]
    [InlineData("en", "the substituted clock", true)]
    [InlineData("en", "a targeted test in Chrono Mock", false)]
    [InlineData("pl", "Rdzeń nie odczytał komendy", true)]
    [InlineData("pl", "Nie udało się zapytać rdzenia", true)]
    [InlineData("pl", "Uruchom cel", true)]
    [InlineData("pl", "podmiana nie zadziałała", true)]
    [InlineData("pl", "Zamroź podmieniony zegar", true)]
    [InlineData("pl", "zegara, który sesja podmienia", true)]
    [InlineData("pl", "prawdziwy zegar, celowo", false)]
    [InlineData("pl", "sprzed tego momentu", true)]
    [InlineData("pl", "fałszywy zegar nie zadziałał", false)]
    public void The_word_lists_catch_the_forms_the_texts_used(string culture, string text, bool caught)
        => Assert.Equal(caught, KeptOut[culture].IsMatch(text));
}
