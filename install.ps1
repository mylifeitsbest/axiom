# Установка Axiom на Windows одной командой (PowerShell):
#   irm https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.ps1 | iex
#
# Скачивает последний релиз, проверяет SHA256, ставит в %LOCALAPPDATA%\Programs\Axiom
# (без прав администратора) и запускает Axiom.
#
# Бета-версия (каждый коммит в main, до выхода версии):
#   $env:AXIOM_BETA=1; irm https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.ps1 | iex
# Дальше новые бета-версии Axiom предлагает сам.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$beta = $env:AXIOM_BETA -and $env:AXIOM_BETA -notin @('0', 'no', 'false')
$base = if ($beta) {
    'https://github.com/mylifeitsbest/axiom/releases/download/beta'
} else {
    'https://github.com/mylifeitsbest/axiom/releases/latest/download'
}
$name = 'Axiom-windows-x64.exe'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("axiom-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Host $(if ($beta) { 'Скачиваю бета-версию Axiom...' } else { 'Скачиваю Axiom...' })
    # Вместо страницы ошибки GitHub целиком — одна понятная строка.
    function Get-File($url, $out) {
        try { Invoke-WebRequest $url -OutFile $out -UseBasicParsing }
        catch {
            # На 404 GitHub (или прокси) иногда просто рвёт соединение — кода ответа нет.
            $code = [int]$_.Exception.Response.StatusCode
            if ($beta) { throw "Бета-версия не скачалась: её ещё нет (появляется минут через 20-30 после изменения) или нет связи с GitHub. Попробуйте позже. ($($_.Exception.Message))" }
            if ($code -eq 404) { throw "Файл не найден на GitHub: $url" }
            throw "Не удалось скачать $url — проверьте интернет или прокси. ($($_.Exception.Message))"
        }
    }
    Get-File "$base/$name" "$tmp\$name"
    Get-File "$base/SHA256SUMS.txt" "$tmp\SHA256SUMS.txt"

    $line = Get-Content "$tmp\SHA256SUMS.txt" | Where-Object { $_ -match ("\s" + [regex]::Escape($name) + '$') }
    $expected = ("$line" -split '\s+')[0]
    $actual = (Get-FileHash "$tmp\$name" -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) {
        $hint = if ($beta) { ' Если сборка как раз обновлялась, запустите установку ещё раз через пару минут.' } else { '' }
        throw "Контрольная сумма не совпала: файл повреждён или подменён.$hint"
    }

    Write-Host 'Устанавливаю...'
    Start-Process -FilePath "$tmp\$name" -ArgumentList '--install' -Wait
    $exe = Join-Path $env:LOCALAPPDATA 'Programs\Axiom\axiom.exe'
    if (-not (Test-Path $exe) -or (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower() -ne $actual) {
        throw 'Не удалось заменить axiom.exe. Закройте Axiom и запустите установку ещё раз.'
    }
    Write-Host "Готово: $exe"
    Write-Host 'Что Axiom отправляет сам и как это выключить: https://github.com/mylifeitsbest/axiom/blob/main/PRIVACY.md'
    Start-Process $exe
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
