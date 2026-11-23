# Установка Axiom на Windows одной командой (PowerShell):
#   irm https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.ps1 | iex
#
# Скачивает последний релиз, проверяет SHA256, ставит в %LOCALAPPDATA%\Programs\Axiom
# (без прав администратора) и запускает Axiom.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$base = 'https://github.com/mylifeitsbest/axiom/releases/latest/download'
$name = 'Axiom-windows-x64.exe'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("axiom-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Host 'Скачиваю Axiom...'
    Invoke-WebRequest "$base/$name" -OutFile "$tmp\$name" -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS.txt" -OutFile "$tmp\SHA256SUMS.txt" -UseBasicParsing

    $line = Get-Content "$tmp\SHA256SUMS.txt" | Where-Object { $_ -match ("\s" + [regex]::Escape($name) + '$') }
    $expected = ("$line" -split '\s+')[0]
    $actual = (Get-FileHash "$tmp\$name" -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) { throw 'Контрольная сумма не совпала: файл повреждён или подменён.' }

    Write-Host 'Устанавливаю...'
    Start-Process -FilePath "$tmp\$name" -ArgumentList '--install' -Wait
    $exe = Join-Path $env:LOCALAPPDATA 'Programs\Axiom\axiom.exe'
    if (-not (Test-Path $exe) -or (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower() -ne $actual) {
        throw 'Не удалось заменить axiom.exe. Закройте Axiom и запустите установку ещё раз.'
    }
    Write-Host "Готово: $exe"
    Start-Process $exe
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
