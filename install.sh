#!/bin/sh
# Установка Axiom на Linux одной командой:
#   curl -fsSL https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.sh | sh
#
# Ubuntu, Debian, Kali: подключает подписанный репозиторий APT, обновления
# приходят вместе с системой (`sudo apt upgrade`).
# Arch и производные: ставит пакет axiom-browser-bin из AUR — через yay или paru,
# а без них собирает его makepkg. Систему при этом не обновляет: зависимости
# ставятся через `pacman -S --needed`, как у любого пакета из AUR.
# Подписанный репозиторий pacman — по желанию, и это `sudo pacman -Syu`
# (обновится вся система):
#   curl -fsSL .../install.sh | sh -s -- --arch-repo     (или AXIOM_ARCH_REPO=1)
# Бета-версия (каждый коммит в main, до выхода версии; только Arch):
#   curl -fsSL .../install.sh | sh -s -- --beta           (или AXIOM_BETA=1)
# Ставит axiom-browser-bin из пре-релиза beta через pacman -U, без обновления системы.
# Дальше новые бета-версии Axiom предлагает сам.
set -eu

# Всё тело — в функции, которая вызывается последней строкой: если `curl | sh`
# оборвётся на середине, оболочка не выполнит половину скрипта.
main() {
REPO_URL="https://mylifeitsbest.github.io/axiom"
KEY_ID="643C6DE601D4BBB661D5322AF2F379CD75CF5C03"
# SHA-256 файла packaging/keys/axiom.gpg — для систем без gpg. Обновить вместе с ключом.
KEY_SHA256="1153068b262a99055e59e872b52d587596f907e38fc5ab0202218b4071bc15dd"
AUR_PKG="axiom-browser-bin"
# Пре-релиз с бета-версиями (.github/workflows/beta.yml).
BETA_URL="https://github.com/mylifeitsbest/axiom/releases/download/beta"
# Тот же PKGBUILD, что в AUR. AXIOM_PKGBUILD_URL подменяет его в CI.
PKGBUILD_URL=${AXIOM_PKGBUILD_URL:-"https://raw.githubusercontent.com/mylifeitsbest/axiom/main/packaging/aur/$AUR_PKG/PKGBUILD"}

say() { printf '\033[1m%s\033[0m\n' "$*"; }
note() { printf '%s\n' "$*"; }
die() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

ARCH_REPO=${AXIOM_ARCH_REPO:-}
BETA=${AXIOM_BETA:-}
for arg in "$@"; do
  case $arg in
    --arch-repo) ARCH_REPO=1 ;;
    --beta) BETA=1 ;;
    *) die "неизвестный параметр $arg (есть --arch-repo и --beta)" ;;
  esac
done
case $ARCH_REPO in '' | 0 | no | false) ARCH_REPO="" ;; *) ARCH_REPO=1 ;; esac
case $BETA in '' | 0 | no | false) BETA="" ;; *) BETA=1 ;; esac

[ "$(uname -s)" = "Linux" ] || die "этот скрипт для Linux. Для Windows: https://github.com/mylifeitsbest/axiom#установка"
[ "$(uname -m)" = "x86_64" ] || die "пока есть только сборка для x86_64"

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else
  command -v sudo >/beta/null 2>&1 || die "нужен sudo (или запустите скрипт от root)"
  SUDO="sudo"
fi

fetch() {
  if command -v curl >/beta/null 2>&1; then curl -fsSL -o "$2" "$1"
  elif command -v wget >/beta/null 2>&1; then wget -q -O "$2" "$1"
  else die "нужен curl или wget"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
chmod 755 "$tmp"

# Вопросы pacman, makepkg и yay — в терминал, даже при `curl | sh` (там stdin —
# сам скрипт). Без терминала (CI, контейнеры) ставим без вопросов. Проверка
# в подоболочке: ошибка перенаправления у встроенных команд иначе завершила бы весь скрипт.
if (exec </beta/tty) 2>/beta/null; then TTY=/beta/tty; NOCONFIRM=""; else TTY=/beta/null; NOCONFIRM=1; fi

sync_hint() {
  die "установка не удалась. Если pacman не смог скачать пакеты (ошибка 404), базы пакетов устарели: обновите систему (sudo pacman -Syu) и запустите скрипт снова."
}

# Пакет уже есть в AUR? (До первой публикации его там нет.)
in_aur() {
  fetch "https://aur.archlinux.org/rpc/v5/info?arg%5B%5D=$AUR_PKG" "$tmp/aur.json" 2>/beta/null \
    && grep -q '"resultcount":[1-9]' "$tmp/aur.json"
}

# Подписанный репозиторий [axiom]: обновления через pacman -Syu. Только по просьбе.
arch_repo() {
  say "Внимание: установка из репозитория [axiom] — это sudo pacman -Syu, обновится вся система, а не только Axiom."
  note "pacman может предложить заменить или удалить пакеты из AUR и сторонних репозиториев."
  note "Без обновления системы: запустите скрипт без --arch-repo, он поставит $AUR_PKG из AUR."
  say "Добавляю ключ подписи Axiom…"
  fetch "$REPO_URL/axiom.asc" "$tmp/axiom.asc"
  # Свежие контейнеры и некоторые сборки идут без локального ключа pacman.
  if ! $SUDO pacman-key --list-secret-keys >/beta/null 2>&1 \
    || [ -z "$($SUDO pacman-key --list-secret-keys 2>/beta/null)" ]; then
    $SUDO pacman-key --init
  fi
  $SUDO pacman-key --add "$tmp/axiom.asc"
  # Доверяем только ключу с известным отпечатком: подменённый ключ не пройдёт.
  $SUDO pacman-key --lsign-key "$KEY_ID"
  if ! grep -q '^\[axiom\]' /etc/pacman.conf; then
    say "Подключаю репозиторий…"
    # shellcheck disable=SC2016 # $arch подставляет сам pacman
    printf '\n[axiom]\nSigLevel = Required\nServer = %s/arch/$arch\n' "$REPO_URL" | $SUDO tee -a /etc/pacman.conf >/beta/null
  fi
  say "Обновляю систему и устанавливаю Axiom (pacman -Syu)…"
  # В Arch нельзя делать -Sy без -u (частичное обновление ломает систему).
  $SUDO pacman -Syu --needed ${NOCONFIRM:+--noconfirm} axiom-browser <"$TTY"
  UPDATES="Обновления придут вместе с системой: sudo pacman -Syu"
}

# Бета-версия: пакет axiom-browser-bin из пре-релиза beta. Тот же пакет, что
# в AUR, только новее: встаёт поверх него. pacman -U систему не обновляет.
arch_beta() {
  pkg="$AUR_PKG-x86_64.pkg.tar.zst"
  say "Скачиваю бета-версию Axiom…"
  fetch "$BETA_URL/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" 2>/dev/null \
    || die "бета-версия ещё не опубликована (или нет связи с GitHub). Она появляется минут через 20-30 после изменения — попробуйте позже."
  fetch "$BETA_URL/$pkg" "$tmp/$pkg"
  sum=$(awk -v f="$pkg" '$2 == f { print $1 }' "$tmp/SHA256SUMS.txt")
  [ -n "$sum" ] || die "в SHA256SUMS.txt бета-версии нет строки для $pkg"
  # Новая сборка могла выкладываться прямо сейчас — тогда суммы не совпадут.
  printf '%s  %s\n' "$sum" "$tmp/$pkg" | sha256sum -c --status - \
    || die "контрольная сумма не совпала. Если сборка как раз обновлялась, запустите скрипт ещё раз через пару минут."
  if pacman -Qq axiom-browser 2>/beta/null | grep -qx axiom-browser; then
    note "Сейчас стоит пакет axiom-browser: pacman предложит заменить его на $AUR_PKG — ответьте y."
  fi
  say "Устанавливаю (pacman -U, без обновления системы)…"
  $SUDO pacman -U --needed ${NOCONFIRM:+--noconfirm} "$tmp/$pkg" <"$TTY" || sync_hint
  UPDATES="Новые бета-версии Axiom предложит сам. Вернуться к обычным версиям: выключите «Бета-версии» в настройках, следующая версия придёт из AUR."
}

# Пакет из AUR без обновления системы.
arch_aur() {
  # axiom-browser (наш репозиторий или сборка из исходников) конфликтует с axiom-browser-bin.
  # Не доводим до вопроса pacman «удалить?», а объясняем.
  if pacman -Qq axiom-browser 2>/beta/null | grep -qx axiom-browser; then
    say "Axiom уже установлен пакетом axiom-browser."
    if grep -q '^\[axiom\]' /etc/pacman.conf; then
      note "Он обновляется вместе с системой: sudo pacman -Syu"
    fi
    note "Перейти на $AUR_PKG из AUR: sudo pacman -R axiom-browser, затем снова этот скрипт."
    exit 0
  fi

  # Базы уже обновлены (pacman -Sy, автообновление баз в pamac), а система — нет:
  # тогда зависимости придут из более новых баз. Предупреждаем, решает человек.
  pending=$(pacman -Qu 2>/beta/null | grep -vc '\[ignored\]' || true)
  if [ "${pending:-0}" -gt 0 ]; then
    say "Внимание: базы pacman новее системы, не установлено обновлений: $pending (pacman -Qu)."
    note "Если ради зависимостей Axiom pacman захочет обновить библиотеки, это частичное обновление."
    note "Проверьте список пакетов перед подтверждением или сначала обновите систему."
  fi

  helper=""
  # yay и paru не работают от root.
  if [ -n "$SUDO" ]; then
    for h in yay paru; do
      if command -v "$h" >/beta/null 2>&1; then helper=$h; break; fi
    done
  fi
  if [ -n "$helper" ] && in_aur; then
    say "Устанавливаю $AUR_PKG из AUR через $helper…"
    "$helper" -S --needed ${NOCONFIRM:+--noconfirm} "$AUR_PKG" <"$TTY" || sync_hint
    UPDATES="Обновления: $helper -Syu, вместе с остальными пакетами."
  else
    arch_makepkg
    UPDATES="Обновления: запустите этот скрипт снова, когда выйдет новая версия."
  fi

  if grep -q '^\[axiom\]' /etc/pacman.conf; then
    note "В /etc/pacman.conf остался репозиторий [axiom] от прежней установки. Пакету из AUR он не нужен, секцию [axiom] можно удалить."
  fi
}

# Тот же PKGBUILD, что в AUR, собранный makepkg. Сборка — это распаковка
# архива из релиза, компилятор не нужен.
arch_makepkg() {
  build="$tmp/$AUR_PKG"
  mkdir "$build"
  say "Скачиваю PKGBUILD пакета $AUR_PKG…"
  fetch "$PKGBUILD_URL" "$build/PKGBUILD"
  if ! command -v fakeroot >/beta/null 2>&1; then
    say "Ставлю fakeroot, он нужен makepkg…"
    $SUDO pacman -S --needed --asdeps ${NOCONFIRM:+--noconfirm} fakeroot <"$TTY" || sync_hint
  fi
  # Всё, что качает и собирает makepkg, — во временной папке, даже если
  # в makepkg.conf заданы свои BUILDDIR, PKGDEST или SRCDEST.
  if [ -z "$SUDO" ]; then
    # makepkg не запускается от root: собираем от nobody, ставим сами.
    # pacman -U докачает зависимости через pacman -S, без обновления системы.
    command -v runuser >/beta/null 2>&1 || die "нужен runuser (util-linux)"
    chown -R nobody "$build"
    say "Собираю пакет…"
    (cd "$build" && runuser -u nobody -- env HOME="$build" \
      BUILDDIR="$build" PKGDEST="$build" SRCDEST="$build" makepkg --nodeps --noconfirm) \
      || die "makepkg не собрал пакет"
    set -- "$build/$AUR_PKG"-*.pkg.tar.*
    say "Устанавливаю…"
    pacman -U --needed ${NOCONFIRM:+--noconfirm} "$1" <"$TTY" || sync_hint
  else
    # makepkg -s ставит зависимости через pacman -S --needed --asdeps, систему не обновляет.
    say "Собираю и устанавливаю пакет (makepkg -si)…"
    (cd "$build" && env BUILDDIR="$build" PKGDEST="$build" SRCDEST="$build" \
      makepkg -si --needed ${NOCONFIRM:+--noconfirm} <"$TTY") || sync_hint
  fi
}

if command -v pacman >/beta/null 2>&1; then
  if [ -n "$BETA" ]; then arch_beta
  elif [ -n "$ARCH_REPO" ]; then arch_repo
  else arch_aur; fi
elif [ -n "$BETA" ]; then
  die "бета-версии есть только для Arch и Windows"
elif command -v apt-get >/beta/null 2>&1; then
  say "Добавляю ключ подписи Axiom…"
  fetch "$REPO_URL/axiom.gpg" "$tmp/axiom.gpg"
  # apt доверяет каждому ключу из файла signed-by, поэтому в файле должен быть
  # ровно один ключ — наш. Без gpg (минимальная система) сверяем файл целиком.
  if command -v gpg >/beta/null 2>&1; then
    keys=$(gpg --batch --show-keys --with-colons "$tmp/axiom.gpg" 2>/beta/null) || die "не удалось прочитать ключ подписи"
    # shellcheck disable=SC2015 # die завершает скрипт
    [ "$(printf '%s\n' "$keys" | grep -c '^pub:')" = 1 ] \
      && printf '%s\n' "$keys" | grep -q "^fpr:::::::::$KEY_ID:" \
      || die "ключ подписи не совпал"
  else
    printf '%s  %s\n' "$KEY_SHA256" "$tmp/axiom.gpg" | sha256sum -c --status - \
      || die "ключ подписи не совпал"
  fi
  $SUDO install -Dm644 "$tmp/axiom.gpg" /usr/share/keyrings/axiom.gpg
  echo "deb [arch=amd64 signed-by=/usr/share/keyrings/axiom.gpg] $REPO_URL/apt stable main" \
    | $SUDO tee /etc/apt/sources.list.d/axiom.list >/beta/null
  # При запуске через `curl | sh` stdin — это сам скрипт: вопросы debconf
  # (например, tzdata в чистой системе) читали бы его и зависали. Ставим без вопросов.
  say "Обновляю списки пакетов…"
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get update </beta/null
  say "Устанавливаю…"
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y axiom-browser </beta/null
  UPDATES="Обновления придут вместе с системой: sudo apt upgrade"
else
  die "не нашёл apt или pacman. Возьмите архив axiom-linux-x86_64.tar.gz: https://github.com/mylifeitsbest/axiom/releases/latest"
fi

say "Готово. Axiom есть в меню приложений, запуск из терминала: axiom-browser"
say "$UPDATES"
}

main "$@"
