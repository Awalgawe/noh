"""Print the next Release dispatches with the identifiers of the runs they follow.

Each stage of a release is a manual dispatch that names the runs before it: a build run,
its attempt and commit, then the installer and desktop runs. This reads those runs on
GitHub, refuses one that is not a finished, successful run on main of the expected
workflow, and prints the `gh workflow run` commands to dispatch next. It dispatches
nothing: the person who releases reads the commands and runs them.

    python tools/release_next.py after-build <build run>
    python tools/release_next.py before-publish <build run> <installer run> <desktop run>

GitHub is read through `gh api`, so `gh` must be signed in. The stages check everything
again when they run; this only spares copying identifiers by hand.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from setup_delivery import REPOSITORY

spec = importlib.util.spec_from_file_location("delivery_release", ROOT / "tools/delivery-release.py")
provenance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provenance)

RELEASE = ".github/workflows/release.yml"
DESKTOP = ".github/workflows/linux-desktop-probe.yml"
PLATFORMS = ("windows-x64", "linux-x64")


class GhApi:
    """Read-only GitHub requests through the signed-in `gh` command."""

    def request(self, route):
        result = subprocess.run(["gh", "api", "repos/" + REPOSITORY + route],
                                check=True, capture_output=True, text=True, timeout=60)
        return json.loads(result.stdout)


def finished_run(github, run_id, workflow):
    """The run as GitHub reports it: its latest attempt, finished and successful, on main."""
    if not str(run_id).isdigit():
        raise ValueError(f"Not a run ID: {run_id}")
    run = github.request(f"/actions/runs/{run_id}")
    if (run["repository"]["full_name"] != REPOSITORY or run["head_repository"]["full_name"] != REPOSITORY
            or run["path"] != workflow or run["head_branch"] != "main" or run["event"] != "workflow_dispatch"):
        raise ValueError(f"Run {run_id} is not a dispatch of {workflow} on main")
    if run["status"] != "completed" or run["conclusion"] != "success":
        raise ValueError(f"Run {run_id} has not succeeded (attempt {run['run_attempt']}: "
                         f"{run['status']}, {run['conclusion']})")
    return run


def attempt_jobs(github, run):
    return github.request(f"/actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")


def build_run(github, run_id):
    run = finished_run(github, run_id, RELEASE)
    try:
        provenance.validate_jobs(attempt_jobs(github, run), PLATFORMS, RELEASE)
    except ValueError as error:
        raise ValueError(f"Run {run_id} is not a successful Windows and Linux build: {error}") from None
    return run


def installer_run(github, run_id):
    run = finished_run(github, run_id, RELEASE)
    if not any(job["name"] == "installer / public-installer" and job["conclusion"] == "success"
               for job in attempt_jobs(github, run)["jobs"]):
        raise ValueError(f"Run {run_id} is not a successful installer stage")
    return run


def dispatch(workflow, **inputs):
    return " ".join(["gh workflow run", Path(workflow).name, "--repo", REPOSITORY, "--ref main",
                     *(f"-f {name}={value}" for name, value in inputs.items())])


def after_build(github, build):
    run = build_run(github, build)
    source = {"source_run_id": run["id"], "source_attempt": run["run_attempt"], "source_commit": run["head_sha"]}
    return [
        "# Step 3, the Windows installers of this build:",
        dispatch(RELEASE, stage="installer", target="windows-x64", **source),
        "# Step 4, every Linux desktop on this build:",
        dispatch(DESKTOP, suite="all", source_run=run["id"], source_attempt=run["run_attempt"],
                 source_commit=run["head_sha"]),
    ]


def before_publish(github, build, installer, desktop):
    run = build_run(github, build)
    wrapper = installer_run(github, installer)
    desktops = finished_run(github, desktop, DESKTOP)
    inputs = {"source_run_id": run["id"], "source_attempt": run["run_attempt"], "source_commit": run["head_sha"],
              "installer_run_id": wrapper["id"], "installer_attempt": wrapper["run_attempt"],
              "desktop_run_id": desktops["id"]}
    return [
        "# The publish stage checks that the installer and desktop runs took this build.",
        "# Rehearsal, nothing published:",
        dispatch(RELEASE, stage="rehearse-publish", target="windows-linux", **inputs),
        "# Step 5, publication as a pre-release:",
        dispatch(RELEASE, stage="publish", target="windows-linux", **inputs),
    ]


def main(argv=None, github=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("after-build").add_argument("build")
    publishing = commands.add_parser("before-publish")
    for name in ("build", "installer", "desktop"):
        publishing.add_argument(name)
    args = parser.parse_args(argv)
    github = github or GhApi()
    if args.command == "after-build":
        lines = after_build(github, args.build)
    else:
        lines = before_publish(github, args.build, args.installer, args.desktop)
    print("\n".join(lines))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"release_next: {error}")
