"""Inspect Linux x86_64 ELF imports without loading or executing native code."""
import hashlib
from pathlib import Path, PurePosixPath
import posixpath
import re
import struct

# The executable ABI uses the desktop's native runtime. Complete ownership is
# enforced separately by require_host_runtime: no shared library may be shipped.
# Keep the historical generic audit available for old candidate verification.
SYSTEM_LIBRARIES = frozenset({
    "ld-linux-x86-64.so.2", "libc.so.6", "libm.so.6", "libpthread.so.0",
    "libdl.so.2", "librt.so.1", "libresolv.so.2", "libutil.so.1",
    "libGL.so.1", "libGLX.so.0", "libEGL.so.1", "libGLdispatch.so.0",
    "libOpenGL.so.0", "libGLESv1_CM.so.1", "libGLESv2.so.2", "libvulkan.so.1",
})
HOST_EXECUTABLE_LIBRARIES = SYSTEM_LIBRARIES | {
    "libasound.so.2", "libstdc++.so.6", "libgcc_s.so.1",
}
INTERPRETER = "/lib64/ld-linux-x86-64.so.2"


def elf_info(data):
    """Read ELF64 little-endian program headers, dynamic imports and versions."""
    def unpack(fmt, offset):
        if offset < 0 or offset + struct.calcsize(fmt) > len(data):
            raise ValueError("Truncated ELF image")
        return struct.unpack_from(fmt, data, offset)

    if data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("Expected ELF64 little-endian version 1")
    kind, machine, version = unpack("<HHI", 16)
    if kind not in (2, 3) or machine != 62 or version != 1:
        raise ValueError("Expected Linux x86_64 executable/shared ELF image")
    if data[7] not in (0, 3):
        raise ValueError("Unsupported ELF operating-system ABI")
    phoff, = unpack("<Q", 32)
    ehsize, phsize, phnum = unpack("<HHH", 52)
    if ehsize != 64 or phsize != 56 or not 0 < phnum < 65535:
        raise ValueError("Invalid or unsupported ELF program header table")
    if phoff < 64 or phoff + phsize * phnum > len(data):
        raise ValueError("Truncated ELF program header table")
    loads, dynamics, interpreters = [], [], []
    for index in range(phnum):
        ptype, _, offset, address, _, size, memory, _ = unpack("<IIQQQQQQ", phoff + index * phsize)
        if offset + size > len(data) or (ptype == 1 and size > memory):
            raise ValueError("Invalid ELF segment extent")
        if ptype == 1:
            loads.append((address, size, offset))
        elif ptype == 2:
            if size > memory:
                raise ValueError("Invalid ELF dynamic segment extent")
            dynamics.append((offset, address, size))
        elif ptype == 3:
            if not 1 < size < 4096 or data[offset + size - 1] != 0:
                raise ValueError("Invalid ELF interpreter")
            interpreters.append(data[offset:offset + size - 1].decode("ascii"))
    if not loads or len(dynamics) > 1 or len(interpreters) > 1:
        raise ValueError("Ambiguous ELF load/dynamic/interpreter segments")

    def address_offset(address, size=1):
        matches = [offset + address - base for base, length, offset in loads
                   if base <= address and address + size <= base + length]
        if len(matches) != 1:
            raise ValueError("Unmapped or ambiguous ELF virtual address")
        return matches[0]

    tags = {}
    if dynamics:
        offset, address, size = dynamics[0]
        # The loader follows p_vaddr, while this reader consumes file bytes.
        # They must name exactly the same file-backed PT_LOAD extent.
        if not size or address_offset(address, size) != offset:
            raise ValueError("ELF dynamic segment has inconsistent file/load addresses")
        if size % 16:
            raise ValueError("Invalid ELF dynamic segment size")
        terminated = False
        for position in range(offset, offset + size, 16):
            tag, value = unpack("<qQ", position)
            if tag == 0:
                terminated = True
                break
            tags.setdefault(tag, []).append(value)
        if not terminated:
            raise ValueError("Unterminated ELF dynamic table")

    def single(tag, default=None):
        values = tags.get(tag, [])
        if len(values) > 1:
            raise ValueError("Duplicate ELF singleton dynamic tag")
        return values[0] if values else default

    table_address, table_size = single(5), single(10)
    if bool(table_address is not None) != bool(table_size is not None):
        raise ValueError("Incomplete ELF dynamic string table")
    table = None if table_address is None else address_offset(table_address, table_size)

    def string(index):
        if table is None or index < 0 or index >= table_size:
            raise ValueError("ELF string is outside the dynamic string table")
        end = data.find(b"\0", table + index, table + table_size)
        if end == -1:
            raise ValueError("Unterminated ELF string")
        return data[table + index:end].decode("utf-8", errors="strict")

    needed = [string(index) for index in tags.get(1, [])]
    soname = single(14)
    rpath, runpath = single(15), single(29)
    if rpath is not None and runpath is not None:
        raise ValueError("ELF has both RPATH and RUNPATH")
    paths = [] if rpath is None and runpath is None else string(rpath if rpath is not None else runpath).split(":")
    requirements = []
    version_address, version_count = single(0x6ffffffe), single(0x6fffffff, 0)
    if bool(version_address is not None) != bool(version_count):
        raise ValueError("Incomplete ELF version requirements")
    if version_count > 4096:
        raise ValueError("Excessive ELF version requirements")
    current = version_address
    for index in range(version_count):
        offset = address_offset(current, 16)
        ver, count, file_index, auxiliary, next_entry = unpack("<HHIII", offset)
        if ver != 1 or not 0 < count <= 4096 or auxiliary < 16:
            raise ValueError("Invalid ELF version requirement")
        library = string(file_index)
        if library not in needed:
            raise ValueError("ELF version requirement lacks a matching import")
        entry = current + auxiliary
        for aux_index in range(count):
            _, _, _, name, following = unpack("<IHHII", address_offset(entry, 16))
            requirements.append({"library": library, "version": string(name)})
            if aux_index + 1 < count and following < 16:
                raise ValueError("Truncated ELF auxiliary version chain")
            if aux_index + 1 == count and following != 0:
                raise ValueError("Inconsistent ELF auxiliary version count")
            entry += following
        if index + 1 < version_count and next_entry < 16:
            raise ValueError("Truncated ELF version chain")
        if index + 1 == version_count and next_entry != 0:
            raise ValueError("Inconsistent ELF version count")
        current += next_entry
    return {"needed": needed, "soname": None if soname is None else string(soname),
            "interpreter": interpreters[0] if interpreters else None,
            "search_paths": paths, "search_kind": "runpath" if runpath is not None else "rpath",
            "version_requirements": requirements}


def safe_name(name):
    path = PurePosixPath(name)
    if (not path.parts or "\\" in name or "\0" in name or ":" in name or path.is_absolute()
            or path.as_posix() != name or any(part in (".", "..") for part in path.parts)):
        raise ValueError(f"Unsafe Linux bundle path: {name}")
    return path


def library_search(image, value):
    if value == "$ORIGIN":
        suffix = ""
    elif value.startswith("$ORIGIN/"):
        suffix = value[len("$ORIGIN/"):]
    else:
        raise ValueError(f"Non-relocatable ELF library search path: {value}")
    if "$" in suffix or "\\" in suffix or "\0" in suffix or suffix.startswith("/"):
        raise ValueError("Unsupported ELF library search expansion")
    candidate = posixpath.normpath(str(PurePosixPath(image).parent / suffix))
    if candidate == ".." or candidate.startswith("../") or candidate.startswith("/"):
        raise ValueError("ELF library search escapes the bundle")
    return PurePosixPath(candidate)


def audit_images(images, maximum_glibc=(2, 39), host_runtime=False):
    """Validate direct dependencies against each object's own origin search path.

    Requiring each library to resolve its own imports also avoids relying on
    inherited RPATH behavior. Dynamic plugins and devices need native tests.
    """
    system_libraries = HOST_EXECUTABLE_LIBRARIES if host_runtime else SYSTEM_LIBRARIES
    report = {"schema_version": 1, "architecture": "x86_64", "images": {},
              "maximum_glibc": ".".join(map(str, maximum_glibc)), "unresolved": [],
              "coverage": "ELF direct imports; dynamic plugins, devices and execution require native acceptance"}
    for name, data in images:
        safe_name(name)
        if name in report["images"]:
            raise ValueError(f"Duplicate Linux image: {name}")
        info = elf_info(data)
        if info["interpreter"] not in (None, INTERPRETER):
            raise ValueError(f"Unexpected ELF interpreter in {name}")
        if PurePosixPath(name).name in system_libraries or info["soname"] in system_libraries:
            raise ValueError(f"Host system/driver library must not be bundled: {name}")
        info["sha256"] = hashlib.sha256(data).hexdigest()
        report["images"][name] = info
    if not report["images"]:
        raise ValueError("Linux bundle contains no ELF images")
    for name, info in report["images"].items():
        directories = [library_search(name, value) for value in info["search_paths"]]
        imports = {}
        for dependency in info["needed"]:
            if not re.fullmatch(r"[A-Za-z0-9_+.-]+", dependency):
                raise ValueError(f"Unsafe ELF import: {dependency}")
            candidates = [str(directory / dependency) for directory in directories]
            provider = next((p for p in candidates if p in report["images"]), None)
            if provider:
                if report["images"][provider]["soname"] not in (None, dependency):
                    raise ValueError(f"ELF provider SONAME mismatch: {provider}")
                imports[dependency] = {"kind": "bundled", "path": provider}
            elif dependency in system_libraries:
                imports[dependency] = {"kind": "host_system_or_graphics_driver"}
            else:
                report["unresolved"].append({"image": name, "library": dependency})
        info["dependencies"] = imports
        for requirement in info["version_requirements"]:
            version = requirement["version"]
            if not version.startswith("GLIBC_"):
                continue
            if version == "GLIBC_ABI_DT_RELR" and maximum_glibc >= (2, 36):
                continue
            match = re.fullmatch(r"GLIBC_([0-9]+(?:\.[0-9]+)+)", version)
            if not match or tuple(map(int, match[1].split("."))) > maximum_glibc:
                raise ValueError(f"ELF requires unsupported glibc version {version}: {name}")
    if report["unresolved"]:
        raise ValueError(f"Unresolved Linux imports: {report['unresolved']}")
    return report


def audit_bundle(root, host_runtime=False):
    def images():
        for path in sorted(Path(root).rglob("*")):
            if path.is_symlink():
                raise ValueError(f"Bundle symlink is not permitted: {path}")
            if path.is_file():
                with path.open("rb") as stream:
                    if stream.read(4) == b"\x7fELF":
                        yield path.relative_to(root).as_posix(), path.read_bytes()
    return audit_images(images(), host_runtime=host_runtime)


def require_host_runtime(report):
    """Fail closed: only NOH workers and the CPU speech executable ship.

    This is an ownership boundary, not an expanding driver exclusion list.
    A newly introduced ELF must receive an explicit architecture review.
    """
    allowed = {'usr/lib/noh/noh-app', 'usr/lib/noh/bin/noh-cli',
               'usr/lib/noh/bin/noh-mcp', 'usr/lib/noh/bin/speech/whisper-cli'}
    found = set()
    for name, info in report['images'].items():
        canonical = name.removeprefix('NOH.AppDir/')
        if canonical not in allowed or info['soname'] or info['search_paths']:
            raise ValueError(f'Linux host-runtime package contains a private ELF/provider: {name}')
        if any(dep['kind'] == 'bundled' for dep in info['dependencies'].values()):
            raise ValueError(f'Linux host-runtime package contains a private import: {name}')
        found.add(canonical)
    if found != allowed:
        raise ValueError(f'Linux host-runtime package is missing executables: {sorted(allowed - found)}')
