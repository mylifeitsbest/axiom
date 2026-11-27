#!/usr/bin/env bash
# Островок на Linux без экрана: вкладка с видео уходит в фон, видео выезжает из чёлки
# (`AXIOM_UI_HASH=peek` — без мыши, `AXIOM_PEEK_HOLD` — не уезжает), снимки экрана.
# island.sh x11|wayland <папка для снимков>
set -u
DISPLAY_KIND=$1
OUT=$2
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=$ROOT/target/debug/axiom
WWW=$ROOT/.github/linux-gui/www
mkdir -p "$OUT"

export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
export NO_AT_BRIDGE=1

# Звук: видео без звука островок не показывает, а без звуковой карты GStreamer не играет.
pulseaudio --daemonize=yes --exit-idle-time=-1 --log-target=stderr 2>"$OUT/pulse.log" || true
sleep 1
pactl load-module module-null-sink sink_name=null >/dev/null 2>&1 || true
pactl set-default-sink null >/dev/null 2>&1 || true

if [ "$DISPLAY_KIND" = x11 ]; then
  Xvfb :99 -screen 0 1280x800x24 >"$OUT/xvfb.log" 2>&1 &
  export DISPLAY=:99 GDK_BACKEND=x11
  sleep 2
  shot() { import -window root "$OUT/$1.png"; }
else
  cat >"$XDG_RUNTIME_DIR/sway.conf" <<'EOF'
output HEADLESS-1 resolution 1280x800 position 0 0
default_border none
default_floating_border none
gaps inner 0
EOF
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
    sway -c "$XDG_RUNTIME_DIR/sway.conf" >"$OUT/sway.log" 2>&1 &
  for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break; sleep 0.2; done
  export WAYLAND_DISPLAY=wayland-1 GDK_BACKEND=wayland
  unset DISPLAY
  shot() { grim "$OUT/$1.png"; }
fi

python3 -m http.server 8000 --bind 127.0.0.1 --directory "$WWW" >"$OUT/http.log" 2>&1 &
sleep 1

run() { # run <имя> <адрес видео> <раскладка>
  local name=$1 url=$2 layout=$3
  local dir=$OUT/$name
  mkdir -p "$dir"
  export AXIOM_PROFILE
  AXIOM_PROFILE=$(mktemp -d)
  cat >"$AXIOM_PROFILE/settings.json" <<EOF
{"onboarded":true,"check_updates":false,"usage_stats":false,"crash_reports":false,"appearance":{"layout":"$layout"}}
EOF
  export AXIOM_UI_HASH=peek AXIOM_PEEK_HOLD=1 AXIOM_PEEK_LOG=$dir/peek.log
  "$BIN" "$url" >"$dir/axiom.log" 2>&1 &
  local pid=$!
  sleep 20
  shot "$name/1-video"
  # Вторая вкладка: видео уходит в фон, островок его подхватывает и через 1,5 с выводит.
  "$BIN" "http://127.0.0.1:8000/other.html" >>"$dir/second.log" 2>&1
  sleep 3
  shot "$name/2-peek"
  sleep 3
  shot "$name/3-peek"
  sleep 6
  shot "$name/4-peek"
  kill "$pid" 2>/dev/null
  sleep 2
  kill -9 "$pid" 2>/dev/null
  pkill -f WebKitWebProcess 2>/dev/null
  pkill -f WebKitNetworkProcess 2>/dev/null
  sleep 1
}

run local-one "http://127.0.0.1:8000/video.html" one
run local-side "http://127.0.0.1:8000/video.html" side
true
