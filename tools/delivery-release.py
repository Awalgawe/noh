#!/usr/bin/env python3
"""Read-only GitHub provenance verification and inert, qualified asset staging."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

import delivery


def api(repository, route):
    result = subprocess.run(["gh", "api", f"repos/{repository}/{route}"], check=True,
                            capture_output=True, timeout=120)
    return json.loads(result.stdout)


def validate_run(run, repository, workflow, commit, attempt):
    if (run["repository"]["full_name"] != repository or run["head_repository"]["full_name"] != repository
            or run["workflow_id"] != workflow or run["head_sha"] != commit or run["head_branch"] != "main"
            or run["event"] != "workflow_dispatch" or run["status"] != "completed"
            or run["conclusion"] != "success" or run["run_attempt"] != attempt
            or run["path"] not in (".github/workflows/delivery-candidates.yml", ".github/workflows/release.yml")):
        raise ValueError("Unapproved source run/repository/attempt/commit")


def validate_jobs(data, platforms, workflow_path):
    wrapped = workflow_path == ".github/workflows/release.yml"
    prefix = "build / " if wrapped else ""
    expected = {prefix + "controls", prefix + "rust-audit / Audit Cargo.lock against RustSec"}
    expected |= {prefix + "Candidate " + platform for platform in platforms}
    if wrapped:
        expected.add("preflight")
    jobs = data["jobs"]
    names = [job["name"] for job in jobs]
    if data["total_count"] != len(jobs) or len(set(names)) != len(names):
        raise ValueError("Incomplete or duplicate source job inventory")
    results = {job["name"]: job["conclusion"] for job in jobs}
    if any(results.get(name) != "success" for name in expected):
        raise ValueError("Missing/skipped/failed build, controls or RustSec audit")
    # GitHub may represent an unselected reusable call by its caller job or
    # its skipped child jobs. None of those jobs may have executed in a build run.
    allowed_skips = {"draft", "draft / verify", "draft / draft", "installer", "installer / public-installer"} if wrapped else set()
    if any(name not in allowed_skips or result != "skipped"
           for name, result in results.items() if name not in expected):
        raise ValueError("Unexpected source job or release stage")


def validate_artifacts(data, platforms, audit_name=None):
    expected = {"delivery-" + p for p in platforms}
    artifacts = data["artifacts"]
    names = [artifact["name"] for artifact in artifacts]
    allowed = expected | ({audit_name} if audit_name else set())
    if (data["total_count"] != len(artifacts) or len(names) != len(set(names))
            or not expected.issubset(names) or not set(names).issubset(allowed)
            or any(a["expired"] for a in artifacts)
            or any(not re.fullmatch(r"sha256:[0-9a-f]{64}", a.get("digest") or "") for a in artifacts)):
        raise ValueError("Missing, extra, ambiguous, unhashed or expired source artifact")
    return [artifact for artifact in artifacts if artifact["name"] in expected]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=int, required=True)
    parser.add_argument("--attempt", type=int, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--target", choices=["windows-x64", "macos-arm64", "all"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    workflow = os.environ.get("NOH_DELIVERY_WORKFLOW_ID", "")
    if (not re.fullmatch(r"[1-9][0-9]*", workflow) or not delivery.COMMIT.fullmatch(args.commit)
            or min(args.run, args.attempt) < 1):
        parser.error("Configure the approved workflow ID and exact positive run/attempt/source inputs")
    run = api(repository, f"actions/runs/{args.run}")
    validate_run(run, repository, int(workflow), args.commit, args.attempt)
    comparison = api(repository, f"compare/{args.commit}...main")
    if comparison["status"] not in ("ahead", "identical"):
        raise ValueError("Source commit is not retained on trusted main")
    platforms = ["windows-x64", "macos-arm64"] if args.target == "all" else [args.target]
    jobs = api(repository, f"actions/runs/{args.run}/attempts/{args.attempt}/jobs?per_page=100")
    validate_jobs(jobs, platforms, run["path"])
    audit_name = f"rust-audit-{args.run}-{args.attempt}"
    artifacts = validate_artifacts(api(repository, f"actions/runs/{args.run}/artifacts?per_page=100"),
                                   platforms, audit_name)
    scratch = Path(os.environ["RUNNER_TEMP"]) / "noh-release-verification"
    scratch.mkdir(exist_ok=False)
    args.output.mkdir(parents=True, exist_ok=False)
    records = []
    for artifact in artifacts:
        path = scratch / f"{artifact['id']}.zip"
        with path.open("xb") as output:
            subprocess.run(["gh", "api", f"repos/{repository}/actions/artifacts/{artifact['id']}/zip"],
                           stdout=output, check=True, timeout=600)
        if "sha256:" + delivery.digest(path) != artifact["digest"]:
            raise ValueError("Downloaded artifact digest does not match GitHub's immutable inventory")
        extracted = scratch / str(artifact["id"])
        extracted.mkdir()
        with zipfile.ZipFile(path) as archive:
            delivery.zip_members(archive)
            if sum(m.file_size for m in archive.infolist()) > 5 * delivery.LIMIT:
                raise ValueError("Artifact expansion exceeds the bounded verification envelope")
            archive.extractall(extracted)
        record = delivery.qualify(extracted)
        if (record["platform"] not in platforms or artifact["name"] != "delivery-" + record["platform"]
                or record["source_commit"] != args.commit or record["run_id"] != str(args.run)
                or record["run_attempt"] != str(args.attempt)):
            raise ValueError("Candidate identity differs from independently checked run metadata")
        for name in record["assets"]:
            destination = args.output / name
            if destination.exists():
                raise ValueError("Duplicate release filename")
            shutil.copy2(extracted / name, destination)
        shutil.copy2(extracted / "DELIVERY.json", args.output / (record["platform"] + "-DELIVERY.json"))
        records.append({"artifact_id": artifact["id"], "artifact_digest": artifact["digest"],
                        "platform": record["platform"], "delivery_sha256": delivery.digest(extracted / "DELIVERY.json")})
    (args.output / "SHA256SUMS").write_text("".join(f"{delivery.digest(p)}  {p.name}\n" for p in sorted(args.output.iterdir())), encoding="utf-8")
    delivery.write(args.output / "STAGED.json", {"schema_version": 1, "source_commit": args.commit,
                   "run_id": args.run, "run_attempt": args.attempt, "records": records,
                   "assets": delivery.inventory(args.output)})
    print("Independently qualified inert assets staged; nothing published")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        raise SystemExit(f"Draft preparation refused: {error}") from error
