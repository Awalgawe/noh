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
    "tools/installer-tools.ps1", "tools/public-installer.py",
    "tools/setup_delivery.py", "tools/test_setup_delivery.py",
    "assets/setup-tools.lock.json", "assets/setup-baseline.lock.json",
    "assets/delivery-windows.lock.json", "assets/windows-native.lock.json",
    ".github/workflows/windows-installers.yml",
}
UNIX_ONLY = {"tools/verify-unix.sh", ".github/workflows/native.yml"}
# No native job reads these files: no application build can tell whether one of them changed.
# The controls of the first job are the only PR check that can read them. They read the tools,
# the desktop image pins, their signing key and the upgrade baseline; the desktop probe script and the release
# workflows that no control opens are first read at a release stage. Each file is named, never
# a pattern; test_ci_scope.py lists what names them, and what names the tools that read them.
RELEASE_ONLY = {
    "tools/release_publication.py", "tools/test_release_publication.py",
    "tools/release_acceptance.py", "tools/test_release_acceptance.py",
    "tools/release_next.py", "tools/test_release_next.py",
    "tools/linux_desktop_candidate.py", "tools/probe-linux-desktop-vm.sh",
    "tools/test-linux-desktop.py", "assets/linux-desktop-images.json",
    "assets/opensuse-signing-key.asc", "assets/upgrade-baseline.lock.json",
    ".github/workflows/release.yml", ".github/workflows/delivery-candidates.yml",
    ".github/workflows/delivery-draft.yml", ".github/workflows/linux-desktop-probe.yml",
}

# The download site: no native job builds or reads it. PR validation's first job checks its builder,
# and the Pages workflow rebuilds it from main.
SITE_ONLY = {
    "tools/build_site.py", "tools/test_build_site.py", "tools/testdata/site-release.json",
    ".github/workflows/pages.yml",
}


# Caption-style plugins: the app reads `assets/effects/<id>/` beside itself at run time, so on a PR a
# change there builds nothing; the light job checks it with a NOH already built (check_style_plugins.py).
# Classique is compiled into the app (src/fx/catalog.rs), so a change to it builds everywhere. On main,
# every plugin change selects every platform: the Rust tests that read the plugins run after each merge.
EFFECTS = "assets/effects/"
COMPILED_EFFECTS = {"classique"}
# The light job's own files: a change to one of them runs it.
PLUGIN_CHECK = {
    "tools/check_style_plugins.py", "tools/test_check_style_plugins.py", "tools/ci_scope.py",
    ".github/workflows/pr.yml", ".github/workflows/native.yml",
}


def effect_folder(path):
    """The plugin folder a path belongs to, or None: a file directly in assets/effects/ is not one."""
    if not path.startswith(EFFECTS):
        return None
    parts = path[len(EFFECTS):].split("/")
    if len(parts) < 2 or parts[0] in {"", ".", ".."} or not parts[-1]:
        return None
    return parts[0]


def style_only(path):
    folder = effect_folder(path)
    return folder is not None and folder not in COMPILED_EFFECTS


def select(paths, event_name="pull_request"):
    """Unknown paths and unavailable change lists require all native platforms and the plugin check.
    Plugins build nothing only on a PR: elsewhere they count as shared inputs."""
    if paths is None:
        return {"windows": True, "unix": True, "plugins": True}
    windows = unix = plugins = False
    for path in paths:
        if effect_folder(path) is not None or path in PLUGIN_CHECK:
            plugins = True
        if event_name == "pull_request" and style_only(path):
            continue
        if path in {"README.md", "AGENTS.md", "CONTRIBUTING.md", "SECURITY.md",
                    ".github/pull_request_template.md"} or (
            path.startswith("docs/") and path.endswith(".md")
        ) or path in RELEASE_ONLY or path in SITE_ONLY or path.startswith("site/"):
            continue
        if path in WINDOWS_ONLY or path.startswith("tools/installer/"):
            windows = True
        elif path in UNIX_ONLY:
            unix = True
        else:
            # Even Rust files named windows/macOS can have shared callers or
            # cfg boundaries. Never infer platform independence from their name.
            windows = unix = True
    return {"windows": windows, "unix": unix, "plugins": plugins}


def changed_paths(event_name, event, cwd=None):
    if event_name == "pull_request":
        base = event.get("pull_request", {}).get("base", {}).get("sha", "")
    elif event_name == "push":
        base = event.get("before", "")
    else:
        return None
    if not re.fullmatch(r"[0-9a-f]{40}", base) or base == "0" * 40:
        return None
    return diff(base, "HEAD", cwd)


def diff(base, head, cwd=None):
    """Every path that differs between two commits, or None when Git cannot compare them."""
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", "--no-renames", "-z", base, head, "--"],
            cwd=cwd, capture_output=True, check=True, timeout=30,
        )
    except (subprocess.SubprocessError, OSError):
        return None
    # Both sides of a rename are included; NUL delimiters preserve unusual names.
    return result.stdout.decode("utf-8", errors="surrogateescape").rstrip("\0").split("\0") if result.stdout else []


SCOPE_KEYS = ("windows", "unix", "plugins")


def passed(scope, results):
    """A missing, failed or cancelled selected job must fail the final check."""
    if results.get("scope") != "success":
        return False
    required = {
        "audit": scope["windows"] or scope["unix"],
        "windows": scope["windows"], "unix": scope["unix"], "plugins": scope["plugins"],
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
        if any(outputs.get(key) not in {"true", "false"} for key in SCOPE_KEYS):
            raise SystemExit("Missing validated platform selection")
        scope = {key: outputs[key] == "true" for key in SCOPE_KEYS}
        results = {key: value["result"] for key, value in needs.items()}
        if not passed(scope, results):
            raise SystemExit("PR validation failed: " + json.dumps(results, sort_keys=True))
        print("Required PR checks passed: " + json.dumps(scope, sort_keys=True))
        return
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    paths = changed_paths(os.environ["GITHUB_EVENT_NAME"], event)
    scope = select(paths, os.environ["GITHUB_EVENT_NAME"])
    print(json.dumps({"platforms": scope, "changed_files": None if paths is None else len(paths)}))
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in scope.items():
            output.write(f"{key}={str(value).lower()}\n")


if __name__ == "__main__":
    main()
