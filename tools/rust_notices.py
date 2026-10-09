"""Preserve original Rust notices; do not infer license clearance from SPDX labels."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import tomllib

NOTICE = re.compile(r"(^|[._-])(licen[sc]e|copying|notice|copyright|authors)([._-]|$)", re.I)


def notice_path(path):
    return (bool(NOTICE.search(path.name)) or path.name.lower() in {"ofl.txt", "ufl.txt"}
            or ("fonts" in path.parts and path.suffix == ".txt"))


def inventory_vendor(vendor, lock_file):
    """Inventory every locked source package, independently of active Cargo features."""
    packages = tomllib.loads(lock_file.read_text(encoding="utf-8"))["package"]
    locked = {(p["name"], p["version"]): p for p in packages if p.get("source")}
    records = []
    found = set()
    for folder in sorted(vendor.iterdir()):
        if not folder.is_dir() or folder.is_symlink():
            raise ValueError("Unexpected vendor entry")
        package = tomllib.loads((folder / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        identity = (package["name"], package["version"])
        if identity not in locked or identity in found or folder.name != "-".join(identity):
            raise ValueError("Vendor package does not match the lockfile")
        checksums = json.loads((folder / ".cargo-checksum.json").read_text(encoding="utf-8"))
        if checksums["package"] != locked[identity].get("checksum"):
            raise ValueError("Vendor crate archive checksum differs from the lockfile")
        found.add(identity)
        records.append({"name": identity[0], "version": identity[1], "license": package.get("license"),
                        "source": locked[identity]["source"], "license_file": package.get("license-file")})
    if found != set(locked):
        raise ValueError("Locked source packages are missing from vendor")
    return records


def source_correspondence(directory, checksums, supplement, evidence_root, output):
    """Check a complete content match when old crates omit VCS metadata.

    The reviewed lock binds these Git blobs to the stated upstream commit. This
    proves matching published contents, not the historical publication revision.
    """
    tree_spec = supplement["source_tree"]
    tree_path = PurePosixPath(tree_spec["path"])
    if (tree_path.is_absolute() or ".." in tree_path.parts or "\\" in str(tree_path)
            or ":" in str(tree_path) or not str(tree_path).startswith("assets/rust-notice-evidence/")):
        raise ValueError("Unsafe supplement source tree path")
    tree_data = (evidence_root / str(tree_path)).read_bytes()
    if hashlib.sha256(tree_data).hexdigest() != tree_spec["sha256"]:
        raise ValueError("Pinned source tree evidence changed")
    tree = json.loads(tree_data)
    expected_url = (supplement["repository"].replace("https://github.com/", "https://api.github.com/repos/")
                    + "/git/trees/" + supplement["revision"])
    if tree.get("truncated") or tree.get("url") != expected_url:
        raise ValueError("Source tree evidence targets a different revision")
    blobs = {entry["path"]: entry["sha"] for entry in tree["tree"] if entry["type"] == "blob"}
    files = supplement["source_correspondence"]
    expected = {name for name in checksums["files"]
                if name not in {"Cargo.toml", ".cargo_vcs_info.json"}}
    if not expected or len(files) != len(expected) or {item["path"] for item in files} != expected:
        raise ValueError("Supplement source correspondence omits published files")
    for item in files:
        path = PurePosixPath(item["path"])
        upstream = "Cargo.toml" if str(path) == "Cargo.toml.orig" else str(path)
        if (path.is_absolute() or ".." in path.parts or "\\" in str(path) or ":" in str(path)
                or item["upstream_path"] != upstream or blobs.get(upstream) != item["git_blob"]):
            raise ValueError("Unsafe supplement source correspondence path")
        source = directory / str(path)
        if source.is_symlink():
            raise ValueError("Source correspondence symlinks are not accepted")
        data = source.read_bytes()
        sha256 = hashlib.sha256(data).hexdigest()
        blob = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if sha256 != item["sha256"] or sha256 != checksums["files"][str(path)] or blob != item["git_blob"]:
            raise ValueError("Published source differs from pinned upstream correspondence")
    if any(blobs.get(item["path"]) != item["git_blob"] for item in supplement["files"]):
        raise ValueError("Notice differs from pinned source tree")
    saved = output / "source-correspondence" / tree_path.name
    saved.parent.mkdir(exist_ok=True)
    saved.write_bytes(tree_data)
    return {"method": "complete-published-content-match", "revision": supplement["revision"],
            "source_tree": {"path": saved.relative_to(output).as_posix(), "sha256": tree_spec["sha256"],
                            "original_text": tree_data.decode("utf-8")},
            "files": files, "limitation": "Matching contents do not identify the historical publication commit."}


def standard_library_notices(spec, output, sysroot=None, compiler=None):
    """Preserve the generated library notice shipped in the exact rustc component."""
    if compiler is None:
        compiler = subprocess.run(["rustc", "-Vv"], check=True, capture_output=True, text=True).stdout
    details = dict(line.split(": ", 1) for line in compiler.splitlines() if ": " in line)
    if details.get("release") != spec["version"] or details.get("commit-hash") != spec["source_commit"]:
        raise ValueError("Standard-library notices require the pinned Rust release/commit")
    if sysroot is None:
        sysroot = Path(subprocess.run(["rustc", "--print", "sysroot"], check=True, capture_output=True, text=True).stdout.strip())
    source = sysroot / "share/doc/rust/COPYRIGHT-library.html"
    manifest = sysroot / ("lib/rustlib/manifest-rustc-" + details["host"])
    if "file:share/doc/rust/COPYRIGHT-library.html" not in manifest.read_text(encoding="utf-8").splitlines():
        raise ValueError("Standard-library notice is not recorded in the installed rustc component")
    data = source.read_bytes()
    if hashlib.sha256(data).hexdigest() != spec["sha256"]:
        raise ValueError("Standard-library notice differs from the pinned original")
    name = "RUST-STANDARD-LIBRARY-COPYRIGHT.html"
    (output / name).write_bytes(data)
    return {"status": "unreviewed-original-standard-library-notice", "version": spec["version"],
            "source_commit": spec["source_commit"], "host": details["host"],
            "component_manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
            "path": name, "sha256": spec["sha256"], "origin": "installed-rustc-component",
            "limitations": ["Original generated notices do not select license alternatives or clear redistribution.",
                            "Native target/component correspondence still requires independent review."]}


def generate(materials, cache, supplements, fetcher, evidence_root=None):
    """Cover the entire vendor graph, including target/build/test-only crates."""
    crates = json.loads((materials / "RUST-INVENTORY.json").read_text(encoding="utf-8"))
    identities = {c["name"] + "-" + c["version"] for c in crates}
    if len(identities) != len(crates) or identities != {p.name for p in (materials / "vendor").iterdir()}:
        raise ValueError("Rust notice inventory does not cover the exact vendor graph")
    output = materials / "rust-notices"
    output.mkdir(exist_ok=False)
    records = []
    missing = []
    original_missing = []
    text = ["NOH Rust dependency notices\nUnreviewed original notice materials.\n"
            "Coverage: entire locked vendor graph, including crates not linked on this platform.\n"
            "Rust standard-library/toolchain and native runtime materials are recorded separately.\n"
            "Explicit per-archive declaration supplements record selected alternatives; other expressions remain unselected.\n"
            "Standard texts are distinguished from original copyright notices. Redistribution still requires review.\n\n"]
    for crate in sorted(crates, key=lambda c: (c["name"], c["version"])):
        identity = crate["name"] + "-" + crate["version"]
        if not re.fullmatch(r"[A-Za-z0-9_.+\-]+", identity):
            raise ValueError("Unsafe crate identity")
        directory = materials / "vendor" / identity
        checksums = json.loads((directory / ".cargo-checksum.json").read_text(encoding="utf-8"))
        texts = []
        paths = [p for p in directory.rglob("*") if p.is_file() and notice_path(p.relative_to(directory))]
        if crate.get("license_file"):
            path = PurePosixPath(crate["license_file"])
            if path.is_absolute() or ".." in path.parts or "\\" in str(path) or ":" in str(path):
                raise ValueError("Unsafe crate license-file path")
            if directory / str(path) not in paths:
                paths.append(directory / str(path))
        for path in sorted(paths):
            if path.is_symlink():
                raise ValueError("Notice symlinks are not accepted")
            name = path.relative_to(directory).as_posix()
            data = path.read_bytes()
            checksum = hashlib.sha256(data).hexdigest()
            if checksums["files"].get(name) != checksum:
                raise ValueError("Vendor notice checksum mismatch: " + identity + "/" + name)
            texts.append(({"path": name, "sha256": checksum, "origin": "vendored-crate"}, data))
        supplement = supplements["crates"].get(identity)
        correspondence = None
        if supplement:
            if checksums["package"] != supplement["package_sha256"]:
                raise ValueError("Supplement targets a different crate archive: " + identity)
            if not re.fullmatch(r"[0-9a-f]{40}", supplement["revision"]):
                raise ValueError("Supplement requires an immutable upstream revision")
            if "source_correspondence" in supplement:
                correspondence = source_correspondence(directory, checksums, supplement,
                                                       evidence_root or Path(__file__).resolve().parent.parent, output)
            else:
                vcs = json.loads((directory / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
                if vcs["git"]["sha1"] != supplement["revision"]:
                    raise ValueError("Supplement targets a different upstream revision: " + identity)
            for item in supplement["files"]:
                expected_url = (supplement["repository"].replace("https://github.com/", "https://raw.githubusercontent.com/")
                                + "/" + supplement["revision"] + "/")
                if not item["url"].startswith(expected_url):
                    raise ValueError("Supplement URL does not identify its exact upstream revision")
                path = fetcher(item, cache, "rust-notice-" + item["sha256"] + ".txt")
                data = path.read_bytes()
                blob = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
                if blob != item["git_blob"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
                    raise ValueError("Upstream notice blob mismatch")
                texts.append((item | {"origin": "pinned-upstream", "revision": supplement["revision"]}, data))
        if not texts:
            original_missing.append(identity)
        declaration = supplements.get("declared_license_supplements", {}).get(identity)
        if declaration:
            # A bounded, per-archive decision, never an automatic SPDX fallback.
            # Preserve the publisher's actual declaration and attribution, and
            # identify the standard text as such rather than inventing an original.
            manifest = (directory / "Cargo.toml").read_bytes()
            package = tomllib.loads(manifest.decode("utf-8"))["package"]
            choices = re.split(r"\s+OR\s+|\s*/\s*", package.get("license", ""))
            if (checksums["package"] != declaration["package_sha256"]
                    or hashlib.sha256(manifest).hexdigest() != checksums["files"]["Cargo.toml"]
                    or package.get("license") != declaration["declared_license"]
                    or package.get("authors", []) != declaration["authors"]
                    or declaration["selected_license"] not in choices):
                raise ValueError("Declared-license supplement does not match published metadata: " + identity)
            spec = declaration["text"]
            relative = PurePosixPath(spec["path"])
            if (relative.is_absolute() or ".." in relative.parts or "\\" in str(relative)
                    or ":" in str(relative) or relative.parts[:2] != ("assets", "license-texts")):
                raise ValueError("Unsafe standard-license text path")
            data = ((evidence_root or Path(__file__).resolve().parent.parent) / str(relative)).read_bytes()
            if hashlib.sha256(data).hexdigest() != spec["sha256"]:
                raise ValueError("Standard-license text changed")
            texts.extend([
                ({"path": "Cargo.toml", "sha256": hashlib.sha256(manifest).hexdigest(),
                  "origin": "published-license-declaration-and-authors"}, manifest),
                (spec | {"origin": "standard-license-text", "selected_license": declaration["selected_license"],
                         "basis": declaration["basis"]}, data),
            ])
        if not texts:
            missing.append(identity)
        text.append("=" * 72 + "\n" + identity + "\nDeclared license: " + str(crate["license"])
                    + "\nRegistry source: " + crate["source"] + "\n")
        files = []
        for spec, data in texts:
            # Decode strictly so a corrupt/non-text input cannot silently lose notice bytes.
            original = data.decode("utf-8")
            text.append("\n--- " + spec["path"] + " (" + spec["origin"] + ") ---\n" + original + "\n")
            files.append(spec)
        if not texts:
            text.append("MISSING ORIGINAL NOTICE TEXT; maintainer resolution required.\n")
        record = crate | {"package_sha256": checksums["package"], "notice_files": files}
        if declaration:
            record["declared_license_supplement"] = declaration
        if correspondence:
            record["source_correspondence"] = correspondence
        if supplement and supplement.get("source_revision_limit"):
            record["source_revision_limit"] = supplement["source_revision_limit"]
        records.append(record)
    notice = output / "RUST-NOTICES.txt"
    notice.write_text("".join(text), encoding="utf-8", newline="\n")
    report = {"schema_version": 1, "status": "unreviewed-notice-materials", "coverage": "entire-vendor-graph",
              "scope_limitations": ["The Cargo inventory excludes the Rust standard library/toolchain and native runtimes; any standard-library notice is recorded separately.",
                                    "Preserved texts do not attest license-expression choices or all distribution obligations."],
              "notice_sha256": hashlib.sha256(notice.read_bytes()).hexdigest(),
              "missing_notice_texts": missing, "missing_original_notice_texts": original_missing,
              "crates": records, "supplement_unresolved": supplements["unresolved"]}
    if "standard_library" in supplements:
        report["standard_library"] = standard_library_notices(supplements["standard_library"], output)
    (output / "RUST-NOTICE-INVENTORY.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report
