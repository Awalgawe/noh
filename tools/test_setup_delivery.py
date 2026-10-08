"""Boundary tests for baseline reuse, safe extraction and private draft identity."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

import setup_delivery as delivery


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        scratch = Path(__file__).resolve().parent.parent / ".mcp-dev"
        scratch.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="setup-transport-test-", dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_asset_replacement_is_not_a_cache_hit(self):
        expected = {"id": 1, "name": "Setup.exe", "size": 3, "sha256": "a" * 64}
        actual = {"id": 1, "name": "Setup.exe", "size": 3, "digest": "sha256:" + "a" * 64, "state": "uploaded"}
        delivery.validate_asset(actual, expected)
        for field, value in (("id", 2), ("size", 4), ("digest", "sha256:" + "b" * 64), ("state", "starter")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                delivery.validate_asset(dict(actual, **{field: value}), expected)

    def test_action_archives_and_release_assets_use_their_required_media_types(self):
        seen = []
        class FakeOpener:
            def open(self, request, timeout):
                seen.append(request.get_header('Accept'))
                return io.BytesIO(b'content')
        github = delivery.GitHub.__new__(delivery.GitHub)
        github.headers = {'Accept': 'application/vnd.github+json'}
        github.opener = FakeOpener()
        expected = hashlib.sha256(b'content').hexdigest()
        github.download('/actions/artifacts/1/zip', self.root / 'artifact.zip', expected)
        github.download('/releases/assets/2', self.root / 'asset.zip', expected)
        self.assertEqual(seen, ['application/vnd.github+json', 'application/octet-stream'])

    def test_partial_historical_catalog_is_preserved_and_required_member_checked(self):
        root = self.root / "history"
        root.mkdir()
        (root / "bootstrap-B.exe").write_bytes(b"old controller")
        data = {"schema": 1, "repository": delivery.REPOSITORY, "package_id": "NOH", "source_commit": "a" * 40,
                "version_b": "0.1.1", "release_tag": "v0.1.1", "files": [
                    {"path": "bootstrap-A.exe", "sha256": "b" * 64},
                    {"path": "bootstrap-B.exe", "sha256": delivery.digest(root / "bootstrap-B.exe")}]}
        delivery.write(root / "candidate.json", data)
        original = delivery.digest(root / "candidate.json")
        delivery.catalog(root, "a" * 40, original, ["bootstrap-B.exe"])
        self.assertEqual(original, delivery.digest(root / "candidate.json"))
        (root / "bootstrap-B.exe").write_bytes(b"replaced controller")
        with self.assertRaises(ValueError):
            delivery.catalog(root, "a" * 40, original, ["bootstrap-B.exe"])
        with self.assertRaises(ValueError):
            delivery.catalog(root, "a" * 40, original, ["missing.exe"])

    def test_archive_paths_duplicates_and_wrong_hash_refused(self):
        for name in ("../escape", "C:/escape", "a\\escape", "a//escape"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                delivery.member(self.root, name)
        archive = self.root / "test.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("member", b"actual")
        expected = {"member": hashlib.sha256(b"actual").hexdigest()}
        delivery.extract(archive, self.root / "ok", expected, 10)
        delivery.extract(archive, self.root / "ok", expected, 10)
        with self.assertRaises(ValueError):
            delivery.extract(archive, self.root / "bad", {"member": "a" * 64}, 10)
        with self.assertRaises(ValueError):
            delivery.extract(archive, self.root / "small", expected, 2)
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("member", b"actual")
            output.writestr("MEMBER", b"actual")
        with self.assertRaises(ValueError):
            delivery.extract(archive, self.root / "duplicate", expected, 20)

    def baseline_fixture(self):
        assets = [{"id": n + 1, "name": f"asset-{n}", "size": n, "sha256": "a" * 64} for n in range(6)]
        support = {"id": 7, "name": "support.zip", "size": 1, "sha256": "a" * 64}
        lock = {"schema": 1, "repository": delivery.REPOSITORY, "repository_id": delivery.REPOSITORY_ID,
                "package_id": "NOH", "release_id": 4, "source_commit": "c" * 40, "version": "0.1.1",
                "candidate_sha256": "d" * 64, "assets": assets, "support": support}
        release = {"draft": True, "published_at": None, "tag_name": "v0.1.1", "target_commitish": "c" * 40,
                   "assets": [dict(a, digest="sha256:" + a["sha256"], state="uploaded") for a in assets + [support]]}
        class FakeGitHub:
            def request(self, route):
                return release
            def download(self, *args):
                raise AssertionError("Metadata preflight must not download historical payloads")
        return lock, release, FakeGitHub()

    def test_preflight_missing_or_changed_baseline_fails_without_download(self):
        lock, release, github = self.baseline_fixture()
        delivery.baseline(lock, github, new_version="0.1.2")
        for old in ("0.1.1", "0.1.0", "00.1.2"):
            with self.subTest(version=old), self.assertRaises(ValueError):
                delivery.baseline(lock, github, new_version=old)
        release["assets"].pop()
        with self.assertRaises(ValueError):
            delivery.baseline(lock, github, new_version="0.1.2")

    def test_draft_identity_refuses_publication_or_other_candidate(self):
        data = {"release_tag": "v0.1.2", "source_commit": "a" * 40}
        marker = "exact candidate"
        good = {"draft": True, "published_at": None, "tag_name": "v0.1.2", "target_commitish": "a" * 40, "body": marker}
        delivery.draft_identity(good, data, marker)
        for field, value in (("draft", False), ("published_at", "today"), ("target_commitish", "b" * 40), ("body", "another candidate")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                delivery.draft_identity(dict(good, **{field: value}), data, marker)

    def test_upload_resume_never_replaces_a_different_asset(self):
        path = self.root / 'installer.exe'
        path.write_bytes(b'exact final installer')
        expected = delivery.digest(path)
        asset = {'id': 5, 'name': path.name, 'size': path.stat().st_size, 'state': 'uploaded', 'digest': 'sha256:' + expected}
        github = delivery.GitHub.__new__(delivery.GitHub)
        github.request = lambda route: {'draft': True, 'published_at': None, 'assets': [asset]}
        self.assertEqual(github.upload(4, path, path.name, expected)['id'], 5)
        asset['digest'] = 'sha256:' + 'a' * 64
        with self.assertRaises(ValueError):
            github.upload(4, path, path.name, expected)

    def test_ci_cannot_create_a_missing_slot_and_can_reuse_only_exact_slot(self):
        calls = []
        releases = []
        class FakeGitHub:
            def request(self, route, method='GET', body=None):
                calls.append((route, method))
                if method != 'GET':
                    raise AssertionError('CI must not create or edit a release')
                return releases
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}):
            with self.assertRaises(ValueError):
                delivery.slot(FakeGitHub(), 'a' * 40, '0.1.2', create=True)
            releases.append({'id': 1, 'tag_name': 'v0.1.2', 'target_commitish': 'a' * 40, 'draft': True,
                             'published_at': None, 'body': delivery.slot_notes('a' * 40, '0.1.2')})
            self.assertEqual(delivery.slot(FakeGitHub(), 'a' * 40, '0.1.2')['id'], 1)
            releases[0]['target_commitish'] = 'b' * 40
            with self.assertRaises(ValueError):
                delivery.slot(FakeGitHub(), 'a' * 40, '0.1.2')
        self.assertTrue(all(method == 'GET' for route, method in calls))

    def test_failed_installer_job_can_reuse_successful_producer_attempt(self):
        archive = self.root / 'source.zip'
        key_bytes = b'{"keys": []}'
        data = {'schema': 1, 'repository': delivery.REPOSITORY, 'package_id': 'NOH', 'source_commit': 'a' * 40,
                'release_tag': 'v0.1.2', 'version_b': '0.1.2', 'files': [{'path': 'keys.json', 'sha256': hashlib.sha256(key_bytes).hexdigest()}]}
        with zipfile.ZipFile(archive, 'w') as output:
            output.writestr('candidate.json', json.dumps(data))
            output.writestr('keys.json', key_bytes)
        keys = self.root / 'trusted-keys.json'; keys.write_bytes(key_bytes)
        run = {'repository': {'id': delivery.REPOSITORY_ID}, 'head_repository': {'id': delivery.REPOSITORY_ID},
               'head_sha': 'a' * 40, 'head_branch': delivery.BRANCH, 'path': delivery.WORKFLOW,
               'run_attempt': 2, 'status': 'completed', 'conclusion': 'failure'}
        job = {'id': 3, 'name': 'candidate', 'conclusion': 'success', 'started_at': '2026-10-06T00:00:00Z', 'completed_at': '2026-10-06T00:10:00Z'}
        artifact = {'id': 4, 'name': 'noh-windows-setup-candidate', 'expired': False, 'digest': 'sha256:' + delivery.digest(archive), 'created_at': '2026-10-06T00:09:00Z'}
        class FakeGitHub:
            def request(self, route):
                if route.endswith('/artifacts?per_page=100'):
                    return {'artifacts': [artifact]}
                if route.endswith('/attempts/1/jobs?per_page=100'):
                    return {'jobs': [job]}
                return run
            def download(self, route, target, expected):
                shutil.copyfile(archive, target)
        args = SimpleNamespace(run_id=2, attempt=1, commit='a' * 40, output=self.root / 'download', public_keys=keys)
        delivery.fetch(args, FakeGitHub())
        self.assertEqual(delivery.read(args.output / 'provenance.json')['attempt'], 1)
        artifact['created_at'] = '2026-10-07T00:09:00Z'
        with self.assertRaises(ValueError):
            delivery.fetch(args, FakeGitHub())
        job['conclusion'] = 'failure'
        with self.assertRaises(ValueError):
            delivery.fetch(args, FakeGitHub())

    @unittest.skipUnless(os.name == "nt" and shutil.which("pwsh"), "Windows PowerShell runner preflight")
    def test_actual_runner_accepts_three_historical_profiles_without_local_A(self):
        old = self.root / "old"
        new = self.root / "new"
        old.mkdir(); new.mkdir()
        files = {}
        for name in ("keys.json", "source-inventory.json", "tools/bootstrap-B.exe", "tools/update-release.exe"):
            target = delivery.member(old, name)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"synthetic preflight input")
            files[name] = delivery.digest(target)
        for profile in delivery.PROFILES:
            tail = delivery.suffix(profile)
            name = f"NOH-0.1.1-win-x64{tail}-Setup.exe"
            target = delivery.member(old, f"packages-B{tail}/{name}")
            target.parent.mkdir(parents=True)
            target.write_bytes(profile.encode())
            files[f"packages-B{tail}/{name}"] = delivery.digest(target)
            release = {"profile": profile, "version": "0.1.1", "package_id": "NOH", "artifacts": [
                {"file_name": name, "sha256": delivery.digest(target)}]}
            delivery.write(old / f"release-B{tail}.json", release)
            delivery.write(old / f"envelope-B{tail}.json", {"synthetic": True})
            for path in (f"release-B{tail}.json", f"envelope-B{tail}.json"):
                files[path] = delivery.digest(old / path)
        previous = {"schema": 1, "repository": delivery.REPOSITORY, "package_id": "NOH", "source_commit": "a" * 40,
                    "version_b": "0.1.1", "files": [{"path": p, "sha256": h} for p, h in files.items()]}
        # The original catalog also names A; none of those files exists locally.
        previous["files"].append({"path": "packages-A/unused-Setup.exe", "sha256": "b" * 64})
        delivery.write(old / "candidate.json", previous)
        old_hash = delivery.digest(old / "candidate.json")
        baseline = {"version": "0.1.1", "source_commit": "a" * 40, "candidate_sha256": old_hash}
        delivery.write(new / "baseline.json", baseline)
        current = {"schema": 1, "package_id": "NOH", "source_commit": "c" * 40, "version_a": "0.1.1", "version_b": "0.1.2",
                   "baseline_sha256": delivery.digest(new / "baseline.json"), "files": [
                       {"path": "baseline.json", "sha256": delivery.digest(new / "baseline.json")}]}
        delivery.write(new / "candidate.json", current)
        command = ["pwsh", "-NoProfile", "-File", str(Path(__file__).with_name("update-setup-clients.ps1")),
                   "-Stage", "Accept", "-ValidateInputsOnly", "-CandidateDirectory", str(new), "-ExpectedCommit", "c" * 40,
                   "-PreviousCandidateDirectory", str(old), "-ExpectedPreviousCommit", "a" * 40,
                   "-ExpectedPreviousCandidateSha256", old_hash]
        for profile in delivery.PROFILES:
            result = subprocess.run(command + ["-Profile", profile], capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            record = json.loads(result.stdout)
            self.assertEqual((record["version_a"], record["version_b"], record["profile"]), ("0.1.1", "0.1.2", profile))
        changed = command.copy(); changed[-1] = "f" * 64
        result = subprocess.run(changed, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exact baseline", result.stderr)
        (old / "tools/bootstrap-B.exe").write_bytes(b"tampered")
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Changed historical", result.stderr)


if __name__ == "__main__":
    unittest.main()
