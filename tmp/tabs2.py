import asyncio, json, sys
import websockets

port, path = sys.argv[1], sys.argv[2]
async def main():
    async with websockets.connect(f"ws://127.0.0.1:{port}{path}", proxy=None, max_size=1<<24, open_timeout=5) as ws:
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
            if t["type"] == "page":
                print("PAGE |", t["title"][:60], "|", t["url"][:120], "|", t["targetId"])
        v = await call("Browser.getVersion")
        print("BROWSER", v["result"]["product"])
asyncio.run(main())
