"""Exercise the actual distribution in an isolated temporary installation."""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import zipfile


repo = Path(__file__).resolve().parent.parent
archives = list((repo / "dist").glob("JIsjtu-*.zip"))
archive = max(archives, key=lambda path: path.stat().st_mtime)
with tempfile.TemporaryDirectory(prefix="jisjtu-package-") as temporary:
    root = Path(temporary)
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        assert not any(name.endswith(".env") for name in names)
        assert len([name for name in names if not name.endswith("/")]) == 6
        package.extractall(root)
    extracted = root / archive.stem
    assert {path.name for path in extracted.iterdir()} == {"install.sh", "agent", "agent_frontend", "skills"}
    prefix = root / "install with space and 'quote"
    profile = root / "shell-profile"
    args = ["sh", str(extracted / "install.sh"), "--prefix", str(prefix), "--profile", str(profile)]
    subprocess.run(args, check=True, capture_output=True, text=True)
    launcher = prefix / "bin/JIsjtu"
    config = prefix / "share/JIsjtu/.env"
    assert config.stat().st_mode & 0o777 == 0o600
    assert subprocess.check_output([str(launcher), "--config"], text=True).strip() == str(config)
    assert "启动服务" in subprocess.check_output([str(launcher), "--help"], text=True)
    with config.open("a") as file:
        file.write("\n# existing-user-config\n")
    user_skill = prefix / "share/JIsjtu/skills/my-skill/SKILL.md"
    user_skill.parent.mkdir(parents=True)
    user_skill.write_text("---\nname: my-skill\ndescription: 用户自己加的技能\n---\n")
    assert (prefix / "share/JIsjtu/skills/README.md").is_file(), "Skills directory must be installed"
    first_profile = profile.read_bytes()
    subprocess.run(args, check=True, capture_output=True, text=True)
    assert profile.read_bytes() == first_profile, "PATH registration must be idempotent"
    assert "existing-user-config" in config.read_text(), "Update must preserve user configuration"
    assert user_skill.is_file(), "Update must preserve user skills"
    resolved = subprocess.check_output(["sh", "-c", '. "$1"; command -v JIsjtu', "sh", str(profile)], text=True)
    assert resolved.strip() == str(launcher), "Paths with spaces and quotes must work"
    conflict_prefix = root / "conflict"
    (conflict_prefix / "bin").mkdir(parents=True)
    conflict = conflict_prefix / "bin/JIsjtu"
    conflict.write_text("unrelated user program\n")
    result = subprocess.run(["sh", str(extracted / "install.sh"), "--prefix", str(conflict_prefix), "--no-path"], capture_output=True)
    assert result.returncode != 0 and conflict.read_text() == "unrelated user program\n"
    print("PASS: archive contents, no credentials, install, quoted paths, PATH, reinstall, config preservation, conflict protection.", flush=True)

    if "--smoke" in __import__("sys").argv:
        # Move the unpacked distribution away: the installed app must be independent.
        extracted.rename(root / "unpacked-moved-away")
        sockets = [socket.socket(), socket.socket()]
        for listener in sockets:
            listener.bind(("127.0.0.1", 0))
        ports = [listener.getsockname()[1] for listener in sockets]
        for listener in sockets:
            listener.close()
        env = dict(os.environ, AGENT_NO_BROWSER="1", AGENT_HTTP_PORT=str(ports[0]), AGENT_TCP_PORT=str(ports[1]))
        with (root / "server.log").open("w+") as log:
            process = subprocess.Popen([str(launcher)], cwd="/private/tmp" if Path("/private/tmp").is_dir() else "/tmp", env=env, stdout=log, stderr=log)
            try:
                base = f"http://127.0.0.1:{ports[0]}"
                for attempt in range(100):
                    if process.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    try:
                        with urllib.request.urlopen(base, timeout=1) as response:
                            assert "交我集" in response.read().decode()
                        break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise AssertionError("HTTP server did not start")
                for asset in ("style.css", "app.js"):
                    with urllib.request.urlopen(f"{base}/{asset}", timeout=2) as response:
                        assert response.read() == (prefix / "share/JIsjtu/agent_frontend" / asset).read_bytes()
                with socket.create_connection(("127.0.0.1", ports[1]), timeout=2) as tcp:
                    assert tcp.recv(32) == b"you:"
                print("PASS: installed release starts from unrelated directory and serves frontend plus TCP without unpacked files.", flush=True)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
