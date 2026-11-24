#!/usr/bin/env bash
# Установка сборщика статистики Axiom на чистую Ubuntu 24.04. Запускать от root:
#   sudo bash deploy.sh
# Идемпотентно: можно гонять повторно, ключ панели и база сохраняются.
set -euo pipefail

if [ "$(id -u)" != 0 ]; then
	echo "Запускать от root: sudo bash deploy.sh" >&2
	exit 1
fi

SRC="$(cd "$(dirname "$0")" && pwd)"
# Домен для HTTPS. sslip.io превращает IP в имя, на которое Let's Encrypt выдаёт
# сертификат, — отдельный домен покупать не нужно. Поменять: AXIOM_STATS_DOMAIN=...
DOMAIN="${AXIOM_STATS_DOMAIN:-179-254-172-240.sslip.io}"

echo "==> Python-служба"
install -D -m 0644 "$SRC/stats.py" /opt/axiom-stats/stats.py

# Секретный ключ панели создаём один раз и больше не трогаем.
if [ ! -f /etc/axiom-stats.env ]; then
	KEY="$(head -c 32 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 32)"
	# umask: файл с ключом ни на миг не бывает доступен на чтение другим.
	(umask 077 && printf 'AXIOM_STATS_KEY=%s\n' "$KEY" >/etc/axiom-stats.env)
fi
install -m 0644 "$SRC/axiom-stats.service" /etc/systemd/system/axiom-stats.service

echo "==> Caddy (HTTPS)"
if ! command -v caddy >/dev/null 2>&1 && [ ! -x /usr/local/bin/caddy ]; then
	curl -fsSL "https://caddyserver.com/api/download?os=linux&arch=amd64" \
		-o /usr/local/bin/caddy
	chmod +x /usr/local/bin/caddy
fi
id caddy >/dev/null 2>&1 || useradd --system --create-home \
	--home-dir /var/lib/caddy --shell /usr/sbin/nologin caddy
mkdir -p /etc/caddy
sed "s/__DOMAIN__/$DOMAIN/" "$SRC/Caddyfile" >/etc/caddy/Caddyfile
install -m 0644 "$SRC/caddy.service" /etc/systemd/system/caddy.service

echo "==> Запуск"
systemctl daemon-reload
systemctl enable --now axiom-stats.service
systemctl enable --now caddy.service
# Повторный запуск: enable --now не перезапускает уже работающие службы,
# и новый stats.py и Caddyfile иначе не применились бы.
systemctl restart axiom-stats.service
systemctl reload caddy.service || systemctl restart caddy.service

# Если включён ufw — открыть порты 80/443 и 22.
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
	ufw allow 22/tcp || true
	ufw allow 80/tcp || true
	ufw allow 443/tcp || true
fi

KEY="$(grep -oP '(?<=AXIOM_STATS_KEY=).*' /etc/axiom-stats.env)"
echo
echo "Готово. Панель (открой в браузере, адрес секретный — никому не давай):"
echo "  https://$DOMAIN/d/$KEY"
echo
echo "Браузер шлёт пинги на https://$DOMAIN/p"
echo "Сертификат Let's Encrypt Caddy возьмёт за 10-30 секунд после старта."
