"""Exercise the actual distribution in an isolated temporary installation."""
import argparse
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import zipfile


repo = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--archive", type=Path, help="ZIP to verify; defaults to the newest platform archive")
parser.add_argument("--smoke", action="store_true", help="also start the installed HTTP/TCP services")
options = parser.parse_args()
archive = options.archive
if archive is None:
    archives = list((repo / "dist").glob("JIsjtu-*.zip"))
    if not archives:
        parser.error("No distribution found; run scripts/package.sh first")
    archive = max(archives, key=lambda path: path.stat().st_mtime)
with tempfile.TemporaryDirectory(prefix="jisjtu-package-") as temporary:
    root = Path(temporary)
    unpacked = root / "unpacked"
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        assert package.testzip() is None, "ZIP must pass CRC validation"
        forbidden = {".env", ".sso", "sessions", ".git", "__pycache__", ".DS_Store"}
        assert not any(forbidden.intersection(Path(name).parts) for name in names), "Private/runtime files must not be distributed"
        assert not any(Path(name).is_absolute() or ".." in Path(name).parts for name in names)
        package.extractall(unpacked)
    extracted = unpacked if (unpacked / "install.sh").is_file() else unpacked / archive.stem
    assert {path.name for path in extracted.iterdir()} == {
        "install.sh", "agent", "agent_frontend", "skills", "README.md", "LICENSE",
        ".env.example", "requirements-sso.txt",
    }
    assert (extracted / "install.sh").read_bytes() == (repo.parent / "install.sh").read_bytes(), "Root and packaged installers must stay in sync"
    assert (extracted / ".env.example").read_bytes() == (repo / ".env.example").read_bytes()
    assert (extracted / "requirements-sso.txt").read_bytes() == (repo / "requirements-sso.txt").read_bytes()
    prefix = root / "install with space and 'quote"
    profile = root / "shell-profile"
    args = ["sh", str(extracted / "install.sh"), "--prefix", str(prefix), "--profile", str(profile)]
    subprocess.run(args, check=True, capture_output=True, text=True)
    launcher = prefix / "bin/JIsjtu"
    config = prefix / "share/JIsjtu/.env"
    assert config.stat().st_mode & 0o777 == 0o600
    assert config.read_bytes() == (extracted / ".env.example").read_bytes(), "Fresh configuration must use the current example"
    assert subprocess.check_output([str(launcher), "--config"], text=True).strip() == str(config)
    help_text = subprocess.check_output([str(launcher), "--help"], text=True)
    assert all(option in help_text for option in ("默认启动终端界面", "--web", "--tui", "--sso-login", "--config"))
    installed_binary = prefix / "share/JIsjtu/agent"
    release_backup = installed_binary.with_name(".agent-launcher-test")
    installed_binary.rename(release_backup)
    try:
        installed_binary.write_text('#!/bin/sh\nprintf "argc=%s\\n" "$#"\nfor arg do printf "arg=%s\\n" "$arg"; done\n')
        installed_binary.chmod(0o755)
        for arguments, expected in (([], "argc=1\narg=--tui\n"), (["--tui"], "argc=1\narg=--tui\n"),
                                    (["--web"], "argc=0\n"), (["web"], "argc=0\n"),
                                    (["--sso-login"], "argc=1\narg=--sso-login\n")):
            assert subprocess.check_output([str(launcher), *arguments], text=True) == expected
    finally:
        release_backup.replace(installed_binary)
    missing_credentials = subprocess.run(
        [str(launcher), "--sso-login"], text=True, capture_output=True, timeout=5,
        env=dict(os.environ, EMAIL_USER_ACCOUNT="", EMAIL_USER_PASSWORD=""),
    )
    assert missing_credentials.returncode != 0 and "EMAIL_USER_ACCOUNT" in missing_credentials.stderr
    assert "未知参数" not in missing_credentials.stderr, "Launcher must forward --sso-login to the release binary"
    with config.open("a") as file:
        file.write("\n# existing-user-config\n")
    original_config = config.read_bytes()
    user_skill = prefix / "share/JIsjtu/skills/my-skill/SKILL.md"
    user_skill.parent.mkdir(parents=True)
    user_skill.write_text("---\nname: my-skill\ndescription: 用户自己加的技能\n---\n")
    for asset in (extracted / "skills").rglob("*"):
        if asset.is_file():
            assert (prefix / "share/JIsjtu/skills" / asset.relative_to(extracted / "skills")).read_bytes() == asset.read_bytes()
    for asset in (".env.example", "requirements-sso.txt", "README.md", "LICENSE"):
        assert (prefix / "share/JIsjtu" / asset).read_bytes() == (extracted / asset).read_bytes()
    session = prefix / "share/JIsjtu/sessions/user-note.txt"
    session.parent.mkdir()
    session.write_text("preserved user session data\n")
    cookie = prefix / "share/JIsjtu/.sso/test/cookies.json"
    cookie.parent.mkdir(parents=True)
    cookie.write_text("[]\n")
    first_profile = profile.read_bytes()
    subprocess.run(args, check=True, capture_output=True, text=True)
    assert profile.read_bytes() == first_profile, "PATH registration must be idempotent"
    assert config.read_bytes() == original_config, "Update must preserve user configuration byte for byte"
    assert user_skill.is_file(), "Update must preserve user skills"
    assert session.read_text() == "preserved user session data\n", "Update must preserve sessions"
    assert cookie.read_text() == "[]\n", "Update must preserve SSO state"
    resolved = subprocess.check_output(["sh", "-c", '. "$1"; command -v JIsjtu', "sh", str(profile)], text=True)
    assert resolved.strip() == str(launcher), "Paths with spaces and quotes must work"
    conflict_prefix = root / "conflict"
    (conflict_prefix / "bin").mkdir(parents=True)
    conflict = conflict_prefix / "bin/JIsjtu"
    conflict.write_text("unrelated user program\n")
    result = subprocess.run(["sh", str(extracted / "install.sh"), "--prefix", str(conflict_prefix), "--no-path"], capture_output=True)
    assert result.returncode != 0 and conflict.read_text() == "unrelated user program\n"
    # The root installer also supports the actual repository layout and newest release.
    repo_prefix = root / "repository-install"
    subprocess.run(["sh", str(repo.parent / "install.sh"), "--prefix", str(repo_prefix), "--no-path"],
                   check=True, capture_output=True, text=True)
    assert (repo_prefix / "share/JIsjtu/agent").read_bytes() == (extracted / "agent").read_bytes(), "Repository and ZIP installs must use the same release"
    print("PASS: archive, dependency files, launcher flags, install, quoted paths, PATH, reinstall, config/session/cookie/skill preservation, repository install, conflict protection.", flush=True)

    if options.smoke:
        # Move the unpacked distribution away: the installed app must be independent.
        extracted.rename(root / "unpacked-moved-away")
        sockets = [socket.socket(), socket.socket()]
        for listener in sockets:
            listener.bind(("127.0.0.1", 0))
        ports = [listener.getsockname()[1] for listener in sockets]
        for listener in sockets:
            listener.close()
        env = dict(os.environ, AGENT_NO_BROWSER="1", AGENT_SSO_AUTO_LOGIN="0", AGENT_HTTP_PORT=str(ports[0]), AGENT_TCP_PORT=str(ports[1]))
        with (root / "server.log").open("w+") as log:
            process = subprocess.Popen([str(launcher), "--web"], cwd="/private/tmp" if Path("/private/tmp").is_dir() else "/tmp", env=env, stdout=log, stderr=log)
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
