// SPDX-License-Identifier: AGPL-3.0-or-later
// Test-only window inspection. No input, focus, visibility, or settings changes.
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class Quad9TrayStartupProbe {
    delegate bool WindowCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll", SetLastError = true)]
    static extern bool EnumWindows(WindowCallback callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetClassName(IntPtr window, StringBuilder name, int size);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] static extern int GetWindowStyle(IntPtr window, int index);

    public static void AssertNoApplicationWindow(uint processId) {
        string unexpected = null;
        bool complete = EnumWindows(delegate(IntPtr window, IntPtr data) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId || !IsWindowVisible(window)) return true;
            StringBuilder name = new StringBuilder(256);
            if (GetClassName(window, name, name.Capacity) == 0) {
                unexpected = "Cannot inspect an owned visible window.";
                return false;
            }
            // Tao and the single-instance plugin mark their event targets visible,
            // using layered, transparent, no-activate, tool-window styles to keep
            // them undisplayed. Exclude only these exact helpers.
            const int helperStyles = 0x08000000 | 0x00080000 | 0x00000080 | 0x00000020;
            if ((name.ToString() == "Tao Thread Event Target" || name.ToString() == "community.quad9.connect-sic") &&
                (GetWindowStyle(window, -20) & helperStyles) == helperStyles)
                return true;
            unexpected = "Tray startup displayed an owned window of class " + name + ".";
            return false;
        }, IntPtr.Zero);
        if (unexpected != null) throw new InvalidOperationException(unexpected);
        if (!complete) throw new Win32Exception(Marshal.GetLastWin32Error());
    }
}
