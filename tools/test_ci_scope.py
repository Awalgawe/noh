"""Check conservative platform selection and the required PR gate."""
import subprocess
import tempfile
from pathlib import Path
import unittest

import ci_scope


class ScopeTests(unittest.TestCase):
    def test_docs_need_no_native_build(self):
        self.assertEqual(ci_scope.select(["README.md", "docs/APP.md", "CONTRIBUTING.md",
                                              "SECURITY.md", ".github/pull_request_template.md"]),
                         {"windows": False, "unix": False})

    def test_windows_tools_are_isolated(self):
        self.assertEqual(ci_scope.select(["tools/windows-qa.ps1", "docs/BUILDING.md"]),
                         {"windows": True, "unix": False})

    def test_unix_checks_keep_all_unix_architectures(self):
        self.assertEqual(ci_scope.select(["tools/verify-unix.sh"]),
                         {"windows": False, "unix": True})

    def test_shared_inputs_require_every_platform(self):
        for path in ("src/media.rs", "src/update/windows.rs", "Cargo.lock", "Cargo.toml",
                     "build.rs", ".cargo/config.toml", "locales/fr.json", "assets/noh-icon.png",
                     "tools/dev.rs", "tools/delivery.py", "tests/media.rs", ".github/workflows/pr.yml",
                     "tools/ci_scope.py", "future/input.dat", "docs/embedded.bin"):
            with self.subTest(path=path):
                self.assertEqual(ci_scope.select([path]), {"windows": True, "unix": True})

    def test_mixed_changes_take_union(self):
        self.assertEqual(ci_scope.select(["tools/windows-qa.ps1", "tools/verify-unix.sh"]),
                         {"windows": True, "unix": True})

    def test_missing_comparison_builds_all(self):
        for event_name, event in (("workflow_dispatch", {}), ("push", {"before": "0" * 40}),
                                  ("pull_request", {}), ("push", {"before": "--bad"})):
            self.assertEqual(ci_scope.select(ci_scope.changed_paths(event_name, event)),
                             {"windows": True, "unix": True})

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
            self.assertEqual(ci_scope.select(paths), {"windows": True, "unix": True})
            self.assertIsNone(ci_scope.changed_paths("push", {"before": "a" * 40}, root))

    def test_gate_accepts_only_expected_success_or_skip(self):
        for windows, unix in ((False, False), (True, False), (False, True), (True, True)):
            scope = {"windows": windows, "unix": unix}
            results = {"scope": "success", "audit": "success" if windows or unix else "skipped",
                       "windows": "success" if windows else "skipped",
                       "unix": "success" if unix else "skipped"}
            self.assertTrue(ci_scope.passed(scope, results))
            for job in results:
                for failure in ("failure", "cancelled", None):
                    with self.subTest(scope=scope, job=job, failure=failure):
                        self.assertFalse(ci_scope.passed(scope, {**results, job: failure}))
            for job, needed in (("windows", windows), ("unix", unix)):
                if needed:
                    self.assertFalse(ci_scope.passed(scope, {**results, job: "skipped"}))


if __name__ == "__main__":
    unittest.main()
