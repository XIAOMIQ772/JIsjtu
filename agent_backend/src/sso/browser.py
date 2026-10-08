"""Local browser/SSO helper. Input and output are JSON; credentials never enter argv.

The jAccount selectors and /profile/current check follow the official login page.
Only this helper's tab and account-specific browser profile are controlled.
"""

import asyncio
import base64
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit


AUTH_ORIGIN = "https://jaccount.sjtu.edu.cn"
ENTRY_URL = AUTH_ORIGIN + "/profile/jaccount?returnUrl=%2F%23%2F"
MAX_CAPTCHA_ATTEMPTS = 3
_ocr = None


class SsoError(Exception):
    """Only explicitly safe messages may be returned to the model or startup log."""


def normalize_account(account):
    account = account.strip()
    if "@" in account:
        user, domain = account.rsplit("@", 1)
        if domain.lower() in {"sjtu.edu.cn", "mail.sjtu.edu.cn", "alumni.sjtu.edu.cn"}:
            account = user
    if not account or len(account) > 128 or any(char.isspace() or ord(char) < 32 for char in account):
        raise SsoError("EMAIL_USER_ACCOUNT 不是有效的 jAccount 账号")
    return account


def campus_host(host):
    return bool(host) and (host == "sjtu.edu.cn" or host.endswith(".sjtu.edu.cn"))


def campus_url(url):
    parts = urlsplit(url)
    return parts.scheme in {"https", "http"} and campus_host(parts.hostname) and not parts.username and not parts.password


def web_url(url):
    parts = urlsplit(url)
    return parts.scheme in {"https", "http"} and bool(parts.hostname) and not parts.username and not parts.password


def cookie_parameter(cookie):
    """Preserve host-only, Secure, HttpOnly, SameSite and server expiry semantics."""
    domain = cookie.get("domain", "")
    if not campus_host(domain.lstrip(".")):
        return None
    expires = cookie.get("expires", -1)
    if expires > 0 and expires <= time.time():
        return None
    result = {key: cookie[key] for key in ("name", "value", "path", "secure", "httpOnly", "sameSite", "priority") if key in cookie}
    scheme = "https" if cookie.get("secure") else "http"
    result["url"] = scheme + "://" + domain.lstrip(".") + cookie.get("path", "/")
    if domain.startswith("."):
        result["domain"] = domain
    if expires > 0:
        result["expires"] = expires
    return result


def browser_binary(configured):
    if configured:
        candidate = shutil.which(configured) or configured
        if Path(candidate).is_file():
            return candidate
        raise SsoError("AGENT_SSO_BROWSER 指定的浏览器不存在")
    windows_roots = []
    if sys.platform == "win32":
        windows_roots = [root for key in ("PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA")
                         if (root := os.environ.get(key))]
    candidates = [
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        *(str(Path(root) / "Microsoft/Edge/Application/msedge.exe") for root in windows_roots),
        *(shutil.which(name) for name in ("microsoft-edge", "microsoft-edge-stable", "msedge")),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        *(str(Path(root) / "Google/Chrome/Application/chrome.exe") for root in windows_roots),
        *(shutil.which(name) for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser")),
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return candidate
    raise SsoError("请安装 Edge、Chrome 或 Chromium，或用 AGENT_SSO_BROWSER 指定可执行文件")


class Cdp:
    def __init__(self, socket):
        self.socket = socket
        self.sequence = 0

    @classmethod
    async def connect(cls, profile):
        try:
            import websockets
        except ImportError:
            raise SsoError('缺少浏览器依赖，请执行 python3 -m pip install "websockets>=15,<18"') from None
        try:
            lines = (profile / "DevToolsActivePort").read_text().splitlines()
            port = int(lines[0])
            path = lines[1]
            if not 0 < port < 65536 or not re.fullmatch(r"/devtools/browser/[\w-]+", path):
                return None
            socket = await websockets.connect(
                f"ws://127.0.0.1:{port}{path}", proxy=None,
                open_timeout=1, close_timeout=2, max_size=16 * 1024 * 1024,
            )
            cdp = cls(socket)
            await cdp.call("Browser.getVersion")
            return cdp
        except (OSError, ValueError, IndexError, TimeoutError):
            return None
        except SsoError:
            raise
        except Exception:
            return None

    async def call(self, method, params=None, session=None):
        self.sequence += 1
        request_id = self.sequence
        request = {"id": request_id, "method": method, "params": params or {}}
        if session:
            request["sessionId"] = session
        await self.socket.send(json.dumps(request))

        async def receive():
            while True:
                response = json.loads(await self.socket.recv())
                if response.get("id") != request_id:
                    continue
                if "error" in response:
                    raise SsoError("浏览器操作失败（" + method + "）")
                return response.get("result", {})

        return await asyncio.wait_for(receive(), timeout=12)

    async def close(self):
        await self.socket.close()


class BackgroundProcess:
    """The guardian owns/reaps Edge; losing this helper's pipe also stops it."""

    def __init__(self, arguments, environment, source=None):
        self.args = arguments
        self.pid = None
        source = source or Path(__file__).with_name("process_guard.py").read_text(encoding="utf-8")
        self.guard = subprocess.Popen(
            [sys.executable, "-u", "-c", source], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0,
            env=environment, start_new_session=True,
        )

    async def start(self, profile):
        request = json.dumps({"arguments": self.args, "profile": str(profile)}) + "\n"
        self.guard.stdin.write(request.encode())
        response = await asyncio.wait_for(
            asyncio.to_thread(self.guard.stdout.readline, 4096), timeout=5,
        )
        try:
            self.pid = json.loads(response)["pid"]
            if not isinstance(self.pid, int) or self.pid <= 0:
                raise ValueError
        except (ValueError, KeyError, TypeError):
            raise SsoError("后台浏览器管理进程启动失败") from None

    def poll(self):
        return self.guard.poll()

    def wait(self, timeout=None):
        return self.guard.wait(timeout=timeout)

    def _command(self, command):
        if not self.guard.stdin.closed:
            with contextlib.suppress(BrokenPipeError, OSError):
                self.guard.stdin.write(command)

    def terminate(self):
        self._command(b"stop\n")

    def kill(self):
        # Ask the guardian to kill its own child, never a detached/reused PID.
        self._command(b"kill\n")

    async def close(self):
        self.terminate()
        try:
            try:
                status = await asyncio.to_thread(self.wait, timeout=12)
            except subprocess.TimeoutExpired:
                self.kill()
                try:
                    status = await asyncio.to_thread(self.wait, timeout=3)
                except subprocess.TimeoutExpired:
                    raise SsoError("后台浏览器未能按时退出，请检查浏览器进程") from None
            if status != 0:
                raise SsoError("后台浏览器管理进程异常退出")
        finally:
            # EOF is also a shutdown request, including on helper cancellation.
            self.guard.stdin.close()
            self.guard.stdout.close()


class Browser:
    def __init__(self, cdp, directory, profile, headless=True):
        self.cdp = cdp
        self.directory = directory
        self.profile = profile
        self.cookie_stamp = profile / "sso-cookies.sha256"
        self.headless = headless
        self.entry_url = ENTRY_URL
        self.auth_origin = AUTH_ORIGIN
        self.target = None
        self.session = None
        self.context = None
        self.process = None

    @classmethod
    async def connect(cls, config, account):
        binary = browser_binary(config.get("browser"))
        headless = bool(config.get("headless", True))
        directory = Path(config["directory"]) / hashlib.sha256(account.encode()).hexdigest()
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        if directory.is_symlink():
            raise SsoError("SSO 会话目录不能是符号链接")
        if os.name != "nt":
            directory.chmod(0o700)
        # Visible pages share a stable profile. Each background worker owns an
        # isolated temporary profile and exits after its request; account cookies
        # remain on disk so subsequent workers can restore the SSO session.
        browser_id = hashlib.sha256(str(Path(binary).resolve()).encode()).hexdigest()[:16]
        if headless:
            profile = Path(tempfile.mkdtemp(prefix="browser-" + browser_id + "-headless-", dir=directory))
        else:
            profile = directory / ("browser-" + browser_id)
            profile.mkdir(mode=0o700, exist_ok=True)
        cdp = None
        child = None
        try:
            if not headless:
                cdp = await Cdp.connect(profile)
            if cdp is None:
                arguments = [binary, "--user-data-dir=" + str(profile), "--remote-debugging-port=0",
                             "--remote-debugging-address=127.0.0.1", "--no-first-run",
                             "--no-default-browser-check", "--no-startup-window"]
                environment = {key: value for key, value in os.environ.items()
                               if not key.endswith(("_PASSWORD", "_TOKEN", "_API_KEY"))
                               and key not in {"EMAIL_USER_ACCOUNT"}}
                if headless:
                    arguments.append("--headless=new")
                    child = BackgroundProcess(arguments, environment, config.get("process_guard"))
                    await child.start(profile)
                else:
                    child = subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                             stderr=subprocess.DEVNULL, env=environment, start_new_session=True)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    cdp = await Cdp.connect(profile)
                    if cdp:
                        break
                    if child.poll() is not None:
                        raise SsoError("专用浏览器启动失败；请检查浏览器安装及 SSO 配置目录是否被占用")
                    await asyncio.sleep(0.15)
                if cdp is None:
                    raise SsoError("无法连接专用浏览器；请关闭使用该 SSO 配置目录的浏览器后重试")
            browser = cls(cdp, directory, profile, headless)
            browser.process = child
            await browser.restore_cookies()
            return browser
        except BaseException:
            try:
                if cdp:
                    await cdp.close()
            finally:
                if headless:
                    if child:
                        await child.close()
                    else:
                        shutil.rmtree(profile, ignore_errors=True)
            raise

    async def close(self):
        try:
            await self.cdp.close()
        finally:
            # An explicitly displayed browser stays open for the user. A borrowed
            # CDP connection also has no authority to stop somebody else's worker.
            if self.headless and self.process:
                await self.process.close()

    async def restore_cookies(self):
        path = self.directory / "cookies.json"
        if not path.exists():
            return
        try:
            snapshot = path.read_text(encoding="utf-8")
            saved = json.loads(snapshot)
            fingerprint = hashlib.sha256(snapshot.encode()).hexdigest()
            # A new snapshot refreshes an already-running browser too. An
            # unchanged snapshot must not overwrite newer in-browser cookies.
            changed = not self.cookie_stamp.exists() or self.cookie_stamp.read_text() != fingerprint
            current = (await self.cdp.call("Storage.getCookies"))["cookies"]
            existing = {(cookie["name"], cookie["domain"], cookie["path"]) for cookie in current}
            cookies = []
            for cookie in saved:
                if changed or (cookie["name"], cookie["domain"], cookie["path"]) not in existing:
                    parameter = cookie_parameter(cookie)
                    if parameter:
                        cookies.append(parameter)
            if cookies:
                await self.cdp.call("Storage.setCookies", {"cookies": cookies})
            self.cookie_stamp.write_text(fingerprint, encoding="utf-8")
        except (ValueError, KeyError, TypeError, OSError):
            raise SsoError("无法读取已保存的 SSO 状态；请检查 AGENT_SSO_DIR") from None

    async def save_cookies(self):
        options = {"browserContextId": self.context} if self.context else None
        cookies = (await self.cdp.call("Storage.getCookies", options))["cookies"]
        cookies = [cookie for cookie in cookies if campus_host(cookie.get("domain", "").lstrip("."))]
        if self.context:
            parameters = [parameter for cookie in cookies if (parameter := cookie_parameter(cookie))]
            if parameters:
                await self.cdp.call("Storage.setCookies", {"cookies": parameters})
            cookies = (await self.cdp.call("Storage.getCookies"))["cookies"]
            cookies = [cookie for cookie in cookies if campus_host(cookie.get("domain", "").lstrip("."))]
        snapshot = json.dumps(cookies)
        stage = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=self.directory, delete=False) as file:
                stage = Path(file.name)
                file.write(snapshot)
                file.flush()
                os.fsync(file.fileno())
            os.replace(stage, self.directory / "cookies.json")
            self.cookie_stamp.write_text(hashlib.sha256(snapshot.encode()).hexdigest(), encoding="utf-8")
        except OSError:
            raise SsoError("登录状态保存失败，请检查 AGENT_SSO_DIR 的写入权限") from None
        finally:
            if stage and stage.exists():
                stage.unlink()

    @contextlib.asynccontextmanager
    async def temporary_context(self, restore=True):
        if self.context:
            raise SsoError("临时浏览器上下文不能嵌套")
        # The browser itself also disposes these pages/popups if this helper is
        # killed or its connection drops, including cancellation by the agent.
        result = await self.cdp.call("Target.createBrowserContext", {"disposeOnDetach": True})
        self.context = result["browserContextId"]
        try:
            if restore:
                cookies = (await self.cdp.call("Storage.getCookies"))["cookies"]
                parameters = [parameter for cookie in cookies if (parameter := cookie_parameter(cookie))]
                if parameters:
                    await self.cdp.call("Storage.setCookies", {"cookies": parameters, "browserContextId": self.context})
            yield
        finally:
            try:
                await self.cdp.call("Target.disposeBrowserContext", {"browserContextId": self.context})
            finally:
                self.context = None
                self.target = None
                self.session = None

    async def new_page(self, login=False):
        options = {"url": "about:blank"}
        if self.context:
            options["browserContextId"] = self.context
        # Explicit newWindow=false fails when the profile has no open window yet.
        if login and not self.headless:
            options["newWindow"] = True
        result = await self.cdp.call("Target.createTarget", options)
        self.target = result["targetId"]
        attached = await self.cdp.call("Target.attachToTarget", {"targetId": self.target, "flatten": True})
        self.session = attached["sessionId"]
        await self.cdp.call("Page.enable", session=self.session)
        await self.cdp.call("Runtime.enable", session=self.session)

    async def evaluate(self, expression):
        result = await self.cdp.call("Runtime.evaluate", {
            "expression": expression, "returnByValue": True, "awaitPromise": True,
        }, self.session)
        if "exceptionDetails" in result:
            raise SsoError("登录页面正在跳转或结构已变化")
        return result.get("result", {}).get("value")

    async def snapshot(self):
        return await self.evaluate("""(() => {
            const image = document.querySelector('#captcha-img');
            const user = document.querySelector('#input-login-user');
            return {
                origin: location.origin, path: location.pathname,
                form: !!user && !!document.querySelector('#input-login-pass'),
                warning: (document.querySelector('#span_warn')?.textContent || '').trim().slice(0, 300),
                imageReady: !!image && image.complete && image.naturalWidth > 0 && image.getBoundingClientRect().width > 0,
                imageSource: image?.src || '',
                captchaPassed: typeof captchaCheckStatus !== 'undefined' && captchaCheckStatus === 'passed',
                interactive: typeof captchaObj !== 'undefined' && captchaObj !== null
            };
        })()""")

    async def authenticated(self, state):
        if state["origin"] != self.auth_origin or not state["path"].startswith("/profile/"):
            return False
        # Return only a boolean. The profile response may contain personal information.
        return await self.evaluate("""(async () => {
            const response = await fetch('/profile/current', {credentials: 'same-origin', cache: 'no-store'});
            if (!response.ok) return false;
            const result = await response.json();
            return result.success === true && result.data != null;
        })()""") is True

    async def captcha_image(self):
        return await self.evaluate("""(() => {
            const image = document.querySelector('#captcha-img');
            if (!image || !image.complete || !image.naturalWidth) return null;
            const canvas = document.createElement('canvas');
            canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
            canvas.getContext('2d').drawImage(image, 0, 0);
            return {source: image.src, image: canvas.toDataURL('image/png').split(',')[1]};
        })()""")

    async def recognize(self, image):
        return await asyncio.to_thread(recognize_captcha, image)

    async def submit(self, account, password, code, source):
        values = json.dumps({"origin": self.auth_origin, "account": account, "password": password,
                             "captcha": code, "source": source})
        return await self.evaluate("""(() => {
            const values = """ + values + """;
            if (location.origin !== values.origin || !location.pathname.startsWith('/jaccount/')) return false;
            const image = document.querySelector('#captcha-img');
            if (values.source && image?.src !== values.source) return false;
            if (typeof switchLoginType === 'function') switchLoginType('password');
            for (const [selector, value] of [
                ['#input-login-user', values.account], ['#input-login-pass', values.password],
                ['#input-login-captcha', values.captcha]
            ]) {
                const input = document.querySelector(selector);
                if (!input) return false;
                Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, value);
                input.dispatchEvent(new Event('input', {bubbles: true}));
                input.dispatchEvent(new Event('change', {bubbles: true}));
            }
            const button = document.querySelector('#submit-password-button');
            if (!button) return false;
            button.click();
            return true;
        })()""")

    async def login(self, account, password):
        try:
            if self.headless:
                async with self.temporary_context():
                    return await self._login(account, password)
            return await self._login(account, password)
        except SsoError as error:
            if self.headless:
                raise SsoError(str(error) + "；如需手动处理，请设置 AGENT_SSO_HEADLESS=0 后重新启动登录") from None
            if self.target:
                raise SsoError(str(error) + "；登录页已保留，可手动处理后重试") from None
            raise
        finally:
            if self.headless and self.target:
                with contextlib.suppress(Exception):
                    closed = await self.cdp.call("Target.closeTarget", {"targetId": self.target})
                    if closed.get("success"):
                        self.target = None
                        self.session = None

    async def _login(self, account, password):
        await self.new_page(login=True)
        await self.cdp.call("Page.navigate", {"url": self.entry_url}, self.session)
        deadline = time.monotonic() + 60
        attempts = 0
        submitted = False
        awaiting_result = False
        previous_source = None
        while time.monotonic() < deadline:
            try:
                state = await self.snapshot()
                if await self.authenticated(state):
                    await self.save_cookies()
                    closed = await self.cdp.call("Target.closeTarget", {"targetId": self.target})
                    if not closed.get("success"):
                        raise SsoError("登录成功，但登录页未能自动关闭")
                    self.target = None
                    return {"ok": True, "reused": not submitted}
            except SsoError as error:
                if str(error) != "登录页面正在跳转或结构已变化":
                    raise
                await asyncio.sleep(0.15)
                continue
            if state["origin"] != self.auth_origin or not state["path"].startswith("/jaccount/") or not state["form"]:
                await asyncio.sleep(0.15)
                continue
            if awaiting_result:
                if state["warning"]:
                    if captcha_error(state["warning"]):
                        awaiting_result = False
                    else:
                        raise SsoError("jAccount 拒绝登录，请检查 EMAIL_USER_ACCOUNT / EMAIL_USER_PASSWORD；邮箱客户端授权码不能替代 jAccount 密码")
                else:
                    await asyncio.sleep(0.15)
                    continue
            if attempts >= MAX_CAPTCHA_ATTEMPTS:
                raise SsoError("验证码识别连续失败，已停止自动尝试")
            if state["interactive"]:
                raise SsoError("当前登录要求额外交互验证")
            if not state["captchaPassed"] and not state["imageReady"]:
                await asyncio.sleep(0.15)
                continue
            if not state["captchaPassed"] and state["imageSource"] == previous_source:
                await self.evaluate("document.querySelector('#captcha-img')?.click()")
                await asyncio.sleep(0.15)
                continue
            code, source = "", ""
            if not state["captchaPassed"]:
                image = await self.captcha_image()
                if not image:
                    await asyncio.sleep(0.15)
                    continue
                source = image["source"]
                code = await self.recognize(image["image"])
                previous_source = source
                attempts += 1
                if not re.fullmatch(r"[A-Za-z0-9]{1,10}", code):
                    continue
            else:
                attempts += 1
            if await self.submit(account, password, code, source):
                submitted = True
                awaiting_result = True
            await asyncio.sleep(0.15)
        raise SsoError("登录未在限定时间内完成，可能需要二次验证或检查网络")

    async def open_site(self, url):
        if not web_url(url):
            raise SsoError("网页地址必须是 HTTP(S) URL，且不能包含账号密码")
        await self.new_page()
        result = await self.cdp.call("Page.navigate", {"url": url}, self.session)
        if result.get("errorText"):
            raise SsoError("网页导航失败，请检查地址和网络")
        return {"ok": True, "headless": self.headless}

    async def page_content(self):
        deadline = time.monotonic() + 20
        previous = None
        stable_since = time.monotonic()
        while time.monotonic() < deadline:
            try:
                state = await self.evaluate("""(() => {
                    const safeUrl = raw => {
                        try {
                            const url = new URL(raw, location.href);
                            if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return null;
                            for (const key of [...url.searchParams.keys()]) {
                                if (/^(access_token|refresh_token|id_token|ticket|password|passwd|pwd|sessionid|sid|samlresponse)$/i.test(key)) url.searchParams.delete(key);
                            }
                            if (/access_token|id_token|password/i.test(url.hash)) url.hash = '';
                            return url.href;
                        } catch { return null; }
                    };
                    const text = (document.body?.innerText || '').trim();
                    const seen = new Set();
                    const links = [...document.links].slice(0, 400).flatMap(link => {
                        const url = safeUrl(link.href);
                        const text = (link.innerText || link.getAttribute('aria-label') || link.title || '').trim().slice(0, 120);
                        if (!url || !text || seen.has(url)) return [];
                        seen.add(url);
                        return [{text, url}];
                    }).slice(0, 80);
                    return {ready: document.readyState, origin: location.origin, loginForm: !!document.querySelector('#input-login-pass'), url: safeUrl(location.href),
                            title: document.title, text: text.slice(0, 16000), truncated: text.length > 16000, links};
                })()""")
            except SsoError as error:
                if str(error) != "登录页面正在跳转或结构已变化":
                    raise
                await asyncio.sleep(0.15)
                continue
            if state["origin"] == AUTH_ORIGIN and state["loginForm"] and state["ready"] == "complete":
                raise SsoError("页面需要 jAccount 登录，请先调用 sso_login 后重试读取")
            signature = (state["url"], state["text"])
            if signature != previous:
                previous = signature
                stable_since = time.monotonic()
            if state["url"] and state["ready"] == "complete" and time.monotonic() - stable_since >= 0.4:
                del state["ready"], state["origin"], state["loginForm"]
                return state
            await asyncio.sleep(0.15)
        raise SsoError("网页内容未在限定时间内加载完成，请稍后重试")

    async def read_site(self, url):
        if not web_url(url):
            raise SsoError("网页地址必须是 HTTP(S) URL，且不能包含账号密码")
        campus = campus_url(url)
        async with self.temporary_context(restore=campus):
            await self.open_site(url)
            page = await self.page_content()
            if campus:
                await self.save_cookies()
        return {"ok": True, "headless": self.headless, "page": page, "page_closed": True}


def captcha_error(message):
    text = message.lower()
    if any(word in text for word in ("password", "account", "密码", "账号", "用户名")):
        return False
    return "captcha" in text or "验证码" in text


def recognize_captcha(encoded):
    global _ocr
    try:
        with contextlib.redirect_stdout(sys.stderr):
            import ddddocr
            if _ocr is None:
                _ocr = ddddocr.DdddOcr(show_ad=False)
            result = _ocr.classification(base64.b64decode(encoded, validate=True))
        return re.sub(r"\s+", "", result)
    except ImportError:
        raise SsoError("缺少本地 OCR 依赖，请执行 python3 -m pip install ddddocr") from None
    except Exception:
        raise SsoError("本地 OCR 无法识别验证码图片，请检查 ddddocr / onnxruntime 安装") from None


async def handle_request(request):
    action = request.get("action")
    if action not in {"login", "read", "open"}:
        raise SsoError("未知的 SSO 操作")
    account = normalize_account(request.get("account", "")) if request.get("account", "").strip() else "anonymous"
    password = request.get("password", "")
    if action == "login" and (account == "anonymous" or not password):
        raise SsoError("EMAIL_USER_ACCOUNT 和 EMAIL_USER_PASSWORD 不能为空")
    browser = await Browser.connect(request, account)
    try:
        if action == "login":
            return await browser.login(account, password)
        if action == "read":
            return await browser.read_site(request.get("url", ""))
        return await browser.open_site(request.get("url", ""))
    finally:
        await browser.close()


def main():
    try:
        request = json.loads(sys.stdin.read(65536))
        result = asyncio.run(asyncio.wait_for(handle_request(request), timeout=85))
    except SsoError as error:
        result = {"ok": False, "error": str(error)}
    except TimeoutError:
        result = {"ok": False, "error": "SSO 操作超时，请检查网络后重试"}
    except Exception:
        result = {"ok": False, "error": "SSO 助手运行失败，请检查浏览器及 Python 依赖"}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
