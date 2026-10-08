"""Minimal CDP driver that reuses the already-running SSO browser.

Usage:
  python3 cdp.py open  <url> [wait_s]
  python3 cdp.py eval  <url> <js> [wait_s]
Reuses the same tab across calls (keeps a tab id in .tabid).
"""
import asyncio
import json
import sys
from pathlib import Path

import websockets

SSO = Path("/Users/mi7qi/project/JIsjtu/agent_backend/.sso")
TABID = Path(__file__).with_name(".tabid")


def browser_ws():
    for path in SSO.glob("*/browser-*/DevToolsActivePort"):
        lines = path.read_text().splitlines()
        return f"ws://127.0.0.1:{int(lines[0])}{lines[1]}"
    raise SystemExit("no running SSO browser")


class Tab:
    def __init__(self, ws):
        self.ws = ws
        self.seq = 0
        self.session = None
        self.target = None

    def new_id(self):
        self.seq += 1
        return self.seq

    async def call(self, method, params=None, session=None):
        mid = self.new_id()
        msg = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        await self.ws.send(json.dumps(msg))
        while True:
            data = json.loads(await self.ws.recv())
            if data.get("id") == mid:
                if "error" in data:
                    raise RuntimeError(f"{method} -> {data['error']}")
                return data.get("result", {})

    async def attach(self, target_id):
        res = await self.call("Target.attachToTarget", {"targetId": target_id, "flatten": True})
        self.target = target_id
        self.session = res["sessionId"]
        await self.call("Page.enable", session=self.session)
        await self.call("Runtime.enable", session=self.session)

    async def use_tab(self, url=None):
        old = TABID.read_text().strip() if TABID.exists() else ""
        if old:
            try:
                await self.attach(old)
                return
            except RuntimeError:
                pass
        res = await self.call("Target.createTarget", {"url": url or "about:blank"})
        TABID.write_text(res["targetId"])
        await self.attach(res["targetId"])

    async def eval(self, js, wait=0.0):
        if wait:
            await asyncio.sleep(wait)
        res = await self.call(
            "Runtime.evaluate",
            {"expression": js, "returnByValue": True, "awaitPromise": True},
            self.session,
        )
        if "exceptionDetails" in res:
            raise RuntimeError(json.dumps(res["exceptionDetails"], ensure_ascii=False)[:800])
        return res.get("result", {}).get("value")

    async def navigate(self, url, wait):
        await self.call("Page.navigate", {"url": url}, self.session)
        deadline = asyncio.get_event_loop().time() + wait
        while asyncio.get_event_loop().time() < deadline:
            await asyncio.sleep(0.4)
            try:
                state = await self.eval("document.readyState")
            except RuntimeError:
                continue
            if state == "complete":
                await asyncio.sleep(1.0)
                return
        await asyncio.sleep(1.0)


async def main():
    mode = sys.argv[1]
    url = sys.argv[2]
    async with websockets.connect(browser_ws(), proxy=None, max_size=32 * 1024 * 1024,
                                  open_timeout=5, close_timeout=2) as ws:
        tab = Tab(ws)
        await tab.use_tab(url)
        if mode == "open":
            wait = float(sys.argv[3]) if len(sys.argv) > 3 else 12
            await tab.navigate(url, wait)
            print(await tab.eval("document.title + ' | ' + location.href"))
        elif mode == "eval":
            js = sys.argv[3]
            wait = float(sys.argv[4]) if len(sys.argv) > 4 else 0
            if url != "-":
                await tab.navigate(url, max(wait, 12))
            out = await tab.eval(js, 0 if url != "-" else wait)
            print(out if isinstance(out, str) else json.dumps(out, ensure_ascii=False))
        else:
            raise SystemExit("mode must be open|eval")


asyncio.run(main())
