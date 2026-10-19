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

function Wait-ValidationService([string] $State) {
    $controller = Get-Service -Name Quad9Connect
    try { $controller.WaitForStatus($State, [TimeSpan]::FromSeconds(30)) }
    finally { $controller.Dispose() }
}

function Invoke-RemovalValidation([string[]] $RemovalArguments) {
    # A separate process releases every cmdlet-owned SCM handle on exit. The
    # parent retains only the deliberate pending-deletion fixture handle.
    $script:sequence++
    $outputPath = Join-Path $shared "removal-$script:sequence.out"
    $errorPath = Join-Path $shared "removal-$script:sequence.err"
    $arguments = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $removal)) + $RemovalArguments
    $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput $outputPath -RedirectStandardError $errorPath
    try {
        if (-not $process.WaitForExit(120000)) {
            $process.Kill()
            $process.WaitForExit()
            throw 'The removal validation process exceeded its deadline.'
        }
        return [pscustomobject]@{ exit_code = $process.ExitCode; error_text = [IO.File]::ReadAllText($errorPath) }
    } finally { $process.Dispose() }
}

$status = Invoke-AdminRequest @{ type = 'get_status' }
if ($status.result.type -ne 'status' -or $status.result.revision -ne 0) { throw 'Fresh service state is not off at revision zero.' }
$read = @{ version = 1; id = 'operator-status'; command = @{ type = 'get_status' } }
$operatorStatus = Invoke-UserClient -User $operator -Request $read
if ($operatorStatus.result.type -ne 'status') { throw 'Authorized standard user could not inspect service state.' }
Invoke-UserClient -User $outsider -Request $read -ExpectedExit 1 | Out-Null
$diagnosticBaseline = @($baseline.families | ForEach-Object {
    [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $_.ipv6) | ConvertTo-Json -Depth 10 -Compress
})
$diagnosticRequest = @{ version = 1; id = 'operator-diagnostics'; command = @{ type = 'run_diagnostics' } }
Invoke-UserClient -User $outsider -Request $diagnosticRequest -ExpectedExit 1 | Out-Null
$diagnostic = Invoke-UserClient -User $operator -Request $diagnosticRequest
if ($diagnostic.result.type -ne 'diagnostics') { throw 'Authorized diagnostics did not return a report.' }
$report = $diagnostic.result.report
if ($report.query -cne 'proto.on.quad9.net.' -or $report.outcome -notin @('doh_observed', 'other_transport_observed', 'unknown_response', 'failed', 'timed_out')) { throw 'The system-resolver observation was lost.' }
$endpoints = @($report.direct_upstream.endpoints)
if ($endpoints.Count -ne 2 -or $endpoints[0].address -cne '9.9.9.9' -or $endpoints[1].address -cne '2620:fe::fe') { throw 'Diagnostics did not report both fixed direct endpoints.' }
foreach ($endpoint in $endpoints) {
    if ($endpoint.outcome -notin @('https_response', 'failed', 'timed_out', 'busy')) { throw 'Unknown direct endpoint outcome.' }
    if ($endpoint.outcome -eq 'https_response') {
        if ($null -eq $endpoint.http_status -or $endpoint.http_status -lt 200 -or $endpoint.http_status -gt 599) { throw 'A reachable endpoint lost its HTTP status.' }
    } elseif ($null -ne $endpoint.http_status) { throw 'An endpoint without a response reported an HTTP status.' }
}
$afterDiagnostic = @($baseline.families | ForEach-Object {
    [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $_.ipv6) | ConvertTo-Json -Depth 10 -Compress
})
if (($diagnosticBaseline -join "`n") -cne ($afterDiagnostic -join "`n")) { throw 'Read-only diagnostics changed native DNS settings.' }
$status = Invoke-AdminRequest @{ type = 'get_status' }
if ($status.result.revision -ne 0 -or $null -ne $status.result.journal) { throw 'Diagnostics changed DNS management or mutation state.' }
Write-Output 'Real LocalSystem diagnostics passed: authorized standard user, unauthorized denial, independent result fields, and unchanged native DNS baseline.'
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
Wait-ValidationService 'Running'
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
    Wait-ValidationService 'Stopped'
    $log = Get-Content -LiteralPath $logPath -Raw | ConvertFrom-Json
    if ('controller_start_failed' -notin $log.records.event) { throw 'Controller startup failure was not recorded.' }
} finally {
    $controller = Get-Service -Name Quad9Connect
    try {
        if ($controller.Status -ne 'Stopped') {
            Stop-Service Quad9Connect
            $controller.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
        }
    } finally { $controller.Dispose() }
    [IO.File]::WriteAllBytes($requestPath, $savedRequests)
}
Start-Service Quad9Connect
foreach ($family in $baseline.families) {
    $actual = [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $family.ipv6)
    if (([string]$actual.NameServer) -ne ([string]$family.snapshot.NameServer)) { throw 'Service validation left changed DNS settings.' }
}
$removal = Join-Path $PSScriptRoot '..\Uninstall-DevelopmentService.ps1'
$expectedHash = (Get-FileHash -LiteralPath $binary).Hash
$heldService = Get-Service Quad9Connect
# ServiceHandle returns a separately owned SafeHandle. Disposing the controller
# does not close it; retain and release that exact handle deterministically.
$heldHandle = $heldService.ServiceHandle
try {
    $attempt = Invoke-RemovalValidation @('-RemovalTimeoutSeconds', '0')
    if ($attempt.exit_code -eq 0 -or $attempt.error_text -notlike '*deletion did not finish*' -or -not (Test-Path -LiteralPath $binary) -or -not (Test-Path -LiteralPath $logPath)) { throw "Pending SCM deletion must retain recovery binaries and logs: $($attempt.error_text)" }
} finally { $heldHandle.Dispose(); $heldService.Dispose() }
$attempt = Invoke-RemovalValidation @('-FinishPendingRemoval', '-ExpectedBinarySha256', ('0' * 64))
if ($attempt.exit_code -eq 0 -or $attempt.error_text -notlike '*binary changed*' -or -not (Test-Path -LiteralPath $binary)) { throw "Deferred cleanup did not reject the wrong binary hash: $($attempt.error_text)" }
$attempt = Invoke-RemovalValidation @('-FinishPendingRemoval', '-ExpectedBinarySha256', $expectedHash)
if ($attempt.exit_code -ne 0) { throw "Deferred removal failed: $($attempt.error_text)" }
if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) { throw 'Development service was not removed.' }
if (Test-Path -LiteralPath $logPath) { throw 'Safe removal retained operational logs.' }
Write-Output 'Service validation passed: standard operator control, unauthorized denial, private storage, bounded malformed frames, native enable/restore, durable retry after restart, and uninstall restoration.'
