#!/usr/bin/env bash
# Блокировка рекламы на Linux без экрана (X11): WebKit компилирует правила, страница
# с рекламой (www/ads.html) открывается после этого и при новом запуске с готовыми
# правилами. Страница сама сообщает, что загрузилось и что видно (запрос /report в
# журнале сервера); проверка падает, если реклама прошла или пропало обычное.
# adblock.sh <папка для снимков и журналов>
set -u
OUT=$1
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=$ROOT/target/debug/axiom
WWW=$ROOT/.github/linux-gui/www
mkdir -p "$OUT"

export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
export NO_AT_BRIDGE=1

Xvfb :99 -screen 0 1280x800x24 >"$OUT/xvfb.log" 2>&1 &
export DISPLAY=:99 GDK_BACKEND=x11
sleep 2

python3 -m http.server 8000 --bind 127.0.0.1 --directory "$WWW" >"$OUT/http.log" 2>&1 &
sleep 1
# На 127.0.0.1 EasyList сам выключает общие косметические правила ($generichide) —
# страница открывается как сайт: news.localhost (тот же 127.0.0.1).
PAGE=http://news.localhost:8000/ads.html

export AXIOM_PROFILE
AXIOM_PROFILE=$(mktemp -d)
cat >"$AXIOM_PROFILE/settings.json" <<'JSON'
{"onboarded":true,"check_updates":false,"usage_stats":false,"crash_reports":false,"adblock_updates":false}
JSON

stop() {
  kill "$1" 2>/dev/null
  sleep 2
  kill -9 "$1" 2>/dev/null
  pkill -f WebKitWebProcess 2>/dev/null
  pkill -f WebKitNetworkProcess 2>/dev/null
  sleep 1
}

# 1. Первый запуск: правила компилируются; страница — когда они готовы.
"$BIN" "about:blank" >"$OUT/axiom-1.log" 2>&1 &
pid=$!
for _ in $(seq 120); do grep -qE "правила WebKit|не принял" "$OUT/axiom-1.log" && break; sleep 1; done
grep "блокировка рекламы" "$OUT/axiom-1.log"
"$BIN" "$PAGE?run=1" >>"$OUT/second.log" 2>&1
sleep 12
import -window root "$OUT/1-compiled.png"
stop "$pid"

# 2. Новый запуск с теми же правилами: страница сразу при старте.
"$BIN" "$PAGE?run=2" >"$OUT/axiom-2.log" 2>&1 &
pid=$!
sleep 15
import -window root "$OUT/2-restart.png"
grep "блокировка рекламы" "$OUT/axiom-2.log"
stop "$pid"

fail=0
for run in 1 2; do
  line=$(grep "GET /report?" "$OUT/http.log" | grep -n "" | sed -n "${run}p" | cut -d: -f2-)
  report=$(python3 -c 'import sys, urllib.parse; l = sys.argv[1]; print(urllib.parse.unquote(l.split("/report?", 1)[1].split(" HTTP/", 1)[0]) if "/report?" in l else "")' "$line")
  echo "запуск $run: ${report:-нет отчёта}"
  python3 - "$report" "$run" <<'PY' || fail=1
import json, sys
raw, run = sys.argv[1], sys.argv[2]
if not raw:
    sys.exit(f"запуск {run}: страница не отчиталась")
r = json.loads(raw)
bad = [n for n in ("adsbygoogle", "yandex", "adimg") if n in r["R"]["ok"]]
bad += [f"{i} виден" for i in ("g1", "g2", "ad_block", "g3") if r["css"][i] != "none"]
bad += [f"{i} скрыт" for i in ("n1", "title") if r["css"][i] == "none"]
if not r["own"]:
    bad.append("свой скрипт не загрузился")
if bad:
    sys.exit(f"запуск {run}: " + ", ".join(bad))
PY
done
count=$(grep -c "заблокировано во вкладке" "$OUT/axiom-2.log" || true)
echo "счётчик (второй запуск): $count"
[ "$count" -gt 0 ] || { echo "счётчик не работает"; fail=1; }
exit $fail
