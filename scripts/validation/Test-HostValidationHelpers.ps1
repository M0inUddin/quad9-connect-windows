Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$tokens = $null; $parseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Test-Windows11Host.ps1'), [ref]$tokens, [ref]$parseErrors) | Out-Null
if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }
. (Join-Path $PSScriptRoot 'HostValidationHelpers.ps1')

$original = [pscustomobject]@{ Code = 0; NameServer = ''; ProfileNameServer = $null; ProfileCount = 0; Properties = @() }
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
