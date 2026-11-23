"""Check conservative platform selection and the required PR gate."""
import itertools
import re
import subprocess
import tempfile
from pathlib import Path
import unittest

import ci_scope

ROOT = Path(__file__).resolve().parent.parent
# Files outside the list that name a release-only file: controls of the first job and release-stage tools.
READ_RELEASE_ONLY = {
    "tools/test-delivery.py", "tools/installer/test_public_ci.py",
    "tools/delivery-release.py", "tools/installer/public-ci.py",
}
MAY_NAME_RELEASE_ONLY = READ_RELEASE_ONLY | {
    "tools/ci_scope.py", "tools/test_ci_scope.py",  # the list and this check
    ".github/workflows/pr.yml",  # its first job only: see the check below
}
# The only other file that names one of those readers: the installer stage of a release.
MAY_NAME_THOSE_READERS = {".github/workflows/windows-installers.yml"}


def platforms(paths, event_name="pull_request"):
    """The native platforms a change selects; the plugin check is tested on its own below."""
    scope = ci_scope.select(paths, event_name)
    return {"windows": scope["windows"], "unix": scope["unix"]}


def embedded_effects(text):
    """Plugin folders an include macro builds into the binary: each include!/include_str!/include_bytes!
    call is read to its balanced closing parenthesis, so concat!(env!(…), "…") is read whole. A call
    naming assets/effects without a folder is returned whole, to be looked at."""
    found = set()
    for macro in re.finditer(r"\binclude(?:_str|_bytes)?!\s*\(", text):
        depth, end = 1, macro.end()
        while end < len(text) and depth:
            depth += {"(": 1, ")": -1}.get(text[end], 0)
            end += 1
        call = text[macro.end():end - 1]
        if "effects" in call.lower():
            folder = re.search(r"assets/effects/([^/\"]+)/", call)
            found.add(folder.group(1) if folder else call)
    return found


def executable_text(path, data):
    """The part of a tracked file a job can act on: a Rust comment names a file without running it."""
    if not path.endswith(".rs"):
        return data
    return b"\n".join(line for line in data.split(b"\n") if not line.lstrip().startswith(b"//"))


class ScopeTests(unittest.TestCase):
    def test_docs_need_no_native_build(self):
        self.assertEqual(platforms(["README.md", "docs/APP.md", "CONTRIBUTING.md",
                                              "SECURITY.md", ".github/pull_request_template.md"]),
                         {"windows": False, "unix": False})

    def test_download_site_needs_no_native_build(self):
        none = {"windows": False, "unix": False}
        for path in sorted(ci_scope.SITE_ONLY) + ["site/template.html", "site/i18n/fr.json"]:
            with self.subTest(path=path):
                self.assertEqual(platforms([path]), none)
        self.assertEqual(platforms([".github/workflows/pages.yml"]), none)
        # A page change beside an app change still builds the app everywhere.
        self.assertEqual(platforms(["site/template.html", "src/main.rs"]), {"windows": True, "unix": True})
        # The app reads its own words: a locale change still builds everywhere, even though the site shows some.
        self.assertEqual(platforms(["locales/fr.lang"]), {"windows": True, "unix": True})

    def test_release_tooling_needs_no_native_build(self):
        none = {"windows": False, "unix": False}
        for path in sorted(ci_scope.RELEASE_ONLY):
            with self.subTest(path=path):
                self.assertEqual(platforms([path]), none)
        self.assertEqual(platforms(sorted(ci_scope.RELEASE_ONLY) + ["docs/RELEASING.md"]), none)
        self.assertEqual(platforms(["tools/release_publication.py", "tools/windows-qa.ps1"]),
                         {"windows": True, "unix": False})
        self.assertEqual(platforms(["tools/release_publication.py", "src/media.rs"]),
                         {"windows": True, "unix": True})
        # Neighbours of the listed files are not release-only because they look alike.
        for path in ("tools/release_notes.py", "tools/test_release_other.py", ".github/workflows/rust-audit.yml",
                     "assets/upgrade-baseline.lock.json.bak", "assets/linux-desktop-images.yaml",
                     "tools/linux_matrix.py", "docs/release-evidence/linux-0.2.0-inputs.json"):
            with self.subTest(path=path):
                self.assertEqual(platforms([path]), {"windows": True, "unix": True})

    def test_release_tooling_is_a_list_of_existing_files(self):
        # Adding a file is a decision: it changes what the required check asks before a merge.
        self.assertEqual(ci_scope.RELEASE_ONLY, {
            "tools/release_publication.py", "tools/test_release_publication.py",
            "tools/release_acceptance.py", "tools/test_release_acceptance.py",
            "tools/release_next.py", "tools/test_release_next.py",
            "tools/linux_desktop_candidate.py", "tools/probe-linux-desktop-vm.sh",
            "tools/test-linux-desktop.py", "assets/linux-desktop-images.json",
            "assets/opensuse-signing-key.asc", "assets/upgrade-baseline.lock.json",
            ".github/workflows/release.yml", ".github/workflows/delivery-candidates.yml",
            ".github/workflows/delivery-draft.yml", ".github/workflows/linux-desktop-probe.yml",
        })
        for path in sorted(ci_scope.RELEASE_ONLY):
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())
        self.assertFalse(ci_scope.RELEASE_ONLY & (ci_scope.WINDOWS_ONLY | ci_scope.UNIX_ONLY))

    def test_rust_comments_do_not_count_as_naming_a_file(self):
        source = (b"//! Tested by the installer stage (`tools/installer/public-ci.py`).\n"
                  b"    // see assets/upgrade-baseline.lock.json\n"
                  b"let lock = \"assets/upgrade-baseline.lock.json\"; // used\n")
        kept = executable_text("src/auto_update/mod.rs", source)
        self.assertNotIn(b"public-ci.py", kept)
        self.assertIn(b"upgrade-baseline.lock.json", kept)  # code that names it still counts
        self.assertEqual(executable_text("tools/run.py", source), source)  # only Rust is filtered

    def test_nothing_a_native_job_reads_names_release_tooling(self):
        tracked = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True).stdout
        names = {path: Path(path).name.encode() for path in ci_scope.RELEASE_ONLY}
        readers = {path: Path(path).name.encode() for path in READ_RELEASE_ONLY}
        found = {}
        second_level = {}
        for item in tracked.decode("utf-8").rstrip(chr(0)).split(chr(0)):
            # Documentation and recorded evidence are read by people, not run by a job.
            if (item in ci_scope.RELEASE_ONLY or item in MAY_NAME_RELEASE_ONLY
                    or item.startswith("docs/") or item.endswith(".md")):
                continue
            data = executable_text(item, (ROOT / item).read_bytes())
            named = sorted(path for path, name in names.items() if name in data)
            if named:
                found[item] = named
            # One level further: a file that runs one of the readers would read the listed files through it.
            named = sorted(path for path, name in readers.items() if name in data)
            if named and item not in MAY_NAME_THOSE_READERS:
                second_level[item] = named
        self.assertEqual(found, {})
        self.assertEqual(second_level, {})
        # In the PR workflow, only the first job may name them: the jobs after it are the native ones.
        workflow = (ROOT / ".github/workflows/pr.yml").read_text(encoding="utf-8")
        first_job, separator, native_jobs = workflow.partition("\n  audit:\n")
        self.assertTrue(separator)
        self.assertIn("\n  scope:\n", first_job)
        for path, name in (names | readers).items():
            with self.subTest(path=path):
                self.assertNotIn(name.decode(), native_jobs)

    def test_synthetic_controls_run_for_every_change(self):
        workflow = (ROOT / ".github/workflows/pr.yml").read_text(encoding="utf-8")
        first_job = workflow.partition("\n  audit:\n")[0]
        steps = first_job.split("\n      - ")
        title = "name: Check build and delivery scripts with synthetic fixtures\n"
        controls = [step for step in steps if step.startswith(title)]
        self.assertEqual(len(controls), 1)
        # Neither that step nor any other step of the first job is conditional or allowed to fail.
        self.assertNotIn("\n        if:", first_job)
        self.assertNotIn("continue-on-error", first_job)
        # The controls that cover the release-only files.
        for command in ("python3 tools/test-delivery.py", "python3 tools/test-linux-desktop.py",
                        "python3 -m unittest discover -s tools/installer -p 'test_*.py'",
                        "python3 -m unittest discover -s tools -p 'test_release_*.py'"):
            with self.subTest(command=command):
                self.assertIn("          " + command + "\n", controls[0] + "\n")

    def test_windows_tools_are_isolated(self):
        self.assertEqual(platforms(["tools/windows-qa.ps1", "docs/BUILDING.md"]),
                         {"windows": True, "unix": False})
        self.assertEqual(platforms(["tools/public-installer.py", "tools/installer/public-verify.iss"]),
                         {"windows": True, "unix": False})

    def test_unix_checks_keep_all_unix_architectures(self):
        self.assertEqual(platforms(["tools/verify-unix.sh"]),
                         {"windows": False, "unix": True})

    def test_shared_inputs_require_every_platform(self):
        for path in ("src/media.rs", "src/auto_update/windows.rs", "Cargo.lock", "Cargo.toml",
                     "build.rs", ".cargo/config.toml", "locales/fr.json", "assets/noh-icon.png",
                     "tools/dev.rs", "tools/delivery.py", "tests/media.rs", ".github/workflows/pr.yml",
                     "tools/ci_scope.py", "future/input.dat", "docs/embedded.bin"):
            with self.subTest(path=path):
                self.assertEqual(platforms([path]), {"windows": True, "unix": True})

    def test_mixed_changes_take_union(self):
        self.assertEqual(platforms(["tools/windows-qa.ps1", "tools/verify-unix.sh"]),
                         {"windows": True, "unix": True})

    def test_missing_comparison_builds_all(self):
        for event_name, event in (("workflow_dispatch", {}), ("push", {"before": "0" * 40}),
                                  ("pull_request", {}), ("push", {"before": "--bad"})):
            self.assertEqual(ci_scope.select(ci_scope.changed_paths(event_name, event)),
                             {"windows": True, "unix": True, "plugins": True})

    def test_real_git_diff_includes_renames_deletions_and_many_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(["git", *args], cwd=root, capture_output=True,
                                      text=True, check=True).stdout.strip()
            git("init", "-q")
            git("config", "user.name", "CI fixture")
            git("config", "user.email", "fixture@example.invalid")
            (root / "src").mkdir()
            (root / "docs").mkdir()
            (root / "src/removed.rs").write_text("old code", encoding="utf-8")
            git("add", ".")
            git("commit", "-qm", "fixture base")
            base = git("rev-parse", "HEAD")
            git("mv", "src/removed.rs", "docs/moved.md")
            for i in range(305):
                (root / f"docs/note {i}.md").write_text("doc", encoding="utf-8")
            git("add", ".")
            git("commit", "-qm", "fixture change")
            paths = ci_scope.changed_paths("pull_request", {"pull_request": {"base": {"sha": base}}}, root)
            self.assertEqual(len(paths), 307)
            self.assertIn("src/removed.rs", paths)
            self.assertIn("docs/moved.md", paths)
            self.assertEqual(platforms(paths), {"windows": True, "unix": True})
            self.assertIsNone(ci_scope.changed_paths("push", {"before": "a" * 40}, root))

    def test_gate_accepts_only_expected_success_or_skip(self):
        for windows, unix, plugins in itertools.product((False, True), repeat=3):
            scope = {"windows": windows, "unix": unix, "plugins": plugins}
            results = {"scope": "success", "audit": "success" if windows or unix else "skipped",
                       "windows": "success" if windows else "skipped",
                       "unix": "success" if unix else "skipped",
                       "plugins": "success" if plugins else "skipped"}
            self.assertTrue(ci_scope.passed(scope, results))
            for job in results:
                for failure in ("failure", "cancelled", None):
                    with self.subTest(scope=scope, job=job, failure=failure):
                        self.assertFalse(ci_scope.passed(scope, {**results, job: failure}))
            for job, needed in (("windows", windows), ("unix", unix), ("plugins", plugins)):
                if needed:
                    self.assertFalse(ci_scope.passed(scope, {**results, job: "skipped"}))


class PluginTests(unittest.TestCase):
    def test_a_style_plugin_alone_builds_nothing_on_a_pr_and_runs_the_light_check(self):
        for paths in (["assets/effects/neon/effect.json"],
                      ["assets/effects/neon/effect.json", "assets/effects/neon/notes/read me.txt",
                       "assets/effects/drop/effect.json", "docs/EFFECTS.md"]):
            with self.subTest(paths=paths):
                self.assertEqual(ci_scope.select(paths), {"windows": False, "unix": False, "plugins": True})
        # A deleted or renamed plugin is listed on both sides of the diff: still a plugin change.
        self.assertEqual(ci_scope.select(["assets/effects/old/effect.json", "assets/effects/new/effect.json"]),
                         {"windows": False, "unix": False, "plugins": True})

    def test_main_and_dispatch_build_everything_for_a_plugin(self):
        # The Rust tests that read assets/effects run after each merge, and build the pack the PRs use.
        for event_name in ("push", "workflow_dispatch"):
            with self.subTest(event_name=event_name):
                self.assertEqual(ci_scope.select(["assets/effects/neon/effect.json"], event_name),
                                 {"windows": True, "unix": True, "plugins": True})
                self.assertEqual(ci_scope.select(["docs/APP.md"], event_name),
                                 {"windows": False, "unix": False, "plugins": False})

    def test_classique_is_compiled_in_and_builds_everywhere(self):
        for path in ("assets/effects/classique/effect.json", "assets/effects/classique/other.png"):
            with self.subTest(path=path):
                self.assertEqual(ci_scope.select([path]), {"windows": True, "unix": True, "plugins": True})

    def test_paths_that_are_not_inside_a_plugin_folder_build_everywhere(self):
        for path in ("assets/effects/README.md", "assets/effects/effect.json", "assets/effects/neon/",
                     "assets/effects-old/neon/effect.json", "assets/effect/neon/effect.json",
                     "src/assets/effects/neon/effect.json", "tests/fixtures/fx/wipe/effect.json",
                     "tests/fixtures/fx-instances-linux.txt"):
            with self.subTest(path=path):
                self.assertEqual(platforms([path]), {"windows": True, "unix": True})
        # A plugin beside app code: the app builds everywhere and the light check runs with that build.
        self.assertEqual(ci_scope.select(["assets/effects/neon/effect.json", "src/fx/plugin.rs"]),
                         {"windows": True, "unix": True, "plugins": True})

    def test_the_light_check_runs_when_its_own_files_change(self):
        for path in sorted(ci_scope.PLUGIN_CHECK):
            with self.subTest(path=path):
                self.assertTrue(ci_scope.select([path])["plugins"])
                self.assertTrue((ROOT / path).is_file())
        for path in ("README.md", "src/main.rs", "site/template.html", "tools/release_next.py"):
            with self.subTest(path=path):
                self.assertFalse(ci_scope.select([path])["plugins"])

    def test_only_classique_is_compiled_into_the_app(self):
        # A plugin folder named by include!/include_str!/include_bytes! is part of the binary: it cannot be
        # style-only.
        tracked = subprocess.run(["git", "ls-files", "-z", "--", "*.rs"], cwd=ROOT, capture_output=True,
                                 check=True).stdout.decode("utf-8").rstrip(chr(0)).split(chr(0))
        embedded = set()
        for item in tracked:
            text = executable_text(item, (ROOT / item).read_bytes()).decode("utf-8", errors="replace")
            embedded |= embedded_effects(text)
        self.assertEqual(embedded, ci_scope.COMPILED_EFFECTS)
        # And no build script or manifest reads the plugin folders at compile time.
        scripts = subprocess.run(["git", "ls-files", "-z", "--", "build.rs", "*/build.rs", "Cargo.toml",
                                  "*/Cargo.toml"], cwd=ROOT, capture_output=True,
                                 check=True).stdout.decode("utf-8").rstrip(chr(0)).split(chr(0))
        self.assertIn("build.rs", scripts)
        for path in scripts:
            with self.subTest(path=path):
                self.assertNotIn(b"assets/effects", (ROOT / path).read_bytes())

    def test_every_include_form_is_seen(self):
        for source, expected in (
            ('pub const C: &str = include_str!("../../assets/effects/classique/effect.json");', {"classique"}),
            ('include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/effects/drop/effect.json"))', {"drop"}),
            ('include!("../../assets/effects/drop/gen.rs");', {"drop"}),
            ('static B: &[u8] = include_bytes! (\n    "../assets/effects/neon/glyphs.bin"\n);', {"neon"}),
            ('include!(concat!(env!("OUT_DIR"), "/build_info.rs"));', set()),
            ('let p = Path::new("assets/effects/drop"); // read at run time', set()),
            ('my_include!("assets/effects/drop/x")', set()),
        ):
            with self.subTest(source=source):
                self.assertEqual(embedded_effects(source), expected)
        # A call naming the effects folder without a plugin folder is reported whole.
        self.assertEqual(embedded_effects('include_str!(EFFECTS_DIR)'), {"EFFECTS_DIR"})


if __name__ == "__main__":
    unittest.main()
