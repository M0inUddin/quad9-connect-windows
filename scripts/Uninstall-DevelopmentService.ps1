#requires -RunAsAdministrator
param(
    [switch] $FinishPendingRemoval,
    [ValidatePattern('^[a-fA-F0-9]{64}$')][string] $ExpectedBinarySha256,
    [ValidateRange(0, 300)][int] $RemovalTimeoutSeconds = 30
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# A child Windows PowerShell process can inherit PowerShell 7's module path.
# Use this edition's built-in modules for elevated recovery commands.
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
$binary = Join-Path $installRoot 'quad9-service.exe'
$lifecycle = Import-Module (Join-Path $PSScriptRoot '..\installer\windows\InstallerLifecycle.psm1') -PassThru -Force
if ($FinishPendingRemoval) {
    if (-not $ExpectedBinarySha256) { throw 'Supply the exact SHA-256 printed when removal was deferred.' }
    & $lifecycle {
        param($Root, $Binary, $Timeout)
        Wait-Quad9ServiceRemoval $Timeout
        Assert-ProtectedDirectory $Root
        Assert-RegularPath $Binary
        Assert-TrustedAcl $Binary
    } $installRoot $binary $RemovalTimeoutSeconds
    if ((Get-FileHash -LiteralPath $binary).Hash -ine $ExpectedBinarySha256) { throw 'The retained recovery binary changed. Preserve it for administrator inspection.' }
    & $binary maintenance restore
    if ($LASTEXITCODE -ne 0) { throw 'Offline restoration did not complete. Recovery files have been retained.' }
    & $lifecycle { Remove-Quad9OperationalLog }
    Remove-Item -LiteralPath $binary
    Write-Output 'Deferred removal completed after offline DNS restoration. Recovery journals remain available.'
    return
}
$service = Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue
if (-not $service) { throw 'The Quad9 development service is not installed. Use the recorded hash with -FinishPendingRemoval to resume deferred cleanup.' }
try {
    if ($service.Status -ne 'Running') { Start-Service -Name Quad9Connect }
    $service.WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
} finally { $service.Dispose() }

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
$service = Get-Service -Name Quad9Connect
try { $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30)) } finally { $service.Dispose() }
$hash = (Get-FileHash -LiteralPath $binary).Hash
try { & $lifecycle { param($Timeout) Remove-ServiceRegistration $Timeout } $RemovalTimeoutSeconds }
catch { throw ("Service deletion did not finish; files were retained. After closing service management tools, run this script with -FinishPendingRemoval -ExpectedBinarySha256 $hash. " + $_.Exception.Message) }
& $lifecycle { Remove-Quad9OperationalLog }
Remove-Item -LiteralPath $binary
Write-Output 'The service was removed after successful DNS restoration. Protected recovery records and operator membership are retained for inspection.'
