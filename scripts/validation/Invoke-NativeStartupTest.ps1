# SPDX-License-Identifier: AGPL-3.0-or-later
# Runs inside the disposable installer's fresh standard-user token.
param([Parameter(Mandatory)][string] $Node, [Parameter(Mandatory)][string] $Desktop, [Parameter(Mandatory)][string] $Output)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$env:TEMP = Join-Path $Output 'temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Path $env:TEMP -Force | Out-Null
Push-Location (Join-Path $repo 'apps/desktop')
try {
    & $Node scripts/native-tray-startup.mjs $Desktop $Output --installed-startup
    exit $LASTEXITCODE
} finally { Pop-Location }
