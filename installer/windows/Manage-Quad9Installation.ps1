#requires -RunAsAdministrator
param(
    [Parameter(Mandatory)][ValidateSet('Install', 'Uninstall', 'Repair')][string] $Operation,
    [string] $Payload,
    [string] $OperatorSid
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
try {
    Import-Module (Join-Path $PSScriptRoot 'InstallerLifecycle.psm1') -Force
    Invoke-Quad9Lifecycle -Operation $Operation -Payload $Payload -OperatorSid $OperatorSid
    exit 0
} catch {
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'lifecycle-error.txt'), ($_ | Out-String) + $_.ScriptStackTrace)
    Write-Error -ErrorAction Continue $_
    exit 1
}
