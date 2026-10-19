# SPDX-License-Identifier: AGPL-3.0-or-later
# Real short-lived child/grandchild processes, with no DNS or machine changes.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'ResourceJob.cs')
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ('quad9-resource-test-' + [guid]::NewGuid())
[IO.Directory]::CreateDirectory($testDirectory) | Out-Null
$escapedDirectory = $testDirectory.Replace("'", "''")
$burn = '$clock = [Diagnostics.Stopwatch]::StartNew(); while ($clock.ElapsedMilliseconds -lt 350) { [Math]::Sqrt(12345) | Out-Null }'
$burnEncoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($burn))
$middleEncoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("$burn; & powershell.exe -NoProfile -NonInteractive -EncodedCommand $burnEncoded"))
$script = @"
Set-Content -LiteralPath '$escapedDirectory\ready' -Value ready
while (-not (Test-Path -LiteralPath '$escapedDirectory\go')) { Start-Sleep -Milliseconds 20 }
& powershell.exe -NoProfile -NonInteractive -EncodedCommand $middleEncoded
& powershell.exe -NoProfile -NonInteractive -EncodedCommand $burnEncoded
Set-Content -LiteralPath '$escapedDirectory\done' -Value done
Start-Sleep -Seconds 60
"@
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
$name = 'Local\Quad9ResourceTest-' + [guid]::NewGuid()
$owner = [Quad9ResourceJob]::new((Get-Process -Id $PID).Path, "-NoProfile -NonInteractive -EncodedCommand $encoded", $name)
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath (Join-Path $testDirectory 'ready'))) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Test host did not start.' }
        Start-Sleep -Milliseconds 20
    }
    $before = [Quad9ResourceJob]::Read($name)
    if (-not [Quad9ResourceJob]::Contains($name, $owner.ProcessId) -or [Quad9ResourceJob]::Contains($name, $PID)) { throw 'Membership must distinguish the test tree from its caller.' }
    Set-Content -LiteralPath (Join-Path $testDirectory 'go') -Value go
    # Intentionally do not observe either child while it runs.
    while (-not (Test-Path -LiteralPath (Join-Path $testDirectory 'done'))) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Test children did not complete.' }
        Start-Sleep -Milliseconds 20
    }
    $after = [Quad9ResourceJob]::Read($name)
    if ($after.TotalProcesses -lt $before.TotalProcesses + 3) { throw 'Exited children or grandchildren were omitted.' }
    if ($after.ProcessIds.Length -ne 1 -or $after.ProcessIds[0] -ne $owner.ProcessId) { throw 'Test children must have exited before the final sample.' }
    if ($after.CpuTicks - $before.CpuTicks -lt 2000000) { throw 'Final child CPU counters were omitted.' }
    $collisionRefused = $false
    try { $duplicate = [Quad9ResourceJob]::new((Get-Process -Id $PID).Path, '', $name); $duplicate.Dispose() } catch { $collisionRefused = $true }
    if (-not $collisionRefused -or ([Quad9ResourceJob]::Read($name)).ProcessIds[0] -ne $owner.ProcessId) { throw 'Job collision must preserve the existing owner.' }
    $missingRefused = $false
    try { [Quad9ResourceJob]::Read($name + '-missing') | Out-Null } catch { $missingRefused = $true }
    if (-not $missingRefused) { throw 'Missing jobs must not silently fall back to partial counters.' }
    Write-Output ('Resource accounting passed: {0} exited children, {1:N3} CPU seconds preserved, collision and missing-job rejection.' -f ($after.TotalProcesses - $before.TotalProcesses), (($after.CpuTicks - $before.CpuTicks) / 10000000.0))
} finally {
    $owner.Dispose()
    # Keep the tiny fixture files for inspection; no recursive cleanup.
}
