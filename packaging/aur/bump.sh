#!/bin/sh
# Новая версия пакета axiom-browser-bin для AUR: подставляет версию и SHA-256
# архива axiom-linux-x86_64.tar.gz в PKGBUILD и .SRCINFO, pkgrel снова 1.
#   sh packaging/aur/bump.sh 0.5.0            # сумма из SHA256SUMS.txt релиза v0.5.0
#   sh packaging/aur/bump.sh 0.5.0 <sha256>   # сумма уже известна (так делает CI)
# Работает и в Git Bash на Windows: makepkg не нужен. CI сверяет .SRCINFO
# с `makepkg --printsrcinfo`, так что ошибка здесь не пройдёт незамеченной.
set -eu

die() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

[ $# -ge 1 ] && [ $# -le 2 ] || die "нужна версия: sh $0 0.5.0 [sha256]"
ver=$1
case $ver in '' | *[!0-9.]* | .* | *. | *..*) die "версия вида 0.5.0, без v: $ver" ;; esac

if [ $# -eq 2 ]; then
  sum=$2
else
  sums_url="https://github.com/mylifeitsbest/axiom/releases/download/v$ver/SHA256SUMS.txt"
  sum=$(curl -fsSL "$sums_url" | awk '$2 == "axiom-linux-x86_64.tar.gz" { print $1 }')
  [ -n "$sum" ] || die "нет суммы архива в $sums_url (релиз уже опубликован?)"
fi
printf '%s\n' "$sum" | grep -Eqx '[0-9a-f]{64}' || die "нет SHA-256 архива (получено: '$sum')"

dir=$(dirname "$0")/axiom-browser-bin
old=$(sed -n 's/^pkgver=//p' "$dir/PKGBUILD")
oldsum=$(sed -n "s/^sha256sums=('\([0-9a-f]\{64\}\)')\$/\1/p" "$dir/PKGBUILD")
[ -n "$old" ] && [ -n "$oldsum" ] || die "не нашёл pkgver и sha256sums в $dir/PKGBUILD"
oldre=$(printf '%s' "$old" | sed 's/\./\\./g')

# sed -i пишется по-разному в GNU и BSD, поэтому через временный файл.
edit() {
  file=$1
  shift
  sed "$@" "$file" > "$file.new"
  mv "$file.new" "$file"
}

edit "$dir/PKGBUILD" \
  -e "s/^pkgver=.*/pkgver=$ver/" \
  -e "s/^pkgrel=.*/pkgrel=1/" \
  -e "s/^sha256sums=.*/sha256sums=('$sum')/"
# В .SRCINFO версия встречается в pkgver, provides и source.
edit "$dir/.SRCINFO" \
  -e "/^[[:space:]]pkgver = /s/$oldre/$ver/" \
  -e "/^[[:space:]]provides = /s/=$oldre\$/=$ver/" \
  -e "/^[[:space:]]source = /s/$oldre/$ver/g" \
  -e "s/^\([[:space:]]\)pkgrel = .*/\1pkgrel = 1/" \
  -e "s/^\([[:space:]]\)sha256sums = .*/\1sha256sums = $sum/"

printf 'axiom-browser-bin: %s -> %s-1, sha256 %s\n' "$old" "$ver" "$sum"
