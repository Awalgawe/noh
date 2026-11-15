"""Mesa's software OpenGL beside an executable, for tests on hosted Windows runners.

A hosted runner has no OpenGL 2.0. The pinned Mesa archive supplies a software
renderer: `opengl32.dll` is only a loader and `libgallium_wgl.dll` holds the
drivers, so both are placed beside the executable or neither. They are test
material: remove them before checking what an installation leaves behind, and
never run this in a job that builds or seals a package.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "assets/software-opengl.lock.json"


def require(value, message):
    if not value:
        raise ValueError(message)


def pin(lock=LOCK):
    data = json.loads(Path(lock).read_text(encoding="utf-8"))
    mesa = data.get("mesa", {})
    require(data.get("schema") == 1 and mesa.get("algorithm") == "SHA256"
            and str(mesa.get("url", "")).startswith("https://github.com/pal1000/mesa-dist-win/releases/download/")
            and len(str(mesa.get("hash", ""))) == 64
            and mesa.get("files") == ["opengl32.dll", "libgallium_wgl.dll"]
            and mesa.get("folder") and mesa.get("name"), "Invalid software OpenGL pin")
    return mesa


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def extract(archive, destination):
    subprocess.run(["7z", "x", str(archive), "-o" + str(destination), "-y"],
                   check=True, stdout=subprocess.DEVNULL, timeout=300)


def download(url, target):
    partial = Path(str(target) + ".partial")
    with urllib.request.urlopen(url, timeout=60) as response, partial.open("wb") as output:
        shutil.copyfileobj(response, output)
    partial.replace(target)


def acquire(cache, lock=LOCK, download=download, extract=extract):
    """Return the two pinned files, downloading and extracting the archive once."""
    mesa = pin(lock)
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / mesa["name"]
    if not archive.is_file():
        download(mesa["url"], archive)
    require(digest(archive) == mesa["hash"], "Software OpenGL archive hash mismatch")
    extracted = cache / (mesa["hash"][:16] + "-extracted")
    if not extracted.is_dir():
        partial = cache / (mesa["hash"][:16] + "-partial")
        shutil.rmtree(partial, ignore_errors=True)
        extract(archive, partial)
        partial.rename(extracted)
    files = [extracted / mesa["folder"] / name for name in mesa["files"]]
    require(all(path.is_file() for path in files),
            "The software OpenGL archive lacks " + " or ".join(mesa["files"]) + ": nothing was deployed")
    return files


def deploy(files, folders):
    """Copy both files beside the executables; refuse to replace anything already there."""
    for folder in folders:
        for source in files:
            target = Path(folder) / source.name
            require(not target.exists(), "Refusing to replace " + str(target))
            shutil.copyfile(source, target)


def remove(files, folders):
    for folder in folders:
        for source in files:
            target = Path(folder) / source.name
            require(target.is_file() and digest(target) == digest(source),
                    "Software OpenGL file missing or changed: " + str(target))
            target.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("deploy", "remove"))
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("folders", type=Path, nargs="+")
    args = parser.parse_args()
    files = acquire(args.cache)
    {"deploy": deploy, "remove": remove}[args.action](files, args.folders)
    print(args.action + ": " + ", ".join(path.name for path in files) + " in "
          + ", ".join(str(folder) for folder in args.folders))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit("Software OpenGL: " + str(error))
