using System.Text;
using ChronoMock.Protocol;

namespace ChronoMock.Protocol.Tests;

/// <summary>
/// What the client does with the core's stdout (R4-W1, R4-N52): how it cuts the stream into lines, and what
/// it makes of each line. <see cref="StreamReader.ReadLineAsync()"/> did the first part before - it split on
/// '\r' too, had no bound, and patched bytes that are not UTF-8 into U+FFFD - so these pin the framing the
/// core and <c>chrono run</c> already use.
/// </summary>
public sealed class CoreStdoutTests
{
    private static ProtocolLineReader ReaderOver(byte[] bytes, int maxLine = ProtocolJson.MaxProtocolLine, bool trickle = false)
        => new(trickle ? new OneByteAtATime(bytes) : new MemoryStream(bytes), maxLine);

    private static byte[] Bytes(string text) => Encoding.UTF8.GetBytes(text);

    private static async Task<List<ProtocolLine>> ReadAll(ProtocolLineReader reader)
    {
        var lines = new List<ProtocolLine>();
        while (true)
        {
            var read = await reader.ReadAsync();
            lines.Add(read);
            if (read.Kind is ProtocolLineKind.Eof)
            {
                return lines;
            }
        }
    }

    [Fact]
    public async Task Lines_end_at_a_newline_and_nowhere_else()
    {
        var lines = await ReadAll(ReaderOver(Bytes("one\ntwo\rstill two\r\nthree\n")));

        Assert.Equal(
            new[] { new ProtocolLine(ProtocolLineKind.Text, "one"), new(ProtocolLineKind.Text, "two\rstill two"), new(ProtocolLineKind.Text, "three"), new(ProtocolLineKind.Eof) },
            lines);
    }

    /// <summary>A line that is not UTF-8 is reported and costs nothing after it - the old reader handed it on
    /// with U+FFFD in place of the bytes, a line nobody wrote.</summary>
    [Fact]
    public async Task A_line_that_is_not_utf8_is_skipped_and_the_next_one_arrives_whole()
    {
        var bytes = new List<byte> { 0x6B, 0x6F, 0x88, 0x6F, (byte)'\n' }; // a Polish word as CP852 writes it
        bytes.AddRange(Bytes("next\n"));

        var lines = await ReadAll(ReaderOver([.. bytes]));

        Assert.Equal(ProtocolLineKind.NotText, lines[0].Kind);
        Assert.Equal(new ProtocolLine(ProtocolLineKind.Text, "next"), lines[1]);
    }

    /// <summary>A line at the bound is read to its newline and dropped, so the read after it starts on the
    /// next line rather than in the middle of this one - also when it arrives a byte at a time.</summary>
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task A_line_at_the_bound_is_skipped_whole(bool trickle)
    {
        var lines = await ReadAll(ReaderOver(Bytes("1234567\n12345678\nafter\n"), maxLine: 8, trickle: trickle));

        Assert.Equal(new ProtocolLine(ProtocolLineKind.Text, "1234567"), lines[0]);
        Assert.Equal(ProtocolLineKind.TooLong, lines[1].Kind);
        Assert.Equal(new ProtocolLine(ProtocolLineKind.Text, "after"), lines[2]);
        Assert.Equal(ProtocolLineKind.Eof, lines[3].Kind);
    }

    /// <summary>The bound the client reads with by default is the mirrored one, counted the way the core
    /// counts it: one byte short of it passes, reaching it does not.</summary>
    [Fact]
    public async Task The_default_bound_is_the_protocol_line_bound()
    {
        var fits = new byte[ProtocolJson.MaxProtocolLine];
        Array.Fill(fits, (byte)'x');
        fits[^1] = (byte)'\n';
        var reaches = new byte[ProtocolJson.MaxProtocolLine + 1];
        Array.Fill(reaches, (byte)'x');
        reaches[^1] = (byte)'\n';

        Assert.Equal(ProtocolLineKind.Text, (await new ProtocolLineReader(new MemoryStream(fits)).ReadAsync()).Kind);
        Assert.Equal(ProtocolLineKind.TooLong, (await new ProtocolLineReader(new MemoryStream(reaches)).ReadAsync()).Kind);
    }

    /// <summary>A last line without its newline is still a line, and the end comes after it - as for the
    /// core's own reader. An over-long last line is reported, not lost.</summary>
    [Fact]
    public async Task The_last_line_needs_no_newline()
    {
        Assert.Equal(
            new[] { new ProtocolLine(ProtocolLineKind.Text, "last"), new(ProtocolLineKind.Eof) },
            await ReadAll(ReaderOver(Bytes("last"))));
        Assert.Equal(
            new[] { new ProtocolLine(ProtocolLineKind.TooLong), new(ProtocolLineKind.Eof) },
            await ReadAll(ReaderOver(Bytes("123456789"), maxLine: 8)));
        Assert.Equal(new[] { new ProtocolLine(ProtocolLineKind.Eof) }, await ReadAll(ReaderOver([])));
    }

    [Fact]
    public void An_event_of_this_protocol_version_is_handed_on()
    {
        var (evt, noise) = CoreClient.Interpret(new(ProtocolLineKind.Text, """{"type":"ack","v":1,"id":3}"""));

        Assert.IsType<AckEvent>(evt);
        Assert.Null(noise);
    }

    /// <summary>An event that names another version is not handed on, and the block says which version.</summary>
    [Fact]
    public void An_event_of_another_protocol_version_is_skipped_and_named()
    {
        var (evt, noise) = CoreClient.Interpret(new(ProtocolLineKind.Text, """{"type":"ack","v":2,"id":3}"""));

        Assert.Null(evt);
        Assert.NotNull(noise);
        Assert.Contains("protocol version 2", noise, StringComparison.Ordinal);
    }

    /// <summary>Except <c>ready</c>: it is the version check itself, and the handshake gate refuses a
    /// mismatched one by name. Dropped here, it would have turned "protocol mismatch" into "no handshake"
    /// after the whole wait.</summary>
    [Fact]
    public void A_ready_of_another_protocol_version_still_reaches_the_handshake()
    {
        var line = """{"type":"ready","v":2,"protocol":2,"core_version":"9.9.9","bitness":"x64"}""";

        var (evt, noise) = CoreClient.Interpret(new(ProtocolLineKind.Text, line));

        Assert.Equal(2, Assert.IsType<ReadyEvent>(evt).Protocol);
        Assert.Null(noise);
    }

    /// <summary>A line that is not JSON goes to the block with its start - never all of it, since it can be
    /// a megabyte long - and a type this build does not know is ignored in silence, as before.</summary>
    [Fact]
    public void A_line_that_is_not_an_event_is_described_by_its_start()
    {
        var (evt, noise) = CoreClient.Interpret(new(ProtocolLineKind.Text, "progress 42%" + new string('#', 1000)));

        Assert.Null(evt);
        Assert.NotNull(noise);
        Assert.Contains("progress 42%", noise, StringComparison.Ordinal);
        Assert.EndsWith("...", noise, StringComparison.Ordinal);
        Assert.True(noise.Length < 400, $"the sample is not cut: {noise.Length} characters");

        Assert.Equal((null, null), CoreClient.Interpret(new(ProtocolLineKind.Text, """{"type":"future_thing","v":1}""")));
        Assert.Equal((null, null), CoreClient.Interpret(new(ProtocolLineKind.Text, "")));
    }

    /// <summary>The two lines the reader could not use are named for what they were.</summary>
    [Fact]
    public void Lines_the_reader_could_not_use_are_named()
    {
        var (notText, notTextNoise) = CoreClient.Interpret(new(ProtocolLineKind.NotText));
        var (tooLong, tooLongNoise) = CoreClient.Interpret(new(ProtocolLineKind.TooLong));

        Assert.Null(notText);
        Assert.NotNull(notTextNoise);
        Assert.Contains("not UTF-8", notTextNoise, StringComparison.Ordinal);
        Assert.Null(tooLong);
        Assert.NotNull(tooLongNoise);
        Assert.Contains($"{ProtocolJson.MaxProtocolLine} bytes", tooLongNoise, StringComparison.Ordinal);
    }

    /// <summary>A stream that hands out one byte per read, so a line crosses every read boundary there is.</summary>
    private sealed class OneByteAtATime(byte[] bytes) : Stream
    {
        private int _at;

        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => bytes.Length;
        public override long Position { get => _at; set => throw new NotSupportedException(); }

        public override int Read(byte[] buffer, int offset, int count)
        {
            if (_at == bytes.Length || count == 0)
            {
                return 0;
            }

            buffer[offset] = bytes[_at++];
            return 1;
        }

        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }
}
