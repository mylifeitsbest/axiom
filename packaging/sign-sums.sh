#!/bin/sh
# Подписывает SHA256SUMS.txt релиза или беты: рядом появляется SHA256SUMS.signed.
#
#   UPDATE_SIGNING_KEY="$(cat key.pem)" sh packaging/sign-sums.sh <версия> <папка>
#
# Формат SHA256SUMS.signed (его проверяют обновление Axiom на Windows — src/update/sign.rs —
# и install.ps1):
#   строка 1 — подпись ECDSA P-256 / SHA-256, r‖s в hex (128 знаков);
#   дальше — подписанный текст: «axiom-update 1», «version <версия>», строки SHA256SUMS.txt.
# Версия внутри подписи: старую подписанную сборку не выдать за новую.
#
# Закрытый ключ — только в секрете GitHub Actions UPDATE_SIGNING_KEY. Открытый —
# packaging/keys/update-signing.pem (и он же зашит в Axiom и install.ps1): подпись
# сверяется с ним, так что ключ не от той пары сборку остановит.
set -eu

VERSION=${1:?версия}
DIR=${2:?папка с SHA256SUMS.txt}
PUB=${UPDATE_SIGNING_PUB:-$(dirname "$0")/keys/update-signing.pem}
if [ -z "${UPDATE_SIGNING_KEY:-}" ]; then
    echo "нет секрета UPDATE_SIGNING_KEY: подписать обновление нечем" >&2
    exit 1
fi
case $VERSION in
    *[!0-9A-Za-z.-]* | '') echo "странная версия: $VERSION" >&2; exit 1 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
umask 077
printf '%s\n' "$UPDATE_SIGNING_KEY" > "$tmp/key.pem"
{
    echo "axiom-update 1"
    echo "version $VERSION"
    cat "$DIR/SHA256SUMS.txt"
} > "$tmp/body"
openssl dgst -sha256 -sign "$tmp/key.pem" -out "$tmp/sig.der" "$tmp/body"
openssl dgst -sha256 -verify "$PUB" -signature "$tmp/sig.der" "$tmp/body" > /dev/null || {
    echo "подпись не сходится с $PUB: в секрете не тот ключ" >&2
    exit 1
}
# DER (SEQUENCE из двух INTEGER) → r‖s по 32 байта: такой формат ждут Windows CNG и .NET.
hex=$(python3 -c '
import sys
d = sys.stdin.buffer.read()
assert d[0] == 0x30, "не DER"
i = 2 if d[1] < 0x80 else 2 + (d[1] & 0x7F)
out = b""
for _ in range(2):
    assert d[i] == 0x02, "не INTEGER"
    n = d[i + 1]
    v = d[i + 2:i + 2 + n].lstrip(b"\0")
    assert len(v) <= 32, "не P-256"
    out += v.rjust(32, b"\0")
    i += 2 + n
print(out.hex())
' < "$tmp/sig.der")
{
    echo "$hex"
    cat "$tmp/body"
} > "$DIR/SHA256SUMS.signed"
echo "подписано: $DIR/SHA256SUMS.signed (версия $VERSION)"
