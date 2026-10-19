// SPDX-License-Identifier: AGPL-3.0-or-later
// Test harness only. No CPU, memory, priority, UI, or breakaway limits are set.
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public sealed class Quad9ResourceJob : IDisposable {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Startup {
        public int cb; public string reserved, desktop, title;
        public int x, y, xSize, ySize, xChars, yChars, fill, flags;
        public short show, reservedSize; public IntPtr reservedData, input, output, error;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInfo { public IntPtr process, thread; public uint pid, tid; }
    [StructLayout(LayoutKind.Sequential)]
    struct Accounting {
        public long user, kernel, periodUser, periodKernel;
        public uint faults, total, active, terminated;
    }
    public sealed class Counters {
        public long CpuTicks; public uint TotalProcesses; public uint ActiveProcesses;
        public uint[] ProcessIds;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateJobObject(IntPtr security, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr OpenJobObject(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcess(string executable, StringBuilder command, IntPtr processSecurity,
        IntPtr threadSecurity, bool inherit, uint flags, IntPtr environment, string cwd,
        ref Startup startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool QueryInformationJobObject(IntPtr job, int kind, IntPtr info, int size, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")] static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool member);
    IntPtr job;
    public uint ProcessId { get; private set; }
    public Quad9ResourceJob(string executable, string arguments, string name) {
        job = CreateJobObject(IntPtr.Zero, name);
        int error = Marshal.GetLastWin32Error();
        if (job == IntPtr.Zero) throw new Win32Exception(error);
        if (error == 183) { CloseHandle(job); job = IntPtr.Zero; throw new InvalidOperationException("Job name already exists."); }
        ProcessInfo process = new ProcessInfo();
        try {
            Startup startup = new Startup(); startup.cb = Marshal.SizeOf(typeof(Startup));
            // Assign before any application code runs, so no child can be missed.
            if (!CreateProcess(executable, new StringBuilder("\"" + executable + "\" " + arguments),
                IntPtr.Zero, IntPtr.Zero, false, 4, IntPtr.Zero, null, ref startup, out process))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!AssignProcessToJobObject(job, process.process)) throw new Win32Exception(Marshal.GetLastWin32Error());
            ProcessId = process.pid;
            if (ResumeThread(process.thread) == uint.MaxValue) throw new Win32Exception(Marshal.GetLastWin32Error());
        } catch {
            if (process.process != IntPtr.Zero) TerminateProcess(process.process, 1);
            Dispose(); throw;
        } finally {
            if (process.thread != IntPtr.Zero) CloseHandle(process.thread);
            if (process.process != IntPtr.Zero) CloseHandle(process.process);
        }
    }
    public static Counters Read(string name) {
        IntPtr handle = OpenJobObject(4, false, name); // JOB_OBJECT_QUERY
        if (handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            int size = Marshal.SizeOf(typeof(Accounting));
            IntPtr buffer = Marshal.AllocHGlobal(size);
            Accounting accounting;
            try {
                if (!QueryInformationJobObject(handle, 1, buffer, size, IntPtr.Zero))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                accounting = (Accounting)Marshal.PtrToStructure(buffer, typeof(Accounting));
            } finally { Marshal.FreeHGlobal(buffer); }
            for (int capacity = 64; capacity <= 65536; capacity *= 2) {
                size = 8 + capacity * IntPtr.Size;
                buffer = Marshal.AllocHGlobal(size);
                try {
                    if (!QueryInformationJobObject(handle, 3, buffer, size, IntPtr.Zero)) {
                        int error = Marshal.GetLastWin32Error();
                        if (error == 234) continue;
                        throw new Win32Exception(error);
                    }
                    int count = Marshal.ReadInt32(buffer, 4);
                    if (count < 0 || count > capacity) throw new InvalidOperationException("Invalid job process count.");
                    uint[] ids = new uint[count];
                    for (int i = 0; i < count; i++) ids[i] = unchecked((uint)Marshal.ReadIntPtr(buffer, 8 + i * IntPtr.Size).ToInt64());
                    return new Counters { CpuTicks = checked(accounting.user + accounting.kernel),
                        TotalProcesses = accounting.total, ActiveProcesses = accounting.active, ProcessIds = ids };
                } finally { Marshal.FreeHGlobal(buffer); }
            }
            throw new InvalidOperationException("Job process list exceeds the test harness limit.");
        } finally { CloseHandle(handle); }
    }
    public static bool? Contains(string name, uint pid) {
        IntPtr process = OpenProcess(0x400, false, pid);
        if (process == IntPtr.Zero) {
            int error = Marshal.GetLastWin32Error();
            if (error == 87) return null; // Exited between CIM and job snapshots.
            throw new Win32Exception(error);
        }
        try {
            IntPtr handle = OpenJobObject(4, false, name);
            if (handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            try {
                bool member;
                if (!IsProcessInJob(process, handle, out member)) throw new Win32Exception(Marshal.GetLastWin32Error());
                return member;
            } finally { CloseHandle(handle); }
        } finally { CloseHandle(process); }
    }
    public void Dispose() {
        if (job != IntPtr.Zero) {
            // Only this freshly created test job, never a pre-existing app/service.
            TerminateJobObject(job, 0); CloseHandle(job); job = IntPtr.Zero;
        }
    }
}
