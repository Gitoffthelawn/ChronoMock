using System.Text;

namespace ChronoMock.Protocol;

/// <summary>What one read of the core's stdout found (see <see cref="ProtocolLineReader"/>).</summary>
internal enum ProtocolLineKind
{
    /// <summary>The stream ended.</summary>
    Eof,

    /// <summary>A line of text, without its line ending.</summary>
    Text,

    /// <summary>A line whose bytes are not UTF-8, skipped.</summary>
    NotText,

    /// <summary>A line of <see cref="ProtocolJson.MaxProtocolLine"/> bytes or more, read to its end and skipped.</summary>
    TooLong,
}

/// <summary>One read's result: what it found, and the line when that is a line of text.</summary>
internal readonly record struct ProtocolLine(ProtocolLineKind Kind, string Text = "");

/// <summary>
/// Reads the core's stdout one protocol line at a time: the bytes up to a '\n' and nothing else, bounded,
/// and decoded as UTF-8 that is refused rather than repaired.
/// <para>
/// <see cref="StreamReader.ReadLineAsync()"/> read this stream before, and it framed lines differently from
/// the core and from <c>chrono run</c> in three ways (R4-N52). It split on '\r' as well, which NDJSON does
/// not. It had no bound, so a stream with no newline grew one string for as long as it ran. And it replaced
/// bytes that are not UTF-8 with U+FFFD, which hands on a line nobody wrote. This reader frames lines the way
/// the other two readers do (<c>wire::read_line_bytes</c>) and reports a line it cannot use instead of
/// patching it up, so one bad line costs that line and nothing after it.
/// </para>
/// </summary>
internal sealed class ProtocolLineReader
{
    private static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly Stream _stream;
    private readonly int _maxLine;
    private readonly byte[] _chunk = new byte[64 * 1024];
    private int _chunkStart;
    private int _chunkEnd;
    private byte[] _line = new byte[4 * 1024];
    private int _lineLength;

    internal ProtocolLineReader(Stream stream, int maxLine = ProtocolJson.MaxProtocolLine)
    {
        ArgumentNullException.ThrowIfNull(stream);
        ArgumentOutOfRangeException.ThrowIfLessThan(maxLine, 1);
        _stream = stream;
        _maxLine = maxLine;
    }

    /// <summary>
    /// The next line. A line that runs to the bound is read on to its newline and reported as
    /// <see cref="ProtocolLineKind.TooLong"/>, so the read after it starts on the next line. A last line
    /// without its newline is still a line, as it is for the core's own reader.
    /// </summary>
    internal async Task<ProtocolLine> ReadAsync()
    {
        _lineLength = 0;
        var tooLong = false;
        while (true)
        {
            if (_chunkStart == _chunkEnd)
            {
                var read = await _stream.ReadAsync(_chunk.AsMemory()).ConfigureAwait(false);
                if (read == 0)
                {
                    return tooLong ? new(ProtocolLineKind.TooLong) : EndOfStream();
                }

                _chunkStart = 0;
                _chunkEnd = read;
            }

            var available = _chunk.AsSpan(_chunkStart, _chunkEnd - _chunkStart);
            var newline = available.IndexOf((byte)'\n');
            var part = newline >= 0 ? available[..newline] : available;
            _chunkStart += newline >= 0 ? newline + 1 : part.Length;
            tooLong = tooLong || !Append(part);
            if (newline >= 0)
            {
                return tooLong ? new(ProtocolLineKind.TooLong) : Decode();
            }
        }
    }

    /// <summary>What the end of the stream leaves: the last line when it had no newline, or the end itself.</summary>
    private ProtocolLine EndOfStream() => _lineLength == 0 ? new(ProtocolLineKind.Eof) : Decode();

    /// <summary>Add a part of the line, or say it no longer fits. Past the bound nothing more is kept - the
    /// rest of the line is only read, to find where the next one starts.</summary>
    private bool Append(ReadOnlySpan<byte> part)
    {
        if (_lineLength + part.Length >= _maxLine)
        {
            _lineLength = 0;
            return false;
        }

        if (_lineLength + part.Length > _line.Length)
        {
            Array.Resize(ref _line, Math.Min(_maxLine, Math.Max(_line.Length * 2, _lineLength + part.Length)));
        }

        part.CopyTo(_line.AsSpan(_lineLength));
        _lineLength += part.Length;
        return true;
    }

    /// <summary>The line as text, a CRLF ending taken off as the other two readers take it, or word that its
    /// bytes are not UTF-8.</summary>
    private ProtocolLine Decode()
    {
        var length = _lineLength;
        if (length > 0 && _line[length - 1] == (byte)'\r')
        {
            length--;
        }

        try
        {
            return new(ProtocolLineKind.Text, StrictUtf8.GetString(_line, 0, length));
        }
        catch (DecoderFallbackException)
        {
            return new(ProtocolLineKind.NotText);
        }
    }
}
