#requires -RunAsAdministrator
param([Parameter(Mandatory)][string] $BinaryPath)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Service mutation validation requires a disposable hosted VM.' }
. (Join-Path $PSScriptRoot 'NativeDnsApi.ps1')
$baseline = Get-Content -LiteralPath (Join-Path $env:RUNNER_TEMP 'quad9.native-baseline.json') -Raw | ConvertFrom-Json
$suffix = [guid]::NewGuid().ToString('N').Substring(0, 6)
$operator = 'q9op' + $suffix
$outsider = 'q9out' + $suffix
$credentials = @{}
foreach ($name in @($operator, $outsider)) {
    $password = ConvertTo-SecureString ([guid]::NewGuid().ToString('N') + 'aA7!') -AsPlainText -Force
    New-LocalUser -Name $name -Password $password -AccountNeverExpires | Out-Null
    Add-LocalGroupMember -SID ([Security.Principal.SecurityIdentifier]'S-1-5-32-545') -Member $name
    $credentials[$name] = [PSCredential]::new("$env:COMPUTERNAME\$name", $password)
}
$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
New-Item -ItemType Directory -Path $installRoot | Out-Null
$rejectedExistingDirectory = $false
try {
    & (Join-Path $PSScriptRoot '..\Install-DevelopmentService.ps1') -BinaryPath $BinaryPath -OperatorAccount "$env:COMPUTERNAME\$operator" -ValidationVm
} catch {
    if ($_.Exception.Message -like '*install directory already exists*') { $rejectedExistingDirectory = $true } else { throw }
}
if (-not $rejectedExistingDirectory -or (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue)) { throw 'Installer reused an existing service directory.' }
$expectedRoot = [IO.Path]::GetFullPath($installRoot)
if ((Resolve-Path -LiteralPath $installRoot).Path -ne $expectedRoot) { throw 'Unexpected validation directory.' }
[IO.Directory]::Delete($expectedRoot, $false) # Only the empty fixture created above; never recursive.
& (Join-Path $PSScriptRoot '..\Install-DevelopmentService.ps1') -BinaryPath $BinaryPath -OperatorAccount "$env:COMPUTERNAME\$operator" -ValidationVm
$binary = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect\quad9-service.exe'
$shared = Join-Path ([Environment]::GetFolderPath('CommonDocuments')) ('Quad9Validation-' + $suffix)
New-Item -ItemType Directory -Path $shared | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Invoke-TestClient.ps1') -Destination (Join-Path $shared 'client.ps1')
$sequence = 0

function Invoke-AdminRequest {
    param([object] $Command, [string] $Id = ('admin-' + [guid]::NewGuid().ToString('N')))
    $json = @{ version = 1; id = $Id; command = $Command } | ConvertTo-Json -Depth 10 -Compress
    $output = $json | & $binary request
    if ($LASTEXITCODE -ne 0) { throw 'Administrator service transport failed.' }
    return $output | ConvertFrom-Json
}
function Invoke-UserClient {
    param([string] $User, [object] $Request, [string] $Mode = 'request', [int] $ExpectedExit = 0)
    $script:sequence++
    $inputPath = Join-Path $shared "request-$script:sequence.json"
    $outputPath = Join-Path $shared "response-$script:sequence.json"
    $errorPath = Join-Path $shared "response-$script:sequence.err"
    $Request | ConvertTo-Json -Depth 10 -Compress | Set-Content -LiteralPath $inputPath -Encoding utf8
    $executable = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f (Join-Path $shared 'client.ps1')), '-RequestFile', ('"{0}"' -f $inputPath), '-Mode', $Mode)
    $process = Start-Process -FilePath $executable -ArgumentList $arguments -Credential $credentials[$User] -WindowStyle Hidden -PassThru -RedirectStandardOutput $outputPath -RedirectStandardError $errorPath
    if (-not $process.WaitForExit(45000)) { Stop-Process -Id $process.Id -Force; throw 'User client exceeded its deadline.' }
    if ($process.ExitCode -ne $ExpectedExit) {
        $detail = Get-Content -LiteralPath $errorPath -Raw
        throw "User client exit $($process.ExitCode), expected ${ExpectedExit}: $detail"
    }
    $output = Get-Content -LiteralPath $outputPath -Raw
    if ($Mode -eq 'request' -and $ExpectedExit -eq 0) { return $output | ConvertFrom-Json }
    return $output
}

$status = Invoke-AdminRequest @{ type = 'get_status' }
if ($status.result.type -ne 'status' -or $status.result.revision -ne 0) { throw 'Fresh service state is not off at revision zero.' }
$read = @{ version = 1; id = 'operator-status'; command = @{ type = 'get_status' } }
$operatorStatus = Invoke-UserClient -User $operator -Request $read
if ($operatorStatus.result.type -ne 'status') { throw 'Authorized standard user could not inspect service state.' }
Invoke-UserClient -User $outsider -Request $read -ExpectedExit 1 | Out-Null
Invoke-UserClient -User $operator -Request $read -Mode deny-storage | Out-Null
Invoke-UserClient -User $operator -Request $read -Mode deny-log-read | Out-Null
Invoke-UserClient -User $operator -Request $read -Mode malformed | Out-Null
Invoke-UserClient -User $operator -Request $read -Mode oversized | Out-Null
$status = Invoke-AdminRequest @{ type = 'get_status' }
if ($status.result.revision -ne 0) { throw 'Rejected or read-only requests changed the mutation revision.' }

$enableRequest = @{ version = 1; id = 'operator-enable'; command = @{ type = 'enable_quad9'; expected_revision = 0; targets = @(@{ adapter_id = $baseline.adapter_id; family = 'ipv4' }) } }
$enabled = Invoke-UserClient -User $operator -Request $enableRequest
if ($enabled.result.type -ne 'enabled') { throw "Native service enable failed: $($enabled | ConvertTo-Json -Depth 8 -Compress)" }
$native = [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $false)
if ($native.NameServer -ne '9.9.9.9,149.112.112.112' -or $native.Properties.Count -ne 2) { throw 'Service did not write the expected strict Quad9 configuration.' }
$restored = Invoke-AdminRequest @{ type = 'restore_previous_settings'; expected_revision = 1 } -Id 'restore-once'
if ($restored.result.type -ne 'restored' -or -not $restored.result.complete) { throw 'Service could not restore its original baseline.' }
$retry = Invoke-UserClient -User $operator -Request $enableRequest
if ($retry.result.type -ne 'enabled' -or $retry.result.revision -ne 1) { throw 'Retry did not return its original receipt.' }
Restart-Service -Name Quad9Connect
(Get-Service -Name Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
$retry = Invoke-UserClient -User $operator -Request $enableRequest
$status = Invoke-AdminRequest @{ type = 'get_status' }
if ($retry.result.type -ne 'enabled' -or $status.result.revision -ne 2 -or $status.result.journal.phase -ne 'restored') { throw 'Retry after restart changed the restored state.' }
$data = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Quad9Connect'
$logPath = Join-Path $data 'startup-events.v1.json'
$log = Get-Content -LiteralPath $logPath -Raw | ConvertFrom-Json
if ($log.version -ne 1 -or 'controller_started' -notin $log.records.event -or 'service_stopped' -notin $log.records.event) { throw 'Real service startup and stop events were not recorded.' }
if ((Get-Item -LiteralPath $logPath).Length -gt 512KB) { throw 'Operational log exceeded its byte limit.' }
foreach ($record in $log.records) {
    if (@($record.PSObject.Properties.Name | Where-Object { $_ -notin @('unix_seconds', 'event', 'windows_error') }).Count) { throw 'Operational logs contain unexpected fields.' }
}
# Log corruption cannot make a previously restored controller unavailable.
Stop-Service Quad9Connect
$savedLog = [IO.File]::ReadAllBytes($logPath)
[IO.File]::WriteAllText($logPath, 'invalid operational log fixture')
try {
    Start-Service Quad9Connect
    $status = Invoke-AdminRequest @{ type = 'get_status' }
    if ($status.result.revision -ne 2 -or $status.result.journal.phase -ne 'restored') { throw 'An unusable operational log changed the controller state.' }
    if ([IO.File]::ReadAllText($logPath) -ne 'invalid operational log fixture') { throw 'Unknown operational log data was overwritten.' }
} finally {
    Stop-Service Quad9Connect -ErrorAction SilentlyContinue
    [IO.File]::WriteAllBytes($logPath, $savedLog)
}
# A controller-start failure must remain identifiable without recording its data.
$requestPath = Join-Path $data 'requests.journal.json'
$savedRequests = [IO.File]::ReadAllBytes($requestPath)
[IO.File]::WriteAllText($requestPath, 'invalid request-state fixture')
try {
    $refused = $false
    try { Start-Service Quad9Connect -ErrorAction Stop } catch { $refused = $true }
    if (-not $refused) { throw 'Invalid request state unexpectedly started the service.' }
    (Get-Service Quad9Connect).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    $log = Get-Content -LiteralPath $logPath -Raw | ConvertFrom-Json
    if ('controller_start_failed' -notin $log.records.event) { throw 'Controller startup failure was not recorded.' }
} finally {
    if ((Get-Service Quad9Connect).Status -ne 'Stopped') {
        Stop-Service Quad9Connect
        (Get-Service Quad9Connect).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    }
    [IO.File]::WriteAllBytes($requestPath, $savedRequests)
}
Start-Service Quad9Connect
foreach ($family in $baseline.families) {
    $actual = [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $family.ipv6)
    if (([string]$actual.NameServer) -ne ([string]$family.snapshot.NameServer)) { throw 'Service validation left changed DNS settings.' }
}
$removal = Join-Path $PSScriptRoot '..\Uninstall-DevelopmentService.ps1'
$expectedHash = (Get-FileHash -LiteralPath $binary).Hash
# Drop unreferenced controllers from the earlier test checks before deliberately
# retaining one real SCM handle across deletion.
[GC]::Collect()
[GC]::WaitForPendingFinalizers()
$heldService = Get-Service Quad9Connect
$null = $heldService.ServiceHandle
try {
    $deferred = $false
    try { & $removal -RemovalTimeoutSeconds 0 } catch { $deferred = $_.Exception.Message -like '*deletion did not finish*' }
    if (-not $deferred -or -not (Test-Path -LiteralPath $binary) -or -not (Test-Path -LiteralPath $logPath)) { throw 'Pending SCM deletion must retain recovery binaries and logs.' }
} finally { $heldService.Dispose() }
$wrongHashRefused = $false
try { & $removal -FinishPendingRemoval -ExpectedBinarySha256 ('0' * 64) } catch { $wrongHashRefused = $_.Exception.Message -like '*binary changed*' }
if (-not $wrongHashRefused -or -not (Test-Path -LiteralPath $binary)) { throw 'Deferred cleanup accepted an unknown recovery binary.' }
& $removal -FinishPendingRemoval -ExpectedBinarySha256 $expectedHash
if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) { throw 'Development service was not removed.' }
if (Test-Path -LiteralPath $logPath) { throw 'Safe removal retained operational logs.' }
Write-Output 'Service validation passed: standard operator control, unauthorized denial, private storage, bounded malformed frames, native enable/restore, durable retry after restart, and uninstall restoration.'
