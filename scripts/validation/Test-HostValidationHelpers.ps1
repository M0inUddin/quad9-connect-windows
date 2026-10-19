Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$tokens = $null; $parseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Test-Windows11Host.ps1'), [ref]$tokens, [ref]$parseErrors) | Out-Null
if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }
. (Join-Path $PSScriptRoot 'HostValidationHelpers.ps1')

$original = [pscustomobject]@{ Code = 0; NameServer = ''; ProfileNameServer = $null; ProfileCount = 0; Properties = @() }
Assert-HostAutomaticBaseline $original $null
foreach ($case in @(
    @{ snapshot = $original; registry = '192.0.2.53' },
    @{ snapshot = (New-HostQuad9Snapshot $false); registry = $null }
)) {
    $refused = $false
    try { Assert-HostAutomaticBaseline $case.snapshot $case.registry } catch { $refused = $true }
    if (-not $refused) { throw 'Unsupported native or registry baselines must be rejected before host writes.' }
}
$plan = [pscustomobject]@{ adapter_id = 'private-test'; families = @(
    [pscustomobject]@{ ipv6 = $false; snapshot = $original; applied = (New-HostQuad9Snapshot $false) },
    [pscustomobject]@{ ipv6 = $true; snapshot = $original; applied = (New-HostQuad9Snapshot $true) }
) }
$script:values = @{ False = $original; True = $original }
$script:writes = [Collections.Generic.List[bool]]::new()
$read = { param($Id, $Ipv6) $script:values[[string]$Ipv6] }
$write = { param($Id, $Ipv6, $Snapshot) $script:writes.Add($Ipv6); $script:values[[string]$Ipv6] = $Snapshot }

$result = Restore-HostDnsBaseline $plan -Read $read -Write $write
if (-not $result.Complete -or $writes.Count -ne 0) { throw 'An unchanged baseline must not be written.' }
$values.False = New-HostQuad9Snapshot $false
$values.True = New-HostQuad9Snapshot $true
$result = Restore-HostDnsBaseline $plan -Read $read -Write $write
if (-not $result.Complete -or $writes.Count -ne 2) { throw 'Both owned family changes must restore.' }

$writes.Clear()
$external = [pscustomobject]@{ Code = 0; NameServer = '192.0.2.53'; ProfileNameServer = $null; ProfileCount = 0; Properties = @() }
$values.False = $external
$values.True = New-HostQuad9Snapshot $true
$result = Restore-HostDnsBaseline $plan -Read $read -Write $write
if ($result.Complete -or $result.Issues.Count -ne 1 -or $writes.Count -ne 1 -or -not $writes[0] -or $values.False -ne $external) {
    throw 'Recovery must preserve an external edit while restoring the other owned family.'
}
$writes.Clear()
$values.False = [pscustomobject]@{ Code = 5 }
$values.True = $original
$result = Restore-HostDnsBaseline $plan -Read $read -Write $write
if ($result.Complete -or $writes.Count -ne 0) { throw 'Failed readback must never cause a speculative restore.' }
$values.False = New-HostQuad9Snapshot $false
$result = Restore-HostDnsBaseline $plan -Read $read -Write { }
if ($result.Complete) { throw 'A write without matching readback must not count as restored.' }
$writes.Clear()
$values.False = New-HostQuad9Snapshot $false
$values.False.Properties[0].Template = 'https://dns.quad9.net/DNS-query'
$result = Restore-HostDnsBaseline $plan -Read $read -Write $write
if ($result.Complete -or $writes.Count -ne 0) { throw 'A case-sensitive DoH path change must be preserved as an external edit.' }
Write-Output 'Host recovery helper tests passed: unchanged baseline, both families, external conflict, unavailable readback, and ineffective write.'

$started = [DateTime]::UtcNow
foreach ($executable in @('C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe', 'C:\Program Files\PowerShell\7\pwsh.exe')) {
    $ownerPlan = [pscustomobject]@{ owner_started_ticks = $started.Ticks; owner_executable = $executable }
    $owner = [pscustomobject]@{ StartTime = $started; Path = $executable }
    if (-not (Test-HostValidationOwner $ownerPlan $owner)) { throw 'Both dedicated PowerShell hosts must be stopped before recovery.' }
    $owner.StartTime = $started.AddSeconds(1)
    if (Test-HostValidationOwner $ownerPlan $owner) { throw 'A reused process ID must not be stopped.' }
    $owner.StartTime = $started
    $owner.Path = 'C:\other.exe'
    $refused = $false
    try { Test-HostValidationOwner $ownerPlan $owner | Out-Null } catch { $refused = $true }
    if (-not $refused) { throw 'An unverified live owner must block recovery.' }
}

# Stand-ins enforce the Windows cmdlet parameter contracts without modifying
# accounts. Exercise preservation of existing membership and group ownership.
$script:members = [Collections.Generic.List[object]]::new()
$script:groupRemoved = $false
function Get-LocalGroup { param($Name) [pscustomobject]@{ Name = $Name } }
function Get-LocalGroupMember { param($Group) $script:members.ToArray() }
function Remove-LocalGroupMember { param($Group, $Member) $script:members.Remove($Member) | Out-Null }
function Remove-LocalGroup { [CmdletBinding()] param([Parameter(Mandatory)] $InputObject) $script:groupRemoved = $true }
$enrollment = [pscustomobject]@{ operator_sid = 'test-user'; group_existed = $false; operator_was_member = $false }
$member = [pscustomobject]@{ SID = [pscustomobject]@{ Value = 'test-user' } }
$members.Add($member)
Restore-HostOperatorEnrollment $enrollment
if ($members.Count -ne 0 -or -not $groupRemoved) { throw 'New enrollment and its empty new group must be removed.' }
$groupRemoved = $false
$members.Add($member)
$enrollment.group_existed = $true
$enrollment.operator_was_member = $true
Restore-HostOperatorEnrollment $enrollment
if ($members.Count -ne 1 -or $groupRemoved) { throw 'Existing enrollment must remain unchanged.' }
$enrollment.group_existed = $false
$enrollment.operator_was_member = $false
$members.Add([pscustomobject]@{ SID = [pscustomobject]@{ Value = 'another-user' } })
Restore-HostOperatorEnrollment $enrollment
if ($members.Count -ne 1 -or $groupRemoved) { throw 'A group with another member must be preserved.' }
Write-Output 'Host cleanup tests passed: both PowerShell hosts, process identity, new enrollment cleanup, and preservation of preexisting or shared groups.'

$fixture = Join-Path ([IO.Path]::GetTempPath()) ('Quad9HostCleanup-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture) | Out-Null
$binary = Join-Path $fixture 'quad9-service.exe'
$unrelated = Join-Path $fixture 'unrelated.txt'
try {
    [IO.File]::WriteAllText($binary, 'recorded test build')
    [IO.File]::WriteAllText($unrelated, 'preserve this file')
    $hash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
    Remove-HostValidationBinary -Path $binary -ExpectedSha256 $hash
    if ((Test-Path -LiteralPath $binary) -or -not (Test-Path -LiteralPath $unrelated)) { throw 'Partial install cleanup must remove only the recorded binary.' }
    Remove-Item -LiteralPath $unrelated
    [IO.Directory]::Delete($fixture, $false)
    [IO.Directory]::CreateDirectory($fixture) | Out-Null
    [IO.File]::WriteAllText($binary, 'unknown replacement')
    $refused = $false
    try { Remove-HostValidationBinary -Path $binary -ExpectedSha256 $hash } catch { $refused = $true }
    if (-not $refused -or -not (Test-Path -LiteralPath $binary)) { throw 'An unknown replacement binary must be retained.' }
} finally {
    foreach ($file in @($binary, $unrelated)) { if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file } }
    if (Test-Path -LiteralPath $fixture) { [IO.Directory]::Delete($fixture, $false) }
}
Write-Output 'Partial install cleanup tests passed: recorded binary removed, unrelated file preserved, and unknown replacement refused.'

$staging = Join-Path ([IO.Path]::GetTempPath()) ('Quad9HostStaging-' + [guid]::NewGuid().ToString('N'))
$stagedPaths = @('scripts\Install-DevelopmentService.ps1', 'scripts\Uninstall-DevelopmentService.ps1', 'scripts\Use-ServiceSystemModules.ps1', 'installer\windows\InstallerLifecycle.psm1')
[IO.Directory]::CreateDirectory($staging) | Out-Null
$stagedModule = $null
try {
    Copy-HostServiceScripts -ScriptsDirectory (Join-Path $PSScriptRoot '..') -RunDirectory $staging
    foreach ($relative in $stagedPaths) {
        $source = Join-Path $PSScriptRoot "..\..\$relative"
        if ((Get-FileHash -LiteralPath (Join-Path $staging $relative)).Hash -ne (Get-FileHash -LiteralPath $source).Hash) { throw 'A staged service dependency is missing or differs from its source.' }
    }
    $stagedModule = Import-Module (Join-Path $staging 'scripts\..\installer\windows\InstallerLifecycle.psm1') -PassThru -Force
    if (-not (Get-Command Invoke-Quad9Lifecycle -Module $stagedModule.Name)) { throw 'Staged recovery could not load its lifecycle module.' }
    $refused = $false
    try { Copy-HostServiceScripts -ScriptsDirectory (Join-Path $PSScriptRoot '..') -RunDirectory $staging } catch { $refused = $true }
    if (-not $refused) { throw 'Host staging must not overwrite an existing dependency directory.' }
} finally {
    if ($null -ne $stagedModule) { Remove-Module $stagedModule -Force }
    foreach ($relative in $stagedPaths) { [IO.File]::Delete((Join-Path $staging $relative)) }
    foreach ($relative in @('scripts', 'installer\windows', 'installer')) {
        $directory = Join-Path $staging $relative
        if ([IO.Directory]::Exists($directory)) { [IO.Directory]::Delete($directory, $false) }
    }
    [IO.Directory]::Delete($staging, $false)
}
Write-Output 'Host staging passed: install, uninstall, module discovery and lifecycle dependencies remain inside the staged run; existing directories are refused.'
