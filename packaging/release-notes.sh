#!/bin/sh
# Описание релиза: что нового в версии.
#   sh packaging/release-notes.sh vX.Y.Z [коммит] > notes.md
# Текст — раздел «## X.Y.Z» из CHANGELOG.md рабочей копии. Раздела нет — заголовки коммитов
# после прошлой версии (без AUR, CI, README и коммита с номером версии).
# В конце — ссылки на все изменения, установку и PRIVACY.md.
set -eu

TAG=$1
VER=${TAG#v}
if [ $# -ge 2 ]; then
    REF=$2
elif git rev-parse -q --verify "refs/tags/$TAG" > /dev/null; then
    REF=$TAG
else
    REF=HEAD
fi
REPO=${GITHUB_REPOSITORY:-mylifeitsbest/axiom}

# Прошлая версия — предыдущий тег vX.Y.Z по номеру (сам тег может ещё не существовать).
PREV=$( { git tag --list 'v*'; echo "$TAG"; } | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
    | sort -uV | awk -v t="$TAG" '$0 == t { print p; exit } { p = $0 }')
# Старые теги не всегда предки (история переписывалась) — тогда сравнивать не с чем.
if [ -n "$PREV" ] && ! git merge-base --is-ancestor "$PREV" "$REF" 2> /dev/null; then
    PREV=
fi

# Раздел версии без пустых строк в начале и в конце.
section=$(tr -d '\r' < CHANGELOG.md | awk -v v="$VER" '
    /^## / { if (on) exit; on = ($2 == v); next }
    on' | sed '/./,$!d')

if [ -n "$section" ]; then
    printf '%s\n' "$section"
else
    echo "## Что нового"
    echo
    git log --no-merges --format='- %s' "${PREV:+$PREV..}$REF" \
        | grep -vE '^- (AUR|CI|README|Axiom [0-9]+\.[0-9]+\.[0-9]+$)' || true
fi

echo
if [ -n "$PREV" ]; then
    echo "**Все изменения:** https://github.com/$REPO/compare/$PREV...$TAG"
else
    echo "**Все изменения:** https://github.com/$REPO/commits/$TAG"
fi
echo
echo "Как установить или обновить — в [README](https://github.com/$REPO#установка)."
echo
echo "[Политика конфиденциальности](https://github.com/$REPO/blob/main/PRIVACY.md)"
