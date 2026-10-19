# Nonprivileged contract tests. Service and DNS operations are replaced explicitly.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
foreach ($file in @(Get-ChildItem (Join-Path $repo 'installer\windows') -File | Where-Object Extension -in @('.ps1', '.psm1'))) {
    $errors = $null
    [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$null, [ref]$errors) | Out-Null
    if ($errors) { throw ($errors.Message -join '; ') }
}
$module = Import-Module (Join-Path $repo 'installer\windows\InstallerLifecycle.psm1') -Force -PassThru
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('quad9-installer-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    & $module {
        param($Fixture)
        function Must-Fail([scriptblock] $Action) {
            $failed = $false
            try { & $Action } catch { $failed = $true }
            if (-not $failed) { throw 'Expected a rejected lifecycle operation.' }
        }
        # Ordinary user-owned directories cannot become trusted service roots.
        $owner = (Get-Acl -LiteralPath $Fixture).GetOwner([Security.Principal.SecurityIdentifier]).Value
        if ($owner -notin $script:Trusted) { Must-Fail { Assert-ProtectedDirectory $Fixture } }
        $script:AclFixture = [Security.AccessControl.DirectorySecurity]::new()
        $script:AclFixture.SetOwner([Security.Principal.SecurityIdentifier]'S-1-5-32-544')
        $script:AclFixture.SetAccessRuleProtection($true, $false)
        function Get-Acl { return $script:AclFixture }
        Must-Fail { Assert-TrustedAcl 'in-memory ACL' }
        foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
            $script:AclFixture.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]$sid, 'FullControl', 'Allow'))
        }
        Assert-TrustedAcl 'in-memory ACL'
        $script:AclFixture.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]'S-1-5-32-545', 'Write', 'Allow'))
        Must-Fail { Assert-TrustedAcl 'in-memory ACL' }
        # The positive inventory cases use an explicitly substituted test root.
        function Assert-ProtectedDirectory([string] $Path) { Assert-RegularPath $Path $true }
        foreach ($name in $script:PayloadNames) { [IO.File]::WriteAllText((Join-Path $Fixture $name), $name) }
        $files = @($script:PayloadNames | ForEach-Object { [pscustomobject]@{ name = $_; sha256 = (Get-FileHash -LiteralPath (Join-Path $Fixture $_)).Hash } })
        $manifest = [pscustomobject]@{ schema = 1; protocol = 1; storage = 1; id = ('a' * 64); version = '0.1.0-dev'; files = $files }
        Assert-Manifest $manifest $Fixture
        $manifest.files[0].name = '..\quad9-service.exe'
        Must-Fail { Assert-Manifest $manifest $Fixture }
        $manifest.files[0].name = 'quad9-service.exe'
        $manifest.storage = 999
        Must-Fail { Assert-Manifest $manifest $Fixture }
        $manifest.storage = 1
        [IO.File]::AppendAllText((Join-Path $Fixture 'quad9-service.exe'), 'changed')
        Must-Fail { Assert-Manifest $manifest $Fixture }
        [IO.File]::WriteAllText((Join-Path $Fixture 'quad9-service.exe'), 'quad9-service.exe')
        $manifest.files = @($files[0]) + @($files[0]) + @($files[2..4])
        Must-Fail { Assert-Manifest $manifest $Fixture }
        $manifest.files = $files
        $fresh = New-InstallationState $manifest $null $null
        if (@($fresh.versions).Count -ne 1 -or $null -eq $fresh.versions[0]) { throw 'Fresh inventory contains a null payload.' }
        $repeat = New-InstallationState $manifest $fresh $null
        if (@($repeat.versions).Count -ne 1) { throw 'Repeated installation duplicated the payload.' }
        [IO.File]::WriteAllText((Join-Path $Fixture 'Uninstall.exe'), 'uninstaller')
        $manifest.files = @($manifest.files) + [pscustomobject]@{ name = 'Uninstall.exe'; sha256 = (Get-FileHash -LiteralPath (Join-Path $Fixture 'Uninstall.exe')).Hash }

        # The offline restore must succeed before SCM deletion or payload removal.
        $script:Events = [Collections.Generic.List[string]]::new()
        $script:FixtureManifest = $manifest
        function Get-CurrentManifest { return $script:FixtureManifest }
        function Get-CimInstance { return @() }
        function Assert-ProtectedDirectory {}
        function Assert-Manifest {}
        function Assert-ServiceIdentity { return [pscustomobject]@{ PathName = 'known service' } }
        function Set-MaintenanceGate { $script:Events.Add('gate') }
        function Stop-InstalledService { $script:Events.Add('stop') }
        function Invoke-Offline { $script:Events.Add('restore'); throw 'external DNS edit' }
        function Set-ServiceVersion { $script:Events.Add('configure') }
        function Start-Service { $script:Events.Add('start') }
        function Clear-MaintenanceGate { $script:Events.Add('ungate') }
        function Remove-ServiceRegistration { throw 'Must not delete a service after failed restoration' }
        function Complete-Quad9Removal { throw 'Must not delete files after failed restoration' }
        $state = [pscustomobject]@{ schema = 1; installed = $true; current = $manifest.id; versions = @($manifest); enrollment = $null }
        Must-Fail { Uninstall-Quad9Payload $state }
        if (-not $state.installed -or ($script:Events -join ',') -cne 'gate,stop,restore,configure,start,ungate') { throw 'Blocked removal did not retain a running recovery mechanism.' }

        # Rollback selects the previous binary, preserves its state and inventories
        # the rejected candidate. No DNS Restore is issued for an active upgrade.
        $script:Root = $Fixture
        function Assert-TrustedAcl {}
        function Start-InstalledService { $script:Events.Add('healthy') }
        function Publish-Installation {}
        function Get-CurrentManifest([object] $State) { return @($State.versions | Where-Object id -eq $State.current)[0] }
        $candidate = $manifest | ConvertTo-Json -Depth 10 | ConvertFrom-Json
        $candidate.id = 'b' * 64
        $pending = [pscustomobject]@{ schema = 1; previous = $state; candidate = $candidate; enrollment = $null }
        [IO.File]::WriteAllText((Join-Path $Fixture 'pending.json'), '{}')
        $script:Events.Clear()
        Restore-PendingInstallation $pending
        $restored = Get-Content -LiteralPath (Join-Path $Fixture 'installation.json') -Raw | ConvertFrom-Json
        if ($restored.current -ne $manifest.id -or $restored.versions.Count -ne 2 -or ($script:Events -join ',') -cne 'stop,configure,healthy,ungate') { throw 'Failed upgrade did not restore the previous service without DNS writes.' }
        if (Test-Path -LiteralPath (Join-Path $Fixture 'pending.json')) { throw 'Successful rollback left a pending transaction.' }
        $script:Events.Clear()
        function Assert-ServiceIdentity { throw 'The SCM command was changed externally' }
        Must-Fail { Repair-Quad9Payload $state }
        if ($script:Events.Count) { throw 'Repair modified an unrelated service registration.' }
        Write-AtomicJson 'atomic-test.json' ([pscustomobject]@{ schema = 1; revision = 1 })
        Write-AtomicJson 'atomic-test.json' ([pscustomobject]@{ schema = 1; revision = 2 })
        if ((Read-InstallJson 'atomic-test.json').revision -ne 2 -or
            (Get-Content -LiteralPath (Join-Path $Fixture 'atomic-test.json.previous') -Raw | ConvertFrom-Json).revision -ne 1) { throw 'Atomic replacement lost the new or previous metadata.' }
        # Windows limits local-group descriptions to 48 characters. Exercise that
        # binding contract before actual installation instead of accepting any text.
        $script:EnrollmentAdds = 0
        function Get-LocalGroup { return $null }
        function New-LocalGroup([string] $Name, [ValidateLength(0,48)][string] $Description) { return [pscustomobject]@{ Name = $Name } }
        function Get-LocalGroupMember { return @() }
        function Get-LocalUser { return [pscustomobject]@{ Kind = 'LocalPrincipal' } }
        function Add-LocalGroupMember([object] $Group, [object] $Member) {
            if ($Member -is [Security.Principal.SecurityIdentifier] -or $Member.Kind -ne 'LocalPrincipal') { throw 'Member requires a local principal, not a SecurityIdentifier object.' }
            $script:EnrollmentAdds++
        }
        Add-Enrollment ([pscustomobject]@{ sid = 'S-1-5-21-1-2-3-1001' })
        if ($script:EnrollmentAdds -ne 1) { throw 'Explicit operator enrollment was not applied.' }
        function Get-CimInstance { return [pscustomobject]@{ ExecutablePath = $null } }
        Must-Fail { Assert-InterfaceClosed $state }
        function Get-CimInstance { return @() }
        function Assert-ServiceIdentity { return $null }
        function New-ProtectedDirectory([string] $Path) { [IO.Directory]::CreateDirectory($Path) | Out-Null }
        $partial = $manifest | ConvertTo-Json -Depth 10 | ConvertFrom-Json
        $partial.id = 'c' * 64
        [IO.Directory]::CreateDirectory((Join-Path $Fixture 'versions')) | Out-Null
        Write-AtomicJson 'staging.json' ([pscustomobject]@{ schema = 1; candidate = $partial })
        $partialPath = Get-VersionPath $partial
        [IO.Directory]::CreateDirectory($partialPath) | Out-Null
        [IO.File]::WriteAllText((Join-Path $partialPath 'quad9-service.exe'), 'interrupted copy')
        Resolve-StagedCandidate (Read-InstallJson 'staging.json') $null
        $retained = @(Get-ChildItem -LiteralPath (Join-Path $Fixture 'retained-staging') -Directory)
        if ((Test-Path -LiteralPath $partialPath) -or $retained.Count -ne 1 -or
            [IO.File]::ReadAllText((Join-Path $retained[0].FullName 'quad9-service.exe')) -cne 'interrupted copy') { throw 'Partial staging was not retained safely for a retry.' }
        # Log cleanup is confined to three fixed names after service removal.
        $script:Data = Join-Path $Fixture 'operational-log-data'
        [IO.Directory]::CreateDirectory($script:Data) | Out-Null
        function Get-Service { return $null }
        foreach ($name in @('startup-events.v1.json', 'startup-events.v1.json.tmp', 'startup-events.v1.lock', 'dns.journal.json', 'external-note.txt')) {
            [IO.File]::WriteAllText((Join-Path $script:Data $name), 'retained fixture')
        }
        Remove-Quad9OperationalLog
        if (@(Get-ChildItem -LiteralPath $script:Data).Count -ne 2 -or
            -not (Test-Path -LiteralPath (Join-Path $script:Data 'dns.journal.json')) -or
            -not (Test-Path -LiteralPath (Join-Path $script:Data 'external-note.txt'))) { throw 'Operational cleanup touched recovery or unrelated files.' }
        [IO.File]::WriteAllText((Join-Path $script:Data 'startup-events.v1.json'), 'still owned by service')
        function Get-Service { return [pscustomobject]@{ Status = 'Running' } }
        Must-Fail { Remove-Quad9OperationalLog }
        if (-not (Test-Path -LiteralPath (Join-Path $script:Data 'startup-events.v1.json'))) { throw 'Log cleanup ran while a service remained.' }
        $script:PendingQueries = 2
        $script:PendingController = [pscustomobject]@{ Disposals = 0 }
        $script:PendingController | Add-Member -MemberType ScriptMethod -Name Dispose -Value { $this.Disposals++ }
        function Get-Service {
            if ($script:PendingQueries -gt 0) { $script:PendingQueries--; return $script:PendingController }
            return $null
        }
        function Test-Path([string] $LiteralPath) {
            if ($LiteralPath -eq 'HKLM:\SYSTEM\CurrentControlSet\Services\Quad9Connect') { return $script:PendingQueries -gt 0 }
            Microsoft.PowerShell.Management\Test-Path -LiteralPath $LiteralPath
        }
        function Start-Sleep { }
        Wait-Quad9ServiceRemoval
        if ($script:PendingController.Disposals -ne 2) { throw 'Removal polling retained its own service handles.' }
        function Get-Service { return $script:PendingController }
        $pendingRefused = $false
        try { Wait-Quad9ServiceRemoval 0 } catch { $pendingRefused = $_.Exception.Message -like '*deletion is pending*' }
        if (-not $pendingRefused) { throw 'A deletion timeout must defer cleanup.' }
        function Get-Service { return $null }
        $script:PendingQueries = 1
        $pendingRefused = $false
        try { Wait-Quad9ServiceRemoval 0 } catch { $pendingRefused = $_.Exception.Message -like '*deletion is pending*' }
        if (-not $pendingRefused) { throw 'A marked registry entry must still defer cleanup when OpenService no longer succeeds.' }
        $script:PendingQueries = 0
        Wait-Quad9ServiceRemoval 0
    } $fixture
    Write-Output 'Installer contract tests passed: payload integrity, unsafe paths, unsupported storage, blocked uninstall, and upgrade rollback.'
} finally {
    $resolved = (Resolve-Path -LiteralPath $fixture).Path
    if ($resolved -ine [IO.Path]::GetFullPath($fixture) -or
        (Split-Path $resolved -Parent) -ine ([IO.Path]::GetTempPath().TrimEnd('\')) -or
        (Split-Path $resolved -Leaf) -notmatch '^quad9-installer-test-[a-f0-9]{32}$') { throw 'Unsafe fixture cleanup scope.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
    Remove-Module $module
}
