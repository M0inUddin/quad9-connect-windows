param([string] $Destination = (Join-Path $PSScriptRoot '..\artifacts\nsis'))
$ErrorActionPreference = 'Stop'
$directory = [IO.Path]::GetFullPath($Destination)
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$zip = Join-Path $directory 'nsis-3.13.zip'
if (-not (Test-Path -LiteralPath $zip)) {
    Invoke-WebRequest -Uri 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip' -OutFile $zip
}
if ((Get-FileHash -LiteralPath $zip).Hash -ne 'BA63DFFC4410EE89193E1CB5A41989991BD77C61068DA17E3156D136B7B0B3D8') { throw 'NSIS archive checksum mismatch.' }
$expanded = Join-Path $directory 'expanded'
if (-not (Test-Path -LiteralPath $expanded)) { Expand-Archive -LiteralPath $zip -DestinationPath $expanded }
$executable = Join-Path $expanded 'nsis-3.13\makensis.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Unexpected NSIS toolchain layout.' }
Write-Output $executable
