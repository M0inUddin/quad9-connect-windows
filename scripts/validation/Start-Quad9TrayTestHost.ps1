# SPDX-License-Identifier: AGPL-3.0-or-later
# Functional test only. The fresh job owns the desktop and all its descendants.
param([Parameter(Mandatory)][string] $Executable, [Parameter(Mandatory)][string] $JobName)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ResourceJob.cs')
Add-Type -Path (Join-Path $PSScriptRoot 'TrayStartupProbe.cs')
$errorPath = if ($env:QUAD9_TRAY_DIAGNOSTICS) { Join-Path (Split-Path $env:QUAD9_TRAY_DIAGNOSTICS -Parent) 'native-host.log' } else { $null }
$owner = [Quad9ResourceJob]::new([IO.Path]::GetFullPath($Executable), '--start-in-tray', $JobName, $errorPath)
try {
    $native = [Diagnostics.Process]::GetProcessById($owner.ProcessId)
    try {
        $created = ([DateTimeOffset]$native.StartTime).ToUnixTimeMilliseconds()
        function Assert-TrayOnly {
            $counters = [Quad9ResourceJob]::Read($JobName)
            # Cumulative total catches even a WebView that was created and exited
            # between snapshots. No child process is needed for a native tray.
            if ($counters.TotalProcesses -ne 1 -or $counters.ActiveProcesses -ne 1 -or
                $counters.ProcessIds.Count -ne 1 -or $counters.ProcessIds[0] -ne $owner.ProcessId) {
                throw 'Tray startup created a child process or lost its original host.'
            }
            [Quad9TrayStartupProbe]::AssertNoApplicationWindow($owner.ProcessId)
        }
        function Check-SecondLaunch([string] $Arguments, [int] $ExpectedCode) {
            $start = [Diagnostics.ProcessStartInfo]::new()
            $start.FileName = [IO.Path]::GetFullPath($Executable)
            $start.Arguments = $Arguments
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $second = [Diagnostics.Process]::Start($start)
            try {
                if (-not $second.WaitForExit(10000)) {
                    $second.Kill()
                    throw 'Second launch did not exit.'
                }
                if ($second.ExitCode -ne $ExpectedCode) { throw "Unexpected second-launch exit code: $($second.ExitCode)" }
            } finally { $second.Dispose() }
        }
        foreach ($phase in 1..2) {
            foreach ($sample in 1..20) {
                Assert-TrayOnly
                Start-Sleep -Milliseconds 100
            }
            if ($phase -eq 1) {
                Check-SecondLaunch '--start-in-tray' 0
                Check-SecondLaunch '--enable' 2
            }
        }
        Assert-TrayOnly
        [Console]::Out.WriteLine((@{
            pid = $owner.ProcessId
            started_unix_ms = $created
            tray_checked_unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            total_processes_before_open = 1
            repeated_tray_launch_stayed_hidden = $true
            unsupported_launch_rejected = $true
        } | ConvertTo-Json -Compress))
        # Node opens the existing host and checks its real embedded frontend.
        # EOF also terminates this job if the driver fails.
        if ([Console]::ReadLine() -eq 'check-open') {
            # Negative control: neither probe may silently accept the real window.
            $detected = $false
            try { [Quad9TrayStartupProbe]::AssertNoApplicationWindow($owner.ProcessId) }
            catch {
                if (-not $_.Exception.InnerException.Message.StartsWith('Tray startup displayed an owned window of class ')) { throw }
                $detected = $true
            }
            if (-not $detected) { throw 'Window probe accepted the open application window.' }
            $counters = [Quad9ResourceJob]::Read($JobName)
            if ($counters.TotalProcesses -le 1) { throw 'Job probe missed the open WebView processes.' }
            [Console]::Out.WriteLine('{"application_window_detected":true,"child_processes_detected":true}')
            [Console]::ReadLine() | Out-Null
        }
    } finally {
        if ($env:QUAD9_TRAY_DIAGNOSTICS) {
            try {
                $counters = [Quad9ResourceJob]::Read($JobName)
                $processes = @(foreach ($memberId in $counters.ProcessIds) {
                    Get-CimInstance Win32_Process -Filter "ProcessId=$memberId" | Select-Object ProcessId, ParentProcessId, Name, CommandLine, SessionId
                })
                $native.Refresh()
                $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
                $listeners = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
                    Where-Object { $_.OwningProcess -in $counters.ProcessIds } | Select-Object LocalAddress, LocalPort, OwningProcess)
                $policies = @{}
                foreach ($hive in @([Microsoft.Win32.Registry]::LocalMachine, [Microsoft.Win32.Registry]::CurrentUser)) {
                    foreach ($path in @('Software\Policies\Microsoft\Edge', 'Software\Policies\Microsoft\Edge\WebView2')) {
                        $key = $hive.OpenSubKey($path)
                        if ($key) {
                            try {
                                foreach ($value in @('RemoteDebuggingAllowed', 'DeveloperToolsAvailability')) {
                                    $policies["$($hive.Name)\$path\$value"] = $key.GetValue($value)
                                }
                            } finally { $key.Dispose() }
                        }
                    }
                }
                @{ processes = $processes; listeners = $listeners; debugging_policies = $policies; main_window = $native.MainWindowTitle; elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) } |
                    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $env:QUAD9_TRAY_DIAGNOSTICS -Encoding UTF8
            } catch { Write-Warning "Could not collect owned-process diagnostics: $_" }
        }
        $native.Dispose()
    }
} finally { $owner.Dispose() }
