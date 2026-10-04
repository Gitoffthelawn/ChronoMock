using System.IO;
using System.Text.Json;
using System.Text.RegularExpressions;
using ChronoMock.App.Localization;

namespace ChronoMock.App.Tests;

/// <summary>
/// Translated templates are filled in one place, which survives a broken translation file (R4-N46), and
/// our own two files never disagree about which values a template takes.
/// </summary>
public class TextFormatTests
{
    [Fact]
    public void A_template_is_filled_with_its_values()
        => Assert.Equal("step 2: day 31 became 28", TextFormat.Fill("step {0}: day {1} became {2}", 2, 31, 28));

    [Theory]
    [InlineData("step {5}")] // a hole with no value
    [InlineData("step {0")] // a brace left open
    [InlineData("step 0}")] // a brace closed that was never opened
    public void A_broken_template_comes_back_as_it_is_rather_than_throw(string template)
    {
        // A translation file is a loose file anyone may edit. Thrown in the middle of applying a result,
        // this took the calculator's result with it, half applied.
        Assert.Equal(template, TextFormat.Fill(template, 1, 2));
    }

    [Fact]
    public void A_template_without_a_placeholder_is_left_alone()
        => Assert.Equal("calc.clamped_step", TextFormat.Fill("calc.clamped_step", 1, 31, 28));

    [Fact]
    public void A_translation_that_can_be_filled_is_used_as_it_is()
        => Assert.Equal("krok 2", TextFormat.Translate(_ => "krok {0}", "calc.clamped_step", 2, 31, 28));

    [Fact]
    public void A_translation_that_cannot_be_filled_falls_back_to_the_default_language()
    {
        // The same sentence with the values in it, rather than the broken template on screen.
        var english = LocalizationService.DefaultTemplate("calc.clamped_step");
        Assert.NotNull(english); // not vacuous: the default file was found and has the key

        var text = TextFormat.Translate(_ => "krok {9}", "calc.clamped_step", 2, 31, 28);

        Assert.Equal(string.Format(System.Globalization.CultureInfo.InvariantCulture, english, 2, 31, 28), text);
    }

    [Fact]
    public void When_the_default_language_cannot_fill_it_either_the_translation_comes_back_as_written()
        => Assert.Equal("broken {7}", TextFormat.Translate(_ => "broken {7}", "no.such.key.anywhere", 1));

    /// <summary>A placeholder index, skipping escaped braces, with any alignment or format after it.</summary>
    private static readonly Regex Placeholder = new(@"(?<!\{)\{(\d+)(?:[,:][^{}]*)?\}", RegexOptions.Compiled);

    [Fact]
    public void Every_template_takes_the_same_values_in_both_languages()
    {
        // The guard at the source, beside the one in TextFormat for files a user edits: a Polish template
        // naming {2} where the English one names {1} would show the wrong value, or the raw template, in
        // one language only - and only to the people reading that language.
        //
        // Only keys present in both are compared here. A key missing from one language is a finding of its
        // own, and LocalizationTests.English_and_Polish_have_the_same_key_set already makes it.
        var english = Templates("en");
        var polish = Templates("pl");

        var mismatched = english.Keys
            .Where(polish.ContainsKey)
            .Where(key => !Holes(english[key]).SetEquals(Holes(polish[key])))
            .Order(StringComparer.Ordinal)
            .ToList();

        Assert.True(
            mismatched.Count == 0,
            "these templates take different values in English and Polish: " + string.Join(", ", mismatched));

        // The canary: a scan over files it cannot read would report no mismatch over nothing. The clamp
        // note is a three-value template the calculator fills, so it must be among those read.
        var withHoles = english.Where(pair => Holes(pair.Value).Count > 0).Select(pair => pair.Key).ToList();
        Assert.Contains("calc.clamped_step", withHoles);
        Assert.True(withHoles.Count >= 10, $"only {withHoles.Count} templates with placeholders were read");
    }

    private static Dictionary<string, string> Templates(string language)
    {
        var path = Path.Combine(TestPaths.AppDirectory(), "Localization", $"Strings.{language}.json");
        using var document = JsonDocument.Parse(
            File.ReadAllText(path),
            new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true });
        return document.RootElement.EnumerateObject()
            .Where(property => property.Value.ValueKind == JsonValueKind.String)
            .ToDictionary(property => property.Name, property => property.Value.GetString()!, StringComparer.Ordinal);
    }

    private static HashSet<int> Holes(string template)
        => Placeholder.Matches(template.Replace("{{", string.Empty, StringComparison.Ordinal))
            .Select(match => int.Parse(match.Groups[1].Value, System.Globalization.CultureInfo.InvariantCulture))
            .ToHashSet();
}
