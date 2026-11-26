"""Exercise OCR and single-page image input with the installed Windows executable."""
import argparse
import base64
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading


class ImageModel(BaseHTTPRequestHandler):
    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        images = [part["image_url"]["url"] for message in data["messages"]
                  if isinstance(message["content"], list) for part in message["content"]
                  if part["type"] == "image_url"]
        if not (self.path == "/v1/chat/completions" and data["stream"] and len(images) == 1
                and images[0].startswith("data:image/jpeg;base64,")):
            return self.send_error(400, "Expected exactly one rendered PDF page")
        jpeg = base64.b64decode(images[0].split(",", 1)[1], validate=True)
        if len(jpeg) < 1000 or not jpeg.startswith(b"\xff\xd8") or not jpeg.endswith(b"\xff\xd9"):
            return self.send_error(400, "Invalid JPEG")
        chunk = {"id": "fixture", "object": "chat.completion.chunk", "created": 1,
                 "model": "fixture", "choices": [{"index": 0, "delta": {"content": "PAGE_RECEIVED"},
                                                   "finish_reason": "stop"}]}
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="Installed agent.exe")
    parser.add_argument("--wine", help="Optional Wine executable for a cross-compiled distribution")
    args = parser.parse_args()
    cmd = ([args.wine] if args.wine else []) + [str(args.binary.resolve()), "--tool", "pdf_analysis"]
    with tempfile.TemporaryDirectory() as temporary, ThreadingHTTPServer(("127.0.0.1", 0), ImageModel) as server:
        shutil.copyfile(Path(__file__).parent / "fixtures/campus-scan.pdf", Path(temporary) / "scan.pdf")
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = dict(os.environ, OPENAI_BASE_URL=f"http://127.0.0.1:{server.server_port}/v1",
                   OPENAI_API_KEY="local-test", MODEL="fixture", NO_PROXY="127.0.0.1")
        def invoke(**options):
            payload = json.dumps(dict(path="scan.pdf", **options))
            return subprocess.run([*cmd, payload], cwd=temporary, env=env, check=True,
                                  stdout=subprocess.PIPE, encoding="utf-8", timeout=180).stdout
        try:
            for number in (1, 2):
                text = "".join(invoke(start_page=number, end_page=number).lower().split())
                assert "上海交通大学" in text and "campuslibrary" in text, text
                assert text.count("nativecoursetitle") == (1 if number == 1 else 0), text
            assert "PAGE_RECEIVED" in invoke(image_page=2)
        finally:
            server.shutdown()
            thread.join()
    print("PASS: mixed and scanned PDF pages, Chinese and English offline OCR, single-page image attachment.")


if __name__ == "__main__":
    main()
