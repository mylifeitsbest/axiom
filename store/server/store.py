#!/usr/bin/env python3
"""Магазин плагинов Axiom: сайт для разработчиков и каталог для браузера.

Отдельная служба: своя база, свой адрес, свой ключ администратора. Со сборщиком
статистики (server/stats.py) у неё нет ничего общего, кроме машины.

Браузер (магазин в «Настройки → Плагины»):
  GET  /api/index        каталог: опубликованные плагины, последние одобренные версии
  GET  /api/p/<id>       сам плагин: {"files": {путь: base64}}; sha256 этих байтов
                         есть в каталоге, браузер сверяет

Сайт:
  /                      что это и какие плагины уже есть
  /apply                 заявка «Стать разработчиком» → секретная ссылка на её статус
  /status/<ссылка>       что с заявкой; после одобрения — личный ключ разработчика
  /login                 вход по ключу (разработчика или администратора)
  /dev                   кабинет разработчика: свои плагины, загрузка новых и версий
  /admin/...             администратор: заявки, разработчики, проверка, каталог

Плагин разработчика попадает в каталог только после проверки администратором.
Ключ администратора открывает всё сразу: его плагины публикуются без проверки
(как плагины Axiom), он одобряет заявки, выдаёт и отзывает ключи, проверяет код.

Ключи хранятся только хэшем. Новый ключ разработчика виден на странице статуса
заявки, пока разработчик ни разу не вошёл, и администратору — чтобы передать.

Переменные окружения:
  AXIOM_STORE_DB         путь к базе (по умолчанию ./store.db)
  AXIOM_STORE_HOST       адрес прослушивания (по умолчанию 127.0.0.1 — за Caddy)
  AXIOM_STORE_PORT       порт (по умолчанию 8788)
  AXIOM_STORE_ADMIN_KEY  ключ администратора (не короче 24 знаков; без него админки нет)
  AXIOM_STORE_INSECURE   1 — куки без Secure (только для проверки на http://127.0.0.1)
  AXIOM_STORE_OFFICIAL   папка с плагинами Axiom: при запуске они публикуются (как `import`)

Плагины Axiom из репозитория: python3 store.py import <папка store/plugins>
Зависимостей нет: только стандартная библиотека.
"""

import base64
import hashlib
import hmac
import io
import json
import os
import re
import secrets
import sqlite3
import sys
import threading
import time
import zipfile
from datetime import datetime, timedelta, timezone
from html import escape
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

DB = os.environ.get("AXIOM_STORE_DB", "store.db")
HOST = os.environ.get("AXIOM_STORE_HOST", "127.0.0.1")
PORT = int(os.environ.get("AXIOM_STORE_PORT", "8788"))
ADMIN_KEY = os.environ.get("AXIOM_STORE_ADMIN_KEY", "")
INSECURE = os.environ.get("AXIOM_STORE_INSECURE") == "1"

ID_RE = re.compile(r"[a-z0-9][a-z0-9-]{1,39}")
VERSION_RE = re.compile(r"[0-9A-Za-z.+-]{1,32}")
DEV_KEY_RE = re.compile(r"axd_[0-9a-f]{40}")
TOKEN_RE = re.compile(r"[A-Za-z0-9_-]{20,64}")
# Что может лежать в папке плагина: код, стили, данные, картинки, описание.
EXTS = {"js", "mjs", "css", "json", "png", "svg", "webp", "jpg", "jpeg", "gif", "ico", "md", "txt"}
IMAGE_TYPES = {"png": "image/png", "svg": "image/svg+xml", "webp": "image/webp",
               "jpg": "image/jpeg", "jpeg": "image/jpeg", "gif": "image/gif", "ico": "image/x-icon"}
PERMISSIONS = {"notify", "tabs"}
# Как в браузере: до 100 файлов, файл до 2 МБ, весь плагин до 5 МБ.
MAX_FILES = 100
MAX_FILE = 2 * 1024 * 1024
MAX_BYTES = 5 * 1024 * 1024
MAX_ICON = 64 * 1024
MAX_SITES = 20
# Загрузка: плагин в base64 (на треть больше) и поля.
MAX_UPLOAD = 7 * 1024 * 1024 + 64 * 1024
MAX_FORM = 16 * 1024
MAX_PENDING = 300
MAX_APPS_PENDING = 500
CODE_PREVIEW = 200 * 1024
SESSION_DAYS = 7
COOKIE = "axs"
STATUS = {"pending": "на проверке", "approved": "опубликован", "rejected": "отклонён"}
# Частота с одного адреса: (сколько, за сколько секунд).
LIMITS = {"apply": (5, 3600), "login": (20, 3600), "upload": (30, 3600),
          "index": (240, 3600), "download": (300, 3600), "page": (600, 3600)}
MAX_BUCKETS = 50_000
MAX_CONNECTIONS = 64


def utcnow():
    return datetime.now(timezone.utc)


def iso(t=None):
    return (t or utcnow()).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def db():
    con = sqlite3.connect(DB, timeout=10)
    con.execute("PRAGMA busy_timeout=5000")
    con.execute("PRAGMA foreign_keys=ON")
    return con


def init():
    con = db()
    with con:
        # Заявки «Стать разработчиком». token_hash — секретная ссылка на статус.
        con.execute(
            """CREATE TABLE IF NOT EXISTS applications(
                id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL,
                name TEXT NOT NULL, contact TEXT NOT NULL, about TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE, status TEXT NOT NULL DEFAULT 'pending',
                reason TEXT, dev INTEGER, decided_at TEXT)""")
        # Разработчики. key_once — новый ключ, пока разработчик его не забрал (не вошёл).
        con.execute(
            """CREATE TABLE IF NOT EXISTS developers(
                id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
                contact TEXT NOT NULL DEFAULT '', key_hash TEXT NOT NULL UNIQUE,
                key_once TEXT, active INTEGER NOT NULL DEFAULT 1, created TEXT NOT NULL)""")
        # Плагины: dev — чей (NULL — Axiom), official — от Axiom, hidden — снят администратором.
        con.execute(
            """CREATE TABLE IF NOT EXISTS plugins(
                id TEXT PRIMARY KEY, dev INTEGER, official INTEGER NOT NULL DEFAULT 0,
                hidden INTEGER NOT NULL DEFAULT 0, installs INTEGER NOT NULL DEFAULT 0,
                created TEXT NOT NULL)""")
        # Версии: bundle — плагин целиком (байты ответа /api/p), meta — строка каталога.
        con.execute(
            """CREATE TABLE IF NOT EXISTS versions(
                n INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL, version TEXT NOT NULL,
                status TEXT NOT NULL, bundle BLOB NOT NULL, sha256 TEXT NOT NULL,
                size INTEGER NOT NULL, meta TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
                at TEXT NOT NULL, reviewed_at TEXT, reason TEXT)""")
        con.execute("CREATE INDEX IF NOT EXISTS versions_id ON versions(id)")
        con.execute(
            """CREATE TABLE IF NOT EXISTS sessions(
                hash TEXT PRIMARY KEY, who TEXT NOT NULL, csrf TEXT NOT NULL,
                expires REAL NOT NULL)""")
    con.close()


# ---------- пакет плагина ----------

def valid_path(path):
    """Путь файла внутри папки плагина: как `market::valid_path` в браузере."""
    if not isinstance(path, str) or not 0 < len(path) <= 200:
        return False
    if "\\" in path or ":" in path or path.startswith("/"):
        return False
    parts = path.split("/")
    if len(parts) > 8 or any(p in ("", ".", "..") or p.startswith(".") for p in parts):
        return False
    ext = path.rsplit(".", 1)[-1].lower() if "." in parts[-1] else ""
    return ext in EXTS


def from_zip(data):
    """Архив .zip → {путь: байты} или текст ошибки. Папка внутри архива (zip самой
    папки плагина) снимается; скрытые файлы и чужие типы пропускаются."""
    try:
        zf = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile:
        return "это не zip-архив"
    infos = [i for i in zf.infolist() if not i.is_dir()]
    names = [i.filename.replace("\\", "/") for i in infos]
    tops = {n.split("/", 1)[0] for n in names}
    strip = len(tops) == 1 and all("/" in n for n in names)
    out, total = {}, 0
    for info, name in zip(infos, names):
        if strip:
            name = name.split("/", 1)[1]
        if not valid_path(name):
            continue
        # Размер из заголовка архива — ещё до распаковки: zip-бомба не развернётся.
        if info.file_size > MAX_FILE:
            return f"файл «{name}» больше 2 МБ"
        total += info.file_size
        if total > MAX_BYTES:
            return "плагин больше 5 МБ"
        if len(out) >= MAX_FILES:
            return f"больше {MAX_FILES} файлов"
        try:
            with zf.open(info) as f:
                body = f.read(MAX_FILE + 1)
        except (zipfile.BadZipFile, RuntimeError, OSError, EOFError):
            return f"файл «{name}» не распаковывается"
        if len(body) > MAX_FILE:
            return f"файл «{name}» больше 2 МБ"
        out[name] = body
    return out


def from_b64(files):
    """{путь: base64} (загрузка папкой со страницы) → {путь: байты} или текст ошибки."""
    if not isinstance(files, dict):
        return "нет файлов"
    if len(files) > MAX_FILES:
        return f"больше {MAX_FILES} файлов"
    out, total = {}, 0
    for path, b64 in files.items():
        if not valid_path(path):
            continue
        if not isinstance(b64, str):
            return f"файл «{path}»: неверный формат"
        try:
            data = base64.b64decode(b64, validate=True)
        except ValueError:
            return f"файл «{path}» повреждён"
        if len(data) > MAX_FILE:
            return f"файл «{path}» больше 2 МБ"
        total += len(data)
        if total > MAX_BYTES:
            return "плагин больше 5 МБ"
        out[path] = data
    return out


def check_bundle(raw):
    """{путь: байты} → (байты пакета, манифест) или текст ошибки."""
    if not raw:
        return "в плагине нет подходящих файлов"
    if "plugin.json" not in raw:
        return "нет plugin.json (он должен лежать в корне папки плагина)"
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
        return "неверный id в plugin.json"
    if not isinstance(ver, str) or not VERSION_RE.fullmatch(ver):
        return "неверная version в plugin.json"
    if not isinstance(name, str) or not 0 < len(name.strip()) <= 60:
        return "неверное name в plugin.json"
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
    return bundle, manifest


def text(v, limit):
    return v.strip()[:limit] if isinstance(v, str) else ""


def meta_of(manifest, raw, author):
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
        "author": author[:100],
        "homepage": homepage if re.match(r"https?://", homepage) else "",
        "permissions": sorted(set(perms)),
        "sites": sites,
        "action": isinstance(action, dict),
        "settings": len(settings) if isinstance(settings, list) else 0,
        "icon": icon,
    }


def add_version(con, raw, who, note=""):
    """Новая версия плагина от `who` ('admin' или 'dev:<id>'). → (код, сообщение)."""
    checked = check_bundle(raw)
    if isinstance(checked, str):
        return 400, checked
    bundle, manifest = checked
    pid, ver = manifest["id"], manifest["version"]
    admin = who == "admin"
    dev = None if admin else int(who.split(":", 1)[1])
    now = iso()
    with con:
        row = con.execute("SELECT dev, official FROM plugins WHERE id=?", (pid,)).fetchone()
        if row and not admin and row[0] != dev:
            owner = "плагином Axiom" if row[1] else "другим разработчиком"
            return 403, f"id «{pid}» уже занят {owner} — возьмите другой id"
        if con.execute("SELECT 1 FROM versions WHERE id=? AND version=?", (pid, ver)).fetchone():
            return 409, f"версия {ver} уже была загружена — увеличьте version в plugin.json"
        if not admin:
            pending = con.execute("SELECT COUNT(*) FROM versions WHERE status='pending'").fetchone()[0]
            if pending >= MAX_PENDING:
                return 503, "очередь проверки переполнена, попробуйте позже"
        if row:
            author_dev = row[0]
        else:
            author_dev = dev
            con.execute("INSERT INTO plugins(id, dev, official, created) VALUES(?,?,?,?)",
                        (pid, dev, 1 if admin else 0, now))
        name = "Axiom"
        if author_dev is not None:
            name = (con.execute("SELECT name FROM developers WHERE id=?", (author_dev,))
                    .fetchone() or ("",))[0]
        meta = meta_of(manifest, raw, text(manifest.get("author"), 100) or name)
        # Прежняя непроверенная версия того же плагина больше не нужна.
        con.execute("DELETE FROM versions WHERE id=? AND status='pending'", (pid,))
        status = "approved" if admin else "pending"
        con.execute(
            """INSERT INTO versions(id, version, status, bundle, sha256, size, meta, note, at,
                   reviewed_at) VALUES(?,?,?,?,?,?,?,?,?,?)""",
            (pid, ver, status, bundle, hashlib.sha256(bundle).hexdigest(),
             sum(len(b) for b in raw.values()), json.dumps(meta, ensure_ascii=False),
             text(note, 1000), now, now if admin else None))
    if admin:
        return 200, f"«{meta['name']}» {ver} опубликован"
    return 200, f"«{meta['name']}» {ver} отправлен на проверку — в магазине появится после неё"


def import_dir(con, folder):
    """Плагины Axiom из папки репозитория: сразу опубликованы. Та же версия
    с другими файлами заменяется (исправление без смены номера)."""
    done = []
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if not os.path.isfile(os.path.join(path, "plugin.json")):
            continue
        raw = {}
        for root, _, names in os.walk(path):
            for f in names:
                full = os.path.join(root, f)
                rel = os.path.relpath(full, path).replace(os.sep, "/")
                if valid_path(rel):
                    with open(full, "rb") as fh:
                        raw[rel] = fh.read()
        checked = check_bundle(raw)
        if isinstance(checked, str):
            raise SystemExit(f"{name}: {checked}")
        bundle, manifest = checked
        pid, ver = manifest["id"], manifest["version"]
        digest = hashlib.sha256(bundle).hexdigest()
        meta = json.dumps(meta_of(manifest, raw, text(manifest.get("author"), 100) or "Axiom"),
                          ensure_ascii=False)
        size = sum(len(b) for b in raw.values())
        now = iso()
        with con:
            row = con.execute("SELECT official FROM plugins WHERE id=?", (pid,)).fetchone()
            if row and not row[0]:
                raise SystemExit(f"{pid}: id уже занят плагином разработчика")
            if not row:
                con.execute("INSERT INTO plugins(id, dev, official, created) VALUES(?,NULL,1,?)",
                            (pid, now))
            old = con.execute("SELECT n, sha256 FROM versions WHERE id=? AND version=?",
                              (pid, ver)).fetchone()
            if old and old[1] == digest:
                done.append(f"{pid} {ver}: без изменений")
                continue
            if old:
                con.execute("""UPDATE versions SET bundle=?, sha256=?, size=?, meta=?,
                                   status='approved', reviewed_at=? WHERE n=?""",
                            (bundle, digest, size, meta, now, old[0]))
                done.append(f"{pid} {ver}: обновлён")
            else:
                con.execute(
                    """INSERT INTO versions(id, version, status, bundle, sha256, size, meta, at,
                           reviewed_at) VALUES(?,?,?,?,?,?,?,?,?)""",
                    (pid, ver, "approved", bundle, digest, size, meta, now, now))
                done.append(f"{pid} {ver}: опубликован")
    return done


# ---------- каталог для браузера ----------

def published(con):
    """Последняя одобренная версия каждого видимого плагина."""
    return con.execute(
        """SELECT v.id, v.version, v.sha256, v.size, v.meta, v.reviewed_at,
                  p.installs, p.official
           FROM versions v JOIN plugins p ON p.id = v.id
           WHERE p.hidden=0 AND v.n = (SELECT MAX(n) FROM versions
                                       WHERE id=v.id AND status='approved')
           ORDER BY p.official DESC, p.installs DESC, v.id""").fetchall()


def catalog(con):
    items = []
    for pid, ver, digest, size, meta, at, installs, official in published(con):
        item = json.loads(meta)
        item.update({"id": pid, "version": ver, "sha256": digest, "size": size,
                     "updated": at or "", "installs": installs, "official": bool(official)})
        items.append(item)
    return items


def download(con, pid):
    """Пакет последней одобренной версии (и +1 установка) или None."""
    if not ID_RE.fullmatch(pid):
        return None
    with con:
        row = con.execute(
            """SELECT v.bundle FROM versions v JOIN plugins p ON p.id = v.id
               WHERE v.id=? AND p.hidden=0 AND v.status='approved'
               ORDER BY v.n DESC LIMIT 1""", (pid,)).fetchone()
        if not row:
            return None
        con.execute("UPDATE plugins SET installs = installs + 1 WHERE id=?", (pid,))
    return bytes(row[0])


# ---------- заявки, ключи, входы ----------

def new_dev_key():
    return "axd_" + secrets.token_hex(20)


def apply(con, form):
    """Заявка «Стать разработчиком». → (секретный ключ ссылки на статус, None) или (None, ошибка)."""
    if form.get("website"):
        return None, "заявка не принята"  # ловушка для ботов: поле скрыто от людей
    name = text(form.get("name"), 80)
    contact = text(form.get("contact"), 200)
    about = text(form.get("about"), 2000)
    if not name or not contact or len(about) < 10:
        return None, "заполните имя, как с вами связаться и пару слов о себе и плагинах"
    with con:
        pending = con.execute(
            "SELECT COUNT(*) FROM applications WHERE status='pending'").fetchone()[0]
        if pending >= MAX_APPS_PENDING:
            return None, "заявок слишком много, попробуйте позже"
        token = secrets.token_urlsafe(24)
        con.execute(
            "INSERT INTO applications(at, name, contact, about, token_hash) VALUES(?,?,?,?,?)",
            (iso(), name, contact, about, sha(token)))
    return token, None


def decide_app(con, app_id, form):
    """Решение по заявке. Одобрение заводит разработчика с новым ключом."""
    action = form.get("do")
    reason = text(form.get("reason"), 1000)
    with con:
        row = con.execute("SELECT name, contact, status FROM applications WHERE id=?",
                          (app_id,)).fetchone()
        if not row:
            return "нет такой заявки"
        name, contact, status = row
        if status != "pending":
            return "по заявке уже решили"
        if action == "approve":
            key = new_dev_key()
            cur = con.execute(
                "INSERT INTO developers(name, contact, key_hash, key_once, created) VALUES(?,?,?,?,?)",
                (name, contact, sha(key), key, iso()))
            con.execute("""UPDATE applications SET status='approved', dev=?, decided_at=?
                           WHERE id=?""", (cur.lastrowid, iso(), app_id))
        elif action == "reject":
            con.execute("""UPDATE applications SET status='rejected', reason=?, decided_at=?
                           WHERE id=?""", (reason, iso(), app_id))
        else:
            return "неизвестное действие"
    return None


def decide_dev(con, dev_id, form):
    action = form.get("do")
    with con:
        if not con.execute("SELECT 1 FROM developers WHERE id=?", (dev_id,)).fetchone():
            return "нет такого разработчика"
        if action == "revoke":
            con.execute("UPDATE developers SET active=0 WHERE id=?", (dev_id,))
            con.execute("DELETE FROM sessions WHERE who=?", (f"dev:{dev_id}",))
        elif action == "restore":
            con.execute("UPDATE developers SET active=1 WHERE id=?", (dev_id,))
        elif action == "rekey":
            # Ключ потеряли или он утёк: прежний перестаёт работать сразу.
            key = new_dev_key()
            con.execute("UPDATE developers SET key_hash=?, key_once=? WHERE id=?",
                        (sha(key), key, dev_id))
            con.execute("DELETE FROM sessions WHERE who=?", (f"dev:{dev_id}",))
        else:
            return "неизвестное действие"
    return None


def review(con, n, form):
    action = form.get("do")
    reason = text(form.get("reason"), 1000)
    with con:
        row = con.execute("SELECT status FROM versions WHERE n=?", (n,)).fetchone()
        if not row:
            return "нет такой версии"
        if action == "approve":
            con.execute("UPDATE versions SET status='approved', reviewed_at=?, reason=NULL WHERE n=?",
                        (iso(), n))
        elif action == "reject":
            if not reason:
                return "напишите причину — разработчик её увидит"
            con.execute("UPDATE versions SET status='rejected', reviewed_at=?, reason=? WHERE n=?",
                        (iso(), reason, n))
        else:
            return "неизвестное действие"
    return None


def set_hidden(con, pid, hidden):
    with con:
        con.execute("UPDATE plugins SET hidden=? WHERE id=?", (1 if hidden else 0, pid))


def login(con, key):
    """Ключ → кто это ('admin' / 'dev:<id>') или None."""
    key = key.strip()
    if len(ADMIN_KEY) >= 24 and hmac.compare_digest(key.encode(), ADMIN_KEY.encode()):
        return "admin"
    if not DEV_KEY_RE.fullmatch(key):
        return None
    with con:
        row = con.execute("SELECT id FROM developers WHERE key_hash=? AND active=1",
                          (sha(key),)).fetchone()
        if not row:
            return None
        # Ключ забран: больше нигде не показываем.
        con.execute("UPDATE developers SET key_once=NULL WHERE id=?", (row[0],))
    return f"dev:{row[0]}"


def new_session(con, who):
    token = secrets.token_urlsafe(32)
    with con:
        con.execute("DELETE FROM sessions WHERE expires<?", (time.time(),))
        con.execute("INSERT INTO sessions(hash, who, csrf, expires) VALUES(?,?,?,?)",
                    (sha(token), who, secrets.token_urlsafe(24),
                     time.time() + SESSION_DAYS * 86400))
    return token


def session(con, token):
    """Кука → (кто, csrf) или (None, None). Отозванный разработчик — уже никто."""
    if not token or not TOKEN_RE.fullmatch(token):
        return None, None
    row = con.execute("SELECT who, csrf, expires FROM sessions WHERE hash=?",
                      (sha(token),)).fetchone()
    if not row or row[2] < time.time():
        return None, None
    who, csrf, _ = row
    if who.startswith("dev:"):
        ok = con.execute("SELECT active FROM developers WHERE id=?",
                         (int(who[4:]),)).fetchone()
        if not ok or not ok[0]:
            return None, None
    return who, csrf


# ---------- частота запросов ----------

class Limiter:
    """Ведро с жетонами на каждый IP: `rate` запросов за `per` секунд. IP — только
    хэшем с солью этого процесса, в базу не попадает."""

    def __init__(self, rate, per):
        self.rate, self.per = rate, per
        self.salt = secrets.token_bytes(16)
        self.buckets = {}
        self.lock = threading.Lock()

    def allow(self, ip):
        key = hashlib.blake2s(ip.encode(), key=self.salt, digest_size=12).digest()
        now = time.monotonic()
        with self.lock:
            tokens, at = self.buckets.get(key, (self.rate, now))
            tokens = min(self.rate, tokens + (now - at) * self.rate / self.per)
            ok = tokens >= 1
            self.buckets[key] = (tokens - 1 if ok else tokens, now)
            if len(self.buckets) > MAX_BUCKETS:
                self.buckets.clear()
            return ok


LIMITERS = {k: Limiter(*v) for k, v in LIMITS.items()}


# ---------- страницы ----------

CSS = """
:root { color-scheme: dark; --bg:#0b0d12; --surface:#141821; --surface2:#1b2030; --line:#232838;
  --ink:#eceef4; --ink2:#bcc0cd; --muted:#8a8fa3; --acc:#8b9cff; --acc2:#6f80ff; --ok:#5fd39a;
  --bad:#ff8a80; --r:16px; }
* { box-sizing:border-box; }
html { scrollbar-gutter:stable; }
body { margin:0; background:var(--bg); color:var(--ink);
  font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }
a { color:var(--acc); text-decoration:none; } a:hover { text-decoration:underline; }
[hidden] { display:none !important; }
/* шапка и вкладки */
.head { position:sticky; top:0; z-index:5; background:color-mix(in oklab,var(--bg) 88%,transparent);
  backdrop-filter:blur(12px); border-bottom:1px solid var(--line); }
.bar { display:flex; align-items:center; gap:16px; max-width:1080px; margin:0 auto; padding:14px 20px 0; }
.logo { font-weight:700; font-size:16px; color:var(--ink); display:flex; gap:10px; align-items:center; }
.logo:hover { text-decoration:none; }
.logo i { width:28px; height:28px; border-radius:9px; display:inline-block;
  background:linear-gradient(135deg,var(--acc),#b07cff); box-shadow:0 4px 14px #6f80ff40; }
.me { margin-left:auto; display:flex; gap:10px; align-items:center; color:var(--muted); font-size:13.5px; }
.me form { display:inline; }
.tabs { display:flex; gap:2px; max-width:1080px; margin:0 auto; padding:8px 14px 0; overflow-x:auto;
  scrollbar-width:none; }
.tabs a { display:flex; align-items:center; gap:7px; padding:9px 12px 11px; color:var(--ink2);
  border-bottom:2px solid transparent; white-space:nowrap; font-weight:500; }
.tabs a:hover { color:var(--ink); text-decoration:none; }
.tabs a.on { color:var(--ink); border-color:var(--acc); }
.tabs a i, .seg a i { font-style:normal; font-size:11.5px; font-weight:700; min-width:20px; text-align:center;
  padding:1px 7px; border-radius:99px; background:var(--acc2); color:#fff; }
main { max-width:1080px; margin:0 auto; padding:22px 20px 80px; }
h1 { font-size:28px; margin:6px 0 6px; letter-spacing:-.015em; line-height:1.2; }
h2 { font-size:13px; text-transform:uppercase; letter-spacing:.08em; color:var(--muted);
  margin:30px 0 12px; font-weight:600; }
h3 { margin:0; font-size:17px; }
.lead { color:var(--ink2); max-width:720px; margin:0 0 20px; }
.card { display:block; background:var(--surface); border:1px solid var(--line); border-radius:var(--r);
  padding:16px 18px; margin:0 0 12px; color:var(--ink); }
a.card:hover { text-decoration:none; border-color:#343b52; background:#171c27; }
.grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(310px,1fr)); gap:12px; }
.grid .card { margin:0; }
.pl { display:flex; gap:14px; align-items:flex-start; }
.ico { width:44px; height:44px; border-radius:12px; flex:none; background:var(--surface2); display:grid;
  place-items:center; overflow:hidden; font-weight:700; color:var(--acc); font-size:18px; }
.ico img { width:28px; height:28px; object-fit:contain; }
.ico.big { width:72px; height:72px; border-radius:18px; font-size:28px; }
.ico.big img { width:46px; height:46px; }
.pl b { display:block; font-size:15.5px; } .pl p { margin:4px 0 0; color:var(--ink2); font-size:13.5px; }
.muted { color:var(--muted); font-size:13px; }
.tag { display:inline-block; padding:1px 9px; border-radius:99px; font-size:12px; background:var(--surface2);
  color:var(--ink2); margin:2px 4px 2px 0; white-space:nowrap; }
.tag.ok { background:#163d2a; color:var(--ok); } .tag.bad { background:#4a1f1a; color:var(--bad); }
.tag.wait { background:#1f2a4d; color:#a9b6ff; }
.tools { display:flex; gap:10px; align-items:center; flex-wrap:wrap; margin:0 0 16px; }
.search { flex:1; min-width:220px; }
.seg { display:inline-flex; gap:2px; padding:3px; background:var(--surface); border:1px solid var(--line);
  border-radius:12px; }
.seg a, .seg button { display:flex; align-items:center; gap:6px; padding:6px 12px; border-radius:9px; border:0;
  background:none; color:var(--ink2); font:inherit; font-weight:500; cursor:pointer; }
.seg a:hover, .seg button:hover { color:var(--ink); text-decoration:none; }
.seg .on { background:var(--surface2); color:var(--ink); }
form.f { display:grid; gap:14px; }
label { display:grid; gap:6px; color:var(--ink2); font-size:13.5px; }
input[type=text], input[type=password], input[type=search], textarea { width:100%; background:var(--bg);
  color:var(--ink); border:1px solid var(--line); border-radius:11px; padding:10px 12px; font:inherit; }
input:focus, textarea:focus { outline:2px solid var(--acc); outline-offset:0; border-color:transparent; }
input[type=file] { color:var(--ink2); font:inherit; font-size:13.5px; }
input[type=file]::file-selector-button { font:inherit; font-weight:600; margin-right:12px; padding:8px 14px;
  border-radius:10px; border:1px solid var(--line); background:var(--surface2); color:var(--ink); cursor:pointer; }
.desc { display:-webkit-box; -webkit-line-clamp:3; line-clamp:3; -webkit-box-orient:vertical; overflow:hidden; }
textarea { min-height:130px; resize:vertical; }
button, .btn { display:inline-flex; align-items:center; gap:8px; font:inherit; font-weight:600; padding:9px 16px;
  border-radius:11px; border:0; cursor:pointer; background:var(--acc2); color:#fff; }
.btn:hover { text-decoration:none; filter:brightness(1.08); }
button.ghost, .btn.ghost { background:transparent; color:var(--ink2); border:1px solid var(--line); font-weight:500; }
button.danger { background:#5a2320; color:#ffb4a8; }
.sm { padding:6px 12px; font-size:13.5px; }
.row { display:flex; gap:10px; align-items:center; flex-wrap:wrap; }
.err { color:#ffb4a3; background:#4a1f1a; padding:10px 14px; border-radius:11px; margin:0 0 14px; }
.okmsg { color:#bff0d5; background:#163d2a; padding:10px 14px; border-radius:11px; margin:0 0 14px; }
.key { font:600 15px ui-monospace,Consolas,monospace; background:var(--bg); border:1px dashed var(--acc);
  padding:12px 14px; border-radius:11px; word-break:break-all; user-select:all; margin:10px 0; }
pre { margin:8px 0 0; background:var(--bg); border:1px solid var(--line); border-radius:12px;
  padding:12px 14px; font:12.5px/1.55 ui-monospace,"Cascadia Code",Consolas,monospace; white-space:pre-wrap;
  word-break:break-word; color:var(--ink2); max-height:520px; overflow:auto; }
details { border:1px solid var(--line); border-radius:12px; padding:8px 12px; margin:8px 0; }
summary { cursor:pointer; }
table { width:100%; border-collapse:collapse; }
td, th { text-align:left; padding:10px 8px; border-top:1px solid var(--line); vertical-align:middle; }
th { color:var(--muted); font-weight:600; font-size:13px; border-top:0; }
tr:first-child > td { border-top:0; }
td form { display:inline; }
.narrow { max-width:560px; }
.center { margin:30px auto; }
.two { display:grid; grid-template-columns:minmax(0,1.6fr) minmax(0,1fr); gap:16px; align-items:start; }
.steps { display:grid; grid-template-columns:repeat(auto-fill,minmax(230px,1fr)); gap:12px; counter-reset:st; }
.steps .card { margin:0; }
.steps .card::before { counter-increment:st; content:counter(st); display:grid; place-items:center; width:30px;
  height:30px; border-radius:9px; background:var(--surface2); color:var(--acc); font-weight:700; margin-bottom:10px; }
.facts { display:grid; grid-template-columns:repeat(auto-fill,minmax(220px,1fr)); gap:12px; }
.facts .card { margin:0; }
.facts b { display:block; color:var(--muted); font-size:12.5px; font-weight:600; text-transform:uppercase;
  letter-spacing:.06em; margin-bottom:6px; }
.tiles { display:grid; grid-template-columns:repeat(auto-fill,minmax(180px,1fr)); gap:12px; }
.tiles .card { margin:0; }
.tiles strong { display:block; font-size:30px; line-height:1.1; }
.empty { text-align:center; padding:34px 20px; color:var(--muted); }
.hero { display:flex; gap:18px; align-items:center; margin:4px 0 18px; }
@media (max-width:760px) { h1 { font-size:23px; } .two { grid-template-columns:1fr; } .me span { display:none; } }
"""

PUBLIC_TABS = [("catalog", "/", "Каталог"), ("developers", "/developers", "Разработчикам"),
               ("apply", "/apply", "Подать заявку")]
DEV_TABS = [("catalog", "/", "Каталог"), ("mine", "/dev", "Мои плагины"),
            ("upload", "/dev/upload", "Загрузить"), ("developers", "/developers", "Разработчикам")]
ADMIN_TABS = [("overview", "/admin", "Обзор"), ("apps", "/admin/apps", "Заявки"),
              ("review", "/admin/review", "Проверка"), ("plugins", "/admin/plugins", "Плагины"),
              ("devs", "/admin/devs", "Разработчики"), ("upload", "/admin/upload", "Загрузить"),
              ("catalog", "/", "Каталог")]
PERM_NAMES = {"notify": "Показывать сообщения внизу окна", "tabs": "Открывать вкладки"}
DOCS = "https://github.com/mylifeitsbest/axiom/blob/main/docs/plugins.md"


def header_info(con, who):
    """Имя в шапке и числа на вкладках администратора."""
    if who == "admin":
        count = lambda sql: con.execute(sql).fetchone()[0]  # noqa: E731
        return "Администратор", {
            "apps": count("SELECT COUNT(*) FROM applications WHERE status='pending'"),
            "review": count("SELECT COUNT(*) FROM versions WHERE status='pending'"),
        }
    if who:
        row = con.execute("SELECT name FROM developers WHERE id=?", (int(who[4:]),)).fetchone()
        return (row[0] if row else "Разработчик"), {}
    return "", {}


def page(title, body, nonce, who=None, csrf="", tab="", name="", counts=None):
    tabs = ADMIN_TABS if who == "admin" else DEV_TABS if who else PUBLIC_TABS
    counts = counts or {}
    on = lambda key: ' class="on"' if key == tab else ""  # noqa: E731
    items = "".join(
        f'<a href="{href}"{on(key)}>{label}'
        f'{f"<i>{counts[key]}</i>" if counts.get(key) else ""}</a>'
        for key, href, label in tabs)
    if who:
        right = (f'<span>{escape(name)}</span><form method="post" action="/logout">'
                 f'<input type="hidden" name="csrf" value="{escape(csrf)}">'
                 '<button class="ghost sm">Выйти</button></form>')
    else:
        right = '<a class="btn sm ghost" href="/login">Войти</a>'
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{escape(title)} — плагины Axiom</title><style>{CSS}</style></head><body>
<header class="head"><div class="bar"><a class="logo" href="/"><i></i>Плагины Axiom</a>
<div class="me">{right}</div></div><nav class="tabs">{items}</nav></header>
<main>{body}</main>
<script nonce="{escape(nonce)}">
for (const el of document.querySelectorAll("[data-utc]")) {{
  const d = new Date(el.dataset.utc);
  if (!isNaN(d)) el.textContent = el.dataset.utc.length > 10 ? d.toLocaleString("ru-RU", {{ dateStyle: "medium", timeStyle: "short" }}) : d.toLocaleDateString("ru-RU");
}}
</script></body></html>"""


def icon_html(meta, big=False):
    cls = "ico big" if big else "ico"
    icon = meta.get("icon")
    if isinstance(icon, str) and icon.startswith("data:image/"):
        return f'<span class="{cls}"><img src="{escape(icon)}" alt=""></span>'
    return f'<span class="{cls}">{escape((meta.get("name") or "?")[:1].upper())}</span>'


def plural(n, one, few, many):
    a, b = abs(n) % 100, abs(n) % 10
    return many if 10 < a < 20 else one if b == 1 else few if 2 <= b <= 4 else many


def installs_text(n):
    return f"{n} {plural(n, 'установка', 'установки', 'установок')}"


def site_name(m):
    return "все сайты" if m in ("<all_urls>", "*") else re.sub(r"^(\*|https?)://|/\*$", "", m)


CATALOG_JS = """
const q = document.getElementById("q"), none = document.getElementById("none");
const cards = [...document.querySelectorAll("[data-pl]")];
let kind = "all";
const apply = () => {
  const t = q.value.trim().toLowerCase();
  let shown = 0;
  for (const c of cards) {
    const ok = (kind === "all" || c.dataset.kind === kind) && (!t || c.dataset.text.includes(t));
    c.hidden = !ok;
    if (ok) shown++;
  }
  none.hidden = shown > 0;
};
q.addEventListener("input", apply);
for (const b of document.querySelectorAll("[data-pick]")) {
  b.addEventListener("click", () => {
    kind = b.dataset.pick;
    for (const x of document.querySelectorAll("[data-pick]")) x.classList.toggle("on", x === b);
    apply();
  });
}
"""


def catalog_body(con):
    items = catalog(con)
    cards = []
    for it in items:
        badge = ' <span class="tag ok">Axiom</span>' if it["official"] else ""
        text_ = f'{it["name"]} {it["description"]} {it["author"]} {it["id"]}'.lower()
        cards.append(f"""<a class="card pl" href="/p/{escape(it["id"])}" data-pl
  data-kind="{"axiom" if it["official"] else "community"}" data-text="{escape(text_)}">{icon_html(it)}<div>
  <b>{escape(it["name"])}{badge}</b><p class="desc">{escape(it["description"])}</p>
  <p class="muted">{escape(it["author"])} · {installs_text(it["installs"])}</p></div></a>""")
    official = sum(1 for it in items if it["official"])
    return f"""<h1>Каталог плагинов</h1>
<p class="lead">Плагины ставятся прямо в браузере: <b>Axiom → Настройки → Плагины → Магазин</b>.
Каждый плагин перед публикацией проверяет команда Axiom.</p>
<div class="tools"><input type="search" class="search" id="q" placeholder="Найти плагин" autocomplete="off">
  <span class="seg"><button class="on" data-pick="all">Все <i>{len(items)}</i></button>
  <button data-pick="axiom">От Axiom</button><button data-pick="community">От разработчиков
  {f"<i>{len(items) - official}</i>" if len(items) - official else ""}</button></span></div>
<div class="grid">{"".join(cards)}</div>
<div class="card empty" id="none"{" hidden" if items else ""}>Ничего не нашлось.</div>
<script nonce="__NONCE__">{CATALOG_JS}</script>"""


def plugin_body(con, pid):
    it = next((x for x in catalog(con) if x["id"] == pid), None)
    if not it:
        return None
    history = con.execute(
        """SELECT version, reviewed_at, note FROM versions WHERE id=? AND status='approved'
           ORDER BY n DESC LIMIT 10""", (pid,)).fetchall()
    badge = ' <span class="tag ok">Axiom</span>' if it["official"] else ""
    sites = "".join(f'<span class="tag">{escape(site_name(s))}</span>' for s in it["sites"]) or "—"
    perms = "".join(f'<div>{escape(PERM_NAMES.get(p, p))}</div>' for p in it["permissions"]) \
        or "Только сама страница: сообщений и вкладок не открывает"
    home = (f'<a class="btn ghost sm" href="{escape(it["homepage"])}" rel="noopener">Страница плагина</a>'
            if it["homepage"] else "")
    rows = "".join(
        f'<tr><td><b>{escape(v)}</b></td><td class="muted"><time data-utc="{escape(at or "")}">'
        f'{escape(at or "")}</time></td><td class="muted">{escape(note or "")}</td></tr>'
        for v, at, note in history)
    return f"""<p><a href="/">← Каталог</a></p>
<div class="hero">{icon_html(it, big=True)}<div><h1>{escape(it["name"])}{badge}</h1>
  <div class="muted">{escape(it["author"])} · версия {escape(it["version"])} · {installs_text(it["installs"])}</div>
</div></div>
<div class="two"><div>
  <div class="card"><p style="margin:0">{escape(it["description"]) or "Без описания."}</p></div>
  <div class="facts">
    <div class="card"><b>Где работает</b>{sites}</div>
    <div class="card"><b>Что ему можно</b>{perms}</div>
    <div class="card"><b>Кнопка на панели</b>{"Есть" if it["action"] else "Нет"}</div>
    <div class="card"><b>Настройки</b>{it["settings"] or "Нет"}</div>
  </div>
  <h2>Версии</h2>
  <div class="card"><table>{rows}</table></div>
</div><div>
  <div class="card"><h3>Как поставить</h3>
    <ol style="margin:10px 0 0;padding-left:20px;color:var(--ink2)">
      <li>Откройте Axiom → <b>Настройки</b> → <b>Плагины</b>.</li>
      <li>Вкладка <b>Магазин</b>, найдите «{escape(it["name"])}».</li>
      <li><b>Установить</b> — плагин сразу включится.</li></ol></div>
  <div class="row">{home}</div>
</div></div>"""


def developers_body(who):
    action = ('<a class="btn" href="/dev/upload">Загрузить плагин</a>' if who and who != "admin" else
              '<a class="btn" href="/apply">Подать заявку</a><a class="btn ghost" href="/login">Войти по ключу</a>')
    return f"""<h1>Разработчикам</h1>
<p class="lead">Плагин для Axiom — папка с <code>plugin.json</code> и скриптами. Чтобы он появился в
магазине у всех, станьте разработчиком: это бесплатно.</p>
<div class="row" style="margin-bottom:22px">{action}<a class="btn ghost" href="{DOCS}">Документация</a></div>
<div class="steps">
  <div class="card"><h3>Заявка</h3><p class="muted">Имя, как с вами связаться и пара слов о плагинах.
    Сайт даст секретную ссылку на статус — сохраните её.</p></div>
  <div class="card"><h3>Ключ</h3><p class="muted">Когда заявку одобрят, по ссылке появится ваш личный
    ключ разработчика. Храните его в менеджере паролей.</p></div>
  <div class="card"><h3>Загрузка</h3><p class="muted">Войдите по ключу и загрузите папку плагина или
    .zip — до 5 МБ, plugin.json в корне.</p></div>
  <div class="card"><h3>Проверка</h3><p class="muted">Мы читаем код и публикуем плагин или объясняем,
    что поправить. Статус видно в «Моих плагинах».</p></div>
</div>
<h2>Что не пройдёт проверку</h2>
<div class="card"><ul style="margin:0;padding-left:20px;color:var(--ink2)">
  <li>Код, который куда-то отправляет данные со страниц без ведома человека.</li>
  <li>Сжатый или запутанный код, который нельзя прочитать.</li>
  <li>Работа на всех сайтах без нужды и то, что ломает сайты.</li>
  <li>Плагин делает не то, что написано в описании.</li></ul></div>"""


def apply_body(error=None, form=None):
    form = form or {}
    v = lambda k: escape(str(form.get(k, "")))  # noqa: E731
    err = f'<p class="err">{escape(error)}</p>' if error else ""
    return f"""<h1>Подать заявку</h1>
<p class="lead">Расскажите о себе и о плагинах, которые хотите сделать. Мы посмотрим заявку и
выдадим личный ключ: с ним вы входите на этот сайт и загружаете плагины.</p>
<div class="two"><div class="card">{err}<form class="f" method="post" action="/apply">
  <label>Имя или ник<input type="text" name="name" maxlength="80" required value="{v("name")}"></label>
  <label>Как с вами связаться (Telegram, почта)<input type="text" name="contact" maxlength="200" required value="{v("contact")}"></label>
  <label>О себе и о плагинах: что хотите сделать, ссылки на свой код, если есть
    <textarea name="about" maxlength="2000" required>{v("about")}</textarea></label>
  <label hidden>Сайт<input type="text" name="website" tabindex="-1" autocomplete="off"></label>
  <div class="row"><button>Отправить заявку</button></div>
</form></div>
<div class="card"><h3>Что дальше</h3><p class="muted">После отправки откроется страница заявки с
секретной ссылкой — сохраните её в закладках. Когда заявку одобрят, там появится ваш ключ
разработчика. Контакт видит только администратор.</p></div></div>"""


def status_body(con, token):
    row = con.execute(
        """SELECT a.at, a.status, a.reason, d.key_once, d.active
           FROM applications a LEFT JOIN developers d ON d.id = a.dev
           WHERE a.token_hash=?""", (sha(token),)).fetchone()
    if not row:
        return None
    at, status, reason, key_once, active = row
    if status == "pending":
        what = """<p class="lead">Заявка на рассмотрении. Сохраните эту страницу в закладках:
  когда заявку одобрят, здесь появится ваш ключ разработчика.</p>"""
    elif status == "rejected":
        what = f"""<p class="err">Заявку отклонили{f": {escape(reason)}" if reason else "."}</p>"""
    elif key_once and active:
        what = f"""<p class="okmsg">Заявку одобрили. Вот ваш личный ключ разработчика:</p>
<p class="key">{escape(key_once)}</p>
<p class="muted">Сохраните его в надёжном месте (менеджер паролей): после первого входа этот ключ
здесь больше не покажется. Никому его не давайте — с ним загружают плагины от вашего имени.</p>
<div class="row"><a class="btn" href="/login">Войти</a></div>"""
    elif active:
        what = """<p class="okmsg">Заявку одобрили, ключ уже получен. Потеряли ключ — напишите нам,
  выпустим новый.</p><div class="row"><a class="btn" href="/login">Войти</a></div>"""
    else:
        what = '<p class="err">Ключ разработчика отозван.</p>'
    return f"""<div class="card narrow center"><h1>Заявка от <time data-utc="{escape(at)}">{escape(at)}</time></h1>
{what}</div>"""


def login_body(error=None):
    err = f'<p class="err">{escape(error)}</p>' if error else ""
    return f"""<div class="card narrow center"><h1>Вход</h1>
<p class="muted">По ключу разработчика или администратора. Ключа нет — <a href="/apply">подайте заявку</a>.</p>
{err}<form class="f" method="post" action="/login">
  <label>Ключ<input type="password" name="key" autocomplete="current-password" required autofocus></label>
  <div class="row"><button>Войти</button></div>
</form></div>"""


UPLOAD_JS = """
const up = document.getElementById("up"), msg = document.getElementById("upmsg");
const say = (t, ok) => { msg.textContent = t; msg.className = ok ? "okmsg" : "err"; msg.hidden = false; };
const b64 = (buf) => { let s = ""; const a = new Uint8Array(buf);
  for (let i = 0; i < a.length; i += 0x8000) s += String.fromCharCode.apply(null, a.subarray(i, i + 0x8000));
  return btoa(s); };
up.addEventListener("submit", async (e) => {
  e.preventDefault();
  const zip = document.getElementById("zip").files[0], dir = document.getElementById("dir").files;
  const body = { csrf: up.dataset.csrf, note: up.elements.note.value };
  try {
    if (zip) body.zip = b64(await zip.arrayBuffer());
    else if (dir.length) {
      body.files = {};
      for (const f of dir) {
        // В пути первой идёт сама папка плагина: её снимаем, скрытые файлы пропускаем.
        const rel = (f.webkitRelativePath || f.name).split("/").slice(1).join("/");
        if (!rel || rel.split("/").some((p) => p.startsWith("."))) continue;
        body.files[rel] = b64(await f.arrayBuffer());
      }
    } else return say("Выберите архив .zip или папку плагина", false);
    say("Загружаем…", true);
    const r = await fetch("/upload", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body) });
    const out = await r.json().catch(() => ({ ok: false, message: "сервер ответил ошибкой " + r.status }));
    say(out.message, out.ok);
    if (out.ok) setTimeout(() => { location.href = up.dataset.done; }, 1500);
  } catch (err) { say("Не загрузилось: " + err.message, false); }
});
"""


def upload_body(csrf, admin):
    hint = ("Ваши плагины публикуются сразу, без проверки, с пометкой «Axiom»." if admin else
            "Плагин уйдёт на проверку и появится в магазине после неё.")
    done = "/admin/plugins" if admin else "/dev"
    return f"""<h1>Загрузить плагин</h1>
<p class="lead">{hint} Новая версия — та же папка с увеличенной <code>version</code> в plugin.json.</p>
<div class="two"><div class="card"><form class="f" id="up" data-csrf="{escape(csrf)}" data-done="{done}">
  <p id="upmsg" hidden></p>
  <label>Архив .zip<input type="file" id="zip" accept=".zip,application/zip"></label>
  <label>…или папка плагина<input type="file" id="dir" webkitdirectory multiple></label>
  <label>Что нового в этой версии (необязательно)<input type="text" name="note" maxlength="1000"></label>
  <div class="row"><button>Загрузить</button></div>
</form></div>
<div class="card"><h3>Что примет магазин</h3><ul style="margin:10px 0 0;padding-left:20px;color:var(--ink2)">
  <li>plugin.json в корне папки, <code>"api": 1</code>.</li>
  <li>Файлы js, mjs, css, json, картинки, md, txt — до 100 файлов, 5 МБ.</li>
  <li>Скрытые файлы (с точки в начале) не уходят.</li>
  <li>Один номер версии — один раз.</li></ul>
  <p class="muted" style="margin-bottom:0"><a href="{DOCS}">Как написать плагин</a></p></div></div>
<script nonce="__NONCE__">{UPLOAD_JS}</script>"""


def version_tag(status, reason):
    cls = {"approved": "ok", "rejected": "bad", "pending": "wait"}.get(status, "")
    tag = f'<span class="tag {cls}">{escape(STATUS.get(status, status))}</span>'
    if status == "rejected" and reason:
        tag += f' <span class="muted">{escape(reason)}</span>'
    return tag


def dev_body(con, dev, csrf):
    rows = []
    for pid, installs, hidden in con.execute(
            "SELECT id, installs, hidden FROM plugins WHERE dev=? ORDER BY created", (dev,)):
        vers = con.execute(
            "SELECT version, status, reason, at, meta FROM versions WHERE id=? ORDER BY n DESC LIMIT 5",
            (pid,)).fetchall()
        if not vers:
            continue
        meta = json.loads(vers[0][4])
        vlist = "".join(f"<tr><td><b>{escape(v)}</b></td><td>{version_tag(s, r)}</td>"
                        f'<td class="muted"><time data-utc="{escape(at)}">{escape(at)}</time></td></tr>'
                        for v, s, r, at, _ in vers)
        hid = ' <span class="tag bad">снят из магазина</span>' if hidden else ""
        live = any(s == "approved" for _, s, _, _, _ in vers)
        link = f' · <a href="/p/{escape(pid)}">в каталоге</a>' if live and not hidden else ""
        rows.append(f"""<div class="card"><div class="pl">{icon_html(meta)}<div>
  <b>{escape(meta.get("name", pid))}{hid}</b><p class="muted">{escape(pid)} · {installs_text(installs)}{link}</p>
  </div></div><table style="margin-top:10px">{vlist}</table></div>""")
    empty = """<div class="card empty">Вы ещё ничего не загрузили.<div class="row" style="justify-content:center;
  margin-top:12px"><a class="btn" href="/dev/upload">Загрузить плагин</a></div></div>"""
    return f"""<h1>Мои плагины</h1>
<p class="lead">Статус каждой версии: на проверке, опубликована или отклонена — с причиной.</p>
{"".join(rows) or empty}"""


def form_btn(action, csrf, do, label, cls="", reason=False):
    extra = ('<input type="text" name="reason" placeholder="Причина — её увидит разработчик" '
             'maxlength="1000" style="min-width:260px;flex:1">' if reason else "")
    return (f'<form method="post" action="{escape(action)}" class="row" style="flex:1">'
            f'<input type="hidden" name="csrf" value="{escape(csrf)}">{extra}'
            f'<button class="{cls}" name="do" value="{do}">{label}</button></form>')


def admin_body(con, csrf):
    count = lambda sql: con.execute(sql).fetchone()[0]  # noqa: E731
    apps = count("SELECT COUNT(*) FROM applications WHERE status='pending'")
    pend = count("SELECT COUNT(*) FROM versions WHERE status='pending'")
    devs = count("SELECT COUNT(*) FROM developers WHERE active=1")
    installs = count("SELECT COALESCE(SUM(installs), 0) FROM plugins")
    live = len(published(con))
    tile = lambda n, label, href: (f'<a class="card" href="{href}"><strong>{n}</strong>'  # noqa: E731
                                   f'<span class="muted">{label}</span></a>')
    recent = []
    for kind, at, what in con.execute(
            """SELECT 'app', at, name FROM applications
               UNION ALL SELECT 'ver', at, id || ' ' || version FROM versions
               ORDER BY 2 DESC LIMIT 8"""):
        label = "Заявка" if kind == "app" else "Версия"
        recent.append(f'<tr><td>{label}</td><td>{escape(what)}</td><td class="muted">'
                      f'<time data-utc="{escape(at)}">{escape(at)}</time></td></tr>')
    return f"""<h1>Обзор</h1>
<p class="lead">Ключ администратора открывает всё: ваши плагины публикуются без проверки.</p>
<div class="tiles">{tile(apps, "заявок ждут решения", "/admin/apps")}
{tile(pend, "версий ждут проверки", "/admin/review")}
{tile(live, "плагинов в магазине", "/admin/plugins")}
{tile(devs, "разработчиков", "/admin/devs")}{tile(installs, "установок всего", "/admin/plugins")}</div>
<h2>Последнее</h2><div class="card"><table>{"".join(recent) or '<tr><td class="muted">Пока тихо.</td></tr>'}</table></div>"""


APP_FILTERS = [("pending", "Новые"), ("approved", "Одобренные"), ("rejected", "Отклонённые")]


def apps_body(con, csrf, flash=None, which="pending"):
    which = which if which in dict(APP_FILTERS) else "pending"
    counts = dict(con.execute("SELECT status, COUNT(*) FROM applications GROUP BY status").fetchall())
    on = lambda k: ' class="on"' if k == which else ""  # noqa: E731
    seg = "".join(f'<a href="/admin/apps?s={k}"{on(k)}>{label}'
                  f'{f"<i>{counts[k]}</i>" if counts.get(k) else ""}</a>' for k, label in APP_FILTERS)
    rows = []
    for aid, at, name, contact, about, status, reason in con.execute(
            """SELECT id, at, name, contact, about, status, reason FROM applications
               WHERE status=? ORDER BY id DESC LIMIT 200""", (which,)):
        act = (form_btn(f"/admin/apps/{aid}", csrf, "approve", "Одобрить и выдать ключ")
               + form_btn(f"/admin/apps/{aid}", csrf, "reject", "Отклонить", "ghost", reason=True)
               if status == "pending" else version_tag(status, reason))
        rows.append(f"""<div class="card"><h3>{escape(name)}</h3><p class="muted" style="margin:4px 0 0">
  {escape(contact)} · <time data-utc="{escape(at)}">{escape(at)}</time></p><pre>{escape(about)}</pre>
  <div class="row" style="margin-top:12px">{act}</div></div>""")
    return f"""<h1>Заявки</h1>{flash or ""}<div class="tools"><span class="seg">{seg}</span></div>
{"".join(rows) or '<div class="card empty">Здесь пусто.</div>'}"""


def devs_body(con, csrf, flash=None):
    rows = []
    for did, name, contact, key_once, active, created in con.execute(
            "SELECT id, name, contact, key_once, active, created FROM developers ORDER BY id DESC"):
        plugins = con.execute("SELECT COUNT(*) FROM plugins WHERE dev=?", (did,)).fetchone()[0]
        key = (f'<p class="muted" style="margin:10px 0 0">Ключ ещё не забран — его видно и по ссылке '
               f'на заявку; при необходимости передайте сами:</p><p class="key">{escape(key_once)}</p>'
               if key_once else "")
        acts = (form_btn(f"/admin/devs/{did}", csrf, "rekey", "Новый ключ", "ghost sm")
                + (form_btn(f"/admin/devs/{did}", csrf, "revoke", "Отозвать ключ", "danger sm") if active
                   else form_btn(f"/admin/devs/{did}", csrf, "restore", "Вернуть доступ", "sm")))
        state = ' <span class="tag bad">ключ отозван</span>' if not active else ""
        rows.append(f"""<div class="card"><div class="row" style="justify-content:space-between">
  <div><h3>{escape(name)}{state}</h3><p class="muted" style="margin:4px 0 0">{escape(contact)} ·
  {plugins} {plural(plugins, "плагин", "плагина", "плагинов")} · с
  <time data-utc="{escape(created[:10])}">{escape(created[:10])}</time></p></div>
  <div class="row">{acts}</div></div>{key}</div>""")
    return f"""<h1>Разработчики</h1>{flash or ""}
{"".join(rows) or '<div class="card empty">Пока никого. Одобренные заявки появятся здесь.</div>'}"""


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
            cut = (f'<p class="muted">… показаны первые {CODE_PREVIEW // 1024} КБ</p>'
                   if len(data) > CODE_PREVIEW else "")
            body = f"<pre>{escape(shown)}</pre>{cut}"
        opened = " open" if path.endswith((".json", ".js", ".mjs", ".css")) else ""
        out.append(f"<details{opened}><summary><b>{escape(path)}</b> "
                   f'<span class="muted">{len(data)} байт</span></summary>{body}</details>')
    return "".join(out)


def review_body(con, csrf, flash=None):
    items = []
    for n, pid, ver, meta, note, at, size, bundle, dev in con.execute(
            """SELECT v.n, v.id, v.version, v.meta, v.note, v.at, v.size, v.bundle, p.dev
               FROM versions v JOIN plugins p ON p.id = v.id
               WHERE v.status='pending' ORDER BY v.n"""):
        m = json.loads(meta)
        live = con.execute("""SELECT version FROM versions WHERE id=? AND status='approved'
                              ORDER BY n DESC LIMIT 1""", (pid,)).fetchone()
        who = con.execute("SELECT name, contact FROM developers WHERE id=?", (dev,)).fetchone() \
            if dev is not None else ("Axiom", "")
        was = f"сейчас в магазине {escape(live[0])}" if live else "новый плагин"
        sites = ", ".join(site_name(s) for s in m.get("sites") or [])
        items.append(f"""<div class="card"><div class="pl">{icon_html(m)}<div style="flex:1">
  <b>{escape(m.get("name", pid))} <span class="muted">{escape(pid)} {escape(ver)} · {was}</span></b>
  <p>{escape(m.get("description", ""))}</p>
  <p class="muted">{escape(who[0] if who else "?")} ({escape(who[1] if who else "")}) · {size} байт ·
  <time data-utc="{escape(at)}">{escape(at)}</time><br>Сайты: {escape(sites)} · разрешения:
    {escape(", ".join(m.get("permissions") or []) or "нет")}</p></div></div>
  {f"<pre>{escape(note)}</pre>" if note else ""}{code_html(bundle)}
  <div class="row" style="margin-top:12px">{form_btn(f"/admin/review/{n}", csrf, "approve", "Опубликовать")}
  {form_btn(f"/admin/review/{n}", csrf, "reject", "Отклонить", "ghost", reason=True)}</div></div>""")
    return f"""<h1>Проверка</h1>
<p class="lead">Код плагина работает на страницах сайтов у тех, кто его поставит: смотрите, что он
делает, куда отправляет данные, нет ли сжатого или запутанного кода.</p>
{flash or ""}{"".join(items) or '<div class="card empty">Ничего не ждёт проверки.</div>'}"""


def plugins_body(con, csrf, flash=None):
    rows = []
    for pid, ver, _, _, meta, at, installs, official in published(con):
        m = json.loads(meta)
        badge = '<span class="tag ok">Axiom</span>' if official else ""
        rows.append(f"<tr><td><div class='pl'>{icon_html(m)}<div><b><a href='/p/{escape(pid)}'>"
                    f"{escape(m.get('name', pid))}</a> {badge}</b><p class='muted'>{escape(pid)} · "
                    f"{escape(m.get('author', ''))}</p></div></div></td><td>{escape(ver)}</td>"
                    f"<td>{installs}</td><td>{form_btn(f'/admin/plugins/{pid}', csrf, 'hide', 'Снять', 'ghost sm')}"
                    "</td></tr>")
    hidden = [f"<tr><td>{escape(pid)} <span class='tag bad'>снят</span></td><td></td><td></td>"
              f"<td>{form_btn(f'/admin/plugins/{pid}', csrf, 'show', 'Вернуть', 'sm')}</td></tr>"
              for (pid,) in con.execute("SELECT id FROM plugins WHERE hidden=1 ORDER BY id")]
    return f"""<h1>Плагины</h1>{flash or ""}
<div class="tools"><a class="btn" href="/admin/upload">Загрузить плагин</a></div>
<div class="card"><table><tr><th>Плагин</th><th>Версия</th><th>Установок</th><th></th></tr>
{"".join(rows) or '<tr><td class="muted">В магазине пусто.</td></tr>'}</table></div>
{f'<h2>Сняты из магазина</h2><div class="card"><table>{"".join(hidden)}</table></div>' if hidden else ""}"""


# ---------- HTTP ----------

def client_ip(peer, forwarded):
    """За Caddy прямой собеседник — 127.0.0.1, настоящий адрес — последний в X-Forwarded-For."""
    if peer in ("127.0.0.1", "::1") and forwarded:
        last = forwarded.split(",")[-1].strip()
        if last:
            return last
    return peer


class Handler(BaseHTTPRequestHandler):
    server_version = "axiom-store"
    timeout = 30

    def log_message(self, *a):
        pass  # без журнала доступа: там были бы IP

    def ip(self):
        return client_ip(self.client_address[0], self.headers.get("X-Forwarded-For", ""))

    def allowed(self, what):
        if LIMITERS[what].allow(self.ip()):
            return True
        self.send(429, b"too many requests")
        return False

    def send(self, code, body=b"", ctype="text/plain; charset=utf-8", headers=()):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def html(self, code, title, body, who=None, csrf="", tab="", con=None, headers=()):
        nonce = secrets.token_urlsafe(16)
        name, counts = header_info(con, who) if con is not None else ("", {})
        doc = page(title, body.replace("__NONCE__", nonce), nonce, who, csrf, tab, name,
                   counts).encode("utf-8")
        csp = (f"default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'; "
               "img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; "
               "frame-ancestors 'none'")
        self.send(code, doc, "text/html; charset=utf-8",
                  [("Content-Security-Policy", csp), ("Cache-Control", "no-store"), *headers])

    def json_out(self, code, value=None, raw=None):
        body = raw if raw is not None else json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send(code, body, "application/json; charset=utf-8")

    def redirect(self, where, headers=()):
        self.send(303, b"", headers=[("Location", where), ("Cache-Control", "no-store"), *headers])

    def cookie(self):
        for part in self.headers.get("Cookie", "").split(";"):
            k, _, v = part.strip().partition("=")
            if k == COOKIE:
                return v
        return ""

    def who(self, con):
        return session(con, self.cookie())

    def body(self, limit):
        try:
            length = int(self.headers.get("Content-Length", 0))
        except ValueError:
            length = -1
        if length < 0 or length > limit:
            # Дочитать (до разумного), иначе клиент получит обрыв вместо ответа.
            left = min(max(length, 0), 1 << 20)
            while left > 0:
                chunk = self.rfile.read(min(left, 65536))
                if not chunk:
                    break
                left -= len(chunk)
            self.send(413, b"too large")
            return None
        return self.rfile.read(length) if length else b""

    def form(self, limit=MAX_FORM):
        raw = self.body(limit)
        if raw is None:
            return None
        try:
            return {k: v[0] for k, v in parse_qs(raw.decode("utf-8"), keep_blank_values=True,
                                                  max_num_fields=20).items()}
        except (UnicodeDecodeError, ValueError):
            self.send(400, b"bad")
            return None

    def handle(self):
        try:
            super().handle()
        except (ConnectionError, TimeoutError):
            self.close_connection = True

    # ----- GET -----

    def do_GET(self):
        url = urlsplit(self.path)
        path = url.path
        if path == "/health":
            return self.send(200, b"ok")
        if path == "/api/index":
            if not self.allowed("index"):
                return
            con = db()
            try:
                out = json.dumps({"plugins": catalog(con)}, ensure_ascii=False,
                                 separators=(",", ":")).encode()
            finally:
                con.close()
            return self.json_out(200, raw=out)
        m = re.fullmatch(r"/api/p/([a-z0-9-]{2,40})", path)
        if m:
            if not self.allowed("download"):
                return
            con = db()
            try:
                body = download(con, m[1])
            finally:
                con.close()
            if body is None:
                return self.json_out(404, {"error": "плагина нет в магазине"})
            return self.json_out(200, raw=body)
        if not self.allowed("page"):
            return
        con = db()
        try:
            self.get_page(con, path, parse_qs(url.query))
        finally:
            con.close()

    def get_page(self, con, path, query):
        who, csrf = self.who(con)
        admin = who == "admin"
        show = lambda title, body, tab="", code=200: self.html(  # noqa: E731
            code, title, body, who, csrf, tab, con)
        if path == "/":
            return show("Каталог", catalog_body(con), "catalog")
        m = re.fullmatch(r"/p/([a-z0-9-]{2,40})", path)
        if m:
            body = plugin_body(con, m[1])
            if body is None:
                return show("Нет такого плагина",
                            '<div class="card empty">Такого плагина в магазине нет.</div>', "catalog", 404)
            return show("Плагин", body, "catalog")
        if path == "/developers":
            return show("Разработчикам", developers_body(who), "developers")
        if path == "/apply":
            return show("Подать заявку", apply_body(), "apply")
        if path == "/login":
            if who:
                return self.redirect("/admin" if admin else "/dev")
            return show("Вход", login_body())
        m = re.fullmatch(r"/status/([A-Za-z0-9_-]{20,64})", path)
        if m:
            body = status_body(con, m[1])
            if body is None:
                return show("Нет такой заявки", '<div class="card empty">Нет такой заявки.</div>', "", 404)
            return show("Заявка", body)
        if path in ("/dev", "/dev/upload"):
            if not who or admin:
                return self.redirect("/admin" if admin else "/login")
            if path == "/dev":
                return show("Мои плагины", dev_body(con, int(who[4:]), csrf), "mine")
            return show("Загрузить плагин", upload_body(csrf, False), "upload")
        pages = {"/admin": ("Обзор", "overview", lambda: admin_body(con, csrf)),
                 "/admin/apps": ("Заявки", "apps",
                                 lambda: apps_body(con, csrf, which=(query.get("s") or ["pending"])[0])),
                 "/admin/review": ("Проверка", "review", lambda: review_body(con, csrf)),
                 "/admin/plugins": ("Плагины", "plugins", lambda: plugins_body(con, csrf)),
                 "/admin/devs": ("Разработчики", "devs", lambda: devs_body(con, csrf)),
                 "/admin/upload": ("Загрузить плагин", "upload", lambda: upload_body(csrf, True))}
        if path in pages:
            if not admin:
                return self.redirect("/login")
            title, tab, render = pages[path]
            return show(title, render(), tab)
        show("Не найдено", '<div class="card empty">Такой страницы нет.</div>', "", 404)

    do_HEAD = do_GET

    # ----- POST -----

    def do_POST(self):
        path = urlsplit(self.path).path
        con = db()
        try:
            if path == "/apply":
                return self.post_apply(con)
            if path == "/login":
                return self.post_login(con)
            if path == "/upload":
                return self.post_upload(con)
            return self.post_admin(con, path)
        finally:
            con.close()

    def post_apply(self, con):
        form = self.form()
        if form is None or not self.allowed("apply"):
            return
        token, error = apply(con, form)
        if error:
            return self.html(400, "Подать заявку", apply_body(error, form), tab="apply", con=con)
        self.redirect(f"/status/{token}")

    def post_login(self, con):
        form = self.form()
        if form is None:
            return
        if not self.allowed("login"):
            return
        who = login(con, form.get("key", ""))
        if not who:
            return self.html(403, "Вход", login_body("Неверный ключ или он отозван"), con=con)
        token = new_session(con, who)
        flags = "HttpOnly; SameSite=Strict; Path=/" + ("" if INSECURE else "; Secure")
        self.redirect("/admin" if who == "admin" else "/dev",
                      [("Set-Cookie", f"{COOKIE}={token}; Max-Age={SESSION_DAYS * 86400}; {flags}")])

    def post_upload(self, con):
        who, csrf = self.who(con)
        raw = self.body(MAX_UPLOAD)
        if raw is None:
            return
        if not who:
            return self.json_out(403, {"ok": False, "message": "войдите по ключу заново"})
        if not self.allowed("upload"):
            return
        try:
            payload = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return self.json_out(400, {"ok": False, "message": "неверный запрос"})
        if not isinstance(payload, dict) or not hmac.compare_digest(
                str(payload.get("csrf", "")).encode(), csrf.encode()):
            return self.json_out(403, {"ok": False, "message": "обновите страницу и попробуйте снова"})
        if isinstance(payload.get("zip"), str):
            try:
                files = from_zip(base64.b64decode(payload["zip"], validate=True))
            except ValueError:
                files = "архив повреждён"
        else:
            files = from_b64(payload.get("files"))
        if isinstance(files, str):
            return self.json_out(400, {"ok": False, "message": files})
        code, message = add_version(con, files, who, payload.get("note", ""))
        self.json_out(code, {"ok": code == 200, "message": message})

    def post_admin(self, con, path):
        form = self.form()
        if form is None:
            return
        who, csrf = self.who(con)
        if path == "/logout":
            if who and hmac.compare_digest(form.get("csrf", "").encode(), csrf.encode()):
                with con:
                    con.execute("DELETE FROM sessions WHERE hash=?", (sha(self.cookie()),))
            return self.redirect("/", [("Set-Cookie", f"{COOKIE}=; Max-Age=0; Path=/")])
        if who != "admin" or not hmac.compare_digest(form.get("csrf", "").encode(), csrf.encode()):
            return self.send(403, b"forbidden")
        m = re.fullmatch(r"/admin/(apps|devs|review)/(\d{1,12})", path)
        p = re.fullmatch(r"/admin/plugins/([a-z0-9-]{2,40})", path)
        if m:
            kind, n = m[1], int(m[2])
            error = {"apps": decide_app, "devs": decide_dev, "review": review}[kind](con, n, form)
            back = f"/admin/{kind}"
            if error:
                body = {"apps": apps_body, "devs": devs_body, "review": review_body}[kind](
                    con, csrf, f'<p class="err">{escape(error)}</p>')
                return self.html(400, "Админка", body, who, csrf, kind, con)
            # Одобрили заявку — ключ на странице разработчиков, чтобы его можно было передать.
            return self.redirect("/admin/devs" if kind == "apps" and form.get("do") == "approve" else back)
        if p and form.get("do") in ("hide", "show"):
            set_hidden(con, p[1], form["do"] == "hide")
            return self.redirect("/admin/plugins")
        self.send(404, b"not found")


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, *args, limit=MAX_CONNECTIONS, **kwargs):
        self.slots = threading.BoundedSemaphore(limit)
        super().__init__(*args, **kwargs)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()

    def handle_error(self, request, client_address):
        if isinstance(sys.exc_info()[1], (ConnectionError, TimeoutError)):
            return
        super().handle_error(request, client_address)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "import":
        init()
        con = db()
        try:
            for line in import_dir(con, sys.argv[2]):
                print(line)
        finally:
            con.close()
        return
    init()
    official = os.environ.get("AXIOM_STORE_OFFICIAL", "")
    if official and os.path.isdir(official):
        con = db()
        try:
            for line in import_dir(con, official):
                print(line, flush=True)
        except SystemExit as err:
            # Сломанный плагин Axiom не должен ронять магазин — остальное работает.
            print(f"плагины Axiom: {err}", flush=True)
        finally:
            con.close()
    if len(ADMIN_KEY) < 24:
        print("AXIOM_STORE_ADMIN_KEY не задан или короче 24 знаков: админка закрыта", flush=True)
    srv = Server((HOST, PORT), Handler)
    print(f"магазин плагинов на http://{HOST}:{PORT} (база {DB})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
