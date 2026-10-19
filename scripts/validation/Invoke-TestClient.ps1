param([string] $RequestFile, [string] $Mode = 'request')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $OutputEncoding
$binary = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Quad9Connect\quad9-service.exe'
if ($Mode -eq 'deny-storage') {
    $path = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Quad9Connect\unauthorized-check.tmp'
    try { [IO.File]::WriteAllText($path, 'denied'); throw 'Operator unexpectedly wrote protected recovery data.' }
    catch [UnauthorizedAccessException] { Write-Output 'storage-denied'; exit 0 }
}
if ($Mode -in @('oversized', 'malformed')) {
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', 'Quad9Connect.v1', [IO.Pipes.PipeAccessRights]0x00120083,
        [IO.Pipes.PipeOptions]::None, [Security.Principal.TokenImpersonationLevel]::Identification, [IO.HandleInheritability]::None)
    try {
        $pipe.Connect(2000)
        if ($Mode -eq 'oversized') {
            $header = [BitConverter]::GetBytes([uint32]65537)
            $pipe.Write($header, 0, 4)
            Write-Output 'oversized-sent'
            exit 0
        }
        $body = [Text.Encoding]::UTF8.GetBytes('{"version":1,"id":"invalid","command":{"type":"run_shell","path":"anything"}}')
        $header = [BitConverter]::GetBytes([uint32]$body.Length)
        $pipe.Write($header, 0, 4); $pipe.Write($body, 0, $body.Length)
        $reader = [IO.BinaryReader]::new($pipe)
        $size = $reader.ReadUInt32()
        if ($size -gt 65536) { throw 'Oversized response' }
        $payload = $reader.ReadBytes($size)
        $pipe.WriteByte(165)
        $response = [Text.Encoding]::UTF8.GetString($payload) | ConvertFrom-Json
        if ($response.result.type -ne 'error' -or $response.result.code -ne 'invalid_request') { throw 'Malformed command was not rejected.' }
        Write-Output 'malformed-rejected'
        exit 0
    } finally { $pipe.Dispose() }
}
Get-Content -LiteralPath $RequestFile -Raw | & $binary request
exit $LASTEXITCODE
