"""Магазин плагинов Axiom: каталог, установка и публикация.

Браузер:
  GET  /store/index           каталог: опубликованные плагины, последние одобренные версии
  GET  /store/p/<id>          сам плагин: {"files": {путь: base64}} — те же байты, что
                              посчитаны в sha256 каталога (браузер сверяет)
  POST /store/submit          прислать плагин на проверку (новый или новую версию)
  POST /store/status          что с присланными: на проверке, опубликован, отклонён

Панель (только с ключом): /d/<ключ>/store — плагины на проверке с их кодом, кнопки
«Опубликовать» и «Отклонить»; опубликованные — «Снять» и «Вернуть».

Плагин в магазине появляется только после проверки человеком: код плагина работает
на чужих страницах, и без проверки магазин был бы раздачей чего угодно.

Кто может прислать новую версию плагина: тот, у кого ключ автора. Ключ (128 бит)
браузер придумывает сам при первой публикации и хранит у себя; сервер помнит только
его sha256. Плагины Axiom (official) обновляются только командой на сервере:
  python3 stats.py store-import <папка с плагинами>
"""

import base64
import hashlib
import hmac
import json
import os
import re
from html import escape

ID_RE = re.compile(r"[a-z0-9][a-z0-9-]{1,39}")
VERSION_RE = re.compile(r"[0-9A-Za-z.+-]{1,32}")
KEY_RE = re.compile(r"[0-9a-f]{32}")
# Что может лежать в папке плагина: код, стили, данные, картинки, описание.
EXTS = {"js", "mjs", "css", "json", "png", "svg", "webp", "jpg", "jpeg", "gif", "ico", "md", "txt"}
IMAGE_TYPES = {"png": "image/png", "svg": "image/svg+xml", "webp": "image/webp",
               "jpg": "image/jpeg", "jpeg": "image/jpeg", "gif": "image/gif", "ico": "image/x-icon"}
MAX_FILES = 100
# Как в браузере: весь плагин до 5 МБ (код и стили), файл до 2 МБ.
MAX_BYTES = 5 * 1024 * 1024
MAX_FILE = 2 * 1024 * 1024
MAX_ICON = 64 * 1024
# base64 — на треть больше самих файлов, плюс поля.
MAX_SUBMIT_BODY = 7 * 1024 * 1024 + 64 * 1024
MAX_STATUS_BODY = 16 * 1024
MAX_STATUS = 100
MAX_PENDING = 300
MAX_AUTHOR = 100
MAX_CONTACT = 200
MAX_NOTE = 1000
MAX_REASON = 1000
MAX_SITES = 20
PERMISSIONS = {"notify", "tabs"}
# Код на странице проверки: больше показываем ссылкой не на что — просто обрезаем.
CODE_PREVIEW = 200 * 1024

STATUS = {"pending": "на проверке", "approved": "опубликован", "rejected": "отклонён"}


def init(con):
    """Таблицы магазина (внутри транзакции `init` сборщика)."""
    con.execute(
        """CREATE TABLE IF NOT EXISTS store_plugins(
            id TEXT PRIMARY KEY,
            key_hash TEXT NOT NULL DEFAULT '',
            official INTEGER NOT NULL DEFAULT 0,
            hidden INTEGER NOT NULL DEFAULT 0,
            installs INTEGER NOT NULL DEFAULT 0,
            created TEXT NOT NULL)"""
    )
    # Версии: bundle — плагин целиком (байты ответа /store/p), meta — строка каталога.
    con.execute(
        """CREATE TABLE IF NOT EXISTS store_versions(
            n INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL, version TEXT NOT NULL, status TEXT NOT NULL,
            bundle BLOB NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL,
            meta TEXT NOT NULL, author TEXT NOT NULL DEFAULT '',
            contact TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
            at TEXT NOT NULL, reviewed_at TEXT, reason TEXT)"""
    )
    con.execute("CREATE INDEX IF NOT EXISTS store_versions_id ON store_versions(id)")


def key_hash(key):
    return hashlib.sha256(key.encode()).hexdigest()


def valid_path(path):
    """Путь файла внутри папки плагина: как `manifest::file_name` в браузере."""
    if not isinstance(path, str) or not 0 < len(path) <= 200:
        return False
    if "\\" in path or ":" in path or path.startswith("/"):
        return False
    parts = path.split("/")
    if len(parts) > 8 or any(p in ("", ".", "..") or p.startswith(".") for p in parts):
        return False
    ext = path.rsplit(".", 1)[-1].lower() if "." in parts[-1] else ""
    return ext in EXTS


def check_bundle(files):
    """Файлы плагина {путь: base64} → (байты пакета, манифест, {путь: байты}) или текст ошибки."""
    if not isinstance(files, dict) or not files:
        return "нет файлов"
    if len(files) > MAX_FILES:
        return f"больше {MAX_FILES} файлов"
    raw, total = {}, 0
    for path, b64 in files.items():
        if not valid_path(path):
            return f"файл «{str(path)[:80]}»: недопустимое имя или тип"
        if not isinstance(b64, str):
            return f"файл «{path}»: неверный формат"
        try:
            data = base64.b64decode(b64, validate=True)
        except ValueError:
            return f"файл «{path}»: неверный base64"
        if len(data) > MAX_FILE:
            return f"файл «{path}» больше 2 МБ"
        total += len(data)
        if total > MAX_BYTES:
            return "плагин больше 5 МБ"
        raw[path] = data
    if "plugin.json" not in raw:
        return "нет plugin.json"
    try:
        manifest = json.loads(raw["plugin.json"].decode("utf-8-sig"))
    except (ValueError, UnicodeDecodeError):
        return "plugin.json не читается"
    if not isinstance(manifest, dict):
        return "plugin.json — не объект"
    if manifest.get("api") != 1:
        return "магазин принимает плагины с \"api\": 1"
    pid, ver, name = manifest.get("id"), manifest.get("version"), manifest.get("name")
    if not isinstance(pid, str) or not ID_RE.fullmatch(pid):
        return "неверный id"
    if not isinstance(ver, str) or not VERSION_RE.fullmatch(ver):
        return "неверная version"
    if not isinstance(name, str) or not 0 < len(name.strip()) <= 60:
        return "неверное name"
    scripts = manifest.get("content_scripts")
    if not isinstance(scripts, list) or not scripts:
        return "нет content_scripts"
    # Файлы из манифеста должны быть в пакете: иначе браузер не загрузит плагин.
    for cs in scripts:
        if not isinstance(cs, dict):
            return "неверные content_scripts"
        for key in ("js", "css"):
            for f in cs.get(key) or []:
                if f not in raw:
                    return f"в пакете нет файла «{str(f)[:80]}» из plugin.json"
    bundle = json.dumps(
        {"files": {p: base64.b64encode(raw[p]).decode() for p in sorted(raw)}},
        separators=(",", ":"), ensure_ascii=True).encode()
    return bundle, manifest, raw


def text(v, limit):
    return v.strip()[:limit] if isinstance(v, str) else ""


def meta_of(manifest, raw):
    """Строка каталога: что показать в магазине, не открывая сам плагин."""
    sites = []
    for cs in manifest.get("content_scripts") or []:
        for m in (cs.get("matches") if isinstance(cs, dict) else None) or []:
            if isinstance(m, str) and m not in sites and len(sites) < MAX_SITES:
                sites.append(m[:200])
    perms = [p for p in manifest.get("permissions") or [] if p in PERMISSIONS]
    icon = None
    rel = manifest.get("icon")
    if isinstance(rel, str) and rel in raw and len(raw[rel]) <= MAX_ICON:
        mime = IMAGE_TYPES.get(rel.rsplit(".", 1)[-1].lower())
        if mime:
            icon = f"data:{mime};base64,{base64.b64encode(raw[rel]).decode()}"
    action = manifest.get("action")
    settings = manifest.get("settings")
    homepage = text(manifest.get("homepage"), 300)
    return {
        "name": text(manifest.get("name"), 60),
        "description": text(manifest.get("description"), 400),
        "author": text(manifest.get("author"), MAX_AUTHOR),
        "homepage": homepage if re.match(r"https?://", homepage) else "",
        "permissions": sorted(set(perms)),
        "sites": sites,
        "action": isinstance(action, dict),
        "settings": len(settings) if isinstance(settings, list) else 0,
        "icon": icon,
    }


def submit(con, payload, now):
    """Плагин на проверку. → (HTTP-код, ответ JSON)."""
    if not isinstance(payload, dict):
        return 400, {"error": "неверный запрос"}
    key = payload.get("key")
    if not isinstance(key, str) or not KEY_RE.fullmatch(key):
        return 400, {"error": "нет ключа автора"}
    checked = check_bundle(payload.get("files"))
    if isinstance(checked, str):
        return 400, {"error": checked}
    bundle, manifest, raw = checked
    pid, ver = manifest["id"], manifest["version"]
    with con:
        row = con.execute("SELECT key_hash, official FROM store_plugins WHERE id=?", (pid,)).fetchone()
        if row:
            stored, official = row
            if official or not stored:
                return 403, {"error": f"id «{pid}» занят плагином Axiom — возьмите другой id"}
            if not hmac.compare_digest(stored, key_hash(key)):
                return 403, {"error": f"id «{pid}» уже занят другим автором — возьмите другой id"}
        if con.execute("SELECT 1 FROM store_versions WHERE id=? AND version=?", (pid, ver)).fetchone():
            return 409, {"error": f"версия {ver} уже была прислана — увеличьте version в plugin.json"}
        pending = con.execute("SELECT COUNT(*) FROM store_versions WHERE status='pending'").fetchone()[0]
        if pending >= MAX_PENDING:
            return 503, {"error": "очередь проверки переполнена, попробуйте позже"}
        if not row:
            con.execute("INSERT INTO store_plugins(id, key_hash, created) VALUES(?,?,?)",
                        (pid, key_hash(key), now))
        # Прежняя версия того же автора, ещё не проверенная, больше не нужна.
        con.execute("DELETE FROM store_versions WHERE id=? AND status='pending'", (pid,))
        cur = con.execute(
            """INSERT INTO store_versions(id, version, status, bundle, sha256, size, meta,
                   author, contact, note, at) VALUES(?,?,?,?,?,?,?,?,?,?,?)""",
            (pid, ver, "pending", bundle, hashlib.sha256(bundle).hexdigest(),
             sum(len(b) for b in raw.values()),
             json.dumps(meta_of(manifest, raw), ensure_ascii=False),
             text(payload.get("author"), MAX_AUTHOR) or text(manifest.get("author"), MAX_AUTHOR),
             text(payload.get("contact"), MAX_CONTACT), text(payload.get("note"), MAX_NOTE), now))
    return 200, {"status": "pending", "id": pid, "version": ver, "n": cur.lastrowid}


def published(con):
    """Последняя одобренная версия каждого видимого плагина."""
    return con.execute(
        """SELECT v.id, v.version, v.sha256, v.size, v.meta, v.reviewed_at,
                  p.installs, p.official
           FROM store_versions v JOIN store_plugins p ON p.id = v.id
           WHERE p.hidden=0 AND v.n = (SELECT MAX(n) FROM store_versions
                                       WHERE id=v.id AND status='approved')
           ORDER BY p.official DESC, p.installs DESC, v.id""").fetchall()


def index(con):
    items = []
    for pid, ver, sha, size, meta, at, installs, official in published(con):
        item = json.loads(meta)
        item.update({"id": pid, "version": ver, "sha256": sha, "size": size,
                     "updated": at or "", "installs": installs, "official": bool(official)})
        items.append(item)
    return json.dumps({"plugins": items}, ensure_ascii=False, separators=(",", ":")).encode()


def download(con, pid):
    """Пакет последней одобренной версии (и +1 установка) или None."""
    if not ID_RE.fullmatch(pid):
        return None
    with con:
        row = con.execute(
            """SELECT v.bundle FROM store_versions v JOIN store_plugins p ON p.id = v.id
               WHERE v.id=? AND p.hidden=0 AND v.status='approved'
               ORDER BY v.n DESC LIMIT 1""", (pid,)).fetchone()
        if not row:
            return None
        con.execute("UPDATE store_plugins SET installs = installs + 1 WHERE id=?", (pid,))
    return bytes(row[0])


def status(con, payload):
    """Что с плагинами, которые присылал этот браузер: {"items": [{id, key}]}."""
    items = payload.get("items") if isinstance(payload, dict) else None
    if not isinstance(items, list) or len(items) > MAX_STATUS:
        return None
    out = {}
    for it in items:
        if not isinstance(it, dict):
            return None
        pid, key = it.get("id"), it.get("key")
        if not isinstance(pid, str) or not ID_RE.fullmatch(pid) or not isinstance(key, str) \
                or not KEY_RE.fullmatch(key):
            return None
        row = con.execute("SELECT key_hash, hidden FROM store_plugins WHERE id=?", (pid,)).fetchone()
        if not row or not row[0] or not hmac.compare_digest(row[0], key_hash(key)):
            continue
        last = con.execute(
            "SELECT version, status, reason FROM store_versions WHERE id=? ORDER BY n DESC LIMIT 1",
            (pid,)).fetchone()
        live = con.execute(
            """SELECT version FROM store_versions WHERE id=? AND status='approved'
               ORDER BY n DESC LIMIT 1""", (pid,)).fetchone()
        if last:
            out[pid] = {"version": last[0], "status": last[1], "reason": last[2] or "",
                        "published": None if row[1] or not live else live[0]}
    return out


def review(con, n, form, now):
    """Решение модератора по присланной версии. → текст ошибки или None."""
    action = form.get("do")
    reason = text(form.get("reason"), MAX_REASON)
    with con:
        row = con.execute("SELECT status FROM store_versions WHERE n=?", (n,)).fetchone()
        if not row:
            return "нет такой версии"
        if action == "approve":
            con.execute("UPDATE store_versions SET status='approved', reviewed_at=?, reason=NULL WHERE n=?",
                        (now, n))
        elif action == "reject":
            if not reason:
                return "напишите причину — автор её увидит"
            con.execute("UPDATE store_versions SET status='rejected', reviewed_at=?, reason=? WHERE n=?",
                        (now, reason, n))
        else:
            return "неизвестное действие"
    return None


def set_hidden(con, pid, hidden):
    with con:
        con.execute("UPDATE store_plugins SET hidden=? WHERE id=?", (1 if hidden else 0, pid))


def import_dir(con, folder, now):
    """Плагины Axiom из папки репозитория (`store/plugins`): сразу опубликованы.
    Та же версия с другими файлами заменяется (исправление без смены номера)."""
    done = []
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if not os.path.isfile(os.path.join(path, "plugin.json")):
            continue
        files = {}
        for root, _, names in os.walk(path):
            for f in names:
                full = os.path.join(root, f)
                rel = os.path.relpath(full, path).replace(os.sep, "/")
                if valid_path(rel):
                    with open(full, "rb") as fh:
                        files[rel] = base64.b64encode(fh.read()).decode()
        checked = check_bundle(files)
        if isinstance(checked, str):
            raise SystemExit(f"{name}: {checked}")
        bundle, manifest, raw = checked
        pid, ver = manifest["id"], manifest["version"]
        sha = hashlib.sha256(bundle).hexdigest()
        meta = json.dumps(meta_of(manifest, raw), ensure_ascii=False)
        size = sum(len(b) for b in raw.values())
        with con:
            row = con.execute("SELECT official FROM store_plugins WHERE id=?", (pid,)).fetchone()
            if row and not row[0]:
                raise SystemExit(f"{pid}: id уже занят плагином другого автора")
            if not row:
                con.execute("INSERT INTO store_plugins(id, official, created) VALUES(?,1,?)", (pid, now))
            old = con.execute("SELECT n, sha256 FROM store_versions WHERE id=? AND version=?",
                              (pid, ver)).fetchone()
            if old and old[1] == sha:
                done.append(f"{pid} {ver}: без изменений")
                continue
            if old:
                con.execute("""UPDATE store_versions SET bundle=?, sha256=?, size=?, meta=?,
                                   status='approved', reviewed_at=? WHERE n=?""",
                            (bundle, sha, size, meta, now, old[0]))
                done.append(f"{pid} {ver}: обновлён")
            else:
                con.execute(
                    """INSERT INTO store_versions(id, version, status, bundle, sha256, size, meta,
                           author, at, reviewed_at) VALUES(?,?,?,?,?,?,?,?,?,?)""",
                    (pid, ver, "approved", bundle, sha, size, meta, "Axiom", now, now))
                done.append(f"{pid} {ver}: опубликован")
    return done


# ---------- страница проверки в панели ----------

def pending_count(con):
    return con.execute("SELECT COUNT(*) FROM store_versions WHERE status='pending'").fetchone()[0]


def code_html(bundle):
    files = json.loads(bytes(bundle))["files"]
    out = []
    for path in sorted(files, key=lambda p: (p != "plugin.json", p)):
        data = base64.b64decode(files[path])
        ext = path.rsplit(".", 1)[-1].lower()
        if ext in IMAGE_TYPES:
            body = f'<p class="muted">картинка, {len(data)} байт</p>'
        else:
            shown = data[:CODE_PREVIEW].decode("utf-8", "replace")
            cut = (f'<p class="muted">… показаны первые {CODE_PREVIEW // 1024} КБ из '
                   f'{len(data) // 1024} КБ</p>') if len(data) > CODE_PREVIEW else ""
            body = f"<pre>{escape(shown)}</pre>{cut}"
        out.append(f'<details{" open" if path.endswith((".json", ".js", ".css")) else ""}>'
                   f"<summary><b>{escape(path)}</b> <span class=\"muted\">{len(data)} байт</span>"
                   f"</summary>{body}</details>")
    return "".join(out)


def page(con, base, css, nonce, error=None):
    """Страница магазина в панели: на проверке (с кодом) и опубликованные."""
    pend = con.execute(
        """SELECT v.n, v.id, v.version, v.meta, v.author, v.contact, v.note, v.at, v.size, v.bundle,
                  (SELECT version FROM store_versions WHERE id=v.id AND status='approved'
                   ORDER BY n DESC LIMIT 1)
           FROM store_versions v WHERE v.status='pending' ORDER BY v.n""").fetchall()
    items = []
    for n, pid, ver, meta, author, contact, note, at, size, bundle, live in pend:
        m = json.loads(meta)
        perms = ", ".join(m.get("permissions") or []) or "нет"
        sites = ", ".join(m.get("sites") or [])
        was = f" (сейчас в магазине {escape(live)})" if live else " (новый плагин)"
        items.append(f"""<section class="box sub">
  <h3>{escape(m.get("name", pid))} <span class="muted">{escape(pid)} {escape(ver)}{was}</span></h3>
  <p>{escape(m.get("description", ""))}</p>
  <p class="muted">Автор: {escape(author) or "—"} · связь: {escape(contact) or "—"} · {size} байт ·
    <time data-utc="{escape(at)}">{escape(at)}</time></p>
  <p class="muted">Сайты: {escape(sites)} · разрешения: {escape(perms)}</p>
  {f'<pre class="text">{escape(note)}</pre>' if note else ""}
  {code_html(bundle)}
  <form method="post" action="{base}/{int(n)}">
    <input type="text" name="reason" placeholder="Причина отказа — её увидит автор" maxlength="{MAX_REASON}">
    <div class="row"><button name="do" value="approve">Опубликовать</button>
      <button class="ghost" name="do" value="reject">Отклонить</button></div>
  </form>
</section>""")
    pub = []
    for pid, ver, sha, size, meta, at, installs, official in published(con):
        m = json.loads(meta)
        badge = '<span class="ans a-fixed">Axiom</span>' if official else ""
        pub.append(f"""<tr><td><b>{escape(m.get("name", pid))}</b> <span class="muted">{escape(pid)}</span>
  {badge}</td><td>{escape(ver)}</td>
  <td>{int(installs)}</td><td><form method="post" action="{base}/p/{escape(pid)}">
  <button class="ghost" name="do" value="hide">Снять</button></form></td></tr>""")
    hidden = con.execute(
        "SELECT id FROM store_plugins WHERE hidden=1 ORDER BY id").fetchall()
    hid = "".join(f"""<tr><td>{escape(pid)}</td><td colspan="2" class="muted">снят</td>
  <td><form method="post" action="{base}/p/{escape(pid)}"><button class="ghost" name="do" value="show">Вернуть</button>
  </form></td></tr>""" for (pid,) in hidden)
    err = f'<p class="err">{escape(error)}</p>' if error else ""
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Axiom — магазин плагинов</title>
<style>{css}
  .sub {{ margin:14px 0; display:grid; gap:8px; }}
  .sub h3 {{ margin:0; font-size:17px; }}
  .sub p {{ margin:0; }}
  .sub p.muted {{ color:var(--ink2); font-size:13.5px; }}
  details {{ border:1px solid var(--line); border-radius:12px; padding:8px 12px; }}
  details pre {{ margin-top:8px; max-height:520px; overflow:auto; }}
  summary {{ cursor:pointer; }}
  table {{ width:100%; border-collapse:collapse; }}
  td, th {{ text-align:left; padding:8px 6px; border-top:1px solid var(--line); }}
  td form {{ display:inline; }}
</style></head><body><div class="wrap">
  <a href="{base[:-len('/store')]}">← Вся статистика</a>
  <h1>Магазин плагинов</h1>
  <div class="top-sub">Присланное появляется в магазине только после «Опубликовать». Код плагина
    работает на страницах сайтов у тех, кто его поставит: смотрите, что он делает и куда ходит.</div>
  {err}
  <h2>На проверке: {len(pend)}</h2>
  {"".join(items) or '<p class="muted">Ничего не ждёт проверки.</p>'}
  <h2>В магазине: {len(pub)}</h2>
  <div class="box"><table><tr><th>Плагин</th><th>Версия</th><th>Установок</th><th></th></tr>
  {"".join(pub)}{hid}</table></div>
</div>
<script nonce="{escape(nonce)}">
for (const el of document.querySelectorAll("[data-utc]")) {{
  const d = new Date(el.dataset.utc);
  if (!isNaN(d)) el.textContent = d.toLocaleString("ru-RU");
}}
</script>
</body></html>"""
