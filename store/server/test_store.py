#!/usr/bin/env python3
"""Проверки магазина плагинов: заявки и ключи, вход, загрузка и проверка плагинов,
каталог и выдача для браузера, права администратора, защита от подделки запросов.

Запуск из корня репозитория: python -m unittest discover -s store/server -v
"""

import base64
import hashlib
import http.client
import io
import json
import os
import re
import sys
import tempfile
import threading
import unittest
import zipfile
from urllib.parse import urlencode

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import store  # noqa: E402

ADMIN = "admin-key-for-tests-0123456789"


def plugin(pid="hello", version="1.0.0", name="Привет", code="console.log(1)", extra=None):
    manifest = {
        "api": 1, "id": pid, "name": name, "version": version, "description": "Пример",
        "icon": "icon.svg", "permissions": ["notify", "everything"],
        "content_scripts": [{"matches": ["<all_urls>"], "js": ["content.js"]}],
        "action": {"title": "Кнопка"},
    }
    files = {"plugin.json": json.dumps(manifest, ensure_ascii=False).encode(),
             "content.js": code.encode(),
             "icon.svg": b'<svg xmlns="http://www.w3.org/2000/svg"/>'}
    files.update(extra or {})
    return files


def b64files(files):
    return {p: base64.b64encode(b).decode() for p, b in files.items()}


def zipped(files, top="my-plugin"):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for p, b in files.items():
            zf.writestr(f"{top}/{p}" if top else p, b)
    return base64.b64encode(buf.getvalue()).decode()


class Client:
    """Браузер для тестов: помнит куку сессии и адрес."""

    def __init__(self, port, ip):
        self.port, self.ip, self.cookie = port, ip, ""

    def go(self, method, path, form=None, body=None):
        con = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        h = {"X-Forwarded-For": self.ip}
        if self.cookie:
            h["Cookie"] = f"{store.COOKIE}={self.cookie}"
        if form is not None:
            body = urlencode(form).encode()
            h["Content-Type"] = "application/x-www-form-urlencoded"
        elif body is not None:
            body = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
        con.request(method, path, body=body, headers=h)
        r = con.getresponse()
        data = r.read()
        con.close()
        m = re.match(rf"{store.COOKIE}=([^;]*)", r.getheader("Set-Cookie") or "")
        if m:
            self.cookie = m[1]
        return r.status, data.decode("utf-8", "replace"), r

    def csrf(self, path):
        _, page, _ = self.go("GET", path)
        m = re.search(r'name="csrf" value="([^"]+)"', page) or re.search(r'data-csrf="([^"]+)"', page)
        return m[1] if m else ""

    def login(self, key):
        return self.go("POST", "/login", form={"key": key})

    def upload(self, files=None, zip_b64=None, note=""):
        body = {"csrf": self.csrf("/dev" if not self.is_admin() else "/admin/plugins"), "note": note}
        if zip_b64 is not None:
            body["zip"] = zip_b64
        else:
            body["files"] = b64files(files)
        status, out, _ = self.go("POST", "/upload", body=body)
        return status, json.loads(out)

    def is_admin(self):
        return self.go("GET", "/admin")[0] == 200


class Store(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        store.DB = os.path.join(cls.tmp.name, "store.db")
        store.ADMIN_KEY = ADMIN
        store.INSECURE = True
        store.init()
        cls.srv = store.Server(("127.0.0.1", 0), store.Handler)
        cls.port = cls.srv.server_address[1]
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()
        cls.tmp.cleanup()

    def setUp(self):
        store.LIMITERS = {k: store.Limiter(*v) for k, v in store.LIMITS.items()}
        con = store.db()
        with con:
            for t in ("versions", "plugins", "sessions", "developers", "applications"):
                con.execute(f"DELETE FROM {t}")
        con.close()
        self.n = 0

    def client(self):
        self.n += 1
        return Client(self.port, f"203.0.113.{self.n}")

    def admin(self):
        a = self.client()
        status, _, _ = a.login(ADMIN)
        self.assertEqual(status, 303)
        return a

    def developer(self, name="Аня"):
        """Заявка → одобрение → ключ со страницы статуса → вход."""
        guest = self.client()
        status, _, resp = guest.go("POST", "/apply", form={
            "name": name, "contact": "t.me/anya", "about": "Хочу сделать плагин для YouTube"})
        self.assertEqual(status, 303)
        status_url = resp.getheader("Location")
        self.assertRegex(status_url, r"^/status/[A-Za-z0-9_-]{20,}$")
        self.assertIn("на рассмотрении", guest.go("GET", status_url)[1])
        admin = self.admin()
        con = store.db()
        app_id = con.execute("SELECT MAX(id) FROM applications").fetchone()[0]
        con.close()
        status, _, resp = admin.go("POST", f"/admin/apps/{app_id}",
                                   form={"do": "approve", "csrf": admin.csrf("/admin/apps")})
        self.assertEqual((status, resp.getheader("Location")), (303, "/admin/devs"))
        page = guest.go("GET", status_url)[1]
        key = re.search(r"axd_[0-9a-f]{40}", page)[0]
        dev = self.client()
        self.assertEqual(dev.login(key)[0], 303)
        # После первого входа ключ больше нигде не показывается.
        self.assertNotIn(key, guest.go("GET", status_url)[1])
        self.assertNotIn(key, admin.go("GET", "/admin/devs")[1])
        return dev, key

    def catalog(self):
        status, data, _ = self.client().go("GET", "/api/index")
        self.assertEqual(status, 200)
        return {p["id"]: p for p in json.loads(data)["plugins"]}

    def pending(self):
        con = store.db()
        rows = [r[0] for r in con.execute("SELECT n FROM versions WHERE status='pending' ORDER BY n")]
        con.close()
        return rows

    def test_developer_flow(self):
        dev, _ = self.developer()
        status, out = dev.upload(plugin(), note="Первая версия")
        self.assertEqual(status, 200, out)
        self.assertIn("на проверку", out["message"])
        self.assertEqual(self.catalog(), {})
        self.assertIn("на проверке", dev.go("GET", "/dev")[1])

        admin = self.admin()
        [n] = self.pending()
        page = admin.go("GET", "/admin/review")[1]
        self.assertIn("Первая версия", page)
        self.assertIn("console.log(1)", page)
        status, _, _ = admin.go("POST", f"/admin/review/{n}",
                                form={"do": "approve", "csrf": admin.csrf("/admin/review")})
        self.assertEqual(status, 303)

        item = self.catalog()["hello"]
        self.assertEqual((item["name"], item["version"], item["author"], item["official"]),
                         ("Привет", "1.0.0", "Аня", False))
        self.assertEqual(item["permissions"], ["notify"])
        self.assertTrue(item["icon"].startswith("data:image/svg+xml;base64,"))
        status, body, _ = self.client().go("GET", "/api/p/hello")
        self.assertEqual(status, 200)
        self.assertEqual(hashlib.sha256(body.encode()).hexdigest(), item["sha256"])
        self.assertEqual(self.catalog()["hello"]["installs"], 1)
        self.assertIn("опубликован", dev.go("GET", "/dev")[1])

    def test_zip_upload_and_versions(self):
        dev, _ = self.developer()
        status, out = dev.upload(zip_b64=zipped(plugin()))
        self.assertEqual(status, 200, out)
        status, out = dev.upload(zip_b64=zipped(plugin(), top=None))
        self.assertEqual(status, 409, "та же версия второй раз")
        self.assertEqual(dev.upload(plugin(version="1.0.1"))[0], 200)
        # Непроверенная прежняя версия заменилась новой.
        self.assertEqual(len(self.pending()), 1)
        status, out = dev.upload(zip_b64=base64.b64encode(b"not a zip").decode())
        self.assertEqual((status, out["message"]), (400, "это не zip-архив"))

    def test_zip_bomb_is_refused_before_unpacking(self):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
            for p, b in plugin().items():
                zf.writestr(p, b)
            zf.writestr("huge.js", b"0" * (store.MAX_FILE + 1))
        self.assertLess(len(buf.getvalue()), 64 * 1024)
        self.assertIn("больше 2 МБ", store.from_zip(buf.getvalue()))

    def test_ids_belong_to_their_developer(self):
        anya, _ = self.developer("Аня")
        boris, _ = self.developer("Борис")
        self.assertEqual(anya.upload(plugin())[0], 200)
        status, out = boris.upload(plugin(version="2.0.0"))
        self.assertEqual(status, 403)
        self.assertIn("другим разработчиком", out["message"])
        # Администратору можно всё: его версия публикуется сразу.
        admin = self.admin()
        status, out = admin.upload(plugin(version="3.0.0"))
        self.assertEqual(status, 200, out)
        self.assertEqual(self.catalog()["hello"]["version"], "3.0.0")

    def test_admin_upload_is_official_and_immediate(self):
        admin = self.admin()
        status, out = admin.upload(plugin(pid="axiom-thing"))
        self.assertEqual(status, 200, out)
        item = self.catalog()["axiom-thing"]
        self.assertTrue(item["official"])
        dev, _ = self.developer()
        status, out = dev.upload(plugin(pid="axiom-thing", version="9.0.0"))
        self.assertEqual(status, 403)
        self.assertIn("Axiom", out["message"])

    def test_revoke_and_rekey(self):
        dev, key = self.developer()
        admin = self.admin()
        con = store.db()
        did = con.execute("SELECT MAX(id) FROM developers").fetchone()[0]
        con.close()
        csrf = admin.csrf("/admin/devs")
        self.assertEqual(admin.go("POST", f"/admin/devs/{did}", form={"do": "revoke", "csrf": csrf})[0], 303)
        # Сессия отозванного больше не работает, войти тем же ключом нельзя.
        self.assertEqual(dev.go("GET", "/dev")[2].getheader("Location"), "/login")
        self.assertEqual(self.client().login(key)[0], 403)
        admin.go("POST", f"/admin/devs/{did}", form={"do": "restore", "csrf": csrf})
        self.assertEqual(self.client().login(key)[0], 303)
        admin.go("POST", f"/admin/devs/{did}", form={"do": "rekey", "csrf": csrf})
        self.assertEqual(self.client().login(key)[0], 403, "старый ключ после нового не работает")
        new = re.search(r"axd_[0-9a-f]{40}", admin.go("GET", "/admin/devs")[1])[0]
        self.assertNotEqual(new, key)
        self.assertEqual(self.client().login(new)[0], 303)

    def test_rejections_are_explained(self):
        dev, _ = self.developer()
        dev.upload(plugin())
        admin = self.admin()
        [n] = self.pending()
        csrf = admin.csrf("/admin/review")
        self.assertEqual(admin.go("POST", f"/admin/review/{n}", form={"do": "reject", "csrf": csrf})[0], 400)
        admin.go("POST", f"/admin/review/{n}",
                 form={"do": "reject", "reason": "Отправляет адреса страниц", "csrf": csrf})
        self.assertIn("Отправляет адреса страниц", dev.go("GET", "/dev")[1])
        # Отклонённую заявку видно по ссылке на статус.
        guest = self.client()
        _, _, resp = guest.go("POST", "/apply", form={"name": "x", "contact": "y", "about": "a" * 20})
        url = resp.getheader("Location")
        con = store.db()
        app_id = con.execute("SELECT MAX(id) FROM applications").fetchone()[0]
        con.close()
        admin.go("POST", f"/admin/apps/{app_id}",
                 form={"do": "reject", "reason": "Нет примеров кода", "csrf": admin.csrf("/admin/apps")})
        self.assertIn("Нет примеров кода", guest.go("GET", url)[1])

    def test_forged_and_anonymous_requests(self):
        admin = self.admin()
        dev, _ = self.developer()
        dev.upload(plugin())
        [n] = self.pending()
        # Без csrf из страницы и без входа — ничего.
        self.assertEqual(admin.go("POST", f"/admin/review/{n}", form={"do": "approve"})[0], 403)
        self.assertEqual(dev.go("POST", f"/admin/review/{n}",
                                form={"do": "approve", "csrf": dev.csrf("/dev")})[0], 403)
        status, out, _ = self.client().go("POST", "/upload", body={"files": b64files(plugin())})
        self.assertEqual(status, 403)
        status, out, _ = dev.go("POST", "/upload", body={"csrf": "wrong", "files": b64files(plugin())})
        self.assertEqual(status, 403)
        for path in ("/admin", "/admin/review", "/dev"):
            self.assertEqual(self.client().go("GET", path)[0], 303, path)
        self.assertEqual(self.client().login("axd_" + "0" * 40)[0], 403)
        self.assertEqual(self.client().go("GET", "/status/" + "x" * 30)[0], 404)
        # Ловушка для ботов.
        status, _, _ = self.client().go("POST", "/apply", form={
            "name": "bot", "contact": "bot", "about": "a" * 30, "website": "http://spam"})
        self.assertEqual(status, 400)

    def test_review_page_escapes_everything(self):
        dev, _ = self.developer()
        evil = plugin(name="<script>alert(1)</script>", code="</pre><script>alert(2)</script>",
                      extra={"notes.md": b"<img src=x onerror=alert(3)>"})
        self.assertEqual(dev.upload(evil, note="<b>x</b>")[0], 200)
        admin = self.admin()
        status, page, resp = admin.go("GET", "/admin/review")
        self.assertEqual(status, 200)
        self.assertNotIn("<script>alert", page)
        self.assertNotIn("<img src=x", page)
        self.assertIn("&lt;/pre&gt;&lt;script&gt;alert(2)&lt;/script&gt;", page)
        csp = resp.getheader("Content-Security-Policy")
        nonce = csp.split("'nonce-")[1].split("'")[0]
        self.assertEqual(page.count("<script"), page.count(f'<script nonce="{nonce}">'))

    def test_bad_bundles(self):
        dev, _ = self.developer()
        self.assertEqual(dev.upload({"content.js": b"1"})[0], 400)
        missing = plugin()
        del missing["content.js"]
        status, out = dev.upload(missing)
        self.assertEqual(status, 400)
        self.assertIn("content.js", out["message"])
        # Чужие типы и скрытые файлы молча не попадают в пакет.
        ok = plugin(extra={"run.exe": b"MZ", ".git/config": b"x", "../x.js": b"1"})
        self.assertEqual(dev.upload(ok)[0], 200)
        admin = self.admin()
        admin.go("POST", f"/admin/review/{self.pending()[0]}",
                 form={"do": "approve", "csrf": admin.csrf("/admin/review")})
        body = self.client().go("GET", "/api/p/hello")[1]
        self.assertEqual(sorted(json.loads(body)["files"]), ["content.js", "icon.svg", "plugin.json"])

    def test_official_import(self):
        root = os.path.join(self.tmp.name, "official")
        folder = os.path.join(root, "dark")
        os.makedirs(folder, exist_ok=True)
        with open(os.path.join(folder, "plugin.json"), "w", encoding="utf-8") as f:
            json.dump({"api": 1, "id": "dark-sites", "name": "Тёмные сайты", "version": "1.0.0",
                       "content_scripts": [{"matches": ["*"], "css": ["dark.css"]}]}, f)
        with open(os.path.join(folder, "dark.css"), "w", encoding="utf-8") as f:
            f.write("html{filter:invert(1)}")
        con = store.db()
        self.assertEqual(store.import_dir(con, root), ["dark-sites 1.0.0: опубликован"])
        self.assertEqual(store.import_dir(con, root), ["dark-sites 1.0.0: без изменений"])
        con.close()
        self.assertTrue(self.catalog()["dark-sites"]["official"])
        self.assertEqual(self.catalog()["dark-sites"]["author"], "Axiom")

    def test_hide_and_show(self):
        admin = self.admin()
        admin.upload(plugin())
        csrf = admin.csrf("/admin/plugins")
        admin.go("POST", "/admin/plugins/hello", form={"do": "hide", "csrf": csrf})
        self.assertNotIn("hello", self.catalog())
        self.assertEqual(self.client().go("GET", "/api/p/hello")[0], 404)
        admin.go("POST", "/admin/plugins/hello", form={"do": "show", "csrf": csrf})
        self.assertIn("hello", self.catalog())

    def test_logins_are_rate_limited(self):
        store.LIMITERS["login"] = store.Limiter(3, 3600)
        c = self.client()
        codes = [c.login("wrong")[0] for _ in range(4)]
        self.assertEqual(codes, [403, 403, 403, 429])


if __name__ == "__main__":
    unittest.main()
