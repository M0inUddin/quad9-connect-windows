# SPDX-License-Identifier: AGPL-3.0-or-later
param([Parameter(Mandatory)][string] $Desktop, [ValidateSet('read-off', 'read-tray', 'off', 'window', 'tray')][string] $Mode)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$module = Import-Module (Join-Path (Split-Path $Desktop -Parent) 'InstallerLifecycle.psm1') -Force -PassThru
& $module {
    param($Desktop, $Mode)
    $read = $Mode.StartsWith('read-')
    $expected = if ($read) { $Mode.Substring(5) } else { $Mode }
    $result = if ($read) { Invoke-Quad9StartupPreference $Desktop } else { Invoke-Quad9StartupPreference $Desktop -Mode $Mode -Write }
    if (-not $result.available -or $result.mode -cne $expected) { throw "Unexpected startup preference: $($result | ConvertTo-Json -Compress)" }
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $task = Find-StartupTask (Open-StartupScheduler).Folder ($script:StartupTaskPrefix + $sid)
    if ($expected -eq 'off') {
        if ($task) { throw 'Off retained a startup task.' }
    } elseif (-not $task -or $task.Definition.Actions.Item(1).Path -ine $Desktop -or
        $task.Definition.Principal.RunLevel -ne 0 -or $task.Definition.Principal.LogonType -ne 3 -or
        (Get-StartupSid $task.Definition.Triggers.Item(1).UserId) -cne $sid) {
        throw 'Startup task did not use the current version and current user without elevation.'
    }
} $Desktop $Mode
Write-Output "Standard-user Windows startup check passed: $Mode"
