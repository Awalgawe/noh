"""Select PR validation platforms conservatively, without compiling the app."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess


WINDOWS_ONLY = {
    "tools/windows-qa.ps1", "tools/setup-tools.ps1",
    "tools/windows_test_runtime.py", "tools/windows_runtime.py",
    "tools/runtime_cache.py", "tools/test-runtime-cache.py",
    "tools/build-installer.ps1", "tools/installer-tools.ps1",
    "tools/prepare-setup-draft.ps1", "tools/setup_delivery.py",
    "tools/test_setup_delivery.py", "tools/update-setup-clients.ps1",
    "tools/update-setup-integration.ps1", "tools/update-setup-probe.ps1",
    "assets/setup-tools.lock.json", "assets/setup-baseline.lock.json",
    "assets/delivery-windows.lock.json", "assets/windows-native.lock.json",
    ".github/workflows/windows-setup.yml",
    ".github/workflows/windows-installers.yml",
}
UNIX_ONLY = {"tools/verify-unix.sh", ".github/workflows/native.yml"}


def select(paths):
    """Unknown paths and unavailable change lists require all native platforms."""
    if paths is None:
        return {"windows": True, "unix": True}
    windows = unix = False
    for path in paths:
        if path in {"README.md", "AGENTS.md", "CONTRIBUTING.md", "SECURITY.md",
                    ".github/pull_request_template.md"} or (
            path.startswith("docs/") and path.endswith(".md")
        ):
            continue
        if path in WINDOWS_ONLY or path.startswith("tools/installer/"):
            windows = True
        elif path in UNIX_ONLY:
            unix = True
        else:
            # Even Rust files named windows/macOS can have shared callers or
            # cfg boundaries. Never infer platform independence from their name.
            windows = unix = True
    return {"windows": windows, "unix": unix}


def changed_paths(event_name, event, cwd=None):
    if event_name == "pull_request":
        base = event.get("pull_request", {}).get("base", {}).get("sha", "")
    elif event_name == "push":
        base = event.get("before", "")
    else:
        return None
    if not re.fullmatch(r"[0-9a-f]{40}", base) or base == "0" * 40:
        return None
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", "--no-renames", "-z", base, "HEAD", "--"],
            cwd=cwd, capture_output=True, check=True, timeout=30,
        )
    except (subprocess.SubprocessError, OSError):
        return None
    # Both sides of a rename are included; NUL delimiters preserve unusual names.
    return result.stdout.decode("utf-8", errors="surrogateescape").rstrip("\0").split("\0") if result.stdout else []


def passed(scope, results):
    """A missing, failed or cancelled selected job must fail the final check."""
    if results.get("scope") != "success":
        return False
    required = {
        "audit": scope["windows"] or scope["unix"],
        "windows": scope["windows"], "unix": scope["unix"],
    }
    return all(results.get(job) == ("success" if needed else "skipped")
               for job, needed in required.items())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gate", action="store_true")
    args = parser.parse_args()
    if args.gate:
        needs = json.loads(os.environ["CI_NEEDS"])
        outputs = needs["scope"]["outputs"]
        if any(outputs.get(key) not in {"true", "false"} for key in ("windows", "unix")):
            raise SystemExit("Missing validated platform selection")
        scope = {key: outputs[key] == "true" for key in ("windows", "unix")}
        results = {key: value["result"] for key, value in needs.items()}
        if not passed(scope, results):
            raise SystemExit("PR validation failed: " + json.dumps(results, sort_keys=True))
        print("Required PR checks passed: " + json.dumps(scope, sort_keys=True))
        return
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    paths = changed_paths(os.environ["GITHUB_EVENT_NAME"], event)
    scope = select(paths)
    print(json.dumps({"platforms": scope, "changed_files": None if paths is None else len(paths)}))
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in scope.items():
            output.write(f"{key}={str(value).lower()}\n")


if __name__ == "__main__":
    main()
