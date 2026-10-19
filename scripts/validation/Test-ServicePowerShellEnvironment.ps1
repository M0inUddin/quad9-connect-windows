# SPDX-License-Identifier: AGPL-3.0-or-later
# Resolves real built-in commands after inheriting an unusable module path.
# No administrator rights, service actions, or DNS changes.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$previousModulePath = $env:PSModulePath
$samplePath = [IO.Path]::GetTempFileName()
try {
    [IO.File]::WriteAllText($samplePath, 'abc', [Text.UTF8Encoding]::new($false))
    Remove-Module Microsoft.PowerShell.Utility, Microsoft.PowerShell.LocalAccounts -ErrorAction SilentlyContinue
    $env:PSModulePath = Join-Path ([IO.Path]::GetTempPath()) 'quad9-invalid-module-path'
    . (Join-Path $PSScriptRoot '..\Use-ServiceSystemModules.ps1')
    $hash = Get-FileHash -LiteralPath $samplePath -Algorithm SHA256
    if ($hash.Hash -ine 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') { throw 'Built-in file hashing did not load correctly.' }
    foreach ($command in @(Get-Command Get-FileHash, Get-LocalGroup)) {
        $trusted = $false
        foreach ($root in @((Join-Path $PSHOME 'Modules'), (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules'))) {
            if ($command.Module.Path.StartsWith(($root + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { $trusted = $true }
        }
        if (-not $trusted) { throw 'A recovery command resolved outside the built-in module directories.' }
    }
    Write-Output "Built-in recovery modules passed under PowerShell $($PSVersionTable.PSVersion): hashing and LocalAccounts, with an unusable inherited module path."
} finally {
    $env:PSModulePath = $previousModulePath
    [IO.File]::Delete($samplePath)
}
