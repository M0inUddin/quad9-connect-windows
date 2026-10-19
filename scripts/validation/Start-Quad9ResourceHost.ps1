# SPDX-License-Identifier: AGPL-3.0-or-later
param([Parameter(Mandatory)][string] $Executable, [Parameter(Mandatory)][string] $JobName)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ResourceJob.cs')
$owner = [Quad9ResourceJob]::new([IO.Path]::GetFullPath($Executable), '', $JobName)
try {
    [Console]::Out.WriteLine((@{ pid = $owner.ProcessId } | ConvertTo-Json -Compress))
    # The Node driver owns this pipe. EOF also cleans up after a failed driver.
    [Console]::ReadLine() | Out-Null
} finally { $owner.Dispose() }
