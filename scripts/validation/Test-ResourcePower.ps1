# SPDX-License-Identifier: AGPL-3.0-or-later
# Real Windows thread state, including nested ownership and exception cleanup.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not ('Quad9ResourcePowerLease' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'ResourcePower.cs') }
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public static class Quad9ResourcePowerProbe {
    [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint flags);
}
'@
$continuous = [uint32]2147483648
$held = [uint32]2147483651
$initial = [Quad9ResourcePowerProbe]::SetThreadExecutionState($continuous)
if ($initial -eq 0) { throw 'Cannot establish the test thread baseline.' }
$outer = $null
$inner = $null
try {
    $outer = [Quad9ResourcePowerLease]::new()
    $actual = [Quad9ResourcePowerProbe]::SetThreadExecutionState($held)
    if (($actual -band 3) -ne 3) { throw 'The visible benchmark did not hold display and system requests.' }
    $inner = [Quad9ResourcePowerLease]::new()
    $inner.Dispose()
    $inner = $null
    $actual = [Quad9ResourcePowerProbe]::SetThreadExecutionState($held)
    if (($actual -band 3) -ne 3) { throw 'The nested lease cleared its outer owner.' }
    try { throw 'intentional benchmark failure' }
    catch { if ($_.Exception.Message -cne 'intentional benchmark failure') { throw } }
    finally { $outer.Dispose(); $outer.Dispose(); $outer = $null }
    $after = [Quad9ResourcePowerProbe]::SetThreadExecutionState($continuous)
    if ($after -ne $continuous) { throw 'Exception cleanup retained a benchmark power request.' }
    Write-Output 'Scoped benchmark power requests passed: acquisition, nested ownership, exception cleanup, and repeated disposal.'
} finally {
    if ($null -ne $inner) { $inner.Dispose() }
    if ($null -ne $outer) { $outer.Dispose() }
    [Quad9ResourcePowerProbe]::SetThreadExecutionState($initial -bor $continuous) | Out-Null
}
