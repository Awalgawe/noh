"""Check caption-style plugins with a NOH that is already built, without compiling it.

pack  (native Linux job, after its build): the debug CLI and the library test executable,
      stripped, with a record of the commit and the workspace they were built in.
fetch (light job): this run's pack if its Linux job ran, otherwise the newest pack from a push
      or dispatch run of this repository's PR validation whose app and tests match the PR base.
check (light job): the library's effect tests on this checkout's assets/effects, then the CLI
      lists the plugins and draws every preset of each changed one, landscape and portrait.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

import ci_scope

ROOT = Path(__file__).resolve().parent.parent
ARTIFACT = "noh-plugin-check-linux"
WORKFLOW = ".github/workflows/pr.yml"
TRUSTED_EVENTS = {"push", "workflow_dispatch"}
PAGES = 10  # of 100 packs: far more than 14 days of main's runs and one day of PRs
RECORD = "build.json"
CLI = "noh"
TESTS = "noh-lib-tests"
# Library tests run on the PR's plugins: every test of the effect engine and of the effect choice.
TEST_FILTERS = ["fx::", "effect_choice::"]
# A filter that matches nothing also passes: these must each run and pass once, at least one per
# filter. Each reads the bundled plugins; renaming one on main means updating this list (the first
# job's test_check_style_plugins.py finds each in its source file).
REQUIRED_TESTS = {
    "bundled_effects_load": "src/fx/tests.rs",
    "bundled_presets_match_the_recorded_instances": "src/fx/tests.rs",
    "every_bundled_preset_draws_deterministically": "src/fx/tests.rs",
    "the_bundled_plugins_take_an_installed_font_in_one_level_2_setting": "src/fx/tests.rs",
    "a_plugin_is_found_by_id_with_typed_checked_values": "src/effect_choice.rs",
}
# The OpenGL checks, ignored by default: they prove this runner draws with software OpenGL.
GPU_TEST = "gpu_checks"
SIZES = [(1280, 720), (720, 1280)]
SRT = ("1\n00:00:00,200 --> 00:00:01,400\nnothing feels the same tonight\n\n"
       "2\n00:00:01,500 --> 00:00:02,800\nÉté, déjà là\n")
PROBE_SECONDS = 0.8  # inside the first cue


class CheckError(Exception):
    pass


def run(command, **kwargs):
    kwargs.setdefault("check", True)
    return subprocess.run(command, **kwargs)


# --- pack ---------------------------------------------------------------------------------------

def library_test_executable(messages):
    """The library's test executable from `cargo test --lib --no-run --message-format=json`."""
    found = []
    for line in messages.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if (message.get("reason") == "compiler-artifact" and message.get("executable")
                and message.get("profile", {}).get("test")
                and "lib" in message.get("target", {}).get("kind", [])):
            found.append(message["executable"])
    if len(found) != 1:
        raise CheckError("Expected one library test executable, found %d" % len(found))
    return found[0]


def pack(target, out):
    messages = run(["cargo", "test", "--locked", "--features", "gui,mcp", "--lib", "--no-run",
                    "--message-format=json"], cwd=ROOT, capture_output=True, text=True).stdout
    tests = Path(library_test_executable(messages))
    cli = Path(target) / "debug" / CLI
    if not cli.is_file():
        raise CheckError("Missing %s: build the CLI first" % cli)
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    for source, name in ((cli, CLI), (tests, TESTS)):
        shutil.copy2(source, out / name)
        run(["strip", str(out / name)])
    record = {"commit": os.environ["GITHUB_SHA"], "workspace": str(ROOT),
              "run_id": os.environ.get("GITHUB_RUN_ID"), "event": os.environ.get("GITHUB_EVENT_NAME")}
    (out / RECORD).write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    sizes = {path.name: path.stat().st_size for path in sorted(out.iterdir())}
    print("Plugin check pack: " + json.dumps(sizes, sort_keys=True))


# --- fetch --------------------------------------------------------------------------------------

def gh_json(path):
    return json.loads(run(["gh", "api", "-H", "Accept: application/vnd.github+json", path],
                          capture_output=True, text=True).stdout)


def trusted(run_info, repository_id):
    """A pack built by a push or dispatch run of this repository's PR validation, never a PR run.
    A push builds main only; a dispatch may build another branch, and same_app decides then."""
    workflow = (run_info.get("path") or "").split("@", 1)[0]
    return (run_info.get("event") in TRUSTED_EVENTS and workflow == WORKFLOW
            and run_info.get("repository", {}).get("id") == repository_id
            and run_info.get("head_repository", {}).get("id") == repository_id
            and (run_info.get("event") != "push" or run_info.get("head_branch") == "main"))


def same_app(commit, base, cwd=ROOT):
    """True when the app and its tests at `commit` are those at `base`: only paths that select no
    native build differ (plugins, documentation, the download site, release tooling)."""
    if not re.fullmatch(r"[0-9a-f]{40}", commit or ""):
        return False
    if run(["git", "cat-file", "-e", commit + "^{commit}"], cwd=cwd, check=False,
           capture_output=True).returncode != 0:
        return False
    paths = ci_scope.diff(commit, base, cwd)
    if paths is None:
        return False
    scope = ci_scope.select(paths, "pull_request")
    return not (scope["windows"] or scope["unix"])


def pick(artifacts, run_info, repository_id, matches):
    """The newest unexpired trusted pack whose commit `matches` the base, or None. The commit is the
    run's head_sha from the API, never what the pack's own record says."""
    for artifact in sorted(artifacts, key=lambda a: a.get("created_at") or "", reverse=True):
        if artifact.get("expired") or artifact.get("name") != ARTIFACT:
            continue
        info = run_info(artifact["workflow_run"]["id"])
        if trusted(info, repository_id) and matches(info.get("head_sha")):
            return artifact, info
    return None


def missing_message(base):
    return ("No NOH built from main matches this PR's base %s yet. Either main's last PR validation run "
            "is still building or was cancelled, or its pack expired (14 days). Fix: wait for the run of "
            "PR validation on main to finish, then re-run this job; or start PR validation on main by hand "
            "(Actions > PR validation > Run workflow, branch main), wait for it, then re-run this job. "
            "Merging main into this branch is needed only if main has moved on since." % base)


def download(repository, artifact, out):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    archive = out / "pack.zip"
    with open(archive, "wb") as handle:
        run(["gh", "api", "repos/%s/actions/artifacts/%d/zip" % (repository, artifact["id"])],
            stdout=handle)
    with zipfile.ZipFile(archive) as zipped:
        names = set(zipped.namelist())
        if names != {CLI, TESTS, RECORD}:
            raise CheckError("Unexpected pack content: %s" % sorted(names))
        zipped.extractall(out)
    archive.unlink()
    for name in (CLI, TESTS):
        (out / name).chmod(0o755)


def fetch(out):
    repository = os.environ["GITHUB_REPOSITORY"]
    event_name = os.environ["GITHUB_EVENT_NAME"]
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    repository_id = event["repository"]["id"]
    unix = os.environ.get("UNIX_RESULT")
    if unix == "success":
        # This run built NOH: check the plugins with that build.
        listing = gh_json("repos/%s/actions/runs/%s/artifacts?name=%s"
                          % (repository, os.environ["GITHUB_RUN_ID"], ARTIFACT))
        artifacts = [a for a in listing.get("artifacts", []) if a.get("name") == ARTIFACT]
        if len(artifacts) != 1:
            raise CheckError("This run's Linux job did not keep its pack")
        download(repository, artifacts[0], out)
        print("Using this run's build")
        return
    if event_name != "pull_request":
        # On main, a plugin change selects every platform: the Linux job must have built.
        raise CheckError("The Linux job did not succeed (%s): nothing to check with" % unix)
    base = event["pull_request"]["base"]["sha"]
    found = None
    # Every PR's one-day pack has the same name: read further pages while none matched.
    for page in range(1, PAGES + 1):
        listing = gh_json("repos/%s/actions/artifacts?name=%s&per_page=100&page=%d"
                          % (repository, ARTIFACT, page))
        artifacts = listing.get("artifacts", [])
        found = pick(artifacts, lambda run_id: gh_json("repos/%s/actions/runs/%d" % (repository, run_id)),
                     repository_id, lambda commit: same_app(commit, base))
        if found or len(artifacts) < 100:
            break
    if not found:
        raise CheckError(missing_message(base))
    artifact, info = found
    download(repository, artifact, out)
    print("Using the build of %s from %s run %s" % (info["head_sha"], info["event"], info["id"]))


# --- check --------------------------------------------------------------------------------------

def check_record(record, workspace):
    """The test executable reads assets through the path it was built in: it must be this one."""
    if record.get("workspace") != str(workspace):
        raise CheckError("The pack was built in %r, this checkout is %r: its tests would read other "
                         "files" % (record.get("workspace"), str(workspace)))


def ran_once(log, name):
    """True when the libtest log shows the test `name` (any module path) passing exactly once."""
    return len(re.findall(r"^test ([a-z_0-9]+::)*%s \.\.\. ok$" % re.escape(name), log, re.M)) == 1


def passed_count(log):
    """Tests passed by one libtest run, or an error when it did not end with `test result: ok`."""
    results = re.findall(r"^test result: (\w+)\. (\d+) passed; (\d+) failed", log, re.M)
    if len(results) != 1 or results[0][0] != "ok" or results[0][2] != "0":
        raise CheckError("The library tests did not pass")
    return int(results[0][1])


def library_result(log, returncode):
    """The number of library tests passed, when the run passed and ran every required test once."""
    count = passed_count(log)
    absent = [name for name in REQUIRED_TESTS if not ran_once(log, name)]
    if count == 0 or returncode != 0 or absent:
        raise CheckError("The library effect tests did not pass; not run once: %s" % absent)
    return count


def changed_effects(paths, effects_dir):
    """Effect folders a change touched that still exist; every folder when the change is unknown."""
    present = sorted(p.name for p in Path(effects_dir).iterdir() if p.is_dir())
    if paths is None:
        return present
    touched = {path.split("/")[2] for path in paths
               if path.startswith("assets/effects/") and path.count("/") >= 3}
    return [name for name in present if name in touched]


def check_listing(listing, bundled):
    """Every folder of `bundled` loaded as an effect, nothing refused. Returns {folder name: effect}."""
    errors = ["refused %s: %s" % (r.get("folder"), r.get("reason")) for r in listing.get("refused", [])]
    loaded = {}
    for effect in listing.get("effects", []):
        folder = effect.get("folder")
        if folder and Path(folder).parent.resolve() == Path(bundled).resolve():
            loaded[Path(folder).name] = effect
    for path in sorted(Path(bundled).iterdir()):
        if path.is_dir() and path.name not in loaded:
            errors.append("folder %s did not load as an effect" % path.name)
    if errors:
        raise CheckError("Effect plugins refused: " + "; ".join(errors))
    return loaded


def frame(ffmpeg, video, size):
    width, height = size
    data = run([ffmpeg, "-v", "error", "-ss", str(PROBE_SECONDS), "-i", str(video), "-frames:v", "1",
                "-f", "rawvideo", "-pix_fmt", "gray", "-"], capture_output=True).stdout
    if len(data) != width * height:
        raise CheckError("Could not read a frame of %s" % video)
    return data


def differs(before, after):
    """Captions drew something: at least 0.05 % of the pixels changed by more than a coding error."""
    changed = sum(1 for a, b in zip(before, after) if abs(a - b) > 24)
    return changed * 2000 >= len(before)


def gl_env(home):
    env = dict(os.environ, LIBGL_ALWAYS_SOFTWARE="1", GALLIUM_DRIVER="llvmpipe",
               XDG_CONFIG_HOME=str(home), RUST_BACKTRACE="1")
    return env


def check(bundle, effects_dir=ROOT / "assets/effects"):
    bundle = Path(bundle)
    check_record(json.loads((bundle / RECORD).read_text(encoding="utf-8")), ROOT)
    work = Path(tempfile.mkdtemp(prefix="noh-plugin-check-"))
    env = gl_env(work / "config")  # an empty user effects folder
    xvfb = ["xvfb-run", "-a", "-s", "-screen 0 1280x1024x24"]
    ffmpeg = shutil.which("ffmpeg") or sys.exit("Install FFmpeg")
    env["NOH_FFMPEG"] = ffmpeg
    print("::group::Library effect tests on this checkout's plugins")
    log = run([str(bundle / TESTS), "--test-threads=1", *TEST_FILTERS], env=env, check=False,
              capture_output=True, text=True)
    print(log.stdout[-20000:] + log.stderr[-5000:])
    print("::endgroup::")
    count = library_result(log.stdout, log.returncode)
    print("::group::OpenGL checks with software rendering")
    gpu = run(xvfb + [str(bundle / TESTS), "--ignored", "--test-threads=1", GPU_TEST], env=env,
              check=False, capture_output=True, text=True)
    print(gpu.stdout[-10000:] + gpu.stderr[-5000:])
    print("::endgroup::")
    if gpu.returncode != 0 or not ran_once(gpu.stdout, GPU_TEST):
        raise CheckError("The OpenGL checks did not pass once")
    # The CLI reads effects/ beside itself, as in the packaged app.
    app = work / "app"
    app.mkdir()
    shutil.copy2(bundle / CLI, app / CLI)
    shutil.copytree(effects_dir, app / "effects")
    listing = json.loads(run([str(app / CLI), "--list-effects=json"], env=env, capture_output=True,
                             text=True).stdout)
    loaded = check_listing(listing, app / "effects")
    event_name = os.environ.get("GITHUB_EVENT_NAME", "")
    event = {}
    if os.environ.get("GITHUB_EVENT_PATH"):
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    names = changed_effects(ci_scope.changed_paths(event_name, event, ROOT), effects_dir)
    print("Effects loaded: %s; drawn: %s" % (sorted(e["id"] for e in loaded.values()), names))
    (work / "lyrics.srt").write_text(SRT, encoding="utf-8")
    failures = []
    for size in SIZES:
        source = work / ("source-%dx%d.mp4" % size)
        run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=%dx%d:r=24:d=3" % size,
             "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:v", "libx264",
             "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", str(source)])
        before = frame(ffmpeg, source, size)
        for name in names:
            effect = loaded[name]
            for preset in effect["presets"]:
                output = work / ("%s-%s-%dx%d.mp4" % (name, preset, *size))
                burn = run(xvfb + [str(app / CLI), "--burn-subtitles", str(source),
                                   "--srt", str(work / "lyrics.srt"), "--effect", effect["id"],
                                   "--effect-preset", preset, "--output", str(output), "--json"],
                           env=env, check=False, capture_output=True, text=True)
                label = "%s %s %dx%d" % (effect["id"], preset, *size)
                if burn.returncode != 0 or not output.is_file():
                    failures.append(label + ": exit %d" % burn.returncode)
                    print("%s failed:\n%s\n%s" % (label, burn.stdout[-4000:], burn.stderr[-4000:]))
                elif not differs(before, frame(ffmpeg, output, size)):
                    failures.append(label + ": no caption drawn at %.1f s" % PROBE_SECONDS)
                else:
                    print(label + ": drawn")
    if failures:
        raise CheckError("Drawing failed: " + "; ".join(failures))
    print("Style plugins checked: %d library tests, %d effects drawn" % (count, len(names)))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("pack")
    command.add_argument("--target", default=str(ROOT / "target"))
    command.add_argument("--out", required=True)
    commands.add_parser("fetch").add_argument("--out", required=True)
    commands.add_parser("check").add_argument("--bundle", required=True)
    args = parser.parse_args()
    try:
        if args.command == "pack":
            pack(args.target, args.out)
        elif args.command == "fetch":
            fetch(args.out)
        else:
            check(args.bundle)
    except (CheckError, subprocess.CalledProcessError) as error:
        print("::error::%s" % error, file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
