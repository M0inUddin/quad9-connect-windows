# SPDX-License-Identifier: AGPL-3.0-or-later
# Real per-user tasks in an isolated namespace, with a harmless launch fixture.
# Test files replace protected payloads explicitly; no service or DNS access.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$module = Import-Module (Join-Path $repo 'installer/windows/InstallerLifecycle.psm1') -Force -PassThru
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('quad9-startup-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$prefix = 'Quad9Connect.TestStartup.' + [guid]::NewGuid().ToString('N') + '.'
try {
    & $module {
        param($Fixture, $Prefix)
        $script:Root = $Fixture
        $script:StartupTaskPrefix = $Prefix
        function Assert-ProtectedDirectory([string] $Path) { Assert-RegularPath $Path $true }
        function Assert-TrustedAcl {}
        function Must-Fail([scriptblock] $Action) {
            $failed = $false
            try { & $Action | Out-Null } catch { $failed = $true }
            if (-not $failed) { throw 'Expected a rejected startup operation.' }
        }
        New-Item -ItemType Directory -Path (Join-Path $Fixture 'versions') | Out-Null
        $fixtureCode = Join-Path $Fixture 'launch-fixture.cs'
        [IO.File]::WriteAllText($fixtureCode, 'using System; using System.IO; using System.Diagnostics; using System.Threading; class LaunchFixture { static void Main(string[] args) { File.WriteAllText(Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "startup-launched.txt"), string.Join(" ", args) + "|" + Environment.UserName + "|" + Process.GetCurrentProcess().Id); Thread.Sleep(30000); } }')
        $fixtureBinary = Join-Path $Fixture 'launch-fixture.exe'
        & (Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319\csc.exe') /nologo /target:winexe "/out:$fixtureBinary" $fixtureCode
        if ($LASTEXITCODE -ne 0) { throw 'Could not build the harmless startup launch fixture.' }
        [IO.File]::WriteAllText((Join-Path $Fixture 'installation.lock'), '')
        $manifests = @(foreach ($id in @(('a' * 64), ('b' * 64))) {
            $directory = Join-Path $Fixture "versions\$id"
            New-Item -ItemType Directory -Path $directory | Out-Null
            $files = @(foreach ($name in @($script:PayloadNames + 'Uninstall.exe')) {
                if ($name -eq 'quad9-desktop.exe') { Copy-Item -LiteralPath $fixtureBinary -Destination (Join-Path $directory $name) }
                else { [IO.File]::WriteAllText((Join-Path $directory $name), 'test fixture; never execute') }
                [pscustomobject]@{ name = $name; sha256 = (Get-FileHash -LiteralPath (Join-Path $directory $name)).Hash }
            })
            [pscustomobject]@{ schema = 1; protocol = 1; storage = 1; version = '0.1.0-dev'; id = $id; files = $files }
        })
        $state = [pscustomobject]@{ schema = 1; installed = $true; current = $manifests[0].id; versions = $manifests }
        Write-AtomicJson 'installation.json' $state
        $desktop = Join-Path (Get-VersionPath $manifests[0]) 'quad9-desktop.exe'
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $name = $Prefix + $sid
        $scheduler = Open-StartupScheduler
        if ((Invoke-Quad9StartupPreference $desktop).mode -ne 'off' -or (Find-StartupTask $scheduler.Folder $name)) { throw 'Read registered an automatic startup task.' }
        if ((Invoke-Quad9StartupPreference $desktop -Mode tray -Write).mode -ne 'tray') { throw 'Tray preference was not registered.' }
        $task = $scheduler.Folder.GetTask($name)
        if ($task.Definition.Principal.RunLevel -ne 0 -or $task.Definition.Principal.LogonType -ne 3 -or
            $task.Definition.Settings.ExecutionTimeLimit -ne 'PT0S' -or
            $task.Definition.Actions.Item(1).Arguments -cne '--start-in-tray') { throw 'Startup credentials, arguments, or lifetime are wrong.' }
        $null = $task.Run($null)
        $marker = Join-Path (Split-Path $desktop -Parent) 'startup-launched.txt'
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        while (-not (Test-Path -LiteralPath $marker) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path -LiteralPath $marker)) { throw 'Windows did not launch the requested tray arguments as the current user.' }
        $launch = [IO.File]::ReadAllText($marker).Split('|')
        if ($launch.Count -ne 3 -or $launch[0] -cne '--start-in-tray' -or $launch[1] -cne [Environment]::UserName) { throw 'Windows launched different arguments or a different user.' }
        $running = [Diagnostics.Process]::GetProcessById([int]$launch[2])
        try {
            if ($running.MainModule.FileName -ine $desktop) { throw 'The launch fixture PID has an unexpected executable.' }
            $null = Invoke-Quad9StartupPreference $desktop -Mode off -Write
            if ($running.HasExited) { throw 'Turning startup off terminated the current interface.' }
        } finally {
            if (-not $running.HasExited) { $running.Kill(); $running.WaitForExit() }
            $running.Dispose()
        }
        $null = Invoke-Quad9StartupPreference $desktop -Mode tray -Write
        $task = $scheduler.Folder.GetTask($name)
        $task.Enabled = $false
        if ((Invoke-Quad9StartupPreference $desktop).mode -ne 'off') { throw 'Disabled Windows task was reported as active.' }
        $state.current = $manifests[1].id
        Write-AtomicJson 'installation.json' $state
        Sync-Quad9StartupTasks $state
        $newDesktop = Join-Path (Get-VersionPath $manifests[1]) 'quad9-desktop.exe'
        $task = $scheduler.Folder.GetTask($name)
        if ($task.Enabled -or $task.Definition.Actions.Item(1).Path -ine $newDesktop -or $task.Definition.Actions.Item(1).Arguments -cne '--start-in-tray') { throw 'Upgrade lost the disabled state, mode, or new path.' }
        if ((Invoke-Quad9StartupPreference $desktop).available) { throw 'An obsolete executable can change startup.' }
        $updated = Invoke-Quad9StartupPreference $newDesktop -Mode window -Write
        if ($updated.mode -ne 'window') { throw "User could not change startup after its task was updated: $($updated | ConvertTo-Json -Compress); enabled=$($scheduler.Folder.GetTask($name).Enabled); args='$($scheduler.Folder.GetTask($name).Definition.Actions.Item(1).Arguments)'" }
        $held = [IO.File]::Open((Join-Path $Fixture 'installation.lock'), 'Open', 'ReadWrite', 'None')
        try { Must-Fail { Invoke-Quad9StartupPreference $newDesktop -Mode off -Write } }
        finally { $held.Dispose() }
        if ((Invoke-Quad9StartupPreference $newDesktop).mode -ne 'window') { throw 'Installation lock changed the preference.' }
        $task = $scheduler.Folder.GetTask($name)
        $definition = $task.Definition
        $definition.RegistrationInfo.Description = 'external task fixture'
        $null = $scheduler.Folder.RegisterTaskDefinition($name, $definition, 4, $sid, $null, 3)
        Must-Fail { Invoke-Quad9StartupPreference $newDesktop -Mode off -Write }
        Sync-Quad9StartupTasks $state -Remove
        if (-not (Find-StartupTask $scheduler.Folder $name)) { throw 'Removal deleted an externally changed task.' }
        $definition.RegistrationInfo.Description = 'Quad9 Connect interface startup v1'
        $null = $scheduler.Folder.RegisterTaskDefinition($name, $definition, 4, $sid, $null, 3)
        $state.current = $manifests[0].id
        Write-AtomicJson 'installation.json' $state
        Sync-Quad9StartupTasks $state
        if ($scheduler.Folder.GetTask($name).Definition.Actions.Item(1).Path -ine $desktop) { throw 'Rollback did not restore startup to the previous executable.' }
        Sync-Quad9StartupTasks $state -Remove
        if (Find-StartupTask $scheduler.Folder $name) { throw 'Uninstall retained an owned startup task.' }
        if ((Invoke-Quad9StartupPreference $desktop -Mode tray -Write).mode -ne 'tray') { throw 'Re-registration failed.' }
        if ((Invoke-Quad9StartupPreference $desktop -Mode off -Write).mode -ne 'off' -or (Find-StartupTask $scheduler.Folder $name)) { throw 'Off did not remove the task.' }
    } $fixture $prefix
    Write-Output 'Windows startup passed: default off, least privilege, real current-user launch, tray/window/off, disabled state, upgrade/rollback, lock exclusion, foreign-task preservation, and removal.'
} finally {
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    foreach ($task in $folder.GetTasks(1)) {
        if ($task.Name.StartsWith($prefix, [StringComparison]::Ordinal)) {
            try { $task.Stop(0) } catch { }
            $folder.DeleteTask($task.Name, 0)
        }
    }
    $resolved = [IO.Path]::GetFullPath($fixture)
    if (-not $resolved.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path $resolved -Leaf) -notlike 'quad9-startup-test-*') { throw 'Unsafe test fixture cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
