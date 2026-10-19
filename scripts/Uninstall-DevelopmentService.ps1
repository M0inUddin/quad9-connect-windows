#requires -RunAsAdministrator
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
$binary = Join-Path $installRoot 'quad9-service.exe'
if (-not (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue)) { throw 'The Quad9 development service is not installed.' }
if ((Get-Service -Name Quad9Connect).Status -ne 'Running') { Start-Service -Name Quad9Connect }
(Get-Service -Name Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))

function Invoke-ServiceRequest {
    param([object] $Command)
    $request = @{ version = 1; id = 'uninstall-' + [guid]::NewGuid().ToString('N'); command = $Command }
    $output = ($request | ConvertTo-Json -Depth 8 -Compress) | & $binary request
    if ($LASTEXITCODE -ne 0) { throw 'Cannot contact the service. Preserve the service and recovery data for repair.' }
    return $output | ConvertFrom-Json
}
$status = Invoke-ServiceRequest @{ type = 'get_status' }
if ($status.result.type -ne 'status') { throw 'Recovery status is unavailable; removal has stopped.' }
$restored = Invoke-ServiceRequest @{ type = 'restore_previous_settings'; expected_revision = $status.result.revision }
if ($restored.result.type -ne 'restored' -or -not $restored.result.complete) {
    throw 'DNS restoration is incomplete. The service, binary, and recovery records have been retained.'
}
Stop-Service -Name Quad9Connect
(Get-Service -Name Quad9Connect).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
& sc.exe delete Quad9Connect | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Service removal failed. Preserve the installed recovery binary.' }
Remove-Item -LiteralPath $binary
Write-Output 'The service was removed after successful DNS restoration. Protected recovery records and operator membership are retained for inspection.'
