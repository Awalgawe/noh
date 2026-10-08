"""Private Setup CI transport: retained baseline, exact artifacts and inert drafts.

This never publishes a release or executes downloaded code. Package signature
verification belongs to build-installer.ps1 before payloads are staged here.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import urllib.error
import urllib.request
import zipfile

REPOSITORY = "Awalgawe/noh"
REPOSITORY_ID = 1406079938
API = f"https://api.github.com/repos/{REPOSITORY}"
PROFILES = ("minimal", "standard", "complete")
WORKFLOW = ".github/workflows/windows-setup.yml"
BRANCH = "codex/setup-release"


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def version(value):
    require(bool(re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value)), "Invalid version")
    return tuple(map(int, value.split(".")))


def member(root, name):
    parts = name.split("/")
    require(not any(p in ("", ".", "..") for p in parts)
            and not any(c in name for c in "\\:") and not PurePosixPath(name).is_absolute(), "Unsafe member path")
    root = Path(root).resolve()
    target = root.joinpath(*parts)
    require(target.resolve().is_relative_to(root), "Member escapes its root")
    for parent in (target, *target.parents):
        require(not parent.is_symlink() and not parent.is_junction(), "Reparse member refused")
        if parent == root:
            break
    return target


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class GitHub:
    def __init__(self):
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if not token:
            env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="Never")
            result = subprocess.run(["git", "credential", "fill"], env=env, input=
                                    f"protocol=https\nhost=github.com\npath={REPOSITORY}.git\n\n",
                                    text=True, capture_output=True, timeout=30, check=True)
            token = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line).get("password")
        require(bool(token), "GitHub authentication unavailable")
        self.headers = {"Authorization": "Bearer " + token, "Accept": "application/vnd.github+json",
                        "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "NOH-private-setup-ci"}
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, route, method="GET", body=None):
        data = None if body is None else json.dumps(body).encode()
        headers = dict(self.headers)
        if data is not None:
            headers["Content-Type"] = "application/json"
        try:
            with self.opener.open(urllib.request.Request(API + route, data=data, headers=headers, method=method), timeout=120) as response:
                return None if response.status == 204 else json.load(response)
        except urllib.error.HTTPError as error:
            raise ValueError(f"GitHub {method} {route}: HTTP {error.code}") from None

    def private(self):
        repo = self.request("")
        require(repo["id"] == REPOSITORY_ID and repo["full_name"] == REPOSITORY and repo["private"],
                "Expected the private NOH repository")

    def download(self, route, target, expected, size=None):
        target = Path(target)
        if target.exists():
            require(digest(target) == expected, "Existing download has another digest")
            if size is not None:
                require(target.stat().st_size == size, "Existing download has another size")
            return
        target.parent.mkdir(parents=True, exist_ok=True)
        # Release asset bytes use octet-stream; Actions archive endpoints require
        # GitHub's JSON media type before redirecting to the signed ZIP URL.
        headers = dict(self.headers)
        if route.startswith('/releases/assets/'):
            headers['Accept'] = 'application/octet-stream'
        try:
            response = self.opener.open(urllib.request.Request(API + route, headers=headers), timeout=120)
        except urllib.error.HTTPError as error:
            if error.code != 302:
                raise
            location = error.headers["Location"]
            require(location.startswith("https://"), "Download redirect is not HTTPS")
            # Credentials must never follow the signed storage redirect.
            response = urllib.request.urlopen(location, timeout=180)
            require(response.url.startswith("https://"), "Storage redirect is not HTTPS")
        partial = target.with_name(target.name + ".partial")
        with response, partial.open("xb") as sink:
            shutil.copyfileobj(response, sink, length=1024 * 1024)
        require(digest(partial) == expected and (size is None or partial.stat().st_size == size), "Downloaded bytes differ")
        partial.rename(target)

    def upload(self, release_id, path, name, expected):
        require(re.fullmatch(r"[A-Za-z0-9._-]+", name) is not None, "Unsafe asset name")
        require(digest(path) == expected, "Upload source changed")
        release = self.request(f"/releases/{release_id}")
        require(release["draft"] and release["published_at"] is None, "Only unpublished drafts may be modified")
        matches = [a for a in release["assets"] if a["name"] == name]
        require(len(matches) <= 1, "Ambiguous draft asset")
        if matches:
            validate_asset(matches[0], {"name": name, "sha256": expected, "size": Path(path).stat().st_size})
            return matches[0]
        headers = dict(self.headers, **{"Content-Type": "application/octet-stream", "Content-Length": str(Path(path).stat().st_size)})
        url = f"https://uploads.github.com/repos/{REPOSITORY}/releases/{release_id}/assets?name={name}"
        with Path(path).open("rb") as source:
            with self.opener.open(urllib.request.Request(url, data=source, headers=headers, method="POST"), timeout=1800) as response:
                uploaded = json.load(response)
        validate_asset(uploaded, {"name": name, "sha256": expected, "size": Path(path).stat().st_size})
        return uploaded


def validate_asset(actual, expected):
    require(actual["name"] == expected["name"] and actual["state"] == "uploaded"
            and actual["size"] == expected["size"] and actual["digest"] == "sha256:" + expected["sha256"]
            and ("id" not in expected or actual["id"] == expected["id"]), "Missing or changed retained asset")


def catalog(root, expected_commit=None, expected_hash=None, selected=None):
    root = Path(root)
    path = root / "candidate.json"
    if expected_hash:
        require(digest(path) == expected_hash, "Candidate catalog digest differs")
    data = read(path)
    require(data["schema"] == 1 and data["repository"] == REPOSITORY and data["package_id"] == "NOH"
            and re.fullmatch("[0-9a-f]{40}", data["source_commit"]), "Unexpected candidate identity")
    if expected_commit:
        require(data["source_commit"] == expected_commit, "Candidate source differs")
    version(data["version_b"])
    require(data["release_tag"] == "v" + data["version_b"], "Candidate tag differs")
    files = {item["path"]: item["sha256"] for item in data["files"]}
    require(len(files) == len(data["files"]) == len({n.casefold() for n in files}), "Duplicate candidate member")
    for name in (selected if selected is not None else files):
        require(name in files and re.fullmatch("[0-9a-f]{64}", files[name]) is not None, "Missing candidate member")
        require(digest(member(root, name)) == files[name], "Changed candidate member: " + name)
    return data


def suffix(profile):
    return "" if profile == "complete" else "-" + profile


def payloads(root, data):
    result = []
    for profile in PROFILES:
        tail = suffix(profile)
        release = read(Path(root) / f"release-B{tail}.json")
        require(release["version"] == data["version_b"] and release["package_id"] == "NOH"
                and release.get("profile", "complete") == profile and len(release["artifacts"]) == 1, "Profile descriptor differs")
        artifact = release["artifacts"][0]
        name = f"NOH-{data['version_b']}-win-x64{tail}-Setup.exe"
        require(artifact["file_name"] == name and artifact["url"] == f"https://github.com/{REPOSITORY}/releases/download/{data['release_tag']}/{name}", "Profile URL differs")
        for relative, asset_name in ((f"packages-B{tail}/{name}", name), (f"envelope-B{tail}.json", f"envelope{tail}.json")):
            path = member(root, relative)
            item = {"path": relative, "name": asset_name, "sha256": digest(path), "size": path.stat().st_size,
                    "profile": profile, "payload": relative.startswith("packages-")}
            if item["payload"]:
                require(item["sha256"] == artifact["sha256"] and item["size"] == artifact["size"], "Package descriptor differs")
            result.append(item)
    return result


def support_names():
    return ["candidate.json", "keys.json", "source-inventory.json", "tools/bootstrap-B.exe", "tools/update-release.exe"] + [f"release-B{suffix(p)}.json" for p in PROFILES]


def retain(args, github):
    root = args.candidate
    report = read(args.qualification)
    require(report["functional_passed"] and report["review_decision"] == "accepted-for-private-draft", "Accepted qualification required")
    data = catalog(root, report["application_commit"], report["candidate_sha256"])
    for check in report["checks"]:
        require(check["passed"] and digest(check["evidence"]) == check["evidence_sha256"], "Qualification evidence differs")
    release = github.request(f"/releases/{args.release_id}")
    require(release["draft"] and release["published_at"] is None and release["tag_name"] == data["release_tag"]
            and release["target_commitish"] == data["source_commit"], "Historical draft identity differs")
    assets = payloads(root, data)
    for item in assets:
        matches = [a for a in release["assets"] if a["name"] == item["name"]]
        require(len(matches) == 1, "Historical payload missing or ambiguous")
        validate_asset(matches[0], item)
        item["id"] = matches[0]["id"]
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / f"NOH-{data['version_b']}-baseline-support.zip"
    names = support_names()
    if not archive.exists():
        with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=1) as output:
            for name in names:
                output.write(member(root, name), name)
    with zipfile.ZipFile(archive) as source:
        require(set(source.namelist()) == set(names) and len(source.namelist()) == len(names), "Support inventory differs")
        for name in names:
            require(hashlib.sha256(source.read(name)).hexdigest() == digest(member(root, name)), "Support member differs")
    spec = {"name": archive.name, "sha256": digest(archive), "size": archive.stat().st_size,
            "members": [{"path": name, "sha256": digest(member(root, name))} for name in names]}
    lock = {"schema": 1, "repository": REPOSITORY, "repository_id": REPOSITORY_ID, "package_id": "NOH",
            "release_id": args.release_id, "source_commit": data["source_commit"], "version": data["version_b"],
            "candidate_sha256": digest(root / "candidate.json"), "qualification_sha256": digest(args.qualification),
            "assets": assets, "support": spec}
    if args.upload:
        spec["id"] = github.upload(args.release_id, archive, archive.name, spec["sha256"])["id"]
        baseline(lock, github)
    write(args.output / "baseline-lock.json", lock)
    print(f"Baseline {'retained' if args.upload else 'planned'}: {args.output / 'baseline-lock.json'}")


def baseline(lock, github, output=None, new_version=None):
    require(lock["schema"] == 1 and lock["repository"] == REPOSITORY and lock["repository_id"] == REPOSITORY_ID
            and lock["package_id"] == "NOH" and re.fullmatch("[0-9a-f]{40}", lock["source_commit"])
            and re.fullmatch("[0-9a-f]{64}", lock["candidate_sha256"]), "Invalid baseline lock")
    if new_version:
        require(version(lock["version"]) < version(new_version), "New version must be newer than the retained baseline")
    release = github.request(f"/releases/{lock['release_id']}")
    require(release["draft"] and release["published_at"] is None and release["tag_name"] == "v" + lock["version"]
            and release["target_commitish"] == lock["source_commit"], "Retained baseline release differs")
    expected = lock["assets"] + [lock["support"]]
    require(len(lock["assets"]) == 6 and len({x["id"] for x in expected}) == 7, "Incomplete or duplicate baseline assets")
    for spec in expected:
        matches = [a for a in release["assets"] if a["id"] == spec["id"]]
        require(len(matches) == 1, "Retained asset missing")
        validate_asset(matches[0], spec)
    if output:
        output = Path(output)
        output.mkdir(parents=True, exist_ok=True)
        support = lock["support"]
        archive = output.parent / support["name"]
        github.download(f"/releases/assets/{support['id']}", archive, support["sha256"], support["size"])
        extract(archive, output, {i["path"]: i["sha256"] for i in support["members"]}, 64 * 1024**2)
        for item in lock["assets"]:
            github.download(f"/releases/assets/{item['id']}", member(output, item["path"]), item["sha256"], item["size"])
        required = [n for n in support_names() if n != "candidate.json"] + [i["path"] for i in lock["assets"]]
        data = catalog(output, lock["source_commit"], lock["candidate_sha256"], required)
        require(data["version_b"] == lock["version"], "Acquired historical version differs")


def extract(archive, output, expected, limit):
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        require(len(entries) == len(expected) and {e.filename for e in entries} == set(expected)
                and len({e.filename.casefold() for e in entries}) == len(entries)
                and sum(e.file_size for e in entries) <= limit, "Unexpected archive inventory or expansion")
        for entry in entries:
            require(not entry.is_dir() and (entry.external_attr >> 16) & 0o170000 != 0o120000, "Archive link/directory refused")
            target = member(output, entry.filename)
            if target.exists():
                require(digest(target) == expected[entry.filename], "Existing extracted member differs")
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(entry) as stream, target.open("xb") as sink:
                shutil.copyfileobj(stream, sink, length=1024 * 1024)
            require(digest(target) == expected[entry.filename], "Extracted member digest differs")


def fetch(args, github):
    run = github.request(f"/actions/runs/{args.run_id}")
    same_run = str(args.run_id) == os.environ.get("GITHUB_RUN_ID")
    require(run["repository"]["id"] == REPOSITORY_ID and run["head_repository"]["id"] == REPOSITORY_ID
            and run["head_sha"] == args.commit and run["head_branch"] == BRANCH and run["path"] == WORKFLOW
            and 1 <= args.attempt <= run["run_attempt"] and (run["status"] == "completed" or (same_run and run["status"] == "in_progress")),
            "Successful trusted producer provenance required")
    jobs = github.request(f"/actions/runs/{args.run_id}/attempts/{args.attempt}/jobs?per_page=100")
    matches = [job for job in jobs["jobs"] if job["name"] == "candidate"]
    require(len(matches) == 1 and matches[0]["conclusion"] == "success", "Producer job has not succeeded")
    producer = matches[0]
    artifacts = github.request(f"/actions/runs/{args.run_id}/artifacts?per_page=100")
    matches = [a for a in artifacts["artifacts"] if a["name"] == "noh-windows-setup-candidate" and not a["expired"]]
    require(len(matches) == 1 and re.fullmatch(r"sha256:[0-9a-f]{64}", matches[0].get("digest") or ""), "Unique hashed producer artifact required")
    artifact = matches[0]
    require(producer["started_at"] <= artifact["created_at"] <= producer["completed_at"],
            "Artifact does not belong to the selected producer attempt")
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / "candidate.zip"
    github.download(f"/actions/artifacts/{artifact['id']}/zip", archive, artifact["digest"][7:])
    with zipfile.ZipFile(archive) as source:
        data = json.loads(source.read("candidate.json"))
        expected = {i["path"]: i["sha256"] for i in data["files"]}
        require(len(expected) == len(data["files"]), "Duplicate producer members")
        expected["candidate.json"] = hashlib.sha256(source.read("candidate.json")).hexdigest()
    root = args.output / "candidate"
    extract(archive, root, expected, 8 * 1024**3)
    data = catalog(root, args.commit, expected["candidate.json"])
    require(read(root / "keys.json") == read(args.public_keys), "Candidate trust differs from independent keys")
    write(args.output / "provenance.json", {"run_id": args.run_id, "attempt": args.attempt, "source_commit": args.commit,
          "producer_job_id": producer["id"], "artifact_id": artifact["id"], "artifact_digest": artifact["digest"], "candidate_sha256": expected["candidate.json"]})
    print(f"Verified exact producer artifact {artifact['id']}")


def wrapper_assets(root, data, installers, expected_commit, complete):
    result = []
    names = [(f"NOH-{data['version_b']}-win-x64-{p}-Offline.exe", "offline", [p]) for p in PROFILES]
    if complete:
        names += [(f"NOH-{data['version_b']}-win-x64-Install.exe", "web", list(PROFILES))]
    for name, mode, profiles in names:
        path = member(installers, name)
        record = read(str(path) + ".json")
        require(record["application_commit"] == data["source_commit"] and record["wrapper_commit"] == expected_commit
                and not record["wrapper_dirty"] and not record["public_distribution"] and record["version"] == data["version_b"]
                and record["candidate_sha256"] == digest(Path(root) / "candidate.json") and record["mode"] == mode
                and [p["profile"] for p in record["profiles"]] == profiles
                and record["sha256"] == digest(path) and record["size"] == path.stat().st_size, "Installer provenance differs")
        result.append({"name": name, "sha256": record["sha256"], "size": record["size"]})
    return result


def draft_identity(release, data, marker):
    require(release["draft"] and release["published_at"] is None and release["tag_name"] == data["release_tag"]
            and release["target_commitish"] == data["source_commit"] and marker in (release["body"] or ""), "Draft identity collision")


def slot_marker(commit, release_version):
    require(re.fullmatch('[0-9a-f]{40}', commit) is not None, 'Full source commit required')
    version(release_version)
    return f'<!-- NOH-private-ci:{commit}:{release_version} -->'


def slot_notes(commit, release_version):
    return (f'NOH {release_version} Windows CI candidate.\n\n{slot_marker(commit, release_version)}\n\n'
            f'Application source: `{commit}`.\n\n'
            'Private CI staging area for signed profile payloads and four double-click installers '
            '(web, Minimal, Standard and Complete offline). Uploads may be incomplete until the '
            'installer job succeeds. Its draft-result.json records the exact run, wrapper commit '
            'and ten verified asset digests.\n\n'
            'Native installation/update/repair and final wizard acceptance remain pending for these '
            "new bytes; the previous release's qualification is not transferred. "
            'Public distribution and Windows code signing are not qualified. The web installer '
            'requires runtime access to this private repository; no credentials are embedded. '
            'Use Standard Offline for an initial private trial.\n')


def slot(github, commit, release_version, create=False):
    marker = slot_marker(commit, release_version)
    data = {'source_commit': commit, 'release_tag': 'v' + release_version}
    notes = slot_notes(commit, release_version)
    matches = [r for r in github.request('/releases?per_page=100') if r['tag_name'] == data['release_tag']]
    require(len(matches) <= 1, 'Ambiguous private slot')
    if not matches:
        require(create and not os.environ.get('GITHUB_ACTIONS'), 'Prepare the exact private draft locally before dispatch; CI never creates or patches releases')
        matches = [github.request('/releases', 'POST', {'tag_name': data['release_tag'], 'target_commitish': commit,
                   'draft': True, 'prerelease': False, 'name': f'NOH {release_version} Windows CI candidate', 'body': notes})]
    draft_identity(matches[0], data, marker)
    require(matches[0]['body'] == notes, 'Private slot notes differ')
    return matches[0]


def draft(args, github):
    root = args.candidate
    provenance = read(root.parent / "provenance.json")
    data = catalog(root, provenance["source_commit"], provenance["candidate_sha256"])
    wrappers = wrapper_assets(root, data, args.installers, args.wrapper_commit, args.finish)
    assets = payloads(root, data)
    marker = slot_marker(data['source_commit'], data['version_b'])
    plan_path = args.installers / "draft-plan.json"
    if plan_path.exists():
        plan = read(plan_path)
        require(plan["marker"] == marker and plan["wrapper_commit"] == args.wrapper_commit
                and plan['candidate_sha256'] == provenance['candidate_sha256'], "Existing draft plan differs")
        release = github.request(f"/releases/{plan['draft_id']}")
    else:
        require(not args.finish, "Payload draft plan must precede finalization")
        release = slot(github, data['source_commit'], data['version_b'])
        # Persist immediately, before any upload. A failed upload is resumed by digest.
        plan = {"draft_id": release["id"], "marker": marker, "wrapper_commit": args.wrapper_commit,
                'candidate_sha256': provenance['candidate_sha256']}
        write(plan_path, plan)
    draft_identity(release, data, marker)
    expected = {a["name"]: a for a in assets + wrappers}
    require(len(release["assets"]) == len({a["name"] for a in release["assets"]}), "Duplicate remote asset")
    for asset in release["assets"]:
        require(asset["name"] in expected, "Unexpected existing draft asset")
        validate_asset(asset, expected[asset["name"]])
    sources = {}
    for item in assets:
        uploaded = github.upload(release["id"], member(root, item["path"]), item["name"], item["sha256"])
        if item["payload"]:
            sources[item["profile"]] = uploaded["url"]
    write(args.installers / "download-sources.json", sources)
    if not args.finish:
        print(f"Private payload draft ready: {release['id']}")
        return
    for item in wrappers:
        github.upload(release["id"], member(args.installers, item["name"]), item["name"], item["sha256"])
    # GITHUB_TOKEN cannot create/patch releases whose target modifies workflows
    # relative to the default branch. Keep the exact locally prepared slot intact.
    notes = slot_notes(data['source_commit'], data['version_b'])
    final = github.request(f"/releases/{release['id']}")
    draft_identity(final, data, marker)
    require(len(final["assets"]) == len(expected) == 10 and len({a["name"] for a in final["assets"]}) == 10
            and final["body"] == notes, "Final private draft is incomplete")
    for asset in final["assets"]:
        require(asset["name"] in expected, "Unexpected final asset")
        validate_asset(asset, expected[asset["name"]])
    write(args.installers / "draft-result.json", {"draft_id": final["id"], "url": final["html_url"], "draft": True,
          "published_at": None, "source_commit": data["source_commit"], "wrapper_commit": args.wrapper_commit,
          "provenance": provenance, "assets": [{k: a[k] for k in ("id", "name", "size", "digest", "state")} for a in final["assets"]],
          "native_acceptance": "pending", "public_distribution": False})
    print(f"Private draft verified: {final['html_url']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    retain_parser = commands.add_parser("retain-baseline")
    retain_parser.add_argument("--candidate", type=Path, required=True)
    retain_parser.add_argument("--qualification", type=Path, required=True)
    retain_parser.add_argument("--release-id", type=int, required=True)
    retain_parser.add_argument("--output", type=Path, required=True)
    retain_parser.add_argument("--upload", action="store_true")
    baseline_parser = commands.add_parser("baseline")
    baseline_parser.add_argument("--lock", type=Path, required=True)
    baseline_parser.add_argument("--output", type=Path)
    baseline_parser.add_argument("--new-version")
    slot_parser = commands.add_parser('slot')
    slot_parser.add_argument('--commit', required=True)
    slot_parser.add_argument('--version', required=True)
    slot_parser.add_argument('--create', action='store_true')
    slot_parser.add_argument('--receipt', type=Path)
    fetch_parser = commands.add_parser("fetch")
    fetch_parser.add_argument("--run-id", type=int, required=True)
    fetch_parser.add_argument("--attempt", type=int, required=True)
    fetch_parser.add_argument("--commit", required=True)
    fetch_parser.add_argument("--output", type=Path, required=True)
    fetch_parser.add_argument("--public-keys", type=Path, required=True)
    draft_parser = commands.add_parser("draft")
    draft_parser.add_argument("--candidate", type=Path, required=True)
    draft_parser.add_argument("--installers", type=Path, required=True)
    draft_parser.add_argument("--wrapper-commit", required=True)
    draft_parser.add_argument("--finish", action="store_true")
    args = parser.parse_args()
    github = GitHub()
    github.private()
    if args.command == "retain-baseline":
        retain(args, github)
    elif args.command == "baseline":
        baseline(read(args.lock), github, args.output, args.new_version)
        print("Baseline bytes verified" if args.output else "Baseline availability and API digests verified; payloads were not downloaded")
    elif args.command == "fetch":
        fetch(args, github)
    elif args.command == 'slot':
        result = slot(github, args.commit, args.version, args.create)
        if args.receipt:
            write(args.receipt, {'draft_id': result['id'], 'source_commit': args.commit, 'version': args.version,
                                'url': result['html_url'], 'draft': True, 'published_at': None})
        print(f"Exact private slot ready: {result['id']} {result['html_url']}")
    else:
        draft(args, github)


if __name__ == "__main__":
    try:
        main()
    except urllib.error.HTTPError as error:
        raise SystemExit(f"Private Setup operation failed: HTTP {error.code}; no automatic mutation retry") from None
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        raise SystemExit(f"Private Setup operation refused: {error}") from None
