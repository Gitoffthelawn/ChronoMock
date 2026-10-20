namespace ChronoMock.Protocol;

/// <summary>
/// An engine question written as a PowerShell command line that asks it exactly (R5-D54): the call operator, the
/// engine by its full path, flags and plain words bare, every other value in single quotes. PowerShell because
/// it is what a Windows 11 terminal opens, and the full path because a portable copy puts nothing on PATH.
/// <para>
/// 🔴 A value is quoted unless it is a plain word, never only when it has a space. PowerShell gives a meaning
/// to the semicolon (it ends the command - measured, the rest of the line is not even parsed), <c>$</c>, <c>#</c>,
/// <c>@</c>, brackets, the backtick and the space, and a format mask may hold any of them. Number-like tokens
/// are not among them: <c>10d</c>, <c>2kb</c> and <c>0x10</c> reach a program as written in PowerShell 5.1 and
/// 7.6 alike (measured with <c>cmd /c echo</c>). Inside single quotes nothing is read but the quote, and a
/// format mask uses single quotes for its literal text (<c>'at' HH:mm</c>), so each one is doubled - and
/// PowerShell takes four more characters as a single quote, which are doubled too.
/// </para>
/// </summary>
public static class PowerShellCommand
{
    /// <summary>The first line of a command that Windows PowerShell 5.1 would pass to the engine wrongly. A
    /// comment, so the pasted text still runs, and it says what to run it in instead.</summary>
    public const string NeedsPowerShell7 =
        "# Run this in PowerShell 7.3 or later - Windows PowerShell 5.1 passes an empty argument, a double quote, "
        + "or a backslash ending an argument with a space in it, to the program wrongly";

    /// <summary>The command line that runs <paramref name="executable"/> with <paramref name="arguments"/>.</summary>
    public static string For(string executable, IReadOnlyList<string> arguments)
    {
        ArgumentNullException.ThrowIfNull(executable);
        ArgumentNullException.ThrowIfNull(arguments);
        var line = "& " + Quote(executable) + string.Concat(arguments.Select(a => " " + (IsBare(a) ? a : Quote(a))));
        return arguments.Any(PassedWronglyByWindowsPowerShell) ? NeedsPowerShell7 + "\r\n" + line : line;
    }

    /// <summary>
    /// A word PowerShell can only read as itself, left bare so the command reads as one would type it: a flag
    /// (<c>--calendar</c>) or a word (<c>calc</c>, <c>today</c>, <c>us-banking</c>) - a lower-case letter first,
    /// then letters, digits and hyphens. None of the characters PowerShell gives a meaning to can appear in it.
    /// </summary>
    private static bool IsBare(string argument)
    {
        var word = argument.StartsWith("--", StringComparison.Ordinal) ? argument[2..] : argument;
        return word.Length > 0 && char.IsAsciiLetterLower(word[0])
               && word.All(c => char.IsAsciiLetterLower(c) || char.IsAsciiDigit(c) || c == '-');
    }

    private static string Quote(string text)
    {
        var quoted = new System.Text.StringBuilder(text.Length + 2).Append('\'');
        foreach (var c in text)
        {
            quoted.Append(c);
            if (IsSingleQuote(c))
            {
                quoted.Append(c);
            }
        }

        return quoted.Append('\'').ToString();
    }

    /// <summary>The characters PowerShell ends a single-quoted string at: the apostrophe and the left, right,
    /// low and reversed single quotation marks. Written as numbers, so a review can tell the five apart.</summary>
    private static bool IsSingleQuote(char c) => c is (char)0x27 or (char)0x2018 or (char)0x2019 or (char)0x201A or (char)0x201B;

    /// <summary>
    /// What Windows PowerShell 5.1 passes to a program differently from how it is written: it drops an empty
    /// argument, does not escape a double quote inside one, and lets a backslash ending an argument it has to
    /// put in quotes escape that closing quote. PowerShell 7.3 and later pass all three as written.
    /// </summary>
    private static bool PassedWronglyByWindowsPowerShell(string argument)
        => argument.Length == 0 || argument.Contains('"', StringComparison.Ordinal)
           || (argument.Any(char.IsWhiteSpace) && argument.EndsWith('\\'));
}
