param(
    [string] $ServiceBinary = (Join-Path $PSScriptRoot '..\target\release\quad9-service.exe'),
    [string] $DesktopBinary = (Join-Path $PSScriptRoot '..\apps\desktop\src-tauri\target\release\quad9-desktop.exe'),
    [string] $MakeNsis = (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'),
    [string] $OutputDirectory = (Join-Path $PSScriptRoot '..\artifacts\installer'),
    [ValidatePattern('^\d+\.\d+\.\d+(-[a-zA-Z0-9.-]+)?$')][string] $Version
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$source = Join-Path $repo 'installer\windows'
if (-not $Version) { $Version = (Get-Content -LiteralPath (Join-Path $repo 'apps\desktop\src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version }
if (-not (Test-Path -LiteralPath $MakeNsis -PathType Leaf)) { throw 'Install NSIS 3.13 and pass its makensis.exe path.' }
$nsisVersion = & $MakeNsis '/VERSION'
if ($nsisVersion -ne 'v3.13') { throw 'This build requires the tested NSIS 3.13 toolchain.' }
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$payload = Join-Path $outputRoot ('payload-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $payload | Out-Null
Copy-Item -LiteralPath $ServiceBinary -Destination (Join-Path $payload 'quad9-service.exe')
Copy-Item -LiteralPath $DesktopBinary -Destination (Join-Path $payload 'quad9-desktop.exe')
foreach ($name in @('InstallerLifecycle.psm1', 'Manage-Quad9Installation.ps1')) { Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $payload $name) }
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $payload 'LICENSE.txt')
$files = @(Get-ChildItem -LiteralPath $payload -File | Sort-Object Name | ForEach-Object { [ordered]@{ name = $_.Name; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash } })
$identity = [Text.Encoding]::UTF8.GetBytes(($files | ConvertTo-Json -Compress) + $version)
$sha = [Security.Cryptography.SHA256]::Create()
try { $id = -join ($sha.ComputeHash($identity) | ForEach-Object { $_.ToString('x2') }) } finally { $sha.Dispose() }
$manifest = [ordered]@{ schema = 1; protocol = 1; storage = 1; id = $id; version = $version; files = $files }
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $payload 'payload.json') -Encoding utf8
$output = Join-Path $outputRoot "Quad9Connect-$version-$($id.Substring(0,12))-unsigned-x64-setup.exe"
Push-Location $source
try { & $MakeNsis '/V3' ('/DPAYLOAD=' + $payload) ('/DVERSION=' + $version) ('/DOUTPUT=' + $output) 'Quad9Connect.nsi' | Write-Host }
finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw 'NSIS compilation failed.' }
$hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $([IO.Path]::GetFileName($output))" | Set-Content -LiteralPath "$output.sha256" -Encoding ascii
Write-Output $output
