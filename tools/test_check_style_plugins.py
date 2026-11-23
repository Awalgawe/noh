"""Check the style plugin check's choices without a NOH build: pack selection, trust, results."""
import json
import re
import subprocess
import tempfile
from pathlib import Path
import unittest

import check_style_plugins as check

REPO = 1411000018
TRUSTED = {"event": "push", "path": ".github/workflows/pr.yml", "head_sha": "a" * 40, "head_branch": "main",
           "repository": {"id": REPO}, "head_repository": {"id": REPO}, "id": 1}


def artifact(identifier, created, run_id, expired=False, name=check.ARTIFACT):
    return {"id": identifier, "name": name, "expired": expired, "created_at": created,
            "workflow_run": {"id": run_id}}


class PackTests(unittest.TestCase):
    def test_the_library_test_executable_is_the_only_lib_test_artifact(self):
        lines = [
            {"reason": "compiler-artifact", "target": {"kind": ["lib"]}, "profile": {"test": False},
             "executable": None},
            {"reason": "compiler-artifact", "target": {"kind": ["bin"]}, "profile": {"test": True},
             "executable": "/t/debug/deps/noh-bin"},
            {"reason": "compiler-artifact", "target": {"kind": ["lib"]}, "profile": {"test": True},
             "executable": "/t/debug/deps/noh-0123"},
            {"reason": "build-finished", "success": True},
        ]
        messages = "\n".join(json.dumps(line) for line in lines) + "\nnot json\n"
        self.assertEqual(check.library_test_executable(messages), "/t/debug/deps/noh-0123")
        with self.assertRaises(check.CheckError):
            check.library_test_executable(json.dumps(lines[0]))


class TrustTests(unittest.TestCase):
    def test_only_push_and_dispatch_runs_of_pr_validation_in_this_repository(self):
        self.assertTrue(check.trusted(TRUSTED, REPO))
        self.assertTrue(check.trusted({**TRUSTED, "event": "workflow_dispatch"}, REPO))
        # A dispatch on another branch counts only through same_app.
        self.assertTrue(check.trusted({**TRUSTED, "event": "workflow_dispatch", "head_branch": "x"}, REPO))
        self.assertTrue(check.trusted({**TRUSTED, "path": ".github/workflows/pr.yml@refs/heads/main"}, REPO))
        for change in ({"event": "pull_request"}, {"event": "pull_request_target"},
                       {"event": "workflow_run"}, {"event": None},
                       {"path": ".github/workflows/native.yml"}, {"path": ".github/workflows/pr.yml.bak"},
                       {"path": None}, {"head_repository": {"id": 7}}, {"head_repository": {}},
                       {"repository": {"id": 7}}, {"head_branch": "feature"}):
            with self.subTest(change=change):
                self.assertFalse(check.trusted({**TRUSTED, **change}, REPO))

    def test_the_newest_trusted_matching_pack_is_picked(self):
        runs = {
            1: {**TRUSTED, "id": 1, "head_sha": "1" * 40},
            2: {**TRUSTED, "id": 2, "head_sha": "2" * 40, "event": "pull_request"},
            3: {**TRUSTED, "id": 3, "head_sha": "3" * 40},
            4: {**TRUSTED, "id": 4, "head_sha": "4" * 40},
        }
        artifacts = [
            artifact(10, "2026-11-20T00:00:00Z", 1),
            artifact(11, "2026-11-23T00:00:00Z", 2),  # newest, but built by a PR
            artifact(12, "2026-11-22T00:00:00Z", 3, expired=True),
            artifact(13, "2026-11-21T00:00:00Z", 4, name="native-ubuntu-24.04"),
            artifact(14, "2026-11-21T12:00:00Z", 1),
        ]
        picked = check.pick(artifacts, runs.__getitem__, REPO, lambda commit: True)
        self.assertEqual(picked[0]["id"], 14)
        # Only a pack whose app matches the base counts, however new.
        picked = check.pick(artifacts, runs.__getitem__, REPO, lambda commit: False)
        self.assertIsNone(picked)
        picked = check.pick([artifact(11, "2026-11-23T00:00:00Z", 2)], runs.__getitem__, REPO,
                            lambda commit: True)
        self.assertIsNone(picked)

    def test_the_missing_pack_message_says_what_to_do(self):
        message = check.missing_message("b" * 40)
        self.assertIn("b" * 40, message)
        self.assertIn("re-run this job", message)
        self.assertIn("Run workflow, branch main", message)

    def test_a_pack_matches_a_base_when_only_plugins_documentation_or_site_differ(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.run(["git", *args], cwd=root, capture_output=True, text=True,
                                      check=True).stdout.strip()

            def commit(path, text):
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text(text, encoding="utf-8")
                git("add", ".")
                git("commit", "-qm", path)
                return git("rev-parse", "HEAD")

            git("init", "-q")
            git("config", "user.name", "CI fixture")
            git("config", "user.email", "fixture@example.invalid")
            built = commit("src/main.rs", "fn main() {}")
            plugin = commit("assets/effects/neon/effect.json", "{}")
            docs = commit("docs/EFFECTS.md", "notes")
            self.assertTrue(check.same_app(built, docs, root))
            self.assertTrue(check.same_app(plugin, built, root))  # either direction
            classique = commit("assets/effects/classique/effect.json", "{}")
            self.assertFalse(check.same_app(built, classique, root))
            app = commit("src/main.rs", "fn main() { }")
            self.assertFalse(check.same_app(built, app, root))
            fixture = commit("tests/fixtures/fx-instances-linux.txt", "x")
            self.assertFalse(check.same_app(app, fixture, root))
            self.assertFalse(check.same_app("c" * 40, app, root))  # not in this checkout
            self.assertFalse(check.same_app("--help", app, root))
            self.assertFalse(check.same_app(None, app, root))


class ResultTests(unittest.TestCase):
    def test_the_pack_must_come_from_this_workspace(self):
        check.check_record({"workspace": "/home/runner/work/noh/noh"}, "/home/runner/work/noh/noh")
        for record in ({"workspace": "/home/runner/work/fork/fork"}, {}):
            with self.subTest(record=record), self.assertRaises(check.CheckError):
                check.check_record(record, "/home/runner/work/noh/noh")

    def test_library_results_need_one_passing_summary(self):
        self.assertEqual(check.passed_count("test a ... ok\n\ntest result: ok. 41 passed; 0 failed; 2 ignored"), 41)
        for log in ("", "test result: FAILED. 40 passed; 1 failed; 0 ignored",
                    "test result: ok. 1 passed; 0 failed\ntest result: ok. 2 passed; 0 failed"):
            with self.subTest(log=log), self.assertRaises(check.CheckError):
                check.passed_count(log)

    def test_required_tests_must_each_pass_once(self):
        log = ("test fx::tests::bundled_effects_load ... ok\n"
               "test fx::tests::bundled_effects_load_twice ... ok\n"
               "test fx::tests::gpu_checks ... FAILED\n"
               "test effect_choice::tests::a ... ok\n")
        self.assertTrue(check.ran_once(log, "bundled_effects_load"))
        self.assertFalse(check.ran_once(log, "gpu_checks"))
        self.assertFalse(check.ran_once(log, "effects_load"))
        self.assertFalse(check.ran_once(log + log, "bundled_effects_load"))
        self.assertFalse(check.ran_once("test fx::tests::bundled_effects_load ... ignored\n",
                                        "bundled_effects_load"))

    def test_a_library_run_missing_one_required_test_fails(self):
        lines = ["test %s::tests::%s ... ok" % ("effect_choice" if path.endswith("effect_choice.rs") else "fx",
                                                name)
                 for name, path in check.REQUIRED_TESTS.items()]
        summary = "test result: ok. %d passed; 0 failed; 3 ignored" % len(lines)
        self.assertEqual(check.library_result("\n".join(lines + ["", summary]), 0), len(lines))
        for index, name in enumerate(check.REQUIRED_TESTS):
            # As if a rename on main made one filter match nothing: the others still pass.
            log = "\n".join(lines[:index] + lines[index + 1:] + ["", summary])
            with self.subTest(missing=name), self.assertRaisesRegex(check.CheckError, name):
                check.library_result(log, 0)
        with self.assertRaises(check.CheckError):
            check.library_result("\n".join(lines + ["", summary]), 101)
        # One required test per filter, so a filter that matches nothing is noticed.
        for prefix, filters in (("src/fx/", "fx::"), ("src/effect_choice.rs", "effect_choice::")):
            self.assertIn(filters, check.TEST_FILTERS)
            self.assertTrue(any(path.startswith(prefix) for path in check.REQUIRED_TESTS.values()), prefix)

    def test_the_required_tests_exist_in_the_library(self):
        # A rename on main must update the list, or the light job fails closed: see it here first.
        for name, path in {**check.REQUIRED_TESTS, check.GPU_TEST: "src/fx/tests.rs"}.items():
            with self.subTest(name=name):
                source = (check.ROOT / path).read_text(encoding="utf-8")
                self.assertRegex(source, r"#\[test\][^{]*\bfn %s\(\)" % name)

    def test_changed_effects_are_the_touched_folders_that_remain(self):
        with tempfile.TemporaryDirectory() as directory:
            effects = Path(directory)
            for name in ("classique", "drop", "neon"):
                (effects / name).mkdir()
            self.assertEqual(check.changed_effects(None, effects), ["classique", "drop", "neon"])
            self.assertEqual(check.changed_effects(
                ["assets/effects/neon/effect.json", "assets/effects/gone/effect.json",
                 "assets/effects/README.md", "docs/EFFECTS.md"], effects), ["neon"])
            self.assertEqual(check.changed_effects(["src/main.rs"], effects), [])

    def test_every_folder_must_load_and_none_be_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            bundled = Path(directory) / "effects"
            for name in ("drop", "neon"):
                (bundled / name).mkdir(parents=True)
            listing = {"refused": [], "effects": [
                {"id": "noh.classic", "folder": None, "presets": ["classique"]},
                {"id": "noh.drop", "folder": str(bundled / "drop"), "presets": ["white"]},
                {"id": "noh.neon", "folder": str(bundled / "neon"), "presets": ["a", "b"]},
            ]}
            loaded = check.check_listing(listing, bundled)
            self.assertEqual(sorted(loaded), ["drop", "neon"])
            refused = {**listing, "refused": [{"folder": str(bundled / "neon"), "reason": "bad json"}]}
            with self.assertRaisesRegex(check.CheckError, "bad json"):
                check.check_listing(refused, bundled)
            missing = {**listing, "effects": listing["effects"][:2]}
            with self.assertRaisesRegex(check.CheckError, "neon did not load"):
                check.check_listing(missing, bundled)
            # An effect loaded from the user's folder does not stand for a bundled one.
            elsewhere = {**listing, "effects": listing["effects"][:2] + [
                {"id": "noh.neon", "folder": str(Path(directory) / "user" / "neon"), "presets": ["a"]}]}
            with self.assertRaisesRegex(check.CheckError, "neon did not load"):
                check.check_listing(elsewhere, bundled)

    def test_a_drawn_caption_changes_the_frame(self):
        before = bytes(1000)
        self.assertFalse(check.differs(before, before))
        self.assertFalse(check.differs(before, bytes([20]) * 1000))  # coding noise
        self.assertTrue(check.differs(before, bytes([200]) + bytes(999)))
        self.assertFalse(check.differs(bytes(100000), bytes([200]) * 49 + bytes(99951)))

    def test_most_probes_must_show_captions(self):
        blank, caption = bytes(1000), bytes([200]) + bytes(999)
        before = [blank] * len(check.PROBES)
        # One empty frame between two chunks of a line (block at 0.833 s) is not a failure.
        after = [caption] * len(check.PROBES)
        after[1] = blank
        shown, enough = check.drawn(before, after)
        self.assertTrue(enough)
        self.assertEqual(len(shown), len(check.PROBES) - 1)
        self.assertEqual(check.drawn(before, [caption, caption] + [blank] * 3), (check.PROBES[:2], False))
        self.assertEqual(check.drawn(before, before), ([], False))

    def test_every_probe_is_inside_a_cue(self):
        cues = [(int(m[0]) * 60 + int(m[1]) + int(m[2]) / 1000, int(m[3]) * 60 + int(m[4]) + int(m[5]) / 1000)
                for m in re.findall(r"00:(\d\d):(\d\d),(\d{3}) --> 00:(\d\d):(\d\d),(\d{3})", check.SRT)]
        self.assertEqual(len(cues), 2)
        for seconds in check.PROBES:
            with self.subTest(seconds=seconds):
                self.assertTrue(any(start + 0.1 < seconds < end - 0.1 for start, end in cues))


if __name__ == "__main__":
    unittest.main()
