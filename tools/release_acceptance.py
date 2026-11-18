#!/usr/bin/env python3
"""Accept a published pre-release from the files a user downloads.

Each platform job downloads the release files anonymously, checks them against the
published `SHA256SUMS`, exercises them and writes one part. `collect` binds the parts
to the digests GitHub reports for the release and writes `ACCEPTANCE.json`; `attach`
adds that single file to the release. Nothing else on the release is touched.
"""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import delivery
import mcp_check
import release_publication as publication

DOWNLOADS = "https://github.com/" + publication.REPOSITORY + "/releases/download/"
TAG = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+")
# A rehearsal may read an existing pre-release whose tag carries a suffix; its files keep the plain version.
REHEARSAL_TAG = re.compile(r"v([0-9]+\.[0-9]+\.[0-9]+)(-[a-z]+\.[0-9]+)?")
EFFECT = "noh.breath"


def require(value, message):
    if not value:
        raise ValueError(message)


def parse_checksums(text):
    """`SHA256SUMS` as name -> digest; refuses anything that is not `<64 hex>  <safe name>`."""
    entries = {}
    for line in text.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        require(match and match.group(2) not in entries and match.group(2) != "SHA256SUMS",
                "Malformed or duplicate SHA256SUMS line")
        entries[match.group(2)] = match.group(1)
    require(entries, "Empty SHA256SUMS")
    return entries


def fetch(url, target):
    """Anonymous download, as a user's browser would do it: no token is ever sent."""
    require(url.startswith(DOWNLOADS), "Unexpected download address")
    partial = Path(str(target) + ".partial")
    with urllib.request.urlopen(url, timeout=120) as response, partial.open("wb") as output:
        shutil.copyfileobj(response, output, length=1024 * 1024)
    partial.replace(target)


def download_release_files(tag, names, folder, fetch=fetch, rehearsal=False):
    """Download `SHA256SUMS` and the named files; return name -> digest for everything downloaded."""
    require((REHEARSAL_TAG if rehearsal else TAG).fullmatch(tag), "A final version tag is required")
    folder.mkdir(parents=True, exist_ok=False)
    fetch(DOWNLOADS + tag + "/SHA256SUMS", folder / "SHA256SUMS")
    sums = parse_checksums((folder / "SHA256SUMS").read_text(encoding="utf-8"))
    downloaded = {"SHA256SUMS": delivery.digest(folder / "SHA256SUMS")}
    for name in names:
        require(name in sums, "The release checksums do not list " + name)
        fetch(DOWNLOADS + tag + "/" + name, folder / name)
        downloaded[name] = delivery.digest(folder / name)
        require(downloaded[name] == sums[name], "Downloaded file differs from SHA256SUMS: " + name)
    return downloaded


def linux_envelope(downloads, version, envelope):
    """Rebuild the delivery folder the build sealed, from the published files."""
    envelope.mkdir()
    for name in (f"NOH-{version}-linux-x64.zip", f"NOH-{version}-linux-x64-materials.zip"):
        shutil.copy2(downloads / name, envelope / name)
    shutil.copy2(downloads / "linux-x64-DELIVERY.json", envelope / "DELIVERY.json")
    (envelope / "SHA256SUMS").write_text("".join(
        f"{delivery.digest(p)}  {p.name}\n" for p in sorted(envelope.iterdir()) if p.is_file()), encoding="utf-8")
    record = delivery.verify_envelope(envelope)
    require(record["platform"] == "linux-x64" and record["version"] == version, "Published Linux delivery differs")
    return record


def burn_with_effect(cli, ffmpeg, work, environment, prefix=()):
    """Burn one caption with a non-Classic effect: it has no libass fallback, so OpenGL must draw it."""
    work.mkdir()
    source, captions, output = work / "source.mp4", work / "captions.srt", work / "captioned.mp4"
    subprocess.run([str(ffmpeg), "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=s=1280x720:r=25",
                    "-f", "lavfi", "-i", "sine=frequency=440", "-t", "3", "-pix_fmt", "yuv420p", str(source)],
                   check=True, timeout=120)
    captions.write_text("1\n00:00:00,200 --> 00:00:02,800\nAcceptance of the published package\n", encoding="utf-8")
    with (work / "burn.log").open("wb") as log:
        result = subprocess.run([*prefix, str(cli), "--burn-subtitles", str(source), "--srt", str(captions),
                                 "--output", str(output), "--effect", EFFECT, "--json"],
                                env=environment, stdout=log, stderr=subprocess.STDOUT, timeout=600)
    require(result.returncode == 0 and output.is_file() and output.stat().st_size > 10000,
            "The caption effect burn failed; see burn.log")
    return {"effect": EFFECT, "exit": result.returncode, "output_bytes": output.stat().st_size,
            "output_sha256": delivery.digest(output)}


def installed_mcp_job(bin_folder, work):
    """One MCP job through an installed noh-mcp.exe, started as an MCP client starts it.

    Its environment holds only the Windows system folders on PATH, so the installed copy must find the
    FFmpeg installed beside it. The same FFmpeg makes the media and decodes the export.
    """
    work.mkdir()
    ffmpeg = bin_folder / "ffmpeg.exe"
    clip, soundtrack, output = work / "synthetic.mp4", work / "silence.wav", work / "mcp-composition.mp4"
    for command in ([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=128x72:rate=12:duration=3",
                     "-c:v", "libx264", "-pix_fmt", "yuv420p", "-n", clip],
                    [ffmpeg, "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                     "-t", "3", "-ac", "2", "-c:a", "pcm_s16le", "-n", soundtrack]):
        subprocess.run([str(part) for part in command], check=True, timeout=120)
    result = mcp_check.mcp_job([bin_folder / "noh-mcp.exe"], clip, soundtrack, output)
    subprocess.run([str(ffmpeg), "-v", "error", "-i", str(output), "-f", "null", "-"], check=True, timeout=120)
    result["export_sha256"] = delivery.digest(output)
    return result


def linux(args):
    """Accept the published Linux files: the whole of it on one machine, or one runner's share with --runner."""
    require((REHEARSAL_TAG if args.rehearsal else TAG).fullmatch(args.tag), "A final version tag is required")
    require(args.runner is None or args.runner in linux_runners(), "Unknown Linux acceptance runner")
    version = REHEARSAL_TAG.fullmatch(args.tag).group(1)
    work = args.output
    names = [f"NOH-{version}-linux-x64.zip", f"NOH-{version}-linux-x64-materials.zip", "linux-x64-DELIVERY.json"]
    downloaded = download_release_files(args.tag, names, work / "downloads", rehearsal=args.rehearsal)
    record = linux_envelope(work / "downloads", version, work / "envelope")
    evidence = work / "evidence"
    evidence.mkdir(parents=True)
    checks = {}
    if args.runner != EFFECT_BURN_RUNNER:
        command = [sys.executable, str(ROOT / "tools/linux_matrix.py"), "--engine", "docker",
                   "--envelope", str(work / "envelope"), "--speech-fixture", str(args.speech_fixture),
                   "--output", str(evidence / "linux-matrix")]
        if args.runner:
            command += ["--cases", args.runner]
        subprocess.run(command, check=True, timeout=3 * 3600)
        checks["distribution_matrix"] = ({args.runner: "passed; results under linux-matrix/"} if args.runner
                                         else "passed; results under linux-matrix/")
    if args.runner in (None, EFFECT_BURN_RUNNER):
        extracted = work / "package"
        extracted.mkdir()
        with zipfile.ZipFile(work / "downloads" / names[0]) as archive:
            members = delivery.zip_members(archive)
            archive.extractall(extracted, members=members)
            for member in members:
                if not member.is_dir():
                    (extracted / member.filename).chmod((member.external_attr >> 16) & 0o777)
        home = extracted / "NOH.AppDir/usr/lib/noh"
        environment = dict(os.environ, LIBGL_ALWAYS_SOFTWARE="1")
        checks["caption_effect_burn"] = burn_with_effect(home / "bin/noh-cli", shutil.which("ffmpeg"),
                                                         evidence / "effect-burn", environment,
                                                         prefix=("xvfb-run", "-a"))
    part = {"platform": "linux-x64", "status": "passed", "tag": args.tag, "rehearsal": args.rehearsal,
            "downloads": downloaded, "exercised": [names[0]],
            "delivery_sha256": delivery.digest(work / "envelope/DELIVERY.json"),
            "source_commit": record["source_commit"], "checks": checks, "limitations": LINUX_LIMITATIONS,
            "run_attempt": run_attempt()}
    if args.runner:
        part["runner"] = args.runner
    delivery.write(evidence / "part.json", part)


def merge_linux_runners(runners):
    """One Linux part from the runners' parts: every runner present, all of them on the same published files."""
    require(set(runners) == set(linux_runners()), "Linux acceptance lacks a runner, or has an unknown one")
    first = runners[EFFECT_BURN_RUNNER]
    same = ("platform", "status", "tag", "rehearsal", "downloads", "exercised", "delivery_sha256", "source_commit")
    for name, part in runners.items():
        require(all(part.get(key) == first.get(key) for key in same),
                "Linux acceptance runners tested different files: " + name)
    matrix = {}
    for name, part in runners.items():
        if name != EFFECT_BURN_RUNNER:
            require(set(part["checks"]) == {"distribution_matrix"} and set(part["checks"]["distribution_matrix"]) == {name},
                    "A distribution runner reported another case: " + name)
            matrix.update(part["checks"]["distribution_matrix"])
    require(set(first["checks"]) == {"caption_effect_burn"}, "The effect burn runner reported other checks")
    merged = {key: first[key] for key in same}
    merged["checks"] = {"distribution_matrix": matrix, "caption_effect_burn": first["checks"]["caption_effect_burn"]}
    merged["runners"] = sorted(runners)
    merged["limitations"] = LINUX_LIMITATIONS + ["Each distribution and the effect burn ran on its own runner, "
                                                 "each downloading and checking the published files itself."]
    return merged


EFFECT_BURN_RUNNER = "effect-burn"
LINUX_LIMITATIONS = ["The distribution matrix and the effect burn use software rendering and virtual audio.",
                     "The effect burn uses the runner's FFmpeg and libmpv packages, as the package expects "
                     "the host to provide them."]


def linux_runners():
    """The Linux acceptance split across runners: one per distribution of the matrix, one for the effect burn."""
    from linux_matrix import cases
    return [case["id"] for case in cases()] + [EFFECT_BURN_RUNNER]


def installer_layout(report, downloads, base):
    """Place the downloaded installers where the installer tests expect them; the Web installer
    gets no component archive beside it, so it must download them from the release."""
    records = report["artifacts"]
    for profile in ("web", "minimal", "standard", "complete"):
        item = records[profile]["installer"]
        source = downloads / item["name"]
        require(delivery.digest(source) == item["sha256"] and source.stat().st_size == int(item["size"]),
                "A published installer is not the one the installer tests recorded: " + item["name"])
        (base / "output" / profile).mkdir(parents=True)
        shutil.copy2(source, base / "output" / profile / item["name"])
    return records


def windows(args):
    """Install every profile from the published Web installer, upgrade the last version by a real
    download, start the interface and burn an effect. No token exists in this job."""
    import importlib.util
    import software_opengl
    spec = importlib.util.spec_from_file_location("public_ci", ROOT / "tools/installer/public-ci.py")
    ci = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ci)
    require(os.name == "nt" and os.environ.get("GITHUB_ACTIONS") == "true"
            and not any(os.environ.get(name) for name in ("GH_TOKEN", "GITHUB_TOKEN")),
            "Windows acceptance runs on a disposable hosted runner, without any token")
    require(ci.registration() is None, "Refusing to replace an existing NOH installation")
    version, work = args.tag[1:], args.output
    names = [f"NOH-{version}-windows-x64-{profile}-Setup.exe" for profile in ("Web", "Minimal", "Standard", "Complete")]
    portable_name = f"NOH-{version}-windows-x64.zip"
    downloaded = download_release_files(args.tag, names + [portable_name, "INSTALLER-TESTS.json", "COMPONENTS.json"],
                                        work / "downloads")
    sums = parse_checksums((work / "downloads/SHA256SUMS").read_text(encoding="utf-8"))
    report = delivery.read(work / "downloads/INSTALLER-TESTS.json")
    catalog = delivery.read(work / "downloads/COMPONENTS.json")
    require(report["version"] == version and report["component_catalog_sha256"] == downloaded["COMPONENTS.json"],
            "The published installer tests or catalog describe another version")
    fetched_by_installer = {}
    for group in catalog["groups"].values():
        require(sums.get(group["name"]) == group["sha256"], "A component archive differs from the catalog: " + group["name"])
        fetched_by_installer[group["name"]] = group["sha256"]
    base = work / "tests"
    records = installer_layout(report, work / "downloads", base)
    web = base / "output/web" / records["web"]["installer"]["name"]
    opengl = software_opengl.acquire(Path(os.environ["RUNNER_TEMP"]) / "noh-software-opengl")
    profiles = [ci.test_profile(base / ("test-" + profile), profile, records, web, opengl) for profile in ci.PROFILES]

    # The portable package: extract it and start its interface and command line.
    portable = base / "test-portable"
    (portable / "package").mkdir(parents=True)
    with zipfile.ZipFile(work / "downloads" / portable_name) as archive:
        archive.extractall(portable / "package", members=delivery.zip_members(archive))
    delivery.verify_bundle(portable / "package", "x86_64-pc-windows-gnu", report["source_commit"])
    portable_results = ci.start_installed(portable, portable / "package", opengl)

    # Upgrade by a real download: the previous Complete version, then the published Minimal installer
    # with no component archive beside it.
    baseline = ci.upgrade_baseline()
    upgrade = base / "test-upgrade"
    upgrade.mkdir()
    previous = upgrade / baseline["name"]
    fetch(DOWNLOADS + baseline["tag"] + "/" + baseline["name"], previous)
    require(delivery.digest(previous) == baseline["sha256"] and previous.stat().st_size == baseline["size"],
            "The upgrade baseline is not the pinned published installer")
    installed = upgrade / "installation with spaces"
    common = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER", "/NOICONS",
              "/TASKS=!desktopicon", "/DIR=" + str(installed)]
    results = [ci.run(upgrade, "install-previous-complete",
                      [previous, *common, "/LOG=" + str(upgrade / "install-previous-complete-inno.log")], 1200)]
    require(ci.registration() and Path(ci.registration()).resolve() == installed.resolve(),
            "The previous version registered another folder")
    marker = installed / "user-data.txt"
    marker.write_text("User data must survive an upgrade.\n", encoding="utf-8")
    minimal = base / "output/minimal" / records["minimal"]["installer"]["name"]
    results.append(ci.run(upgrade, "upgrade-with-minimal-download",
                          [minimal, "/PROFILE=minimal", *common,
                           "/LOG=" + str(upgrade / "upgrade-with-minimal-download-inno.log")], 2400))
    require(ci.registration() and Path(ci.registration()).resolve() == installed.resolve(),
            "The upgrade registered another folder")
    ci.verify_installed(installed, records["minimal"]["profile_files"]["complete"])
    require(marker.read_text(encoding="utf-8") == "User data must survive an upgrade.\n", "The upgrade altered user data")
    results.extend(ci.start_installed(upgrade, installed, opengl))
    environment = {k: v for k, v in os.environ.items() if not k.startswith("NOH_")}
    environment["GALLIUM_DRIVER"] = "llvmpipe"
    software_opengl.deploy(opengl, [installed / "bin"])
    try:
        burn = burn_with_effect(installed / "bin/noh-cli.exe", installed / "bin/ffmpeg.exe",
                                upgrade / "effect-burn", environment)
    finally:
        software_opengl.remove(opengl, [installed / "bin"])
    mcp = installed_mcp_job(installed / "bin", upgrade / "mcp")
    ci.verify_installed(installed, records["minimal"]["profile_files"]["complete"])
    results.append(ci.remove_test_installation(upgrade, installed))
    evidence = work / "evidence"
    evidence.mkdir()
    with zipfile.ZipFile(evidence / "windows-acceptance-evidence.zip", "x", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(base.rglob("*.log")) + sorted(base.glob("test-*/installed-gui.*")):
            archive.write(path, path.relative_to(base).as_posix())
    delivery.write(evidence / "part.json", {
        "platform": "windows-x64", "status": "passed", "tag": args.tag, "rehearsal": False, "downloads": downloaded,
        "run_attempt": run_attempt(),
        "fetched_by_installer": fetched_by_installer,
        "exercised": names + [portable_name] + sorted(fetched_by_installer),
        "checks": {"profiles_from_the_web_installer": profiles,
                   "portable_package_started": portable_results,
                   "upgrade_by_download": {"from": baseline["tag"], "results": results,
                                           "complete_components_retained": True, "user_data_preserved": True},
                   "caption_effect_burn": burn,
                   "mcp_job_on_the_installed_copy": mcp},
        "limitations": ["Silent installation only; the interactive wizard is not exercised.",
                        "The interface and the effect burn use Mesa's software OpenGL placed beside the "
                        "executables for the test; the renderer was not read back.",
                        "The previous version is installed but never run before the upgrade.",
                        "The component archives were downloaded and checked by the installers themselves; the "
                        "test checked the files they installed, not the archives.",
                        "Microsoft prerequisite download is not executed in silent mode."]})


def run_attempt():
    """The attempt of the workflow run writing a part; 1 outside GitHub Actions."""
    value = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
    require(re.fullmatch(r"[1-9][0-9]*", value) is not None, "Unexpected run attempt: " + value)
    return int(value)


def latest_parts(folder):
    """Each platform's part, or each Linux runner's part, from the latest attempt that wrote one.

    After "re-run failed jobs", a job keeps the artifact of the attempt that ran it, and the
    collecting job of a later attempt also sees the artifacts of earlier ones: a runner may thus
    appear in several attempts, and its latest part is the one that counts. The same part twice
    in one attempt is refused. That download-artifact's pattern returns earlier attempts'
    artifacts in a re-run is how GitHub keeps artifacts per run; it is not exercised by a test here.
    """
    chosen, seen = {}, set()
    for path in sorted(folder.glob("*/part.json")):
        part = delivery.read(path)
        key, attempt = (part.get("platform"), part.get("runner")), part.get("run_attempt", 1)
        require(isinstance(attempt, int) and not isinstance(attempt, bool) and attempt >= 1,
                "Unusable run attempt in " + str(path))
        require((key, attempt) not in seen, "The same acceptance part twice in one attempt: " + str(path))
        seen.add((key, attempt))
        if key not in chosen or attempt > chosen[key][0]:
            chosen[key] = (attempt, path, part)
    return [(path, part) for _, path, part in sorted(chosen.values(), key=lambda item: str(item[1]))]


def collect(args):
    """Bind the platform parts to the files the publishing run verified, and write ACCEPTANCE.json."""
    from setup_delivery import GitHub
    require(TAG.fullmatch(args.tag), "A final version tag is required")
    github = GitHub()
    release = github.request("/releases/tags/" + args.tag)
    require(not release["draft"] and release["prerelease"], "Only a published pre-release is accepted")
    scratch = Path(os.environ["RUNNER_TEMP"]) / "noh-release-acceptance"
    scratch.mkdir(exist_ok=False)
    verified = publication.verified_publication(github, release, int(os.environ["NOH_DELIVERY_WORKFLOW_ID"]),
                                                scratch)["assets"]
    parts, exercised, linux_runners = {}, {}, {}
    for path, part in latest_parts(args.parts):
        require(part.get("platform") in publication.PLATFORMS and part["platform"] not in parts
                and part.get("status") == "passed" and part.get("tag") == args.tag and not part.get("rehearsal"),
                "Unusable acceptance part: " + str(path))
        tested = {**part["downloads"], **part.get("fetched_by_installer", {})}
        for name, digest in tested.items():
            require(verified.get(name) == "sha256:" + digest, "A tested file is not the one that was published: " + name)
        for name in part["exercised"]:
            require(name in tested, "A part claims a file it did not check: " + name)
            exercised[name] = "sha256:" + tested[name]
        if part["platform"] == "linux-x64" and "runner" in part:
            require(part["runner"] not in linux_runners, "Duplicate Linux acceptance runner: " + str(path))
            linux_runners[part["runner"]] = part
        else:
            parts[part["platform"]] = part
    if linux_runners:
        require("linux-x64" not in parts, "Linux was accepted both whole and by runner")
        parts["linux-x64"] = merge_linux_runners(linux_runners)
    require(set(parts) == set(publication.PLATFORMS), "Both platforms must have been accepted")
    delivery.write(args.output, {
        "schema_version": 1, "status": "passed", "tag": args.tag, "release_id": release["id"],
        "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "workflow_commit": os.environ["GITHUB_SHA"],
        "release_files": verified, "exercised_files": exercised, "platforms": parts,
        "limitations": ["Hosted runners have no hardware GPU, audio device, NVENC or CUDA.",
                        "Nobody looked at a rendered picture; captures and exports are kept as evidence.",
                        "Silent installation only; the interactive wizard is not exercised.",
                        "The Linux desktop screens kept with the release were reviewed by nobody.",
                        "release_files lists every published file; only exercised_files were installed or run."]})


def attach(args):
    """Add ACCEPTANCE.json to the published pre-release; a re-run replaces that one file only."""
    from setup_delivery import GitHub, validate_asset
    github = GitHub()
    release = github.request("/releases/tags/" + args.tag)
    require(not release["draft"] and release["prerelease"], "Only a published pre-release is accepted")
    report = delivery.read(args.report)
    require(report["tag"] == args.tag and report["release_id"] == release["id"] and report["status"] == "passed",
            "The report does not describe this release")
    for asset in release["assets"]:
        if asset["name"] == "ACCEPTANCE.json":
            github.request("/releases/assets/" + str(asset["id"]), "DELETE")
    size = args.report.stat().st_size
    headers = dict(github.headers, **{"Content-Type": "application/json", "Content-Length": str(size)})
    url = (f"https://uploads.github.com/repos/{publication.REPOSITORY}/releases/{release['id']}"
           "/assets?name=ACCEPTANCE.json")
    with args.report.open("rb") as source:
        with github.opener.open(urllib.request.Request(url, data=source, headers=headers, method="POST"),
                                timeout=300) as response:
            uploaded = delivery.json.load(response)
    validate_asset(uploaded, {"name": "ACCEPTANCE.json", "sha256": delivery.digest(args.report), "size": size})
    after = {a["name"]: a["digest"] for a in github.request("/releases/" + str(release["id"]))["assets"]
             if a["name"] not in ("ACCEPTANCE.json", "PUBLICATION.json")}
    require(after == report["release_files"], "Another release file changed during acceptance")
    print("ACCEPTANCE.json attached to " + args.tag)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    on_linux = commands.add_parser("linux")
    on_linux.add_argument("--tag", required=True)
    on_linux.add_argument("--speech-fixture", type=Path, required=True)
    on_linux.add_argument("--output", type=Path, required=True)
    on_linux.add_argument("--rehearsal", action="store_true",
                          help="Accept a suffixed pre-release tag; the part is marked and cannot be collected")
    on_linux.add_argument("--runner", help="One runner's share: a distribution of the matrix, or effect-burn")
    on_windows = commands.add_parser("windows")
    on_windows.add_argument("--tag", required=True)
    on_windows.add_argument("--output", type=Path, required=True)
    collecting = commands.add_parser("collect")
    collecting.add_argument("--tag", required=True)
    collecting.add_argument("--parts", type=Path, required=True)
    collecting.add_argument("--output", type=Path, required=True)
    attaching = commands.add_parser("attach")
    attaching.add_argument("--tag", required=True)
    attaching.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    {"linux": linux, "windows": windows, "collect": collect, "attach": attach}[args.command](args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        raise SystemExit(f"Release acceptance refused: {error}") from error
