"""Own one temporary background browser and reap it when its controller exits.

The first stdin line contains only launch arguments and the temporary profile.
Subsequent lines request shutdown; EOF also requests shutdown. Credentials and
cookies never enter this process. It never searches for or kills other browsers.
"""

import asyncio
import contextlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import threading


def read_line(limit):
    # Use raw reads: a daemon blocked on BufferedReader during interpreter
    # shutdown can itself hang the guardian when the browser exits first.
    line = bytearray()
    while len(line) < limit:
        byte = os.read(sys.stdin.fileno(), 1)
        if not byte or byte == b"\n":
            break
        line.extend(byte)
    return bytes(line)


async def request_browser_exit(profile):
    import websockets

    lines = (profile / "DevToolsActivePort").read_text().splitlines()
    port, endpoint = int(lines[0]), lines[1]
    if not 0 < port < 65536 or not re.fullmatch(r"/devtools/browser/[\w-]+", endpoint):
        return
    async with websockets.connect(
        f"ws://127.0.0.1:{port}{endpoint}", proxy=None,
        open_timeout=1, close_timeout=0.2, max_size=1024 * 1024,
    ) as socket:
        await socket.send(json.dumps({"id": 1, "method": "Browser.close"}))
        await asyncio.wait_for(socket.recv(), timeout=1)


def stop_browser(child, profile):
    if child.poll() is not None:
        return
    with contextlib.suppress(Exception):
        asyncio.run(asyncio.wait_for(request_browser_exit(profile), timeout=2))
    try:
        child.wait(timeout=3)
        return
    except subprocess.TimeoutExpired:
        pass
    # The Popen handle is still owned and unreaped, so PID reuse cannot target
    # another application between the normal and forced shutdown attempts.
    with contextlib.suppress(ProcessLookupError):
        child.terminate()
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError):
            child.kill()
        child.wait(timeout=2)


def main():
    child = None
    profile = None
    try:
        request = json.loads(read_line(131072))
        profile = Path(request["profile"])
        child = subprocess.Popen(
            request["arguments"], stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        stopping = threading.Event()

        def stop_signal(*_):
            stopping.set()

        for name in ("SIGINT", "SIGTERM"):
            if hasattr(signal, name):
                signal.signal(getattr(signal, name), stop_signal)

        def watch_controller():
            try:
                while True:
                    command = read_line(64)
                    if not command:
                        return
                    if command.strip() == b"kill":
                        with contextlib.suppress(ProcessLookupError):
                            child.kill()
                    if command.strip() in {b"stop", b"kill"}:
                        stopping.set()
            finally:
                stopping.set()

        threading.Thread(target=watch_controller, daemon=True).start()
        print(json.dumps({"pid": child.pid}), flush=True)
        while child.poll() is None and not stopping.wait(0.2):
            pass
        return 0
    except Exception:
        # Never forward subprocess arguments or tracebacks to the caller.
        print(json.dumps({"error": "后台浏览器管理进程启动失败"}), flush=True)
        return 1
    finally:
        try:
            if child is not None:
                stop_browser(child, profile)
        finally:
            if profile is not None and (child is None or child.poll() is not None):
                shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
