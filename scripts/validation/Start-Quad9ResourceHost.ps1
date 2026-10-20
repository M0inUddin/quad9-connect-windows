# SPDX-License-Identifier: AGPL-3.0-or-later
param([Parameter(Mandatory)][string] $Executable, [Parameter(Mandatory)][string] $JobName)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ResourceJob.cs')
$owner = [Quad9ResourceJob]::new([IO.Path]::GetFullPath($Executable), '', $JobName)
try {
    $native = [Diagnostics.Process]::GetProcessById($owner.ProcessId)
    try { $created = ([DateTimeOffset]$native.StartTime).ToUnixTimeMilliseconds() }
    finally { $native.Dispose() }
    [Console]::Out.WriteLine((@{ pid = $owner.ProcessId; started_unix_ms = $created } | ConvertTo-Json -Compress))
    # The Node driver owns this pipe. EOF also cleans up after a failed driver.
    [Console]::ReadLine() | Out-Null
} finally { $owner.Dispose() }
