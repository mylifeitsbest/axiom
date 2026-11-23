#!/bin/sh
# Установка Axiom на Linux одной командой:
#   curl -fsSL https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.sh | sh
#
# Ubuntu, Debian, Kali — пакет .deb через apt; Arch и производные — пакет через pacman.
# Пакет скачивается из последнего релиза и проверяется по SHA256SUMS.txt.
set -eu

REPO="mylifeitsbest/axiom"
BASE="https://github.com/$REPO/releases/latest/download"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "этот скрипт для Linux. Для Windows: https://github.com/$REPO#установка"
[ "$(uname -m)" = "x86_64" ] || die "пока есть только сборка для x86_64"

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else
  command -v sudo >/dev/null 2>&1 || die "нужен sudo (или запустите скрипт от root)"
  SUDO="sudo"
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -q --show-progress -O "$2" "$1"
  else die "нужен curl или wget"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
chmod 755 "$tmp"

if command -v pacman >/dev/null 2>&1; then
  file="axiom-x86_64.pkg.tar.zst"
elif command -v apt-get >/dev/null 2>&1; then
  file="axiom_amd64.deb"
else
  die "не нашёл apt или pacman. Возьмите архив axiom-linux-x86_64.tar.gz: https://github.com/$REPO/releases/latest"
fi

say "Скачиваю $file…"
fetch "$BASE/$file" "$tmp/$file"
fetch "$BASE/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt"

say "Проверяю контрольную сумму…"
(cd "$tmp" && grep " $file\$" SHA256SUMS.txt | sha256sum -c -) || die "контрольная сумма не совпала"
chmod 644 "$tmp/$file"

say "Устанавливаю…"
if [ "$file" = "axiom_amd64.deb" ]; then
  $SUDO apt-get install -y "$tmp/$file"
else
  $SUDO pacman -U --noconfirm "$tmp/$file"
fi

say "Готово. Axiom есть в меню приложений, запуск из терминала: axiom"
