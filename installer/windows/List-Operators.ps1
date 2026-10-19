param([Parameter(Mandatory)][string] $OutputPath)
$ErrorActionPreference = 'Stop'
$lines = [Collections.Generic.List[string]]::new()
$lines.Add('[Users]')
$users = @(Get-LocalUser | Where-Object Enabled | Sort-Object Name)
$lines.Add('count=' + $users.Count)
for ($i = 0; $i -lt $users.Count; $i++) {
    $lines.Add("name$i=" + $env:COMPUTERNAME + '\' + $users[$i].Name)
    $lines.Add("sid$i=" + $users[$i].SID.Value)
}
[IO.File]::WriteAllLines($OutputPath, $lines, [Text.Encoding]::Unicode)
