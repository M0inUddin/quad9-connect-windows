Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:Root = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
$script:Data = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Quad9Connect'
$script:Trusted = @('S-1-5-18', 'S-1-5-32-544')
$script:PayloadNames = @('quad9-service.exe', 'quad9-desktop.exe', 'Manage-Quad9Installation.ps1', 'InstallerLifecycle.psm1', 'LICENSE.txt')
$script:Arp = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Quad9Connect'

function Assert-RegularPath([string] $Path, [bool] $Directory = $false) {
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.PSIsContainer -ne $Directory) {
        throw "Unexpected file type or reparse point: $Path"
    }
}

function Assert-ProtectedDirectory([string] $Path, [bool] $AllowRead = $true) {
    Assert-RegularPath $Path $true
    Assert-TrustedAcl $Path $AllowRead $true
}

function Assert-TrustedAcl([string] $Path, [bool] $AllowRead = $true, [bool] $RequireProtected = $false) {
    $acl = Get-Acl -LiteralPath $Path
    if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $script:Trusted -or ($RequireProtected -and -not $acl.AreAccessRulesProtected)) {
        throw "Untrusted owner or inherited installation permissions: $Path"
    }
    $write = [Security.AccessControl.FileSystemRights]'WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership'
    $full = [Security.AccessControl.FileSystemRights]::FullControl
    $systemFull = $false
    $adminFull = $false
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -ne 'Allow' -or
            ($rule.IdentityReference.Value -notin $script:Trusted -and
                (-not $AllowRead -or ($rule.FileSystemRights -band $write)))) {
            throw "Unsafe installation permissions: $Path"
        }
        if (($rule.FileSystemRights -band $full) -eq $full) {
            if ($rule.IdentityReference.Value -eq 'S-1-5-18') { $systemFull = $true }
            if ($rule.IdentityReference.Value -eq 'S-1-5-32-544') { $adminFull = $true }
        }
    }
    if (-not $systemFull -or -not $adminFull) { throw "Missing administrator/SYSTEM access or an empty/null DACL: $Path" }
}

function New-ProtectedDirectory([string] $Path) {
    if (Test-Path -LiteralPath $Path) { throw "Installation directory already exists: $Path" }
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner([Security.Principal.SecurityIdentifier]'S-1-5-32-544')
    $acl.SetAccessRuleProtection($true, $false)
    $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    foreach ($sid in $script:Trusted) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]$sid,
            [Security.AccessControl.FileSystemRights]::FullControl, $inherit, 'None', 'Allow'))
    }
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]'S-1-5-32-545',
        [Security.AccessControl.FileSystemRights]::ReadAndExecute, $inherit, 'None', 'Allow'))
    # Apply the complete DACL atomically at creation, including on first install.
    if ($PSVersionTable.PSEdition -eq 'Desktop') { [IO.Directory]::CreateDirectory($Path, $acl) | Out-Null }
    else { [IO.FileSystemAclExtensions]::CreateDirectory($acl, $Path) | Out-Null }
    Assert-ProtectedDirectory $Path
}

function Write-AtomicJson([string] $Name, [object] $Value) {
    $path = Join-Path $script:Root $Name
    $temporary = "$path.tmp"
    if (Test-Path -LiteralPath $path) { Assert-RegularPath $path }
    if (Test-Path -LiteralPath $temporary) { Assert-RegularPath $temporary; Remove-Item -LiteralPath $temporary }
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($Value | ConvertTo-Json -Depth 30 -Compress))
    $file = [IO.File]::Open($temporary, 'CreateNew', 'Write', 'None')
    try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
    if (Test-Path -LiteralPath $path) {
        # A real backup path also avoids Windows PowerShell converting a null
        # string argument into an invalid empty path for File.Replace.
        $backup = "$path.previous"
        if (Test-Path -LiteralPath $backup) { Assert-RegularPath $backup; Assert-TrustedAcl $backup; Remove-Item -LiteralPath $backup }
        [IO.File]::Replace($temporary, $path, $backup)
    }
    else { [IO.File]::Move($temporary, $path) }
}

function Read-InstallJson([string] $Name) {
    $path = Join-Path $script:Root $Name
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    Assert-RegularPath $path
    Assert-TrustedAcl $path
    if ((Get-Item -LiteralPath $path).Length -gt 1MB) { throw 'Installation metadata is too large.' }
    $value = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($value.schema -ne 1) { throw 'Unsupported installation metadata. Use the matching installer to repair.' }
    return $value
}

function Assert-PayloadMetadata([object] $Manifest, [switch] $Installed) {
    if ($Manifest.schema -ne 1 -or $Manifest.protocol -ne 1 -or $Manifest.storage -ne 1 -or
        $Manifest.id -cnotmatch '^[a-f0-9]{64}$' -or $Manifest.version -notmatch '^\d+\.\d+\.\d+(-[a-zA-Z0-9.-]+)?$') {
        throw 'Unsupported installer payload format.'
    }
    $names = @($script:PayloadNames)
    if ($Installed) { $names += 'Uninstall.exe' }
    if (@($Manifest.files).Count -ne $names.Count) { throw 'Unexpected payload inventory.' }
    foreach ($name in $names) {
        $entries = @($Manifest.files | Where-Object { $_.name -ceq $name })
        if ($entries.Count -ne 1 -or $entries[0].sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Invalid payload inventory.' }
    }
}

function Assert-Manifest([object] $Manifest, [string] $Directory, [switch] $Installed, [switch] $RecoveryOnly) {
    Assert-PayloadMetadata $Manifest -Installed:$Installed
    Assert-RegularPath $Directory $true
    if ($Installed) { Assert-ProtectedDirectory $Directory } else { Assert-ProtectedDirectory $Directory $false }
    foreach ($entry in $Manifest.files) {
        $name = $entry.name
        if ($RecoveryOnly -and $name -cne 'quad9-service.exe') { continue }
        $path = Join-Path $Directory $name
        Assert-RegularPath $path
        if ($Installed) { Assert-TrustedAcl $path }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine $entry.sha256) { throw "Installed payload checksum mismatch: $name" }
    }
}

function Get-VersionPath([object] $Manifest) {
    if ($Manifest.id -cnotmatch '^[a-f0-9]{64}$') { throw 'Invalid installation identifier.' }
    Assert-ProtectedDirectory (Join-Path $script:Root 'versions')
    return Join-Path (Join-Path $script:Root 'versions') $Manifest.id
}

function Get-CurrentManifest([object] $State, [switch] $RecoveryOnly) {
    if (-not $State.installed) { throw 'Quad9 Connect is not installed.' }
    Assert-ProtectedDirectory (Join-Path $script:Root 'versions')
    $versions = @($State.versions | Where-Object { $_.id -ceq $State.current })
    if ($versions.Count -ne 1) { throw 'The current installation is missing or ambiguous.' }
    Assert-Manifest $versions[0] (Get-VersionPath $versions[0]) -Installed -RecoveryOnly:$RecoveryOnly
    return $versions[0]
}

function Get-ServiceCommand([object] $Manifest) {
    return '"{0}" service' -f (Join-Path (Get-VersionPath $Manifest) 'quad9-service.exe')
}

function Assert-ServiceIdentity([object[]] $Manifests) {
    $service = Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'"
    if ($null -eq $service) { return $null }
    $commands = @($Manifests | ForEach-Object { Get-ServiceCommand $_ })
    if ($service.PathName -cnotin $commands -or $service.StartName -notin @('LocalSystem', 'NT AUTHORITY\SYSTEM')) {
        throw 'The existing service does not match this installation. Its configuration has been preserved.'
    }
    return $service
}

function Stop-InstalledService([object[]] $Manifests) {
    $identity = Assert-ServiceIdentity $Manifests
    if ($null -eq $identity) { return }
    $process = if ($identity.ProcessId) { Get-Process -Id $identity.ProcessId -ErrorAction SilentlyContinue } else { $null }
    $service = Get-Service -Name Quad9Connect
    try {
        if ($service.Status -ne 'Stopped') { Stop-Service -Name Quad9Connect; $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30)) }
        if ($process -and -not $process.WaitForExit(30000)) { throw 'The service has not exited. Recovery files and binaries have been retained.' }
    } finally {
        $service.Dispose()
        if ($process) { $process.Dispose() }
    }
}

function Set-ServiceVersion([object] $Manifest) {
    $command = Get-ServiceCommand $Manifest
    if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) {
        $result = Invoke-CimMethod -InputObject (Get-CimInstance Win32_Service -Filter "Name='Quad9Connect'") -MethodName Change -Arguments @{ PathName = $command; StartMode = 'Automatic' }
        if ($result.ReturnValue -ne 0) { throw "Cannot update service configuration: $($result.ReturnValue)" }
    } else {
        New-Service -Name Quad9Connect -DisplayName 'Quad9 Connect' -BinaryPathName $command -StartupType Automatic -Description 'Explicit encrypted DNS management and conflict-aware recovery.' | Out-Null
    }
}

function Start-InstalledService([object] $Manifest, [switch] $AllowRecovery) {
    $null = Assert-ServiceIdentity @($Manifest)
    Start-Service -Name Quad9Connect
    $service = Get-Service -Name Quad9Connect
    try { $service.WaitForStatus('Running', [TimeSpan]::FromSeconds(30)) } finally { $service.Dispose() }
    $binary = Join-Path (Get-VersionPath $Manifest) 'quad9-service.exe'
    $output = '{"version":1,"id":"installer-status","command":{"type":"get_status"}}' | & $binary request
    if ($LASTEXITCODE -ne 0) { throw 'The installed service is not responding. Repair is required.' }
    $response = $output | ConvertFrom-Json
    if ($response.result.type -ne 'status' -or ($response.result.recovery_blocked -and -not $AllowRecovery)) { throw 'Service recovery is incomplete. Installation has stopped.' }
}

function Invoke-Offline([object] $Manifest, [string] $Command) {
    & (Join-Path (Get-VersionPath $Manifest) 'quad9-service.exe') maintenance $Command
    if ($LASTEXITCODE -ne 0) { throw "Offline $Command failed. The service, binaries, and recovery records have been retained." }
}

function Set-MaintenanceGate {
    Assert-ProtectedDirectory $script:Data $false
    $path = Join-Path $script:Data 'installer-maintenance'
    if (Test-Path -LiteralPath $path) { Assert-RegularPath $path; Assert-TrustedAcl $path $false; return }
    $file = [IO.File]::Open($path, 'CreateNew', 'Write', 'None')
    try { $file.Flush($true) } finally { $file.Dispose() }
}

function Clear-MaintenanceGate {
    if (-not (Test-Path -LiteralPath $script:Data)) { return }
    Assert-ProtectedDirectory $script:Data $false
    $path = Join-Path $script:Data 'installer-maintenance'
    if (Test-Path -LiteralPath $path) { Assert-RegularPath $path; Assert-TrustedAcl $path $false; Remove-Item -LiteralPath $path }
}

function Get-Enrollment([string] $Sid) {
    if ($Sid -notmatch '^S-1-5-21-\d+-\d+-\d+-\d+$') { throw 'Select a local user SID explicitly. Group SIDs and service identities are not operators.' }
    $user = Get-LocalUser -SID ([Security.Principal.SecurityIdentifier]$Sid)
    if (-not $user.Enabled) { throw 'The selected local user is disabled.' }
    $group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
    $member = $group -and @(Get-LocalGroupMember -Group $group | Where-Object { $_.SID.Value -eq $Sid }).Count -gt 0
    return [pscustomobject]@{ sid = $Sid; group_created = ($null -eq $group); member_added = (-not $member) }
}

function Add-Enrollment([object] $Enrollment) {
    $group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
    if ($null -eq $group) { $group = New-LocalGroup -Name Quad9ConnectOperators -Description 'Users authorized to control Quad9 DNS' }
    if (-not @(Get-LocalGroupMember -Group $group | Where-Object { $_.SID.Value -eq $Enrollment.sid }).Count) {
        $user = Get-LocalUser -SID ([Security.Principal.SecurityIdentifier]$Enrollment.sid)
        Add-LocalGroupMember -Group $group -Member $user
    }
}

function Undo-Enrollment([object] $Enrollment) {
    $group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
    if ($null -eq $group) { return }
    if ($Enrollment.member_added) {
        $member = @(Get-LocalGroupMember -Group $group | Where-Object { $_.SID.Value -eq $Enrollment.sid })
        if ($member.Count) { Remove-LocalGroupMember -Group $group -Member $member }
    }
    if ($Enrollment.group_created -and @(Get-LocalGroupMember -Group $group).Count -eq 0) { Remove-LocalGroup -InputObject $group }
}

function Publish-Installation([object] $State) {
    $manifest = Get-CurrentManifest $State
    $directory = Get-VersionPath $manifest
    New-Item -Path $script:Arp -Force | Out-Null
    $properties = @{
        DisplayName = 'Quad9 Connect (unsigned development build)'; DisplayVersion = $manifest.version
        Publisher = 'Quad9 Connect community contributors'; InstallLocation = $script:Root
        UninstallString = ('"{0}"' -f (Join-Path $directory 'Uninstall.exe'))
        # NSIS's self-copying uninstaller bootstrap does not propagate the child
        # exit code. Management tools need the synchronous administrator command.
        QuietUninstallString = ('"{0}" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{1}" -Operation Uninstall' -f
            (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'), (Join-Path $directory 'Manage-Quad9Installation.ps1'))
    }
    foreach ($name in $properties.Keys) { New-ItemProperty -Path $script:Arp -Name $name -Value $properties[$name] -PropertyType String -Force | Out-Null }
    foreach ($name in @('NoModify', 'NoRepair')) { New-ItemProperty -Path $script:Arp -Name $name -Value 1 -PropertyType DWord -Force | Out-Null }
    $menu = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Quad9 Connect.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($menu)
    $shortcut.TargetPath = Join-Path $directory 'quad9-desktop.exe'
    $shortcut.WorkingDirectory = $directory
    $shortcut.Save()
}

function Remove-ServiceRegistration {
    & "$env:SystemRoot\System32\sc.exe" delete Quad9Connect | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot remove the stopped service. Recovery binaries have been retained.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Service removal is pending. Close service management tools and run uninstall again.' }
        Start-Sleep -Milliseconds 100
    }
}

function Restore-PendingInstallation([object] $Pending) {
    $candidate = $Pending.candidate
    # Stopping an identified SCM service does not execute the candidate's files.
    # Quarantined desktop/uninstaller files must not block the old service.
    Assert-PayloadMetadata $candidate -Installed
    $previous = $Pending.previous
    $manifests = @($candidate)
    if ($previous -and $previous.installed) { $manifests += Get-CurrentManifest $previous -RecoveryOnly }
    Stop-InstalledService $manifests
    if ($previous -and $previous.installed) {
        $old = Get-CurrentManifest $previous -RecoveryOnly
        Set-ServiceVersion $old
        Start-InstalledService $old -AllowRecovery
        # Keep the rejected payload in the known inventory for a later uninstall.
        if ($candidate.id -notin @($previous.versions.id)) { $previous.versions = @($previous.versions) + $candidate }
        Write-AtomicJson 'installation.json' $previous
        try { Publish-Installation $previous } catch { Write-Warning 'The previous service is running. Reinstall to repair unrelated application files.' }
    } else {
        if (Assert-ServiceIdentity @($candidate)) {
            Assert-Manifest $candidate (Get-VersionPath $candidate) -Installed -RecoveryOnly
            try { Invoke-Offline $candidate 'restore' }
            catch { Set-ServiceVersion $candidate; Start-Service -Name Quad9Connect; throw }
            Remove-ServiceRegistration
        }
        Undo-Enrollment $Pending.enrollment
        $removed = [pscustomobject]@{ schema = 1; installed = $false; current = $candidate.id; versions = @($candidate); enrollment = $Pending.enrollment }
        Write-AtomicJson 'installation.json' $removed
    }
    Clear-MaintenanceGate
    Remove-Item -LiteralPath (Join-Path $script:Root 'pending.json')
}

function Resolve-StagedCandidate([object] $Staging, [object] $State) {
    $candidate = $Staging.candidate
    Assert-PayloadMetadata $candidate -Installed
    if ($State -and @($State.versions | Where-Object { $_.id -ceq $candidate.id }).Count) {
        # Pending recovery already placed it in the known cleanup inventory.
        Remove-Item -LiteralPath (Join-Path $script:Root 'staging.json')
        return
    }
    $allowed = @($candidate)
    if ($State -and $State.installed) { $allowed += Get-CurrentManifest $State -RecoveryOnly }
    $service = Assert-ServiceIdentity $allowed
    if ($service -and $service.PathName -ceq (Get-ServiceCommand $candidate)) { throw 'A staged candidate is registered without replacement intent. Preserve it for administrator recovery.' }
    $source = Get-VersionPath $candidate
    if (Test-Path -LiteralPath $source) {
        Assert-RegularPath $source $true
        Assert-TrustedAcl $source
        $retained = Join-Path $script:Root 'retained-staging'
        if (-not (Test-Path -LiteralPath $retained)) { New-ProtectedDirectory $retained }
        Assert-ProtectedDirectory $retained
        $destination = Join-Path $retained ($candidate.id + '-' + [guid]::NewGuid().ToString('N'))
        # Both absolute paths are fixed descendants of the protected install root.
        # Directory.Move is a same-volume rename and never traverses nested links.
        if ([IO.Path]::GetFullPath($source) -ine (Join-Path (Join-Path $script:Root 'versions') $candidate.id) -or
            (Split-Path ([IO.Path]::GetFullPath($destination)) -Parent) -ine $retained) { throw 'Unexpected staging recovery scope.' }
        [IO.Directory]::Move($source, $destination)
    }
    Remove-Item -LiteralPath (Join-Path $script:Root 'staging.json')
}

function Test-WebView2 {
    $client = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($path in @("HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$client", "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$client")) {
        $entry = Get-ItemProperty -LiteralPath $path -ErrorAction SilentlyContinue
        if ($entry -and $entry.PSObject.Properties['pv'] -and $entry.pv -and [version]$entry.pv -gt [version]'0.0.0.0') { return }
    }
    throw 'Install the Microsoft Edge WebView2 Evergreen Runtime for all users, then run this installer again: https://developer.microsoft.com/microsoft-edge/webview2/'
}

function New-InstallationState([object] $Manifest, [object] $State, [object] $Enrollment) {
    # Do not assign an empty array from an if-expression's output: PowerShell
    # collapses it to null, which would insert a null version in the inventory.
    $versions = @()
    if ($State) { $versions = @($State.versions | Where-Object { $_.id -cne $Manifest.id }) }
    return [pscustomobject]@{ schema = 1; installed = $true; current = $Manifest.id; versions = @($versions) + $Manifest; enrollment = $Enrollment }
}

function Assert-InterfaceClosed([object] $State) {
    $paths = @($State.versions | ForEach-Object { Join-Path (Get-VersionPath $_) 'quad9-desktop.exe' })
    foreach ($process in Get-CimInstance Win32_Process -Filter "Name='quad9-desktop.exe'") {
        if (-not $process.ExecutablePath -or $process.ExecutablePath -iin $paths) {
            throw 'Quit Quad9 Connect from its tray menu, then retry setup. The service and DNS settings have been preserved.'
        }
    }
}

function Install-Quad9Payload([string] $Payload, [string] $OperatorSid, [object] $State) {
    Test-WebView2
    $manifestPath = Join-Path $Payload 'payload.json'
    Assert-RegularPath $manifestPath
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-Manifest $manifest $Payload
    $previous = if ($State -and $State.installed) { $State } else { $null }
    if ($previous) {
        Assert-InterfaceClosed $previous
        $old = Get-CurrentManifest $previous
        if ([version]($manifest.version.Split('-')[0]) -lt [version]($old.version.Split('-')[0])) { throw 'Downgrades require the matching recovery procedure.' }
        $null = Assert-ServiceIdentity @($old)
        if (-not (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue)) { throw 'The service is missing. Run Repair before upgrading.' }
        if ($OperatorSid -and $OperatorSid -ne $previous.enrollment.sid) { throw 'Upgrades preserve operator access. Change membership separately as an administrator.' }
        $enrollment = $previous.enrollment
    } else {
        if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) { throw 'An unmanaged Quad9 service already exists. Restore and remove it before installing.' }
        $enrollment = Get-Enrollment $OperatorSid
    }
    $versionsRoot = Join-Path $script:Root 'versions'
    if (-not (Test-Path -LiteralPath $versionsRoot)) { New-ProtectedDirectory $versionsRoot }
    Assert-ProtectedDirectory $versionsRoot
    $destination = Get-VersionPath $manifest
    if (Test-Path -LiteralPath $destination) {
        # Reinstalling an identical payload uses the previously verified inventory.
        $known = @($State.versions | Where-Object { $_.id -ceq $manifest.id })
        if ($known.Count -ne 1) { throw 'An unknown staged payload exists. Inspect it before repair.' }
        $manifest = $known[0]
        Assert-Manifest $manifest $destination -Installed
    } else {
        $uninstaller = Join-Path $Payload 'Uninstall.exe'
        Assert-RegularPath $uninstaller
        $manifest.files = @($manifest.files) + [pscustomobject]@{ name = 'Uninstall.exe'; sha256 = (Get-FileHash -LiteralPath $uninstaller).Hash }
        # Persist ownership before creating or partially copying the directory.
        Write-AtomicJson 'staging.json' ([pscustomobject]@{ schema = 1; candidate = $manifest })
        New-ProtectedDirectory $destination
        foreach ($name in $script:PayloadNames) { Copy-Item -LiteralPath (Join-Path $Payload $name) -Destination (Join-Path $destination $name) }
        Copy-Item -LiteralPath $uninstaller -Destination (Join-Path $destination 'Uninstall.exe')
        Assert-Manifest $manifest $destination -Installed
    }
    $pending = [pscustomobject]@{ schema = 1; candidate = $manifest; previous = $previous; enrollment = $enrollment }
    Write-AtomicJson 'pending.json' $pending
    if (Test-Path -LiteralPath (Join-Path $script:Root 'staging.json')) { Remove-Item -LiteralPath (Join-Path $script:Root 'staging.json') }
    try {
        if ($previous) { Set-MaintenanceGate; Stop-InstalledService @($old) }
        # This executable reads only the fixed protected store and rejects unknown formats.
        Invoke-Offline $manifest 'check'
        Set-MaintenanceGate
        if (-not $previous) { Add-Enrollment $enrollment }
        Set-ServiceVersion $manifest
        Start-InstalledService $manifest
        $next = New-InstallationState $manifest $State $enrollment
        Write-AtomicJson 'installation.json' $next
        Publish-Installation $next
        Clear-MaintenanceGate
        Remove-Item -LiteralPath (Join-Path $script:Root 'pending.json')
    } catch {
        $failure = $_
        try { Restore-PendingInstallation $pending } catch { throw "Installation failed and automatic rollback could not finish. Run the retained administrator Repair command. $failure $_" }
        throw $failure
    }
}

function Uninstall-Quad9Payload([object] $State) {
    # Only the current, authenticated recovery executable is required to restore.
    # Damaged obsolete versions must not prevent restoration of active DNS.
    $manifest = Get-CurrentManifest $State -RecoveryOnly
    Assert-InterfaceClosed $State
    $identity = Assert-ServiceIdentity @($manifest)
    Set-MaintenanceGate
    try {
        Stop-InstalledService @($manifest)
        Invoke-Offline $manifest 'restore'
    } catch {
        if ($identity) { Set-ServiceVersion $manifest; Start-Service -Name Quad9Connect }
        Clear-MaintenanceGate
        throw
    }
    if ($identity) { Remove-ServiceRegistration }
    # A durable removed state makes interrupted cleanup safe to resume.
    $State.installed = $false
    Write-AtomicJson 'installation.json' $State
    Clear-MaintenanceGate
    Complete-Quad9Removal $State
}

function Complete-Quad9Removal([object] $State) {
    if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) { throw 'A service still exists. Refusing file removal.' }
    if (Test-Path -LiteralPath $script:Data) { Assert-ProtectedDirectory $script:Data $false; Clear-MaintenanceGate }
    Undo-Enrollment $State.enrollment
    foreach ($version in $State.versions) {
        $directory = Get-VersionPath $version
        if (-not (Test-Path -LiteralPath $directory)) { continue }
        try { Assert-ProtectedDirectory $directory } catch { Write-Warning 'An unsafe obsolete directory was preserved.'; continue }
        foreach ($entry in $version.files) {
            if ($entry.name -cnotin @($script:PayloadNames + 'Uninstall.exe')) { throw 'Unknown installed file.' }
            $path = Join-Path $directory $entry.name
            if (Test-Path -LiteralPath $path) {
                try { Assert-RegularPath $path; Assert-TrustedAcl $path } catch { Write-Warning 'An unsafe installed file was preserved.'; continue }
                if ((Get-FileHash -LiteralPath $path).Hash -ine $entry.sha256) { Write-Warning 'A changed installed file was preserved for inspection.'; continue }
                Remove-Item -LiteralPath $path
            }
        }
        if (@(Get-ChildItem -LiteralPath $directory -Force).Count -eq 0) { [IO.Directory]::Delete($directory, $false) }
    }
    if (Test-Path -LiteralPath $script:Arp) { Remove-Item -LiteralPath $script:Arp }
    $menu = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Quad9 Connect.lnk'
    if (Test-Path -LiteralPath $menu) { Assert-RegularPath $menu; Remove-Item -LiteralPath $menu }
    Write-Output 'Quad9 Connect was removed after DNS restoration. Protected recovery and installation metadata remain available for inspection.'
}

function Repair-Quad9Payload([object] $State) {
    $manifest = Get-CurrentManifest $State -RecoveryOnly
    $null = Assert-ServiceIdentity @($manifest)
    Set-MaintenanceGate
    try { Stop-InstalledService @($manifest); Invoke-Offline $manifest 'restore' }
    finally {
        $null = Assert-ServiceIdentity @($manifest)
        Set-ServiceVersion $manifest
        Start-Service -Name Quad9Connect
        Clear-MaintenanceGate
    }
    Publish-Installation $State
}

function Invoke-Quad9Lifecycle([string] $Operation, [string] $Payload, [string] $OperatorSid) {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this command as an administrator.' }
    if (-not [Environment]::Is64BitProcess -or [Environment]::OSVersion.Version.Build -lt 22000) { throw 'This installer requires 64-bit Windows 11 or a compatible test VM.' }
    if (-not (Test-Path -LiteralPath $script:Root)) {
        if ($Operation -ne 'Install') { throw 'Quad9 Connect is not installed.' }
        New-ProtectedDirectory $script:Root
    }
    Assert-ProtectedDirectory $script:Root
    $lockPath = Join-Path $script:Root 'installation.lock'
    if (Test-Path -LiteralPath $lockPath) { Assert-RegularPath $lockPath; Assert-TrustedAcl $lockPath }
    $lock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
    try {
        $pending = Read-InstallJson 'pending.json'
        if ($pending) { Restore-PendingInstallation $pending }
        $state = Read-InstallJson 'installation.json'
        $staging = Read-InstallJson 'staging.json'
        if ($staging) { Resolve-StagedCandidate $staging $state }
        if ($Operation -eq 'Install' -and $state -and -not $state.installed) {
            Complete-Quad9Removal $state
            $state = $null
        }
        switch ($Operation) {
            'Install' { Install-Quad9Payload $Payload $OperatorSid $state }
            'Uninstall' {
                if ($null -eq $state) { throw 'Installation metadata is missing. Use the matching installer to repair.' }
                if ($state.installed) { Uninstall-Quad9Payload $state } else { Complete-Quad9Removal $state }
            }
            'Repair' { Repair-Quad9Payload $state }
            default { throw 'Unsupported lifecycle operation.' }
        }
    } finally { $lock.Dispose() }
}

Export-ModuleMember -Function Invoke-Quad9Lifecycle
