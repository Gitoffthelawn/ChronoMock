namespace ChronoMock.Protocol.Tests;

/// <summary>
/// An engine question as a PowerShell command line (R5-D54): what is left bare, what is quoted and how, and when
/// the command says it needs PowerShell 7.
/// </summary>
public class PowerShellCommandTests
{
    private const string Engine = @"C:\Program Files\Chrono Mock\core\x64\chrono.exe";

    /// <summary>
    /// Flags and plain words stay bare, and everything else is quoted - a value can carry a character
    /// PowerShell gives a meaning to, and a bare semicolon ends the command. Reversal probe: leave values with
    /// no space bare and the zone and step are bare.
    /// </summary>
    [Fact]
    public void Flags_and_words_are_bare_and_every_other_value_is_quoted()
    {
        var command = PowerShellCommand.For(
            Engine, ["calc", "--base", "today", "--shift", "+10d", "--calendar", "us-banking", "--zone", "+02:00"]);

        Assert.Equal(
            @"& 'C:\Program Files\Chrono Mock\core\x64\chrono.exe' calc --base today --shift '+10d' --calendar us-banking --zone '+02:00'",
            command);
    }

    /// <summary>A value that starts with a digit or holds anything but lower-case letters, digits and hyphens is
    /// quoted, whatever else it looks like.</summary>
    [Theory]
    [InlineData("10d")]
    [InlineData("2kb")]
    [InlineData("2026-10-20")]
    [InlineData("Today")]
    [InlineData("$env:PATH")]
    [InlineData("a,b")]
    [InlineData("x;y")]
    [InlineData("-3d")]
    [InlineData("--Base")]
    public void A_value_that_is_not_a_plain_word_is_quoted(string value)
        => Assert.EndsWith(" '" + value + "'", PowerShellCommand.For(Engine, [value]), StringComparison.Ordinal);

    /// <summary>
    /// A single quote is doubled inside the quotes - the format mask's own quoting of literal text - and so is
    /// each of the four other characters PowerShell ends a single-quoted string at. Reversal probe: leave one
    /// undoubled and the string ends early.
    /// </summary>
    [Fact]
    public void Every_kind_of_single_quote_is_doubled()
    {
        var right = (char)0x2019;
        var low = (char)0x201A;

        Assert.EndsWith(" '''at'' HH:mm'", PowerShellCommand.For(Engine, ["'at' HH:mm"]), StringComparison.Ordinal);
        Assert.EndsWith(
            $" 'it{right}{right}s {low}{low}x'", PowerShellCommand.For(Engine, [$"it{right}s {low}x"]), StringComparison.Ordinal);
        Assert.StartsWith("& 'C:\\O''Brien\\chrono.exe'", PowerShellCommand.For(@"C:\O'Brien\chrono.exe", []), StringComparison.Ordinal);
    }

    /// <summary>
    /// What Windows PowerShell 5.1 passes wrongly puts a comment first that says to run it in PowerShell 7.3 -
    /// and only that: an ordinary command has no first line to step over.
    /// </summary>
    [Theory]
    [InlineData("say \"hi\"")]
    [InlineData("")]
    [InlineData(@"a folder\")]
    public void A_command_windows_powershell_would_pass_wrongly_says_so_first(string value)
    {
        var command = PowerShellCommand.For(Engine, ["calc", "--format", value]);

        Assert.StartsWith(PowerShellCommand.NeedsPowerShell7 + "\r\n& ", command, StringComparison.Ordinal);
        Assert.StartsWith("# ", PowerShellCommand.NeedsPowerShell7, StringComparison.Ordinal);
    }

    [Theory]
    [InlineData(@"no-space\")]
    [InlineData("yyyy-MM-dd")]
    public void An_ordinary_command_is_one_line(string value)
        => Assert.DoesNotContain("\n", PowerShellCommand.For(Engine, ["calc", "--format", value]), StringComparison.Ordinal);
}
