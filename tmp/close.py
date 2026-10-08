import asyncio, json
from pathlib import Path
import websockets

SSO = Path("/Users/mi7qi/project/JIsjtu/agent_backend/.sso")
TABID = Path(__file__).with_name(".tabid")

ports = list(SSO.glob("*/browser-*/DevToolsActivePort"))
if not ports:
    print("no running SSO browser")
    raise SystemExit(0)
lines = ports[0].read_text().splitlines()
ws_url = f"ws://127.0.0.1:{int(lines[0])}{lines[1]}"

async def main():
    async with websockets.connect(ws_url, proxy=None, open_timeout=5) as ws:
        seq = 0
        async def call(method, params=None, session=None):
            nonlocal seq
            seq += 1
            msg = {"id": seq, "method": method, "params": params or {}}
            if session:
                msg["sessionId"] = session
            await ws.send(json.dumps(msg))
            while True:
                d = json.loads(await ws.recv())
                if d.get("id") == seq:
                    return d
        info = await call("Target.getTargets")
        pages = [t for t in info["result"]["targetInfos"]
                 if t["type"] == "page" and t.get("url", "").startswith(("http://", "https://"))]
        print("found tabs:", [(t["targetId"][:8], t["url"][:60]) for t in pages])
        own = TABID.read_text().strip() if TABID.exists() else ""
        for t in pages:
            if t["targetId"] == own or "i.sjtu.edu.cn" in t["url"]:
                r = await call("Target.closeTarget", {"targetId": t["targetId"]})
                print("closed", t["targetId"][:8], r.get("result"))
        print("Browser.close ->", (await call("Browser.close")).get("result"))

asyncio.run(main())
