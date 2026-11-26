"""Collect the pinned Windows x64 PDFium library and its redistribution notices."""
import hashlib
from pathlib import Path
from tempfile import NamedTemporaryFile
from urllib.request import urlopen
from zipfile import ZipFile


VERSION = "5.15.2"
WHEEL = f"pypdfium2-{VERSION}-py3-none-win_amd64.whl"
URL = "https://files.pythonhosted.org/packages/fe/7f/8aa446529fa14523e5c18f89e823afc95526689f76a75efe68a82a10d850/" + WHEEL
SHA256 = "74d1bace34fb01c660d0b64344d5a4b7d276942b7c6623c88ab59ea21c5acfbd"


def add_pdfium_files(files, stage):
    cache = Path(__file__).resolve().parents[1] / "target" / "windows-assets"
    cache.mkdir(parents=True, exist_ok=True)
    wheel = cache / WHEEL
    if not wheel.exists():
        with urlopen(URL, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            raise ValueError("PDFium download failed the pinned SHA-256 check")
        with NamedTemporaryFile(dir=cache, delete=False) as temporary:
            temporary.write(data)
        Path(temporary.name).replace(wheel)
    if hashlib.sha256(wheel.read_bytes()).hexdigest() != SHA256:
        raise ValueError(f"Invalid cached PDFium wheel, remove {wheel} and rebuild")

    with ZipFile(wheel) as contents:
        entries = {
            "pdf-tools/pdfium.dll": "pypdfium2_raw/pdfium.dll",
            "pdf-tools/licenses/pdfium/version.json": "pypdfium2_raw/version.json",
        }
        license_prefix = f"pypdfium2-{VERSION}.dist-info/licenses/"
        for name in contents.namelist():
            if name.startswith(license_prefix) and not name.endswith("/"):
                relative = name[len(license_prefix):]
                entries[f"pdf-tools/licenses/pdfium/{relative}"] = name
        if "pdf-tools/licenses/pdfium/BUILD_LICENSES/pdfium.txt" not in entries:
            raise ValueError("The wheel does not contain the PDFium license")
        for name, member in entries.items():
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(contents.read(member))
            files[name] = destination
