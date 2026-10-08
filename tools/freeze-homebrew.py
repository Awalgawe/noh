#!/usr/bin/env python3
"""Prepare intermediate Homebrew inputs; source enrichment/review is required."""
import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import tarfile
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-archive", type=Path, required=True)
    parser.add_argument("--core-commit", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with tarfile.open(args.core_archive) as archive:
        recipes = {m.name.split("/", 1)[1]: archive.extractfile(m).read()
                   for m in archive.getmembers() if m.isfile() and "/Formula/" in m.name}
    frozen = {}

    def fetch(name):
        with urllib.request.urlopen(f"https://formulae.brew.sh/api/formula/{name}.json", timeout=60) as response:
            data = json.load(response)
        path = data["ruby_source_path"]
        recipe = recipes[path]
        checksum = hashlib.sha256(recipe).hexdigest()
        if checksum != data["ruby_source_checksum"]["sha256"]:
            raise ValueError(f"API/formula snapshot mismatch for {name}; freeze a fresh core snapshot")
        bottles = data["bottle"]["stable"]["files"]
        bottle = bottles.get("arm64_tahoe", bottles.get("all"))
        if not bottle:
            raise ValueError(f"No macOS 26 arm64 bottle: {name}")
        if bottle["sha256"].encode() not in recipe:
            raise ValueError(f"Bottle is not in the frozen recipe: {name}")
        source = data["urls"]["stable"]
        if not source.get("checksum") and not source.get("revision"):
            raise ValueError(f"No checksum-pinned source archive: {name}")
        return name, {"version": data["versions"]["stable"], "revision": data["revision"],
                      "license": data["license"], "recipe": path, "recipe_sha256": checksum,
                      "bottle": bottle, "source": source, "patches": data["patches"],
                      "dependencies": data["dependencies"], "build_dependencies": data["build_dependencies"]}

    pending = {"ffmpeg@7", "mpv"}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        while pending:
            results = list(pool.map(fetch, sorted(pending)))
            frozen.update(results)
            pending = {dep for _, data in results for dep in data["dependencies"]} - frozen.keys()
    # The public API does not expose all stable resources or local/DATA patches.
    # Never present this partial inventory as usable source acquisition inputs.
    lock = {"schema_version": 1, "status": "incomplete-source-inputs",
            "source_enrichment_required": "Add stable resources and SHA-256 for local/DATA patches from the exact recipes before collection",
            "bottle_tag": "arm64_tahoe", "core_commit": args.core_commit,
            "core": {"url": f"https://github.com/Homebrew/homebrew-core/archive/{args.core_commit}.tar.gz",
                     "sha256": hashlib.sha256(args.core_archive.read_bytes()).hexdigest()},
            "formulae": dict(sorted(frozen.items()))}
    args.output.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    print(f"Frozen {len(frozen)} intermediate formulae; source enrichment is required before acquisition")


if __name__ == "__main__":
    main()
