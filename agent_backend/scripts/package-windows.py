"""Build the Windows x64 release ZIP with Cargo and the MinGW-w64 toolchain."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile

from pdfium_asset import add_pdfium_files
from tesseract_asset import add_tesseract_files


BACKEND = Path(__file__).resolve().parents[1]
PROJECT = BACKEND.parent
TARGET = "x86_64-pc-windows-gnu"
SKILLS = (
    "course-selection-research", "email-to-teacher", "exam-review-plan",
    "lecture-notes", "sjtu-campus-guide", "weekly-study-plan",
)


def verify_binary(binary):
    details = subprocess.check_output(
        ["x86_64-w64-mingw32-objdump", "-p", binary], text=True,
        env=dict(os.environ, LC_ALL="C"),
    )
    if "file format pei-x86-64" not in details:
        raise ValueError(f"{binary} is not a Windows x64 executable")
    imports = set(re.findall(r"dll name:\s*(\S+)", details.lower()))
    system_dlls = {
        "advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll", "combase.dll", "crypt32.dll",
        "dnsapi.dll", "gdi32.dll", "iphlpapi.dll", "kernel32.dll", "msvcrt.dll",
        "ncrypt.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "rpcrt4.dll", "secur32.dll",
        "shell32.dll", "shlwapi.dll", "user32.dll", "userenv.dll", "ws2_32.dll",
    }
    unexpected = {name for name in imports - system_dlls if not name.startswith("api-ms-win-")}
    if not imports or unexpected:
        raise ValueError(f"Review Windows DLL requirements before packaging: {sorted(unexpected)}")


def windows_config(stage):
    template = (BACKEND / ".env.example").read_text(encoding="utf-8")
    examples = {
        "通过 macOS 通知提醒": "通过 Windows 系统通知提醒",
        "# AGENT_SSO_PYTHON=/absolute/path/to/python3": "# AGENT_SSO_PYTHON=C:/Users/学生/AppData/Local/JIsjtu/.venv/Scripts/python.exe",
        "# AGENT_SSO_BROWSER=/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge": '# AGENT_SSO_BROWSER="C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"',
        "# AGENT_SSO_DIR=/absolute/path/to/private/sso": "# AGENT_SSO_DIR=C:/Users/学生/AppData/Local/JIsjtu/.sso",
    }
    for original, replacement in examples.items():
        template = template.replace(original, replacement)
    output = stage / ".env.example"
    output.write_bytes(template.encode("utf-8"))
    return output


def main():
    target_dir = BACKEND / "target"
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "--target", TARGET, "--target-dir", target_dir],
        cwd=BACKEND, check=True,
    )
    release = target_dir / TARGET / "release" / "agent.exe"
    destination = PROJECT / "JIsjtu-Windows-x64.zip"
    # Work on the same filesystem so the checked archive can be published atomically.
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        stage = Path(temporary)
        binary = stage / "agent.exe"
        subprocess.run(["x86_64-w64-mingw32-strip", "-p", "-o", binary, release], check=True)
        files = {"agent.exe": binary, "LICENSE": PROJECT / "LICENSE"}
        for name in (
            "Install.cmd", "install.ps1", "JIsjtu.cmd", "README.md",
            "Set-ShortcutId.ps1", "Setup-Campus.cmd",
        ):
            files[name] = BACKEND / "packaging" / "windows" / name
        files[".env.example"] = windows_config(stage)
        files["requirements-sso.txt"] = BACKEND / "requirements-sso.txt"
        for name in ("index.html", "app.js", "style.css"):
            entry = f"agent_frontend/{name}"
            files[entry] = PROJECT / entry
        for skill in SKILLS:
            entry = f"skills/{skill}/SKILL.md"
            files[entry] = PROJECT / entry
        add_pdfium_files(files, stage)
        add_tesseract_files(files, stage)
        for name, path in files.items():
            if name.lower().endswith((".exe", ".dll")):
                verify_binary(path)

        archive = stage / destination.name
        with zipfile.ZipFile(archive, "w") as package:
            for name, path in sorted(files.items()):
                info = zipfile.ZipInfo(name)
                info.create_system = 0
                info.external_attr = 0x20
                content = path.read_bytes()
                if name.endswith(".cmd"):
                    content = content.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
                package.writestr(info, content, compress_type=zipfile.ZIP_DEFLATED)
        with zipfile.ZipFile(archive) as package:
            if package.testzip():
                raise ValueError("Windows archive failed CRC verification")
        archive.replace(destination)
    print(destination)


if __name__ == "__main__":
    main()
