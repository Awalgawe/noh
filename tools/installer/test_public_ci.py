"""Exercise rejection paths without installing software or using the network."""
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import subprocess
import unittest
import zipfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("public_ci", Path(__file__).with_name("public-ci.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)
spec = importlib.util.spec_from_file_location('public_web', Path(__file__).with_name('public-web.py'))
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)


class InstallerGuards(unittest.TestCase):
    def test_prepare_preserves_commit_identity_across_zip_extraction(self):
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
            jobs = ['preflight', 'build / controls', 'build / rust-audit / Audit Cargo.lock against RustSec',
                    'build / Candidate windows-x64']
            responses = {'': {'id': 1411000018}, '/actions/runs/1': run,
                '/compare/' + source + '...main': {'status': 'identical'},
                '/actions/runs/1/attempts/1/jobs?per_page=100': {'total_count': len(jobs),
                    'jobs': [{'name': name, 'conclusion': 'success'} for name in jobs]},
                '/actions/runs/1/artifacts?per_page=100': {'total_count': 1, 'artifacts': [
                    {'id': 9, 'name': 'delivery-windows-x64', 'expired': False, 'digest': 'sha256:' + 'b' * 64}]}}

            def download(route, target, digest):
                self.assertEqual(route, '/actions/artifacts/9/zip')
                self.assertEqual(digest, 'b' * 64)
                with zipfile.ZipFile(target, 'w') as archive:
                    archive.writestr(portable_name, inner.getvalue())
                    archive.writestr('DELIVERY.json', '{}')

            with patch.object(ci, 'GitHub') as github, \
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
                ci.prepare(root / 'work')
                verify.assert_called_once_with(root / 'work/portable', 'x86_64-pc-windows-gnu', source)
                self.assertEqual(ci.delivery.read(root / 'work/SOURCE.json')['source_commit'], source)

    def profile_records(self, root):
        for profile in ci.PROFILES:
            folder = root / profile
            folder.mkdir()
            asset = folder / f'NOH-0.1.0-windows-x64-{profile.title()}-Setup.exe'
            asset.write_bytes(profile.encode())
            ci.delivery.write(folder / 'INSTALLER.json', {
                'source_commit': '1' * 40, 'portable_manifest_sha256': '2' * 64,
                'distribution_profile': profile,
                'installer': {'name': asset.name, 'sha256': ci.delivery.digest(asset), 'size': asset.stat().st_size}})

    def test_web_rejects_changed_offline_installer_before_compiling(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.profile_records(root)
            next((root / 'minimal').glob('*.exe')).write_bytes(b'changed')
            with patch.object(web.subprocess, 'run') as compiler:
                with self.assertRaisesRegex(ValueError, 'bytes changed'):
                    web.build(root, root / 'compiler', root / 'output')
                compiler.assert_not_called()
            self.assertFalse((root / 'output').exists())

    def test_web_rejects_mixed_application_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.profile_records(root)
            path = root / 'standard/INSTALLER.json'
            record = ci.delivery.read(path)
            record['source_commit'] = 'a' * 40
            ci.delivery.write(path, record)
            with self.assertRaisesRegex(ValueError, 'Mixed application'):
                web.build(root, root / 'compiler', root / 'output')

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

    def test_only_the_known_host_graphics_failure_can_be_reported_unavailable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def known_failure(args, **kwargs):
                kwargs['stdout'].write(b'Error: OpenGL(PainterError("egui_glow requires opengl 2.0+. "))\n')
                return subprocess.CompletedProcess(args, 1)
            with patch.object(ci.subprocess, 'run', side_effect=known_failure):
                result = ci.run(root, 'gui', ['unused'], allow_missing_opengl=True)
                self.assertEqual(result['exit'], 1)
                self.assertIn('unavailable', result['outcome'])
                with self.assertRaises(ValueError):
                    ci.run(root, 'gui', ['unused'])
            with patch.object(ci.subprocess, 'run', return_value=subprocess.CompletedProcess(['unused'], 1)):
                with self.assertRaises(ValueError):
                    ci.run(root, 'other-gui-error', ['unused'], allow_missing_opengl=True)

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


if __name__ == "__main__":
    unittest.main()
