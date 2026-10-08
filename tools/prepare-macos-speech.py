#!/usr/bin/env python3
"""Prepare pinned native Whisper and models for maintainers, never at app startup."""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = "1.9.4"
SOURCE = f"https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v{VERSION}.tar.gz"
SOURCE_SHA = "57e280cee375ab02425b806ad5146b99f6eb9357e3c2b31357c8a6af2e2e44ae"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fetch(url, path, expected):
    if path.is_file() and digest(path) == expected:
        return
    pending = path.with_suffix(path.suffix + ".download")
    subprocess.run(["curl", "--fail", "--location", "--retry", "3",
                    "--connect-timeout", "30", "--max-time", "1800",
                    "--output", str(pending), url], check=True, timeout=1900)
    if digest(pending) != expected:
        raise RuntimeError(f"Checksum mismatch: {url}")
    pending.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cmake", default=shutil.which("cmake"))
    parser.add_argument("--output", type=Path, default=ROOT / ".mcp-dev/macos-speech")
    args = parser.parse_args()
    if platform.system() != "Darwin" or not args.cmake:
        parser.error("Run on macOS with CMake installed (or pass --cmake).")
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((ROOT / "assets/speech-bundle.json").read_text())
    for name, url in [
        ("ggml-small.bin", manifest["model"]),
        ("ggml-silero-v6.2.0.bin", manifest["vad"]),
        ("WHISPER-MODEL-LICENSE.txt", manifest["licenses"][0]),
        ("SILERO-LICENSE.txt", manifest["licenses"][1]),
    ]:
        fetch(url, destination / name, manifest["sha256"][name])
    archive = destination / "whisper-source.tar.gz"
    fetch(SOURCE, archive, SOURCE_SHA)
    with tarfile.open(archive) as source:
        source.extractall(destination / "source", filter="data")
    source = destination / "source" / f"whisper.cpp-{VERSION}"
    build = destination / "build"
    options = [
        "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF",
        "-DWHISPER_BUILD_TESTS=OFF", "-DWHISPER_BUILD_SERVER=OFF",
        "-DWHISPER_SDL2=OFF", "-DWHISPER_USE_SYSTEM_GGML=OFF",
        "-DWHISPER_USE_SYSTEM_LLAMA=OFF", "-DGGML_BACKEND_DL=OFF",
        "-DGGML_NATIVE=OFF", "-DGGML_METAL=ON",
        "-DGGML_METAL_EMBED_LIBRARY=ON", "-DGGML_BLAS=ON",
        "-DGGML_BLAS_VENDOR=Apple", "-DGGML_OPENMP=OFF",
    ]
    subprocess.run([args.cmake, "-S", str(source), "-B", str(build), *options],
                   check=True, timeout=120)
    subprocess.run([args.cmake, "--build", str(build), "--target", "whisper-cli",
                    "--parallel", "4"], check=True, timeout=1800)
    executable = destination / "whisper-cli"
    shutil.copy2(build / "bin/whisper-cli", executable)
    shutil.copy2(source / "LICENSE", destination / "WHISPER-CPP-LICENSE.txt")
    (destination / "WHISPER-BUILD.json").write_text(json.dumps({
        "version": VERSION, "source": SOURCE, "source_sha256": SOURCE_SHA,
        "architecture": platform.machine(), "cmake_options": options,
        "executable_sha256": digest(executable),
        "cmake_version": subprocess.check_output([args.cmake, "--version"], text=True),
    }, indent=2) + "\n")
    print(f"Prepared native speech bundle: {destination}")


if __name__ == "__main__":
    main()
