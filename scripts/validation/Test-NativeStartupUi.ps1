# SPDX-License-Identifier: AGPL-3.0-or-later
# Windows accessibility-provider test of the installed client, not browser IPC.
param([Parameter(Mandatory)][int] $DesktopProcessId,
    [Parameter(Mandatory)][string] $Executable, [Parameter(Mandatory)][string] $Output)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Startup UI writes require the disposable hosted VM.' }
$env:PSModulePath = Join-Path $PSHOME 'Modules'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class Quad9StartupUiWindow {
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int left, top, right, bottom; }
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out Rect bounds);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
    public static bool Capture(IntPtr window, string path) {
        Rect bounds;
        if (!GetWindowRect(window, out bounds)) return false;
        int width = bounds.right - bounds.left, height = bounds.bottom - bounds.top;
        if (width < 1 || height < 1 || width > 4096 || height > 4096) return false;
        using (var bitmap = new Bitmap(width, height)) {
            using (var graphics = Graphics.FromImage(bitmap)) {
                IntPtr dc = graphics.GetHdc();
                try { if (!PrintWindow(window, dc, 2)) return false; }
                finally { graphics.ReleaseHdc(dc); }
            }
            bitmap.Save(path, ImageFormat.Png);
        }
        return true;
    }
}
'@
$desktop = [Diagnostics.Process]::GetProcessById($DesktopProcessId)
if ($desktop.MainModule.FileName -ine [IO.Path]::GetFullPath($Executable)) { throw 'The test PID does not own the expected executable.' }
$module = Import-Module (Join-Path (Split-Path $Executable -Parent) 'InstallerLifecycle.psm1') -Force -PassThru
$script:window = $null
function Until([scriptblock] $Probe, [string] $Message, [int] $Seconds = 35) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    $lastError = ''
    do {
        try { $value = & $Probe; if ($value) { return $value } }
        catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 150
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "$Message $lastError"
}
function Find([string] $Name, [object] $Type, [object] $Root = $script:window) {
    $nameCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $Name)
    $condition = if ($Type) {
        [Windows.Automation.AndCondition]::new($nameCondition,
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, $Type))
    } else { $nameCondition }
    return $Root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
}
function Read-Window {
    $script:window = Until {
        $desktop.Refresh()
        if ($desktop.HasExited) { throw 'The owned desktop host exited.' }
        if ($desktop.MainWindowHandle -ne [IntPtr]::Zero) {
            [Windows.Automation.AutomationElement]::FromHandle($desktop.MainWindowHandle)
        }
    } 'The owned application window did not open.'
}
function Invoke-Button([string] $Name) {
    $button = Until { Find $Name ([Windows.Automation.ControlType]::Button) } "Missing button: $Name"
    $button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
}
function Startup-Group { Find 'Start Quad9 when signing in to Windows' ([Windows.Automation.ControlType]::Group) }
function Radio([string] $Name, [object] $Group = $script:window) { Find $Name ([Windows.Automation.ControlType]::RadioButton) $Group }
function Selected([object] $Element) {
    $Element -and $Element.Current.IsEnabled -and
        $Element.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Current.IsSelected
}
function Select-Radio([string] $Name, [object] $Group = $script:window) {
    $element = Until { Radio $Name $Group } "Missing radio: $Name"
    if (-not $element.Current.IsEnabled) { throw "The requested radio is disabled: $Name" }
    $element.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Select()
}
function Read-Mode { (& $module { param($Desktop); Invoke-Quad9StartupPreference $Desktop } $Executable).mode }
function Assert-Mode([string] $Label, [string] $Mode) {
    $null = Until { $group = Startup-Group; $group -and (Selected (Radio $Label $group)) } "The UI did not settle on $Label."
    if ((Read-Mode) -cne $Mode) { throw "The actual Windows task does not match $Label." }
}
function Launch-Second([string] $Arguments = '') {
    $start = [Diagnostics.ProcessStartInfo]::new($Executable, $Arguments)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $second = [Diagnostics.Process]::Start($start)
    try {
        if (-not $second.WaitForExit(10000)) { $second.Kill(); throw 'Second launch did not exit.' }
        if ($second.ExitCode -ne 0) { throw 'Second launch failed.' }
    } finally { $second.Dispose() }
}
try {
    Read-Window
    $null = Until { Find 'Ready when you are' $null } 'The installed app did not load its authenticated off state.'
    Invoke-Button 'Settings'
    $null = Until { Startup-Group } 'The installed Windows startup controls did not load.'
    Assert-Mode 'Off' 'off'
    Select-Radio 'Dark'
    $navigated = $false
    foreach ($choice in @(@('Tray','tray'), @('Open window','window'), @('Off','off'), @('Tray','tray'))) {
        Select-Radio $choice[0] (Startup-Group)
        if ($choice[0] -eq 'Tray' -and -not $navigated) {
            Invoke-Button 'Connection'
            Invoke-Button 'Settings'
            $navigated = $true
        }
        Assert-Mode $choice[0] $choice[1]
    }
    Launch-Second '--start-in-tray'
    Assert-Mode 'Tray' 'tray'
    if (-not (Selected (Radio 'Dark'))) { throw 'Repeated tray launch lost the visible settings/theme.' }
    $desktop.Refresh()
    $oldWindow = $desktop.MainWindowHandle
    if (-not [Quad9StartupUiWindow]::PostMessage($oldWindow, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'Could not request closure of the owned window.' }
    $null = Until { -not [Quad9StartupUiWindow]::IsWindow($oldWindow) } 'The idle window did not release.' 15
    Launch-Second
    Read-Window
    Invoke-Button 'Settings'
    Assert-Mode 'Tray' 'tray'
    if (-not (Selected (Radio 'Dark'))) { throw 'Reopening the interface lost its theme.' }
    $desktop.Refresh()
    $screenshot = [Quad9StartupUiWindow]::Capture($desktop.MainWindowHandle, (Join-Path $Output 'native-windows-startup.png'))
    @{ passed = $true; driver = 'Windows UI Automation'; mode = 'tray'; pending_save_navigation = $true;
        repeated_tray_launch = $true; window_recreation = $true; actual_task_readback = $true; screenshot_captured = $screenshot } | ConvertTo-Json -Compress
} catch {
    # Names/control types from the owned window only. This is diagnostic output,
    # not an input source, and the parent bounds this worker's entire lifetime.
    if ($script:window) {
        @($script:window.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) |
            ForEach-Object { @{ name = $_.Current.Name; type = $_.Current.ControlType.ProgrammaticName; enabled = $_.Current.IsEnabled } }) |
            ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $Output 'native-accessibility-tree.json') -Encoding UTF8
    }
    throw
} finally { $desktop.Dispose() }
