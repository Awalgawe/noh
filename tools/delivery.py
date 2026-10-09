#!/usr/bin/env python3
"""Acquire locked candidate inputs, preserve materials and verify inert delivery assets."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import struct
import subprocess
import tarfile
import tempfile
import urllib.parse
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parents[1]
LIMIT = 2 * 1024 ** 3  # GitHub release assets must be strictly smaller.
HEX = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
# Classification only: minimum-OS and loader behavior still need native acceptance.
WINDOWS_SYSTEM_DLLS = {name + ".dll" for name in (
    "advapi32 avicap32 avrt bcrypt bcryptprimitives cfgmgr32 combase comctl32 comdlg32 "
    "crypt32 d2d1 d3d9 d3d11 d3d12 d3dcompiler_47 dbghelp dinput8 dsound dwmapi "
    "dwrite dxgi gdi32 gdiplus glu32 dnsapi imm32 iphlpapi kernel32 mf mfplat mfreadwrite mfuuid "
    "mpr msacm32 msvcrt msvfw32 msimg32 ncrypt netapi32 normaliz ntdll ole32 "
    "oleacc oleaut32 opengl32 powrprof propsys psapi rpcrt4 secur32 setupapi "
    "shcore shell32 shlwapi ucrtbase user32 userenv usp10 uxtheme version winhttp "
    "wininet winmm winspool ws2_32 wsock32 wtsapi32"
).split()}


def pe_imports(data):
    """Read x64 PE32+ normal/delay imports without running the image."""
    def unpack(fmt, offset):
        if offset < 0 or offset + struct.calcsize(fmt) > len(data):
            raise ValueError("Truncated PE image")
        return struct.unpack_from(fmt, data, offset)

    if data[:2] != b"MZ":
        raise ValueError("Not a PE image")
    header, = unpack("<I", 0x3c)
    if data[header:header + 4] != b"PE\0\0":
        raise ValueError("Invalid PE signature")
    machine, count = unpack("<HH", header + 4)
    optional_size, = unpack("<H", header + 20)
    optional = header + 24
    magic, = unpack("<H", optional)
    if machine != 0x8664 or magic != 0x20b:
        raise ValueError("Expected Windows x64 PE32+ image")
    if optional_size < 112 or optional + optional_size + count * 40 > len(data):
        raise ValueError("Truncated PE headers")
    directory_count, = unpack("<I", optional + 108)
    if directory_count > (optional_size - 112) // 8:
        raise ValueError("Invalid PE data-directory count")
    sections = []
    for i in range(count):
        virtual_size, virtual, raw_size, raw = unpack("<IIII", optional + optional_size + 40 * i + 8)
        if raw + raw_size > len(data):
            raise ValueError("Truncated PE section")
        sections.append((virtual, max(virtual_size, raw_size), raw, raw_size))

    def rva(value, size=1):
        matches = [raw + value - virtual for virtual, span, raw, raw_size in sections
                   if virtual <= value < virtual + span and value - virtual + size <= raw_size]
        if len(matches) != 1:
            raise ValueError(f"Unmapped/ambiguous PE RVA: {value:x}")
        return matches[0]

    imports = {"normal": set(), "delay": set()}
    image_base, = unpack("<Q", optional + 24)
    for kind, index, size, name_offset in (("normal", 1, 20, 12), ("delay", 13, 32, 4)):
        if index >= directory_count:
            continue
        directory, length = unpack("<II", optional + 112 + 8 * index)
        if not directory:
            if length:
                raise ValueError("Invalid empty PE import directory")
            continue
        if length < size:
            raise ValueError("Invalid PE import directory length")
        # This bounds descriptor parsing and ensures each read is backed by section bytes.
        rva(directory, length)
        for delta in range(0, length - size + 1, size):
            offset = rva(directory + delta, size)
            if not any(data[offset:offset + size]):
                break
            value, = unpack("<I", offset + name_offset)
            if kind == "delay":
                attributes, = unpack("<I", offset)
                if attributes not in (0, 1):
                    raise ValueError("Unsupported PE delay-import attributes")
                if attributes == 0:
                    value -= image_base
            name_start = rva(value)
            end = data.find(b"\0", name_start, min(name_start + 512, len(data)))
            if end == -1:
                raise ValueError("Unterminated PE library name")
            rva(value, end - name_start + 1)
            try:
                name = data[name_start:end].decode("ascii").lower()
            except UnicodeDecodeError as error:
                raise ValueError("Invalid PE library name") from error
            relative(name)
            if PurePosixPath(name).name != name or not name.endswith(".dll"):
                raise ValueError("Expected a bare DLL import name")
            imports[kind].add(name)
        else:
            raise ValueError("Unterminated PE import table")
    return {kind: sorted(names) for kind, names in imports.items()}


def audit_pe_images(images):
    """Check bundled imports and explicit prerequisites without consulting the host."""
    speech = read(ROOT / "assets/speech-bundle.json")
    prerequisite = speech.get("windows_prerequisite", {})
    external_crt = set(prerequisite.get("files", []))
    permitted_crt = {"msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll"}
    if external_crt and (external_crt != permitted_crt or prerequisite.get("architecture") != "x64"):
        raise ValueError("Unsupported Windows speech prerequisite")
    report = {"schema_version": 1, "scope": "x64 PE normal/delay imports; co-located DLLs",
              "limitations": ["LoadLibrary/dlopen names are not inferred from code.",
                              "Windows/API-set classification does not qualify minimum OS or loader behavior.",
                              "NVIDIA GPU use requires its separately installed driver; CPU fallback needs acceptance."],
              "images": {}, "missing": [], "external_prerequisites": []}
    for name, data in images:
        relative(name)
        key = name.casefold()
        if key in report["images"]:
            raise ValueError(f"Case-colliding PE image: {name}")
        report["images"][key] = {"path": name, "sha256": hashlib.sha256(data).hexdigest(),
                                 "imports": pe_imports(data)}
    if not report["images"]:
        raise ValueError("No native Windows images to audit")
    for key, image in report["images"].items():
        dependencies = {}
        parent = PurePosixPath(key).parent
        for dependency in sorted(set().union(*image["imports"].values())):
            candidate = str(parent / dependency)
            if candidate in report["images"]:
                dependencies[dependency] = {"kind": "bundled", "path": report["images"][candidate]["path"]}
            elif dependency in WINDOWS_SYSTEM_DLLS or re.fullmatch(r"(?:api|ext)-ms-win-[a-z0-9-]+\.dll", dependency):
                dependencies[dependency] = {"kind": "windows"}
            elif PurePosixPath(key).name == "ggml-cuda.dll" and dependency == "nvcuda.dll":
                dependencies[dependency] = {"kind": "external_nvidia_driver"}
            elif (dependency in external_crt and parent == PurePosixPath("bin/speech")
                  and speech["sha256"].get(PurePosixPath(key).name) == image["sha256"]):
                # Only the exact locked speech images may depend on this prerequisite.
                # It is not a Windows system component or proof of a working installation.
                dependencies[dependency] = {"kind": "external_visual_cpp_runtime"}
                if not report["external_prerequisites"]:
                    report["external_prerequisites"].append(prerequisite)
            else:
                dependencies[dependency] = {"kind": "missing"}
                report["missing"].append({"image": image["path"], "library": dependency})
        image["dependencies"] = dependencies
    return report


def audit_pe_bundle(bundle):
    return audit_pe_images((path.relative_to(bundle).as_posix(), path.read_bytes())
                           for path in sorted(Path(bundle).rglob("*"))
                           if path.is_file() and path.suffix.lower() in (".exe", ".dll"))


def require_pe_closure(report):
    if report["missing"]:
        missing = sorted({entry["library"] for entry in report["missing"]})
        raise ValueError("Missing bundled Windows dependencies: " + ", ".join(missing))


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checked(command, **kwargs):
    kwargs.setdefault("timeout", 3600)
    return subprocess.run([str(x) for x in command], check=True, **kwargs)


def relative(name):
    p = PurePosixPath(name)
    if (not name or "\\" in name or ":" in name or p.is_absolute()
            or any(part in ("", ".", "..") or part.endswith((".", " "))
                   or re.fullmatch(r"(?i)(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?", part)
                   for part in name.split("/"))):
        raise ValueError(f"Unsafe relative path: {name!r}")
    return p


def inventory(folder):
    files = {}
    seen = set()
    for path in sorted(Path(folder).rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Symlinks are not accepted in the delivery envelope: {path}")
        if path.is_file():
            name = path.relative_to(folder).as_posix()
            relative(name)
            if name.casefold() in seen:
                raise ValueError(f"Case-colliding filename: {name}")
            seen.add(name.casefold())
            files[name] = {"sha256": digest(path), "size": path.stat().st_size}
    return files


def fetch(spec, cache, name):
    relative(name)
    url, expected = spec.get("download_url", spec["url"]), spec["sha256"]
    if urllib.parse.urlparse(url).scheme != "https" or not HEX.fullmatch(expected):
        raise ValueError(f"Unpinned or insecure download: {name}")
    path = Path(cache) / name
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_file():
        if digest(path) != expected:
            raise ValueError(f"Corrupt cached download: {name}; remove it deliberately before retrying")
        return path
    pending = path.with_name(path.name + ".download")
    checked(["curl", "--fail", "--location", "--proto", "=https", "--proto-redir", "=https",
             "--retry", "2", "--connect-timeout", "30", "--max-time", "1800",
             "--output", pending, url])
    if digest(pending) != expected:
        raise ValueError(f"SHA-256 mismatch: {name}")
    pending.replace(path)
    return path


def zip_members(archive):
    seen = set()
    for member in archive.infolist():
        name = member.filename.rstrip("/")
        relative(name)
        if name.casefold() in seen:
            raise ValueError(f"Duplicate ZIP member: {name}")
        seen.add(name.casefold())
        kind = (member.external_attr >> 16) & 0o170000
        if kind not in (0, stat.S_IFREG, stat.S_IFDIR):
            raise ValueError(f"Special ZIP member: {name}")
    return archive.infolist()


def copy_member(archive, basename, output, expected):
    with zipfile.ZipFile(archive) as z:
        members = zip_members(z)
        matches = [m for m in members if not m.is_dir() and PurePosixPath(m.filename).name == basename]
        if len(matches) != 1:
            raise ValueError(f"Missing/ambiguous upstream member: {basename}")
        with z.open(matches[0]) as src, Path(output).open("wb") as dst:
            shutil.copyfileobj(src, dst)
    if digest(output) != expected:
        raise ValueError(f"Upstream file checksum mismatch: {basename}")


def visual_cpp_inputs(spec, installer, license_document, runtime, materials):
    """Extract pinned Microsoft CAB payloads; never execute/install the redistributable."""
    if platform.system() != "Windows":
        raise ValueError("Visual C++ payload extraction requires Windows expand.exe")
    if digest(installer) != spec["sha256"] or digest(license_document) != spec["license"]["sha256"]:
        raise ValueError("Visual C++ installer/license checksum mismatch")
    work = materials / "visual-cpp"
    work.mkdir(exist_ok=False)
    raw = Path(installer).read_bytes()
    outer = spec["cabinet"]
    cab = work / "payload.cab"
    cab.write_bytes(raw[outer["offset"]:outer["offset"] + outer["size"]])
    if digest(cab) != outer["sha256"]:
        raise ValueError("Visual C++ embedded cabinet checksum mismatch")
    # Select only flat, fixed members; no installer, MSI or arbitrary CAB tree is run.
    relative(outer["member"])
    if PurePosixPath(outer["member"]).name != outer["member"]:
        raise ValueError("Expected flat Visual C++ cabinet member")
    checked(["expand.exe", "-F:" + outer["member"], cab, work], capture_output=True)
    inner = work / outer["member"]
    if digest(inner) != outer["member_sha256"]:
        raise ValueError("Visual C++ x64 cabinet checksum mismatch")
    for name, item in spec["files"].items():
        if relative(name).name != name or relative(item["member"]).name != item["member"]:
            raise ValueError("Expected flat Visual C++ runtime member")
        checked(["expand.exe", "-F:" + item["member"], inner, work], capture_output=True)
        source = work / item["member"]
        if digest(source) != item["sha256"]:
            raise ValueError(f"Visual C++ runtime checksum mismatch: {name}")
        shutil.copy2(source, runtime / name)
    with zipfile.ZipFile(license_document) as document:
        zip_members(document)
        root = ET.fromstring(document.read("word/document.xml"))
        ns = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}
        text = "\n".join("".join(t.text or "" for t in p.findall(".//w:t", ns))
                         for p in root.findall(".//w:p", ns)) + "\n"
    (runtime / "VISUAL-CPP-LICENSE.txt").write_text(text, encoding="utf-8")
    shutil.copy2(license_document, work / "LICENSE.docx")
    write(work / "PROVENANCE.json", spec)


def ffmpeg_producer_inputs(spec, archive, destination, materials):
    """Preserve the producer's matched binary and original build declarations."""
    if digest(archive) != spec["sha256"]:
        raise ValueError("FFmpeg producer archive checksum mismatch")
    copy_member(archive, PurePosixPath(spec["member"]).name,
                destination / "ffmpeg.exe", spec["executable_sha256"])
    producer = materials / "ffmpeg-producer"
    producer.mkdir()
    for item in spec["producer_materials"]:
        copy_member(archive, item["member"], producer / str(relative(item["name"])), item["sha256"])
    write(producer / "PROVENANCE.json", {
        "status": "unreviewed-producer-materials", "archive": spec,
        "coverage": "Producer README/version declarations and license; not a complete corresponding-source attestation"})


def windows_inputs(cache, destination):
    lock = read(ROOT / "assets/delivery-windows.lock.json")
    speech = read(ROOT / "assets/speech-bundle.json")
    preview = read(ROOT / "assets/preview-runtime.json")
    destination.mkdir(parents=True, exist_ok=False)
    runtime, materials = destination / "speech", destination / "materials"
    runtime.mkdir()
    materials.mkdir()
    # Microsoft supplies the runtime directly to the end user. Do not acquire or
    # put its installer, CAB payloads or DLLs in either distributed archive.
    common_sources(cache, materials)
    for item in lock["sources"]:
        shutil.copy2(fetch(item, cache, item["name"]), materials / item["name"])
    native_spec = lock["native_runtime"]
    native_lock = ROOT / str(relative(native_spec["lock"]))
    if digest(native_lock) != native_spec["sha256"]:
        raise ValueError("Native runtime lock checksum mismatch")
    from windows_runtime import acquire
    acquire(read(native_lock), cache, destination, materials)
    with tarfile.open(materials / "whisper-b5130.tar.gz") as tar:
        names = [m for m in tar.getmembers() if m.name.endswith("/LICENSE") and m.name.count("/") == 1]
        if len(names) != 1:
            raise ValueError("Whisper source LICENSE is ambiguous")
        (runtime / "WHISPER-CPP-LICENSE.txt").write_bytes(tar.extractfile(names[0]).read())
    whisper = fetch({"url": speech["runtime"], "sha256": speech["runtime_sha256"]}, cache, "whisper-b5130.zip")
    cuda = fetch({"url": speech["cuda_cublas"], "sha256": speech["cuda_cublas_sha256"]}, cache, "cublas-11.11.3.6.zip")
    for name, expected in speech["sha256"].items():
        if name in ("ggml-small.bin", "ggml-silero-v6.2.0.bin"):
            url = speech["model" if name == "ggml-small.bin" else "vad"]
            shutil.copy2(fetch({"url": url, "sha256": expected}, cache, name), runtime / name)
        elif name in ("WHISPER-MODEL-LICENSE.txt", "SILERO-LICENSE.txt"):
            spec = next(x for x in lock["licenses"] if x["name"] == name)
            shutil.copy2(fetch(spec, cache, name), runtime / name)
        elif name == "CUDA-RUNTIME-LICENSE.txt":
            copy_member(cuda, "LICENSE", runtime / name, expected)
        elif name != "WHISPER-CPP-LICENSE.txt":
            source = cuda if name.startswith(("cublas64", "cublasLt")) else whisper
            copy_member(source, name, runtime / name, expected)
        if digest(runtime / name) != expected:
            raise ValueError(f"Speech input mismatch: {name}")
    # Keep acquisition explicit; any caller-seeded cache is rehashed by fetch.
    if digest(destination / "native/preview/libmpv-2.dll") != preview["sha256"]:
        raise ValueError("Preview DLL differs from the authoritative preview lock")
    write(materials / "INPUTS.json", {"windows": lock, "speech": speech, "preview": preview,
                                      "status": "unqualified", "files": inventory(materials)})
    validate_ffmpeg(destination / "native/export/ffmpeg.exe")


def validate_ffmpeg(executable):
    text = checked([executable, "-version"], capture_output=True, text=True).stdout
    if not text.startswith("ffmpeg version 7.1") or "--enable-nonfree" in text:
        raise ValueError("Expected FFmpeg 7.1 without --enable-nonfree")
    filters = checked([executable, "-hide_banner", "-filters"], capture_output=True, text=True).stdout
    if not any(line.split()[1:2] == ["subtitles"] for line in filters.splitlines()):
        raise ValueError("Missing FFmpeg subtitles/libass filter")
    encoders = checked([executable, "-hide_banner", "-encoders"], capture_output=True, text=True).stdout
    for name in ("libx264", "aac", "png"):
        if not any(line.split()[1:2] == [name] for line in encoders.splitlines()):
            raise ValueError(f"Missing FFmpeg encoder: {name}")


def git_source_archive(spec, cache, output):
    """Export an exact public Git tree without checkout, hooks or mutable tags."""
    url = urllib.parse.urlsplit(spec["url"])
    revision = spec.get("revision", "")
    if (url.scheme != "https" or not url.hostname or url.username or url.password
            or url.query or url.fragment or not isinstance(revision, str) or not COMMIT.fullmatch(revision)):
        raise ValueError("Git sources require credential-free HTTPS and an exact commit")
    if output.exists():
        raise ValueError("Refusing to overwrite Git source archive")
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="git-source-", dir=cache) as temporary:
        work = Path(temporary)
        template = work / "empty-template"
        template.mkdir()
        config = work / "empty-config"
        config.write_text("", encoding="utf-8")
        env = os.environ | {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1",
                            "GIT_CONFIG_COUNT": "0", "GIT_TERMINAL_PROMPT": "0"}
        repository = work / "repository"
        checked(["git", "init", "--bare", f"--template={template}", repository], env=env)
        checked(["git", "-C", repository, "-c", "protocol.file.allow=never", "fetch",
                 "--depth=1", "--no-tags", spec["url"], revision], env=env, timeout=180)
        actual = checked(["git", "-C", repository, "rev-parse", "FETCH_HEAD"], env=env,
                         capture_output=True, text=True).stdout.strip()
        if actual != revision:
            raise ValueError("Git source resolved to a different commit")
        tree = checked(["git", "-C", repository, "ls-tree", "-r", "-z", revision],
                       env=env, capture_output=True).stdout
        if any(entry.startswith(b"160000 ") for entry in tree.split(b"\0")):
            raise ValueError("Git source has submodules requiring separate pinned source inputs")
        # Keep every tracked source, including files marked export-ignore upstream.
        (repository / "info").mkdir(exist_ok=True)
        (repository / "info/attributes").write_text("* -export-ignore -export-subst\n", encoding="utf-8")
        archive = work / "source.tar"
        checked(["git", "-C", repository, "archive", "--format=tar", "--prefix=source/",
                 f"--output={archive}", revision], env=env)
        shutil.copy2(archive, output)
    return spec | {"sha256": digest(output), "file": output.name}


def macos_source_materials(lock, core, cache, destination, names=None):
    """Preserve pinned recipes, stable sources/resources and every declared patch."""
    selected = sorted(names if names is not None else lock["formulae"])
    if (lock.get("status") == "incomplete-source-inputs"
            or any("resources" not in lock["formulae"][name]
                   or any(not patch.get("sha256") for patch in lock["formulae"][name]["patches"])
                   for name in selected)):
        raise ValueError("Incomplete Homebrew source inputs; enrich stable resources and patch checksums from the exact recipes")
    if digest(core) != lock["core"]["sha256"]:
        raise ValueError("Homebrew core source checksum mismatch")
    destination.mkdir(parents=True, exist_ok=False)
    records = {}
    with tarfile.open(core) as archive:
        prefix = "homebrew-core-" + lock["core_commit"]

        def core_file(name):
            member = archive.getmember(prefix + "/" + str(relative(name)))
            if not member.isfile():
                raise ValueError("Expected a regular Homebrew source file")
            return archive.extractfile(member).read()

        def acquire(spec, output):
            if spec.get("sha256"):
                cached = fetch(spec, cache, "macos-" + output.parent.name + "-" + output.name)
                shutil.copy2(cached, output)
                return spec | {"file": output.name}
            return git_source_archive(spec, cache, output)

        for name in selected:
            relative(name)
            if "/" in name:
                raise ValueError("Expected a simple Homebrew formula name")
            formula = lock["formulae"][name]
            folder = destination / name
            folder.mkdir()
            recipe = core_file(formula["recipe"])
            if hashlib.sha256(recipe).hexdigest() != formula["recipe_sha256"]:
                raise ValueError(f"Formula changed: {name}")
            (folder / "recipe.rb").write_bytes(recipe)
            source = formula["source"]
            source_spec = {"url": source["url"], **({"sha256": source["checksum"]}
                           if source.get("checksum") else {"revision": source.get("revision")})}
            if source.get("download_url"):
                source_spec["download_url"] = source["download_url"]
            record = {"recipe_sha256": formula["recipe_sha256"], "bottle": formula["bottle"],
                      "source": acquire(source_spec, folder / "source.archive"),
                      "resources": [], "patches": []}
            resource_names = set()
            for resource in formula["resources"]:
                # Resource names are logical identifiers (e.g. Test::Harness),
                # not filesystem paths. Preserve the original name in provenance.
                label = urllib.parse.quote(resource["name"], safe="-_.")
                relative("resource-" + label + ".archive")
                if not label or label.casefold() in resource_names:
                    raise ValueError("Unsafe or duplicate Homebrew resource name")
                resource_names.add(label.casefold())
                record["resources"].append(acquire(resource, folder / ("resource-" + label + ".archive")))
            for index, patch in enumerate(formula["patches"]):
                output = folder / f"patch-{index}.patch"
                if patch.get("url"):
                    result = acquire(patch, output)
                else:
                    if patch.get("file"):
                        data = core_file(patch["file"])
                    elif patch.get("data") and b"\n__END__\n" in recipe:
                        data = recipe.partition(b"\n__END__\n")[2]
                    else:
                        raise ValueError("Missing Homebrew patch source")
                    if hashlib.sha256(data).hexdigest() != patch["sha256"]:
                        raise ValueError("Homebrew patch checksum mismatch")
                    output.write_bytes(data)
                    result = patch | {"file": output.name}
                    if patch.get("file"):
                        result["source_file"] = patch["file"]
                record["patches"].append(result)
            records[name] = record
            write(folder / "SOURCES.json", record)
    write(destination / "SOURCES.json", {"schema_version": 1, "status": "unreviewed-source-materials",
          "core_commit": lock["core_commit"], "core_sha256": lock["core"]["sha256"], "formulae": records})


def macos_inputs(cache, destination):
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise ValueError("macOS arm64 acquisition requires an Apple Silicon Mac")
    lock = read(ROOT / "assets/delivery-macos.lock.json")
    destination.mkdir(parents=True, exist_ok=False)
    materials = destination / "materials"
    materials.mkdir()
    common_sources(cache, materials)
    core = fetch(lock["core"], cache, "homebrew-core.tar.gz")
    shutil.copy2(core, materials / core.name)
    # The build runner is disposable. Do not use this on a personal Homebrew installation.
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("Locked Homebrew replacement is restricted to disposable GitHub runners")
    repository = Path(checked(["brew", "--repository"], capture_output=True, text=True).stdout.strip())
    tap = repository / "Library/Taps/homebrew/homebrew-core"
    if tap.exists():
        raise ValueError("Expected an API-only runner without an existing homebrew/core tap")
    tap.mkdir(parents=True)
    with tarfile.open(core) as archive:
        archive.extractall(destination / "core", filter="data")
    snapshot = next((destination / "core").iterdir())
    for path in snapshot.iterdir():
        shutil.move(str(path), tap / path.name)
    checked(["git", "init", tap])
    env = os.environ | {"HOMEBREW_NO_AUTO_UPDATE": "1", "HOMEBREW_NO_INSTALL_FROM_API": "1",
                        "HOMEBREW_NO_INSTALL_CLEANUP": "1", "HOMEBREW_NO_ANALYTICS": "1"}
    names = []

    def visit(name):
        if name in names:
            return
        for dep in lock["formulae"][name]["dependencies"]:
            visit(dep)
        names.append(name)

    visit("ffmpeg@7")
    visit("mpv")
    macos_source_materials(lock, core, cache, materials / "homebrew-sources", names)
    for name in names:
        formula = lock["formulae"][name]
        if digest(tap / formula["recipe"]) != formula["recipe_sha256"]:
            raise ValueError(f"Formula changed: {name}")
        # Homebrew's formula SHA-256 checks apply to downloads and resources.
        checked(["brew", "fetch", "--force-bottle", name], env=env)
        cached = Path(checked(["brew", "--cache", "--force-bottle", name], env=env,
                              capture_output=True, text=True).stdout.strip())
        if digest(cached) != formula["bottle"]["sha256"]:
            raise ValueError(f"Homebrew selected an unpinned bottle: {name}")
        checked(["brew", "reinstall", "--force-bottle", name], env=env)
    write(materials / "HOMEBREW-INPUTS.json", lock)
    # Actual bottle build provenance, native closure and license review remain separate gates.
    checked(["python3", ROOT / "tools/prepare-macos-speech.py", "--output", destination / "speech"])
    shutil.copy2(destination / "speech/whisper-source.tar.gz", materials / "whisper-1.9.4.tar.gz")
    ffmpeg = Path(checked(["brew", "--prefix", "ffmpeg@7"], env=env,
                          capture_output=True, text=True).stdout.strip()) / "bin/ffmpeg"
    validate_ffmpeg(ffmpeg)
    write(destination / "PATHS.json", {"ffmpeg": str(ffmpeg),
          "libmpv": str(Path(checked(["brew", "--prefix", "mpv"], env=env,
                                    capture_output=True, text=True).stdout.strip()) / "lib/libmpv.2.dylib")})


def sources(destination, cache=None):
    destination.mkdir(parents=True, exist_ok=False)
    checked(["git", "archive", "--format=zip", "--output", destination / "NOH-source.zip", "HEAD"], cwd=ROOT)
    vendor = checked(["cargo", "vendor", "--locked", "--offline", "--versioned-dirs", destination / "vendor"],
                     cwd=ROOT, capture_output=True, text=True)
    (destination / "cargo-vendor-config.toml").write_text(portable_vendor_config(vendor.stdout), encoding="utf-8")
    from rust_notices import generate, inventory_vendor
    crates = inventory_vendor(destination / "vendor", ROOT / "Cargo.lock")
    write(destination / "RUST-INVENTORY.json", sorted(crates, key=lambda c: (c["name"], c["version"])))
    generate(destination, cache or ROOT / "logs/delivery/downloads", read(ROOT / "assets/rust-notices.lock.json"), fetch)
    (destination / "README.txt").write_text(
        "Unqualified candidate source materials. NOH-source.zip is the exact Git tree.\n"
        "vendor/ contains every locked Rust crate, including original licenses and sources.\n"
        "Place vendor/ inside the extracted NOH project root, beside Cargo.toml.\n"
        "Copy cargo-vendor-config.toml\n"
        "into .cargo/config.toml by APPENDING its source tables to the existing LLD config.\n"
        "Use the toolchain/target/features and native acquisition recorded by the candidate.\n"
        "rust-notices/ preserves original notice texts and an explicit missing-text report.\n"
        "The SPDX expressions are an inventory, not a completed license review.\n"
        "Native corresponding-source gaps are listed in assets/delivery-policy.json.\n", encoding="utf-8")


def common_sources(cache, materials):
    policy = read(ROOT / "assets/delivery-policy.json")
    spec = policy["rust_source"]
    archive = fetch(spec, cache, f"rustc-{policy['rust_version']}-src.tar.xz")
    shutil.copy2(archive, materials / archive.name)
    write(materials / "RUST-TOOLCHAIN-SOURCE.json", {"version": policy["rust_version"], **spec})


def portable_vendor_config(text):
    result, count = re.subn(r'^directory\s*=.*$', 'directory = "vendor"', text, flags=re.MULTILINE)
    if count < 1:
        raise ValueError("Cargo did not return a vendor source directory")
    return result


def authorize_redistribution(target):
    policy = read(ROOT / "assets/delivery-policy.json")
    gaps = policy["platforms"][target]["redistribution_unresolved"]
    if gaps:
        raise ValueError("Public artifact upload blocked: " + " | ".join(gaps))
    paths = ["Cargo.lock", "LICENSE", "assets/speech-bundle.json"]
    paths += ["assets/rust-notices.lock.json", "tools/rust_notices.py"]
    paths += (["assets/delivery-windows.lock.json", "assets/preview-runtime.json",
               "assets/windows-native.lock.json", "tools/windows_runtime.py"] if target == "windows-x64"
              else ["assets/delivery-macos.lock.json", "tools/prepare-macos-speech.py"])
    locks = {name: digest(ROOT / name) for name in paths}
    locks["rust_standard_library_source"] = policy["rust_source"]["sha256"]
    records = [r for r in policy["redistribution_records"] if r.get("platform") == target and r.get("inputs") == locks]
    if len(records) != 1:
        raise ValueError("No unique reviewed redistribution-material authority for the exact locked inputs")
    required = {"native_corresponding_sources", "native_notices", "rust_sources_notices"}
    if target == "windows-x64":
        required.add("cuda_terms")
    evidence = records[0].get("evidence", {})
    if set(evidence) != required:
        raise ValueError("Incomplete redistribution evidence")
    for spec in evidence.values():
        path = ROOT / str(relative(spec["path"]))
        if not path.is_file() or digest(path) != spec["sha256"]:
            raise ValueError("Missing or changed redistribution evidence")


def verify_bundle(bundle, target, commit):
    manifest = read(bundle / "manifest.json")
    build = manifest["build"]
    if (manifest["schema_version"] != 2 or build["target"] != target or build["profile"] != "release"
            or build["git_revision"] != commit or build["git_dirty"] is not False
            or set(build["features"]) - {"default"} != {"gui", "mcp"}):
        raise ValueError("Wrong source, profile, target or features in packaged build identity")
    files = inventory(bundle)
    expected = manifest["sha256"]
    if set(files) - {"manifest.json"} != set(expected):
        raise ValueError("Bundle manifest does not cover the exact file inventory")
    for name, hash_value in expected.items():
        if files[name]["sha256"] != hash_value:
            raise ValueError(f"Bundle manifest checksum mismatch: {name}")
    return manifest


def zip_folder(folder, output):
    if output.exists():
        raise ValueError(f"Refusing to overwrite delivery asset: {output}")
    # Cargo's original vendor files can carry Unix-epoch timestamps. ZIP dates
    # start in 1980; clamp metadata while preserving exact file contents/hashes.
    with zipfile.ZipFile(output, "x", zipfile.ZIP_DEFLATED, compresslevel=6,
                         allowZip64=True, strict_timestamps=False) as archive:
        for name in inventory(folder):
            archive.write(folder / name, name)
    if output.stat().st_size >= LIMIT:
        raise ValueError(f"Release asset is not strictly below 2 GiB: {output.name}")


def reject_microsoft_payloads(files):
    """The prerequisite is downloaded from Microsoft, never redistributed here."""
    forbidden = {"msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll"}
    for name in files:
        path = PurePosixPath(name.lower())
        if (path.name in forbidden or "visual-cpp" in path.parts
                or (path.name.startswith(("vc_redist", "vcredist")) and path.suffix == ".exe")):
            raise ValueError(f"Microsoft runtime payload must not be redistributed: {name}")


def seal(bundle, materials, output, target, commit):
    policy = read(ROOT / "assets/delivery-policy.json")
    manifest = verify_bundle(bundle, policy["platforms"][target]["target"], commit)
    if target == "windows-x64":
        require_pe_closure(audit_pe_bundle(bundle))
        reject_microsoft_payloads(inventory(bundle))
        reject_microsoft_payloads(inventory(materials))
    version = manifest["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("Unsafe package version")
    output.mkdir(parents=True, exist_ok=False)
    prefix = f"NOH-{version}-{target}"
    zip_folder(bundle, output / f"{prefix}.zip")
    if target == "macos-arm64" and (bundle / "MACOS-SIGNING.json").is_file():
        from macos_signing import verify_archive
        verify_archive(output / f"{prefix}.zip", bundle)
    zip_folder(materials, output / f"{prefix}-materials.zip")
    record = {"schema_version": 1, "status": "unqualified", "platform": target, "version": version,
              "source_commit": commit, "build": manifest["build"],
              "run_id": os.environ.get("GITHUB_RUN_ID"), "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
              "unresolved": policy["platforms"][target]["unresolved"],
              "extracted_bytes": sum(f["size"] for f in inventory(bundle).values()),
              "bundle_files": inventory(bundle), "materials_files": inventory(materials),
              "assets": inventory(output)}
    write(output / "DELIVERY.json", record)
    (output / "SHA256SUMS").write_text("".join(f"{digest(p)}  {p.name}\n" for p in sorted(output.iterdir())
                                              if p.is_file()), encoding="utf-8")
    verify_envelope(output)


def verify_zip(path, expected):
    with zipfile.ZipFile(path) as archive:
        members = [m for m in zip_members(archive) if not m.is_dir()]
        if {m.filename for m in members} != set(expected):
            raise ValueError(f"ZIP inventory mismatch: {path.name}")
        for member in members:
            spec = expected[member.filename]
            if member.file_size != spec["size"]:
                raise ValueError(f"ZIP size mismatch: {member.filename}")
            with archive.open(member) as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != spec["sha256"]:
                    raise ValueError(f"ZIP checksum mismatch: {member.filename}")


def verify_envelope(folder):
    record = read(folder / "DELIVERY.json")
    if record.get("schema_version") != 1 or record.get("status") != "unqualified":
        raise ValueError("Unsupported candidate record")
    if record.get("platform") not in ("windows-x64", "macos-arm64") or not COMMIT.fullmatch(record.get("source_commit", "")):
        raise ValueError("Invalid candidate platform/source identity")
    version = record.get("version", "")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("Invalid candidate version")
    prefix = f"NOH-{version}-{record['platform']}"
    if set(record["assets"]) != {prefix + ".zip", prefix + "-materials.zip"}:
        raise ValueError("Unexpected candidate asset names")
    actual = inventory(folder)
    if set(actual) != set(record["assets"]) | {"DELIVERY.json", "SHA256SUMS"}:
        raise ValueError("Missing or extra delivery asset")
    for name, spec in record["assets"].items():
        if actual[name] != spec or spec["size"] >= LIMIT:
            raise ValueError(f"Asset integrity or size failure: {name}")
        files = record["materials_files" if name.endswith("-materials.zip") else "bundle_files"]
        if record["platform"] == "windows-x64":
            reject_microsoft_payloads(files)
        verify_zip(folder / name, files)
        if record["platform"] == "windows-x64" and not name.endswith("-materials.zip"):
            with zipfile.ZipFile(folder / name) as archive:
                report = audit_pe_images((member.filename, archive.read(member))
                                         for member in archive.infolist()
                                         if PurePosixPath(member.filename).suffix.lower() in (".exe", ".dll"))
            require_pe_closure(report)
    if any(item["size"] >= LIMIT for item in actual.values()):
        raise ValueError("Oversized delivery metadata")
    expected_sums = "".join(f"{digest(folder / name)}  {name}\n" for name in sorted(actual) if name != "SHA256SUMS")
    if (folder / "SHA256SUMS").read_text(encoding="utf-8") != expected_sums:
        raise ValueError("Invalid SHA256SUMS")
    return record


def qualify(folder):
    record = verify_envelope(folder)
    authorize_redistribution(record["platform"])
    policy = read(ROOT / "assets/delivery-policy.json")
    platform_policy = policy["platforms"][record["platform"]]
    if platform_policy["unresolved"]:
        raise ValueError("Public draft blocked: " + " | ".join(platform_policy["unresolved"]))
    matches = [r for r in policy["qualification_records"] if
               r.get("source_commit") == record["source_commit"] and r.get("platform") == record["platform"]
               and r.get("delivery_sha256") == digest(folder / "DELIVERY.json")]
    if len(matches) != 1:
        raise ValueError("No unique independently reviewed qualification for these final bytes")
    qualification = matches[0]
    required = {"corresponding_sources", "rust_notices", "native_import_closure", "clean_install",
                "download_protection", "audio_gpu", "offline_transcription", "media_export"}
    if record["platform"] == "macos-arm64":
        required |= {"developer_id", "notarization", "stapling"}
    if set(qualification.get("evidence", {})) != required:
        raise ValueError("Incomplete reviewed qualification evidence")
    for spec in qualification["evidence"].values():
        path = ROOT / str(relative(spec["path"]))
        if not path.is_file() or digest(path) != spec["sha256"]:
            raise ValueError("Missing or changed reviewed qualification evidence")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    tasks = parser.add_subparsers(dest="command", required=True)
    acquire = tasks.add_parser("acquire")
    acquire.add_argument("--platform", choices=["windows-x64", "macos-arm64"], required=True)
    acquire.add_argument("--cache", type=Path, required=True)
    acquire.add_argument("--output", type=Path, required=True)
    source = tasks.add_parser("sources")
    source.add_argument("--output", type=Path, required=True)
    source.add_argument("--cache", type=Path)
    native_sources = tasks.add_parser("macos-sources")
    native_sources.add_argument("--cache", type=Path, required=True)
    native_sources.add_argument("--output", type=Path, required=True)
    redistribution = tasks.add_parser("redistribution")
    redistribution.add_argument("--platform", choices=["windows-x64", "macos-arm64"], required=True)
    imports = tasks.add_parser("audit-pe")
    imports.add_argument("--bundle", type=Path, required=True)
    imports.add_argument("--output", type=Path, required=True)
    packing = tasks.add_parser("seal")
    packing.add_argument("--platform", choices=["windows-x64", "macos-arm64"], required=True)
    packing.add_argument("--bundle", type=Path, required=True)
    packing.add_argument("--materials", type=Path, required=True)
    packing.add_argument("--output", type=Path, required=True)
    packing.add_argument("--commit", required=True)
    for command in ("verify", "qualify"):
        child = tasks.add_parser(command)
        child.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "acquire":
        (windows_inputs if args.platform == "windows-x64" else macos_inputs)(args.cache.resolve(), args.output.resolve())
    elif args.command == "sources":
        sources(args.output.resolve(), args.cache.resolve() if args.cache else None)
    elif args.command == "macos-sources":
        lock = read(ROOT / "assets/delivery-macos.lock.json")
        core = fetch(lock["core"], args.cache.resolve(), "homebrew-core.tar.gz")
        macos_source_materials(lock, core, args.cache.resolve(), args.output.resolve())
    elif args.command == "redistribution":
        authorize_redistribution(args.platform)
    elif args.command == "audit-pe":
        if args.output.exists():
            raise ValueError("Refusing to overwrite PE import report")
        report = audit_pe_bundle(args.bundle.resolve())
        write(args.output, report)
        require_pe_closure(report)
    elif args.command == "seal":
        if not COMMIT.fullmatch(args.commit):
            parser.error("Expected exact lowercase 40-hex source commit")
        seal(args.bundle.resolve(), args.materials.resolve(), args.output.resolve(), args.platform, args.commit)
    else:
        (verify_envelope if args.command == "verify" else qualify)(args.directory.resolve())
    print(f"Delivery {args.command} completed; public readiness is not implied")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        raise SystemExit(f"Delivery refused: {error}") from error
