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

        # The offline restore must succeed before SCM deletion or payload removal.
        $script:Events = [Collections.Generic.List[string]]::new()
        $script:FixtureManifest = $manifest
        function Get-CurrentManifest { return $script:FixtureManifest }
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
        # Windows limits local-group descriptions to 48 characters. Exercise that
        # binding contract before actual installation instead of accepting any text.
        $script:EnrollmentAdds = 0
        function Get-LocalGroup { return $null }
        function New-LocalGroup([string] $Name, [ValidateLength(0,48)][string] $Description) { return [pscustomobject]@{ Name = $Name } }
        function Get-LocalGroupMember { return @() }
        function Add-LocalGroupMember { $script:EnrollmentAdds++ }
        Add-Enrollment ([pscustomobject]@{ sid = 'S-1-5-21-1-2-3-1001' })
        if ($script:EnrollmentAdds -ne 1) { throw 'Explicit operator enrollment was not applied.' }
    } $fixture
    Write-Output 'Installer contract tests passed: payload integrity, unsafe paths, unsupported storage, blocked uninstall, and upgrade rollback.'
} finally {
    # Remove only known flat fixtures from this unique test-owned directory.
    foreach ($file in Get-ChildItem -LiteralPath $fixture -File -Force) { Remove-Item -LiteralPath $file.FullName }
    [IO.Directory]::Delete($fixture, $false)
    Remove-Module $module
}
