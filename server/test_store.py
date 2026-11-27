#!/usr/bin/env python3
"""Проверки магазина плагинов: приём на проверку, ключ автора, каталог и выдача,
решения модератора, официальные плагины, экранирование кода на странице проверки.

Запуск из корня репозитория: python -m unittest discover -s server -v
"""

import base64
import hashlib
import http.client
import json
import os
import sys
import tempfile
import threading
import unittest
from urllib.parse import urlencode

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stats  # noqa: E402
import store  # noqa: E402

KEY = "storekey123"
AUTHOR = "a" * 32
OTHER = "b" * 32


def b64(text):
    return base64.b64encode(text.encode()).decode()


def plugin(pid="hello", version="1.0.0", name="Привет", code="console.log(1)", extra=None):
    manifest = {
        "api": 1, "id": pid, "name": name, "version": version,
        "description": "Пример", "author": "Аня", "icon": "icon.svg",
        "permissions": ["notify", "everything"],
        "content_scripts": [{"matches": ["<all_urls>"], "js": ["content.js"]}],
        "action": {"title": "Кнопка"},
    }
    files = {"plugin.json": b64(json.dumps(manifest, ensure_ascii=False)),
             "content.js": b64(code),
             "icon.svg": b64('<svg xmlns="http://www.w3.org/2000/svg"/>')}
    files.update(extra or {})
    return files


class Store(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        stats.DB = os.path.join(cls.tmp.name, "store.db")
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
        stats.STORE_LIMITERS = {p: stats.Limiter(*lim) for p, lim in stats.STORE_LIMITS.items()}
        con = stats.db()
        with con:
            con.execute("DELETE FROM store_versions")
            con.execute("DELETE FROM store_plugins")
        con.close()
        self.ip = 0

    def request(self, method, path, body=None, form=False):
        con = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        self.ip += 1
        h = {"X-Forwarded-For": f"203.0.113.{self.ip}"}
        if form:
            body = urlencode(body).encode()
            h["Content-Type"] = "application/x-www-form-urlencoded"
        elif body is not None:
            body = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
        con.request(method, path, body=body, headers=h)
        resp = con.getresponse()
        data = resp.read()
        con.close()
        return resp.status, data, resp

    def submit(self, files, key=AUTHOR, **more):
        status, data, _ = self.request("POST", "/store/submit", {"key": key, "files": files, **more})
        return status, json.loads(data)

    def index(self):
        status, data, _ = self.request("GET", "/store/index")
        self.assertEqual(status, 200)
        return {p["id"]: p for p in json.loads(data)["plugins"]}

    def pending(self):
        con = stats.db()
        rows = con.execute("SELECT n FROM store_versions WHERE status='pending' ORDER BY n").fetchall()
        con.close()
        return [r[0] for r in rows]

    def decide(self, n, do, reason=""):
        status, _, resp = self.request("POST", f"/d/{KEY}/store/{n}", {"do": do, "reason": reason},
                                       form=True)
        return status, resp

    def test_submit_review_index_download(self):
        status, reply = self.submit(plugin(), author="Аня", contact="t.me/anya", note="Первый")
        self.assertEqual((status, reply["status"], reply["id"]), (200, "pending", "hello"))
        # На проверке — в каталоге его нет, скачать нельзя.
        self.assertEqual(self.index(), {})
        self.assertEqual(self.request("GET", "/store/p/hello")[0], 404)

        [n] = self.pending()
        status, resp = self.decide(n, "approve")
        self.assertEqual(status, 303)
        self.assertEqual(resp.getheader("Location"), f"/d/{KEY}/store")

        item = self.index()["hello"]
        self.assertEqual((item["name"], item["version"], item["author"]), ("Привет", "1.0.0", "Аня"))
        # Только известные разрешения, значок — data: только для картинок.
        self.assertEqual(item["permissions"], ["notify"])
        self.assertTrue(item["icon"].startswith("data:image/svg+xml;base64,"))
        self.assertEqual((item["sites"], item["action"], item["installs"]), (["<all_urls>"], True, 0))

        status, body, _ = self.request("GET", "/store/p/hello")
        self.assertEqual(status, 200)
        self.assertEqual(hashlib.sha256(body).hexdigest(), item["sha256"])
        files = json.loads(body)["files"]
        self.assertEqual(base64.b64decode(files["content.js"]).decode(), "console.log(1)")
        self.assertEqual(self.index()["hello"]["installs"], 1)

    def test_author_key_owns_the_id(self):
        self.assertEqual(self.submit(plugin())[0], 200)
        status, reply = self.submit(plugin(version="1.0.1"), key=OTHER)
        self.assertEqual(status, 403)
        self.assertIn("занят другим автором", reply["error"])
        # Та же версия ещё раз — нельзя, новая — можно и заменяет непроверенную.
        self.assertEqual(self.submit(plugin())[0], 409)
        self.assertEqual(self.submit(plugin(version="1.0.1"))[0], 200)
        self.assertEqual(len(self.pending()), 1)
        self.assertEqual(self.submit({"plugin.json": "x"}, key="nope")[0], 400)

    def test_status_and_rejection(self):
        self.submit(plugin())
        [n] = self.pending()
        status, _ = self.decide(n, "reject")
        self.assertEqual(status, 400, "без причины отклонить нельзя")
        self.assertEqual(self.decide(n, "reject", "Ходит на чужой сервер")[0], 303)
        status, data, _ = self.request("POST", "/store/status",
                                       {"items": [{"id": "hello", "key": AUTHOR}, {"id": "hello", "key": OTHER}]})
        self.assertEqual(status, 200)
        items = json.loads(data)["items"]
        self.assertEqual(items["hello"], {"version": "1.0.0", "status": "rejected",
                                          "reason": "Ходит на чужой сервер", "published": None})
        # Чужим ключом о плагине ничего не узнать.
        status, data, _ = self.request("POST", "/store/status", {"items": [{"id": "hello", "key": OTHER}]})
        self.assertEqual(json.loads(data)["items"], {})
        self.assertEqual(self.request("POST", "/store/status", {"items": "x"})[0], 400)

    def test_hide_and_show(self):
        self.submit(plugin())
        self.decide(self.pending()[0], "approve")
        self.assertEqual(self.request("POST", f"/d/{KEY}/store/p/hello", {"do": "hide"}, form=True)[0], 303)
        self.assertNotIn("hello", self.index())
        self.assertEqual(self.request("GET", "/store/p/hello")[0], 404)
        self.request("POST", f"/d/{KEY}/store/p/hello", {"do": "show"}, form=True)
        self.assertIn("hello", self.index())

    def test_bad_bundles(self):
        cases = {
            "traversal": plugin(extra={"../x.js": b64("1")}),
            "hidden": plugin(extra={".git/config": b64("1")}),
            "exe": plugin(extra={"run.exe": b64("MZ")}),
            "backslash": plugin(extra={"a\\b.js": b64("1")}),
            "not base64": plugin(extra={"x.js": "%%%"}),
        }
        for name, files in cases.items():
            self.assertEqual(self.submit(files)[0], 400, name)
        no_manifest = plugin()
        del no_manifest["plugin.json"]
        self.assertEqual(self.submit(no_manifest)[0], 400)
        missing = plugin()
        del missing["content.js"]
        status, reply = self.submit(missing)
        self.assertEqual(status, 400)
        self.assertIn("content.js", reply["error"])
        big = plugin(extra={f"f{i}.js": b64("x" * 1_090_000) for i in range(5)})
        status, reply = self.submit(big)
        self.assertEqual(status, 400)
        self.assertIn("5 МБ", reply["error"])
        self.assertEqual(self.pending(), [])

    def test_official_import(self):
        root = os.path.join(self.tmp.name, "official")
        folder = os.path.join(root, "dark")
        os.makedirs(folder, exist_ok=True)
        manifest = {"api": 1, "id": "dark-sites", "name": "Тёмные сайты", "version": "1.0.0",
                    "content_scripts": [{"matches": ["*"], "css": ["dark.css"]}]}
        with open(os.path.join(folder, "plugin.json"), "w", encoding="utf-8") as f:
            json.dump(manifest, f)
        with open(os.path.join(folder, "dark.css"), "w", encoding="utf-8") as f:
            f.write("html{filter:invert(1)}")
        con = stats.db()
        self.assertEqual(store.import_dir(con, root, "2026-11-27T00:00:00Z"),
                         ["dark-sites 1.0.0: опубликован"])
        self.assertEqual(store.import_dir(con, root, "2026-11-27T00:00:00Z"),
                         ["dark-sites 1.0.0: без изменений"])
        with open(os.path.join(folder, "dark.css"), "w", encoding="utf-8") as f:
            f.write("html{filter:invert(.9)}")
        self.assertEqual(store.import_dir(con, root, "2026-11-27T00:00:00Z"),
                         ["dark-sites 1.0.0: обновлён"])
        con.close()
        item = self.index()["dark-sites"]
        self.assertTrue(item["official"])
        # Чужой автор не может прислать плагин под id Axiom.
        status, reply = self.submit(plugin(pid="dark-sites"))
        self.assertEqual(status, 403)
        self.assertIn("Axiom", reply["error"])

    def test_review_page_escapes_code_and_needs_the_key(self):
        evil = plugin(name="<script>alert(1)</script>", code="</pre><script>alert(2)</script>",
                      extra={"notes.md": b64("<img src=x onerror=alert(3)>")})
        self.assertEqual(self.submit(evil, author="<b>x</b>", note="<i>y</i>")[0], 200)
        status, data, resp = self.request("GET", f"/d/{KEY}/store")
        page = data.decode()
        self.assertEqual(status, 200)
        self.assertIn("На проверке: 1", page)
        self.assertNotIn("<script>alert", page)
        self.assertNotIn("<img src=x", page)
        self.assertIn("&lt;/pre&gt;&lt;script&gt;alert(2)&lt;/script&gt;", page)
        csp = resp.getheader("Content-Security-Policy")
        self.assertIn("default-src 'none'", csp)
        self.assertIn("form-action 'self'", csp)
        self.assertEqual(self.request("GET", "/d/wrong/store")[0], 404)
        self.assertEqual(self.decide(self.pending()[0], "approve")[0], 303)
        self.assertEqual(self.request("POST", "/d/wrong/store/1", {"do": "approve"}, form=True)[0], 404)
        # Главная панель ведёт на страницу магазина.
        status, data, _ = self.request("GET", f"/d/{KEY}")
        self.assertIn(f"/d/{KEY}/store", data.decode())
        self.assertIn("в магазине 1 плагин", data.decode())

    def test_submissions_are_rate_limited(self):
        stats.STORE_LIMITERS["/store/submit"] = stats.Limiter(2, 3600)
        con = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        codes = []
        for i in range(3):
            body = json.dumps({"key": AUTHOR, "files": plugin(version=f"1.0.{i}")})
            con.request("POST", "/store/submit", body=body,
                        headers={"Content-Type": "application/json", "X-Forwarded-For": "192.0.2.7"})
            r = con.getresponse()
            r.read()
            codes.append(r.status)
        con.close()
        self.assertEqual(codes, [200, 200, 429])


if __name__ == "__main__":
    unittest.main()
