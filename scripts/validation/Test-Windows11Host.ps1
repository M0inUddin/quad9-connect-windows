#requires -RunAsAdministrator
param(
    [Parameter(Mandatory)][string] $RunDirectory,
    [string] $BinaryPath,
    [string] $InspectorPath,
    [guid] $AdapterId,
    [string] $OperatorSid,
    [switch] $Recover
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$OutputEncoding = [Text.UTF8Encoding]::new($false)
$programData = [Environment]::GetFolderPath('CommonApplicationData')
$runRoot = [IO.Path]::GetFullPath($RunDirectory)
if ([IO.Path]::GetDirectoryName($runRoot) -ne $programData -or
    [IO.Path]::GetFileName($runRoot) -notmatch '^Quad9ConnectHostValidation-[a-f0-9]{32}$') {
    throw 'The run directory must be a unique direct child of ProgramData.'
}
$taskName = [IO.Path]::GetFileName($runRoot)
$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
$installed = Join-Path $installRoot 'quad9-service.exe'
$serviceData = Join-Path $programData 'Quad9Connect'
$powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'

function Invoke-HostRequest {
    param($Request)
    $output = ($Request | ConvertTo-Json -Depth 12 -Compress) | & $installed request
    if ($LASTEXITCODE -ne 0) { throw 'Service IPC failed.' }
    $response = $output | ConvertFrom-Json
    if ($response.id -ne $Request.id -or $response.version -ne 1) { throw 'Mismatched IPC response.' }
    return $response
}
function Assert-ProtectedRunDirectory {
    $security = Get-Acl -LiteralPath $runRoot
    $trusted = @('S-1-5-18', 'S-1-5-32-544')
    if ($security.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted) { throw 'The recovery directory owner is not trusted.' }
    $writeRights = [Security.AccessControl.FileSystemRights]'Write, Delete, ChangePermissions, TakeOwnership'
    foreach ($rule in $security.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and $rule.IdentityReference.Value -notin $trusted -and ($rule.FileSystemRights -band $writeRights)) {
            throw 'The recovery directory is writable by an untrusted principal.'
        }
    }
}
function Invoke-HostCommand {
    param($Command)
    Invoke-HostRequest @{ version = 1; id = 'host-' + [guid]::NewGuid().ToString('N'); command = $Command }
}
function Assert-HostBaseline {
    param($Plan)
    foreach ($family in $Plan.families) {
        $actual = [Quad9Validation.DnsApi]::Read([guid]$Plan.adapter_id, [bool]$family.ipv6)
        if ((Get-HostDnsSignature $actual) -cne (Get-HostDnsSignature $family.snapshot)) { throw 'Native baseline comparison failed.' }
    }
}
function Remove-HostTestInstallation {
    param($Plan)
    if (-not (Test-Path -LiteralPath (Join-Path $runRoot 'installation-started'))) {
        $recovery = Restore-HostDnsBaseline $Plan
        if (-not $recovery.Complete) { throw ($recovery.Issues -join ' ') }
        Assert-HostBaseline $Plan
        return
    }
    $service = Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'"
    if ($null -ne $service) {
        if ($service.PathName -ne ('"{0}" service' -f $installed) -or
            (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $Plan.binary_sha256) {
            throw 'The installed service is no longer this test build; retain it for inspection.'
        }
        try {
            & (Join-Path $runRoot 'scripts\Uninstall-DevelopmentService.ps1')
        } catch {
            # Prevent further service writes before attempting the narrow native
            # recovery below. Keep the binary and journal if uninstall failed.
            $controller = [ServiceProcess.ServiceController]::new('Quad9Connect')
            if ($controller.Status -ne 'Stopped') { $controller.Stop() }
            $controller.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
        }
    }
    $recovery = Restore-HostDnsBaseline $Plan
    if (-not $recovery.Complete) { throw ($recovery.Issues -join ' ') }
    Assert-HostBaseline $Plan
    if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) {
        throw 'DNS is restored, but the stopped service and recovery records require inspection.'
    }
    # Copying can complete before service creation. Remove only the recorded
    # executable in that partial-install state, leaving any unknown files alone.
    Remove-HostValidationBinary -Path $installed -ExpectedSha256 $Plan.binary_sha256
    Restore-HostOperatorEnrollment $Plan
    # Retain this run's recovery records without making a later development
    # install mistake them for a preexisting installation. Never delete them.
    $archive = Join-Path $runRoot 'retained-service-state'
    if (Test-Path -LiteralPath $serviceData) {
        if (Test-Path -LiteralPath $archive) { throw 'Recovery archive already exists; preserve both copies.' }
        if ((Get-Item -LiteralPath $serviceData).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected service data reparse point.' }
        # Both absolute paths are fixed below ProgramData, and this run refused
        # any preexisting service data before installing its fresh service.
        if ([IO.Path]::GetFullPath($serviceData) -ne (Join-Path $programData 'Quad9Connect') -or
            [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($archive)) -ne $runRoot) { throw 'Unexpected archive path.' }
        Move-Item -LiteralPath $serviceData -Destination $archive
    }
    if (Test-Path -LiteralPath $installRoot) {
        # Remove only the now-empty directory created by this run.
        [IO.Directory]::Delete($installRoot, $false)
    }
}

if ($Recover) {
    Assert-ProtectedRunDirectory
    . (Join-Path $runRoot 'NativeDnsApi.ps1')
    . (Join-Path $runRoot 'HostValidationHelpers.ps1')
    if (Test-Path -LiteralPath (Join-Path $runRoot 'completed')) {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
        return
    }
    $plan = Get-Content -LiteralPath (Join-Path $runRoot 'plan.json') -Raw | ConvertFrom-Json
    $owner = Get-Process -Id $plan.owner_pid -ErrorAction SilentlyContinue
    if (Test-HostValidationOwner $plan $owner) {
        Stop-Process -Id $owner.Id -Force
        if (-not $owner.WaitForExit(10000)) { throw 'The test process did not stop; recovery cannot race it.' }
    }
    try {
        Remove-HostTestInstallation $plan
        'Recovered baseline and removed the temporary service.' | Set-Content -LiteralPath (Join-Path $runRoot 'watchdog-result.txt')
        New-Item -ItemType File -Path (Join-Path $runRoot 'completed') | Out-Null
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    } catch {
        $_.Exception.Message | Set-Content -LiteralPath (Join-Path $runRoot 'watchdog-result.txt')
        throw
    }
    return
}

$os = Get-CimInstance Win32_OperatingSystem
if ($os.Caption -notlike '*Windows 11*' -or -not [Environment]::Is64BitProcess) { throw 'Use a 64-bit administrator shell on Windows 11.' }
$hostArguments = [Environment]::GetCommandLineArgs()
$fileIndex = [Array]::IndexOf($hostArguments, '-File')
if ($fileIndex -lt 0 -or $fileIndex + 1 -ge $hostArguments.Length -or
    [IO.Path]::GetFullPath($hostArguments[$fileIndex + 1]) -ne $PSCommandPath) {
    throw 'Launch this test in a dedicated PowerShell process with -File.'
}
if ((Test-Path -LiteralPath $runRoot) -or (Test-Path -LiteralPath $installRoot) -or
    (Test-Path -LiteralPath $serviceData) -or (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue)) {
    throw 'This test requires fresh run, service, Program Files, and service-data locations. Existing recovery state is preserved.'
}
New-Item -ItemType Directory -Path $runRoot | Out-Null
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetOwner([Security.Principal.SecurityIdentifier]'S-1-5-32-544')
$acl.SetAccessRuleProtection($true, $false)
$inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]$sid,
        [Security.AccessControl.FileSystemRights]::FullControl, $inherit, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow))
}
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]'S-1-5-32-545',
    [Security.AccessControl.FileSystemRights]::ReadAndExecute, $inherit, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow))
Set-Acl -LiteralPath $runRoot -AclObject $acl
Assert-ProtectedRunDirectory
Start-Transcript -LiteralPath (Join-Path $runRoot 'transcript.txt') | Out-Null
$record = [ordered]@{ schema_version = 1; operating_system = $os.Caption; version = $os.Version; build = $os.BuildNumber;
    result = 'failed'; stage = 'preflight'; native_roundtrip = 'not_run'; service_roundtrip = 'not_run';
    restart_persistence = 'not_run'; historical_retry = 'not_run'; baseline_restored = $false; service_removed = $false; error = $null }
$plan = $null
$armed = $false
try {
    foreach ($file in @('Test-Windows11Host.ps1', 'NativeDnsApi.ps1', 'HostValidationHelpers.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination (Join-Path $runRoot $file)
    }
    . (Join-Path $PSScriptRoot 'HostValidationHelpers.ps1')
    Copy-HostServiceScripts -ScriptsDirectory (Join-Path $PSScriptRoot '..') -RunDirectory $runRoot
    Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $runRoot 'quad9-service.exe')
    Copy-Item -LiteralPath $InspectorPath -Destination (Join-Path $runRoot 'quad9-inspect.exe')
    . (Join-Path $runRoot 'NativeDnsApi.ps1')
    . (Join-Path $runRoot 'HostValidationHelpers.ps1')
    $inspection = (& (Join-Path $runRoot 'quad9-inspect.exe') adapters --json) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Adapter inspection failed.' }
    $adapter = @($inspection.adapters | Where-Object { $_.id -eq $AdapterId.ToString() -and $_.candidate })
    if ($adapter.Count -ne 1) { throw 'The selected physical adapter is unavailable or unsupported.' }
    $families = @()
    foreach ($ipv6 in @($false, $true)) {
        if (($ipv6 -and -not $adapter[0].ipv6_usable) -or (-not $ipv6 -and -not $adapter[0].ipv4_enabled)) { continue }
        $snapshot = [Quad9Validation.DnsApi]::Read($AdapterId, $ipv6)
        $protocol = if ($ipv6) { 'Tcpip6' } else { 'Tcpip' }
        $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$protocol\Parameters\Interfaces\{$AdapterId}")
        $static = $null
        if ($null -ne $key) {
            try {
                if ($key.GetValueNames() -contains 'NameServer') {
                    if ($key.GetValueKind('NameServer') -ne [Microsoft.Win32.RegistryValueKind]::String) { throw 'Unsupported static DNS value type.' }
                    $static = $key.GetValue('NameServer', $null)
                }
            } finally { $key.Dispose() }
        }
        Assert-HostAutomaticBaseline $snapshot $static
        $families += [pscustomobject]@{ ipv6 = $ipv6; snapshot = $snapshot; applied = (New-HostQuad9Snapshot $ipv6) }
    }
    if ($families.Count -eq 0) { throw 'No supported address family is available.' }
    foreach ($other in $inspection.adapters | Where-Object { $_.connected }) {
        foreach ($ipv6 in @($false, $true)) {
            $active = if ($ipv6) { $other.has_ipv6_dns } else { $other.has_ipv4_dns }
            if ($active -and ($other.id -ne $AdapterId.ToString() -or @($families | Where-Object { $_.ipv6 -eq $ipv6 }).Count -eq 0)) {
                throw 'Another active resolver scope is outside this test selection.'
            }
        }
    }
    foreach ($path in @('SOFTWARE\Policies\Microsoft\Windows NT\DNSClient', 'SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig')) {
        $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path)
        if ($null -ne $key) {
            try { if ($key.ValueCount -ne 0 -or $key.SubKeyCount -ne 0) { throw 'Managed DNS policy is outside this host test.' } } finally { $key.Dispose() }
        }
    }
    $group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
    $wasMember = $false
    if ($null -ne $group) { $wasMember = @((Get-LocalGroupMember -Group $group) | Where-Object { $_.SID.Value -eq $OperatorSid }).Count -ne 0 }
    $plan = [pscustomobject]@{ schema_version = 1; adapter_id = $AdapterId.ToString(); families = $families;
        operator_sid = $OperatorSid; group_existed = ($null -ne $group); operator_was_member = $wasMember;
        owner_pid = $PID; owner_started_ticks = (Get-Process -Id $PID).StartTime.ToUniversalTime().Ticks;
        owner_executable = (Get-Process -Id $PID).Path;
        binary_sha256 = (Get-FileHash -LiteralPath (Join-Path $runRoot 'quad9-service.exe') -Algorithm SHA256).Hash }
    $plan | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath (Join-Path $runRoot 'plan.json') -Encoding utf8
    $action = New-ScheduledTaskAction -Execute $powershell -Argument ('-NoProfile -ExecutionPolicy Bypass -File "{0}" -RunDirectory "{1}" -Recover' -f (Join-Path $runRoot 'Test-Windows11Host.ps1'), $runRoot)
    $triggers = @((New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(3)), (New-ScheduledTaskTrigger -AtStartup))
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $triggers -User 'SYSTEM' -RunLevel Highest -Settings (New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::FromMinutes(2))) | Out-Null
    $armed = $true
    $record['families'] = @($families | ForEach-Object { if ($_.ipv6) { 'ipv6' } else { 'ipv4' } })
    $record.stage = 'native_write_restore'
    foreach ($family in $families) {
        try {
            [Quad9Validation.DnsApi]::Write($AdapterId, $family.ipv6, $family.applied.NameServer, (ConvertTo-DohValues @($family.applied.Properties)))
            $actual = [Quad9Validation.DnsApi]::Read($AdapterId, $family.ipv6)
            if ((Get-HostDnsSignature $actual) -cne (Get-HostDnsSignature $family.applied)) { throw 'Strict DoH readback did not match.' }
        } finally {
            $restored = Restore-HostDnsBaseline $plan
            if (-not $restored.Complete) { throw ($restored.Issues -join ' ') }
        }
    }
    Assert-HostBaseline $plan
    $record.native_roundtrip = 'passed'
    $record.stage = 'service_install'
    $operator = ([Security.Principal.SecurityIdentifier]$OperatorSid).Translate([Security.Principal.NTAccount]).Value
    New-Item -ItemType File -Path (Join-Path $runRoot 'installation-started') | Out-Null
    & (Join-Path $runRoot 'scripts\Install-DevelopmentService.ps1') -BinaryPath (Join-Path $runRoot 'quad9-service.exe') -OperatorAccount $operator
    $service = Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'"
    if ($service.StartName -ne 'LocalSystem') { throw 'The service did not start as LocalSystem.' }
    $status = Invoke-HostCommand @{ type = 'get_status' }
    if ($status.result.type -ne 'status' -or $status.result.revision -ne 0 -or $null -ne $status.result.journal) { throw 'Fresh service state must be off.' }
    Assert-HostBaseline $plan
    $targets = @($families | ForEach-Object { @{ adapter_id = $AdapterId.ToString(); family = $(if ($_.ipv6) { 'ipv6' } else { 'ipv4' }) } })
    $enable = @{ version = 1; id = 'host-enable-' + [guid]::NewGuid().ToString('N'); command = @{ type = 'enable_quad9'; expected_revision = 0; targets = $targets } }
    $record.stage = 'service_enable'
    $enabled = Invoke-HostRequest $enable
    if ($enabled.result.type -ne 'enabled') { throw ("Service enable failed: " + ($enabled.result | ConvertTo-Json -Compress)) }
    foreach ($family in $families) {
        if ((Get-HostDnsSignature ([Quad9Validation.DnsApi]::Read($AdapterId, $family.ipv6))) -cne (Get-HostDnsSignature $family.applied)) { throw 'Service strict DoH readback did not match.' }
    }
    $record.stage = 'restart_while_active'
    Restart-Service -Name Quad9Connect
    (Get-Service Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    $status = Invoke-HostCommand @{ type = 'get_status' }
    if ($status.result.type -ne 'status' -or -not $status.result.observed_matches -or $status.result.journal.phase -ne 'committed') { throw 'Committed configuration did not survive the service restart.' }
    $record.restart_persistence = 'passed'
    $record.stage = 'service_restore'
    $restored = Invoke-HostCommand @{ type = 'restore_previous_settings'; expected_revision = $status.result.revision }
    if ($restored.result.type -ne 'restored' -or -not $restored.result.complete) { throw 'Service restoration did not complete.' }
    Assert-HostBaseline $plan
    $record.service_roundtrip = 'passed'
    Restart-Service -Name Quad9Connect
    (Get-Service Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    $retry = Invoke-HostRequest $enable
    $status = Invoke-HostCommand @{ type = 'get_status' }
    if ($retry.result.type -ne 'enabled' -or $retry.result.revision -ne 1 -or $status.result.revision -ne 2 -or $status.result.journal.phase -ne 'restored') { throw 'A historical retry changed the restored state.' }
    Assert-HostBaseline $plan
    $record.historical_retry = 'passed'
    $record.result = 'passed'
} catch {
    $record.error = $_.Exception.Message
} finally {
    if ($armed) {
        try {
            Remove-HostTestInstallation $plan
            $record.baseline_restored = $true
            $record.service_removed = $true
            New-Item -ItemType File -Path (Join-Path $runRoot 'completed') | Out-Null
            Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
        } catch {
            $record.result = 'failed'
            $record.error = [string]$record.error + ' Cleanup: ' + $_.Exception.Message
            try { Assert-HostBaseline $plan; $record.baseline_restored = $true } catch { }
            $record.service_removed = ($null -eq (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue))
        }
    }
    $record['finished_utc'] = [DateTimeOffset]::UtcNow.ToString('o')
    $record | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runRoot 'result.json') -Encoding utf8
    Stop-Transcript | Out-Null
}
if ($record.result -ne 'passed') { exit 1 }
