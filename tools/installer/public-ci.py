"""Build/test the initial public installer on a disposable GitHub Windows runner.

The qualified application is reused byte for byte. Source, wrapper and workflow
identities remain distinct. Staging never publishes or modifies the live release.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import delivery
from setup_delivery import GitHub

SOURCE = "df41ed864aef4aef4bf8df0da2460cd7173e2cf1"
ARCHIVE = "NOH-0.1.0-windows-x64.zip"
ARCHIVE_SHA = "79eae3216d45a290ce5e47ae530e28530b66c0eb1ac6d1f58fee193c7cfd3765"
MANIFEST_SHA = "27afb3ce2f57a4f3d59e6afbee6354cc50c0c37a61cdf6c0e299e9ec11a04b6d"
RELEASE_ID = 407538113
SETUP = "NOH-0.1.0-windows-x64-Setup.exe"
UNINSTALL_KEY = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NOH.Public_is1"


def require(value, message):
    if not value:
        raise ValueError(message)


def runner():
    require(os.name == "nt" and os.environ.get("GITHUB_ACTIONS") == "true"
            and os.environ.get("GITHUB_REPOSITORY_ID") == "1411000018"
            and os.environ.get("NOH_RUNNER_ENVIRONMENT") == "github-hosted",
            "This installation test requires the disposable hosted Windows runner")
    return Path(os.environ["RUNNER_TEMP"]) / "noh-public-installer"


def api(route):
    request = urllib.request.Request("https://api.github.com/repos/Awalgawe/noh" + route,
                                     headers={"User-Agent": "NOH-installer-validation"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def prepare(work):
    work.mkdir(exist_ok=False)
    delivery.authorize_redistribution("windows-x64")
    release = api("/releases/tags/v0.1.0")
    require(release["id"] == RELEASE_ID and not release["draft"] and not release["prerelease"],
            "The qualified ordinary release is missing")
    require(api("/commits/v0.1.0")["sha"] == SOURCE, "Release tag source changed")
    assets = [a for a in release["assets"] if a["name"] == ARCHIVE]
    require(len(assets) == 1 and assets[0]["digest"] == "sha256:" + ARCHIVE_SHA,
            "Qualified portable digest changed")
    archive = work / ARCHIVE
    request = urllib.request.Request(assets[0]["browser_download_url"],
                                     headers={"User-Agent": "NOH-installer-validation"})
    with urllib.request.urlopen(request, timeout=180) as response, archive.open("xb") as output:
        shutil.copyfileobj(response, output, length=1024 * 1024)
    require(delivery.digest(archive) == ARCHIVE_SHA, "Downloaded portable digest differs")
    bundle = work / "portable"
    bundle.mkdir()
    with zipfile.ZipFile(archive) as source:
        members = delivery.zip_members(source)
        source.extractall(bundle, members=members)
    require(delivery.digest(bundle / "manifest.json") == MANIFEST_SHA, "Manifest differs")
    delivery.verify_bundle(bundle, "x86_64-pc-windows-gnu", SOURCE)
    # The release's wrapper sources, including all seven languages, are unchanged.
    for name in ("tools/public-installer.py", "tools/installer/public.iss",
                 "tools/installer/public-messages.iss"):
        original = subprocess.check_output(["git", "show", SOURCE + ":" + name], cwd=ROOT)
        require(original.replace(b"\r\n", b"\n") == (ROOT / name).read_bytes().replace(b"\r\n", b"\n"),
                "The reviewed installer sources changed: " + name)
    print("Exact qualified portable and unchanged wrapper sources verified", flush=True)


def run(work, name, args, timeout=600, env=None):
    started = time.monotonic()
    with (work / (name + ".log")).open("wb") as log:
        result = subprocess.run([str(a) for a in args], cwd=work, env=env,
                                stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
    record = {"name": name, "exit": result.returncode, "seconds": round(time.monotonic() - started, 3)}
    print(json.dumps(record), flush=True)
    require(result.returncode == 0, name + " failed; inspect " + name + ".log")
    return record


def build(work):
    run(work, "compile", [sys.executable, ROOT / "tools/public-installer.py",
                         "--bundle", work / "portable", "--commit", SOURCE,
                         "--compiler", os.environ["NOH_ISCC_DIRECTORY"], "--output", work / "output"], 1200)


def registration():
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY, 0,
                            winreg.KEY_READ | winreg.KEY_WOW64_64KEY) as key:
            return winreg.QueryValueEx(key, "InstallLocation")[0]
    except FileNotFoundError:
        return None


def verify_installed(folder, expected):
    actual = delivery.inventory(folder)
    for name, spec in expected.items():
        require(actual.get(name) == spec, "Installed payload differs: " + name)
    extras = set(actual) - set(expected)
    require(extras <= {"licenses/INNO-SETUP-LICENSE.txt", "unins000.exe", "unins000.dat", "user-data.txt"},
            "Unexpected installed files: " + repr(extras))
    require((folder / "unins000.exe").is_file(), "Uninstaller missing")


def test(work):
    require(registration() is None, "Refusing to replace an existing NOH installation")
    installed = work / "installation with spaces"
    require(not installed.exists(), "Installation test directory already exists")
    expected = delivery.inventory(work / "portable")
    setup = work / "output" / SETUP
    record = delivery.read(work / "output/INSTALLER.json")
    require(delivery.digest(setup) == record["installer"]["sha256"], "Installer digest changed")
    results = []
    for stage in ("install", "reinstall"):
        results.append(run(work, stage, [setup, "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART",
                                       "/CURRENTUSER", "/NOICONS", "/LANG=fr", "/TASKS=!desktopicon",
                                       "/DIR=" + str(installed), "/LOG=" + str(work / (stage + "-inno.log"))]))
        require(registration() and Path(registration()).resolve() == installed.resolve(),
                "Per-user uninstall registration points elsewhere")
        verify_installed(installed, expected)
        marker = installed / "user-data.txt"
        if stage == "install":
            marker.write_text("Preserve user data during reinstall and uninstall.\n", encoding="utf-8")
        else:
            require(marker.read_text(encoding="utf-8") == "Preserve user data during reinstall and uninstall.\n",
                    "Reinstall changed user data")
    env = {k: v for k, v in os.environ.items() if not k.startswith("NOH_")}
    env["PATH"] = os.environ["SystemRoot"] + "/System32;" + os.environ["SystemRoot"]
    env["APPDATA"] = str(work / "profile/roaming")
    env["LOCALAPPDATA"] = str(work / "profile/local")
    for key in ("APPDATA", "LOCALAPPDATA"):
        Path(env[key]).mkdir(parents=True)
    env["NOH_CAPTURE_UI"] = str(work / "installed-gui.ppm")
    env["NOH_LANGUAGE"] = "fr"
    results.append(run(work, "installed-gui", [installed / "noh.exe"], 90, env))
    gui = delivery.read(work / "installed-gui.json")
    require(gui["capture_ready"] and not gui["timed_out"], "Installed GUI capture failed")
    results.append(run(work, "installed-cli", [installed / "bin/noh-cli.exe", "--version"], 30, env))
    results.append(run(work, "uninstall", [installed / "unins000.exe", "/VERYSILENT",
                                          "/SUPPRESSMSGBOXES", "/NORESTART",
                                          "/LOG=" + str(work / "uninstall-inno.log")], 180))
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and (registration() is not None or (installed / "unins000.exe").exists()):
        time.sleep(0.5)
    require(registration() is None, "Uninstall registry entry remains")
    remaining = delivery.inventory(installed)
    require(set(remaining) == {"user-data.txt"}, "Uninstall left application files or removed user data")
    require(delivery.digest(setup) == record["installer"]["sha256"], "Test changed installer bytes")
    report = {"status": "passed", "source_commit": SOURCE, "portable_sha256": ARCHIVE_SHA,
              "wrapper_commit": os.environ["GITHUB_SHA"], "manifest_sha256": MANIFEST_SHA,
              "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
              "installer": record["installer"], "materials": record["materials"],
              "host": platform.platform(), "results": results, "installed_files_verified": len(expected),
              "user_data_preserved": True, "uninstall_registration_removed": True,
              "limitations": ["Silent installation on a disposable hosted Windows runner; no interactive wizard acceptance.",
                              "Microsoft prerequisite download is not executed in silent mode.",
                              "GUI startup is checked; audio/GPU/media qualification is reused from the unchanged portable.",
                              "No Authenticode signing or SmartScreen reputation is claimed."]}
    delivery.write(work / "output/INSTALLER-TESTS.json", report)


def stage(work):
    report = delivery.read(work / "output/INSTALLER-TESTS.json")
    require(report["status"] == "passed" and report["wrapper_commit"] == os.environ["GITHUB_SHA"]
            and report["run_id"] == os.environ["GITHUB_RUN_ID"]
            and report["run_attempt"] == os.environ["GITHUB_RUN_ATTEMPT"], "Test provenance differs")
    delivery.authorize_redistribution("windows-x64")
    github = GitHub()
    repo = github.request("")
    require(repo["id"] == 1411000018 and repo["full_name"] == "Awalgawe/noh", "Wrong repository")
    names = [SETUP, report["materials"]["name"], "INSTALLER.json", "INSTALLER-TESTS.json"]
    for name, key in ((SETUP, "installer"), (report["materials"]["name"], "materials")):
        require(delivery.digest(work / "output" / name) == report[key]["sha256"], "Tested bytes changed")
    evidence = work / "output/installer-test-evidence.zip"
    with zipfile.ZipFile(evidence, "x", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(work.glob("*.log")) + sorted(work.glob("installed-gui.*")):
            archive.write(path, path.name)
    names.append(evidence.name)
    tag = "installer-check-v0.1.0-" + os.environ["GITHUB_RUN_ID"] + "-" + os.environ["GITHUB_RUN_ATTEMPT"]
    draft = github.request("/releases", "POST", {"tag_name": tag, "target_commitish": os.environ["GITHUB_SHA"],
        "name": "Windows installer review (unpublished)", "draft": True, "prerelease": False,
        "body": "Exact hosted installer test output; independent review pending. Do not publish this staging release."})
    assets = []
    for name in names:
        path = work / "output" / name
        asset = github.upload(draft["id"], path, name, delivery.digest(path))
        assets.append({key: asset[key] for key in ("id", "name", "digest", "size")})
    receipt = {"draft_id": draft["id"], "tag": tag, "assets": assets, "wrapper_commit": os.environ["GITHUB_SHA"]}
    print(json.dumps(receipt, indent=2), flush=True)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
        summary.write("Installer built and tested. Unpublished review draft: " + str(draft["id"]) + "\n\n")
        summary.write("```json\n" + json.dumps(receipt, indent=2) + "\n```\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("prepare", "build", "test", "stage"))
    arguments = parser.parse_args()
    globals()[arguments.stage](runner())
