#requires -RunAsAdministrator
param([switch] $RestoreOnly)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'This mutating validation script runs only inside disposable GitHub-hosted Windows VMs.'
}
. (Join-Path $PSScriptRoot 'NativeDnsApi.ps1')
$baselinePath = Join-Path $env:RUNNER_TEMP 'quad9.native-baseline.json'
if ($RestoreOnly) {
    if (Test-Path -LiteralPath $baselinePath) {
        Restore-ValidationBaseline (Get-Content -LiteralPath $baselinePath -Raw | ConvertFrom-Json)
    }
    return
}

$adapter = Get-NetIPConfiguration | Where-Object { $null -ne $_.IPv4DefaultGateway } | Select-Object -First 1
if ($null -eq $adapter) { throw 'No validation adapter has an IPv4 default gateway.' }
$id = [guid]$adapter.NetAdapter.InterfaceGuid
$families = @()
foreach ($ipv6 in @($false, $true)) {
    $snapshot = [Quad9Validation.DnsApi]::Read($id, $ipv6)
    if ($snapshot.Code -ne 0) { throw "Native read failed for IPv6=${ipv6}: $($snapshot.Code)" }
    if ($snapshot.ProfileNameServer -or $snapshot.ProfileCount -ne 0) { throw 'Profile DNS settings require separate validation.' }
    $protocol = if ($ipv6) { 'Tcpip6' } else { 'Tcpip' }
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$protocol\Parameters\Interfaces\{$id}"
    $static = Get-ItemPropertyValue -LiteralPath $key -Name NameServer -ErrorAction SilentlyContinue
    $normalize = { param($text) (([string]$text -split '[,\s]+' | Where-Object { $_ }) -join ',').ToLowerInvariant() }
    if ((& $normalize $static) -ne (& $normalize $snapshot.NameServer)) {
        throw "API and static configuration disagree for IPv6=$ipv6"
    }
    foreach ($address in ([string]$snapshot.NameServer -split '[,\s]+' | Where-Object { $_ })) {
        if (([Net.IPAddress]::Parse($address).AddressFamily -eq [Net.Sockets.AddressFamily]::InterNetworkV6) -ne $ipv6) {
            throw 'Family-specific native read returned a server from another address family.'
        }
    }
    $families += @{ ipv6 = $ipv6; snapshot = $snapshot }
}
$baseline = @{ adapter_id = $id.ToString(); families = $families }
$baseline | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $baselinePath -Encoding utf8
try {
    foreach ($family in $families) {
        $servers = if ($family.ipv6) { '2620:fe::fe,2620:fe::9' } else { '9.9.9.9,149.112.112.112' }
        $properties = @(0, 1 | ForEach-Object { [Quad9Validation.DohValue]@{
            Version = 1; Index = $_; Type = 1; Template = 'https://dns.quad9.net/dns-query'; Flags = 2
        } })
        [Quad9Validation.DnsApi]::Write($id, $family.ipv6, $servers, $properties)
        $actual = [Quad9Validation.DnsApi]::Read($id, $family.ipv6)
        if ($actual.Code -ne 0 -or $actual.NameServer -ne $servers -or $actual.Properties.Count -ne 2) {
            throw "Quad9 readback did not match for IPv6=$($family.ipv6): $($actual | ConvertTo-Json -Depth 6 -Compress)"
        }
        if (@($actual.Properties | Where-Object { $_.Flags -ne 2 -or $_.Template -ne 'https://dns.quad9.net/dns-query' }).Count -ne 0) {
            throw 'Strict encryption readback did not match.'
        }
        Restore-ValidationBaseline @{ adapter_id = $id.ToString(); families = @($family) }
        $restored = [Quad9Validation.DnsApi]::Read($id, $family.ipv6)
        if (([string]$restored.NameServer) -ne ([string]$family.snapshot.NameServer) -or
            ($restored.Properties | ConvertTo-Json -Depth 6 -Compress) -ne ($family.snapshot.Properties | ConvertTo-Json -Depth 6 -Compress)) {
            throw 'Original assignment and DoH settings were not restored exactly.'
        }
        Write-Output "Strict DoH write, readback, and baseline restore passed for IPv6=$($family.ipv6)."
    }
} finally {
    Restore-ValidationBaseline $baseline
}
