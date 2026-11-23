#!/bin/sh
# Установка Axiom на Linux одной командой:
#   curl -fsSL https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.sh | sh
#
# Подключает подписанный репозиторий Axiom, поэтому обновления приходят вместе
# с системой: `sudo apt upgrade` или `sudo pacman -Syu`.
# Ubuntu, Debian, Kali — apt; Arch и производные — pacman.
set -eu

REPO_URL="https://mylifeitsbest.github.io/axiom"
KEY_ID="643C6DE601D4BBB661D5322AF2F379CD75CF5C03"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "этот скрипт для Linux. Для Windows: https://github.com/mylifeitsbest/axiom#установка"
[ "$(uname -m)" = "x86_64" ] || die "пока есть только сборка для x86_64"

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else
  command -v sudo >/dev/null 2>&1 || die "нужен sudo (или запустите скрипт от root)"
  SUDO="sudo"
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -q -O "$2" "$1"
  else die "нужен curl или wget"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
chmod 755 "$tmp"

if command -v pacman >/dev/null 2>&1; then
  say "Добавляю ключ подписи Axiom…"
  fetch "$REPO_URL/axiom.asc" "$tmp/axiom.asc"
  # Свежие контейнеры и некоторые сборки идут без локального ключа pacman.
  if ! $SUDO pacman-key --list-secret-keys >/dev/null 2>&1 \
    || [ -z "$($SUDO pacman-key --list-secret-keys 2>/dev/null)" ]; then
    $SUDO pacman-key --init
  fi
  $SUDO pacman-key --add "$tmp/axiom.asc"
  # Доверяем только ключу с известным отпечатком: подменённый ключ не пройдёт.
  $SUDO pacman-key --lsign-key "$KEY_ID"
  if ! grep -q '^\[axiom\]' /etc/pacman.conf; then
    say "Подключаю репозиторий…"
    printf '\n[axiom]\nSigLevel = Required\nServer = %s/arch/$arch\n' "$REPO_URL" | $SUDO tee -a /etc/pacman.conf >/dev/null
  fi
  say "Устанавливаю…"
  # В Arch нельзя делать -Sy без -u (частичное обновление ломает систему).
  # Если есть терминал, pacman спросит подтверждение там; иначе ставим без вопросов.
  if { : </dev/tty; } 2>/dev/null; then
    $SUDO pacman -Syu --needed axiom-browser </dev/tty
  else
    $SUDO pacman -Syu --needed --noconfirm axiom-browser
  fi
elif command -v apt-get >/dev/null 2>&1; then
  say "Добавляю ключ подписи Axiom…"
  fetch "$REPO_URL/axiom.gpg" "$tmp/axiom.gpg"
  if command -v gpg >/dev/null 2>&1; then
    gpg --batch --show-keys --with-colons "$tmp/axiom.gpg" 2>/dev/null | grep -q "fpr:::::::::$KEY_ID:" \
      || die "отпечаток ключа не совпал"
  fi
  $SUDO install -Dm644 "$tmp/axiom.gpg" /usr/share/keyrings/axiom.gpg
  echo "deb [arch=amd64 signed-by=/usr/share/keyrings/axiom.gpg] $REPO_URL/apt stable main" \
    | $SUDO tee /etc/apt/sources.list.d/axiom.list >/dev/null
  # При запуске через `curl | sh` stdin — это сам скрипт: вопросы debconf
  # (например, tzdata в чистой системе) читали бы его и зависали. Ставим без вопросов.
  say "Обновляю списки пакетов…"
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get update </dev/null
  say "Устанавливаю…"
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y axiom-browser </dev/null
else
  die "не нашёл apt или pacman. Возьмите архив axiom-linux-x86_64.tar.gz: https://github.com/mylifeitsbest/axiom/releases/latest"
fi

say "Готово. Axiom есть в меню приложений, запуск из терминала: axiom-browser"
say "Обновления придут вместе с системой."
