#!/usr/bin/env bash
# Магазин плагинов Axiom на сервер (Ubuntu 24.04, где уже стоит Caddy). Запускать от root
# из клона репозитория (или из папки, куда скопированы store/server и store/plugins):
#   sudo bash store/server/deploy.sh
# Идемпотентно: ключ администратора и база сохраняются.
set -euo pipefail

if [ "$(id -u)" != 0 ]; then
	echo "Запускать от root: sudo bash deploy.sh" >&2
	exit 1
fi

SRC="$(cd "$(dirname "$0")" && pwd)"
# Свой адрес, отдельный от статистики. sslip.io превращает IP в имя для сертификата.
DOMAIN="${AXIOM_STORE_DOMAIN:-plugins.179-254-172-240.sslip.io}"
CADDY="$(command -v caddy || echo /usr/local/bin/caddy)"

echo "==> Служба магазина"
install -D -m 0644 "$SRC/store.py" /opt/axiom-store/store.py
# Плагины Axiom: store/plugins рядом со store/server (или plugins рядом с deploy.sh).
rm -rf /opt/axiom-store/official
for PLUGINS in "$SRC/../plugins" "$SRC/plugins"; do
	if [ -d "$PLUGINS" ]; then
		mkdir -p /opt/axiom-store/official
		cp -r "$PLUGINS"/. /opt/axiom-store/official/
		chmod -R a+rX /opt/axiom-store/official
		break
	fi
done
# Ключ администратора создаём один раз и больше не трогаем.
if [ ! -f /etc/axiom-store.env ]; then
	KEY="axa_$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
	(umask 077 && printf 'AXIOM_STORE_ADMIN_KEY=%s\n' "$KEY" >/etc/axiom-store.env)
fi
install -m 0644 "$SRC/axiom-store.service" /etc/systemd/system/axiom-store.service
systemctl daemon-reload
systemctl enable --now axiom-store.service
# Повторный запуск: новый store.py и плагины Axiom — только после перезапуска.
systemctl restart axiom-store.service

echo "==> Caddy: $DOMAIN"
mkdir -p /etc/caddy/sites
sed "s/__DOMAIN__/$DOMAIN/" "$SRC/Caddyfile" >/etc/caddy/sites/axiom-store.caddy
# Основной Caddyfile (его пишет server/deploy.sh) уже подключает /etc/caddy/sites; на
# сервере со старым Caddyfile — дописать строку.
if ! grep -q '^import /etc/caddy/sites/\*\.caddy' /etc/caddy/Caddyfile; then
	printf '\nimport /etc/caddy/sites/*.caddy\n' >>/etc/caddy/Caddyfile
fi
# Ошибка в конфиге — Caddy продолжит работать со старым, а мы остановимся здесь.
"$CADDY" validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
systemctl reload caddy.service || systemctl restart caddy.service

sleep 1
systemctl is-active --quiet axiom-store.service || {
	journalctl -u axiom-store.service -n 30 --no-pager
	exit 1
}
journalctl -u axiom-store.service -n 20 --no-pager -o cat | grep -E ': (опубликован|обновлён|без изменений)|плагины Axiom' || true

KEY="$(grep -oP '(?<=AXIOM_STORE_ADMIN_KEY=).*' /etc/axiom-store.env)"
echo
echo "Готово. Сайт магазина: https://$DOMAIN"
echo "Ключ администратора (вход на сайте, никому не давать):"
echo "  $KEY"
echo "Сертификат Let's Encrypt Caddy возьмёт за 10-30 секунд после старта."
