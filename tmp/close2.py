import asyncio, json
import websockets

PORT = 52715
WS = open("/Users/mi7qi/project/JIsjtu/agent_backend/.sso/0e203783ac1b6d6496114ac15cd52e92b3926067bf168f018488d5b605c8cdc9/browser/DevToolsActivePort").read().splitlines()
url = f"ws://127.0.0.1:{int(WS[0])}{WS[1]}"

async def main():
    async with websockets.connect(url, proxy=None, open_timeout=5) as ws:
        seq = 0
        async def call(method, params=None):
            nonlocal seq
            seq += 1
            await ws.send(json.dumps({"id": seq, "method": method, "params": params or {}}))
            while True:
                d = json.loads(await ws.recv())
                if d.get("id") == seq:
                    return d
        info = await call("Target.getTargets")
        pages = [t for t in info["result"]["targetInfos"] if t["type"] == "page"]
        for t in pages:
            print("TAB:", t["targetId"][:8], t.get("url", "")[:90])
        for t in pages:
            if t.get("url", "").startswith(("http://", "https://")):
                await call("Target.closeTarget", {"targetId": t["targetId"]})
        await call("Browser.close")
        print("closed leftover SSO browser on port", WS[0])

asyncio.run(main())
