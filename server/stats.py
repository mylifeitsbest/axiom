#!/usr/bin/env python3
"""Тихий сборщик статистики Axiom.

Принимает короткие пинги от браузеров (`POST /p`) и показывает приватную панель
(`GET /d/<ключ>`). Хранит только агрегаты в SQLite — ни IP, ни идентификаторов.
Зависимостей нет: всё из стандартной библиотеки, запускается на голой Ubuntu.

Переменные окружения:
  AXIOM_STATS_DB    путь к базе (по умолчанию ./stats.db)
  AXIOM_STATS_HOST  адрес прослушивания (по умолчанию 127.0.0.1 — за Caddy)
  AXIOM_STATS_PORT  порт (по умолчанию 8787)
  AXIOM_STATS_KEY   секрет в адресе панели: /d/<ключ>. Без него панель закрыта.
"""

import json
import os
import re
import sqlite3
from datetime import datetime, timedelta, timezone
from html import escape
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DB = os.environ.get("AXIOM_STATS_DB", "stats.db")
HOST = os.environ.get("AXIOM_STATS_HOST", "127.0.0.1")
PORT = int(os.environ.get("AXIOM_STATS_PORT", "8787"))
KEY = os.environ.get("AXIOM_STATS_KEY", "")

VERSION_RE = re.compile(r"^\d{1,9}\.\d{1,9}\.\d{1,9}$")
OSES = {"windows", "linux", "macos", "other"}
MAX_BODY = 512


def db():
    con = sqlite3.connect(DB, timeout=10)
    con.execute("PRAGMA busy_timeout=5000")
    con.execute(
        """CREATE TABLE IF NOT EXISTS hits(
            day TEXT, v TEXT, os TEXT,
            pings INTEGER DEFAULT 0, dau INTEGER DEFAULT 0,
            wau INTEGER DEFAULT 0, mau INTEGER DEFAULT 0,
            installs INTEGER DEFAULT 0,
            PRIMARY KEY(day, v, os))"""
    )
    return con


def today():
    return datetime.now(timezone.utc).strftime("%Y-%m-%d")


def record(payload):
    """Проверить пинг и прибавить к агрегатам за сегодня."""
    v = payload.get("v", "")
    os_ = payload.get("os", "")
    if not isinstance(v, str) or not VERSION_RE.match(v):
        return False
    if os_ not in OSES:
        return False
    flag = lambda k: 1 if payload.get(k) is True else 0
    con = db()
    with con:
        con.execute(
            """INSERT INTO hits(day, v, os, pings, dau, wau, mau, installs)
               VALUES(?,?,?,1,?,?,?,?)
               ON CONFLICT(day, v, os) DO UPDATE SET
                 pings=pings+1, dau=dau+excluded.dau, wau=wau+excluded.wau,
                 mau=mau+excluded.mau, installs=installs+excluded.installs""",
            (today(), v, os_, flag("d"), flag("w"), flag("m"), flag("new")),
        )
    con.close()
    return True


# ---------- панель ----------

def q1(con, sql, args=()):
    row = con.execute(sql, args).fetchone()
    return row[0] if row and row[0] is not None else 0


def dashboard():
    con = db()
    t = today()
    d7 = (datetime.now(timezone.utc) - timedelta(days=6)).strftime("%Y-%m-%d")
    d30 = (datetime.now(timezone.utc) - timedelta(days=29)).strftime("%Y-%m-%d")

    dau = q1(con, "SELECT SUM(dau) FROM hits WHERE day=?", (t,))
    wau = q1(con, "SELECT SUM(wau) FROM hits WHERE day>=?", (d7,))
    mau = q1(con, "SELECT SUM(mau) FROM hits WHERE day>=?", (d30,))
    pings_today = q1(con, "SELECT SUM(pings) FROM hits WHERE day=?", (t,))
    installs_all = q1(con, "SELECT SUM(installs) FROM hits")
    installs_30 = q1(con, "SELECT SUM(installs) FROM hits WHERE day>=?", (d30,))

    # Ряд DAU за 30 дней.
    series = {r[0]: r[1] for r in con.execute(
        "SELECT day, SUM(dau) FROM hits WHERE day>=? GROUP BY day", (d30,))}
    days = [(datetime.now(timezone.utc) - timedelta(days=i)).strftime("%Y-%m-%d")
            for i in range(29, -1, -1)]
    bars = [(d, series.get(d, 0)) for d in days]

    by_ver = con.execute(
        "SELECT v, SUM(dau) a FROM hits WHERE day>=? GROUP BY v ORDER BY a DESC",
        (d30,)).fetchall()
    by_os = con.execute(
        "SELECT os, SUM(dau) a FROM hits WHERE day>=? GROUP BY os ORDER BY a DESC",
        (d30,)).fetchall()
    con.close()

    return render(dau, wau, mau, pings_today, installs_all, installs_30,
                  bars, by_ver, by_os)


def render(dau, wau, mau, pings_today, installs_all, installs_30, bars, by_ver, by_os):
    peak = max((n for _, n in bars), default=0) or 1
    w, h, gap = 24, 90, 4
    svg = []
    for i, (day, n) in enumerate(bars):
        bh = round(h * n / peak)
        x = i * (w + gap)
        svg.append(
            f'<rect x="{x}" y="{h - bh}" width="{w}" height="{bh}" rx="3" '
            f'fill="#7c9cff"><title>{escape(day)}: {n}</title></rect>'
        )
    chart_w = len(bars) * (w + gap)

    def tbl(rows, label):
        body = "".join(
            f"<tr><td>{escape(str(k))}</td><td>{n}</td></tr>" for k, n in rows
        ) or '<tr><td colspan="2" class="muted">пока пусто</td></tr>'
        return (f'<table><thead><tr><th>{label}</th><th>актив. 30д</th></tr>'
                f'</thead><tbody>{body}</tbody></table>')

    def card(n, label):
        return f'<div class="card"><div class="n">{n}</div><div class="l">{label}</div></div>'

    updated = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="300">
<title>Axiom — статистика</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{ margin:0; background:#0e1016; color:#e7e9f0;
    font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif; }}
  .wrap {{ max-width:900px; margin:0 auto; padding:32px 20px 64px; }}
  h1 {{ font-size:20px; margin:0 0 2px; }}
  .sub {{ color:#8b90a3; font-size:13px; margin-bottom:24px; }}
  .cards {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(130px,1fr));
    gap:12px; margin-bottom:28px; }}
  .card {{ background:#171a23; border:1px solid #232735; border-radius:14px; padding:16px; }}
  .card .n {{ font-size:30px; font-weight:700; letter-spacing:-.5px; }}
  .card .l {{ color:#8b90a3; font-size:12px; margin-top:4px; }}
  h2 {{ font-size:13px; text-transform:uppercase; letter-spacing:.08em;
    color:#8b90a3; margin:28px 0 12px; }}
  .chart {{ background:#171a23; border:1px solid #232735; border-radius:14px;
    padding:16px; overflow-x:auto; }}
  .grid {{ display:grid; grid-template-columns:1fr 1fr; gap:16px; }}
  table {{ width:100%; border-collapse:collapse; background:#171a23;
    border:1px solid #232735; border-radius:14px; overflow:hidden; }}
  th,td {{ text-align:left; padding:9px 14px; font-size:14px; }}
  th {{ color:#8b90a3; font-weight:500; font-size:12px; background:#12141c; }}
  tr+tr td {{ border-top:1px solid #1d212c; }}
  td:last-child,th:last-child {{ text-align:right; font-variant-numeric:tabular-nums; }}
  .muted {{ color:#6b7083; }}
  @media (max-width:560px) {{ .grid {{ grid-template-columns:1fr; }} }}
</style></head><body><div class="wrap">
  <h1>Axiom — сколько людей пользуется</h1>
  <div class="sub">Обновлено {updated}. Страница сама обновляется каждые 5 минут.</div>
  <div class="cards">
    {card(dau, "сегодня (DAU)")}
    {card(wau, "за 7 дней (WAU)")}
    {card(mau, "за 30 дней (MAU)")}
    {card(installs_30, "установок за 30д")}
    {card(installs_all, "установок всего")}
    {card(pings_today, "пингов сегодня")}
  </div>
  <h2>Активные в день, 30 дней</h2>
  <div class="chart"><svg width="{chart_w}" height="{h + 4}"
    viewBox="0 0 {chart_w} {h + 4}" xmlns="http://www.w3.org/2000/svg">{''.join(svg)}</svg></div>
  <div class="grid">
    <div><h2>Версии</h2>{tbl(by_ver, "версия")}</div>
    <div><h2>Системы</h2>{tbl(by_os, "ОС")}</div>
  </div>
</div></body></html>"""


# ---------- HTTP ----------

class Handler(BaseHTTPRequestHandler):
    server_version = "axiom-stats"

    def log_message(self, *a):
        pass  # не пишем лог доступа — там были бы IP

    def _send(self, code, body=b"", ctype="text/plain; charset=utf-8"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_POST(self):
        if self.path != "/p":
            return self._send(404, b"not found")
        try:
            length = int(self.headers.get("Content-Length", 0))
        except ValueError:
            return self._send(400, b"bad")
        if length <= 0 or length > MAX_BODY:
            return self._send(400, b"bad")
        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return self._send(400, b"bad")
        if not isinstance(payload, dict) or not record(payload):
            return self._send(400, b"bad")
        self._send(204)

    def do_GET(self):
        if self.path == "/health":
            return self._send(200, b"ok")
        if KEY and self.path.rstrip("/") == f"/d/{KEY}":
            return self._send(200, dashboard().encode("utf-8"),
                              "text/html; charset=utf-8")
        self._send(404, b"not found")

    do_HEAD = do_GET


def main():
    db().close()  # создать базу и таблицу на старте
    srv = ThreadingHTTPServer((HOST, PORT), Handler)
    print(f"axiom-stats на http://{HOST}:{PORT} (база {DB})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
