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

import hmac
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

VERSION_RE = re.compile(r"\d{1,4}\.\d{1,4}\.\d{1,4}")
OSES = {"windows", "linux", "macos", "other"}
MAX_BODY = 512
# Строк (версия × ОС) за день. Поддельные версии иначе раздували бы базу без предела.
MAX_ROWS_PER_DAY = 200
# То же для десятиминутных ячеек «последних двух суток».
MAX_ROWS_PER_SLOT = 50
# Сколько держать десятиминутные ячейки: на них «сейчас», часы и лента.
RECENT_KEEP = timedelta(hours=48)
SLOT_MIN = 10


def db():
    con = sqlite3.connect(DB, timeout=10)
    con.execute("PRAGMA busy_timeout=5000")
    return con


def init():
    """Создать таблицы и дописать новые столбцы в базу от прошлой версии."""
    con = db()
    with con:
        # hits — итоги за день по версии и системе. Флаги приходят от браузера:
        # dau/wau/mau — «первая отметка за день/неделю/месяц», installs — новая
        # установка, updates — первая отметка после обновления, vdau — первая
        # отметка за день на этой версии.
        con.execute(
            """CREATE TABLE IF NOT EXISTS hits(
                day TEXT, v TEXT, os TEXT,
                pings INTEGER DEFAULT 0, dau INTEGER DEFAULT 0,
                wau INTEGER DEFAULT 0, mau INTEGER DEFAULT 0,
                installs INTEGER DEFAULT 0,
                PRIMARY KEY(day, v, os))"""
        )
        cols = {r[1] for r in con.execute("PRAGMA table_info(hits)")}
        if "updates" not in cols:
            con.execute("ALTER TABLE hits ADD COLUMN updates INTEGER DEFAULT 0")
        if "vdau" not in cols:
            con.execute("ALTER TABLE hits ADD COLUMN vdau INTEGER DEFAULT 0")
            # Старые браузеры флага vd не шлют: для них он равен d.
            con.execute("UPDATE hits SET vdau = dau")
        # recent — то же по десятиминуткам за двое суток: «сейчас», часы, лента.
        con.execute(
            """CREATE TABLE IF NOT EXISTS recent(
                slot TEXT, v TEXT,
                pings INTEGER DEFAULT 0, installs INTEGER DEFAULT 0,
                updates INTEGER DEFAULT 0,
                PRIMARY KEY(slot, v))"""
        )
    con.close()


def utcnow():
    return datetime.now(timezone.utc)


def day_key(t):
    return t.strftime("%Y-%m-%d")


def slot_key(t):
    """Десятиминутка: '2026-11-24T13:10'. Строки сортируются как время."""
    return t.strftime("%Y-%m-%dT%H:") + f"{t.minute // SLOT_MIN * SLOT_MIN:02d}"


def q1(con, sql, args=()):
    row = con.execute(sql, args).fetchone()
    return row[0] if row and row[0] is not None else 0


def record(payload):
    """Проверить пинг и прибавить к агрегатам."""
    v = payload.get("v", "")
    os_ = payload.get("os", "")
    # fullmatch: с `$` в match() проходит и "1.2.3\n".
    if not isinstance(v, str) or not VERSION_RE.fullmatch(v):
        return False
    if not isinstance(os_, str) or os_ not in OSES:
        return False
    flag = lambda k: 1 if payload.get(k) is True else 0
    d, new, up = flag("d"), flag("new"), flag("up")
    # До 0.4.0 браузер не слал vd; тогда «первый раз сегодня на этой версии» = d.
    vd = flag("vd") if "vd" in payload else d

    now = utcnow()
    day, slot = day_key(now), slot_key(now)
    con = db()
    known = con.execute(
        "SELECT 1 FROM hits WHERE day=? AND v=? AND os=?", (day, v, os_)).fetchone()
    if not known and q1(con, "SELECT COUNT(*) FROM hits WHERE day=?", (day,)) >= MAX_ROWS_PER_DAY:
        con.close()
        return False
    slot_known = con.execute(
        "SELECT 1 FROM recent WHERE slot=? AND v=?", (slot, v)).fetchone()
    slot_full = not slot_known and q1(
        con, "SELECT COUNT(*) FROM recent WHERE slot=?", (slot,)) >= MAX_ROWS_PER_SLOT
    with con:
        con.execute(
            """INSERT INTO hits(day, v, os, pings, dau, wau, mau, installs, updates, vdau)
               VALUES(?,?,?,1,?,?,?,?,?,?)
               ON CONFLICT(day, v, os) DO UPDATE SET
                 pings=pings+1, dau=dau+excluded.dau, wau=wau+excluded.wau,
                 mau=mau+excluded.mau, installs=installs+excluded.installs,
                 updates=updates+excluded.updates, vdau=vdau+excluded.vdau""",
            (day, v, os_, d, flag("w"), flag("m"), new, up, vd),
        )
        if not slot_full:
            con.execute(
                """INSERT INTO recent(slot, v, pings, installs, updates)
                   VALUES(?,?,1,?,?)
                   ON CONFLICT(slot, v) DO UPDATE SET
                     pings=pings+1, installs=installs+excluded.installs,
                     updates=updates+excluded.updates""",
                (slot, v, new, up),
            )
        con.execute("DELETE FROM recent WHERE slot<?", (slot_key(now - RECENT_KEEP),))
    con.close()
    return True


# ---------- панель ----------

def ver_key(v):
    return tuple(int(x) for x in v.split("."))


def dashboard():
    con = db()
    now = utcnow()
    t = day_key(now)
    d30 = day_key(now - timedelta(days=29))
    # Браузер ставит флаги w/m на первый пинг календарной недели (с понедельника,
    # UTC) и месяца. Поэтому и считаем с их начала: в скользящем окне 7/30 дней
    # человек, заходивший в обе недели (оба месяца), попадал бы дважды.
    week_start = day_key(now - timedelta(days=now.weekday()))
    month_start = now.strftime("%Y-%m-01")
    # Открытый браузер отмечается раз в 60–61 минуту. Окно — от начала
    # десятиминутки час назад, то есть 60–70 минут: каждый открытый попадёт.
    hour_ago = slot_key(now - timedelta(hours=1))

    s = {
        "now": now,
        "online": q1(con, "SELECT SUM(pings) FROM recent WHERE slot>=?", (hour_ago,)),
        "dau": q1(con, "SELECT SUM(dau) FROM hits WHERE day=?", (t,)),
        "wau": q1(con, "SELECT SUM(wau) FROM hits WHERE day>=?", (week_start,)),
        "mau": q1(con, "SELECT SUM(mau) FROM hits WHERE day>=?", (month_start,)),
        "pings_today": q1(con, "SELECT SUM(pings) FROM hits WHERE day=?", (t,)),
        "inst_today": q1(con, "SELECT SUM(installs) FROM hits WHERE day=?", (t,)),
        "inst_all": q1(con, "SELECT SUM(installs) FROM hits"),
        "upd_today": q1(con, "SELECT SUM(updates) FROM hits WHERE day=?", (t,)),
        "upd_all": q1(con, "SELECT SUM(updates) FROM hits"),
        "week_start": week_start,
    }

    # По дням: активные и из них новые.
    rows = {r[0]: (r[1] or 0, r[2] or 0) for r in con.execute(
        "SELECT day, SUM(dau), SUM(installs) FROM hits WHERE day>=? GROUP BY day", (d30,))}
    s["days"] = [(d, *rows.get(d, (0, 0))) for d in
                 (day_key(now - timedelta(days=i)) for i in range(29, -1, -1))]

    # По часам за сутки: сколько отметок пришло в каждый час ≈ сколько
    # браузеров было открыто.
    hours = {r[0]: r[1] for r in con.execute(
        "SELECT substr(slot, 1, 13), SUM(pings) FROM recent GROUP BY 1")}
    top = now.replace(minute=0, second=0, microsecond=0)
    s["hours"] = [(h, hours.get(h.strftime("%Y-%m-%dT%H"), 0)) for h in
                  (top - timedelta(hours=i) for i in range(23, -1, -1))]

    # Лента: установки и обновления за двое суток, свежие сверху.
    s["feed"] = con.execute(
        """SELECT slot, v, installs, updates FROM recent
           WHERE installs>0 OR updates>0 ORDER BY slot DESC, v DESC LIMIT 30""").fetchall()

    # Версии: сколько людей на ней сегодня и сколько всего на неё обновились.
    vers = {}
    for v, today, upd in con.execute(
            """SELECT v, SUM(CASE WHEN day=? THEN vdau ELSE 0 END), SUM(updates)
               FROM hits WHERE day>=? GROUP BY v""", (t, d30)):
        # Старые версии, на которых сегодня никого, только загромождали бы таблицу.
        if today or upd:
            vers[v] = (today or 0, upd or 0)
    s["versions"] = sorted(vers.items(), key=lambda kv: ver_key(kv[0]), reverse=True)
    s["oses"] = con.execute(
        """SELECT os, SUM(dau) a FROM hits WHERE day=? GROUP BY os
           HAVING a>0 ORDER BY a DESC""", (t,)).fetchall()
    con.close()
    return render(s)


MONTHS = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля",
          "августа", "сентября", "октября", "ноября", "декабря"]


def plural(n, one, few, many):
    n = abs(n) % 100
    if 10 < n < 20:
        return many
    n %= 10
    return one if n == 1 else few if 2 <= n <= 4 else many


def bar_path(x, y, w, h, r=4):
    """Столбик со скруглённым верхом и плоским низом (стоит на оси)."""
    if h <= 0:
        return ""
    r = min(r, h, w / 2)
    return (f"M{x:.1f},{y + h:.1f}V{y + r:.1f}Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f}"
            f"H{x + w - r:.1f}Q{x + w:.1f},{y:.1f} {x + w:.1f},{y + r:.1f}V{y + h:.1f}Z")


def utc_iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:00Z")


def chart_days(days):
    """30 столбиков: вернувшиеся (синий) и новые (оранжевый) сверху."""
    W, H, top, bottom = 840, 150, 18, 22
    n = len(days)
    step = W / n
    bw = step - 6
    peak = max((a for _, a, _ in days), default=0) or 1
    plot = H - top - bottom
    out = [f'<line x1="0" x2="{W}" y1="{H - bottom}" y2="{H - bottom}" class="axis"/>']
    for i, (day, active, new) in enumerate(days):
        x = i * step + 3
        new = min(new, active)
        old = active - new
        h_all = plot * active / peak
        h_old = plot * old / peak
        base = H - bottom
        tip = f"{int(day[8:])} {MONTHS[int(day[5:7]) - 1]}: {active} {plural(active, 'человек', 'человека', 'человек')}"
        if new:
            tip += f", из них новых {new}"
        if old:
            # Нижний сегмент плоский; если сверху новые — зазор 2px.
            seg_top = base - h_old
            r = 0 if new else 4
            out.append(f'<path d="{bar_path(x, seg_top, bw, h_old, r)}" class="s-old"/>')
        if new:
            gap = 2 if old else 0
            y = base - h_all
            out.append(f'<path d="{bar_path(x, y, bw, max(h_all - h_old - gap, 1))}" class="s-new"/>')
        if active and (i == n - 1 or active == peak):
            out.append(f'<text x="{x + bw / 2:.1f}" y="{base - h_all - 5:.1f}" '
                       f'class="val" text-anchor="middle">{active}</text>')
        # Широкая невидимая зона для подсказки.
        out.append(f'<rect x="{i * step:.1f}" y="0" width="{step:.1f}" height="{H}" '
                   f'class="hit"><title>{escape(tip)}</title></rect>')
        if i in (0, 7, 14, 21) or i == n - 1:
            label = "сегодня" if i == n - 1 else f"{int(day[8:])} {MONTHS[int(day[5:7]) - 1][:3]}"
            anchor = "end" if i == n - 1 else "start"
            lx = x + bw if i == n - 1 else x
            out.append(f'<text x="{lx:.1f}" y="{H - 5}" class="tick" '
                       f'text-anchor="{anchor}">{label}</text>')
    return f'<svg viewBox="0 0 {W} {H}" class="chart-svg" role="img">{"".join(out)}</svg>'


def chart_hours(hours):
    W, H, top, bottom = 840, 120, 18, 22
    n = len(hours)
    step = W / n
    bw = step - 8
    peak = max((c for _, c in hours), default=0) or 1
    plot = H - top - bottom
    base = H - bottom
    out = [f'<line x1="0" x2="{W}" y1="{base}" y2="{base}" class="axis"/>']
    for i, (h, c) in enumerate(hours):
        x = i * step + 4
        bh = plot * c / peak
        cls = "s-old now" if i == n - 1 else "s-old"
        out.append(f'<path d="{bar_path(x, base - bh, bw, bh)}" class="{cls}"/>')
        if c and (c == peak or i == n - 1):
            out.append(f'<text x="{x + bw / 2:.1f}" y="{base - bh - 5:.1f}" '
                       f'class="val" text-anchor="middle">{c}</text>')
        out.append(f'<rect x="{i * step:.1f}" y="0" width="{step:.1f}" height="{H}" class="hit">'
                   f'<title data-utc="{utc_iso(h)}" data-fmt="hour-tip" data-n="{c}">'
                   f'{c}</title></rect>')
        if i % 3 == 0 or i == n - 1:
            out.append(f'<text x="{x + bw / 2:.1f}" y="{H - 5}" class="tick" text-anchor="middle"'
                       f' data-utc="{utc_iso(h)}" data-fmt="{"now" if i == n - 1 else "h"}">'
                       f'{h:%H}</text>')
    return f'<svg viewBox="0 0 {W} {H}" class="chart-svg" role="img">{"".join(out)}</svg>'


def feed_html(feed):
    if not feed:
        return ('<div class="feed-empty">Пока ничего. Здесь появятся новые установки и '
                'обновления — через минуту после того, как они случатся.</div>')
    items = []
    for slot, v, inst, upd in feed:
        when = f'<time data-utc="{escape(slot)}:00Z" data-fmt="feed">{escape(slot[11:])}</time>'
        if inst:
            items.append(
                f'<li><span class="dot d-new"></span>{when}<span class="what">'
                f'+{inst} {plural(inst, "новая установка", "новые установки", "новых установок")}'
                f'</span><span class="ver">{escape(v)}</span></li>')
        if upd:
            items.append(
                f'<li><span class="dot d-upd"></span>{when}<span class="what">'
                f'{upd} {plural(upd, "обновился", "обновились", "обновились")} до {escape(v)}'
                f'</span></li>')
    return f'<ul class="feed">{"".join(items)}</ul>'


def render(s):
    def tile(n, label, sub, accent=False):
        cls = "tile accent" if accent else "tile"
        return (f'<div class="{cls}"><div class="l">{label}</div><div class="n">{n}</div>'
                f'<div class="sub">{sub}</div></div>')

    now = s["now"]
    month_name = MONTHS[now.month - 1]
    ws = s["week_start"]
    week_sub = f"с понедельника, {int(ws[8:])} {MONTHS[int(ws[5:7]) - 1]}"
    midnight = utc_iso(now.replace(hour=0, minute=0, second=0, microsecond=0))

    tiles = "".join([
        tile(f'≈{s["online"]}', "Сейчас", "открывали браузер за последний час", True),
        tile(s["dau"], "Сегодня",
             f'разных людей с <time data-utc="{midnight}" data-fmt="hm">00:00 UTC</time>'),
        tile(s["wau"], "За неделю", week_sub),
        tile(s["mau"], "За месяц", f"с 1 {month_name}"),
    ])
    small = "".join([
        tile(f'+{s["inst_today"]}', "Новые установки сегодня", f'всего за всё время: {s["inst_all"]}'),
        tile(s["upd_today"], "Обновились сегодня", f'всего за всё время: {s["upd_all"]}'),
    ])

    vrows = "".join(
        f'<tr><td>{escape(v)}</td><td>{today}</td><td>{upd or "—"}</td></tr>'
        for v, (today, upd) in s["versions"]
    ) or '<tr><td colspan="3" class="muted">сегодня ещё никого</td></tr>'
    orows = "".join(
        f'<tr><td>{escape(o)}</td><td>{n}</td></tr>' for o, n in s["oses"]
    ) or '<tr><td colspan="2" class="muted">сегодня ещё никого</td></tr>'

    pt = s["pings_today"]
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="60">
<title>Axiom — статистика</title>
<style>
  :root {{ color-scheme: dark;
    --bg:#0e1016; --surface:#171a23; --line:#232735; --ink:#e7e9f0; --ink2:#b5b9c8;
    --muted:#8b90a3; --axis:#383c4a; --old:#3987e5; --new:#d95926; --upd:#199e70; }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; background:var(--bg); color:var(--ink);
    font:15px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
  .wrap {{ max-width:920px; margin:0 auto; padding:32px 20px 64px; }}
  h1 {{ font-size:20px; margin:0 0 2px; }}
  .top-sub {{ color:var(--muted); font-size:13px; margin-bottom:22px; }}
  .tiles {{ display:grid; grid-template-columns:repeat(4,1fr); gap:12px; }}
  .tiles.small {{ grid-template-columns:repeat(2,1fr); margin-top:12px; }}
  .tile {{ background:var(--surface); border:1px solid var(--line); border-radius:14px; padding:14px 16px; }}
  .tile .l {{ color:var(--ink2); font-size:13px; font-weight:600; }}
  .tile .n {{ font-size:34px; font-weight:700; letter-spacing:-.5px; line-height:1.15; margin:4px 0 2px;
    font-variant-numeric:tabular-nums; }}
  .tiles.small .n {{ font-size:26px; }}
  .tile .sub {{ color:var(--muted); font-size:12px; }}
  .tile.accent {{ border-color:#2d4a7a; background:linear-gradient(180deg,#18223a,#171a23); }}
  h2 {{ font-size:13px; text-transform:uppercase; letter-spacing:.08em; color:var(--muted);
    margin:30px 0 6px; font-weight:600; }}
  .hint {{ color:var(--muted); font-size:13px; margin:0 0 10px; }}
  .panel {{ background:var(--surface); border:1px solid var(--line); border-radius:14px; padding:14px 16px; }}
  .chart-svg {{ width:100%; height:auto; display:block; overflow:visible; }}
  .chart-svg .axis {{ stroke:var(--axis); stroke-width:1; }}
  .chart-svg .s-old {{ fill:var(--old); }}
  .chart-svg .s-old.now {{ fill:#86b6ef; }}
  .chart-svg .s-new {{ fill:var(--new); }}
  .chart-svg .val {{ fill:var(--ink); font-size:12px; font-weight:600; }}
  .chart-svg .tick {{ fill:var(--muted); font-size:11px; }}
  .chart-svg .hit {{ fill:transparent; }}
  .chart-svg .hit:hover {{ fill:rgba(255,255,255,.04); }}
  .legend {{ display:flex; gap:16px; font-size:12px; color:var(--ink2); margin-bottom:8px; }}
  .legend i {{ display:inline-block; width:10px; height:10px; border-radius:3px; margin-right:6px;
    vertical-align:-1px; }}
  .feed {{ list-style:none; margin:0; padding:0; max-height:260px; overflow:auto; }}
  .feed li {{ display:flex; align-items:center; gap:10px; padding:7px 0; font-size:14px; }}
  .feed li+li {{ border-top:1px solid #1d212c; }}
  .feed time {{ color:var(--muted); font-variant-numeric:tabular-nums; min-width:92px; font-size:13px; }}
  .feed .what {{ flex:1; }}
  .feed .ver {{ color:var(--muted); font-size:12px; }}
  .dot {{ width:8px; height:8px; border-radius:50%; flex:none; }}
  .d-new {{ background:var(--new); }} .d-upd {{ background:var(--upd); }}
  .feed-empty {{ color:var(--muted); font-size:14px; }}
  .grid {{ display:grid; grid-template-columns:3fr 2fr; gap:16px; }}
  table {{ width:100%; border-collapse:collapse; background:var(--surface);
    border:1px solid var(--line); border-radius:14px; overflow:hidden; }}
  th,td {{ text-align:left; padding:9px 14px; font-size:14px; }}
  th {{ color:var(--muted); font-weight:500; font-size:12px; background:#12141c; }}
  tr+tr td {{ border-top:1px solid #1d212c; }}
  td:not(:first-child),th:not(:first-child) {{ text-align:right; font-variant-numeric:tabular-nums; }}
  .muted {{ color:#6b7083; }}
  details {{ margin-top:30px; color:var(--ink2); font-size:14px; }}
  summary {{ cursor:pointer; color:var(--muted); font-size:13px; }}
  details ul {{ padding-left:18px; margin:10px 0 0; }}
  details li {{ margin:6px 0; }}
  @media (max-width:700px) {{ .tiles {{ grid-template-columns:repeat(2,1fr); }} .grid {{ grid-template-columns:1fr; }} }}
</style></head><body><div class="wrap">
  <h1>Axiom — сколько людей пользуется</h1>
  <div class="top-sub">Обновлено в <time data-utc="{utc_iso(now)}" data-fmt="hm">{now:%H:%M} UTC</time>.
    Страница обновляется сама раз в минуту.</div>

  <div class="tiles">{tiles}</div>
  <div class="tiles small">{small}</div>

  <h2>Что происходило</h2>
  <p class="hint">Новые установки и обновления за последние двое суток, с точностью до 10 минут.</p>
  <div class="panel">{feed_html(s["feed"])}</div>

  <h2>По часам, последние сутки</h2>
  <p class="hint">Сколько браузеров было открыто в каждый час. Открытый браузер отмечается раз в час,
    поэтому столбик ≈ число людей. Последний столбик — текущий час, он ещё растёт.</p>
  <div class="panel">{chart_hours(s["hours"])}</div>

  <h2>По дням, 30 дней</h2>
  <p class="hint">Сколько разных людей открывали браузер в каждый день.</p>
  <div class="panel">
    <div class="legend"><span><i style="background:var(--old)"></i>уже пользовались</span>
      <span><i style="background:var(--new)"></i>новые установки</span></div>
    {chart_days(s["days"])}
  </div>

  <div class="grid">
    <div><h2>Версии</h2><table><thead><tr><th>версия</th><th>людей сегодня</th>
      <th>обновились на неё</th></tr></thead><tbody>{vrows}</tbody></table></div>
    <div><h2>Системы</h2><table><thead><tr><th>система</th><th>людей сегодня</th></tr></thead>
      <tbody>{orows}</tbody></table></div>
  </div>

  <details><summary>Как это считается</summary><ul>
    <li>Пока браузер открыт, он раз в час шлёт короткую отметку: версию, систему и пару флажков
      («первый раз сегодня», «новая установка», «только что обновился»). Ни имён, ни адресов, ни IP.</li>
    <li>«Сейчас» — сколько отметок пришло за последний час. Каждый открытый браузер даёт одну,
      перезапуск может дать лишнюю, поэтому ≈.</li>
    <li>Сутки, недели и месяцы считаются по UTC: сутки начинаются в
      <time data-utc="{midnight}" data-fmt="hm">00:00 UTC</time> по твоему времени.</li>
    <li>Отметок сегодня: {pt}. Это не люди: один человек, который просидел в браузере 5 часов, даёт 5 отметок.</li>
    <li>Об обновлениях браузер сообщает начиная с версии 0.4.0. Кто обновится на неё с более
      старой версии, тоже попадёт в счёт.</li>
    <li>«Людей сегодня» по версиям в сумме может быть больше, чем «Сегодня»: тот, кто обновился
      днём, был сегодня на двух версиях.</li>
  </ul></details>
</div>
<script>
(() => {{
  const pad = n => String(n).padStart(2, "0");
  const hm = d => pad(d.getHours()) + ":" + pad(d.getMinutes());
  const sameDay = (a, b) => a.toDateString() === b.toDateString();
  const today = new Date(), yest = new Date(Date.now() - 864e5);
  const fmt = {{
    hm, h: d => pad(d.getHours()),
    now: () => "сейчас",
    feed: d => (sameDay(d, today) ? "сегодня " : sameDay(d, yest) ? "вчера " : d.toLocaleDateString("ru-RU") + " ") + hm(d),
    "hour-tip": (d, el) => {{
      const end = new Date(d.getTime() + 36e5);
      return pad(d.getHours()) + ":00–" + pad(end.getHours()) + ":00 — " + el.dataset.n;
    }},
  }};
  for (const el of document.querySelectorAll("[data-utc]")) {{
    const d = new Date(el.dataset.utc), f = fmt[el.dataset.fmt];
    if (!isNaN(d) && f) el.textContent = f(d, el);
  }}
}})();
</script>
</body></html>"""


# ---------- HTTP ----------

class Handler(BaseHTTPRequestHandler):
    server_version = "axiom-stats"
    # Таймаут сокета: клиент, приславший Content-Length и замолчавший,
    # иначе держал бы поток вечно.
    timeout = 10

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
        if KEY and hmac.compare_digest(self.path.rstrip("/").encode(), f"/d/{KEY}".encode()):
            return self._send(200, dashboard().encode("utf-8"),
                              "text/html; charset=utf-8")
        self._send(404, b"not found")

    do_HEAD = do_GET


def main():
    init()
    srv = ThreadingHTTPServer((HOST, PORT), Handler)
    print(f"axiom-stats на http://{HOST}:{PORT} (база {DB})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
