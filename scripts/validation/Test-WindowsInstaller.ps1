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
    if ($process.ExitCode -ne $ExpectedExit) {
        foreach ($stage in Get-ChildItem -LiteralPath ([Environment]::GetFolderPath('ProgramFiles')) -Filter 'Quad9Connect-Setup-*' -Directory) {
            $errorFile = Join-Path $stage.FullName 'lifecycle-error.txt'
            if (Test-Path -LiteralPath $errorFile) { Get-Content -LiteralPath $errorFile | Write-Host }
        }
        throw "Setup exit $($process.ExitCode), expected $ExpectedExit. Executable: $Executable"
    }
}
function State { return Get-Content -LiteralPath (Join-Path $root 'installation.json') -Raw | ConvertFrom-Json }
function Current-Directory {
    $state = State
    return Join-Path $root ('versions\' + $state.current)
}
function Test-Startup([string] $Mode) {
    $client = Join-Path $output 'startup-client.ps1'
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Invoke-StartupTestClient.ps1') -Destination $client -Force
    $arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -Desktop "{1}" -Mode {2}' -f
        $client, (Join-Path (Current-Directory) 'quad9-desktop.exe'), $Mode
    $process = Start-Process -FilePath $powershell -ArgumentList $arguments -Credential $credential -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $output "startup-$Mode.out") -RedirectStandardError (Join-Path $output "startup-$Mode.err")
    try {
        if ($process.ExitCode -ne 0) {
            Get-Content -LiteralPath (Join-Path $output "startup-$Mode.err") | Write-Host
            throw "Standard-user startup check failed: $Mode"
        }
    } finally { $process.Dispose() }
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
function Wait-Quad9State([string] $Status) {
    $controller = Get-Service Quad9Connect
    try { $controller.WaitForStatus($Status, [TimeSpan]::FromSeconds(30)) }
    finally { $controller.Dispose() }
}
function Enable-VM {
    # Only the fixture binary permits a VM adapter. The distributable installer
    # always contains the production service and always registers `service`.
    $command = (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'").PathName
    Stop-Service -Name Quad9Connect
    Wait-Quad9State 'Stopped'
    $fixture = Join-Path $root 'vm-fixture'
    if (-not (Test-Path -LiteralPath $fixture)) { New-Item -ItemType Directory -Path $fixture | Out-Null }
    $fixtureBinary = Join-Path $fixture 'quad9-service.exe'
    Copy-Item -LiteralPath $VmServiceBinary -Destination $fixtureBinary -Force
    $changed = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'") -MethodName Change -Arguments @{ PathName = ('"{0}" service-vm' -f $fixtureBinary) }
    if ($changed.ReturnValue) { throw 'Could not configure VM fixture service.' }
    try {
        Start-Service Quad9Connect
        Wait-Quad9State 'Running'
        $status = Request @{ type = 'get_status' }
        $enabled = Request @{ type = 'enable_quad9'; expected_revision = $status.result.revision; targets = @(@{ adapter_id = $baseline.adapter_id; family = 'ipv4' }) }
        if ($enabled.result.type -ne 'enabled') { throw ($enabled | ConvertTo-Json -Depth 8 -Compress) }
    } finally {
        Stop-Service Quad9Connect
        Wait-Quad9State 'Stopped'
        $changed = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'") -MethodName Change -Arguments @{ PathName = $command }
        if ($changed.ReturnValue) { throw 'Could not restore production service command.' }
        Start-Service Quad9Connect
        Wait-Quad9State 'Running'
    }
    Assert-Active
}

try {
    Run-Setup $Installer ('/S /OPERATORSID=' + $user.SID.Value)
    $initial = State
    $status = Request @{ type = 'get_status' }
    if ($status.result.revision -ne 0 -or $null -ne $status.result.journal) { throw 'Fresh installation enabled DNS.' }
    Assert-Baseline
    Test-Startup 'read-off'
    Test-Startup 'tray'
    Test-Startup 'off'
    $nativeOutput = Join-Path $output 'native-startup'
    New-Item -ItemType Directory -Path $nativeOutput -Force | Out-Null
    # Only this report/profile folder is user-writable. Installed payloads,
    # executable test sources, and service data keep their existing permissions.
    $nativeAcl = Get-Acl -LiteralPath $nativeOutput
    $nativeAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($user.SID, 'Modify', 'ContainerInherit, ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $nativeOutput -AclObject $nativeAcl
    $nativeArguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -Node "{1}" -Desktop "{2}" -Output "{3}"' -f
        (Join-Path $PSScriptRoot 'Invoke-NativeStartupTest.ps1'), (Get-Command node).Source,
        (Join-Path (Current-Directory) 'quad9-desktop.exe'), $nativeOutput
    $nativeUser = Start-Process -FilePath $powershell -ArgumentList $nativeArguments -Credential $credential -LoadUserProfile -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $output 'native-startup.out') -RedirectStandardError (Join-Path $output 'native-startup.err')
    try {
        if ($nativeUser.ExitCode -ne 0) {
            Get-Content -LiteralPath (Join-Path $output 'native-startup.err') | Write-Host
            throw 'Installed native UI Windows startup validation failed.'
        }
    } finally { $nativeUser.Dispose() }
    # A second user's task exercises installer updates and removal across users.
    $startupModule = Import-Module (Join-Path (Current-Directory) 'InstallerLifecycle.psm1') -Force -PassThru
    & $startupModule { param($Desktop); $null = Invoke-Quad9StartupPreference $Desktop -Mode window -Write } (Join-Path (Current-Directory) 'quad9-desktop.exe')
    Assert-Baseline
    $status = Request @{ type = 'get_status' }
    if ($status.result.revision -ne 0 -or $null -ne $status.result.journal) { throw 'Windows startup preferences changed DNS management.' }
    # Refuse removal before deleting retry commands when the tray UI is open.
    $previousProfile = $env:WEBVIEW2_USER_DATA_FOLDER
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $output 'interface-profile'
    $interface = Start-Process -FilePath (Join-Path (Current-Directory) 'quad9-desktop.exe') -WindowStyle Hidden -PassThru
    try {
        Start-Sleep -Seconds 2
        if ($interface.HasExited) { throw 'The interface fixture exited unexpectedly.' }
        & $powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path (Current-Directory) 'Manage-Quad9Installation.ps1') -Operation Uninstall
        if ($LASTEXITCODE -ne 1 -or -not (State).installed -or -not (Test-Path (Join-Path (Current-Directory) 'Manage-Quad9Installation.ps1'))) { throw 'An open interface caused partial quiet removal.' }
        Assert-Baseline
        Test-Startup 'read-tray'
    } finally {
        if (-not $interface.HasExited) { Stop-Process -Id $interface.Id -Force; $interface.WaitForExit() }
        $interface.Dispose()
        $env:WEBVIEW2_USER_DATA_FOLDER = $previousProfile
    }
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
    [IO.File]::WriteAllText((Join-Path (Join-Path $root ('versions\' + $initial.current)) 'external-note.txt'), 'preserve this unrelated file')
    $journalHash = (Get-FileHash -LiteralPath (Join-Path $data 'dns.journal.json')).Hash
    # Simulate termination midway through copying a new directory, before any
    # service replacement. Retry retains the partial bytes and resumes safely.
    $partial = $initial.versions[0] | ConvertTo-Json -Depth 15 | ConvertFrom-Json
    $partial.id = 'e' * 64
    $partialPath = Join-Path $root ('versions\' + $partial.id)
    $lifecycleModule = Import-Module (Join-Path $repo 'installer\windows\InstallerLifecycle.psm1') -Force -PassThru
    & $lifecycleModule {
        param($Candidate, $Path)
        Write-AtomicJson 'staging.json' ([pscustomobject]@{ schema = 1; candidate = $Candidate })
        New-ProtectedDirectory $Path
    } $partial $partialPath
    [IO.File]::WriteAllText((Join-Path $partialPath 'quad9-service.exe'), 'interrupted copy')
    Run-Setup $Installer '/S'
    if ((Test-Path -LiteralPath $partialPath) -or (Test-Path -LiteralPath (Join-Path $root 'staging.json'))) { throw 'Partial staging was not reconciled.' }
    $quarantined = @(Get-ChildItem -LiteralPath (Join-Path $root 'retained-staging') -Directory)
    if ($quarantined.Count -ne 1 -or [IO.File]::ReadAllText((Join-Path $quarantined[0].FullName 'quad9-service.exe')) -cne 'interrupted copy') { throw 'Interrupted copy bytes were not retained.' }
    Assert-Active
    $upgrade = & (Join-Path $repo 'scripts\Build-WindowsInstaller.ps1') -ServiceBinary $ServiceBinary -DesktopBinary $DesktopBinary -MakeNsis $MakeNsis -OutputDirectory $output -Version '0.1.0-dev.upgrade'
    Run-Setup $upgrade '/S'
    $upgraded = State
    if ($upgraded.current -eq $initial.current -or $upgraded.enrollment.sid -ne $user.SID.Value) { throw 'Upgrade did not change payload or preserve authorization.' }
    Assert-Active
    Test-Startup 'read-tray'
    Test-Startup 'window'
    Test-Startup 'tray'
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
    Test-Startup 'read-tray'
    Assert-Active

    # Simulate an interrupted replacement with durable pending metadata and a gate.
    $state = State
    $candidate = @($state.versions | Where-Object id -eq $initial.current)[0]
    Remove-Item -LiteralPath (Join-Path (Join-Path $root ('versions\' + $candidate.id)) 'quad9-desktop.exe')
    Remove-Item -LiteralPath (Join-Path (Join-Path $root ('versions\' + $candidate.id)) 'Uninstall.exe')
    @{ schema = 1; previous = $state; candidate = $candidate; enrollment = $state.enrollment } | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $root 'pending.json')
    [IO.File]::WriteAllText($gate, '')
    Run-Setup $upgrade '/S'
    if (Test-Path -LiteralPath (Join-Path $root 'pending.json')) { throw 'Interrupted upgrade was not reconciled.' }
    Assert-Active

    # Quarantine-like damage to a rejected, obsolete payload must not prevent
    # current-version restoration or removal. Changed files remain for inspection.
    $obsolete = @((State).versions | Where-Object version -eq '0.1.0-dev.rejected')[0]
    $obsoleteDirectory = Join-Path $root ('versions\' + $obsolete.id)
    [IO.File]::AppendAllText((Join-Path $obsoleteDirectory 'quad9-service.exe'), 'quarantined fixture')
    Remove-Item -LiteralPath (Join-Path $obsoleteDirectory 'quad9-desktop.exe')

    # An external edit blocks uninstall and must remain untouched.
    [Quad9Validation.DnsApi]::Write([guid]$baseline.adapter_id, $false, '149.112.112.112,9.9.9.9', @())
    $uninstaller = Join-Path (Current-Directory) 'Uninstall.exe'
    # NSIS's self-copying bootstrap returns zero even when its child aborts.
    # Wait for its descendants, then inspect the actual retained state.
    Run-Setup $uninstaller '/S'
    if ((Get-Service Quad9Connect).Status -ne 'Running' -or -not (Test-Path -LiteralPath (Join-Path $data 'dns.journal.json'))) { throw 'Blocked uninstall removed its recovery mechanism.' }
    if ([Quad9Validation.DnsApi]::Read([guid]$baseline.adapter_id, $false).NameServer -ne '149.112.112.112,9.9.9.9') { throw 'Uninstall overwrote an external edit.' }
    # QuietUninstallString is the synchronous, elevated management entry point.
    # Its exit code must report the same blocked removal accurately.
    $quiet = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Quad9Connect').QuietUninstallString
    if ($quiet -notlike '*Manage-Quad9Installation.ps1* -Operation Uninstall') { throw 'Quiet uninstall does not use the synchronous lifecycle command.' }
    & $powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path (Current-Directory) 'Manage-Quad9Installation.ps1') -Operation Uninstall
    if ($LASTEXITCODE -ne 1 -or (Get-Service Quad9Connect).Status -ne 'Running') { throw 'Quiet uninstall did not report incomplete recovery and retain the service.' }
    Test-Startup 'read-tray'
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
    $startupScheduler = New-Object -ComObject 'Schedule.Service'
    $startupScheduler.Connect()
    if (@($startupScheduler.GetFolder('\').GetTasks(1) | Where-Object Name -ceq ("Quad9Connect.Startup." + $user.SID.Value)).Count) { throw 'Safe uninstall left the enrolled user startup task.' }
    $runnerSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if (@($startupScheduler.GetFolder('\').GetTasks(1) | Where-Object Name -ceq ("Quad9Connect.Startup." + $runnerSid)).Count) { throw 'Safe uninstall left the native UI user startup task.' }
    if (-not (Test-Path -LiteralPath (Join-Path $data 'dns.journal.json'))) { throw 'Uninstall discarded the audit and recovery journal.' }
    foreach ($name in @('startup-events.v1.json', 'startup-events.v1.json.tmp', 'startup-events.v1.lock')) {
        if (Test-Path -LiteralPath (Join-Path $data $name)) { throw 'Safe uninstall retained an operational log file.' }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $obsoleteDirectory 'quad9-service.exe'))) { throw 'Uninstall deleted a changed obsolete file.' }
    $operatorsIni = Join-Path $output 'operators-after-removal.ini'
    & $powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $repo 'installer\windows\List-Operators.ps1') -OutputPath $operatorsIni
    if ($LASTEXITCODE -ne 0 -or [IO.File]::ReadAllText($operatorsIni) -notmatch '(?m)^installed=0\r?$') { throw 'Removed metadata incorrectly permits keeping operator access during reinstall.' }
    # Reinstall after removal must discard inventories of files already removed.
    Run-Setup $Installer ('/S /OPERATORSID=' + $user.SID.Value)
    Assert-Baseline
    if (-not @(Get-ChildItem -LiteralPath (Join-Path $root 'retained-removal') -Filter external-note.txt -Recurse -File).Count) { throw 'Reinstall lost retained unrelated files.' }
    Run-Setup (Join-Path (Current-Directory) 'Uninstall.exe') '/S'
    Assert-Baseline
    [ordered]@{ result = 'passed'; first_install_dns_unchanged = $true; open_interface_preserved = $true; standard_user_authorization = $true; maintenance_gate = $true; partial_staging_recovered = $true; active_upgrade = $true; failed_upgrade_rollback = $true; interrupted_upgrade = $true; missing_candidate_ui_recovered = $true; conflict_blocks_uninstall = $true; quiet_uninstall_exit_code = $true; administrator_repair = $true; uninstall_restores_dns = $true; reinstall = $true; os_build = [Environment]::OSVersion.Version.Build } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'installer-validation.json')
    Write-Output 'Real NSIS installer lifecycle passed on this disposable VM.'
} finally {
    $service = Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') { Stop-Service Quad9Connect -ErrorAction Continue }
    Restore-ValidationBaseline $baseline
    Remove-LocalUser -Name $operator -ErrorAction SilentlyContinue
}
