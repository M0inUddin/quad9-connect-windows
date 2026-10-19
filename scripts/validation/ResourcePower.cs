// SPDX-License-Identifier: AGPL-3.0-or-later
// Test harness only. No input, focus, power policy, or security policy changes.
using System;
using System.Runtime.InteropServices;

public sealed class Quad9ResourcePowerLease : IDisposable {
    const uint Continuous = 0x80000000;
    const uint DisplayAndSystemRequired = Continuous | 3;
    [DllImport("kernel32.dll")] static extern uint SetThreadExecutionState(uint flags);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    readonly uint thread;
    readonly uint previous;
    bool disposed;
    public Quad9ResourcePowerLease() {
        thread = GetCurrentThreadId();
        previous = SetThreadExecutionState(DisplayAndSystemRequired);
        if (previous == 0) throw new InvalidOperationException("Windows refused the benchmark power request.");
    }
    public void Dispose() {
        if (disposed) return;
        if (GetCurrentThreadId() != thread) throw new InvalidOperationException("Release the benchmark power request on its creating thread.");
        if (SetThreadExecutionState(previous | Continuous) == 0)
            throw new InvalidOperationException("Windows refused to restore the thread execution state.");
        disposed = true;
    }
}
