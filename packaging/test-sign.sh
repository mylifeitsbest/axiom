#!/bin/bash
# Проверка подписи обновлений без настоящего ключа (CI, Linux):
# packaging/sign-sums.sh подписывает одноразовым ключом в нужном формате, ключ
# не от той пары останавливает его, а открытый ключ в packaging/keys совпадает
# с зашитым в Axiom (src/update/sign.rs) и install.ps1.
set -euo pipefail
cd "$(dirname "$0")/.."

t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$t/key.pem"
openssl pkey -in "$t/key.pem" -pubout -out "$t/pub.pem"
printf '%064d  Axiom-windows-x64.exe\n' 1 > "$t/SHA256SUMS.txt"
UPDATE_SIGNING_PUB="$t/pub.pem" UPDATE_SIGNING_KEY="$(cat "$t/key.pem")" \
    sh packaging/sign-sums.sh 0.6.2-beta.7 "$t"
head -n1 "$t/SHA256SUMS.signed" | grep -qxE '[0-9a-f]{128}'
sed -n 2,4p "$t/SHA256SUMS.signed" | diff - <(printf 'axiom-update 1\nversion 0.6.2-beta.7\n%064d  Axiom-windows-x64.exe\n' 1)

# Ключ в секрете не от той пары — публикации нет.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$t/other.pem"
if UPDATE_SIGNING_PUB="$t/pub.pem" UPDATE_SIGNING_KEY="$(cat "$t/other.pem")" \
    sh packaging/sign-sums.sh 0.6.2 "$t" 2> /dev/null; then
    echo "подписал ключом не от той пары" >&2
    exit 1
fi
if UPDATE_SIGNING_KEY='' sh packaging/sign-sums.sh 0.6.2 "$t" 2> /dev/null; then
    echo "подписал без ключа" >&2
    exit 1
fi

# Один открытый ключ везде.
key=$(openssl pkey -pubin -in packaging/keys/update-signing.pem -outform DER | tail -c 64 | od -An -tx1 | tr -d ' \n')
grep -q "'$key'" install.ps1 || { echo "в install.ps1 другой ключ" >&2; exit 1; }
rust=$(sed -n '/^const KEYS/,/^\]\];/p' src/update/sign.rs | grep -oE '0x[0-9a-f]{2}' | cut -c3- | tr -d '\n')
[ "$rust" = "$key" ] || { echo "в src/update/sign.rs другой ключ" >&2; exit 1; }
echo "подпись обновлений: скрипт и ключи в порядке"
