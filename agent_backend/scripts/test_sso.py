"""SSO integration tests using a local fake identity provider and an isolated browser.

Run: python3 -m unittest discover -s agent_backend/scripts -p test_sso.py -v
No real account, password, or jAccount request is used.
"""

import asyncio
import base64
import http.cookies
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from PIL import Image, ImageDraw, ImageFont
from websockets.exceptions import ConnectionClosed


spec = importlib.util.spec_from_file_location("sso_browser", Path(__file__).resolve().parents[1] / "src/sso/browser.py")
sso = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sso)


FORM = b"""<!doctype html><html><body>
<form id="passwordForm" onsubmit="event.preventDefault()">
<input id="input-login-user" name="user">
<input id="input-login-pass" name="pass" type="password">
<input id="input-login-captcha" name="captcha">
<img id="captcha-img" onclick="refreshCaptcha()">
<button id="submit-password-button" type="button">Login</button>
<span id="span_warn"></span></form>
<script>
let captchaCheckStatus = 'failed', captchaObj = null;
const challenge = new URL(location.href).searchParams.get('challenge');
function switchLoginType(type) {}
function refreshCaptcha() {
  document.querySelector('#captcha-img').src = '/jaccount/captcha?uuid=' + challenge + '&t=' + Math.random();
}
refreshCaptcha();
document.querySelector('#submit-password-button').onclick = async () => {
  document.querySelector('#span_warn').textContent = '';
  const response = await fetch('/jaccount/ulogin', {method: 'POST', headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({user: document.querySelector('#input-login-user').value,
      pass: document.querySelector('#input-login-pass').value, captcha: document.querySelector('#input-login-captcha').value,
      uuid: challenge})});
  const result = await response.json();
  if (result.errno === 0) location.href = result.url;
  else { document.querySelector('#span_warn').textContent = result.error; refreshCaptcha(); }
};
</script></body></html>"""


def captcha_png():
    image = Image.new("RGB", (140, 48), "white")
    fonts = ["/System/Library/Fonts/Supplemental/Arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]
    font = next((ImageFont.truetype(path, 36) for path in fonts if Path(path).is_file()), ImageFont.load_default())
    ImageDraw.Draw(image).text((10, 3), "1234", fill="black", font=font)
    output = io.BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


class IdentityProvider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def cookie(self, name):
        values = http.cookies.SimpleCookie(self.headers.get("Cookie", ""))
        return values[name].value if name in values else None

    def respond(self, status, body=b"", content_type="text/html", headers=None):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/profile/jaccount":
            if self.cookie("SSO") == "active":
                self.respond(302, headers={"Location": "/profile/"})
            else:
                self.server.challenge += 1
                challenge = str(self.server.challenge)
                self.respond(302, headers={"Location": "/jaccount/jalogin?challenge=" + challenge,
                                          "Set-Cookie": "challenge=" + challenge + "; Path=/; HttpOnly; SameSite=Lax"})
        elif path == "/jaccount/jalogin":
            self.respond(200, FORM)
        elif path == "/jaccount/captcha":
            challenge = parse_qs(urlsplit(self.path).query)["uuid"][0]
            self.server.images.append((challenge, self.cookie("challenge"), self.path))
            self.respond(200, self.server.image, "image/png")
        elif path == "/profile/current":
            active = self.cookie("SSO") == "active"
            self.respond(200, json.dumps({"success": active, "data": {"accountNo": "student"} if active else None}).encode(), "application/json")
        elif path == "/profile/":
            self.respond(200, b"<!doctype html><html><body>Profile page</body></html>")
        elif path == "/portal":
            if self.cookie("SSO") == "active":
                self.respond(200, b'<!doctype html><html><body>Campus page<a href="/courses">Courses</a></body></html>')
            else:
                self.respond(302, headers={"Location": "/profile/jaccount"})
        elif path == "/courses":
            self.respond(200, b"<!doctype html><html><body><h1>Courses</h1><table><tr><td>Linear Algebra</td><td>Monday 09:00</td></tr></table></body></html>")
        else:
            self.respond(404)

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.posts.append(request)
        headers = {}
        if request["user"] != "student" or request["pass"] != "test-password":
            result = {"errno": 1, "error": "Wrong account or password"}
        elif request["uuid"] != self.cookie("challenge") or request["captcha"] != "1234":
            result = {"errno": 2, "error": "Wrong captcha"}
        else:
            result = {"errno": 0, "url": "/profile/"}
            headers["Set-Cookie"] = "SSO=active; Path=/; HttpOnly; SameSite=Lax"
        self.respond(200, json.dumps(result).encode(), "application/json", headers)


class SsoLogicTests(unittest.TestCase):
    def test_account_normalization_and_cookie_scope(self):
        self.assertEqual(sso.normalize_account(" student@SJTU.edu.cn "), "student")
        self.assertEqual(sso.normalize_account("student"), "student")
        for account in ("", "bad account", "bad\naccount"):
            with self.assertRaises(sso.SsoError):
                sso.normalize_account(account)
        self.assertTrue(sso.campus_url("https://i.sjtu.edu.cn/"))
        self.assertFalse(sso.campus_url("https://sjtu.edu.cn.attacker.test/"))
        self.assertFalse(sso.campus_url("https://name:password@i.sjtu.edu.cn/"))

    def test_cookie_restore_preserves_security_and_expiry(self):
        cookie = {"name": "SSO", "value": "test-cookie", "domain": "jaccount.sjtu.edu.cn", "path": "/",
                  "secure": True, "httpOnly": True, "sameSite": "Lax", "expires": -1}
        restored = sso.cookie_parameter(cookie)
        self.assertNotIn("domain", restored, "host-only cookies must remain host-only")
        self.assertNotIn("expires", restored, "session cookies must remain session cookies")
        self.assertEqual(restored["url"], "https://jaccount.sjtu.edu.cn/")
        self.assertTrue(restored["secure"] and restored["httpOnly"])
        self.assertIsNone(sso.cookie_parameter(dict(cookie, domain="example.com")))
        self.assertIsNone(sso.cookie_parameter(dict(cookie, expires=time.time() - 1)))

    def test_only_captcha_errors_are_retryable(self):
        self.assertTrue(sso.captcha_error("Wrong captcha"))
        self.assertTrue(sso.captcha_error("验证码错误"))
        self.assertFalse(sso.captcha_error("Wrong account or password"))
        self.assertFalse(sso.captcha_error("账号、密码或验证码有误"))


class SsoProcessGuardTests(unittest.IsolatedAsyncioTestCase):
    async def start_child(self, code):
        temporary = tempfile.TemporaryDirectory(prefix="jisjtu-guard-test-")
        self.addCleanup(temporary.cleanup)
        profile = Path(temporary.name) / "profile"
        profile.mkdir()
        process = sso.BackgroundProcess([sys.executable, "-c", code, str(profile)], dict(os.environ))
        self.addAsyncCleanup(process.close)
        await process.start(profile)
        return process, profile

    async def test_guard_exits_when_child_exits_with_controller_pipe_still_open(self):
        process, profile = await self.start_child("pass")
        self.assertEqual(await asyncio.to_thread(process.wait, timeout=5), 0)
        self.assertFalse(profile.exists())

    @unittest.skipIf(os.name == "nt", "Windows terminate is already a forced exit")
    async def test_guard_kills_its_unresponsive_child_and_removes_profile(self):
        process, profile = await self.start_child(
            "import signal, sys, time; from pathlib import Path; "
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            "Path(sys.argv[1], 'ready').touch(); time.sleep(60)"
        )
        deadline = time.monotonic() + 5
        while not (profile / "ready").exists():
            self.assertLess(time.monotonic(), deadline)
            await asyncio.sleep(0.05)
        await process.close()
        self.assertEqual(process.poll(), 0)
        with self.assertRaises(ProcessLookupError):
            os.kill(process.pid, 0)
        self.assertFalse(profile.exists())


class SsoBrowserTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jisjtu-sso-test-")
        self.addCleanup(self.temporary.cleanup)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), IdentityProvider)
        self.addCleanup(self.server.server_close)
        self.server.challenge = 0
        self.server.posts, self.server.images = [], []
        self.server.image = captcha_png()
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addAsyncCleanup(asyncio.to_thread, self.server.shutdown)
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        # Only the fixture's cookies are accepted in these isolated browser tests.
        self.scope = patch.object(sso, "campus_host", lambda host: host == "127.0.0.1")
        self.scope.start()
        self.addCleanup(self.scope.stop)
        self.config = {"action": "login", "directory": self.temporary.name, "browser": os.environ.get("AGENT_SSO_BROWSER")}
        self.browser = await sso.Browser.connect(self.config, "student")
        self.addAsyncCleanup(self.close_browser)
        self.configure_browser()

    def configure_browser(self):
        self.browser.auth_origin = self.origin
        self.browser.entry_url = self.origin + "/profile/jaccount"

    async def close_browser(self, browser=None):
        browser = browser or self.browser
        if browser.headless:
            await browser.close()
            return
        # Visible browsers intentionally outlive helper disconnection. Only this
        # fixture's own visible process is stopped during test teardown.
        try:
            await browser.cdp.call("Browser.close")
        except Exception:
            pass
        await browser.cdp.close()
        if browser.process:
            try:
                await asyncio.to_thread(browser.process.wait, timeout=5)
            except subprocess.TimeoutExpired:
                # This Popen handle belongs only to this test's temporary profile.
                browser.process.terminate()
                try:
                    await asyncio.to_thread(browser.process.wait, timeout=5)
                except subprocess.TimeoutExpired:
                    browser.process.kill()
                    await asyncio.to_thread(browser.process.wait, timeout=5)

    async def test_real_ocr_login_closes_only_its_tab_and_restores_session(self):
        self.assertTrue(self.browser.headless)
        self.assertIn("--headless=new", self.browser.process.args)
        other = await self.browser.cdp.call("Target.createTarget", {"url": "about:blank"})
        result = await self.browser.login("student", "test-password")
        self.assertEqual(result, {"ok": True, "reused": False})
        self.assertEqual(len(self.server.posts), 1)
        self.assertTrue(all(challenge == cookie for challenge, cookie, _ in self.server.images))
        targets = (await self.browser.cdp.call("Target.getTargets"))["targetInfos"]
        self.assertIn(other["targetId"], [target["targetId"] for target in targets])
        self.assertFalse(any("/profile/" in target["url"] or "/jaccount/" in target["url"] for target in targets))
        cookie_file = self.browser.directory / "cookies.json"
        self.assertTrue(cookie_file.exists())
        if os.name != "nt":
            self.assertEqual(cookie_file.stat().st_mode & 0o777, 0o600)
        await self.browser.cdp.call("Storage.clearCookies")
        await self.close_browser()
        self.browser = await sso.Browser.connect(self.config, "student")
        self.configure_browser()
        self.assertEqual(await self.browser.login("student", "test-password"), {"ok": True, "reused": True})
        self.assertEqual(len(self.server.posts), 1, "restored sessions must not resubmit the password")
        kept = await self.browser.cdp.call("Target.createTarget", {"url": "about:blank"})
        opened = await sso.Browser.connect(dict(self.config, action="read"), "student")
        try:
            self.assertTrue(opened.headless, "campus tools must also run without a visible window")
            self.assertIsNotNone(opened.process)
            self.assertNotEqual(opened.process.pid, self.browser.process.pid)
            result = await opened.read_site(self.origin + "/portal")
            self.assertTrue(result["headless"] and result["page_closed"])
            self.assertIn("Campus page", result["page"]["text"])
            self.assertEqual(result["page"]["links"], [{"text": "Courses", "url": self.origin + "/courses"}])
            courses = await opened.read_site(result["page"]["links"][0]["url"])
            self.assertIn("Linear Algebra", courses["page"]["text"])
            self.assertIsNone(opened.target)
            self.assertEqual((await opened.cdp.call("Target.getBrowserContexts"))["browserContextIds"], [])
            targets = (await opened.cdp.call("Target.getTargets"))["targetInfos"]
            self.assertNotIn(kept["targetId"], [target["targetId"] for target in targets])
        finally:
            await opened.close()
        targets = (await self.browser.cdp.call("Target.getTargets"))["targetInfos"]
        self.assertIn(kept["targetId"], [target["targetId"] for target in targets])

    async def test_read_failure_and_cancellation_close_only_temporary_pages(self):
        kept = await self.browser.cdp.call("Target.createTarget", {"url": "about:blank"})
        with patch.object(self.browser, "page_content", side_effect=sso.SsoError("simulated read failure")):
            with self.assertRaisesRegex(sso.SsoError, "simulated read failure"):
                await self.browser.read_site(self.origin + "/courses")
        self.assertEqual((await self.browser.cdp.call("Target.getBrowserContexts"))["browserContextIds"], [])

        ready = asyncio.Event()
        async def waiting_content():
            ready.set()
            await asyncio.Future()

        with patch.object(self.browser, "page_content", side_effect=waiting_content):
            task = asyncio.create_task(self.browser.read_site(self.origin + "/courses"))
            try:
                await asyncio.wait_for(ready.wait(), timeout=5)
            finally:
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
        self.assertIsNone(self.browser.target)
        self.assertEqual((await self.browser.cdp.call("Target.getBrowserContexts"))["browserContextIds"], [])
        targets = (await self.browser.cdp.call("Target.getTargets"))["targetInfos"]
        self.assertIn(kept["targetId"], [target["targetId"] for target in targets])

    async def test_disconnected_helper_disposes_its_pages_and_popups(self):
        kept = await self.browser.cdp.call("Target.createTarget", {"url": "about:blank"})
        connection = await sso.Cdp.connect(self.browser.profile)
        reader = sso.Browser(connection, self.browser.directory, self.browser.profile)
        try:
            with self.assertRaises(ConnectionClosed):
                async with reader.temporary_context():
                    await reader.new_page()
                    context_id = reader.context
                    await reader.cdp.call("Target.createTarget", {"url": "about:blank", "browserContextId": context_id})
                    # Mimic a helper killed by cancellation: its finally block can
                    # no longer talk to CDP; the browser must do the cleanup.
                    await reader.cdp.close()
            deadline = time.monotonic() + 5
            while context_id in (await self.browser.cdp.call("Target.getBrowserContexts"))["browserContextIds"]:
                self.assertLess(time.monotonic(), deadline, "disconnected helper left its pages behind")
                await asyncio.sleep(0.05)
            targets = (await self.browser.cdp.call("Target.getTargets"))["targetInfos"]
            self.assertIn(kept["targetId"], [target["targetId"] for target in targets])
            self.assertFalse(any(target.get("browserContextId") == context_id for target in targets))
        finally:
            await reader.cdp.close()

    async def test_captcha_error_retries_with_a_fresh_image(self):
        images = []
        async def recognize(image):
            images.append(image)
            return "wrong" if len(images) == 1 else "1234"
        self.browser.recognize = recognize
        result = await self.browser.login("student", "test-password")
        self.assertTrue(result["ok"])
        self.assertEqual([post["captcha"] for post in self.server.posts], ["wrong", "1234"])
        self.assertGreaterEqual(len({path for _, _, path in self.server.images}), 2)

    async def test_refresh_during_ocr_does_not_submit_the_old_challenge(self):
        count = 0
        async def recognize(image):
            nonlocal count
            count += 1
            if count == 1:
                await self.browser.evaluate("document.querySelector('#captcha-img').click()")
            return "1234"
        self.browser.recognize = recognize
        self.assertTrue((await self.browser.login("student", "test-password"))["ok"])
        self.assertEqual(count, 2)
        self.assertEqual(len(self.server.posts), 1)

    async def test_bad_password_stops_and_closes_the_hidden_login_page(self):
        async def recognize(image):
            return "1234"
        self.browser.recognize = recognize
        with self.assertRaises(sso.SsoError) as error:
            await self.browser.login("student", "invalid-secret-password")
        self.assertNotIn("invalid-secret-password", str(error.exception))
        self.assertIn("AGENT_SSO_HEADLESS=0", str(error.exception))
        self.assertNotIn("登录页已保留", str(error.exception))
        self.assertEqual(len(self.server.posts), 1)
        self.assertIsNone(self.browser.target)
        targets = (await self.browser.cdp.call("Target.getTargets"))["targetInfos"]
        self.assertFalse(any("/jaccount/" in target["url"] for target in targets))

    async def test_background_login_and_visible_browser_share_updated_sessions(self):
        # Launch the visible process without a page, so this test opens no window.
        visible_config = dict(self.config, action="open", headless=False)
        visible = await sso.Browser.connect(visible_config, "student")
        try:
            self.assertFalse(visible.headless)
            self.assertIsNotNone(visible.process, "visible pages must not reuse the headless process")
            self.assertNotIn("--headless=new", visible.process.args)

            async def session_after_reconnect(config):
                connected = await sso.Browser.connect(config, "student")
                try:
                    if config.get("headless", True):
                        self.assertIsNotNone(connected.process)
                    else:
                        self.assertIsNone(connected.process, "visible pages should reuse their browser")
                    cookies = (await connected.cdp.call("Storage.getCookies"))["cookies"]
                    return next(cookie["value"] for cookie in cookies if cookie["name"] == "SSO")
                finally:
                    await connected.close()

            async def recognize(image):
                return "1234"
            self.browser.recognize = recognize
            self.assertTrue((await self.browser.login("student", "test-password"))["ok"])
            self.assertEqual(await session_after_reconnect(visible_config), "active")

            cookie = {"name": "SSO", "value": "renewed-session", "url": self.origin + "/", "httpOnly": True}
            await self.browser.cdp.call("Storage.setCookies", {"cookies": [cookie]})
            await self.browser.save_cookies()
            self.assertEqual(await session_after_reconnect(visible_config), "renewed-session")

            # The server may rotate a cookie before the next explicit save.
            cookie["value"] = "server-rotated-session"
            await visible.cdp.call("Storage.setCookies", {"cookies": [cookie]})
            self.assertEqual(await session_after_reconnect(visible_config), "server-rotated-session")
            await visible.save_cookies()
            self.assertEqual(await session_after_reconnect(self.config), "server-rotated-session")
            self.assertEqual(len(self.server.posts), 1)
        finally:
            await self.close_browser(visible)

    def assert_background_stopped(self, browser):
        self.assertEqual(browser.process.poll(), 0, "guardian must exit and reap Edge before returning")
        self.assertFalse(browser.profile.exists(), "temporary profile must be removed after Edge exits")
        if os.name != "nt":
            with self.assertRaises(ProcessLookupError):
                os.kill(browser.process.pid, 0)

    async def test_helper_stops_its_background_process_after_success_or_failure(self):
        connect = sso.Browser.connect
        created = []

        async def track_browser(*args, **kwargs):
            browser = await connect(*args, **kwargs)
            created.append(browser)
            return browser

        request = dict(self.config, action="read", account="student", url=self.origin + "/courses")
        with patch.object(sso.Browser, "connect", side_effect=track_browser):
            result = await sso.handle_request(request)
            self.assertIn("Linear Algebra", result["page"]["text"])
            self.assert_background_stopped(created[-1])
            with patch.object(sso.Browser, "page_content", side_effect=sso.SsoError("simulated failure")):
                with self.assertRaisesRegex(sso.SsoError, "simulated failure"):
                    await sso.handle_request(request)
            self.assert_background_stopped(created[-1])
        self.assertIsNone(self.browser.process.poll(), "another worker must remain running")

    async def test_cancelled_helper_stops_its_background_process(self):
        ready = asyncio.Event()

        async def waiting_content():
            ready.set()
            await asyncio.Future()

        request = dict(self.config, action="read", account="student", url=self.origin + "/courses")
        with patch.object(sso.Browser, "connect", return_value=self.browser), \
                patch.object(self.browser, "page_content", side_effect=waiting_content):
            task = asyncio.create_task(sso.handle_request(request))
            try:
                await asyncio.wait_for(ready.wait(), timeout=5)
            finally:
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
        self.assert_background_stopped(self.browser)

    async def test_lost_controller_pipe_stops_the_background_browser(self):
        # A killed helper cannot execute finally. The OS closes its pipe; the
        # independent guardian must still shut down/reap its own browser.
        self.browser.process.guard.stdin.close()
        await asyncio.to_thread(self.browser.process.wait, timeout=15)
        self.assert_background_stopped(self.browser)

    async def test_visible_browser_survives_helper_close(self):
        # Launch without a page so this test does not open a desktop window.
        visible = await sso.Browser.connect(dict(self.config, headless=False), "student")
        try:
            await visible.close()
            self.assertIsNone(visible.process.poll())
            visible.cdp = await sso.Cdp.connect(visible.profile)
            self.assertIsNotNone(visible.cdp)
            await visible.cdp.call("Browser.getVersion")
        finally:
            await self.close_browser(visible)

    async def test_embedded_helper_runs_without_source_tree(self):
        source = Path(sso.__file__)
        request = dict(self.config, action="read", url=self.origin + "/courses",
                       process_guard=source.with_name("process_guard.py").read_text())
        result = await asyncio.to_thread(
            subprocess.run, [sys.executable, "-c", source.read_text()],
            input=json.dumps(request), text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            cwd=self.temporary.name, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("Linear Algebra", json.loads(result.stdout)["page"]["text"])
        self.assertEqual(list(Path(self.temporary.name).glob("*/browser-*-headless-*")), [self.browser.profile])

    async def test_http_200_profile_is_not_proof_of_login(self):
        await self.browser.new_page()
        await self.browser.cdp.call("Page.navigate", {"url": self.origin + "/profile/"}, self.browser.session)
        self.assertFalse(await self.browser.authenticated(await self.browser.snapshot()))

    async def test_foreign_origin_cannot_receive_credentials(self):
        await self.browser.new_page()
        await self.browser.cdp.call("Page.navigate", {"url": self.origin + "/jaccount/jalogin?challenge=1"}, self.browser.session)
        self.browser.auth_origin = sso.AUTH_ORIGIN
        self.assertFalse(await self.browser.submit("student", "test-password", "1234", ""))
        self.assertEqual(self.server.posts, [])


if __name__ == "__main__":
    unittest.main()
