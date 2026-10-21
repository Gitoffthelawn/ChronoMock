using System.IO; // The WPF SDK trims System.IO from implicit usings (Path collides with Shapes.Path).
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

namespace ChronoMock.App;

/// <summary>What a Windows shortcut starts: the program, the arguments it hands it, and the folder it starts
/// it in.</summary>
internal sealed record ShortcutTarget(string Program, string Arguments, string WorkingFolder);

/// <summary>
/// Reading a shortcut (.lnk) through the shell's own link object, IShellLinkW, never by parsing the file
/// (R5-D80). The file format has data blocks for environment strings, known folders and installer links
/// that the shell already resolves, and a reader of our own would get one of them wrong.
/// </summary>
/// <remarks>
/// Nothing is resolved: Resolve can search the disk and show a window, so a moved target is reported as
/// missing rather than looked for. An installer's advertised shortcut (SLDF_HAS_DARWINID) names an icon
/// file in the installer's cache where a path should be - sometimes an ".exe" - so it is refused as naming
/// no program rather than started. The interfaces are declared in the Windows SDK's order
/// (ShObjIdl_core.h), and only as far as the last method used.
///
/// Read from a pool thread, which is in the multithreaded apartment: CLSID_ShellLink is registered with the
/// threading model Both (read from the registry, InProcServer32), so it lives in the caller's apartment and
/// needs no message pump. The intake test reads a real shortcut down that very path.
/// </remarks>
internal static class ShellShortcut
{
    /// <summary>CLSID_ShellLink.</summary>
    private static readonly Guid ShellLinkClass = new("00021401-0000-0000-C000-000000000046");

    /// <summary>The longest path Windows names, in characters - a shortcut's arguments are bounded by it too.</summary>
    private const int LongestText = 32768;

    /// <summary>SLDF_HAS_DARWINID: a shortcut an installer advertises, which names no program file.</summary>
    private const uint AdvertisedByAnInstaller = 0x00001000;

    /// <summary>STGM_READ.</summary>
    private const int ReadOnly = 0;

    /// <summary>The program a shortcut starts, with its arguments and working folder - or null when the file
    /// is not a shortcut the shell can read, or names no program a session could start.</summary>
    public static ShortcutTarget? Read(string path)
    {
        object? link = null;
        try
        {
            link = Activator.CreateInstance(Type.GetTypeFromCLSID(ShellLinkClass, throwOnError: true)!);
            ((IPersistFile)link!).Load(path, ReadOnly);
            if (((IShellLinkDataList)link).GetFlags(out var flags) == 0 && (flags & AdvertisedByAnInstaller) != 0)
            {
                return null;
            }

            var shell = (IShellLinkW)link;
            var text = new StringBuilder(LongestText);
            if (shell.GetPath(text, text.Capacity, IntPtr.Zero, 0) != 0 || text.Length == 0)
            {
                return null; // S_FALSE: the shortcut names no file system path
            }

            var program = text.ToString();
            text.Clear();
            var arguments = shell.GetArguments(text, text.Capacity) == 0 ? text.ToString() : string.Empty;
            text.Clear();
            var folder = shell.GetWorkingDirectory(text, text.Capacity) == 0 ? text.ToString() : string.Empty;
            return new ShortcutTarget(
                Environment.ExpandEnvironmentVariables(program),
                arguments,
                Environment.ExpandEnvironmentVariables(folder));
        }
        catch (Exception fault) when (fault is COMException or IOException or UnauthorizedAccessException
                                          or InvalidCastException or ArgumentException)
        {
            return null; // not a shortcut, or one the shell will not read
        }
        finally
        {
            if (link is not null && Marshal.IsComObject(link))
            {
                Marshal.FinalReleaseComObject(link);
            }
        }
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("000214F9-0000-0000-C000-000000000046")]
    private interface IShellLinkW
    {
        [PreserveSig]
        int GetPath([MarshalAs(UnmanagedType.LPWStr)] StringBuilder file, int capacity, IntPtr findData, uint flags);

        [PreserveSig]
        int GetIDList(out IntPtr list);

        [PreserveSig]
        int SetIDList(IntPtr list);

        [PreserveSig]
        int GetDescription([MarshalAs(UnmanagedType.LPWStr)] StringBuilder name, int capacity);

        [PreserveSig]
        int SetDescription([MarshalAs(UnmanagedType.LPWStr)] string name);

        [PreserveSig]
        int GetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] StringBuilder folder, int capacity);

        [PreserveSig]
        int SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string folder);

        [PreserveSig]
        int GetArguments([MarshalAs(UnmanagedType.LPWStr)] StringBuilder arguments, int capacity);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("45e2b4ae-b1c3-11d0-b92f-00a0c90312e1")]
    private interface IShellLinkDataList
    {
        [PreserveSig]
        int AddDataBlock(IntPtr block);

        [PreserveSig]
        int CopyDataBlock(uint signature, out IntPtr block);

        [PreserveSig]
        int RemoveDataBlock(uint signature);

        [PreserveSig]
        int GetFlags(out uint flags);
    }
}
