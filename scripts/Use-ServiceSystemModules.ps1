# SPDX-License-Identifier: AGPL-3.0-or-later
# Process-local module discovery for elevated development service scripts.
# PowerShell 7 also needs Windows' built-in LocalAccounts module directory.
$quad9ServiceModuleRoots = @(
    (Join-Path $PSHOME 'Modules'),
    (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules')
)
$env:PSModulePath = [string]::Join([IO.Path]::PathSeparator, $quad9ServiceModuleRoots)
