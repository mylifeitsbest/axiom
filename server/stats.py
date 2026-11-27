#!/usr/bin/env python3
"""Тихий сборщик статистики Axiom.

Принимает короткие пинги от браузеров (`POST /p`) и отчёты об ошибках (`POST /r`),
показывает приватную панель (`GET /d/<ключ>`, отчёт — `/d/<ключ>/r/<номер>`).
На сообщение из «Сообщить о проблеме» можно ответить со страницы отчёта
(`POST /d/<ключ>/r/<номер>`): браузер узнаёт ответ по случайному ключу сообщения
(`POST /a`) и показывает его человеку.
Пинги хранятся только агрегатами в SQLite; ни IP, ни идентификаторов в базе нет
(IP живёт лишь в памяти ограничителя частоты, и то в виде хэша с солью).
Зависимостей нет: всё из стандартной библиотеки, запускается на голой Ubuntu.

Отчёт из «Сообщить о проблеме» может прийти с файлами (`POST /rf`): снимки экрана,
фото, видео, текст. Они лежат на диске рядом с базой (`files/<номер отчёта>/<n>`),
на панели картинки и видео показываются прямо в отчёте.

Переменные окружения:
  AXIOM_STATS_DB        путь к базе (по умолчанию ./stats.db)
  AXIOM_STATS_HOST      адрес прослушивания (по умолчанию 127.0.0.1 — за Caddy)
  AXIOM_STATS_PORT      порт (по умолчанию 8787)
  AXIOM_STATS_KEY       секрет в адресе панели: /d/<ключ>. Без него панель закрыта.
  AXIOM_STATS_FILES     папка для файлов к отчётам (по умолчанию files рядом с базой)
  AXIOM_STATS_FILES_MB  сколько места им можно занять, МБ (по умолчанию 2048):
                        больше — самые старые файлы удаляются, отчёты остаются
"""

import hashlib
import hmac
import json
import os
import re
import secrets
import shutil
import sqlite3
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from html import escape
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, quote, urlsplit

DB = os.environ.get("AXIOM_STATS_DB", "stats.db")
HOST = os.environ.get("AXIOM_STATS_HOST", "127.0.0.1")
PORT = int(os.environ.get("AXIOM_STATS_PORT", "8787"))
KEY = os.environ.get("AXIOM_STATS_KEY", "")
# Админка магазина плагинов — отдельный сайт со своим входом; сюда — только ссылка на неё.
STORE_ADMIN_URL = os.environ.get("AXIOM_STATS_STORE_URL", "https://plugins.179-254-172-240.sslip.io/admin")

# Версия: выпуск 0.6.3 или бета 0.6.3-beta.190 (беты шлют свой номер с 0.6.3-beta.N,
# до того отмечались номером выпуска).
VERSION_RE = re.compile(r"\d{1,4}\.\d{1,4}\.\d{1,4}(-beta\.[1-9]\d{0,8})?")
OSES = {"windows", "linux", "macos", "other"}
MAX_BODY = 512

# Отчёты об ошибках: вылет, упавший процесс страницы, «Сообщить о проблеме».
REPORT_KINDS = {"crash": "вылет", "renderer": "упал процесс", "user": "от человека"}
REPORT_VERSION_RE = re.compile(r"\d{1,4}\.\d{1,4}\.\d{1,4}(-beta\.\d{1,9})?")
MAX_REPORT_BODY = 64 * 1024
MAX_REPORT_TEXT = 16 * 1024
MAX_REPORTS = 2000  # больше — самые старые удаляются: база не растёт без предела
REPORTS_KEEP = timedelta(days=90)

# Ответы на «Сообщить о проблеме». Ключ ответа браузер выбирает сам: 128 случайных
# бит, с другими сообщениями, пингами и самим браузером он не связан.
TOKEN_RE = re.compile(r"[0-9a-f]{32}")
ANSWERS = {"fixed": "исправлено", "reply": "ответ"}
MAX_ANSWER_TEXT = 2000
MAX_ANSWER_BODY = 32 * 1024
MAX_ASK_BODY = 4096
# Сколько ключей браузер спрашивает за раз (своих сообщений у него не больше).
MAX_ASK = 16

# Файлы к «Сообщить о проблеме» (POST /rf): строка JSON отчёта со списком файлов
# [{"name", "size"}], перевод строки и сами файлы подряд. Как и в браузере
# (src/reports/files.rs): до 10 файлов, вместе до 50 МБ.
MAX_FILES = 10
MAX_FILES_BYTES = 50 * 1024 * 1024
MAX_FILE_NAME = 200
FILES_KEEP_BYTES = int(os.environ.get("AXIOM_STATS_FILES_MB", "2048")) * 1024 * 1024
# Сколько свободного места оставить на диске: меньше — файлы не сохраняются.
FILES_FREE_BYTES = 1024 * 1024 * 1024
# Текстовый файл на странице отчёта — первые 64 КБ, целиком — по ссылке.
TEXT_PREVIEW = 64 * 1024
# Тип файла узнаётся по первым байтам; браузеру отдаётся только один из этих.
FILE_TYPES = {"image/png", "image/jpeg", "image/gif", "image/webp", "image/avif",
              "video/mp4", "video/webm", "video/x-matroska", "text/plain; charset=utf-8"}
HEIC_BRANDS = {b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1"}

# Сообщения из Axiom, который ещё не давал ключа ответа (до 0.6.1-beta.151). Отправителя
# узнаёт его же браузер — по тому, что есть только у него: последние строки запуска
# из журнала («=== Axiom 0.6.1-beta.148 · windows · 2026-11-26 18:39:13 UTC ===»),
# а без журнала — строка о системе и час отправки. Сервер раздаёт лишь хэши с солью
# (`old`), браузер сверяет их со своим журналом и забирает ответ (`c`): сообщение
# получает ключ и дальше живёт как новое.
HEADER_RE = re.compile(r"=== Axiom \S+ · \S+ · \d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC ===")
OLD_HEADERS = 3
MAX_CLAIMS = 2
MAX_CLAIM_TEXT = 600
MAX_OLD = 50
# Текст сообщения, который браузер получит вместе с ответом («Вы писали…»).
KEEP_TEXT = 600

# Ограничение частоты по IP: (запросов, за секунд). Браузер пингует раз в час,
# а отчёты шлёт редко — больше бывает только у накрутки. Об ответах спрашивает
# раз в 20 минут; запас — на несколько браузеров за одним адресом. Отчёты с файлами
# ещё и по байтам: не больше 300 МБ файлов в час с одного адреса.
LIMITS = {"/p": (30, 3600), "/r": (20, 3600), "/a": (120, 3600), "/rf": (10, 3600)}
FILE_BYTES_LIMIT = (300 * 1024 * 1024, 3600)
MAX_BUCKETS = 50_000
# Строк (версия × ОС) за день. Поддельные версии иначе раздували бы базу без предела.
MAX_ROWS_PER_DAY = 200
# То же для десятиминутных ячеек «последних двух суток».
MAX_ROWS_PER_SLOT = 50
# Сколько держать десятиминутные ячейки: на них «сейчас», часы и лента.
RECENT_KEEP = timedelta(hours=48)
SLOT_MIN = 10
# Одновременных соединений не больше: каждое — свой поток, и медленные клиенты
# (прислали Content-Length и молчат до таймаута) иначе съели бы память сервера.
MAX_CONNECTIONS = 64


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
        # отметка за день на этой версии, imports — перенесли закладки и историю
        # из другого браузера (один раз на профиль), imports_old — переносили
        # ещё до счётчика, браузер нашёл это в профиле при обновлении.
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
        for col in ("imports", "imports_old"):
            if col not in cols:
                con.execute(f"ALTER TABLE hits ADD COLUMN {col} INTEGER DEFAULT 0")
        # recent — то же по десятиминуткам за двое суток: «сейчас», часы, лента.
        con.execute(
            """CREATE TABLE IF NOT EXISTS recent(
                slot TEXT, v TEXT,
                pings INTEGER DEFAULT 0, installs INTEGER DEFAULT 0,
                updates INTEGER DEFAULT 0,
                PRIMARY KEY(slot, v))"""
        )
        rcols = {r[1] for r in con.execute("PRAGMA table_info(recent)")}
        for col in ("imports", "imports_old"):
            if col not in rcols:
                con.execute(f"ALTER TABLE recent ADD COLUMN {col} INTEGER DEFAULT 0")
        # online — отметки «в счёт часа»: по ним «сейчас» и столбики по часам.
        # До этого столбца в счёт шла каждая отметка — так и переносим.
        if "online" not in rcols:
            con.execute("ALTER TABLE recent ADD COLUMN online INTEGER DEFAULT 0")
            con.execute("UPDATE recent SET online = pings")
        # reports — отчёты об ошибках как есть (браузер уже вычистил личное).
        con.execute(
            """CREATE TABLE IF NOT EXISTS reports(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at TEXT NOT NULL, kind TEXT NOT NULL, v TEXT NOT NULL, os TEXT NOT NULL,
                text TEXT NOT NULL DEFAULT '', log TEXT NOT NULL DEFAULT '')"""
        )
        # Ответ на сообщение человека: ключ, по которому браузер спросит, что ответили
        # (ans: fixed | reply), текст, в какой версии, когда и когда браузер его забрал.
        # У отчётов до этих столбцов ключа нет — ответить на них нельзя.
        pcols = {r[1] for r in con.execute("PRAGMA table_info(reports)")}
        for col in ("token", "ans", "ans_text", "ans_v", "ans_at", "ans_got"):
            if col not in pcols:
                con.execute(f"ALTER TABLE reports ADD COLUMN {col} TEXT")
        con.execute("CREATE INDEX IF NOT EXISTS reports_token ON reports(token)")
        # report_files — файлы к отчётам: сами байты в files/<report>/<n>.
        # gone — файл удалён, чтобы уложиться в FILES_KEEP_BYTES (отчёт остался).
        con.execute(
            """CREATE TABLE IF NOT EXISTS report_files(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                report INTEGER NOT NULL, n INTEGER NOT NULL,
                name TEXT NOT NULL, type TEXT NOT NULL, size INTEGER NOT NULL,
                gone INTEGER NOT NULL DEFAULT 0)"""
        )
        con.execute("CREATE INDEX IF NOT EXISTS report_files_report ON report_files(report)")
    con.close()


def files_root():
    return os.environ.get("AXIOM_STATS_FILES") or os.path.join(
        os.path.dirname(os.path.abspath(DB)), "files")


def file_path(report, n):
    return os.path.join(files_root(), str(int(report)), str(int(n)))


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
    d, new, up, im, imo = flag("d"), flag("new"), flag("up"), flag("im"), flag("imo")
    # До 0.4.0 браузер не слал vd; тогда «первый раз сегодня на этой версии» = d.
    vd = flag("vd") if "vd" in payload else d
    # Браузеры старше 2026-11-26 не шлют o: у них в счёт идёт каждая отметка,
    # перезапуски тоже.
    o = flag("o") if "o" in payload else 1

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
            """INSERT INTO hits(day, v, os, pings, dau, wau, mau, installs, updates, vdau,
                                imports, imports_old)
               VALUES(?,?,?,1,?,?,?,?,?,?,?,?)
               ON CONFLICT(day, v, os) DO UPDATE SET
                 pings=pings+1, dau=dau+excluded.dau, wau=wau+excluded.wau,
                 mau=mau+excluded.mau, installs=installs+excluded.installs,
                 updates=updates+excluded.updates, vdau=vdau+excluded.vdau,
                 imports=imports+excluded.imports, imports_old=imports_old+excluded.imports_old""",
            (day, v, os_, d, flag("w"), flag("m"), new, up, vd, im, imo),
        )
        if not slot_full:
            con.execute(
                """INSERT INTO recent(slot, v, pings, installs, updates, imports, imports_old,
                                      online)
                   VALUES(?,?,1,?,?,?,?,?)
                   ON CONFLICT(slot, v) DO UPDATE SET
                     pings=pings+1, installs=installs+excluded.installs,
                     updates=updates+excluded.updates, imports=imports+excluded.imports,
                     imports_old=imports_old+excluded.imports_old,
                     online=online+excluded.online""",
                (slot, v, new, up, im, imo, o),
            )
        con.execute("DELETE FROM recent WHERE slot<?", (slot_key(now - RECENT_KEEP),))
    con.close()
    return True


def check_report(payload):
    """Проверить отчёт об ошибке. Вернуть кортеж для базы или None."""
    if not isinstance(payload, dict):
        return None
    kind, v, os_ = payload.get("kind"), payload.get("version"), payload.get("os")
    text, log = payload.get("text", ""), payload.get("log", "")
    if not isinstance(kind, str) or kind not in REPORT_KINDS:
        return None
    if not isinstance(v, str) or not REPORT_VERSION_RE.fullmatch(v):
        return None
    if not isinstance(os_, str) or os_ not in OSES:
        return None
    if not isinstance(text, str) or not isinstance(log, str):
        return None
    if len(text) > MAX_REPORT_TEXT or len(log) > MAX_REPORT_BODY:
        return None
    # Пустой отчёт ни о чём не говорит; NUL в тексте — только мусор.
    if not text.strip() and not log.strip():
        return None
    # Ключ ответа — только у сообщения человека. Кривой ключ не повод терять
    # сообщение: оно сохранится, только ответить на него будет нельзя.
    token = payload.get("token")
    if kind != "user" or not isinstance(token, str) or not TOKEN_RE.fullmatch(token):
        token = None
    return kind, v, os_, text.replace("\0", ""), log.replace("\0", ""), token


def store_report(report, files=(), tmp=None):
    """Записать отчёт. files — [(n, имя, тип, размер)] из папки tmp: она становится
    files/<номер отчёта>. Старые отчёты уходят вместе со своими файлами."""
    kind, v, os_, text, log, token = report
    now = utcnow()
    con = db()
    with con:
        cur = con.execute(
            "INSERT INTO reports(at, kind, v, os, text, log, token) VALUES(?,?,?,?,?,?,?)",
            (now.strftime("%Y-%m-%dT%H:%M:%SZ"), kind, v, os_, text, log, token))
        rid = cur.lastrowid
        if files and tmp:
            # Папка с этим номером могла остаться от прежней базы.
            target = os.path.join(files_root(), str(rid))
            shutil.rmtree(target, ignore_errors=True)
            os.replace(tmp, target)
            con.executemany(
                "INSERT INTO report_files(report, n, name, type, size) VALUES(?,?,?,?,?)",
                [(rid, *f) for f in files])
        con.execute("DELETE FROM reports WHERE at<?",
                    ((now - REPORTS_KEEP).strftime("%Y-%m-%dT%H:%M:%SZ"),))
        con.execute(
            "DELETE FROM reports WHERE id NOT IN (SELECT id FROM reports ORDER BY id DESC LIMIT ?)",
            (MAX_REPORTS,))
        prune_files(con)
    con.close()
    return rid


def prune_files(con):
    """Файлы удалённых отчётов — с диска; сверх FILES_KEEP_BYTES — самые старые."""
    for (rid,) in con.execute(
            "SELECT DISTINCT report FROM report_files WHERE report NOT IN (SELECT id FROM reports)"
    ).fetchall():
        shutil.rmtree(os.path.join(files_root(), str(int(rid))), ignore_errors=True)
    con.execute("DELETE FROM report_files WHERE report NOT IN (SELECT id FROM reports)")
    total = q1(con, "SELECT SUM(size) FROM report_files WHERE gone=0")
    if total <= FILES_KEEP_BYTES:
        return
    for fid, rid, n, size in con.execute(
            "SELECT id, report, n, size FROM report_files WHERE gone=0 ORDER BY id").fetchall():
        if total <= FILES_KEEP_BYTES:
            break
        try:
            os.remove(file_path(rid, n))
        except FileNotFoundError:
            pass
        con.execute("UPDATE report_files SET gone=1 WHERE id=?", (fid,))
        total -= size


def sniff(head):
    """MIME-тип файла по первым байтам или None, если такой не принимаем."""
    if head.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if head.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if head[:6] in (b"GIF87a", b"GIF89a"):
        return "image/gif"
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        return "image/webp"
    if head[4:8] == b"ftyp":
        brand = head[8:12]
        if brand in (b"avif", b"avis"):
            return "image/avif"
        # MOV тоже: Chrome играет его как MP4.
        return None if brand in HEIC_BRANDS else "video/mp4"
    if head[:4] == b"\x1a\x45\xdf\xa3":
        return "video/webm" if b"webm" in head[:64] else "video/x-matroska"
    if head and all(b >= 0x20 or b in b"\t\n\r\x0c\x1b" for b in head):
        try:
            head.decode("utf-8")
            return "text/plain; charset=utf-8"
        except UnicodeDecodeError as e:
            # Начало файла могло оборваться посреди буквы.
            if e.start >= len(head) - 3 and e.reason == "unexpected end of data":
                return "text/plain; charset=utf-8"
    return None


def clean_name(name):
    name = re.split(r"[\\/]", name)[-1]
    name = "".join(c for c in name if c.isprintable()).strip()
    return name[:MAX_FILE_NAME] or "файл"


def check_files(payload, body_left):
    """Список файлов из отчёта: [(имя, размер)] или None. Размеры в сумме — ровно
    остаток тела запроса."""
    files = payload.get("files") if isinstance(payload, dict) else None
    if not isinstance(files, list) or not 1 <= len(files) <= MAX_FILES:
        return None
    out = []
    for f in files:
        if not isinstance(f, dict):
            return None
        name, size = f.get("name"), f.get("size")
        if not isinstance(name, str) or len(name) > 1024:
            return None
        if not isinstance(size, int) or isinstance(size, bool) or size < 1:
            return None
        out.append((clean_name(name), size))
    total = sum(size for _, size in out)
    if total != body_left or total > MAX_FILES_BYTES:
        return None
    return out


def receive_files(rfile, files, keep):
    """Прочитать файлы из тела запроса во временную папку. Каждый проверяется по
    первым байтам: незнакомый тип не сохраняется. keep=False — только дочитать
    (на диске мало места). Вернуть (папка, [(n, имя, тип, размер)]) или None,
    если тело оборвалось."""
    tmp = os.path.join(files_root(), "tmp-" + secrets.token_hex(8))
    if keep:
        os.makedirs(tmp)
    stored = []
    try:
        for n, (name, size) in enumerate(files):
            path = os.path.join(tmp, str(n))
            out = open(path, "wb") if keep else None
            # kind: None — ещё не ясно, "" — не принимаем, иначе MIME-тип.
            left, head, kind = size, b"", None
            try:
                while left > 0:
                    chunk = rfile.read(min(left, 65536))
                    if not chunk:
                        raise EOFError
                    left -= len(chunk)
                    if kind is None:
                        head += chunk[:4096 - len(head)]
                        if len(head) >= 4096 or left == 0:
                            kind = sniff(head) or ""
                    if out:
                        out.write(chunk)
                        if kind == "":
                            # Незнакомый тип: дочитываем, не сохраняя.
                            out.close()
                            os.remove(path)
                            out = None
            finally:
                if out:
                    out.close()
            if keep and kind:
                stored.append((n, name, kind, size))
    except (EOFError, OSError):
        shutil.rmtree(tmp, ignore_errors=True)
        return None
    if not stored:
        shutil.rmtree(tmp, ignore_errors=True)
    return tmp, stored


def room_for(length):
    """Хватит ли места на диске под ещё столько байт файлов."""
    root = files_root()
    os.makedirs(root, exist_ok=True)
    return shutil.disk_usage(root).free - length > FILES_FREE_BYTES


def check_ask(payload):
    """Вопрос браузера: {"t": [ключи], "old": true, "c": [{"id": n, "p": "…"}]}.
    Вернуть (ключи, нужны ли старые, заявки на старые) или None."""
    if not isinstance(payload, dict):
        return None
    tokens = payload.get("t", [])
    if not isinstance(tokens, list) or len(tokens) > MAX_ASK:
        return None
    if not all(isinstance(t, str) and TOKEN_RE.fullmatch(t) for t in tokens):
        return None
    old = payload.get("old", False)
    claims = payload.get("c", [])
    if not isinstance(old, bool) or not isinstance(claims, list) or len(claims) > MAX_CLAIMS:
        return None
    for c in claims:
        if not (isinstance(c, dict) and isinstance(c.get("id"), int) and not isinstance(c.get("id"), bool)
                and isinstance(c.get("p"), str) and len(c["p"]) <= MAX_CLAIM_TEXT):
            return None
    return list(dict.fromkeys(tokens)), old, [(c["id"], c["p"]) for c in claims]


def old_salt(id_):
    """Соль хэшей старого сообщения: своя у каждого и не угадывается без ключа панели."""
    return hmac.new(KEY.encode(), f"old:{int(id_)}".encode(), hashlib.sha256).hexdigest()[:32]


def fp_hash(salt, preimage):
    return hashlib.sha256((salt + preimage).encode("utf-8")).hexdigest()


def old_fingerprints(text, log, at):
    """По чему браузер узнает своё старое сообщение: (k, [прообразы]). k > 0 — последние
    k строк запуска из журнала одной строкой; k = 0 — строка о системе и час отправки."""
    heads = [line.strip() for line in log.splitlines() if HEADER_RE.fullmatch(line.strip())]
    if heads:
        last = heads[-OLD_HEADERS:]
        return len(last), ["h|" + "\n".join(last)]
    first = text.lstrip().split("\n", 1)[0].strip()
    if first.startswith("Axiom ") and " · " in first and len(at) >= 13:
        return 0, [f"a|{first}|{at[:13]}"]
    return 0, []


def message_text(text):
    """Что написал человек — без служебной строки «Axiom … · система · движок»."""
    text = text.strip()
    first, _, rest = text.partition("\n")
    if first.startswith("Axiom ") and " · " in first:
        text = rest
    return text.strip()[:KEEP_TEXT]


def old_answers():
    """Ответы на старые сообщения, которые ещё никто не забрал: только хэши, без текста."""
    since = (utcnow() - REPORTS_KEEP).strftime("%Y-%m-%dT%H:%M:%SZ")
    con = db()
    rows = con.execute(
        """SELECT id, at, text, log FROM reports WHERE kind='user' AND token IS NULL
           AND ans IS NOT NULL AND at>=? ORDER BY id DESC LIMIT ?""", (since, MAX_OLD)).fetchall()
    con.close()
    out = []
    for id_, at, text, log in rows:
        k, pre = old_fingerprints(text, log, at)
        if pre:
            salt = old_salt(id_)
            out.append({"id": id_, "s": salt, "k": k, "h": [fp_hash(salt, p) for p in pre]})
    return out


def claim(claims):
    """Браузер узнал своё старое сообщение: выдать ему ключ, ответ и текст сообщения.
    Забрать можно один раз — потом сообщение спрашивают по ключу, как новое."""
    out = {}
    now = utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
    con = db()
    with con:
        for id_, p in claims:
            row = con.execute(
                """SELECT at, kind, text, log, token, ans, ans_text, ans_v, ans_at
                   FROM reports WHERE id=?""", (id_,)).fetchone()
            if not row:
                continue
            at, kind, text, log, token, ans, ans_text, ans_v, ans_at = row
            if kind != "user" or token or not ans:
                continue
            _, pre = old_fingerprints(text, log, at)
            if not any(hmac.compare_digest(p.encode(), x.encode()) for x in pre):
                continue
            token = secrets.token_hex(16)
            cur = con.execute("UPDATE reports SET token=?, ans_got=? WHERE id=? AND token IS NULL",
                              (token, now, id_))
            if cur.rowcount == 1:
                out[str(id_)] = {"token": token, "sent": at, "msg": message_text(text),
                                 "s": ans, "text": ans_text or "", "v": ans_v or "", "at": ans_at or ""}
    con.close()
    return out


def answers(tokens):
    """Ответы на сообщения с этими ключами: {ключ: {s, text, v, at}} — только на те,
    на которые ответили. Впервые отданный ответ помечается полученным."""
    if not tokens:
        return {}
    now = utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
    marks = ",".join("?" * len(tokens))
    out = {}
    con = db()
    with con:
        rows = con.execute(
            f"""SELECT id, token, ans, ans_text, ans_v, ans_at FROM reports
                WHERE token IN ({marks}) AND ans IS NOT NULL ORDER BY id""", tokens).fetchall()
        for id_, token, ans, text, v, at in rows:
            out[token] = {"s": ans, "text": text or "", "v": v or "", "at": at or ""}
            con.execute("UPDATE reports SET ans_got=? WHERE id=? AND ans_got IS NULL", (now, id_))
    con.close()
    return out


def set_answer(id_, form):
    """Ответить на сообщение №id_ (поля формы: s, text, v). Вернуть None или причину
    отказа. s=clear убирает ответ (если браузер его уже забрал — у человека он останется)."""
    s = form.get("s", "")
    text = form.get("text", "").replace("\0", "").replace("\r\n", "\n").strip()
    v = form.get("v", "").strip()
    if s not in ANSWERS and s != "clear":
        return "Неизвестный ответ"
    if len(text) > MAX_ANSWER_TEXT:
        return f"Ответ длиннее {MAX_ANSWER_TEXT} символов"
    if v and not REPORT_VERSION_RE.fullmatch(v):
        return "Версия — как 0.6.1 или 0.6.1-beta.150"
    if s == "reply" and not text:
        return "Напишите ответ"
    con = db()
    row = con.execute("SELECT kind, token FROM reports WHERE id=?", (id_,)).fetchone()
    if not row:
        con.close()
        return "Нет такого отчёта"
    if row[0] != "user":
        con.close()
        return "Ответить можно только на сообщение человека"
    with con:
        if s == "clear":
            con.execute(
                """UPDATE reports SET ans=NULL, ans_text=NULL, ans_v=NULL, ans_at=NULL,
                   ans_got=NULL WHERE id=?""", (id_,))
        else:
            con.execute(
                "UPDATE reports SET ans=?, ans_text=?, ans_v=?, ans_at=?, ans_got=NULL WHERE id=?",
                (s, text, v, utcnow().strftime("%Y-%m-%dT%H:%M:%SZ"), id_))
    con.close()
    return None


# ---------- ограничение частоты ----------

class Limiter:
    """Ведро с жетонами на каждый IP: `rate` запросов за `per` секунд, с запасом
    на всплеск того же размера. IP хранится только хэшем с солью этого процесса.
    `cost` — сколько жетонов стоит запрос (для предела по байтам — его размер)."""

    def __init__(self, rate, per, clock=time.monotonic):
        self.rate, self.per, self.clock = rate, per, clock
        self.salt = secrets.token_bytes(16)
        self.buckets = {}
        self.lock = threading.Lock()

    def allow(self, ip, cost=1):
        key = hashlib.blake2s(ip.encode(), key=self.salt, digest_size=12).digest()
        now = self.clock()
        with self.lock:
            tokens, at = self.buckets.get(key, (self.rate, now))
            tokens = min(self.rate, tokens + (now - at) * self.rate / self.per)
            ok = tokens >= cost
            self.buckets[key] = (tokens - cost if ok else tokens, now)
            if len(self.buckets) > MAX_BUCKETS:
                self._prune(now)
            return ok

    def _prune(self, now):
        # Полные вёдра ничего не помнят — их можно забыть.
        full = [k for k, (t, at) in self.buckets.items()
                if t + (now - at) * self.rate / self.per >= self.rate]
        for k in full:
            del self.buckets[k]
        if len(self.buckets) > MAX_BUCKETS:
            self.buckets.clear()


LIMITERS = {path: Limiter(*limit) for path, limit in LIMITS.items()}
FILE_BYTES = Limiter(*FILE_BYTES_LIMIT)


def client_ip(peer, forwarded):
    """IP клиента. За Caddy прямой собеседник — 127.0.0.1, а настоящий адрес — последний
    в X-Forwarded-For (его дописывает сам Caddy). Остальным заголовок не верим."""
    if peer in ("127.0.0.1", "::1") and forwarded:
        last = forwarded.split(",")[-1].strip()
        if last:
            return last
    return peer


# ---------- панель ----------

def ver_key(v):
    """Порядок версий: 0.6.3 < 0.6.3-beta.5 < 0.6.3-beta.9 < 0.6.4."""
    main, _, beta = v.partition("-beta.")
    return (*(int(x) for x in main.split(".")), 1 if beta else 0, int(beta or 0))


def is_beta(v):
    return "-beta." in v


# 0.7.0 вышла по ошибке и через час была отозвана, те же изменения вышли как 0.6.2.
# Браузеры, что успели её поставить, так и отмечаются «0.7.0» (обновления до 0.6.x
# для них «старее»), — на панели они считаются вместе с 0.6.2. В базе всё как пришло.
SHOWN_AS = {"0.7.0": "0.6.2"}


def shown_version(v):
    return SHOWN_AS.get(v, v)


def online_now(con, now):
    """Сколько браузеров открыто: отметки «в счёт часа» за последние 60 минут.

    Открытый браузер даёт такую раз в 60–61 минуту, так что за час — ровно одну.
    Ячейки по 10 минут; та, на которую пришлась граница часа, берётся долей
    (в 18:08 из ячейки 17:00–17:10 — две десятых). Больше, чем разных людей
    сегодня, не бывает: в полночь каждый открытый браузер отмечается заново,
    а старые версии без флага o считают и отметки при перезапуске.
    """
    start = now - timedelta(hours=1)
    edge = slot_key(start)
    inside = q1(con, "SELECT SUM(online) FROM recent WHERE slot>?", (edge,))
    at_edge = q1(con, "SELECT SUM(online) FROM recent WHERE slot=?", (edge,))
    slot = timedelta(minutes=SLOT_MIN)
    edge_end = start.replace(minute=start.minute // SLOT_MIN * SLOT_MIN,
                             second=0, microsecond=0) + slot
    n = round(inside + at_edge * ((edge_end - start) / slot))
    return min(n, q1(con, "SELECT SUM(dau) FROM hits WHERE day=?", (day_key(now),)))


def dashboard(nonce=""):
    con = db()
    now = utcnow()
    t = day_key(now)
    d30 = day_key(now - timedelta(days=29))
    # Браузер ставит флаги w/m на первый пинг календарной недели (с понедельника,
    # UTC) и месяца. Поэтому и считаем с их начала: в скользящем окне 7/30 дней
    # человек, заходивший в обе недели (оба месяца), попадал бы дважды.
    week_start = day_key(now - timedelta(days=now.weekday()))
    month_start = now.strftime("%Y-%m-01")

    s = {
        "now": now,
        "online": online_now(con, now),
        "dau": q1(con, "SELECT SUM(dau) FROM hits WHERE day=?", (t,)),
        "wau": q1(con, "SELECT SUM(wau) FROM hits WHERE day>=?", (week_start,)),
        "mau": q1(con, "SELECT SUM(mau) FROM hits WHERE day>=?", (month_start,)),
        "pings_today": q1(con, "SELECT SUM(pings) FROM hits WHERE day=?", (t,)),
        "inst_today": q1(con, "SELECT SUM(installs) FROM hits WHERE day=?", (t,)),
        "inst_all": q1(con, "SELECT SUM(installs) FROM hits"),
        "upd_today": q1(con, "SELECT SUM(updates) FROM hits WHERE day=?", (t,)),
        "upd_all": q1(con, "SELECT SUM(updates) FROM hits"),
        "imp_today": q1(con, "SELECT SUM(imports) FROM hits WHERE day=?", (t,)),
        "imp_all": q1(con, "SELECT SUM(imports + imports_old) FROM hits"),
        "imp_old": q1(con, "SELECT SUM(imports_old) FROM hits"),
        "week_start": week_start,
    }

    # По дням: активные и из них новые.
    rows = {r[0]: (r[1] or 0, r[2] or 0) for r in con.execute(
        "SELECT day, SUM(dau), SUM(installs) FROM hits WHERE day>=? GROUP BY day", (d30,))}
    s["days"] = [(d, *rows.get(d, (0, 0))) for d in
                 (day_key(now - timedelta(days=i)) for i in range(29, -1, -1))]

    # По часам за сутки: сколько отметок «в счёт часа» пришло в каждый час =
    # сколько браузеров было открыто. Как и «сейчас», не больше людей за тот день.
    hours = {r[0]: r[1] for r in con.execute(
        "SELECT substr(slot, 1, 13), SUM(online) FROM recent GROUP BY 1")}
    top = now.replace(minute=0, second=0, microsecond=0)
    s["hours"] = [(h, min(hours.get(h.strftime("%Y-%m-%dT%H"), 0),
                          rows.get(day_key(h), (0, 0))[0]))
                  for h in (top - timedelta(hours=i) for i in range(23, -1, -1))]

    # Лента: установки, обновления и переезды за двое суток, свежие сверху.
    s["feed"] = con.execute(
        """SELECT slot, v, installs, updates, imports, imports_old FROM recent
           WHERE installs>0 OR updates>0 OR imports>0 OR imports_old>0
           ORDER BY slot DESC, v DESC LIMIT 30""").fetchall()

    # Версии: сколько людей на ней сегодня и сколько всего на неё обновились.
    vers = {}
    for v, today, upd in con.execute(
            """SELECT v, SUM(CASE WHEN day=? THEN vdau ELSE 0 END), SUM(updates)
               FROM hits WHERE day>=? GROUP BY v""", (t, d30)):
        was_today, was_upd = vers.get(shown_version(v), (0, 0))
        vers[shown_version(v)] = (was_today + (today or 0), was_upd + (upd or 0))
    # Старые версии, на которых сегодня никого, только загромождали бы таблицу.
    # Беты выходят по нескольку в день — их показываем, только пока на них кто-то есть.
    vers = {v: n for v, n in vers.items() if n[0] or (n[1] and not is_beta(v))}
    s["versions"] = sorted(vers.items(), key=lambda kv: ver_key(kv[0]), reverse=True)
    s["oses"] = con.execute(
        """SELECT os, SUM(dau) a FROM hits WHERE day=? GROUP BY os
           HAVING a>0 ORDER BY a DESC""", (t,)).fetchall()
    # Отчёты об ошибках: свежие сверху и сколько каких пришло за сутки.
    s["reports"] = con.execute(
        """SELECT r.id, r.at, r.kind, r.v, r.os, substr(r.text, 1, 2000), 1,
                  r.ans, r.ans_got,
                  (SELECT COUNT(*) FROM report_files f WHERE f.report = r.id)
           FROM reports r ORDER BY r.id DESC LIMIT 30""").fetchall()
    s["waiting"] = q1(con, "SELECT COUNT(*) FROM reports WHERE kind='user' AND (ans IS NULL OR ans='')")
    s["reports_day"] = dict(con.execute(
        "SELECT kind, COUNT(*) FROM reports WHERE at>=? GROUP BY kind",
        ((now - timedelta(days=1)).strftime("%Y-%m-%dT%H:%M:%SZ"),)).fetchall())
    con.close()
    return render(s, nonce)


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


# Графики рисуются дважды: широкий и узкий для телефона (CSS показывает один).
# Подписи в SVG масштабируются вместе с ним: широкий на экране телефона ужат
# втрое, и 11-пиксельные подписи становились 4-пиксельными.
NARROW_W = 360


def chart_svg(W, H, out):
    cls = "narrow" if W == NARROW_W else "wide"
    return f'<svg viewBox="0 0 {W} {H}" class="chart-svg {cls}" role="img">{"".join(out)}</svg>'


def chart_days(days, W=840):
    """30 столбиков: вернувшиеся (синий) и новые (оранжевый) сверху."""
    H, top, bottom = 150, 18, 22
    n = len(days)
    step = W / n
    gap = 6 if W > NARROW_W else 3
    bw = step - gap
    peak = max((a for _, a, _ in days), default=0) or 1
    plot = H - top - bottom
    out = [f'<line x1="0" x2="{W}" y1="{H - bottom}" y2="{H - bottom}" class="axis"/>']
    for i, (day, active, new) in enumerate(days):
        x = i * step + gap / 2
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
    return chart_svg(W, H, out)


def chart_hours(hours, W=840):
    H, top, bottom = 120, 18, 22
    n = len(hours)
    step = W / n
    gap = 8 if W > NARROW_W else 4
    bw = step - gap
    peak = max((c for _, c in hours), default=0) or 1
    plot = H - top - bottom
    base = H - bottom
    out = [f'<line x1="0" x2="{W}" y1="{base}" y2="{base}" class="axis"/>']
    for i, (h, c) in enumerate(hours):
        x = i * step + gap / 2
        bh = plot * c / peak
        cls = "s-old now" if i == n - 1 else "s-old"
        out.append(f'<path d="{bar_path(x, base - bh, bw, bh)}" class="{cls}"/>')
        if c and (c == peak or i == n - 1):
            out.append(f'<text x="{x + bw / 2:.1f}" y="{base - bh - 5:.1f}" '
                       f'class="val" text-anchor="middle">{c}</text>')
        out.append(f'<rect x="{i * step:.1f}" y="0" width="{step:.1f}" height="{H}" class="hit">'
                   f'<title data-utc="{utc_iso(h)}" data-fmt="hour-tip" data-n="{c}">'
                   f'{c}</title></rect>')
        # На узком графике подпись за два часа до «сейчас» налезала бы на него.
        if i == n - 1 or i % 3 == 0 and (W > NARROW_W or i < n - 3):
            out.append(f'<text x="{x + bw / 2:.1f}" y="{H - 5}" class="tick" text-anchor="middle"'
                       f' data-utc="{utc_iso(h)}" data-fmt="{"now" if i == n - 1 else "h"}">'
                       f'{h:%H}</text>')
    return chart_svg(W, H, out)


def feed_html(feed):
    if not feed:
        return ('<div class="feed-empty">Пока ничего. Здесь появятся новые установки, '
                'обновления и переезды — через минуту после того, как они случатся.</div>')
    items = []
    for slot, v, inst, upd, imp, imp_old in feed:
        v = shown_version(v)
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
        if imp:
            items.append(
                f'<li><span class="dot d-imp"></span>{when}<span class="what">'
                f'{imp} {plural(imp, "человек перенёс", "человека перенесли", "человек перенесли")}'
                f' закладки и историю из другого браузера</span>'
                f'<span class="ver">{escape(v)}</span></li>')
        if imp_old:
            items.append(
                f'<li><span class="dot d-imp"></span>{when}<span class="what">'
                f'{imp_old} {plural(imp_old, "человек переезжал", "человека переезжали", "человек переезжали")}'
                f' раньше — нашлось в профиле</span>'
                f'<span class="ver">{escape(v)}</span></li>')
    return f'<ul class="feed">{"".join(items)}</ul>'


def first_line(text):
    """О чём отчёт: первая непустая строка, кроме служебной «Axiom 0.5.4 · система · движок»."""
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if lines and lines[0].startswith("Axiom ") and " · " in lines[0]:
        lines = lines[1:]
    return lines[0] if lines else ""


def report_href(id_):
    return f"/d/{escape(KEY)}/r/{int(id_)}"


def reports_html(rows):
    if not rows:
        return ('<div class="panel feed-empty">Отчётов пока нет. Сюда придут вылеты, упавшие '
                'процессы страниц и сообщения из «Сообщить о проблеме».</div>')
    trs = []
    for id_, at, kind, v, os_, text, can, ans, got, nfiles in rows:
        line = first_line(text)
        if len(line) > 140:
            line = line[:139] + "…"
        href = report_href(id_)
        clip = (f' <span class="clip" title="{nfiles} {plural(nfiles, "файл", "файла", "файлов")}">'
                f'📎{nfiles}</span>') if nfiles else ""
        # Классы ячеек нужны телефонной раскладке: там строка — карточка, ячейки переставлены.
        trs.append(
            f'<tr><td class="c-at"><a href="{href}"><time data-utc="{escape(at)}" data-fmt="feed">'
            f'{escape(at[:16].replace("T", " "))}</time></a></td>'
            f'<td class="c-kind"><span class="kind k-{escape(kind)}">{escape(REPORT_KINDS.get(kind, kind))}</span></td>'
            f'<td class="c-v">{escape(v)}</td><td class="c-os">{escape(os_)}</td>'
            f'<td class="line"><a href="{href}">{escape(line) or "—"}</a>{clip}</td>'
            f'<td class="c-ans">{answer_badge(kind, can, ans, got)}</td></tr>')
    return ('<table class="reps"><thead><tr><th>когда</th><th>что</th><th>версия</th>'
            f'<th>система</th><th>о чём</th><th>ответ</th></tr></thead><tbody>{"".join(trs)}</tbody></table>')


def answer_badge(kind, can, ans, got):
    """Ответили ли человеку: «исправлено ✓» (✓ — браузер ответ забрал), «ждёт ответа»."""
    if kind != "user" or not can:
        return ""
    if not ans:
        return '<span class="ans a-wait">ждёт ответа</span>'
    mark = " ✓" if got else ""
    title = "Браузер ответ забрал" if got else "Браузер ещё не забрал ответ"
    return f'<span class="ans a-{escape(ans)}" title="{title}">{escape(ANSWERS.get(ans, ans))}{mark}</span>'


def reports_hint(day):
    parts = []
    for kind, forms in (("crash", ("вылет", "вылета", "вылетов")),
                        ("renderer", ("упавший процесс", "упавших процесса", "упавших процессов")),
                        ("user", ("сообщение", "сообщения", "сообщений"))):
        n = day.get(kind, 0)
        if n:
            parts.append(f"{n} {plural(n, *forms)}")
    tail = ("За сутки: " + ", ".join(parts) + ".") if parts else "За сутки — ни одного."
    return ("Вылеты, упавшие процессы страниц и «Сообщить о проблеме», 30 последних. "
            "Адреса страниц и личные данные браузер вычищает ещё до отправки. " + tail)


REPORT_CSS = """
  :root { color-scheme: dark; --bg:#0e1016; --surface:#171a23; --line:#232735; --ink:#e7e9f0;
    --ink2:#b5b9c8; --muted:#8b90a3; --acc:#3987e5; }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--ink);
    font:15px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }
  .wrap { max-width:1100px; margin:0 auto; padding:28px 20px 64px; }
  a { color:#86b6ef; text-decoration:none; } a:hover { text-decoration:underline; }
  h1 { font-size:20px; margin:14px 0 2px; }
  .top-sub { color:var(--muted); font-size:13px; margin-bottom:18px; }
  h2 { font-size:13px; text-transform:uppercase; letter-spacing:.08em; color:var(--muted);
    margin:26px 0 8px; font-weight:600; }
  pre { margin:0; background:var(--surface); border:1px solid var(--line); border-radius:14px;
    padding:14px 16px; font:12.5px/1.55 ui-monospace,"Cascadia Code",Consolas,monospace;
    white-space:pre-wrap; word-break:break-word; color:var(--ink2); }
  pre.text { color:var(--ink); }
  .box { background:var(--surface); border:1px solid var(--line); border-radius:14px; padding:14px 16px; }
  .ans-now { margin:0 0 14px; color:var(--ink2); }
  .ans-now b { color:var(--ink); }
  .ans-now pre { margin-top:8px; }
  form { display:grid; gap:10px; }
  .opts { display:flex; gap:8px; flex-wrap:wrap; }
  .opts label { display:flex; align-items:center; gap:6px; padding:6px 12px; border:1px solid var(--line);
    border-radius:99px; cursor:pointer; color:var(--ink2); }
  .opts label:has(input:checked) { border-color:var(--acc); color:var(--ink); background:#16324f; }
  .opts input { accent-color:var(--acc); margin:0; }
  input[type=text], textarea { width:100%; background:var(--bg); color:var(--ink); border:1px solid var(--line);
    border-radius:10px; padding:9px 11px; font:inherit; }
  textarea { min-height:120px; resize:vertical; }
  .row { display:flex; gap:10px; align-items:center; flex-wrap:wrap; }
  button { font:inherit; font-weight:600; padding:8px 16px; border-radius:10px; border:0; cursor:pointer;
    background:var(--acc); color:#fff; }
  button.ghost { background:transparent; color:var(--muted); border:1px solid var(--line); font-weight:500; }
  .err { color:#ffb4a3; background:#4a1f1a; padding:8px 12px; border-radius:10px; margin:0 0 12px; }
  .ans { display:inline-block; padding:1px 8px; border-radius:99px; font-size:12px; white-space:nowrap; }
  .a-fixed { background:#163d2a; color:#8fe3b0; }
  .a-reply { background:#16324f; color:#9cc8f5; }
  .a-wait { background:#232735; color:var(--muted); }
  .nav { display:flex; justify-content:space-between; margin-top:22px; font-size:14px; }
  .muted { color:var(--muted); }
  .files { display:grid; grid-template-columns:repeat(auto-fill,minmax(320px,1fr)); gap:14px; }
  .file { margin:0; background:var(--surface); border:1px solid var(--line); border-radius:14px;
    overflow:hidden; display:flex; flex-direction:column; }
  .file.wide { grid-column:1/-1; }
  .file img, .file video { display:block; width:100%; max-height:70vh; object-fit:contain; background:#000; }
  .file pre { border:0; border-radius:0; max-height:480px; overflow:auto; }
  .file figcaption { padding:9px 14px; font-size:13px; color:var(--ink2); display:flex; gap:8px;
    flex-wrap:wrap; border-top:1px solid var(--line); }
  .file figcaption b { font-weight:600; color:var(--ink); word-break:break-all; }
  .file.gone { padding:12px 14px; color:var(--muted); font-size:13px; }
  @media (max-width:700px) {
    .wrap { padding:20px 12px 48px; }
    pre { padding:12px; font-size:12px; }
    .files { grid-template-columns:1fr; }
  }
"""


def answer_html(id_, kind, token, ans, ans_text, ans_v, ans_at, ans_got, error=None, form=None,
                old=(0, [])):
    """Раздел «Ответ человеку» на странице отчёта: что уже ответили и форма."""
    err = f'<p class="err">{escape(error)}</p>' if error else ""
    if kind != "user":
        return err
    k, pre = old
    note = ""
    if not token and not pre:
        return (f'<h2>Ответ человеку</h2>{err}<p class="muted">Ответить нельзя: сообщение пришло '
                'из старого Axiom без журнала и без строки о системе — не по чему узнать отправителя.</p>')
    if not token:
        how = ("по строкам запуска из журнала" if k else
               "по версии, системе и часу отправки (журнал не приложен — узнается менее надёжно)")
        note = ('<p class="muted">Сообщение из Axiom, который ещё не давал ключа ответа. Ответ дойдёт, '
                'когда человек обновится до 0.6.1-beta.152 или новее: его браузер узнает своё сообщение '
                f'{how}, пока они есть в журнале.</p>')
    now = ""
    if ans:
        when = f'<time data-utc="{escape(ans_at or "")}">{escape((ans_at or "").replace("T", " "))}</time>'
        got = (f'Браузер забрал ответ <time data-utc="{escape(ans_got)}">{escape(ans_got.replace("T", " "))}</time> — '
               'человек его видит.' if ans_got else
               'Браузер ещё не забрал ответ: он спрашивает раз в 20 минут, пока Axiom открыт.' if token else
               'Браузер ещё не узнал своё сообщение.')
        ver = f" · версия {escape(ans_v)}" if ans_v else ""
        body = f'<pre class="text">{escape(ans_text)}</pre>' if ans_text else ""
        now = (f'<div class="ans-now"><b>Ответ: {escape(ANSWERS.get(ans, ans))}</b>{ver} · {when}<br>'
               f'{got}{body}</div>')
    # После ошибки форма сохраняет то, что ввели.
    f = form if form is not None else {"s": ans or "fixed", "text": ans_text or "", "v": ans_v or ""}
    s = f.get("s") if f.get("s") in ANSWERS else "fixed"
    opts = "".join(
        f'<label><input type="radio" name="s" value="{k}"{" checked" if k == s else ""}> {label}</label>'
        for k, label in (("fixed", "Исправлено"), ("reply", "Ответ")))
    clear = '<button class="ghost" name="do" value="clear">Убрать ответ</button>' if ans else ""
    return f"""<h2>Ответ человеку</h2>
  <div class="box">{err}{note}{now}
  <form method="post" action="{report_href(id_)}" accept-charset="utf-8">
    <div class="opts">{opts}</div>
    <input type="text" name="v" value="{escape(f.get("v", ""))}" maxlength="40"
      placeholder="В какой версии исправлено (можно пусто): 0.6.1-beta.150">
    <textarea name="text" maxlength="{MAX_ANSWER_TEXT}"
      placeholder="Что сделали или что ответить. Для «Исправлено» можно пусто.">{escape(f.get("text", ""))}</textarea>
    <div class="row"><button>{"Изменить ответ" if ans else "Отправить ответ"}</button>{clear}
      <span class="muted">Человек увидит ответ в Axiom в течение 20 минут.</span></div>
  </form></div>"""


def human_size(n):
    if n < 1024:
        return f"{n} Б"
    if n < 1024 * 1024:
        return f"{max(1, round(n / 1024))} КБ"
    return f"{n / 1024 / 1024:.1f} МБ".replace(".", ",")


def file_href(id_, n):
    return f"{report_href(id_)}/f/{int(n)}"


def files_html(id_, files):
    """Файлы отчёта: картинки и видео — прямо на странице, текст — началом."""
    if not files:
        return ""
    items = []
    for n, name, type_, size, gone in files:
        what = f"<b>{escape(name)}</b><span class=\"muted\">{human_size(size)}</span>"
        if gone:
            items.append(f'<div class="file gone">{what} — удалён: файлы к отчётам заняли всё '
                         f'отведённое им место</div>')
            continue
        href = file_href(id_, n)
        cls = "file"
        if type_.startswith("image/"):
            body = f'<a href="{href}"><img src="{href}" alt="{escape(name)}" loading="lazy"></a>'
        elif type_.startswith("video/"):
            body = f'<video src="{href}" controls preload="metadata"></video>'
        else:
            cls = "file wide"
            try:
                with open(file_path(id_, n), "rb") as f:
                    text = f.read(TEXT_PREVIEW).decode("utf-8", "replace")
            except OSError:
                text = "(файл не прочитать)"
            more = f"\n…ещё {human_size(size - TEXT_PREVIEW)} — целиком по ссылке «скачать»" \
                if size > TEXT_PREVIEW else ""
            body = f'<pre class="log">{escape(text + more)}</pre>'
        items.append(f'<figure class="{cls}">{body}<figcaption>{what}'
                     f'<a href="{href}?dl=1">скачать</a></figcaption></figure>')
    return f'<h2>Файлы</h2><div class="files">{"".join(items)}</div>'


def report_page(id_, nonce="", error=None, form=None):
    """Один отчёт целиком. Всё, что прислал браузер, — только через escape()."""
    con = db()
    row = con.execute(
        """SELECT id, at, kind, v, os, text, log, token, ans, ans_text, ans_v, ans_at, ans_got
           FROM reports WHERE id=?""", (id_,)).fetchone()
    older = con.execute("SELECT MAX(id) FROM reports WHERE id<?", (id_,)).fetchone()[0]
    newer = con.execute("SELECT MIN(id) FROM reports WHERE id>?", (id_,)).fetchone()[0]
    files = con.execute(
        "SELECT n, name, type, size, gone FROM report_files WHERE report=? ORDER BY n",
        (id_,)).fetchall()
    con.close()
    if not row:
        return None
    _, at, kind, v, os_, text, log, *answer = row
    what = REPORT_KINDS.get(kind, kind)
    answer = answer_html(id_, kind, *answer, error=error, form=form,
                         old=old_fingerprints(text, log, at))
    log_html = (f'<pre class="log">{escape(log)}</pre>' if log.strip()
                else '<p class="muted">Журнал не приложен.</p>')
    nav = (f'<a href="{report_href(newer)}">← новее</a>' if newer else "<span></span>") + \
          (f'<a href="{report_href(older)}">старше →</a>' if older else "<span></span>")
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Axiom — отчёт №{int(id_)}</title>
<style>{REPORT_CSS}</style></head><body><div class="wrap">
  <a href="/d/{escape(KEY)}#reports">← Все отчёты</a>
  <h1>Отчёт №{int(id_)}: {escape(what)}</h1>
  <div class="top-sub"><time data-utc="{escape(at)}">{escape(at.replace("T", " "))}</time>
    · версия {escape(v)} · {escape(os_)}</div>
  <h2>Что случилось</h2>
  <pre class="text">{escape(text) or "—"}</pre>
  {files_html(id_, files)}
  {answer}
  <h2>Журнал</h2>
  {log_html}
  <div class="nav">{nav}</div>
</div>
<script nonce="{escape(nonce)}">
for (const el of document.querySelectorAll("[data-utc]")) {{
  const d = new Date(el.dataset.utc);
  if (!isNaN(d)) el.textContent = d.toLocaleString("ru-RU");
}}
</script>
</body></html>"""


def render(s, nonce=""):
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
        tile(f'≈{s["online"]}', "Сейчас", "браузеров было открыто за последний час", True),
        tile(s["dau"], "Сегодня",
             f'разных людей с <time data-utc="{midnight}" data-fmt="hm">00:00 UTC</time>'),
        tile(s["wau"], "За неделю", week_sub),
        tile(s["mau"], "За месяц", f"с 1 {month_name}"),
    ])
    small = "".join([
        tile(f'+{s["inst_today"]}', "Новые установки сегодня", f'всего за всё время: {s["inst_all"]}'),
        tile(s["upd_today"], "Обновились сегодня", f'всего за всё время: {s["upd_all"]}'),
        tile(s["imp_all"], "Переехали за всё время",
             f'перенесли закладки и историю; сегодня +{s["imp_today"]}'),
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
<noscript><meta http-equiv="refresh" content="60"></noscript>
<title>Axiom — статистика</title>
<style>
  :root {{ color-scheme: dark;
    --bg:#0e1016; --surface:#171a23; --line:#232735; --ink:#e7e9f0; --ink2:#b5b9c8;
    --muted:#8b90a3; --axis:#383c4a; --old:#3987e5; --new:#d95926; --upd:#199e70; --imp:#a274e8; }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; background:var(--bg); color:var(--ink);
    font:15px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
  .wrap {{ max-width:960px; margin:0 auto; padding:0 20px 64px; }}
  h1 {{ font-size:20px; margin:0 0 2px; }}
  .top-sub {{ color:var(--muted); font-size:13px; }}
  [hidden] {{ display:none !important; }}
  /* Шапка с вкладками: остаётся наверху, пока листаешь вкладку. */
  .head {{ position:sticky; top:0; z-index:5; margin:0 -20px 22px; padding:22px 20px 0;
    background:color-mix(in oklab,var(--bg) 90%,transparent); backdrop-filter:blur(12px);
    border-bottom:1px solid var(--line); }}
  .tabs {{ display:flex; gap:2px; margin-top:12px; overflow-x:auto; scrollbar-width:none; }}
  .tabs a {{ display:flex; align-items:center; gap:7px; padding:9px 12px 11px; color:var(--ink2);
    text-decoration:none; border-bottom:2px solid transparent; white-space:nowrap; font-weight:500; }}
  .tabs a:hover {{ color:var(--ink); }}
  .tabs a.on {{ color:var(--ink); border-color:#5b8def; }}
  .tabs a.ext {{ margin-left:auto; color:#86b6ef; }}
  .tabs a i {{ font-style:normal; font-size:11.5px; font-weight:700; min-width:20px; text-align:center;
    padding:1px 7px; border-radius:99px; background:#3d5fd6; color:#fff; }}
  section.tab > h2:first-child {{ margin-top:4px; }}
  .how {{ color:var(--ink2); font-size:14px; }}
  .how ul {{ padding-left:18px; margin:0; }}
  .how li {{ margin:8px 0; }}
  .tiles {{ display:grid; grid-template-columns:repeat(4,1fr); gap:12px; }}
  .tiles.small {{ grid-template-columns:repeat(3,1fr); margin-top:12px; }}
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
  .d-new {{ background:var(--new); }} .d-upd {{ background:var(--upd); }} .d-imp {{ background:var(--imp); }}
  .feed-empty {{ color:var(--muted); font-size:14px; }}
  .grid {{ display:grid; grid-template-columns:3fr 2fr; gap:16px; }}
  table {{ width:100%; border-collapse:collapse; background:var(--surface);
    border:1px solid var(--line); border-radius:14px; overflow:hidden; }}
  th,td {{ text-align:left; padding:9px 14px; font-size:14px; }}
  th {{ color:var(--muted); font-weight:500; font-size:12px; background:#12141c; }}
  tr+tr td {{ border-top:1px solid #1d212c; }}
  td:not(:first-child),th:not(:first-child) {{ text-align:right; font-variant-numeric:tabular-nums; }}
  .muted {{ color:#6b7083; }}
  .reps td,.reps th {{ text-align:left; }}
  .reps td {{ vertical-align:top; }}
  .reps time {{ color:var(--muted); font-size:13px; white-space:nowrap; font-variant-numeric:tabular-nums; }}
  .reps .line {{ width:55%; word-break:break-word; }}
  .reps a {{ color:var(--ink); text-decoration:none; }}
  .reps a:hover {{ text-decoration:underline; }}
  .kind {{ display:inline-block; padding:1px 8px; border-radius:99px; font-size:12px; white-space:nowrap;
    background:#232735; color:var(--ink2); }}
  .k-crash {{ background:#4a1f1a; color:#ffb4a3; }}
  .k-renderer {{ background:#3d3315; color:#f1d48a; }}
  .k-user {{ background:#16324f; color:#9cc8f5; }}
  .ans {{ display:inline-block; padding:1px 8px; border-radius:99px; font-size:12px; white-space:nowrap; }}
  .a-fixed {{ background:#163d2a; color:#8fe3b0; }}
  .a-reply {{ background:#16324f; color:#9cc8f5; }}
  .a-wait {{ background:#232735; color:var(--muted); }}
  .clip {{ color:var(--muted); font-size:12px; white-space:nowrap; margin-left:6px; }}
  .chart-svg.narrow {{ display:none; }}
  @media (max-width:700px) {{ .tiles {{ grid-template-columns:repeat(2,1fr); }}
    .tiles.small {{ grid-template-columns:1fr; }} .grid {{ grid-template-columns:1fr; }}
    .wrap {{ padding:0 12px 48px; }}
    .head {{ margin:0 -12px 18px; padding:18px 12px 0; }}
    .tile {{ padding:12px 14px; }}
    .tile .n {{ font-size:30px; }}
    .panel {{ padding:12px 14px; }}
    th,td {{ padding:9px 12px; }}
    /* Отчёты на телефоне: строка таблицы — карточка. Сверху что и когда, справа ответ,
       ниже — о чём во всю ширину, под ним мелко версия и система. */
    .reps thead {{ display:none; }}
    .reps, .reps tbody {{ display:block; }}
    .reps tr {{ display:flex; flex-wrap:wrap; align-items:center; gap:6px 10px; padding:12px 14px; }}
    .reps tr+tr {{ border-top:1px solid #1d212c; }}
    .reps td, .reps tr+tr td {{ display:block; padding:0; border:0; }}
    .reps .c-kind {{ order:0; }}
    .reps .c-at {{ order:1; }}
    .reps .c-ans {{ order:2; margin-left:auto; }}
    .reps .c-ans:empty {{ display:none; }}
    .reps .line {{ order:3; flex-basis:100%; width:auto; font-size:15px; line-height:1.4; }}
    .reps .c-v, .reps .c-os {{ order:4; font-size:12px; color:var(--muted); }}
    .reps .c-os {{ order:5; }}
  }}
  @media (max-width:560px) {{ .chart-svg.wide {{ display:none; }} .chart-svg.narrow {{ display:block; }} }}
</style></head><body><div class="wrap">
  <header class="head"><h1>Axiom — сколько людей пользуется</h1>
  <div class="top-sub">Обновлено в <time data-utc="{utc_iso(now)}" data-fmt="hm">{now:%H:%M} UTC</time>.
    Страница обновляется сама раз в минуту.</div>
  <nav class="tabs">
    <a href="#overview" data-tab="overview">Сводка</a>
    <a href="#reports" data-tab="reports">Отчёты{f"<i>{s['waiting']}</i>" if s["waiting"] else ""}</a>
    <a href="#charts" data-tab="charts">Графики</a>
    <a href="#versions" data-tab="versions">Версии и системы</a>
    <a href="#how" data-tab="how">Как считается</a>
    <a class="ext" href="{escape(STORE_ADMIN_URL)}" target="_blank" rel="noopener noreferrer"
      title="Админка магазина плагинов: заявки, проверка, плагины">Плагины ↗</a>
  </nav></header>

  <section class="tab" id="overview">
  <div class="tiles">{tiles}</div>
  <div class="tiles small">{small}</div>

  <h2>Что происходило</h2>
  <p class="hint">Новые установки, обновления и переезды из других браузеров за последние двое суток,
    с точностью до 10 минут.</p>
  <div class="panel">{feed_html(s["feed"])}</div>
  </section>

  <section class="tab" id="reports">
  <h2>Отчёты</h2>
  <p class="hint">{reports_hint(s["reports_day"])}</p>
  {reports_html(s["reports"])}
  </section>

  <section class="tab" id="charts">
  <h2>По часам, последние сутки</h2>
  <p class="hint">Сколько браузеров было открыто в каждый час. Открытый браузер отмечается раз в час,
    поэтому столбик ≈ число людей. Последний столбик — текущий час, он ещё растёт.</p>
  <div class="panel">{chart_hours(s["hours"])}{chart_hours(s["hours"], NARROW_W)}</div>

  <h2>По дням, 30 дней</h2>
  <p class="hint">Сколько разных людей открывали браузер в каждый день.</p>
  <div class="panel">
    <div class="legend"><span><i style="background:var(--old)"></i>уже пользовались</span>
      <span><i style="background:var(--new)"></i>новые установки</span></div>
    {chart_days(s["days"])}{chart_days(s["days"], NARROW_W)}
  </div>
  </section>

  <section class="tab" id="versions">
  <div class="grid">
    <div><h2>Версии</h2><table><thead><tr><th>версия</th><th>людей сегодня</th>
      <th>обновились на неё</th></tr></thead><tbody>{vrows}</tbody></table></div>
    <div><h2>Системы</h2><table><thead><tr><th>система</th><th>людей сегодня</th></tr></thead>
      <tbody>{orows}</tbody></table></div>
  </div>
  </section>

  <section class="tab how" id="how">
  <h2>Как это считается</h2>
  <div class="panel"><ul>
    <li>Пока браузер открыт, он раз в час шлёт короткую отметку: версию, систему и пару флажков
      («первый раз сегодня», «новая установка», «только что обновился»). Ни имён, ни адресов, ни IP.</li>
    <li>«Сейчас» — у скольких браузер был открыт за последние 60 минут. Открытый браузер
      засчитывается раз в час; перезапуск (обновление, вылет, закрыл и открыл) не добавляет.
      Старые версии (0.6.1 и раньше) засчитывают и перезапуски, поэтому ≈. Больше, чем «Сегодня», не бывает.</li>
    <li>Сутки, недели и месяцы считаются по UTC: сутки начинаются в
      <time data-utc="{midnight}" data-fmt="hm">00:00 UTC</time> по твоему времени.</li>
    <li>Отметок сегодня: {pt}. Это не люди: один человек, который просидел в браузере 5 часов, даёт 5 отметок.</li>
    <li>Об обновлениях браузер сообщает начиная с версии 0.4.0. Кто обновится на неё с более
      старой версии, тоже попадёт в счёт.</li>
    <li>«Людей сегодня» по версиям в сумме может быть больше, чем «Сегодня»: тот, кто обновился
      днём, был сегодня на двух версиях.</li>
    <li>Бета-версии отмечаются своим номером (0.6.3-beta.190), переход на следующую бету —
      обновление. В таблице бета видна, пока на ней сегодня кто-то есть. Беты до этого
      отмечались номером выпуска, на котором стоят (0.6.3), и считались вместе с ним.</li>
    <li>«Переехали» — сколько людей перенесли в Axiom закладки или историю из другого браузера.
      Каждый считается один раз, повторный импорт не добавляет. Какой это был браузер и что именно
      перенесли, не отправляется.</li>
    <li>Кто импортировал до появления этого счётчика, попадает в «за всё время», когда обновится:
      браузер один раз смотрит в профиль (импортированные закладки лежат в папках, а история бывает
      старше самого Axiom). В «сегодня» такие не идут. Найдено так: {s["imp_old"]}. Кто импортировал
      и больше не открывает Axiom, не посчитается.</li>
    <li>Отчёты об ошибках браузер шлёт сам, когда вылетел или упал процесс страницы (если человек
      не выключил это в настройках), и по кнопке «Сообщить о проблеме». Адреса страниц, пути с именем
      пользователя, почту и токены он вычищает ещё до отправки. Хранятся 90 дней, не больше {MAX_REPORTS}.</li>
    <li>К «Сообщить о проблеме» можно приложить снимки экрана, фото, видео и текст (📎 в списке):
      до {MAX_FILES} файлов и {MAX_FILES_BYTES // 1024 // 1024} МБ на отчёт. У фото и видео браузер
      убирает место съёмки и данные камеры. Всем файлам вместе отведено {FILES_KEEP_BYTES // 1024 // 1024} МБ:
      больше — самые старые файлы удаляются, сами отчёты остаются.</li>
  </ul></div>
  </section>
</div>
<script nonce="{escape(nonce)}">
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
  const tabs = [...document.querySelectorAll("[data-tab]")];
  const secs = [...document.querySelectorAll("section.tab")];
  const pick = () => {{
    const want = location.hash.slice(1);
    const id = secs.some(s => s.id === want) ? want : "overview";
    for (const s of secs) s.hidden = s.id !== id;
    for (const t of tabs) t.classList.toggle("on", t.dataset.tab === id);
    scrollTo(0, 0);
  }};
  pick();
  addEventListener("hashchange", pick);
  // Раз в минуту — свежие цифры на той же вкладке.
  setTimeout(() => location.reload(), 60000);
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

    def _send(self, code, body=b"", ctype="text/plain; charset=utf-8", nonce=None, forms=False):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Content-Type-Options", "nosniff")
        if nonce is not None:
            # Панель показывает присланный кем угодно текст: даже если escape() где-то
            # пропустят, чужой скрипт не выполнится. Ключ в адресе не уйдёт в Referer.
            # Формы — только на странице отчёта (ответ человеку) и только на этот же сервер.
            # Картинки и видео — только файлы отчётов с этого же сервера.
            action = "'self'" if forms else "'none'"
            self.send_header(
                "Content-Security-Policy",
                f"default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'; "
                "img-src 'self'; media-src 'self'; "
                f"base-uri 'none'; form-action {action}; frame-ancestors 'none'")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Cache-Control", "no-store")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def handle(self):
        # Клиент закрыл соединение, не дождавшись ответа (браузер закрыли посреди
        # отправки), — это не ошибка сервера, и трассировка в журнале не нужна.
        try:
            super().handle()
        except (ConnectionError, TimeoutError):
            self.close_connection = True

    def drain(self, length, cap=1 << 20):
        left = min(length, cap)
        while left > 0:
            chunk = self.rfile.read(min(left, 65536))
            if not chunk:
                break
            left -= len(chunk)

    def body(self, limit):
        """Тело запроса или None (ответ об ошибке уже отправлен)."""
        try:
            length = int(self.headers.get("Content-Length", 0))
        except ValueError:
            self._send(400, b"bad")
            return None
        # Тело дочитываем и при отказе: закрытый с непрочитанными данными сокет
        # даёт клиенту RST вместо ответа.
        if length > limit:
            self.drain(length)
            self._send(413, b"too large")
            return None
        return self.rfile.read(length) if length > 0 else b""

    def do_POST(self):
        path = self.path
        m = re.fullmatch(r"/d/([^/?#]+)/r/(\d{1,12})/?", path)
        if m:
            return self.post_answer(m)
        if path not in LIMITERS:
            return self._send(404, b"not found")
        if path == "/rf":
            return self.post_files()
        limit = {"/p": MAX_BODY, "/a": MAX_ASK_BODY}.get(path, MAX_REPORT_BODY)
        raw = self.body(limit)
        if raw is None:
            return
        ip = client_ip(self.client_address[0], self.headers.get("X-Forwarded-For", ""))
        if not LIMITERS[path].allow(ip):
            return self._send(429, b"too many")
        if not raw:
            return self._send(400, b"bad")
        try:
            payload = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return self._send(400, b"bad")
        if path == "/a":
            ask = check_ask(payload)
            if ask is None:
                return self._send(400, b"bad")
            tokens, old, claims = ask
            reply = {"answers": answers(tokens)}
            if old:
                reply["old"] = old_answers()
            if claims:
                reply["claimed"] = claim(claims)
            out = json.dumps(reply, ensure_ascii=False).encode("utf-8")
            return self._send(200, out, "application/json; charset=utf-8")
        if path == "/p":
            ok = isinstance(payload, dict) and record(payload)
        else:
            report = check_report(payload)
            ok = report is not None and bool(store_report(report))
        if not ok:
            return self._send(400, b"bad")
        self._send(204)

    def post_answer(self, m):
        """Ответ человеку с формы на странице отчёта (или `curl --data-urlencode`)."""
        raw = self.body(MAX_ANSWER_BODY)
        if raw is None:
            return
        if not KEY or not hmac.compare_digest(m[1].encode(), KEY.encode()):
            return self._send(404, b"not found")
        try:
            form = {k: v[0] for k, v in parse_qs(
                raw.decode("utf-8"), keep_blank_values=True, max_num_fields=10).items()}
        except (UnicodeDecodeError, ValueError):
            return self._send(400, b"bad")
        if form.get("do") == "clear":
            form["s"] = "clear"
        id_ = int(m[2])
        error = set_answer(id_, form)
        if error is None:
            # Назад на страницу отчёта: обновление страницы не пришлёт форму второй раз.
            self.send_response(303)
            self.send_header("Location", report_href(id_))
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        nonce = secrets.token_urlsafe(16)
        page = report_page(id_, nonce, error=error, form=form)
        if page is None:
            return self._send(404, b"not found")
        self._send(400, page.encode("utf-8"), "text/html; charset=utf-8", nonce, forms=True)
    def post_files(self):
        """Отчёт с файлами: строка JSON (отчёт и список файлов), `\\n`, файлы подряд.
        Файлы идут прямо на диск, в память целиком не читаются."""
        try:
            length = int(self.headers.get("Content-Length", 0))
        except ValueError:
            return self._send(400, b"bad")
        if length > MAX_REPORT_BODY + 1 + MAX_FILES_BYTES:
            self.drain(length)
            return self._send(413, b"too large")
        ip = client_ip(self.client_address[0], self.headers.get("X-Forwarded-For", ""))
        if not LIMITERS["/rf"].allow(ip) or not FILE_BYTES.allow(ip, length):
            self.drain(length)
            return self._send(429, b"too many")
        # Видео на медленной связи идёт долго: каждый кусок ждём дольше обычного.
        self.connection.settimeout(60)
        line = self.rfile.readline(min(length, MAX_REPORT_BODY + 2))
        left = length - len(line)
        try:
            payload = json.loads(line) if line.endswith(b"\n") else None
        except (ValueError, UnicodeDecodeError):
            payload = None
        report = check_report(payload)
        files = check_files(payload, left) if report else None
        if files is None:
            self.drain(left)
            return self._send(400, b"bad")
        keep = room_for(left)
        got = receive_files(self.rfile, files, keep)
        if got is None:
            return self._send(400, b"bad")
        tmp, stored = got
        if not keep:
            kind, v, os_, text, log, token = report
            report = (kind, v, os_, text + "\n\n(Файлы не сохранены: на сервере мало места.)",
                      log, token)
        try:
            ok = bool(store_report(report, stored, tmp))
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        self._send(204 if ok else 400)

    def send_file(self, rid, n, download):
        """Файл отчёта. Тип — только из списка, определённого сервером по байтам;
        скрипты не выполнятся (sandbox), видео можно перематывать (Range)."""
        con = db()
        row = con.execute(
            "SELECT name, type FROM report_files WHERE report=? AND n=? AND gone=0",
            (rid, n)).fetchone()
        con.close()
        if not row or row[1] not in FILE_TYPES:
            return self._send(404, b"not found")
        name, type_ = row
        try:
            f = open(file_path(rid, n), "rb")
        except OSError:
            return self._send(404, b"not found")
        with f:
            size = os.fstat(f.fileno()).st_size
            start, end, status = 0, size - 1, 200
            m = re.fullmatch(r"bytes=(\d*)-(\d*)", self.headers.get("Range", "").strip())
            if m and (m[1] or m[2]):
                if m[1]:
                    start, end = int(m[1]), min(int(m[2]) if m[2] else size - 1, size - 1)
                else:
                    start = max(0, size - int(m[2]))
                if start > end:
                    self.send_response(416)
                    self.send_header("Content-Range", f"bytes */{size}")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                status = 206
            self.send_response(status)
            self.send_header("Content-Type", type_)
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("Accept-Ranges", "bytes")
            if status == 206:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Security-Policy", "default-src 'none'; sandbox")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Cache-Control", "private, max-age=86400")
            plain = re.sub(r"[^A-Za-z0-9._-]", "_", name) or "file"
            self.send_header(
                "Content-Disposition",
                f"{'attachment' if download else 'inline'}; filename=\"{plain}\"; "
                f"filename*=UTF-8''{quote(name, safe='')}")
            self.end_headers()
            if self.command == "HEAD":
                return
            f.seek(start)
            left = end - start + 1
            try:
                while left > 0:
                    chunk = f.read(min(left, 65536))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    left -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass  # перемотали видео — браузер бросил прежний запрос

    def do_GET(self):
        url = urlsplit(self.path)
        if url.path == "/health":
            return self._send(200, b"ok")
        m = re.fullmatch(r"/d/([^/?#]+)(?:/r/(\d{1,12})(?:/f/(\d{1,3}))?)?/?", url.path)
        if KEY and m and hmac.compare_digest(m[1].encode(), KEY.encode()):
            if m[3] is not None:
                return self.send_file(int(m[2]), int(m[3]), url.query == "dl=1")
            nonce = secrets.token_urlsafe(16)
            page = dashboard(nonce) if m[2] is None else report_page(int(m[2]), nonce)
            if page is not None:
                return self._send(200, page.encode("utf-8"), "text/html; charset=utf-8", nonce,
                                  forms=m[2] is not None)
        self._send(404, b"not found")

    do_HEAD = do_GET


class Server(ThreadingHTTPServer):
    """Поток на соединение, но не больше MAX_CONNECTIONS сразу: лишние
    соединения закрываются без ответа, а не копят потоки."""

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
        # Обрыв соединения клиентом — обычное дело, остальное — в журнал.
        if isinstance(sys.exc_info()[1], (ConnectionError, TimeoutError)):
            return
        super().handle_error(request, client_address)


def main():
    init()
    srv = Server((HOST, PORT), Handler)
    print(f"axiom-stats на http://{HOST}:{PORT} (база {DB})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
