#!/usr/bin/env python3
"""Проверки сборщика: приём отчётов об ошибках, экранирование в панели,
ограничение частоты, миграция базы и счёт «сейчас».

Запуск из корня репозитория: python -m unittest discover -s server -v
"""

import contextlib
import http.client
import io
import json
import os
import socket
import sqlite3
import struct
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stats  # noqa: E402

KEY = "testkey123"


def report(**over):
    r = {"kind": "crash", "version": "0.5.4", "os": "windows",
         "text": "Axiom 0.5.4 · Windows 11 · WebView2 142\n\nПаника: всё сломалось", "log": "строка журнала\n"}
    r.update(over)
    return r


class Server(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        stats.DB = os.path.join(cls.tmp.name, "test.db")
        stats.KEY = KEY
        stats.init()
        cls.srv = stats.Server(("127.0.0.1", 0), stats.Handler)
        cls.port = cls.srv.server_address[1]
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()
        cls.tmp.cleanup()

    def setUp(self):
        # Свежие вёдра: тесты не делят лимиты между собой.
        stats.LIMITERS = {p: stats.Limiter(*lim) for p, lim in stats.LIMITS.items()}
        con = stats.db()
        with con:
            con.execute("DELETE FROM reports")
        con.close()
        self.ip = 0

    def request(self, method, path, body=None, ip=None, headers=None):
        con = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        elif isinstance(body, str):
            body = body.encode()
        h = {"Content-Type": "application/json"}
        if ip is None:
            # Каждый запрос — со своего «адреса», если тест не про лимиты.
            self.ip += 1
            ip = f"198.51.100.{self.ip}"
        h["X-Forwarded-For"] = ip
        h.update(headers or {})
        con.request(method, path, body=body, headers=h)
        resp = con.getresponse()
        data = resp.read()
        con.close()
        return resp.status, data.decode("utf-8", "replace"), resp

    def rows(self):
        con = stats.db()
        rows = con.execute("SELECT kind, v, os, text, log FROM reports ORDER BY id").fetchall()
        con.close()
        return rows

    # ---------- /r ----------

    def test_valid_report_is_stored_without_ip(self):
        status, _, _ = self.request("POST", "/r", report())
        self.assertEqual(status, 204)
        self.assertEqual(self.rows(), [("crash", "0.5.4", "windows", report()["text"], "строка журнала\n")])
        con = stats.db()
        cols = {r[1] for r in con.execute("PRAGMA table_info(reports)")}
        con.close()
        # Ключ ответа и сам ответ — без IP и без чего-либо о браузере.
        self.assertEqual(cols, {"id", "at", "kind", "v", "os", "text", "log",
                                "token", "ans", "ans_text", "ans_v", "ans_at", "ans_got"})

    def test_all_kinds_and_beta_versions(self):
        for kind in ("crash", "renderer", "user"):
            status, _, _ = self.request("POST", "/r", report(kind=kind, version="0.5.4-beta.312"))
            self.assertEqual(status, 204, kind)
        # Без журнала — можно, без текста, но с журналом — тоже.
        self.assertEqual(self.request("POST", "/r", report(log=""))[0], 204)
        payload = report()
        del payload["text"]
        self.assertEqual(self.request("POST", "/r", payload)[0], 204)
        self.assertEqual(len(self.rows()), 5)

    def test_bad_reports_are_rejected(self):
        bad = [
            report(kind="hack"), report(kind=1), report(version="1.2"),
            report(version="1.2.3\n"), report(version="0.5.4-rc1"), report(version=None),
            report(os="plan9"), report(text=5), report(log=["x"]),
            report(text="x" * (stats.MAX_REPORT_TEXT + 1)),
            report(text="  ", log="\n"),
            [report()], "не json", "",
        ]
        for body in bad:
            status, _, _ = self.request("POST", "/r", body)
            self.assertEqual(status, 400, repr(body)[:80])
        self.assertEqual(self.rows(), [])

    def test_too_large_body_is_refused_before_reading(self):
        body = json.dumps(report(log="x" * (stats.MAX_REPORT_BODY + 10)))
        status, _, _ = self.request("POST", "/r", body)
        self.assertEqual(status, 413)
        # Ровно 64 КБ тела — ещё можно.
        payload = report(log="")
        room = stats.MAX_REPORT_BODY - len(json.dumps(payload).encode())
        payload["log"] = "y" * room
        self.assertEqual(len(json.dumps(payload).encode()), stats.MAX_REPORT_BODY)
        self.assertEqual(self.request("POST", "/r", payload)[0], 204)

    def test_unknown_paths(self):
        self.assertEqual(self.request("POST", "/x", report())[0], 404)
        self.assertEqual(self.request("GET", "/r")[0], 404)

    # ---------- панель ----------

    def test_dashboard_and_report_page_escape_everything(self):
        evil = report(
            version="0.5.4", kind="user",
            text='<script>alert(1)</script>\n<img src=x onerror="alert(2)">',
            log='</pre><script>alert(3)</script>&amp;')
        self.assertEqual(self.request("POST", "/r", evil)[0], 204)
        con = stats.db()
        rid = con.execute("SELECT MAX(id) FROM reports").fetchone()[0]
        con.close()

        status, page, resp = self.request("GET", f"/d/{KEY}")
        self.assertEqual(status, 200)
        self.assertIn("Отчёты", page)
        self.assertIn("&lt;script&gt;alert(1)&lt;/script&gt;", page)
        self.assertNotIn("<script>alert", page)
        self.assertIn(f"/d/{KEY}/r/{rid}", page)
        self.check_headers(resp, page)

        status, page, resp = self.request("GET", f"/d/{KEY}/r/{rid}")
        self.assertEqual(status, 200)
        self.assertIn("&lt;script&gt;alert(1)&lt;/script&gt;", page)
        self.assertIn("&lt;img src=x onerror=&quot;alert(2)&quot;&gt;", page)
        self.assertIn("&lt;/pre&gt;&lt;script&gt;alert(3)&lt;/script&gt;&amp;amp;", page)
        self.assertNotIn("<script>alert", page)
        self.assertNotIn("<img", page)
        self.check_headers(resp, page)

    def check_headers(self, resp, page):
        csp = resp.getheader("Content-Security-Policy") or ""
        self.assertIn("default-src 'none'", csp)
        self.assertEqual(resp.getheader("Referrer-Policy"), "no-referrer")
        # Каждый скрипт страницы — с одноразовым nonce из заголовка.
        nonce = csp.split("'nonce-")[1].split("'")[0]
        self.assertEqual(page.count("<script"), page.count(f'<script nonce="{nonce}">'))

    def test_report_page_needs_the_key_and_an_existing_id(self):
        self.assertEqual(self.request("POST", "/r", report())[0], 204)
        self.assertEqual(self.request("GET", "/d/wrong/r/1")[0], 404)
        self.assertEqual(self.request("GET", f"/d/{KEY}/r/999999")[0], 404)
        self.assertEqual(self.request("GET", f"/d/{KEY}/r/abc")[0], 404)
        self.assertEqual(self.request("GET", f"/d/{KEY}x")[0], 404)

    def test_first_line_skips_the_about_line(self):
        self.assertEqual(stats.first_line("Axiom 0.5.4 · Windows · WebView2\n\nПаника: x"), "Паника: x")
        self.assertEqual(stats.first_line("  \nВкладка не грузится\nподробности"), "Вкладка не грузится")
        self.assertEqual(stats.first_line(""), "")

    # ---------- ответы человеку ----------

    TOKEN = "0123456789abcdef0123456789abcdef"

    def last_id(self):
        con = stats.db()
        rid = con.execute("SELECT MAX(id) FROM reports").fetchone()[0]
        con.close()
        return rid

    def answer(self, rid, key=KEY, **fields):
        from urllib.parse import urlencode
        return self.request("POST", f"/d/{key}/r/{rid}", urlencode(fields),
                            headers={"Content-Type": "application/x-www-form-urlencoded"})

    def ask(self, *tokens):
        status, body, _ = self.request("POST", "/a", {"t": list(tokens)})
        self.assertEqual(status, 200, body)
        return json.loads(body)["answers"]

    def test_only_user_messages_keep_a_valid_answer_token(self):
        cases = [("user", self.TOKEN, self.TOKEN), ("crash", self.TOKEN, None),
                 ("user", "XYZ", None), ("user", 5, None), ("user", None, None)]
        for kind, token, kept in cases:
            self.assertEqual(self.request("POST", "/r", report(kind=kind, token=token))[0], 204)
            con = stats.db()
            got = con.execute("SELECT token FROM reports WHERE id=?", (self.last_id(),)).fetchone()[0]
            con.close()
            self.assertEqual(got, kept, (kind, token))

    def test_answer_reaches_the_browser_once_posted(self):
        self.assertEqual(self.request("POST", "/r", report(kind="user", token=self.TOKEN))[0], 204)
        rid = self.last_id()
        other = "f" * 32
        self.assertEqual(self.ask(self.TOKEN, other), {})
        status, page, resp = self.request("GET", f"/d/{KEY}/r/{rid}")
        self.assertIn("Ответ человеку", page)
        self.assertIn("form-action 'self'", resp.getheader("Content-Security-Policy"))
        self.assertIn("ждёт ответа", self.request("GET", f"/d/{KEY}")[1])

        status, _, resp = self.answer(rid, s="fixed", v="0.6.1-beta.150", text="Сделали <b>кнопку</b>")
        self.assertEqual(status, 303)
        self.assertEqual(resp.getheader("Location"), f"/d/{KEY}/r/{rid}")
        page = self.request("GET", f"/d/{KEY}/r/{rid}")[1]
        self.assertIn("ещё не забрал", page)
        self.assertIn("Сделали &lt;b&gt;кнопку&lt;/b&gt;", page)

        got = self.ask(self.TOKEN, other)
        self.assertEqual(list(got), [self.TOKEN])
        self.assertEqual(got[self.TOKEN]["s"], "fixed")
        self.assertEqual(got[self.TOKEN]["v"], "0.6.1-beta.150")
        self.assertEqual(got[self.TOKEN]["text"], "Сделали <b>кнопку</b>")
        self.assertTrue(got[self.TOKEN]["at"])
        self.assertIn("Браузер забрал ответ", self.request("GET", f"/d/{KEY}/r/{rid}")[1])
        self.assertIn("исправлено ✓", self.request("GET", f"/d/{KEY}")[1])

        # Исправленный ответ уходит снова, с новым временем; «убрать» — пропадает.
        self.assertEqual(self.answer(rid, s="reply", text="Уточните, пожалуйста")[0], 303)
        self.assertEqual(self.ask(self.TOKEN)[self.TOKEN]["s"], "reply")
        self.assertEqual(self.answer(rid, s="fixed", do="clear")[0], 303)
        self.assertEqual(self.ask(self.TOKEN), {})

    def test_bad_answers_are_refused(self):
        self.assertEqual(self.request("POST", "/r", report(kind="user", token=self.TOKEN))[0], 204)
        rid = self.last_id()
        self.assertEqual(self.request("POST", "/r", report(kind="user"))[0], 204)
        no_token = self.last_id()
        self.assertEqual(self.request("POST", "/r", report(kind="crash", token=self.TOKEN))[0], 204)
        crash = self.last_id()
        self.assertEqual(self.answer(rid, key="wrong", s="fixed")[0], 404)
        self.assertEqual(self.answer(999999, s="fixed")[0], 404)
        for fields in ({"s": "hack"}, {"s": "reply"}, {"s": "fixed", "v": "0.6"},
                       {"s": "fixed", "text": "x" * (stats.MAX_ANSWER_TEXT + 1)}):
            status, page, _ = self.answer(rid, **fields)
            self.assertEqual(status, 400, fields)
            self.assertIn('class="err"', page)
        self.assertIn("ответить нельзя", self.answer(no_token, s="fixed")[1])
        self.assertIn("ответить нельзя", self.answer(crash, s="fixed")[1])
        self.assertIn("ещё не умел", self.request("GET", f"/d/{KEY}/r/{no_token}")[1])
        self.assertEqual(self.ask(self.TOKEN), {})

    def test_bad_answer_questions_are_refused(self):
        bad = [{"t": "x"}, {"t": ["XYZ"]}, {"t": [5]}, {"t": ["a" * 32] * (stats.MAX_ASK + 1)},
               [self.TOKEN], {}]
        for body in bad:
            self.assertEqual(self.request("POST", "/a", body)[0], 400, repr(body)[:60])
        self.assertEqual(self.request("GET", "/a")[0], 404)

    # ---------- ограничение частоты ----------

    def test_reports_are_rate_limited_per_ip(self):
        rate = stats.LIMITS["/r"][0]
        for i in range(rate):
            self.assertEqual(self.request("POST", "/r", report(), ip="203.0.113.7")[0], 204, i)
        self.assertEqual(self.request("POST", "/r", report(), ip="203.0.113.7")[0], 429)
        # Другой адрес лимит не задевает.
        self.assertEqual(self.request("POST", "/r", report(), ip="203.0.113.8")[0], 204)
        self.assertEqual(len(self.rows()), rate + 1)
        # Неверные запросы тоже тратят жетоны: перебором лимит не обойти.
        for _ in range(rate):
            self.request("POST", "/r", report(kind="bad"), ip="203.0.113.11")
        self.assertEqual(self.request("POST", "/r", report(), ip="203.0.113.11")[0], 429)

    def test_pings_are_rate_limited_per_ip(self):
        rate = stats.LIMITS["/p"][0]
        ping = {"v": "0.5.4", "os": "linux", "d": False}
        for i in range(rate):
            self.assertEqual(self.request("POST", "/p", ping, ip="203.0.113.9")[0], 204, i)
        self.assertEqual(self.request("POST", "/p", ping, ip="203.0.113.9")[0], 429)
        self.assertEqual(self.request("POST", "/p", ping, ip="203.0.113.10")[0], 204)

    def test_forwarded_for_is_trusted_only_from_caddy(self):
        self.assertEqual(stats.client_ip("127.0.0.1", "203.0.113.1"), "203.0.113.1")
        # Caddy дописывает настоящий адрес последним: подставленный клиентом не в счёт.
        self.assertEqual(stats.client_ip("127.0.0.1", "1.1.1.1, 203.0.113.2"), "203.0.113.2")
        self.assertEqual(stats.client_ip("127.0.0.1", ""), "127.0.0.1")
        self.assertEqual(stats.client_ip("198.51.100.5", "203.0.113.3"), "198.51.100.5")

    def test_limiter_refills_over_time(self):
        now = [1000.0]
        lim = stats.Limiter(2, 10, clock=lambda: now[0])
        self.assertTrue(lim.allow("a"))
        self.assertTrue(lim.allow("a"))
        self.assertFalse(lim.allow("a"))
        self.assertTrue(lim.allow("b"))
        now[0] += 5  # полжетона в секунду → ровно один
        self.assertTrue(lim.allow("a"))
        self.assertFalse(lim.allow("a"))
        # IP в памяти — только хэшем.
        self.assertNotIn("a", lim.buckets)

    # ---------- хранение ----------

    def test_report_table_is_bounded(self):
        old = stats.MAX_REPORTS
        stats.MAX_REPORTS = 5
        try:
            for i in range(8):
                self.assertTrue(stats.store_report(stats.check_report(report(text=f"r{i}"))))
        finally:
            stats.MAX_REPORTS = old
        self.assertEqual([r[3] for r in self.rows()], [f"r{i}" for i in range(3, 8)])

    def test_migration_adds_reports_to_an_old_database(self):
        path = os.path.join(self.tmp.name, "old.db")
        con = sqlite3.connect(path)
        con.execute("""CREATE TABLE hits(day TEXT, v TEXT, os TEXT, pings INTEGER DEFAULT 0,
            dau INTEGER DEFAULT 0, wau INTEGER DEFAULT 0, mau INTEGER DEFAULT 0,
            installs INTEGER DEFAULT 0, PRIMARY KEY(day, v, os))""")
        con.execute("INSERT INTO hits(day, v, os, pings, dau) VALUES('2026-11-01','0.3.0','windows',3,2)")
        con.commit()
        con.close()
        saved = stats.DB
        stats.DB = path
        try:
            stats.init()
            stats.init()  # повторный запуск ничего не ломает
            con = stats.db()
            tables = {r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            vdau = con.execute("SELECT vdau FROM hits").fetchone()[0]
            con.close()
        finally:
            stats.DB = saved
        self.assertTrue({"hits", "recent", "reports"} <= tables)
        self.assertEqual(vdau, 2)


class Online(unittest.TestCase):
    """«Сейчас» и столбики по часам: сколько браузеров открыто, а не сколько отметок."""

    NOW = stats.datetime(2026, 11, 26, 18, 8, tzinfo=stats.timezone.utc)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.saved = stats.DB, stats.utcnow
        stats.DB = os.path.join(self.tmp.name, "online.db")
        stats.init()

    def tearDown(self):
        stats.DB, stats.utcnow = self.saved
        self.tmp.cleanup()

    def ping(self, hh, mm, **flags):
        at = self.NOW.replace(hour=hh, minute=mm)
        stats.utcnow = lambda: at
        self.assertTrue(stats.record({"v": "0.6.1", "os": "windows", **flags}))

    def online(self):
        con = stats.db()
        try:
            return stats.online_now(con, self.NOW)
        finally:
            con.close()

    def test_restart_within_the_hour_does_not_count(self):
        self.ping(17, 15, d=True, o=True)
        self.ping(17, 40, up=True, vd=True, o=False)  # тот же человек обновился
        self.ping(17, 50, d=True, o=True)
        self.assertEqual(self.online(), 2)

    def test_old_browsers_without_the_flag_count_every_ping(self):
        self.ping(17, 15, d=True)
        self.ping(17, 40, up=True)
        self.assertEqual(self.online(), 1, "но не больше, чем людей сегодня")
        self.ping(17, 45, d=True)
        self.assertEqual(self.online(), 2)
        self.ping(17, 55, d=True)
        self.assertEqual(self.online(), 3)

    def test_window_is_one_hour_not_seventy_minutes(self):
        # В 18:08 граница часа — 17:08: из ячейки 17:00–17:10 идут две десятых.
        for _ in range(5):
            self.ping(17, 3, d=True, o=True)
        self.ping(17, 45, d=True, o=True)
        self.ping(16, 55, d=True, o=True)  # раньше часа — мимо
        self.assertEqual(self.online(), 2)

    def test_dashboard_shows_people_not_pings(self):
        self.ping(17, 15, d=True, o=True)
        for m in (20, 25, 30):
            self.ping(17, m, o=False)  # перезапуски
        stats.utcnow = lambda: self.NOW
        page = stats.dashboard()
        self.assertIn('<div class="n">≈1</div>', page)
        self.assertIn('data-fmt="hour-tip" data-n="1"', page)

    def test_migration_copies_pings_to_online(self):
        con = stats.db()
        with con:
            con.execute("DROP TABLE recent")
            con.execute("""CREATE TABLE recent(slot TEXT, v TEXT, pings INTEGER DEFAULT 0,
                installs INTEGER DEFAULT 0, updates INTEGER DEFAULT 0, PRIMARY KEY(slot, v))""")
            con.execute("INSERT INTO recent(slot, v, pings) VALUES('2026-11-26T17:50','0.6.1',4)")
        con.close()
        stats.init()
        stats.init()  # повторный запуск не копирует второй раз
        con = stats.db()
        online = con.execute("SELECT online FROM recent").fetchone()[0]
        con.close()
        self.assertEqual(online, 4)



class Connections(unittest.TestCase):
    """Предел одновременных соединений и тихий обрыв клиентом."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        stats.DB = os.path.join(self.tmp.name, "test.db")
        stats.init()
        stats.LIMITERS = {p: stats.Limiter(*lim) for p, lim in stats.LIMITS.items()}
        self.srv = stats.Server(("127.0.0.1", 0), stats.Handler, limit=2)
        self.port = self.srv.server_address[1]
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def tearDown(self):
        self.srv.shutdown()
        self.srv.server_close()
        self.tmp.cleanup()

    def health(self):
        con = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            con.request("GET", "/health")
            return con.getresponse().status
        except (ConnectionError, http.client.HTTPException, OSError):
            return None
        finally:
            con.close()

    def slow(self):
        """Клиент прислал заголовки с Content-Length и молчит."""
        s = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        s.sendall(b"POST /p HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\n")
        return s

    def test_slow_clients_do_not_pile_up_threads(self):
        held = [self.slow(), self.slow()]
        time.sleep(0.3)
        # Оба места заняты: третье соединение закрывают, сервер не плодит потоки.
        self.assertIsNone(self.health())
        for s in held:
            s.close()
        # Места освободились — сервер отвечает как прежде.
        for _ in range(50):
            if self.health() == 200:
                break
            time.sleep(0.05)
        self.assertEqual(self.health(), 200)

    def test_client_that_hangs_up_leaves_no_traceback(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            for _ in range(5):
                # Тело не дослано, соединение оборвано (SO_LINGER с нулём шлёт RST):
                # ответ сервера упирается в закрытый сокет.
                s = self.slow()
                s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER,
                             struct.pack("HH" if os.name == "nt" else "ii", 1, 0))
                s.close()
            time.sleep(0.5)
            self.assertEqual(self.health(), 200)
        self.assertNotIn("Traceback", err.getvalue())


if __name__ == "__main__":
    unittest.main()
