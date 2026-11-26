# Checks the signature verification in install.ps1 against the test vector that
# packaging/sign-sums.sh produced (src/update/testdata): a valid file gives the exe
# sum, a tampered one, a wrong key or a missing signature throw. Runs in CI (windows).
#   powershell -NoProfile -File packaging/test-install.ps1
# ASCII only: Windows PowerShell 5.1 reads files without a BOM in the ANSI code page.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$text = [IO.File]::ReadAllText((Join-Path $root 'install.ps1'), [Text.Encoding]::UTF8)
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "install.ps1 does not parse: $($errors[0].Message)" }
$ast.FindAll({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] }, $true) |
    ForEach-Object { . ([scriptblock]::Create($_.Extent.Text)) }
$real = $ast.FindAll({ param($n) $n -is [Management.Automation.Language.AssignmentStatementAst] -and "$($n.Left)" -eq '$signingKey' }, $true) |
    Select-Object -First 1
$realKey = "$($real.Right)".Trim("'")

$data = Join-Path $root 'src\update\testdata'
$key = (Get-Content (Join-Path $data 'key.hex') -Raw).Trim()
$signed = Join-Path $data 'SHA256SUMS.signed'
$failed = 0
function Expect-Throw($what, [scriptblock]$block) {
    try { & $block; Write-Host "FAIL: $what did not throw"; $script:failed++ } catch { Write-Host "ok: $what" }
}

$sum = Get-AxiomSignedSum $signed 'Axiom-windows-x64.exe' $key
if ($sum -ne ('1' * 64)) { Write-Host "FAIL: valid file gave '$sum'"; $failed++ } else { Write-Host 'ok: valid file' }
Expect-Throw 'missing file name' { Get-AxiomSignedSum $signed 'other.exe' $key }
Expect-Throw 'real key on a test file' { Get-AxiomSignedSum $signed 'Axiom-windows-x64.exe' $realKey }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('axiom-test-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $bytes = [IO.File]::ReadAllBytes($signed)
    $at = [Text.Encoding]::ASCII.GetString($bytes).IndexOf('Axiom-windows-x64.exe') - 3
    $bytes[$at] = if ($bytes[$at] -eq 0x30) { 0x31 } else { 0x30 }
    [IO.File]::WriteAllBytes("$tmp\tampered", $bytes)
    Expect-Throw 'tampered sum' { Get-AxiomSignedSum "$tmp\tampered" 'Axiom-windows-x64.exe' $key }
    $body = [IO.File]::ReadAllBytes($signed)
    $nl = [Array]::IndexOf($body, [byte]10)
    [IO.File]::WriteAllBytes("$tmp\unsigned", $body[($nl + 1)..($body.Length - 1)])
    Expect-Throw 'no signature' { Get-AxiomSignedSum "$tmp\unsigned" 'Axiom-windows-x64.exe' $key }
    [IO.File]::WriteAllBytes("$tmp\empty", [byte[]]@())
    Expect-Throw 'empty file' { Get-AxiomSignedSum "$tmp\empty" 'Axiom-windows-x64.exe' $key }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
if ($realKey.Length -ne 128) { Write-Host 'FAIL: $signingKey not found in install.ps1'; $failed++ }
if ($failed) { throw "install.ps1 signature checks: $failed failed" }
Write-Host 'install.ps1 signature checks: all passed'
