"""Controls for release publication and promotion; no network and no real release."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import release_publication as publication

COMMIT = "a" * 40
WRAPPER = "b" * 40


def spec(name, seed, size=10):
    return {"name": name, "sha256": seed * 64, "size": size}


def identity():
    return {"commit": COMMIT, "build_run": 11, "build_attempt": 1, "installer_run": 22, "installer_attempt": 2,
            "wrapper_commit": WRAPPER, "version": "1.2.3", "portable_sha256": "c" * 64, "delivery_sha256": "d" * 64,
            "linux_delivery_sha256": "e" * 64, "desktop_run": 44, "desktop_attempt": 1, "desktop_cases": 7,
            "desktop_case_attempts": {"arch": 1, "debian-13": 1}, "changes_commit": COMMIT}


def report():
    artifacts = {profile: {"compiler": {"pins": {}}, "installer": spec(f"NOH-1.2.3-windows-x64-{profile.title()}-Setup.exe", str(index)),
                           "materials": spec(f"NOH-1.2.3-windows-x64-{profile}-installer-sources.zip", "abcdef"[index])}
                 for index, profile in enumerate(publication.PROFILES)}
    artifacts["maintenance"] = {"compiler": {"pins": {}}, "installer": spec("noh-components.exe", "7"),
                                "materials": spec("NOH-1.2.3-windows-x64-maintenance-installer-sources.zip", "8")}
    return {"status": "passed", "source_commit": COMMIT, "source_run_id": "11", "source_run_attempt": "1",
            "run_id": "22", "run_attempt": "2", "wrapper_commit": WRAPPER, "version": "1.2.3",
            "portable_sha256": "c" * 64, "delivery_sha256": "d" * 64, "manifest_sha256": "e" * 64,
            "component_catalog_sha256": "f" * 64,
            "profiles": [{"profile": p, "status": "passed", "installed_gui_passed": True}
                         for p in ("minimal", "standard", "complete")],
            "additions": {"status": "passed"}, "upgrade": {"status": "passed", "from": "v1.2.2"},
            "artifacts": artifacts, "limitations": ["Silent installation only."]}


def catalog():
    return {"version": "1.2.3", "source_commit": COMMIT,
            "groups": {"media": {"name": "NOH-windows-x64-media-0123.zip", "sha256": "9" * 64, "size": "20"},
                       "speech": {"name": "NOH-windows-x64-speech-4567.zip", "sha256": "0" * 64, "size": "30"}}}


class Publication(unittest.TestCase):
    def test_a_report_of_this_build_and_run_is_accepted(self):
        publication.validate_report(report(), identity())

    def test_a_report_of_another_build_run_or_outcome_is_refused(self):
        changes = [("status", "failed"), ("source_commit", "0" * 40), ("source_run_id", "12"), ("run_id", "23"),
                   ("run_attempt", "1"), ("wrapper_commit", COMMIT), ("version", "1.2.4"),
                   ("portable_sha256", "0" * 64), ("delivery_sha256", "0" * 64)]
        for key, value in changes:
            altered = report()
            altered[key] = value
            with self.assertRaisesRegex(ValueError, key, msg=key):
                publication.validate_report(altered, identity())

    def test_every_profile_the_additions_and_the_upgrade_must_have_passed(self):
        cases = {}
        cases["profile failed"] = report()
        cases["profile failed"]["profiles"][1]["status"] = "failed"
        cases["interface not started"] = report()
        cases["interface not started"]["profiles"][0]["installed_gui_passed"] = False
        cases["profile missing"] = report()
        cases["profile missing"]["profiles"].pop()
        cases["additions"] = report()
        cases["additions"]["additions"] = {"status": "failed"}
        cases["no upgrade"] = report()
        del cases["no upgrade"]["upgrade"]
        for name, altered in cases.items():
            with self.assertRaises(ValueError, msg=name):
                publication.validate_report(altered, identity())

    def test_published_installer_files_follow_the_recorded_digests(self):
        assets = publication.installer_assets(report(), catalog())
        self.assertEqual(len(assets), 4 + 5 + 2)
        self.assertNotIn("noh-components.exe", assets)
        self.assertIn("NOH-1.2.3-windows-x64-maintenance-installer-sources.zip", assets)
        self.assertEqual(assets["NOH-windows-x64-speech-4567.zip"], {"sha256": "0" * 64, "size": 30})
        self.assertEqual(assets["NOH-1.2.3-windows-x64-Web-Setup.exe"], {"sha256": "0" * 64, "size": 10})

    def test_a_catalog_of_another_build_or_a_duplicate_name_is_refused(self):
        for key, value in (("version", "1.2.4"), ("source_commit", "0" * 40)):
            other = catalog()
            other[key] = value
            with self.assertRaisesRegex(ValueError, "Component catalog differs"):
                publication.installer_assets(report(), other)
        extra = catalog()
        extra["groups"]["fonts"] = spec("fonts.zip", "1")
        with self.assertRaisesRegex(ValueError, "Component catalog differs"):
            publication.installer_assets(report(), extra)
        duplicate = catalog()
        duplicate["groups"]["media"]["name"] = "NOH-1.2.3-windows-x64-Web-Setup.exe"
        with self.assertRaisesRegex(ValueError, "duplicate"):
            publication.installer_assets(report(), duplicate)
        unsafe = report()
        unsafe["artifacts"]["web"]["installer"]["name"] = "../Setup.exe"
        with self.assertRaisesRegex(ValueError, "Invalid"):
            publication.installer_assets(unsafe, catalog())

    def test_checksums_list_every_file_but_themselves_in_name_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            for name in ("b.zip", "A.json", "SHA256SUMS"):
                (folder / name).write_text(name, encoding="utf-8")
            lines = publication.checksums(folder).splitlines()
            self.assertEqual([line.split("  ")[1] for line in lines], ["A.json", "b.zip"])
            self.assertTrue(all(len(line.split("  ")[0]) == 64 for line in lines))

    def test_the_changelog_entry_must_exist_once_and_speak_as_released(self):
        text = "# Changelog\n\n## 1.2.3\n\n- New effect.\n\n## 1.2.2 — 2026-01-01\n\n- Older.\n"
        self.assertEqual(publication.changelog_section(text, "1.2.3"), "- New effect.")
        self.assertEqual(publication.changelog_section(text, "1.2.2"), "- Older.")
        # A leading paragraph pointing to the downloads is not repeated on the release page itself.
        linked = "## 1.2.3\n\nDownload the [packages](https://example.invalid/v1.2.3).\nSecond line.\n\n- New effect.\n"
        self.assertEqual(publication.changelog_section(linked, "1.2.3"), "- New effect.")
        with self.assertRaises(ValueError):
            publication.changelog_section("## 1.2.3\n\nDownload the [packages](https://example.invalid).\n", "1.2.3")
        for bad in ("# Changelog\n\n## 1.2.3 — unreleased\n\n- New effect.\n", "# Changelog\n\n## 1.2.30\n\n- Other.\n",
                    "## 1.2.3\n\n## 1.2.2\n\n- Older.\n", "## 1.2.3\n\n- a\n\n## 1.2.3\n\n- b\n"):
            with self.assertRaises(ValueError, msg=bad):
                publication.changelog_section(bad, "1.2.3")

    def test_notes_open_with_the_pending_status_and_state_the_limits(self):
        notes = publication.release_notes("1.2.3", identity(), ["Silent installation only."], "- New effect.")
        self.assertTrue(notes.startswith(publication.PENDING + "\n"))
        for expected in ("actions/runs/11", "actions/runs/22", "actions/runs/44", "7 desktops", COMMIT,
                         "- Silent installation only.", "No hardware GPU", "- New effect.",
                         "blob/v1.2.3/docs/INSTALLING.md", "PUBLICATION.json"):
            self.assertIn(expected, notes)

    def test_notes_say_when_the_changes_were_written_after_the_build(self):
        same = publication.release_notes("1.2.3", identity() | {"changes_commit": COMMIT}, [], "- New effect.")
        self.assertNotIn("corrected after the build", same)
        later = publication.release_notes("1.2.3", identity() | {"changes_commit": "f" * 40}, [], "- New effect.")
        self.assertIn("corrected after the build and is read from commit `" + "f" * 40 + "`", later)
        self.assertLess(later.index("### Changes"), later.index("corrected after the build"))
        self.assertLess(later.index("corrected after the build"), later.index("- New effect."))

    def test_notes_name_a_desktop_that_passed_only_when_re_run(self):
        once = publication.release_notes("1.2.3", identity(), [], "- New effect.")
        self.assertNotIn("re-run", once)
        again = identity() | {"desktop_attempt": 2, "desktop_case_attempts": {"arch": 1, "debian-13": 2}}
        notes = publication.release_notes("1.2.3", again, [], "- New effect.")
        self.assertIn("failed first and passed when re-run in the same run: debian-13 (attempt 2).", notes)
        self.assertNotIn("arch (attempt", notes)
        with mock.patch.dict("os.environ", {"GITHUB_RUN_ID": "55", "GITHUB_RUN_ATTEMPT": "1"}):
            record = publication.publication_record(again, 9, [])
        self.assertEqual(record["desktop_case_attempts"], {"arch": 1, "debian-13": 2})
        self.assertEqual(record["changes_commit"], COMMIT)

    def test_provenance_binds_both_runs_and_every_profile(self):
        record = publication.installer_provenance(report(), identity())
        self.assertEqual((record["build_run"], record["installer_run"], record["wrapper_commit"]), (11, 22, WRAPPER))
        self.assertEqual(set(record["profiles"]), set(publication.PROFILES) | {"maintenance"})
        self.assertEqual(record["validation"]["upgrade_from"], "v1.2.2")
        self.assertEqual(record["limits"], ["Silent installation only."])

    def acceptance(self):
        names = [f"NOH-1.2.3-windows-x64-{p}-Setup.exe" for p in ("Web", "Minimal", "Standard", "Complete")]
        names += ["NOH-1.2.3-windows-x64.zip", "NOH-1.2.3-linux-x64.zip",
                  "NOH-windows-x64-media-0123abcd.zip", "NOH-windows-x64-speech-4567ef01.zip"]
        verified = {name: "sha256:" + format(index, "x") * 64 for index, name in enumerate(names)}
        verified["SHA256SUMS"] = "sha256:" + "a" * 64
        release = {"id": 5, "tag_name": "v1.2.3"}
        report_ = {"schema_version": 1, "status": "passed", "tag": "v1.2.3", "release_id": 5,
                   "run_id": "33", "run_attempt": "1", "release_files": dict(verified),
                   "exercised_files": {name: verified[name] for name in names},
                   "platforms": {"windows-x64": {"status": "passed"}, "linux-x64": {"status": "passed"}}}
        return report_, release, {"run": "33", "attempt": "1"}, verified

    def test_acceptance_of_the_verified_files_allows_promotion(self):
        publication.validate_acceptance(*self.acceptance())

    def test_acceptance_of_other_files_another_run_or_a_partial_exercise_is_refused(self):
        def altered(change):
            accepted, release, run, verified = copy.deepcopy(self.acceptance())
            change(accepted, release, run, verified)
            return accepted, release, run, verified
        cases = {
            "failed": lambda a, r, i, v: a.update(status="failed"),
            "other tag": lambda a, r, i, v: a.update(tag="v1.2.4"),
            "other release": lambda a, r, i, v: a.update(release_id=6),
            "other run": lambda a, r, i, v: i.update(run="34"),
            "file replaced after publication": lambda a, r, i, v: v.update({"SHA256SUMS": "sha256:" + "9" * 64}),
            "file added after publication": lambda a, r, i, v: a["release_files"].update(extra="sha256:" + "8" * 64),
            "platform failed": lambda a, r, i, v: a["platforms"]["linux-x64"].update(status="failed"),
            "platform missing": lambda a, r, i, v: a["platforms"].pop("windows-x64"),
            "portable not exercised": lambda a, r, i, v: a["exercised_files"].pop("NOH-1.2.3-windows-x64.zip"),
            "component not exercised": lambda a, r, i, v: a["exercised_files"].pop("NOH-windows-x64-speech-4567ef01.zip"),
            "exercised another file": lambda a, r, i, v: a["exercised_files"].update(
                {"NOH-1.2.3-linux-x64.zip": "sha256:" + "7" * 64}),
        }
        for name, change in cases.items():
            with self.assertRaises(ValueError, msg=name):
                publication.validate_acceptance(*altered(change))

    def test_a_desktop_case_must_have_tested_this_linux_package(self):
        candidate = {"source_commit": COMMIT, "run_id": 11, "run_attempt": 1, "delivery_sha256": "e" * 64}
        publication.validate_desktop_candidate(candidate, identity())
        for key, value in (("source_commit", "0" * 40), ("run_id", 12), ("run_attempt", 2), ("delivery_sha256", "0" * 64)):
            with self.assertRaisesRegex(ValueError, key):
                publication.validate_desktop_candidate({**candidate, key: value}, identity())
        self.assertEqual(len(publication.desktop_cases()), 7)

    def test_the_publication_record_lists_every_uploaded_file(self):
        import os
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            files = []
            for name in ("SHA256SUMS", "NOH-1.2.3-linux-x64.zip"):
                files.append(Path(temporary) / name)
                files[-1].write_text(name, encoding="utf-8")
            with patch.dict(os.environ, {"GITHUB_RUN_ID": "55", "GITHUB_RUN_ATTEMPT": "1"}):
                record = publication.publication_record(identity(), 5, files)
        self.assertEqual((record["tag"], record["release_id"], record["run_id"]), ("v1.2.3", 5, "55"))
        self.assertEqual(set(record["assets"]), {"SHA256SUMS", "NOH-1.2.3-linux-x64.zip"})
        self.assertTrue(all(value.startswith("sha256:") for value in record["assets"].values()))

    def test_release_listings_follow_every_page(self):
        from unittest.mock import Mock
        github = Mock()
        github.request.side_effect = [[{"id": n} for n in range(100)], [{"id": 100}]]
        self.assertEqual(len(publication.all_releases(github)), 101)
        self.assertEqual(github.request.call_count, 2)

    def finalizing(self, root, **draft_changes):
        import json, os
        from types import SimpleNamespace
        from unittest.mock import Mock
        record_path = root / "PUBLICATION.json"
        record = {"tag": "v1.2.3", "release_id": 5, "source_commit": COMMIT, "run_id": "55", "run_attempt": "1",
                  "assets": {"SHA256SUMS": "sha256:" + "1" * 64}}
        record_path.write_text(json.dumps(record), encoding="utf-8")
        draft = {"id": 5, "draft": True, "published_at": None, "tag_name": "v1.2.3", "target_commitish": COMMIT,
                 "assets": [{"name": "SHA256SUMS", "digest": "sha256:" + "1" * 64},
                            {"name": "PUBLICATION.json", "digest": "sha256:" + publication.delivery.digest(record_path)}],
                 **draft_changes}
        calls = []
        def request(route, method="GET", body=None):
            calls.append((route, method, body))
            if method == "PATCH":
                return {"draft": False, "prerelease": True, "tag_name": "v1.2.3", "html_url": "https://example.invalid/r"}
            return {"": {"id": publication.REPOSITORY_ID}, "/releases/5": draft, "/git/matching-refs/tags/v1.2.3": [],
                    "/releases/latest": {"tag_name": "v1.2.2"}}[route]
        github = Mock()
        github.request.side_effect = request
        environment = {"GITHUB_REF": "refs/heads/main", "GITHUB_RUN_ID": "55", "GITHUB_RUN_ATTEMPT": "1"}
        return github, calls, SimpleNamespace(record=record_path), environment

    def test_finalize_publishes_only_the_unchanged_staged_draft_of_its_own_run(self):
        import os
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            github, calls, args, environment = self.finalizing(Path(temporary))
            with patch.object(publication, "GitHub", return_value=github), patch.dict(os.environ, environment):
                publication.finalize(args)
            patches = [call for call in calls if call[1] == "PATCH"]
            self.assertEqual(patches, [("/releases/5", "PATCH", {"draft": False, "prerelease": True, "make_latest": "false"})])
        refusals = {
            "already published": {"draft": False},
            "other commit": {"target_commitish": "0" * 40},
            "file replaced": {"assets": [{"name": "SHA256SUMS", "digest": "sha256:" + "9" * 64}]},
        }
        for name, change in refusals.items():
            with tempfile.TemporaryDirectory() as temporary:
                github, calls, args, environment = self.finalizing(Path(temporary), **change)
                with patch.object(publication, "GitHub", return_value=github), patch.dict(os.environ, environment):
                    with self.assertRaises(ValueError, msg=name):
                        publication.finalize(args)
                self.assertFalse([call for call in calls if call[1] == "PATCH"], name)
        with tempfile.TemporaryDirectory() as temporary:
            # After publication, a failed read of the latest release is a warning, not a lost version.
            github, calls, args, environment = self.finalizing(Path(temporary))
            answer = github.request.side_effect
            def flaky(route, method="GET", body=None):
                if route == "/releases/latest":
                    calls.append((route, method, body))
                    raise ValueError("GitHub GET /releases/latest: HTTP 502")
                return answer(route, method, body)
            github.request.side_effect = flaky
            with patch.object(publication, "GitHub", return_value=github), patch.dict(os.environ, environment):
                publication.finalize(args)
            self.assertEqual(len([call for call in calls if call[0] == "/releases/latest"]), 3)
        with tempfile.TemporaryDirectory() as temporary:
            github, calls, args, environment = self.finalizing(Path(temporary))
            with patch.object(publication, "GitHub", return_value=github), \
                    patch.dict(os.environ, {**environment, "GITHUB_RUN_ID": "56"}):
                with self.assertRaisesRegex(ValueError, "not this run's"):
                    publication.finalize(args)

    def test_the_publishing_run_is_trusted_by_its_artifact_not_by_its_conclusion(self):
        from unittest.mock import Mock
        run = {"repository": {"full_name": publication.REPOSITORY}, "head_repository": {"full_name": publication.REPOSITORY},
               "workflow_id": 7, "path": ".github/workflows/release.yml", "head_branch": "main",
               "event": "workflow_dispatch", "status": "completed", "conclusion": "failure", "run_attempt": 1,
               "head_sha": COMMIT}
        github = Mock()
        github.request.side_effect = lambda route: run if route.startswith("/actions/runs/") else {"status": "identical"}
        self.assertIs(publication.release_run(github, 55, 1, 7, successful=False), run)
        with self.assertRaisesRegex(ValueError, "Not a completed Release run"):
            publication.release_run(github, 55, 1, 7)
        run["status"] = "in_progress"
        with self.assertRaisesRegex(ValueError, "Not a completed Release run"):
            publication.release_run(github, 55, 1, 7, successful=False)

    def test_names_shared_with_the_workflows(self):
        self.assertEqual(publication.installer_draft_tag("1.2.3", 22, 2), "profile-installer-check-v1.2.3-22-2")
        self.assertEqual(publication.evidence_artifact_name(22, 2), "installer-evidence-22-2")
        self.assertEqual(publication.acceptance_artifact_name(33, 1), "acceptance-33-1")
        self.assertEqual(publication.publication_artifact_name(55, 1), "publication-55-1")
        self.assertEqual(publication.desktop_artifact_name("arch", 44, 1), "linux-desktop-arch-44-1")


if __name__ == "__main__":
    unittest.main()
