Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
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
