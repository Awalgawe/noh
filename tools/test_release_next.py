import contextlib
import io
import unittest

import release_next

REPOSITORY = release_next.REPOSITORY
BUILD_JOBS = ["preflight", "build / controls", "build / rust-audit / Audit Cargo.lock against RustSec",
              "build / Candidate windows-x64", "build / Candidate linux-x64"]
COMMIT = "a" * 40


def run(run_id, workflow=release_next.RELEASE, attempt=1, conclusion="success", status="completed",
        branch="main", event="workflow_dispatch", repository=REPOSITORY):
    return {"id": run_id, "run_attempt": attempt, "head_sha": COMMIT, "path": workflow, "head_branch": branch,
            "event": event, "status": status, "conclusion": conclusion,
            "repository": {"full_name": repository}, "head_repository": {"full_name": repository}}


def jobs(names, failed=()):
    listed = [{"name": name, "conclusion": "failure" if name in failed else "success", "steps": []} for name in names]
    linux = next((job for job in listed if job["name"] == "build / Candidate linux-x64"), None)
    if linux:
        linux["steps"] = [{"name": "Test the same Linux candidate across the distribution matrix",
                           "conclusion": linux["conclusion"]}]
    return {"total_count": len(listed), "jobs": listed}


class FakeGitHub:
    def __init__(self, runs, attempts):
        self.runs, self.attempts, self.routes = runs, attempts, []

    def request(self, route):
        self.routes.append(route)
        parts = route.strip("/").split("/")
        if len(parts) == 3:
            return self.runs[int(parts[2])]
        return self.attempts[(int(parts[2]), int(parts[4]))]


def github():
    return FakeGitHub(
        {100: run(100, attempt=2), 200: run(200), 300: run(300, release_next.DESKTOP, attempt=3)},
        {(100, 2): jobs(BUILD_JOBS), (200, 1): jobs(["preflight", "installer / public-installer"])})


def said(argv, gh):
    with contextlib.redirect_stdout(io.StringIO()) as out:
        release_next.main(argv, gh)
    return out.getvalue().splitlines()


class ReleaseNext(unittest.TestCase):
    def test_after_a_build_the_installer_and_desktop_dispatches_name_its_latest_attempt(self):
        gh = github()
        lines = [line for line in said(["after-build", "100"], gh) if not line.startswith("#")]
        self.assertEqual(lines, [
            f"gh workflow run release.yml --repo {REPOSITORY} --ref main -f stage=installer -f target=windows-x64"
            f" -f source_run_id=100 -f source_attempt=2 -f source_commit={COMMIT}",
            f"gh workflow run linux-desktop-probe.yml --repo {REPOSITORY} --ref main -f suite=all"
            f" -f source_run=100 -f source_attempt=2 -f source_commit={COMMIT}",
        ])
        self.assertIn("/actions/runs/100/attempts/2/jobs?per_page=100", gh.routes)

    def test_before_publication_the_rehearsal_and_the_publication_name_all_three_runs(self):
        lines = [line for line in said(["before-publish", "100", "200", "300"], github()) if not line.startswith("#")]
        inputs = (f"-f source_run_id=100 -f source_attempt=2 -f source_commit={COMMIT}"
                  " -f installer_run_id=200 -f installer_attempt=1 -f desktop_run_id=300")
        self.assertEqual(lines, [
            f"gh workflow run release.yml --repo {REPOSITORY} --ref main -f stage=rehearse-publish"
            f" -f target=windows-linux {inputs}",
            f"gh workflow run release.yml --repo {REPOSITORY} --ref main -f stage=publish -f target=windows-linux {inputs}",
        ])

    def test_a_run_that_is_not_finished_successful_or_on_main_is_refused(self):
        cases = {
            "has not succeeded": run(100, attempt=2, conclusion="failure"),
            "has not succeeded ": run(100, attempt=2, status="in_progress", conclusion=None),
            "is not a dispatch": run(100, attempt=2, branch="claude/x"),
            "is not a dispatch ": run(100, attempt=2, event="push"),
            "is not a dispatch  ": run(100, attempt=2, repository="someone/noh"),
            "is not a dispatch   ": run(100, attempt=2, workflow=".github/workflows/pr.yml"),
        }
        for message, record in cases.items():
            with self.subTest(message=message):
                gh = github()
                gh.runs[100] = record
                with self.assertRaisesRegex(ValueError, message.strip()):
                    said(["after-build", "100"], gh)

    def test_a_run_of_another_stage_is_refused(self):
        gh = github()
        with self.assertRaisesRegex(ValueError, "Run 200 is not a successful Windows and Linux build"):
            said(["after-build", "200"], gh)
        with self.assertRaisesRegex(ValueError, "Run 100 is not a successful installer stage"):
            said(["before-publish", "100", "100", "300"], gh)
        with self.assertRaisesRegex(ValueError, "Run 200 is not a dispatch of .github/workflows/linux-desktop-probe.yml"):
            said(["before-publish", "100", "200", "200"], gh)
        gh.attempts[(100, 2)] = jobs(BUILD_JOBS, failed={"build / Candidate linux-x64"})
        with self.assertRaisesRegex(ValueError, "Run 100 is not a successful Windows and Linux build"):
            said(["after-build", "100"], gh)

    def test_only_a_run_number_is_asked_for(self):
        gh = github()
        for value in ("12x", "../100", "100 -f stage=publish"):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Not a run ID"):
                said(["after-build", value], gh)
        self.assertEqual(gh.routes, [])

    def test_github_is_read_through_gh_api_on_this_repository_only(self):
        calls = []

        def fake_run(command, **kwargs):
            calls.append((command, kwargs))
            return type("Result", (), {"stdout": '{"ok": true}'})()

        original = release_next.subprocess.run
        release_next.subprocess.run = fake_run
        try:
            self.assertEqual(release_next.GhApi().request("/actions/runs/1"), {"ok": True})
        finally:
            release_next.subprocess.run = original
        self.assertEqual(calls[0][0], ["gh", "api", f"repos/{REPOSITORY}/actions/runs/1"])
        self.assertTrue(calls[0][1]["check"])


if __name__ == "__main__":
    unittest.main()
