"""Build the bundled Windows x64 offline OCR engine using CMake, Make and MinGW-w64.

The package builder calls add_tesseract_files(). To stage just the OCR runtime:
    python3 scripts/tesseract_asset.py <output-directory>
JISJTU_OCR_CACHE may point to an existing source and CMake build cache.
"""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

BACKEND = Path(__file__).resolve().parents[1]
MODELS = "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/4.1.0"
GCC = "https://raw.githubusercontent.com/gcc-mirror/gcc/releases/gcc-15.1.0"
MINGW = "https://raw.githubusercontent.com/mingw-w64/mingw-w64/v14.0.0"
SOURCES = {
    "leptonica.tar.gz": ("https://codeload.github.com/DanBloomberg/leptonica/tar.gz/refs/tags/1.85.0",
                         "c01376bce0379d4ea4bc2ec5d5cbddaa49e2f06f88242619ab8c059e21adf233"),
    "tesseract.tar.gz": ("https://codeload.github.com/tesseract-ocr/tesseract/tar.gz/refs/tags/5.5.2",
                         "6235ea0dae45ea137f59c09320406f5888383741924d98855bd2ce0d16b54f21"),
    "chi_sim.traineddata": (MODELS + "/chi_sim.traineddata", "a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730"),
    "eng.traineddata": (MODELS + "/eng.traineddata", "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2"),
    "LICENSE-tessdata": (MODELS + "/LICENSE", "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"),
    "GCC-RUNTIME": (GCC + "/COPYING.RUNTIME", "9d6b43ce4d8de0c878bf16b54d8e7a10d9bd42b75178153e3af6a815bdc90f74"),
    "GCC-GPLv3": (GCC + "/COPYING3", "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903"),
    "MinGW": (MINGW + "/COPYING", "99a69660981156c21336fdb5661f89341b013c94e4bf9e1c7467b4745718397f"),
    "MinGW-AUTHORS": (MINGW + "/AUTHORS", "8f308316e7a94b8410f871868bd3948f126526dc1775aeee97a0e365f9a0fd9f"),
    "Winpthreads": (MINGW + "/mingw-w64-libraries/winpthreads/COPYING", "63263614cdd29f2f93cba85e992f041b31f9fc7b4033692f31269489a8a1b177"),
}

TOOLCHAIN = """set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_EXE_LINKER_FLAGS_INIT "-static -Wl,--no-insert-timestamp")
"""


def fetch_sources(cache):
    for name, (url, digest) in SOURCES.items():
        dest = cache / name
        if dest.is_file() and hashlib.sha256(dest.read_bytes()).hexdigest() == digest:
            continue
        with tempfile.TemporaryDirectory(dir=cache) as temp:
            downloaded = Path(temp) / name
            subprocess.run(["curl", "-fLsS", "--retry", "2", "--connect-timeout", "15", "--max-time", "180",
                            "--output", downloaded, url], check=True)
            if hashlib.sha256(downloaded.read_bytes()).hexdigest() != digest:
                raise ValueError(f"SHA-256 mismatch for {url}")
            downloaded.replace(dest)


def build(cache):
    prefix = cache / "prefix"
    toolchain = cache / "mingw.cmake"
    if not toolchain.exists() or toolchain.read_text() != TOOLCHAIN:
        toolchain.write_text(TOOLCHAIN)
    common = [f"-DCMAKE_TOOLCHAIN_FILE={toolchain}", f"-DCMAKE_INSTALL_PREFIX={prefix}",
              "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DSW_BUILD=OFF"]
    jobs = str(min(os.cpu_count() or 1, 8))
    for archive, source, build_dir, target, options in (
        ("leptonica.tar.gz", "leptonica-1.85.0", "build-lept", "install",
         ["-DBUILD_PROG=OFF"] + [f"-DENABLE_{codec}=OFF" for codec in ("PNG", "TIFF", "JPEG", "ZLIB", "GIF", "OPENJPEG", "WEBP")]),
        ("tesseract.tar.gz", "tesseract-5.5.2", "build-tess", "tesseract",
         [f"-DCMAKE_FIND_ROOT_PATH={prefix}", "-DDISABLE_ARCHIVE=ON", "-DDISABLE_CURL=ON",
          "-DDISABLE_TIFF=ON", "-DLEPT_TIFF_RESULT=1", "-DGRAPHICS_DISABLED=ON", "-DENABLE_NATIVE=OFF",
          "-DBUILD_TRAINING_TOOLS=OFF", "-DBUILD_TESTS=OFF", "-DENABLE_CCACHE=OFF"]),
    ):
        if not (cache / source).is_dir():
            with tempfile.TemporaryDirectory(dir=cache) as temp:
                subprocess.run(["tar", "-xzf", cache / archive, "-C", temp], check=True)
                (Path(temp) / source).rename(cache / source)
        subprocess.run(["cmake", "-S", source, "-B", build_dir, *common, *options], cwd=cache, check=True)
        subprocess.run(["cmake", "--build", build_dir, "--target", target, "--parallel", jobs], cwd=cache, check=True)
    return cache / "build-tess/bin/tesseract.exe"


def add_tesseract_files(files, stage):
    cache = Path(os.environ.get("JISJTU_OCR_CACHE", BACKEND / "target/windows-ocr-5.5.2")).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    fetch_sources(cache)
    binary = build(cache)
    output = subprocess.check_output(["x86_64-w64-mingw32-objdump", "-p", binary],
                                     env=dict(os.environ, LC_ALL="C"), text=True).lower()
    imports = set(re.findall(r"dll name:\s*(\S+)", output))
    if ("file format pei-x86-64" not in output or "kernel32.dll" not in imports
            or any(name not in {"kernel32.dll", "msvcrt.dll"} and not name.startswith("api-ms-win-crt-") for name in imports)):
        raise ValueError(f"Unexpected runtime dependencies for Windows OCR: {sorted(imports)}")

    paths = {
        "tessdata/chi_sim.traineddata": cache / "chi_sim.traineddata",
        "tessdata/eng.traineddata": cache / "eng.traineddata",
        "licenses/tesseract/Tesseract.txt": cache / "tesseract-5.5.2/LICENSE",
        "licenses/tesseract/Leptonica.txt": cache / "leptonica-1.85.0/leptonica-license.txt",
        "licenses/tesseract/Tessdata.txt": cache / "LICENSE-tessdata",
    }
    for name in ("GCC-RUNTIME", "GCC-GPLv3", "MinGW", "MinGW-AUTHORS", "Winpthreads"):
        paths[f"licenses/tesseract/{name}.txt"] = cache / name
    folder = stage / "pdf-tools"
    folder.mkdir(parents=True, exist_ok=True)
    exe = folder / "tesseract.exe"
    subprocess.run(["x86_64-w64-mingw32-strip", "-p", "-o", exe, binary], check=True)
    files["pdf-tools/tesseract.exe"] = exe
    for relative, source in paths.items():
        path = folder / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, path)
        files["pdf-tools/" + relative] = path


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    add_tesseract_files({}, parser.parse_args().output)
