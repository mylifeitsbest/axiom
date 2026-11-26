# Установка Axiom на Windows одной командой (PowerShell):
#   irm https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.ps1 | iex
#
# Скачивает последний релиз, проверяет подпись и SHA256, ставит в
# %LOCALAPPDATA%\Programs\Axiom (без прав администратора) и запускает Axiom.
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
# Открытый ключ подписи обновлений (ECDSA P-256, X и Y): тот же, что зашит в Axiom
# (src/update/sign.rs) и лежит в packaging/keys/update-signing.pem.
$signingKey = '6b9376d18b5dda65cd48a2eeb70fa524aa8326322ab534dcee8aed6a0699f0f09cde64879757e8fbfe6d9dfc28366a673dcf9f7532dd85aa9427887f824f9466'
# 0.6.1 вышла до подписи: без неё принимаем только этот самый файл.
$unsignedLast = '7958a3894ccd346cb6c72b8f9c47c24e26b3b133770b1ff4bee40c4ddf6ff797'

function ConvertFrom-AxiomHex([string]$hex) {
    if ($hex -notmatch '^([0-9a-fA-F]{2})+$') { return $null }
    $bytes = New-Object byte[] ($hex.Length / 2)
    for ($i = 0; $i -lt $bytes.Length; $i++) { $bytes[$i] = [Convert]::ToByte($hex.Substring($i * 2, 2), 16) }
    , $bytes
}

# Сумма файла $file из SHA256SUMS.signed, если подпись верна (см. packaging/sign-sums.sh):
# первая строка — подпись r и s в hex, дальше подписанный текст. Иначе — исключение.
function Get-AxiomSignedSum([string]$path, [string]$file, [string]$keyHex) {
    $raw = [IO.File]::ReadAllBytes($path)
    $nl = [Array]::IndexOf($raw, [byte]10)
    if ($nl -lt 1) { throw 'Файл подписи пустой или обрезан.' }
    $sig = ConvertFrom-AxiomHex ([Text.Encoding]::ASCII.GetString($raw, 0, $nl).TrimEnd("`r"))
    $key = ConvertFrom-AxiomHex $keyHex
    if (-not $sig -or $sig.Length -ne 64 -or -not $key -or $key.Length -ne 64) { throw 'В файле подписи нет подписи.' }
    $body = New-Object byte[] ($raw.Length - $nl - 1)
    [Array]::Copy($raw, $nl + 1, $body, 0, $body.Length)
    # BCRYPT_ECCKEY_BLOB открытого ключа P-256: «ECS1», длина координаты 32, X, Y.
    [byte[]]$blob = [byte[]](0x45, 0x43, 0x53, 0x31, 0x20, 0, 0, 0) + $key
    $cng = [Security.Cryptography.CngKey]::Import($blob, [Security.Cryptography.CngKeyBlobFormat]::EccPublicBlob)
    $ecdsa = New-Object Security.Cryptography.ECDsaCng($cng)
    $ecdsa.HashAlgorithm = [Security.Cryptography.CngAlgorithm]::Sha256
    if (-not $ecdsa.VerifyData($body, $sig)) { throw 'Подпись не сошлась: файл подменён.' }
    $lines = [Text.Encoding]::UTF8.GetString($body) -split "`n"
    if ($lines[0] -cne 'axiom-update 1') { throw 'Незнакомый формат файла подписи.' }
    $line = $lines | Where-Object { $_ -match ('^[0-9a-f]{64}\s+\*?' + [regex]::Escape($file) + '$') } | Select-Object -First 1
    if (-not $line) { throw "В подписанных суммах нет $file." }
    ("$line" -split '\s+')[0]
}
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
    $actual = (Get-FileHash "$tmp\$name" -Algorithm SHA256).Hash.ToLower()
    # Сумма без подписи из того же релиза ничего не доказывает: подменивший exe
    # подменил бы и её. Поэтому верим только подписанной.
    $signed = $true
    try { Invoke-WebRequest "$base/SHA256SUMS.signed" -OutFile "$tmp\SHA256SUMS.signed" -UseBasicParsing }
    catch { $signed = $false }
    if ($signed) {
        $expected = Get-AxiomSignedSum "$tmp\SHA256SUMS.signed" $name $signingKey
    } elseif (-not $beta -and $actual -eq $unsignedLast) {
        $expected = $actual
    } else {
        $hint = if ($beta) { ' Если сборка как раз обновлялась, запустите установку ещё раз через пару минут.' } else { '' }
        throw "Нет подписи у скачанной версии: ставить её небезопасно.$hint"
    }
    if ($expected -ne $actual) {
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
    Start-Process $exe
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
