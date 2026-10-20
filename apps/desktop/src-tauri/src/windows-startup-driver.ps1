# SPDX-License-Identifier: AGPL-3.0-or-later
# The native host supplies the compiled-in module and its own executable path.
# Frontend input is limited to a Rust enum; no command, path, or SID is accepted.
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $OutputEncoding
[Console]::InputEncoding = $OutputEncoding
try {
    $request = [Console]::In.ReadToEnd() | ConvertFrom-Json
    $module = New-Module -ScriptBlock ([ScriptBlock]::Create($request.module))
    $value = & $module {
        param($Executable, $Mode)
        if ($null -eq $Mode) { Invoke-Quad9StartupPreference $Executable }
        else { Invoke-Quad9StartupPreference $Executable -Mode $Mode -Write }
    } $request.executable $request.mode
    @{ ok = $true; value = $value; error = $null } | ConvertTo-Json -Depth 5 -Compress
} catch {
    @{ ok = $false; value = $null; error = $_.Exception.Message } | ConvertTo-Json -Compress
}
