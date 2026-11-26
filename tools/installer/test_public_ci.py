"""Exercise rejection paths without installing software or using the network."""
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import subprocess
import unittest
import zipfile
import unittest.mock
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("public_ci", Path(__file__).with_name("public-ci.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)
spec = importlib.util.spec_from_file_location('public_installer', Path(__file__).resolve().parents[1] / 'public-installer.py')
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)


class InstallerGuards(unittest.TestCase):
    def test_prepare_preserves_commit_identity_across_zip_extraction(self):
        for platforms in (['windows-x64'], ['windows-x64', 'linux-x64'], list(ci.delivery.PLATFORMS)):
            with self.subTest(platforms=platforms):
                self.prepare_case(platforms)

    def test_combined_run_rejects_missing_failed_or_skipped_linux_before_download(self):
        for conclusion in ('missing', 'failure', 'skipped'):
            for platforms in (['windows-x64', 'linux-x64'], list(ci.delivery.PLATFORMS)):
                with self.subTest(platforms=platforms, conclusion=conclusion):
                    self.prepare_case(platforms, conclusion)

    def prepare_case(self, platforms, linux_result='success'):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = 'a' * 40
            inner = io.BytesIO()
            with zipfile.ZipFile(inner, 'w') as archive:
                archive.writestr('manifest.json', '{}')
            portable_name = 'NOH-0.1.1-windows-x64.zip'
            run = {'repository': {'full_name': 'Awalgawe/noh'}, 'head_repository': {'full_name': 'Awalgawe/noh'},
                   'workflow_id': 77, 'head_sha': source, 'head_branch': 'main', 'event': 'workflow_dispatch',
                   'status': 'completed', 'conclusion': 'success', 'run_attempt': 1, 'path': '.github/workflows/release.yml'}
            names = ['preflight', 'build / controls', 'build / rust-audit / Audit Cargo.lock against RustSec']
            jobs = [{'name': name, 'conclusion': 'success'} for name in names]
            for platform in platforms:
                if platform == 'linux-x64' and linux_result == 'missing':
                    continue
                jobs.append({'name': 'build / Candidate ' + platform,
                             'conclusion': linux_result if platform == 'linux-x64' else 'success'})
                if platform == 'linux-x64':
                    jobs[-1]['steps'] = [{'name': 'Test the same Linux candidate across the distribution matrix',
                                         'conclusion': linux_result}]
            responses = {'': {'id': 1411000018}, '/actions/runs/1': run,
                '/compare/' + source + '...main': {'status': 'identical'},
                '/actions/runs/1/attempts/1/jobs?per_page=100': {'total_count': len(jobs),
                    'jobs': jobs},
                '/actions/runs/1/artifacts?per_page=100': {'total_count': len(platforms), 'artifacts': [
                    {'id': 9 + index, 'name': 'delivery-' + platform, 'expired': False,
                     'digest': 'sha256:' + 'b' * 64} for index, platform in enumerate(platforms)]}}
            if 'linux-x64' in platforms:
                inventory = responses['/actions/runs/1/artifacts?per_page=100']
                inventory['artifacts'].append({'id': 20, 'name': 'linux-validation-1-1',
                                              'expired': False, 'digest': 'sha256:' + 'd' * 64})
                inventory['total_count'] += 1

            def download(route, target, digest):
                self.assertEqual(route, '/actions/artifacts/9/zip')
                self.assertEqual(digest, 'b' * 64)
                with zipfile.ZipFile(target, 'w') as archive:
                    archive.writestr(portable_name, inner.getvalue())
                    archive.writestr('DELIVERY.json', '{}')

            with patch.object(ci, 'GitHub') as github, \
                    patch.object(ci, 'fetch_upgrade_baseline') as baseline, \
                    patch.object(ci.delivery, 'authorize_redistribution'), \
                    patch.object(ci.delivery, 'verify_envelope', return_value={
                        'source_commit': source, 'run_id': '1', 'run_attempt': '1',
                        'platform': 'windows-x64', 'version': '0.1.1'}), \
                    patch.object(ci.delivery, 'verify_bundle') as verify, \
                    patch.object(ci.subprocess, 'check_output', side_effect=lambda args, **kw:
                        (ci.ROOT / args[-1].split(':', 1)[1]).read_bytes()), \
                    patch.dict(os.environ, {'NOH_SOURCE_COMMIT': source, 'NOH_SOURCE_RUN': '1',
                        'NOH_SOURCE_ATTEMPT': '1', 'NOH_DELIVERY_WORKFLOW_ID': '77', 'GITHUB_SHA': 'c' * 40}):
                github.return_value.request.side_effect = responses.__getitem__
                github.return_value.download.side_effect = download
                if linux_result != 'success':
                    with self.assertRaisesRegex(ValueError, 'Missing/skipped/failed'):
                        ci.prepare(root / 'work')
                    github.return_value.download.assert_not_called()
                    return
                ci.prepare(root / 'work')
                github.return_value.download.assert_called_once()
                baseline.assert_called_once_with(root / 'work', github.return_value, '0.1.1')
                verify.assert_called_once_with(root / 'work/portable', 'x86_64-pc-windows-gnu', source)
                self.assertEqual(ci.delivery.read(root / 'work/SOURCE.json')['source_commit'], source)

    def test_web_cannot_embed_a_full_installer_or_be_a_maintenance_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for profile, maintenance in [('complete', False), ('standard', False), ('minimal', True)]:
                with self.assertRaisesRegex(ValueError, 'Web setup embeds only'):
                    web.build(root, 'a' * 40, root, root / 'output', profile=profile,
                              maintenance=maintenance, web=True)
            self.assertFalse((root / 'output').exists())

    def test_public_installer_messages_cover_all_seven_languages(self):
        translations = {}
        for line in Path(__file__).with_name('public-messages.iss').read_text(encoding='utf-8-sig').splitlines():
            if '=' in line:
                name, value = line.split('=', 1)
                language, key = name.split('.')
                translations.setdefault(language, {})[key] = value
        self.assertEqual(set(translations), {'en', 'fr', 'de', 'es', 'ja', 'ko', 'zh'})
        for language, values in translations.items():
            self.assertEqual(set(values), set(translations['en']), language)
            self.assertTrue(all(values.values()))

    def test_a_missing_opengl_renderer_fails_the_interface_check(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def known_failure(args, **kwargs):
                kwargs['stdout'].write(b'Error: OpenGL(PainterError("egui_glow requires opengl 2.0+. "))\n')
                return subprocess.CompletedProcess(args, 1)
            with patch.object(ci.subprocess, 'run', side_effect=known_failure):
                with self.assertRaisesRegex(ValueError, 'unexpected exit'):
                    ci.run(root, 'gui', ['unused'])

    def test_the_upgrade_baseline_is_the_recorded_published_installer(self):
        baseline = ci.upgrade_baseline()
        self.assertEqual(baseline['tag'], 'v0.5.0')
        self.assertEqual(baseline['name'], 'NOH-0.5.0-windows-x64-Complete-Setup.exe')
        # From 0.1.6 a publication record is a file of the release (PUBLICATION.json), not of this
        # repository: the pin's digest and size were read there when the pin was moved.
        valid = {'schema': 1, 'tag': 'v0.5.0', 'installer': baseline['name'],
                 'sha256': baseline['sha256'], 'size': baseline['size']}
        with tempfile.TemporaryDirectory() as temporary:
            lock = Path(temporary) / 'lock.json'
            lock.write_text(json.dumps(valid), encoding='utf-8')
            self.assertEqual(ci.upgrade_baseline(lock), baseline)
            for changes in ({'tag': 'v0.4.0'}, {'installer': 'NOH-0.5.0-windows-x64-Minimal-Setup.exe'},
                            {'sha256': 'abc'}, {'size': '1235231086'}, {'size': 0}, {'schema': 2}):
                lock.write_text(json.dumps({**valid, **changes}), encoding='utf-8')
                with self.assertRaises(ValueError, msg=str(changes)):
                    ci.upgrade_baseline(lock)

    def test_the_baseline_download_is_bound_to_the_pin_and_to_a_published_release(self):
        pin = ci.upgrade_baseline()
        release = {'draft': False, 'prerelease': False, 'assets': [{'id': 5, 'name': pin['name']},
                                                                    {'id': 6, 'name': 'SHA256SUMS'}]}
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            github = unittest.mock.Mock()
            github.request.return_value = release
            ci.fetch_upgrade_baseline(work, github, '0.2.1')
            github.request.assert_called_once_with('/releases/tags/' + pin['tag'])
            github.download.assert_called_once_with('/releases/assets/5', work / 'baseline' / pin['name'],
                                                    pin['sha256'], pin['size'])
            for change, message in (({'prerelease': True}, 'not a published release'),
                                    ({'draft': True}, 'not a published release'),
                                    ({'assets': []}, 'missing or ambiguous')):
                github.request.return_value = {**release, **change}
                with self.assertRaisesRegex(ValueError, message):
                    ci.fetch_upgrade_baseline(work, github, '0.2.1')
            with self.assertRaisesRegex(ValueError, 'version under test'):
                ci.fetch_upgrade_baseline(work, github, pin['tag'][1:])

    def test_the_tests_run_without_github_access(self):
        # The test step has no token, and the installed application must never see one:
        # downloads belong to the prepare step.
        import inspect
        for function in (ci.test, ci.test_profile, ci.test_additions, ci.test_upgrade, ci.start_installed):
            self.assertNotIn('GitHub(', inspect.getsource(function), function.__name__)
        self.assertIn('fetch_upgrade_baseline(work, github', inspect.getsource(ci.prepare))

    def test_local_host_is_rejected(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "disposable hosted Windows"):
                ci.runner()

    def test_installed_payload_changes_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "noh.exe").write_bytes(b"qualified application")
            expected = ci.delivery.inventory(root)
            (root / "unins000.exe").write_bytes(b"uninstaller")
            ci.verify_installed(root, expected)
            (root / "noh.exe").write_bytes(b"changed application")
            with self.assertRaisesRegex(ValueError, "payload differs"):
                ci.verify_installed(root, expected)

    def test_extra_payload_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "noh.exe").write_bytes(b"qualified application")
            expected = ci.delivery.inventory(root)
            (root / "unins000.exe").write_bytes(b"uninstaller")
            (root / "extra.dll").write_bytes(b"unexpected native dependency")
            with self.assertRaisesRegex(ValueError, "Unexpected installed files"):
                ci.verify_installed(root, expected)

    def test_upgrade_check_still_rejects_a_retired_file_left_behind(self):
        # The installer deletes the files earlier releases shipped and this one does not:
        # the upgrade check must keep refusing them, never list them as allowed extras.
        lines = web.retired_lines({"noh.exe": {}})
        for name in web.RETIRED:
            self.assertIn('Type: files; Name: "{app}\\' + name.replace("/", "\\") + '"', lines)
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "noh.exe").write_bytes(b"qualified application")
                expected = ci.delivery.inventory(root)
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_bytes(b"left by an earlier release")
                with self.assertRaisesRegex(ValueError, "Unexpected installed files"):
                    ci.verify_installed(root, expected)


if __name__ == "__main__":
    unittest.main()
