#!/usr/bin/env python3
"""Build the pinned, offline CPU speech runtime for a Linux x86_64 delivery."""
import argparse
import hashlib
import platform
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tarfile

from delivery import ROOT, digest, fetch, read, relative, write
from linux_elf import elf_info

VERSION = "1.9.4"
SOURCE = {
    "url": f"https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v{VERSION}.tar.gz",
    "sha256": "57e280cee375ab02425b806ad5146b99f6eb9357e3c2b31357c8a6af2e2e44ae",
}
OPTIONS = [
    "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF",
    "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON",
    "-DWHISPER_BUILD_TESTS=OFF", "-DWHISPER_BUILD_SERVER=OFF",
    "-DWHISPER_BUILD_IS_DEV=OFF",
    "-DWHISPER_SDL2=OFF", "-DWHISPER_USE_SYSTEM_GGML=OFF",
    "-DWHISPER_USE_SYSTEM_LLAMA=OFF", "-DGGML_BACKEND_DL=OFF",
    "-DGGML_NATIVE=OFF", "-DGGML_METAL=OFF", "-DGGML_CUDA=OFF",
    "-DGGML_VULKAN=OFF", "-DGGML_BLAS=OFF", "-DGGML_OPENMP=OFF",
    "-DGGML_CPU_ALL_VARIANTS=OFF", "-DGGML_SSE42=OFF", "-DGGML_BMI2=OFF",
    "-DGGML_AVX=OFF", "-DGGML_AVX2=OFF", "-DGGML_FMA=OFF",
    "-DGGML_F16C=OFF", "-DGGML_AVX_VNNI=OFF", "-DGGML_AVX512=OFF",
    "-DGGML_AVX512_VBMI=OFF", "-DGGML_AVX512_VNNI=OFF",
    "-DGGML_AVX512_BF16=OFF", "-DGGML_AMX_TILE=OFF",
    "-DGGML_AMX_INT8=OFF", "-DGGML_AMX_BF16=OFF",
    "-DCMAKE_C_FLAGS=-march=x86-64 -mtune=generic",
    "-DCMAKE_CXX_FLAGS=-march=x86-64 -mtune=generic",
]


def source_tree(archive, destination):
    if destination.is_symlink():
        raise ValueError("Whisper source cache must not be a symlink")
    expected = {}
    with tarfile.open(archive) as source:
        for member in source.getmembers():
            relative(member.name.rstrip('/'))
            if member.isdir():
                continue
            if not member.isreg() or member.name in expected:
                raise ValueError("Whisper source archive contains a link, device or duplicate")
            with source.extractfile(member) as stream:
                expected[member.name] = hashlib.file_digest(stream, 'sha256').hexdigest()
        # Upstream CMakeLists.txt:47 writes this file into its source directory.
        # Accept only the exact rendering of the pinned template, not arbitrary
        # extra cache files or changes to actual compiler inputs.
        generated = f'whisper.cpp-{VERSION}/bindings/javascript/package.json'
        if (destination / generated).exists():
            template = generated.replace('package.json', 'package-tmpl.json')
            with source.extractfile(template) as stream:
                value = stream.read()
            allowed = {hashlib.sha256(value.replace(b'@WHISPER_VERSION@', version.encode())).hexdigest()
                       for version in (VERSION, VERSION + '-dev')}
            actual_generated = digest(destination / generated)
            if actual_generated not in allowed:
                raise ValueError("Generated Whisper package metadata differs from its pinned template")
            expected[generated] = actual_generated
        if not destination.exists():
            destination.mkdir(parents=True)
            source.extractall(destination, filter="data")
    actual = {}
    for path in destination.rglob('*'):
        if path.is_symlink():
            raise ValueError("Whisper source cache contains a symlink")
        if path.is_file():
            actual[path.relative_to(destination).as_posix()] = digest(path)
    if actual != expected:
        raise ValueError("Whisper source cache differs from the pinned archive; repair the cache deliberately")


def check_cpu_commands(path):
    commands = read(path)
    if not commands:
        raise ValueError("Whisper build has no compiler commands")
    forbidden = re.compile(r'-m(?:avx\S*|fma|f16c|bmi\S*|sse[34]\S*|ssse3|amx\S*|arch=native)')
    for item in commands:
        flags = item['arguments'] if 'arguments' in item else shlex.split(item['command'])
        if (any(forbidden.fullmatch(flag) for flag in flags)
                or '-march=x86-64' not in flags or '-mtune=generic' not in flags
                or any(flag.startswith('-march=') and flag != '-march=x86-64' for flag in flags)):
            raise ValueError("Whisper compiler flags exceed the declared x86-64 CPU baseline")
    return commands


def prepare(output, cache, seeds=(), jobs=2):
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("Build the speech runtime natively on Linux x86_64")
    if not 1 <= jobs <= 32:
        raise ValueError("Build jobs must be between 1 and 32")
    output.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    speech = read(ROOT / "assets/speech-bundle.json")
    for name, url in (
        ("ggml-small.bin", speech["model"]),
        ("ggml-silero-v6.2.0.bin", speech["vad"]),
        ("WHISPER-MODEL-LICENSE.txt", speech["licenses"][0]),
        ("SILERO-LICENSE.txt", speech["licenses"][1]),
    ):
        expected = speech["sha256"][name]
        destination = output / name
        if destination.is_file() and digest(destination) == expected:
            continue
        for seed in seeds:
            existing = seed / name
            if existing.is_file() and digest(existing) == expected:
                shutil.copy2(existing, destination)
                break
        else:
            shutil.copy2(fetch({"url": url, "sha256": expected}, cache, name), destination)
    archive = fetch(SOURCE, cache, f"whisper-{VERSION}.tar.gz")
    source_root = cache / f"whisper-{VERSION}-source"
    source_tree(archive, source_root)
    source = source_root / f"whisper.cpp-{VERSION}"
    build = cache / f"whisper-{VERSION}-linux-x86_64-build"
    subprocess.run(["cmake", "--fresh", "-S", source, "-B", build, *OPTIONS], check=True, timeout=180)
    commands = check_cpu_commands(build / 'compile_commands.json')
    subprocess.run(["cmake", "--build", build, "--target", "whisper-cli",
                    "--parallel", str(jobs)], check=True, timeout=1800)
    executable = output / "whisper-cli"
    shutil.copy2(build / "bin/whisper-cli", executable)
    executable.chmod(0o755)
    info = elf_info(executable.read_bytes())
    shutil.copy2(source / "LICENSE", output / "WHISPER-CPP-LICENSE.txt")
    write(output / "WHISPER-BUILD.json", {
        "schema_version": 1, "version": VERSION, "source": SOURCE,
        "architecture": "x86_64", "cpu_baseline": "x86-64; SSE2; no AVX requirement",
        "cmake_options": OPTIONS, "elf": info,
        "compile_commands_sha256": digest(build / 'compile_commands.json'),
        "compiler_commands": commands,
        "compiler": subprocess.check_output(["c++", "--version"], text=True),
        "cmake_version": subprocess.check_output(["cmake", "--version"], text=True),
        "sha256": {p.name: digest(p) for p in sorted(output.iterdir())
                   if p.is_file() and p.name != "WHISPER-BUILD.json"},
    })
    print(f"Prepared Linux CPU speech runtime: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--seed", type=Path, action="append", default=[])
    parser.add_argument("--jobs", type=int, default=2)
    args = parser.parse_args()
    prepare(args.output.resolve(), args.cache.resolve(), args.seed, args.jobs)


if __name__ == "__main__":
    main()
