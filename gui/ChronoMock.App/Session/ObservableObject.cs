using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace ChronoMock.App;

/// <summary>
/// Minimal hand-rolled <see cref="INotifyPropertyChanged"/>. No MVVM package is pulled in for one screen -
/// a controls/MVVM dependency would be a rule-8 licence-sieve event, not worth it here (gui-and-cli-constraints).
/// </summary>
public abstract class ObservableObject : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>Set a backing field and raise <see cref="PropertyChanged"/> only when the value actually changes.</summary>
    protected bool Set<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value))
        {
            return false;
        }

        field = value;
        RaisePropertyChanged(name);
        return true;
    }

    /// <summary>
    /// Set a list and raise <see cref="PropertyChanged"/> only when what it holds changes, item by item.
    /// <see cref="Set{T}"/> compares a list by reference, so a list rebuilt with the same items announced a
    /// change all the same - the audit is rebuilt from every coverage report, and a family of 256 processes
    /// reporting the same channels re-ran every binding on the audit 256 times for nothing (R5-WY4).
    /// </summary>
    protected bool SetItems<T>(ref IReadOnlyList<T> field, IReadOnlyList<T> value, [CallerMemberName] string? name = null)
    {
        ArgumentNullException.ThrowIfNull(value);
        if (field.SequenceEqual(value))
        {
            return false;
        }

        field = value;
        RaisePropertyChanged(name);
        return true;
    }

    /// <summary>Raise <see cref="PropertyChanged"/> for a property, e.g. a computed one that depends on another.</summary>
    protected void RaisePropertyChanged(string? name)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
