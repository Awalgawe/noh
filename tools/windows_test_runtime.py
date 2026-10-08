"""Prepare only the locked export backend for disposable Windows CI tests."""
import argparse
import hashlib
from pathlib import Path, PurePosixPath

import delivery
from windows_runtime import archive


def acquire(lock, cache, output):
    files = lock["roles"]["export"]
    if len({name.casefold() for name in files}) != len(files):
        raise ValueError("Duplicate Windows test runtime filename")
    selected = {}
    for name, item in files.items():
        if len(delivery.relative(name).parts) != 1:
            raise ValueError("Expected a bare test runtime filename")
        selected.setdefault(item["package"], []).append((name, item))
    output.mkdir(parents=True)
    for identity, items in selected.items():
        spec = lock["packages"][identity]["binary"]
        package = delivery.fetch(spec, cache, PurePosixPath(spec["url"]).name)
        with archive(package) as tar:
            members = {}
            for member in tar.getmembers():
                delivery.relative(member.name.rstrip("/"))
                if member.name in members:
                    raise ValueError("Duplicate test runtime package member")
                members[member.name] = member
            for name, item in items:
                member = members[item["member"]]
                if (not member.isfile() or not member.name.startswith("mingw64/bin/")
                        or PurePosixPath(member.name).name.casefold() != name.casefold()):
                    raise ValueError("Test runtime member must be the exact regular bin file")
                data = tar.extractfile(member).read()
                if hashlib.sha256(data).hexdigest() != item["sha256"]:
                    raise ValueError("Test runtime file checksum mismatch: " + name)
                (output / name).write_bytes(data)
    report = delivery.audit_pe_bundle(output)
    delivery.require_pe_closure(report)
    delivery.write(output / "PE-IMPORTS.json", report)
    delivery.validate_ffmpeg(output / "ffmpeg.exe")
    return output / "ffmpeg.exe"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    spec = delivery.read(delivery.ROOT / "assets/delivery-windows.lock.json")["native_runtime"]
    path = delivery.ROOT / str(delivery.relative(spec["lock"]))
    if delivery.digest(path) != spec["sha256"]:
        raise ValueError("Native runtime lock checksum mismatch")
    executable = acquire(delivery.read(path), args.cache, args.output)
    print("Verified locked CI FFmpeg: " + str(executable))


if __name__ == "__main__":
    main()
