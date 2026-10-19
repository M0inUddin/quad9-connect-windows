#requires -RunAsAdministrator
param(
    [Parameter(Mandatory)][string] $BinaryPath,
    [Parameter(Mandatory)][string] $OperatorAccount,
    [switch] $ValidationVm
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($ValidationVm -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted')) {
    throw 'Virtual-adapter validation mode is limited to disposable GitHub-hosted VMs.'
}
if (Get-Service -Name Quad9Connect -ErrorAction SilentlyContinue) {
    throw 'The service already exists. Restore DNS and uninstall it before replacing this development build.'
}
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'The service binary does not exist.' }
$operatorSid = ([Security.Principal.NTAccount]$OperatorAccount).Translate([Security.Principal.SecurityIdentifier])
$group = Get-LocalGroup -Name Quad9ConnectOperators -ErrorAction SilentlyContinue
if ($null -eq $group) { $group = New-LocalGroup -Name Quad9ConnectOperators -Description 'Users authorized to control Quad9 DNS management' }
if (-not (Get-LocalGroupMember -Group $group | Where-Object { $_.SID -eq $operatorSid })) {
    Add-LocalGroupMember -Group $group -Member $OperatorAccount
}

$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect'
if (Test-Path -LiteralPath $installRoot) {
    $owner = (Get-Acl -LiteralPath $installRoot).GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($owner -notin @('S-1-5-18', 'S-1-5-32-544')) { throw 'The existing install directory has an untrusted owner.' }
    if ((Get-Item -LiteralPath $installRoot).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'The install directory must not be a reparse point.' }
} else {
    New-Item -ItemType Directory -Path $installRoot | Out-Null
}
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]$sid,
        [Security.AccessControl.FileSystemRights]::FullControl, $inherit, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow))
}
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]'S-1-5-32-545',
    [Security.AccessControl.FileSystemRights]::ReadAndExecute, $inherit, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow))
Set-Acl -LiteralPath $installRoot -AclObject $acl
$destination = Join-Path $installRoot 'quad9-service.exe'
Copy-Item -LiteralPath $binary -Destination $destination
$mode = if ($ValidationVm) { 'service-vm' } else { 'service' }
New-Service -Name Quad9Connect -DisplayName 'Quad9 Connect development service' -StartupType Automatic -BinaryPathName ('"{0}" {1}' -f $destination, $mode) -Description 'Manages explicit Quad9 DNS settings and conflict-aware recovery.' | Out-Null
Start-Service -Name Quad9Connect
(Get-Service -Name Quad9Connect).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
Write-Output 'Development service installed. Existing user sessions need a new sign-in token to recognize newly added operator membership.'
Write-Output 'DNS management remains off until an authorized client explicitly enables it.'
