"""Acquire explicit MSYS2 native files and their paired source materials."""
import hashlib
import io
from pathlib import Path, PurePosixPath
import shutil
import sys
import tarfile
import tempfile
import tomllib

import delivery
import rust_notices


def native_rust_notices(crates, notices, cache, output_name="rust-native"):
    supplements = delivery.read(delivery.ROOT / "assets/rust-notices.lock.json")
    supplements.pop("standard_library", None)
    with tempfile.TemporaryDirectory(prefix="noh-native-notices-") as temporary:
        work = Path(temporary)
        vendor = work / "vendor"
        vendor.mkdir()
        records = []
        for path in sorted(crates.glob("*.crate")):
            identity = path.stem
            folder = vendor / identity
            folder.mkdir()
            hashes = {}
            with tarfile.open(path) as tar:
                files = {}
                for member in tar.getmembers():
                    name = delivery.relative(member.name)
                    if name.parts[0] != identity:
                        raise ValueError("Unexpected native crate archive root")
                    if member.isdir():
                        continue
                    if not member.isfile():
                        raise ValueError("Native crate links/special files are not accepted")
                    if len(name.parts) < 2:
                        raise ValueError("Native crate file cannot replace its root")
                    relative = str(PurePosixPath(*name.parts[1:]))
                    if relative in files or relative == ".cargo-checksum.json":
                        raise ValueError("Duplicate/generated native crate entry")
                    files[relative] = member
                file_names = {name.casefold() for name in files}
                for name in files:
                    if any(str(parent).casefold() in file_names for parent in PurePosixPath(name).parents):
                        raise ValueError("Native crate file conflicts with a parent directory")
                manifest = tar.extractfile(files["Cargo.toml"]).read()
                package = tomllib.loads(manifest.decode("utf-8"))["package"]
                license_file = package.get("license-file")
                if license_file:
                    license_path = PurePosixPath(license_file)
                    if (license_path.is_absolute() or ".." in license_path.parts
                            or "\\" in str(license_path) or ":" in str(license_path)):
                        raise ValueError("Unsafe crate license-file path")
                    license_file = str(license_path)
                # The original .crate archives remain in the retained source
                # materials. This temporary tree serves only notice generation.
                # Hash every source member, but materialize only its consumers.
                # Complete-content correspondence still needs the entire crate.
                keep_sources = "source_correspondence" in supplements["crates"].get(identity, {})
                for relative, member in files.items():
                    data = manifest if relative == "Cargo.toml" else tar.extractfile(member).read()
                    hashes[relative] = hashlib.sha256(data).hexdigest()
                    if (keep_sources or relative in {"Cargo.toml", ".cargo_vcs_info.json", license_file}
                            or rust_notices.notice_path(PurePosixPath(relative))):
                        target = folder / relative
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(data)
            delivery.write(folder / ".cargo-checksum.json",
                           {"package": delivery.digest(path), "files": hashes})
            records.append({"name": package["name"], "version": package["version"],
                            "license": package.get("license"), "license_file": package.get("license-file"),
                            "source": "registry+https://github.com/rust-lang/crates.io-index"})
        delivery.write(work / "RUST-INVENTORY.json", records)
        report = rust_notices.generate(work, cache, supplements, delivery.fetch, delivery.ROOT)
        shutil.copytree(work / "rust-notices", notices / output_name)
        return report["missing_notice_texts"]


def archive(path):
    with path.open("rb") as source:
        magic = source.read(4)
    if magic != b"\x28\xb5\x2f\xfd":
        return tarfile.open(path)
    if sys.version_info >= (3, 14):
        return tarfile.open(path, "r:zst")
    # Local preparation can reuse the already pinned build-tool module. Hosted
    # acquisition uses Python 3.14's standard-library Zstandard decoder.
    try:
        import zstandard
    except ImportError as error:
        raise ValueError("MSYS2 acquisition needs Python 3.14 or the zstandard build module") from error
    with path.open("rb") as source, zstandard.ZstdDecompressor().stream_reader(source) as stream:
        data = stream.read(2 * delivery.LIMIT + 1)
    if len(data) > 2 * delivery.LIMIT:
        raise ValueError("Native package expansion exceeds the preparation bound")
    return tarfile.open(fileobj=io.BytesIO(data))


def source_notices(source, identity, specs, notices):
    """Retain pinned original terms from the paired upstream source archive."""
    selected = [item for item in specs if item["component"] == identity]
    if not selected:
        return
    for item in selected:
        if "repository_file" not in item:
            continue
        if delivery.digest(source) != item["source_archive_sha256"]:
            raise ValueError("Git notice source archive checksum mismatch")
        data = (delivery.ROOT / str(delivery.relative(item["repository_file"]))).read_bytes()
        if hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError("Original Git source notice checksum mismatch")
        target = notices / identity / "upstream" / str(delivery.relative(item["source_member"]))
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    selected = [item for item in selected if "repository_file" not in item]
    if not selected:
        return
    with archive(source) as outer:
        for outer_name in sorted({item["source_archive_member"] for item in selected}):
            outer_member = outer.getmember(outer_name)
            if not outer_member.isfile():
                raise ValueError("Source notice archive is not a regular member")
            with tarfile.open(fileobj=io.BytesIO(outer.extractfile(outer_member).read())) as inner:
                for item in selected:
                    if item["source_archive_member"] != outer_name:
                        continue
                    member = inner.getmember(item["source_member"])
                    if not member.isfile():
                        raise ValueError("Source notice is not a regular member")
                    data = inner.extractfile(member).read()
                    if hashlib.sha256(data).hexdigest() != item["sha256"]:
                        raise ValueError("Original source notice checksum mismatch")
                    target = notices / identity / "upstream" / str(delivery.relative(member.name))
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)


def native_toolchain_materials(specs, cache, sources, notices):
    """Keep exact producer std sources/notices, without redistributing compilers."""
    for spec in specs:
        version = str(delivery.relative(spec["version"]))
        for kind in ("binary", "standard_library_source"):
            package = spec[kind]
            path = delivery.fetch(package, cache, PurePosixPath(package["url"]).name)
            with archive(path) as tar:
                buildinfo = tar.extractfile(".BUILDINFO").read()
                if hashlib.sha256(buildinfo).hexdigest() != package["buildinfo_sha256"]:
                    raise ValueError("Native Rust BUILDINFO checksum mismatch")
                recipe = "pkgbuild_sha256sum = " + spec["recipe_sha256"]
                if recipe not in buildinfo.decode("utf-8").splitlines():
                    raise ValueError("Native Rust binary/source recipe mismatch")
                for item in package["notices"]:
                    member = tar.getmember(item["member"])
                    if not member.isfile():
                        raise ValueError("Native Rust notice is not a regular file")
                    data = tar.extractfile(member).read()
                    if hashlib.sha256(data).hexdigest() != item["sha256"]:
                        raise ValueError("Native Rust notice checksum mismatch")
                    target = notices / "rust-toolchains" / version / str(delivery.relative(member.name))
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
            if kind == "standard_library_source":
                target = sources / "rust-toolchains" / path.name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target)
        for item in spec["recipe_files"]:
            path = delivery.ROOT / str(delivery.relative(item["path"]))
            if delivery.digest(path) != item["sha256"]:
                raise ValueError("Native Rust recipe material checksum mismatch")
            target = sources / "rust-toolchains" / version / path.name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
    delivery.write(sources / "rust-toolchains/PROVENANCE.json", specs)


def standard_library_dependency_materials(lock, cache, sources, notices):
    """Cover all locked std dependencies, including unused targets and tests."""
    expected = {}
    for graph in lock["standard_library_dependency_locks"]:
        original = delivery.fetch(graph["source"], cache, PurePosixPath(graph["source"]["url"]).name)
        opener = archive(original) if original.name.endswith(".zst") else tarfile.open(original, "r|*")
        with opener as tar:
            data = None
            for member in tar:
                if member.name == graph["member"]:
                    if not member.isfile():
                        raise ValueError("Rust std Cargo.lock is not a regular member")
                    data = tar.extractfile(member).read()
                    break
        if data is None or hashlib.sha256(data).hexdigest() != graph["sha256"]:
            raise ValueError("Rust std dependency lock differs from producer source")
        saved = delivery.ROOT / str(delivery.relative(graph["path"]))
        if saved.read_bytes() != data:
            raise ValueError("Retained Rust std dependency lock changed")
        packages = {}
        for item in tomllib.loads(data.decode("utf-8"))["package"]:
            if not item.get("source"):
                continue
            if item["source"] != "registry+https://github.com/rust-lang/crates.io-index":
                raise ValueError("Unreviewed Rust std dependency source")
            identity = item["name"] + "-" + item["version"]
            packages[identity] = item["checksum"]
            if identity in expected and expected[identity] != item["checksum"]:
                raise ValueError("Conflicting Rust std dependency checksums")
            expected[identity] = item["checksum"]
        if packages != graph["packages"]:
            raise ValueError("Incomplete Rust std dependency graph")
    if expected != {name: spec["sha256"] for name, spec in lock["standard_library_crates"].items()}:
        raise ValueError("Rust std crate inventory does not cover its exact dependency graphs")
    crates = sources / "rust-toolchains/crates"
    crates.mkdir(parents=True)
    for identity, spec in lock["standard_library_crates"].items():
        path = delivery.fetch(spec, cache, str(delivery.relative(identity)) + ".crate")
        shutil.copy2(path, crates / path.name)
    gaps = native_rust_notices(crates, notices, cache, output_name="rust-standard-libraries")
    if gaps:
        raise ValueError("Missing Rust std dependency notices: " + ", ".join(gaps))
    delivery.write(sources / "rust-toolchains/DEPENDENCIES.json", lock["standard_library_dependency_locks"])


def acquire(lock, cache, destination, materials, crate_cache=None):
    """No supplier discovery: every runtime file has one pinned package/member."""
    if lock.get("schema_version") != 1 or set(lock["roles"]) != {"preview", "export"}:
        raise ValueError("Unsupported native runtime lock")
    native = destination / "native"
    native.mkdir()
    sources = materials / "native-sources"
    sources.mkdir()
    notices = materials / "native-notices"
    notices.mkdir()
    selected = {}
    for role, files in lock["roles"].items():
        for name, item in files.items():
            if PurePosixPath(name).name != name:
                raise ValueError("Expected a bare native filename")
            delivery.relative(name)
            selected.setdefault(item["package"], []).append((role, name, item))
        (native / role).mkdir()
    for identity, record in lock["packages"].items():
        delivery.relative(identity)
        binary = delivery.fetch(record["binary"], cache, PurePosixPath(record["binary"]["url"]).name)
        with archive(binary) as tar:
            members = {}
            for member in tar.getmembers():
                delivery.relative(member.name.rstrip("/"))
                if member.name in members:
                    raise ValueError("Duplicate native package member")
                members[member.name] = member
            for role, name, item in selected.get(identity, []):
                member = members[item["member"]]
                if (not member.name.startswith("mingw64/bin/") or
                        PurePosixPath(member.name).name.casefold() != name.casefold()):
                    raise ValueError("Native member identity mismatch")
                if not member.isfile():
                    raise ValueError("Linked/special native runtime member")
                data = tar.extractfile(member).read()
                if hashlib.sha256(data).hexdigest() != item["sha256"]:
                    raise ValueError("Native runtime file checksum mismatch: " + name)
                (native / role / name).write_bytes(data)
            for member in members.values():
                prefix = "mingw64/share/licenses/"
                if member.name.startswith(prefix) and member.isfile():
                    target = notices / identity / str(delivery.relative(member.name[len(prefix):]))
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(tar.extractfile(member).read())
        source = delivery.fetch(record["source"], cache, PurePosixPath(record["source"]["url"]).name)
        shutil.copy2(source, sources / source.name)
        source_notices(source, identity, lock["source_notices"], notices)
    crates = sources / "native-rust-crates"
    crates.mkdir()
    for identity, spec in lock["native_cargo"]["crates"].items():
        path = delivery.fetch(spec, crate_cache or cache, identity + ".crate")
        shutil.copy2(path, crates / path.name)
    gaps = native_rust_notices(crates, notices, cache)
    native_toolchain_materials(lock["native_rust_toolchains"], cache, sources, notices)
    standard_library_dependency_materials(lock, cache, sources, notices)
    delivery.write(sources / "INPUTS.json", lock)
    delivery.write(notices / "PROVENANCE.json", {
        "status": "unreviewed-original-package-notices", "provider": "MSYS2",
        "native_lock_sha256": delivery.digest(delivery.ROOT / "assets/windows-native.lock.json"),
        "limits": lock["unresolved"], "native_rust_original_text_gaps": gaps,
        "files": delivery.inventory(notices)})
    delivery.validate_ffmpeg(native / "export/ffmpeg.exe")
    return native
