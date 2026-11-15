#!/usr/bin/env python3
"""Publish an exact hosted build as a pre-release, then promote it once accepted.

`publish` gathers the bytes of one successful Release build run and of the
installer stage that wrapped them, checks every file against a digest recorded by
those runs, and publishes them unchanged under the final tag as a pre-release.
`promote` turns that same release into the latest version once the acceptance run
attached its report. Nothing is rebuilt, replaced or re-tagged: a defect means the
next version number.
"""
import argparse
import importlib.util
import os
from pathlib import Path
import re
import shutil
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import delivery
from setup_delivery import GitHub, REPOSITORY

spec = importlib.util.spec_from_file_location("delivery_release", ROOT / "tools/delivery-release.py")
provenance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provenance)

REPOSITORY_ID = 1411000018
PLATFORMS = ("windows-x64", "linux-x64")
PROFILES = ("web", "minimal", "standard", "complete")
PENDING = "**Pre-release: the acceptance tests of these exact files have not passed yet.**"
ACCEPTED = "**Accepted: the acceptance tests passed on these exact files.**"
SHA = re.compile(r"[0-9a-f]{64}")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


def require(value, message):
    if not value:
        raise ValueError(message)


def installer_draft_tag(version, run, attempt):
    return f"profile-installer-check-v{version}-{run}-{attempt}"


def evidence_artifact_name(run, attempt):
    return f"installer-evidence-{run}-{attempt}"


def acceptance_artifact_name(run, attempt):
    return f"acceptance-{run}-{attempt}"


def publication_artifact_name(run, attempt):
    return f"publication-{run}-{attempt}"


def desktop_artifact_name(case, run, attempt):
    return f"linux-desktop-{case}-{run}-{attempt}"


def validate_report(report, identity):
    """The installer test report must describe this build, this installer run and a full pass."""
    expected = {"status": "passed", "source_commit": identity["commit"],
                "source_run_id": str(identity["build_run"]), "source_run_attempt": str(identity["build_attempt"]),
                "run_id": str(identity["installer_run"]), "run_attempt": str(identity["installer_attempt"]),
                "wrapper_commit": identity["wrapper_commit"], "version": identity["version"],
                "portable_sha256": identity["portable_sha256"], "delivery_sha256": identity["delivery_sha256"]}
    for key, value in expected.items():
        require(report.get(key) == value, "Installer tests do not describe this build: " + key)
    profiles = {entry.get("profile"): entry for entry in report.get("profiles", [])}
    require(set(profiles) == {"minimal", "standard", "complete"}
            and all(entry.get("status") == "passed" and entry.get("installed_gui_passed") is True
                    for entry in profiles.values()), "An installer profile did not pass with its interface check")
    require(report.get("additions", {}).get("status") == "passed", "Component additions did not pass")
    require(report.get("upgrade", {}).get("status") == "passed", "The upgrade from the last version did not pass")


def installer_assets(report, catalog):
    """Published installer files and their recorded digests: name -> {sha256, size}."""
    artifacts = report["artifacts"]
    require(set(artifacts) == set(PROFILES) | {"maintenance"}, "Unexpected installer profiles")
    assets = {}

    def add(item):
        name, sha256, size = item.get("name"), item.get("sha256"), item.get("size")
        require(isinstance(name, str) and re.fullmatch(r"[A-Za-z0-9._-]+", name) and SHA.fullmatch(str(sha256))
                and str(size).isdigit() and name not in assets, "Invalid or duplicate installer asset record")
        assets[name] = {"sha256": sha256, "size": int(size)}

    for profile in PROFILES:
        add(artifacts[profile]["installer"])
        add(artifacts[profile]["materials"])
    # The maintenance helper is embedded in every installer; only its sources are published.
    add(artifacts["maintenance"]["materials"])
    require(catalog.get("version") == report["version"] and catalog.get("source_commit") == report["source_commit"]
            and set(catalog.get("groups", {})) == {"media", "speech"}, "Component catalog differs from this build")
    for group in catalog["groups"].values():
        add(group)
    return assets


def installer_provenance(report, identity):
    return {"schema_version": 1, "version": identity["version"], "source_commit": identity["commit"],
            "build_run": identity["build_run"], "build_attempt": identity["build_attempt"],
            "installer_run": identity["installer_run"], "installer_attempt": identity["installer_attempt"],
            "wrapper_commit": identity["wrapper_commit"],
            "portable_manifest_sha256": report["manifest_sha256"],
            "component_catalog_sha256": report["component_catalog_sha256"],
            "profiles": {name: {key: record[key] for key in ("compiler", "installer", "materials") if key in record}
                         for name, record in sorted(report["artifacts"].items())},
            "validation": {"hosted_installer_tests": report["status"],
                           "upgrade_from": report["upgrade"].get("from"),
                           "upgrade": report["upgrade"]["status"]},
            "limits": list(report.get("limitations", []))}


def checksums(folder):
    """One line per file, sorted by name; the list never names itself."""
    names = sorted(p.name for p in Path(folder).iterdir() if p.name != "SHA256SUMS")
    return "".join(f"{delivery.digest(Path(folder) / name)}  {name}\n" for name in names)


def changelog_section(text, version):
    """The changelog entry of this version, without its heading."""
    lines = text.splitlines()
    starts = [i for i, line in enumerate(lines) if line.startswith("## ")]
    matches = [i for i in starts if re.fullmatch(r"## " + re.escape(version) + r"(\s.*)?", lines[i])]
    require(len(matches) == 1, "The changelog has no single entry for " + version)
    end = next((i for i in starts if i > matches[0]), len(lines))
    section = "\n".join(lines[matches[0] + 1:end]).strip()
    # An entry may open with a paragraph sending the reader to the release's downloads. In the
    # release's own notes that paragraph would point to the page it is on: leave it out.
    opening, _, rest = section.partition("\n\n")
    if opening.startswith("Download "):
        section = rest.strip()
    require(section and "unreleased" not in lines[matches[0]].lower(),
            "The changelog entry of " + version + " is empty or still marked unreleased")
    return section


def desktop_rerun_lines(identity):
    """Say which desktops passed only on a later attempt of the run: a re-run must leave a trace."""
    again = {case: n for case, n in sorted(identity.get("desktop_case_attempts", {}).items()) if n > 1}
    if not again:
        return []
    return ["- These desktops failed first and passed when re-run in the same run: "
            + ", ".join(f"{case} (attempt {n})" for case, n in again.items())
            + ". See the known issues under Changes."]


def changes_source_lines(identity):
    """Say so when the changes were written after the build: the tagged tree then holds an older text."""
    commit = identity.get("changes_commit")
    if not commit or commit == identity["commit"]:
        return []
    return [f"This text was corrected after the build and is read from commit `{commit}`; "
            f"the changelog inside the tagged tree is older.", ""]


def release_notes(version, identity, limits, changes):
    runs = "https://github.com/" + REPOSITORY + "/actions/runs/"
    return "\n".join([
        PENDING, "",
        f"Windows and Linux packages of NOH {version}, built once by [run {identity['build_run']}]"
        f"({runs}{identity['build_run']}), attempt {identity['build_attempt']}, from commit `{identity['commit']}`. "
        f"The Windows installers wrap those same bytes ([run {identity['installer_run']}]"
        f"({runs}{identity['installer_run']}), attempt {identity['installer_attempt']}).", "",
        f"**Install**: follow the [installation guide](https://github.com/{REPOSITORY}/blob/v{version}/docs/INSTALLING.md). "
        "On Windows, start with the Web installer. On Linux, install your distribution's FFmpeg and libmpv "
        "packages first; the baseline is Ubuntu 24.04 with glibc 2.39.", "",
        "### What the hosted runs checked", "",
        "- The build ran the test suite and, for Linux, the distribution matrix on the exact package.",
        "- The installer stage installed every profile, added and repaired components, upgraded the last "
        "published version, started the installed interface and uninstalled.",
        f"- [Run {identity['desktop_run']}]({runs}{identity['desktop_run']}) started the exact Linux package on "
        f"{identity['desktop_cases']} desktops in virtual machines.",
        *desktop_rerun_lines(identity),
        "- Each file here matches a digest recorded by those runs. `SHA256SUMS` lists them.", "",
        "### Limits", "",
        *["- " + limit for limit in limits],
        "- No hardware GPU, audio device, NVENC or CUDA is exercised on hosted runners.",
        "- The Linux desktop checks ran in virtual machines; their screens are kept as evidence and were "
        "reviewed by nobody.", "",
        "### Files", "",
        "- `NOH-" + version + "-windows-x64-Web-Setup.exe`: guided Windows installation. The Minimal, Standard "
        "and Complete installers work offline.",
        "- `NOH-" + version + "-windows-x64.zip`, `NOH-" + version + "-linux-x64.zip`: portable packages.",
        "- `-materials.zip` and `-installer-sources.zip`: corresponding sources, notices and build materials; "
        "not application downloads.",
        "- `INSTALLER-TESTS.json`, `installer-test-evidence.zip`, `linux-desktop-evidence.zip`, `ACCEPTANCE.json` "
        "(once attached): what was observed.",
        "- `PUBLICATION.json`: the digests of these files as the publishing run verified them.", "",
        "### Changes", "", *changes_source_lines(identity), changes, ""])


def download_artifact(github, artifact, scratch):
    """Download and extract one Actions artifact, bound to the digest GitHub recorded for it."""
    require(not artifact["expired"] and re.fullmatch("sha256:[0-9a-f]{64}", artifact.get("digest") or ""),
            "Expired or unhashed artifact: " + artifact["name"])
    archive = scratch / (str(artifact["id"]) + ".zip")
    github.download("/actions/artifacts/" + str(artifact["id"]) + "/zip", archive,
                    artifact["digest"].removeprefix("sha256:"))
    extracted = scratch / str(artifact["id"])
    extracted.mkdir()
    with zipfile.ZipFile(archive) as bundle:
        members = delivery.zip_members(bundle)
        require(sum(m.file_size for m in members) <= 5 * delivery.LIMIT, "Artifact expansion exceeds the limit")
        bundle.extractall(extracted, members=members)
    archive.unlink()
    return extracted


def all_releases(github):
    """Every release and draft, across pages."""
    releases, page = [], 1
    while True:
        batch = github.request(f"/releases?per_page=100&page={page}")
        releases.extend(batch)
        if len(batch) < 100:
            return releases
        page += 1
        require(page <= 50, "Unbounded release listing")


def release_run(github, run_id, attempt, workflow, successful=True):
    """A completed, manually dispatched Release run on main; returns the run record.

    `successful=False` is for the publishing run only: what makes it trustworthy is its artifact,
    which exists only when its verification passed. A step that failed after the release became
    public must not make the published files unverifiable.
    """
    run = github.request("/actions/runs/" + str(run_id))
    require(run["repository"]["full_name"] == REPOSITORY and run["head_repository"]["full_name"] == REPOSITORY
            and run["workflow_id"] == workflow and run["path"] == ".github/workflows/release.yml"
            and run["head_branch"] == "main" and run["event"] == "workflow_dispatch"
            and run["status"] == "completed" and (run["conclusion"] == "success" or not successful)
            and run["run_attempt"] == attempt, "Not a completed Release run on main: " + str(run_id))
    require(github.request("/compare/" + run["head_sha"] + "...main")["status"] in ("ahead", "identical"),
            "The run's commit is not retained on main")
    return run


def one_artifact(github, run_id, name):
    data = github.request("/actions/runs/" + str(run_id) + "/artifacts?per_page=100")
    matches = [a for a in data["artifacts"] if a["name"] == name]
    require(data["total_count"] == len(data["artifacts"]) and len(matches) == 1, "Missing or ambiguous artifact: " + name)
    return matches[0]


def gather_build(github, identity, workflow, scratch, output):
    """Copy the unchanged packages of the build run into `output`; return their records."""
    run = github.request("/actions/runs/" + str(identity["build_run"]))
    provenance.validate_run(run, REPOSITORY, workflow, identity["commit"], identity["build_attempt"])
    require(github.request("/compare/" + identity["commit"] + "...main")["status"] in ("ahead", "identical"),
            "Source commit is not retained on main")
    route = "/actions/runs/" + str(identity["build_run"])
    provenance.validate_jobs(github.request(route + "/attempts/" + str(identity["build_attempt"]) + "/jobs?per_page=100"),
                             PLATFORMS, run["path"])
    suffix = f"-{identity['build_run']}-{identity['build_attempt']}"
    artifacts = provenance.validate_artifacts(github.request(route + "/artifacts?per_page=100"), PLATFORMS,
                                              "rust-audit" + suffix, "linux-validation" + suffix)
    records = {}
    for artifact in artifacts:
        extracted = download_artifact(github, artifact, scratch)
        record = delivery.verify_envelope(extracted)
        delivery.authorize_redistribution(record["platform"])
        require(record["platform"] in PLATFORMS and artifact["name"] == "delivery-" + record["platform"]
                and record["platform"] not in records and record["source_commit"] == identity["commit"]
                and record["run_id"] == str(identity["build_run"])
                and record["run_attempt"] == str(identity["build_attempt"]), "Candidate identity differs")
        for name in record["assets"]:
            require(not (output / name).exists(), "Duplicate release filename")
            shutil.move(extracted / name, output / name)
        shutil.move(extracted / "DELIVERY.json", output / (record["platform"] + "-DELIVERY.json"))
        records[record["platform"]] = record
    require(set(records) == set(PLATFORMS) and len({r["version"] for r in records.values()}) == 1,
            "The build does not hold one version for Windows and Linux")
    return records


def gather_installers(github, identity, workflow, scratch, output):
    """Copy the installer stage's files into `output`, each checked against its recorded digest."""
    run = release_run(github, identity["installer_run"], identity["installer_attempt"], workflow)
    identity["wrapper_commit"] = run["head_sha"]
    jobs = github.request("/actions/runs/" + str(identity["installer_run"]) + "/attempts/"
                          + str(identity["installer_attempt"]) + "/jobs?per_page=100")
    require(any(job["name"] == "installer / public-installer" and job["conclusion"] == "success"
                for job in jobs["jobs"]), "The installer stage did not succeed in that run")
    evidence = download_artifact(github, one_artifact(
        github, identity["installer_run"],
        evidence_artifact_name(identity["installer_run"], identity["installer_attempt"])), scratch)
    require({p.name for p in evidence.iterdir()} == {"INSTALLER-TESTS.json", "installer-test-evidence.zip"},
            "Unexpected installer evidence")
    report = delivery.read(evidence / "INSTALLER-TESTS.json")
    validate_report(report, identity)
    tag = installer_draft_tag(identity["version"], identity["installer_run"], identity["installer_attempt"])
    drafts = [r for r in all_releases(github) if r["tag_name"] == tag]
    require(len(drafts) == 1 and drafts[0]["draft"] and drafts[0]["published_at"] is None,
            "Missing or ambiguous installer staging draft: " + tag)
    staged = {asset["name"]: asset for asset in drafts[0]["assets"]}
    require("COMPONENTS.json" in staged, "The staging draft has no component catalog")
    catalog_path = scratch / "COMPONENTS.json"
    github.download("/releases/assets/" + str(staged["COMPONENTS.json"]["id"]), catalog_path,
                    report["component_catalog_sha256"])
    expected = installer_assets(report, delivery.read(catalog_path))
    for name, item in sorted(expected.items()):
        require(name in staged and not (output / name).exists(), "Missing or duplicate staged installer file: " + name)
        github.download("/releases/assets/" + str(staged[name]["id"]), output / name, item["sha256"], item["size"])
    shutil.copy2(catalog_path, output / "COMPONENTS.json")
    for name in ("INSTALLER-TESTS.json", "installer-test-evidence.zip"):
        shutil.copy2(evidence / name, output / name)
    return report


def desktop_cases():
    matrix = delivery.read(ROOT / "assets/linux-validation-matrix.json")
    cases = [row["id"] for row in matrix["required_desktops"]]
    require(cases and len(set(cases)) == len(cases), "Invalid required desktop list")
    return cases


def validate_desktop_candidate(candidate, identity):
    expected = {"source_commit": identity["commit"], "run_id": identity["build_run"],
                "run_attempt": identity["build_attempt"], "delivery_sha256": identity["linux_delivery_sha256"]}
    for key, value in expected.items():
        require(candidate.get(key) == value, "A desktop case tested another Linux package: " + key)


def desktop_case_passed(folder):
    """A desktop artifact keeps what the guest wrote under runtime/: its case.json lists the checks."""
    from linux_matrix import CHECKS
    record = folder / "runtime" / "case.json"
    return record.is_file() and delivery.read(record).get("checks") == {check: "passed" for check in CHECKS}


def gather_desktops(github, identity, scratch, output):
    """Every required desktop must have started this exact Linux package in one successful run on main."""
    run = github.request("/actions/runs/" + str(identity["desktop_run"]))
    require(run["repository"]["full_name"] == REPOSITORY and run["head_repository"]["full_name"] == REPOSITORY
            and run["path"] == ".github/workflows/linux-desktop-probe.yml" and run["head_branch"] == "main"
            and run["event"] == "workflow_dispatch" and run["status"] == "completed"
            and run["conclusion"] == "success", "Not a successful Linux desktop run on main")
    require(github.request("/compare/" + run["head_sha"] + "...main")["status"] in ("ahead", "identical"),
            "The desktop run's commit is not retained on main")
    attempt, cases, taken = run["run_attempt"], desktop_cases(), {}
    inventory = github.request("/actions/runs/" + str(identity["desktop_run"]) + "/artifacts?per_page=100")
    require(inventory["total_count"] == len(inventory["artifacts"]), "Incomplete desktop artifact listing")
    with zipfile.ZipFile(output / "linux-desktop-evidence.zip", "x", zipfile.ZIP_DEFLATED) as evidence:
        for case in cases:
            # After "re-run failed jobs" a case keeps the artifact of the attempt that ran it: take its
            # latest one. What binds it to the build is the content of CANDIDATE.json, not the name.
            pattern = re.compile(re.escape(f"linux-desktop-{case}-{identity['desktop_run']}-") + "([1-9][0-9]*)")
            found = sorted((int(pattern.fullmatch(a["name"]).group(1)), a) for a in inventory["artifacts"]
                           if pattern.fullmatch(a["name"]))
            require(found and found[-1][0] <= attempt, "No desktop result for " + case)
            folder, taken[case] = download_artifact(github, found[-1][1], scratch), found[-1][0]
            validate_desktop_candidate(delivery.read(folder / "CANDIDATE.json"), identity)
            require(desktop_case_passed(folder), "A desktop case did not pass every check: " + case)
            for path in sorted(p for p in folder.rglob("*") if p.is_file()):
                evidence.write(path, case + "/" + path.relative_to(folder).as_posix())
            shutil.rmtree(folder)
    identity.update(desktop_attempt=attempt, desktop_cases=len(cases), desktop_case_attempts=taken)


def publication_record(identity, release_id, files):
    """What this run verified and uploaded: the reference later stages compare the release with."""
    return {"schema_version": 1, "tag": "v" + identity["version"], "release_id": release_id,
            "source_commit": identity["commit"],
            "build_run": identity["build_run"], "build_attempt": identity["build_attempt"],
            "installer_run": identity["installer_run"], "installer_attempt": identity["installer_attempt"],
            "desktop_run": identity["desktop_run"], "desktop_attempt": identity["desktop_attempt"],
            "desktop_case_attempts": identity["desktop_case_attempts"],
            "changes_commit": identity["changes_commit"],
            "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
            "assets": {path.name: "sha256:" + delivery.digest(path) for path in files}}


def verified_publication(github, release, workflow, scratch):
    """The release's files must still be the ones its publishing run verified.

    `PUBLICATION.json` on the release only names that run. It is believed once the run is a
    successful Release run on main that holds the same bytes as an artifact. Release files can
    be replaced after publication; this is what ties them back to the build.
    """
    attached = [a for a in release["assets"] if a["name"] == "PUBLICATION.json"]
    require(len(attached) == 1, "The release has no single publication record")
    claimed = scratch / "attached-PUBLICATION.json"
    github.download("/releases/assets/" + str(attached[0]["id"]), claimed, attached[0]["digest"].removeprefix("sha256:"))
    claim = delivery.read(claimed)
    run_id, attempt = str(claim.get("run_id")), str(claim.get("run_attempt"))
    require(re.fullmatch("[1-9][0-9]*", run_id) and re.fullmatch("[1-9][0-9]*", attempt), "The publication record names no run")
    release_run(github, int(run_id), int(attempt), workflow, successful=False)
    evidence = download_artifact(github, one_artifact(github, run_id, publication_artifact_name(run_id, attempt)), scratch)
    require(delivery.digest(evidence / "PUBLICATION.json") == delivery.digest(claimed),
            "The attached publication record is not the one that run produced")
    record = delivery.read(evidence / "PUBLICATION.json")
    current = {a["name"]: a["digest"] for a in release["assets"] if a["name"] not in ("PUBLICATION.json", "ACCEPTANCE.json")}
    require(record.get("schema_version") == 1 and record.get("tag") == release["tag_name"]
            and record.get("release_id") == release["id"] and record.get("assets") == current,
            "A release file is not the one the publishing run verified")
    return record


def publish(args):
    require(os.environ.get("GITHUB_REF") == "refs/heads/main", "Publication runs from main")
    workflow = int(os.environ["NOH_DELIVERY_WORKFLOW_ID"])
    github = GitHub()
    repository = github.request("")
    require(repository["id"] == REPOSITORY_ID and repository["full_name"] == REPOSITORY, "Wrong repository")
    identity = {"commit": args.commit, "build_run": args.build_run, "build_attempt": args.build_attempt,
                "installer_run": args.installer_run, "installer_attempt": args.installer_attempt,
                "desktop_run": args.desktop_run}
    require(delivery.COMMIT.fullmatch(args.commit) and min(args.build_run, args.build_attempt, args.installer_run,
                                                           args.installer_attempt, args.desktop_run) >= 1,
            "Exact commit, runs and attempts are required")
    scratch = Path(os.environ["RUNNER_TEMP"]) / "noh-release-publication"
    scratch.mkdir(exist_ok=False)
    output = scratch / "release"
    output.mkdir()
    records = gather_build(github, identity, workflow, scratch, output)
    version = records["windows-x64"]["version"]
    require(VERSION.fullmatch(version), "Unexpected version: " + version)
    print("Build verified: both packages match their recorded digests, version " + version, flush=True)
    identity.update(version=version,
                    portable_sha256=delivery.digest(output / f"NOH-{version}-windows-x64.zip"),
                    delivery_sha256=delivery.digest(output / "windows-x64-DELIVERY.json"),
                    linux_delivery_sha256=delivery.digest(output / "linux-x64-DELIVERY.json"))
    report = gather_installers(github, identity, workflow, scratch, output)
    print("Installers verified: the staged installers match the tested report", flush=True)
    gather_desktops(github, identity, scratch, output)
    print(f"Desktops verified: {identity['desktop_cases']} cases passed, attempts {identity['desktop_case_attempts']}",
          flush=True)
    final_tag = "v" + version
    # A rehearsal draft never carries the real tag: publishing it by mistake could not create v<version>.
    tag = f"rehearsal-v{version}-{os.environ['GITHUB_RUN_ID']}" if args.rehearse else final_tag
    title = ("REHEARSAL, never publish: " if args.rehearse else "") + "NOH " + version
    require(not [r for r in github.request("/git/matching-refs/tags/" + final_tag)
                 if r["ref"] == "refs/tags/" + final_tag], "Tag already exists: " + final_tag)
    existing = [r for r in all_releases(github) if r["tag_name"] == tag]
    # A run killed after creating its draft leaves it behind: take it over when it is recognisably ours.
    require(all(r["draft"] and r["published_at"] is None and r["target_commitish"] == args.commit
                and r["name"] == title for r in existing) and len(existing) <= 1,
            "A release or another draft already uses " + tag + ": inspect it; delete it only if it is an unpublished "
            "draft left by a failed publish run")
    # The changelog is not in the packages. Its entry is read from this checkout, so that a
    # correction made on main after the build reaches the notes without building again; the notes
    # then say which commit the text comes from. The built commit must be part of that history.
    git = lambda *command: provenance.subprocess.check_output(["git", *command], cwd=ROOT).decode("utf-8").strip()
    text_commit = git("rev-parse", "HEAD")
    require(provenance.subprocess.run(["git", "merge-base", "--is-ancestor", args.commit, text_commit],
                                      cwd=ROOT).returncode == 0,
            "This checkout does not descend from the built commit: its changelog cannot describe that build")
    identity["changes_commit"] = text_commit
    text = (ROOT / "docs/CHANGELOG.md").read_text(encoding="utf-8")
    try:
        changes = changelog_section(text, version)
    except ValueError as error:
        require(args.rehearse, str(error))
        changes = "REHEARSAL WARNING: " + str(error)
        print(changes, flush=True)
    delivery.write(output / "INSTALLER-PROVENANCE.json", installer_provenance(report, identity))
    (output / "SHA256SUMS").write_text(checksums(output), encoding="utf-8")
    files = sorted(output.iterdir())
    require(all(p.is_file() and p.stat().st_size < delivery.LIMIT for p in files), "Unpublishable release entry")
    body = release_notes(version, identity, report.get("limitations", []), changes)
    draft = existing[0] if existing else github.request("/releases", "POST", {
        "tag_name": tag, "target_commitish": args.commit, "name": title, "draft": True, "prerelease": True, "body": body})
    record_path = scratch / "PUBLICATION.json"
    try:
        # A draft taken over from a failed run still holds that run's record: it is replaced by ours.
        for stale in [a for a in draft.get("assets", []) if a["name"] == "PUBLICATION.json"]:
            github.request("/releases/assets/" + str(stale["id"]), "DELETE")
        expected = publication_record(identity, draft["id"], files)
        delivery.write(record_path, expected)
        print(f"Draft {draft['id']} created under {tag}; uploading {len(files) + 1} files", flush=True)
        for path in files + [record_path]:
            github.upload(draft["id"], path, path.name, delivery.digest(path))
        uploaded = {a["name"]: a["digest"] for a in github.request("/releases/" + str(draft["id"]))["assets"]}
        require(uploaded == {**expected["assets"], "PUBLICATION.json": "sha256:" + delivery.digest(record_path)},
                "The uploaded files differ from the verified ones")
        github.request("/releases/" + str(draft["id"]), "PATCH", {"body": body})
    except BaseException:
        # Nothing is public yet and a draft creates no tag: remove it so that the stage can run again.
        github.request("/releases/" + str(draft["id"]), "DELETE")
        raise
    if args.rehearse:
        github.request("/releases/" + str(draft["id"]), "DELETE")
        for name in (tag, final_tag):
            require(not [r for r in github.request("/git/matching-refs/tags/" + name) if r["ref"] == "refs/tags/" + name],
                    "The rehearsal left a tag behind: " + name)
        print(f"Rehearsal complete: {len(files) + 1} files verified, uploaded to a draft and read back; "
              "the draft was deleted and nothing was published.")
        return
    print(f"Staged {final_tag} as an unpublished draft with {len(files) + 1} verified files. The finalize step "
          "publishes it once the publication record is kept as this run's artifact.")


def finalize(args):
    """Publish the staged draft. It runs only after the publication record became an artifact of this
    run: later stages need that artifact, so nothing irreversible may happen before it exists."""
    require(os.environ.get("GITHUB_REF") == "refs/heads/main", "Publication runs from main")
    github = GitHub()
    require(github.request("")["id"] == REPOSITORY_ID, "Wrong repository")
    record = delivery.read(args.record)
    tag = record["tag"]
    require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) and record["run_id"] == os.environ["GITHUB_RUN_ID"]
            and record["run_attempt"] == os.environ["GITHUB_RUN_ATTEMPT"], "The record is not this run's")
    draft = github.request("/releases/" + str(record["release_id"]))
    require(draft["draft"] and draft["published_at"] is None and draft["tag_name"] == tag
            and draft["target_commitish"] == record["source_commit"], "The staged draft changed or is already published")
    require({a["name"]: a["digest"] for a in draft["assets"]}
            == {**record["assets"], "PUBLICATION.json": "sha256:" + delivery.digest(args.record)},
            "The staged draft no longer holds the verified files")
    require(not [r for r in github.request("/git/matching-refs/tags/" + tag) if r["ref"] == "refs/tags/" + tag],
            "Tag already exists: " + tag)
    published = github.request("/releases/" + str(draft["id"]), "PATCH",
                               {"draft": False, "prerelease": True, "make_latest": "false"})
    print(f"PUBLISHED {tag} with {len(draft['assets'])} files: {published['html_url']}", flush=True)
    # From here the release is public and its record is kept: acceptance and promotion rely on that
    # artifact, not on how this step ends. Only a wrong public state is a failure: never run this stage
    # again and never delete anything because of it.
    require(not published["draft"] and published["prerelease"] and published["tag_name"] == tag,
            "The release IS published but is not a pre-release: inspect it, do not re-run")

    def latest():
        for _ in range(3):
            try:
                return github.request("/releases/latest")["tag_name"]
            except ValueError as error:
                print("Could not read the latest release: " + str(error), flush=True)
        return None

    current = latest()
    if current == tag:
        github.request("/releases/" + str(draft["id"]), "PATCH", {"make_latest": "false"})
        current = latest()
    if current is None:
        print("WARNING: the latest release could not be read; check by hand that it is not " + tag, flush=True)
    require(current != tag, "The pre-release IS published and became the latest release: inspect it, do not re-run")


def required_exercised(version, catalog_names):
    """The files a user installs or runs; acceptance must have exercised every one of them."""
    return ({f"NOH-{version}-windows-x64-{profile}-Setup.exe" for profile in ("Web", "Minimal", "Standard", "Complete")}
            | {f"NOH-{version}-windows-x64.zip", f"NOH-{version}-linux-x64.zip"} | set(catalog_names))


def validate_acceptance(report, release, identity, publication_assets):
    """The acceptance report must describe this release's verified files, this run and a full pass."""
    require(report.get("schema_version") == 1 and report.get("status") == "passed"
            and report.get("tag") == release["tag_name"] and report.get("release_id") == release["id"]
            and report.get("run_id") == identity["run"] and report.get("run_attempt") == identity["attempt"],
            "The acceptance report does not describe this release and run")
    require(report.get("release_files") == publication_assets,
            "The acceptance report covers other files than the publishing run verified")
    require(set(report.get("platforms", {})) == set(PLATFORMS)
            and all(p.get("status") == "passed" for p in report["platforms"].values()),
            "A platform did not pass acceptance")
    exercised = report.get("exercised_files", {})
    components = {name for name in publication_assets if re.fullmatch(r"NOH-windows-x64-(media|speech)-[0-9a-f]+\.zip", name)}
    require(len(components) == 2 and required_exercised(release["tag_name"][1:], components) <= set(exercised)
            and all(publication_assets.get(name) == digest for name, digest in exercised.items()),
            "Acceptance did not exercise every installer, package and component archive of this release")


def promote(args):
    require(os.environ.get("GITHUB_REF") == "refs/heads/main", "Promotion runs from main")
    workflow = int(os.environ["NOH_DELIVERY_WORKFLOW_ID"])
    require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", args.tag), "A final version tag is required")
    github = GitHub()
    require(github.request("")["id"] == REPOSITORY_ID, "Wrong repository")
    release = github.request("/releases/tags/" + args.tag)
    require(not release["draft"] and release["prerelease"], "Only a published pre-release can be promoted")
    require(release["body"].startswith(PENDING), "The release notes do not carry the pending status line")
    attached = [a for a in release["assets"] if a["name"] == "ACCEPTANCE.json"]
    require(len(attached) == 1, "The release has no single acceptance report")
    scratch = Path(os.environ["RUNNER_TEMP"]) / "noh-release-promotion"
    scratch.mkdir(exist_ok=False)
    publication = verified_publication(github, release, workflow, scratch)
    # The attached file only names the run that claims to have produced it. It is believed
    # once that run is a successful Release run on main holding the same bytes as an artifact.
    claimed = scratch / "attached-ACCEPTANCE.json"
    github.download("/releases/assets/" + str(attached[0]["id"]), claimed,
                    attached[0]["digest"].removeprefix("sha256:"))
    claim = delivery.read(claimed)
    run_id, attempt = str(claim.get("run_id")), str(claim.get("run_attempt"))
    require(re.fullmatch("[1-9][0-9]*", run_id) and re.fullmatch("[1-9][0-9]*", attempt),
            "The acceptance report names no run")
    release_run(github, int(run_id), int(attempt), workflow)
    evidence = download_artifact(github, one_artifact(
        github, run_id, acceptance_artifact_name(run_id, attempt)), scratch)
    require(delivery.digest(evidence / "ACCEPTANCE.json") == delivery.digest(claimed),
            "The attached acceptance report is not the one that run produced")
    validate_acceptance(delivery.read(evidence / "ACCEPTANCE.json"), release, {"run": run_id, "attempt": attempt},
                        publication["assets"])
    promoted = github.request("/releases/" + str(release["id"]), "PATCH", {
        "prerelease": False, "make_latest": "true",
        "body": ACCEPTED + release["body"][len(PENDING):]})
    require(not promoted["prerelease"] and github.request("/releases/latest")["tag_name"] == args.tag,
            "The release was not promoted to latest")
    print(f"Promoted {args.tag}: {promoted['html_url']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    publication = commands.add_parser("publish")
    publication.add_argument("--commit", required=True)
    for name in ("build-run", "build-attempt", "installer-run", "installer-attempt", "desktop-run"):
        publication.add_argument("--" + name, type=int, required=True)
    publication.add_argument("--rehearse", action="store_true",
                             help="Verify, upload to a draft, read back, then delete the draft: nothing is published")
    finalizing = commands.add_parser("finalize")
    finalizing.add_argument("--record", type=Path, required=True)
    promotion = commands.add_parser("promote")
    promotion.add_argument("--tag", required=True)
    args = parser.parse_args()
    {"publish": publish, "finalize": finalize, "promote": promote}[args.command](args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, provenance.subprocess.SubprocessError) as error:
        raise SystemExit(f"Release publication refused: {error}") from error
