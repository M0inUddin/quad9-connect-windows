#requires -RunAsAdministrator
param(
    [Parameter(Mandatory)][string] $Installer,
    [Parameter(Mandatory)][string] $ServiceBinary,
    [Parameter(Mandatory)][string] $DesktopBinary,
    [Parameter(Mandatory)][string] $VmServiceBinary,
    [Parameter(Mandatory)][string] $MakeNsis
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Installer mutation tests require a disposable hosted VM.' }
. (Join-Path $PSScriptRoot 'NativeDnsApi.ps1')
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$root = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
$data = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Quad9Connect'
if ((Test-Path -LiteralPath $root) -or (Test-Path -LiteralPath $data) -or (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue)) { throw 'Installer tests need a clean VM.' }
$baselinePath = Join-Path $env:RUNNER_TEMP 'quad9.native-baseline.json'
if (-not (Test-Path -LiteralPath $baselinePath)) { throw 'Run the native contract test to save the original baseline first.' }
$baseline = Get-Content -LiteralPath $baselinePath -Raw | ConvertFrom-Json
$operator = 'q9inst' + [guid]::NewGuid().ToString('N').Substring(0,6)
$password = ConvertTo-SecureString ([guid]::NewGuid().ToString('N') + 'aA7!') -AsPlainText -Force
$user = New-LocalUser -Name $operator -Password $password -AccountNeverExpires
Add-LocalGroupMember -SID ([Security.Principal.SecurityIdentifier]'S-1-5-32-545') -Member $user
$credential = [PSCredential]::new("$env:COMPUTERNAME\$operator", $password)
$output = Join-Path $repo 'artifacts\installer-validation'
New-Item -ItemType Directory -Path $output -Force | Out-Null
$powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'

function Run-Setup([string] $Executable, [string] $Arguments, [int] $ExpectedExit = 0) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne $ExpectedExit) { throw "Setup exit $($process.ExitCode), expected $ExpectedExit. Executable: $Executable" }
}
function State { return Get-Content -LiteralPath (Join-Path $root 'installation.json') -Raw | ConvertFrom-Json }
function Current-Directory {
    $state = State
    return Join-Path $root ('versions\' + $state.current)
}
function Request([object] $Command, [string] $Id = ('test-' + [guid]::NewGuid().ToString('N'))) {
    $binary = Join-Path (Current-Directory) 'quad9-service.exe'
    $json = @{ version = 1; id = $Id; command = $Command } | ConvertTo-Json -Depth 12 -Compress
    $response = $json | & $binary request
    if ($LASTEXITCODE -ne 0) { throw 'Installed service request failed.' }
    return $response | ConvertFrom-Json
}
function Assert-Baseline {
    foreach ($family in $baseline.families) {
        $snapshot = [Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, [bool]$family.ipv6)
        if (([string]$snapshot.NameServer) -cne ([string]$family.snapshot.NameServer) -or
            ($snapshot.Properties | ConvertTo-Json -Depth 6 -Compress) -cne ($family.snapshot.Properties | ConvertTo-Json -Depth 6 -Compress)) { throw 'Original DNS baseline was not preserved.' }
    }
}
function Assert-Active {
    $response = Request @{ type = 'get_status' }
    if ($response.result.journal.phase -ne 'committed' -or -not $response.result.observed_matches) { throw 'Upgrade lost active DNS or its original recovery snapshot.' }
}
function Enable-VM {
    # Only the fixture binary permits a VM adapter. The distributable installer
    # always contains the production service and always registers `service`.
    $command = (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'").PathName
    Stop-Service -Name Quad9Connect
    (Get-Service Quad9Connect).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    $fixture = Join-Path $root 'vm-fixture'
    if (-not (Test-Path -LiteralPath $fixture)) { New-Item -ItemType Directory -Path $fixture | Out-Null }
    $fixtureBinary = Join-Path $fixture 'quad9-service.exe'
    Copy-Item -LiteralPath $VmServiceBinary -Destination $fixtureBinary -Force
    $changed = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'") -MethodName Change -Arguments @{ PathName = ('"{0}" service-vm' -f $fixtureBinary) }
    if ($changed.ReturnValue) { throw 'Could not configure VM fixture service.' }
    try {
        Start-Service Quad9Connect
        (Get-Service Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
        $status = Request @{ type = 'get_status' }
        $enabled = Request @{ type = 'enable_quad9'; expected_revision = $status.result.revision; targets = @(@{ adapter_id = $baseline.adapter_id; family = 'ipv4' }) }
        if ($enabled.result.type -ne 'enabled') { throw ($enabled | ConvertTo-Json -Depth 8 -Compress) }
    } finally {
        Stop-Service Quad9Connect
        (Get-Service Quad9Connect).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
        $changed = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'") -MethodName Change -Arguments @{ PathName = $command }
        if ($changed.ReturnValue) { throw 'Could not restore production service command.' }
        Start-Service Quad9Connect
        (Get-Service Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    }
    Assert-Active
}

try {
    Run-Setup $Installer ('/S /OPERATORSID=' + $user.SID.Value)
    $initial = State
    $status = Request @{ type = 'get_status' }
    if ($status.result.revision -ne 0 -or $null -ne $status.result.journal) { throw 'Fresh installation enabled DNS.' }
    Assert-Baseline
    # A new standard-user token sees the explicit enrollment and cannot write
    # installed binaries, recovery state, or invoke offline administrator recovery.
    $client = Join-Path $output 'client.ps1'
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Invoke-TestClient.ps1') -Destination $client
    $requestFile = Join-Path $output 'request.json'
    '{"version":1,"id":"standard-status","command":{"type":"get_status"}}' | Set-Content -LiteralPath $requestFile
    foreach ($mode in @('request', 'deny-install', 'deny-storage', 'maintenance-check')) {
        $binary = Join-Path (Current-Directory) 'quad9-service.exe'
        $args = '-NoProfile -ExecutionPolicy Bypass -File "{0}" -Mode {1} -RequestFile "{2}" -BinaryPath "{3}"' -f $client, $mode, $requestFile, $binary
        $process = Start-Process -FilePath $powershell -ArgumentList $args -Credential $credential -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $output "$mode.out") -RedirectStandardError (Join-Path $output "$mode.err")
        $expected = if ($mode -eq 'maintenance-check') { 1 } else { 0 }
        if ($process.ExitCode -ne $expected) { throw "Standard-user check failed: $mode" }
    }
    # The service rejects even authorized mutations while installation owns the gate.
    $gate = Join-Path $data 'installer-maintenance'
    [IO.File]::WriteAllText($gate, '')
    try {
        $blocked = Request @{ type = 'restore_previous_settings'; expected_revision = 0 }
        if ($blocked.result.type -ne 'error' -or $blocked.result.code -ne 'maintenance') { throw 'Installer gate allowed a DNS mutation.' }
    } finally { Remove-Item -LiteralPath $gate }
    Enable-VM
    $journalHash = (Get-FileHash -LiteralPath (Join-Path $data 'dns.journal.json')).Hash
    $upgrade = & (Join-Path $repo 'scripts\Build-WindowsInstaller.ps1') -ServiceBinary $ServiceBinary -DesktopBinary $DesktopBinary -MakeNsis $MakeNsis -OutputDirectory $output -Version '0.1.0-dev.upgrade'
    Run-Setup $upgrade '/S'
    $upgraded = State
    if ($upgraded.current -eq $initial.current -or $upgraded.enrollment.sid -ne $user.SID.Value) { throw 'Upgrade did not change payload or preserve authorization.' }
    Assert-Active
    if ((Get-FileHash -LiteralPath (Join-Path $data 'dns.journal.json')).Hash -ne $journalHash) { throw 'Upgrade rewrote the original journal.' }

    # This executable accepts preflight but cannot run as a service. Exercise
    # rollback after SCM replacement, rather than only rejecting a bad manifest.
    $badService = Join-Path $output 'bad-service.exe'
    $code = 'public class BadService { public static int Main(string[] a) { return a.Length == 2 && a[0] == "maintenance" && a[1] == "check" ? 0 : 1; } }'
    $codeFile = Join-Path $output 'bad-service.cs'
    [IO.File]::WriteAllText($codeFile, $code)
    & "$env:SystemRoot\Microsoft.NET\Framework64\v4.0.30319\csc.exe" /nologo /target:exe "/out:$badService" $codeFile
    if ($LASTEXITCODE -ne 0) { throw 'Failed to compile bad-service fixture.' }
    $bad = & (Join-Path $repo 'scripts\Build-WindowsInstaller.ps1') -ServiceBinary $badService -DesktopBinary $DesktopBinary -MakeNsis $MakeNsis -OutputDirectory $output -Version '0.1.0-dev.rejected'
    Run-Setup $bad '/S' 1
    if ((State).current -ne $upgraded.current -or (Get-Service Quad9Connect).Status -ne 'Running') { throw 'Failed upgrade did not restore the previous running service.' }
    Assert-Active

    # Simulate an interrupted replacement with durable pending metadata and a gate.
    $state = State
    $candidate = @($state.versions | Where-Object id -eq $initial.current)[0]
    @{ schema = 1; previous = $state; candidate = $candidate; enrollment = $state.enrollment } | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $root 'pending.json')
    [IO.File]::WriteAllText($gate, '')
    Run-Setup $upgrade '/S'
    if (Test-Path -LiteralPath (Join-Path $root 'pending.json')) { throw 'Interrupted upgrade was not reconciled.' }
    Assert-Active

    # An external edit blocks uninstall and must remain untouched.
    [Quad9Validation.DnsApi]::Write([guid]$baseline.adapter_id, $false, '149.112.112.112,9.9.9.9', @())
    $uninstaller = Join-Path (Current-Directory) 'Uninstall.exe'
    Run-Setup $uninstaller '/S' 1
    if ((Get-Service Quad9Connect).Status -ne 'Running' -or -not (Test-Path -LiteralPath (Join-Path $data 'dns.journal.json'))) { throw 'Blocked uninstall removed its recovery mechanism.' }
    if ([Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $false).NameServer -ne '149.112.112.112,9.9.9.9') { throw 'Uninstall overwrote an external edit.' }
    # The administrator explicitly resolves the conflict to the saved baseline.
    Restore-ValidationBaseline $baseline
    & $powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path (Current-Directory) 'Manage-Quad9Installation.ps1') -Operation Repair
    if ($LASTEXITCODE -ne 0) { throw 'Administrator repair failed.' }
    Assert-Baseline
    Enable-VM
    $uninstaller = Join-Path (Current-Directory) 'Uninstall.exe'
    Run-Setup $uninstaller '/S'
    Assert-Baseline
    if ((Get-Service Quad9Connect -ErrorAction SilentlyContinue) -or (State).installed -or (Get-LocalGroup Quad9ConnectOperators -ErrorAction SilentlyContinue)) { throw 'Safe uninstall left installed resources or operator enrollment.' }
    if (-not (Test-Path -LiteralPath (Join-Path $data 'dns.journal.json'))) { throw 'Uninstall discarded the audit and recovery journal.' }
    # Reinstall after removal must discard inventories of files already removed.
    Run-Setup $Installer ('/S /OPERATORSID=' + $user.SID.Value)
    Assert-Baseline
    Run-Setup (Join-Path (Current-Directory) 'Uninstall.exe') '/S'
    Assert-Baseline
    [ordered]@{ result = 'passed'; first_install_dns_unchanged = $true; standard_user_authorization = $true; maintenance_gate = $true; active_upgrade = $true; failed_upgrade_rollback = $true; interrupted_upgrade = $true; conflict_blocks_uninstall = $true; administrator_repair = $true; uninstall_restores_dns = $true; reinstall = $true; os_build = [Environment]::OSVersion.Version.Build } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'installer-validation.json')
    Write-Output 'Real NSIS installer lifecycle passed on this disposable VM.'
} finally {
    $service = Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') { Stop-Service Quad9Connect -ErrorAction Continue }
    Restore-ValidationBaseline $baseline
    Remove-LocalUser -Name $operator -ErrorAction SilentlyContinue
}
