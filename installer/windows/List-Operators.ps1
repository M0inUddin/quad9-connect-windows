param([Parameter(Mandatory)][string] $OutputPath)
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
$lines = [Collections.Generic.List[string]]::new()
$lines.Add('[Users]')
$installed = $false
$statePath = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect\installation.json'
if ((Test-Path -LiteralPath $statePath -PathType Leaf) -and (Get-Item -LiteralPath $statePath).Length -le 1MB) {
    try {
        $state = [IO.File]::ReadAllText($statePath) | ConvertFrom-Json
        $installed = $state.schema -eq 1 -and $state.installed -is [bool] -and $state.installed
    } catch { $installed = $false }
}
$lines.Add('installed=' + [int]$installed)
$users = @(Get-LocalUser | Where-Object Enabled | Sort-Object Name)
$lines.Add('count=' + $users.Count)
for ($i = 0; $i -lt $users.Count; $i++) {
    $lines.Add("name$i=" + $env:COMPUTERNAME + '\' + $users[$i].Name)
    $lines.Add("sid$i=" + $users[$i].SID.Value)
}
[IO.File]::WriteAllLines($OutputPath, $lines, [Text.Encoding]::Unicode)
