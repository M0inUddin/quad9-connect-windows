Set-StrictMode -Version Latest

function Copy-HostServiceScripts {
    param([string] $ScriptsDirectory, [string] $RunDirectory)
    $scriptDirectory = Join-Path $RunDirectory 'scripts'
    $moduleDirectory = Join-Path $RunDirectory 'installer\windows'
    if ((Test-Path -LiteralPath $scriptDirectory) -or (Test-Path -LiteralPath (Join-Path $RunDirectory 'installer'))) {
        throw 'Host service staging requires fresh dependency directories.'
    }
    [IO.Directory]::CreateDirectory($scriptDirectory) | Out-Null
    [IO.Directory]::CreateDirectory($moduleDirectory) | Out-Null
    foreach ($file in @('Install-DevelopmentService.ps1', 'Uninstall-DevelopmentService.ps1', 'Use-ServiceSystemModules.ps1')) {
        Copy-Item -LiteralPath (Join-Path $ScriptsDirectory $file) -Destination (Join-Path $scriptDirectory $file)
    }
    # Preserve the uninstaller's repository-relative module path inside the
    # protected run directory, including for later recovery from the staged copy.
    Copy-Item -LiteralPath (Join-Path $ScriptsDirectory '..\installer\windows\InstallerLifecycle.psm1') -Destination (Join-Path $moduleDirectory 'InstallerLifecycle.psm1')
}

function Remove-HostValidationBinary {
    param([string] $Path, [string] $ExpectedSha256)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $ExpectedSha256) {
        throw 'The staged service binary is not the recorded test build; preserve it.'
    }
    Remove-Item -LiteralPath $Path
}

function Assert-HostAutomaticBaseline {
    param($Snapshot, $StaticNameServer)
    Get-HostDnsSignature $Snapshot | Out-Null
    if (-not [string]::IsNullOrEmpty([string]$StaticNameServer) -or
        -not [string]::IsNullOrEmpty([string]$Snapshot.NameServer) -or
        $Snapshot.Properties.Count -ne 0 -or $Snapshot.ProfileNameServer -or $Snapshot.ProfileCount -ne 0) {
        throw 'This host runner requires an automatic DNS baseline without per-server or profile settings. Use disposable VM fixtures for other baselines.'
    }
}

function Test-HostValidationOwner {
    param($Plan, $Owner)
    if ($null -eq $Owner -or $Owner.StartTime.ToUniversalTime().Ticks -ne $Plan.owner_started_ticks) { return $false }
    if ($Owner.Path -ne $Plan.owner_executable) { throw 'The live owner executable does not match the saved test process.' }
    return $true
}

function Restore-HostOperatorEnrollment {
    param($Plan)
    $group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
    if ($null -eq $group) { return }
    if (-not $Plan.operator_was_member) {
        $member = Get-LocalGroupMember -Group $group | Where-Object { $_.SID.Value -eq $Plan.operator_sid }
        if ($member) { Remove-LocalGroupMember -Group $group -Member $member }
    }
    if (-not $Plan.group_existed -and @(Get-LocalGroupMember -Group $group).Count -eq 0) {
        Remove-LocalGroup -InputObject $group
    }
}

function Get-HostDnsSignature {
    param([Parameter(Mandatory)] $Snapshot)
    if ($Snapshot.Code -ne 0) { throw "Native DNS read failed: $($Snapshot.Code)" }
    $properties = @($Snapshot.Properties | Sort-Object Index | ForEach-Object {
        [ordered]@{ Version = $_.Version; Index = $_.Index; Type = $_.Type; Template = $_.Template; Flags = $_.Flags }
    })
    [ordered]@{
        NameServer = [string]$Snapshot.NameServer
        ProfileNameServer = [string]$Snapshot.ProfileNameServer
        ProfileCount = $Snapshot.ProfileCount
        Properties = $properties
    } | ConvertTo-Json -Depth 8 -Compress
}

function New-HostQuad9Snapshot {
    param([bool] $Ipv6)
    [pscustomobject]@{
        Code = 0
        NameServer = $(if ($Ipv6) { '2620:fe::fe,2620:fe::9' } else { '9.9.9.9,149.112.112.112' })
        ProfileNameServer = $null
        ProfileCount = 0
        Properties = @(0, 1 | ForEach-Object {
            [pscustomobject]@{ Version = 1; Index = $_; Type = 1; Template = 'https://dns.quad9.net/dns-query'; Flags = 2 }
        })
    }
}

function Restore-HostDnsBaseline {
    param(
        [Parameter(Mandatory)] $Plan,
        [scriptblock] $Read = { param($Id, $Ipv6) [Quad9Validation.DnsApi]::Read([guid]$Id, $Ipv6) },
        [scriptblock] $Write = { param($Id, $Ipv6, $Snapshot)
            [Quad9Validation.DnsApi]::Write([guid]$Id, $Ipv6, $Snapshot.NameServer, (ConvertTo-DohValues @($Snapshot.Properties)))
        }
    )
    $issues = [Collections.Generic.List[string]]::new()
    foreach ($family in $Plan.families) {
        try {
            $current = & $Read $Plan.adapter_id ([bool]$family.ipv6)
            $signature = Get-HostDnsSignature $current
            $baseline = Get-HostDnsSignature $family.snapshot
            if ($signature -ceq $baseline) { continue }
            if ($signature -cne (Get-HostDnsSignature $family.applied)) {
                throw 'Settings changed outside this test; preserve them for inspection.'
            }
            & $Write $Plan.adapter_id ([bool]$family.ipv6) $family.snapshot | Out-Null
            if ((Get-HostDnsSignature (& $Read $Plan.adapter_id ([bool]$family.ipv6))) -cne $baseline) {
                throw 'Baseline readback did not match after restoration.'
            }
        } catch {
            $issues.Add("IPv6=$($family.ipv6): $($_.Exception.Message)")
        }
    }
    [pscustomobject]@{ Complete = ($issues.Count -eq 0); Issues = $issues.ToArray() }
}
