import asyncio, json
import websockets

PORT, PATH = 53650, "/devtools/browser/8506f2b1-f236-49ef-97d4-9db626816e28"
URL = "https://shuiyuan.sjtu.edu.cn/"

async def main():
    async with websockets.connect(f"ws://127.0.0.1:{PORT}{PATH}", proxy=None,
                                  max_size=1 << 24, open_timeout=5) as ws:
        i = 0
        async def call(m, params=None, session=None):
            nonlocal i
            i += 1
            msg = {"id": i, "method": m, "params": params or {}}
            if session:
                msg["sessionId"] = session
            await ws.send(json.dumps(msg))
            while True:
                d = json.loads(await ws.recv())
                if d.get("id") == i:
                    if "error" in d:
                        raise RuntimeError(str(d["error"])[:300])
                    return d.get("result", {})

        targets = await call("Target.getTargets")
        page = next((t for t in targets["targetInfos"]
                     if t["type"] == "page" and "shuiyuan" in t["url"]), None)
        if page is None:
            page = await call("Target.createTarget", {
                "url": URL, "newWindow": True, "width": 1400, "height": 900})
            tid = page["targetId"]
        else:
            tid = page["targetId"]
            print("reused existing window")
        await asyncio.sleep(6)
        att = await call("Target.attachToTarget", {"targetId": tid, "flatten": True})
        sid = att["sessionId"]
        await call("Runtime.enable", session=sid)
        r = await call("Runtime.evaluate", {
            "expression": "document.title + ' | ' + location.href + ' | chars=' + (document.body.innerText||'').length",
            "returnByValue": True}, session=sid)
        print("WINDOW:", r["result"].get("value"))

asyncio.run(main())
