import asyncio, json
import websockets
from pathlib import Path
SSO = Path("/Users/mi7qi/project/JIsjtu/agent_backend/.sso")

def browser_ws():
    for p in SSO.glob("*/browser-*/DevToolsActivePort"):
        l = p.read_text().splitlines()
        return f"ws://127.0.0.1:{int(l[0])}{l[1]}"
    raise SystemExit("no browser")

async def main():
    async with websockets.connect(browser_ws(), proxy=None, max_size=1<<24) as ws:
        i = 0
        async def call(m, params=None):
            nonlocal i
            i += 1
            await ws.send(json.dumps({"id": i, "method": m, "params": params or {}}))
            while True:
                d = json.loads(await ws.recv())
                if d.get("id") == i:
                    return d
        r = await call("Target.getTargets")
        for t in r["result"]["targetInfos"]:
            print(t["type"], "|", t["title"][:50], "|", t["url"][:110])
asyncio.run(main())
