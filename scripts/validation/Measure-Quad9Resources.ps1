# Read-only counters. Run against release binaries after installation with DNS off.
param(
    [int] $HostProcessId = 0,
    [switch] $DesktopOnly,
    [switch] $RequireForeground,
    [string] $CpuJobName = '',
    [ValidateSet('service', 'window', 'tray')][string] $Mode = 'service',
    [ValidateRange(1, 3600)][int] $DurationSeconds = 300,
    [ValidateRange(0, 300)][int] $SettleSeconds = 30,
    [Parameter(Mandatory)][string] $OutputPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($CpuJobName) {
    if ($HostProcessId -le 0 -or $Mode -eq 'service') { throw 'Job accounting requires a desktop host.' }
    if (-not ('Quad9ResourceJob' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'ResourceJob.cs') }
}
if ($RequireForeground -and ($Mode -ne 'window' -or $HostProcessId -le 0)) { throw 'Foreground validation requires a desktop window measurement.' }
if (-not ('Quad9ResourceForeground' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Quad9ResourceForeground {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    public static uint ProcessId() {
        uint processId;
        GetWindowThreadProcessId(GetForegroundWindow(), out processId);
        return processId;
    }
}
'@
}
$service = Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'"
if ($DesktopOnly -and ($Mode -eq 'service' -or $HostProcessId -le 0)) { throw 'Desktop-only sampling requires window/tray mode and the desktop PID.' }
if (-not $DesktopOnly -and (-not $service -or $service.State -ne 'Running')) { throw 'The installed Quad9 service must be running.' }
if ($Mode -ne 'service' -and $HostProcessId -le 0) { throw 'Window/tray measurements require the desktop PID.' }
if ($Mode -eq 'service' -and $HostProcessId -ne 0) { throw 'Service-only sampling cannot include a desktop PID.' }
$roots = @()
$serviceId = 0
if (-not $DesktopOnly) { $serviceId = [int]$service.ProcessId; $roots += $serviceId }
if ($HostProcessId -gt 0) { $roots += $HostProcessId }
$identities = @{}
foreach ($rootId in $roots) {
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$rootId"
    $expectedName = if ($rootId -eq $serviceId) { 'quad9-service.exe' } else { 'quad9-desktop.exe' }
    if (-not $p -or $p.Name -ine $expectedName) { throw 'Unexpected measurement root.' }
    if ($HostProcessId -gt 0 -and $HostProcessId -eq $serviceId) { throw 'The desktop PID cannot be the service PID.' }
    $identities[$rootId] = $p.CreationDate
}
Write-Output "Settling $Mode for $SettleSeconds seconds; then sampling for $DurationSeconds seconds."
Start-Sleep -Seconds $SettleSeconds
$samples = [Collections.Generic.List[object]]::new()
$cpuStart = @{}
$cpuLast = @{}
$previousKeys = @()
$previousServiceKeys = @()
$churn = $false
$serviceChurn = $false
$jobCpuStart = $null
$jobCpuLast = $null
$jobTotalStart = $null
$jobTotalLast = $null
$jobExitedStart = $null
$clock = [Diagnostics.Stopwatch]::StartNew()
do {
    $all = @(Get-CimInstance Win32_Process)
    foreach ($rootId in $roots) {
        $rootProcess = @($all | Where-Object ProcessId -eq $rootId)
        if ($rootProcess.Count -ne 1 -or $rootProcess[0].CreationDate -ne $identities[$rootId]) { throw 'A measurement root exited or its PID was reused.' }
    }
    $selected = @($all | Where-Object { $_.ProcessId -in $roots })
    $serviceProcessIds = @($serviceId | Where-Object { $_ -gt 0 })
    $jobCounters = $null
    if ($CpuJobName) {
        $jobCounters = [Quad9ResourceJob]::Read($CpuJobName)
        $counterSeconds = $clock.Elapsed.TotalSeconds
        if ($HostProcessId -notin $jobCounters.ProcessIds) { throw 'The desktop host is not in the supplied accounting job.' }
        $selected = @($all | Where-Object { $_.ProcessId -in $jobCounters.ProcessIds -or ($serviceId -gt 0 -and $_.ProcessId -eq $serviceId) })
        if ($null -eq $jobCpuStart) { $jobCpuStart = $jobCounters.CpuTicks; $jobTotalStart = $jobCounters.TotalProcesses }
        $jobCpuLast = $jobCounters.CpuTicks
        $jobTotalLast = $jobCounters.TotalProcesses
        $exited = $jobCounters.TotalProcesses - $jobCounters.ActiveProcesses
        if ($null -eq $jobExitedStart) { $jobExitedStart = $exited }
        if ($exited -gt $jobExitedStart) { $churn = $true }
    }
    do {
        $selectedIds = @($selected | ForEach-Object ProcessId)
        $children = @($all | Where-Object {
            $candidate = $_
            if ($candidate.ParentProcessId -notin $selectedIds -or $candidate.ProcessId -in $selectedIds) { return $false }
            $parent = @($selected | Where-Object ProcessId -eq $candidate.ParentProcessId)
            $parent.Count -eq 1 -and $candidate.CreationDate -ge $parent[0].CreationDate
        })
        if ($CpuJobName) {
            foreach ($descendant in @($children | Where-Object { $_.ParentProcessId -notin $serviceProcessIds })) {
                try { $member = [Quad9ResourceJob]::Contains($CpuJobName, $descendant.ProcessId) }
                catch { throw ('Cannot verify job membership for ' + $descendant.Name + ' PID ' + $descendant.ProcessId + ': ' + $_.Exception.Message) }
                if ($member -eq $false) { throw ('A desktop descendant escaped the accounting job: ' + $descendant.Name) }
            }
        }
        $serviceProcessIds += @($children | Where-Object { $_.ParentProcessId -in $serviceProcessIds } | ForEach-Object ProcessId)
        $selected += $children
    } while ($children.Count)
    $keys = @()
    $serviceKeys = @()
    $memory = [uint64]0
    $serviceMemory = [uint64]0
    foreach ($p in $selected) {
        $key = '{0}:{1:o}' -f $p.ProcessId, $p.CreationDate
        $keys += $key
        if ($null -eq $p.KernelModeTime -or $null -eq $p.UserModeTime -or $null -eq $p.WorkingSetSize) { throw 'Process counters are unavailable. Run the sampler elevated.' }
        $ticks = [uint64]$p.KernelModeTime + [uint64]$p.UserModeTime
        if ($p.ProcessId -in $serviceProcessIds) { $serviceKeys += $key }
        if (-not $CpuJobName -or $p.ProcessId -in $serviceProcessIds) {
            if (-not $cpuStart.ContainsKey($key)) { $cpuStart[$key] = if ($samples.Count -eq 0) { $ticks } else { [uint64]0 } }
            $cpuLast[$key] = $ticks
        }
        $memory += [uint64]$p.WorkingSetSize
        if ($p.ProcessId -eq $serviceId) { $serviceMemory = [uint64]$p.WorkingSetSize }
    }
    if ($samples.Count -gt 0 -and @($previousKeys | Where-Object { $_ -notin $keys }).Count) { $churn = $true }
    if ($samples.Count -gt 0 -and @($previousServiceKeys | Where-Object { $_ -notin $serviceKeys }).Count) { $serviceChurn = $true }
    $previousKeys = $keys
    $previousServiceKeys = $serviceKeys
    if (-not $CpuJobName) { $counterSeconds = $clock.Elapsed.TotalSeconds }
    $samples.Add([pscustomobject][ordered]@{ seconds = $counterSeconds; working_set_bytes = $memory; service_working_set_bytes = $serviceMemory; processes = $selected.Count; webviews = @($selected | Where-Object Name -eq 'msedgewebview2.exe').Count; host_foreground = ($HostProcessId -gt 0 -and [Quad9ResourceForeground]::ProcessId() -eq $HostProcessId) })
    $interval = $samples[-1].seconds - $samples[0].seconds
    if ($interval -lt $DurationSeconds) { Start-Sleep -Milliseconds 2000 }
} while ($interval -lt $DurationSeconds)
$elapsed = $samples[-1].seconds - $samples[0].seconds
$cpuTicks = [uint64]0
foreach ($key in $cpuLast.Keys) { $cpuTicks += $cpuLast[$key] - $cpuStart[$key] }
if ($CpuJobName) { $cpuTicks += $jobCpuLast - $jobCpuStart }
$cpuPercent = ($cpuTicks / 10000000.0) / $elapsed * 100
$maxMiB = ($samples | Measure-Object working_set_bytes -Maximum).Maximum / 1MB
$foregroundThroughout = @($samples | Where-Object { -not $_.host_foreground }).Count -eq 0
$complete = -not $DesktopOnly -and $elapsed -ge 300 -and $SettleSeconds -ge 30 -and (-not $RequireForeground -or $foregroundThroughout)
$cpuComplete = -not $serviceChurn -and ([bool]$CpuJobName -or -not $churn)
$desktopInterval = $DesktopOnly -and $elapsed -ge 300 -and $SettleSeconds -ge 30 -and (-not $RequireForeground -or $foregroundThroughout)
$limit = switch ($Mode) { 'service' { 40 }; 'window' { 250 }; 'tray' { 100 } }
$report = [ordered]@{
    schema = 2; mode = $Mode; measured_seconds = $elapsed; settle_seconds = $SettleSeconds
    complete_process_set = (-not $DesktopOnly)
    five_minute_interval = ($elapsed -ge 300)
    foreground_required = [bool]$RequireForeground; host_foreground_throughout = $foregroundThroughout
    cpu_percent_one_logical_core = $cpuPercent; process_exits_observed = $churn
    cpu_accounting = $(if ($CpuJobName) { 'Windows job totals include exited desktop descendants; separate service counters when supplied.' } else { 'Sampled process counters; exits may omit final CPU time.' })
    cpu_measurement_complete = $cpuComplete
    desktop_job_processes_at_start = $jobTotalStart; desktop_job_processes_at_end = $jobTotalLast
    desktop_only_cpu_target_pass = $(if ($desktopInterval -and $cpuComplete) { $cpuPercent -lt 0.5 } else { $null })
    average_working_set_mib = ($samples | Measure-Object working_set_bytes -Average).Average / 1MB
    maximum_working_set_mib = $maxMiB
    maximum_service_working_set_mib = ($samples | Measure-Object service_working_set_bytes -Maximum).Maximum / 1MB
    cpu_target_pass = $(if ($complete -and $cpuComplete) { $cpuPercent -lt 0.5 } else { $null })
    memory_target_pass = $(if ($complete) { $maxMiB -le $limit } else { $null })
    service_memory_target_pass = $(if ($complete) { ($samples | Measure-Object service_working_set_bytes -Maximum).Maximum -le 40MB } else { $null })
    window_or_tray_state = 'Caller establishes the native window state; counters do not infer visibility.'
    attribution = 'Installed service PID, desktop PID when supplied, and recursive descendants; includes shared resident pages in each process working set.'
    samples = $samples.ToArray()
}
$fullPath = [IO.Path]::GetFullPath($OutputPath)
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($fullPath)) | Out-Null
[IO.File]::WriteAllText($fullPath, ($report | ConvertTo-Json -Depth 8))
Write-Output ("{0}: peak {1:N2} MiB; CPU {2:N4}% of one logical core; {3:N1} seconds. Report: {4}" -f $Mode, $maxMiB, $cpuPercent, $elapsed, $fullPath)
