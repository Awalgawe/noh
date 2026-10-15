"""Verify Linux archives reject missing private libraries and lost launch modes."""
import importlib.util
import hashlib
import json
from pathlib import Path
import re
import shutil
import tempfile
import unittest
import zipfile

import delivery
import linux_package
import linux_matrix

spec = importlib.util.spec_from_file_location('elf_fixtures', Path(__file__).with_name('test-linux-elf.py'))
elf_fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(elf_fixtures)
COMMIT = 'a' * 40


class LinuxPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / 'bundle'
        home = self.bundle / 'NOH.AppDir/usr/lib/noh'
        home.joinpath('lib').mkdir(parents=True)
        self.app = home / 'noh-app'
        self.app.write_bytes(elf_fixtures.fixture(('libasound.so.2',), rpath=None))
        self.library = home / 'lib/libnative.so.1'
        self.workers = []
        for name in ('bin/noh-cli', 'bin/noh-mcp', 'bin/speech/whisper-cli'):
            worker = home / name
            worker.parent.mkdir(parents=True, exist_ok=True)
            worker.write_bytes(elf_fixtures.fixture(('libc.so.6',), rpath=None))
            self.workers.append(worker)
        delivery.write(home / 'HOST-REQUIREMENTS.json', delivery.read(delivery.ROOT / 'assets/linux-host.json'))
        self.launcher = self.bundle / 'NOH.AppDir/AppRun'
        self.launcher.write_bytes(b'#!/bin/sh\nexec "$(dirname "$0")/usr/lib/noh/noh-app" "$@"\n')
        for path in (self.app, self.launcher, *self.workers):
            path.chmod(0o755)
        self.materials = self.root / 'materials'
        self.materials.mkdir()
        (self.materials / 'LICENSE.txt').write_text('Synthetic fixture notice')
        self.manifest = {'schema_version': 2, 'version': '0.1.0',
                         'runtime_dependencies': {'distribution': 'linux-host-runtime',
                             'contract': 'NOH.AppDir/usr/lib/noh/HOST-REQUIREMENTS.json'}, 'build': {
            'target': 'x86_64-unknown-linux-gnu', 'profile': 'release', 'features': ['gui', 'mcp'],
            'git_revision': COMMIT, 'git_dirty': False}}
        self.update_manifest()

    def update_manifest(self):
        self.manifest['sha256'] = {n: v['sha256'] for n, v in delivery.inventory(self.bundle).items()
                                   if n != 'manifest.json'}
        delivery.write(self.bundle / 'manifest.json', self.manifest)

    def seal(self):
        output = self.root / 'assets'
        delivery.seal(self.bundle, self.materials, output, 'linux-x64', COMMIT)
        return output

    def test_linux_archive_preserves_executable_modes_and_closure(self):
        output = self.seal()
        record = delivery.verify_envelope(output)
        self.assertEqual(record['platform'], 'linux-x64')
        self.assertEqual(set(record['assets']), {'NOH-0.1.0-linux-x64.zip', 'NOH-0.1.0-linux-x64-materials.zip'})

    def test_matrix_extracts_the_same_bytes_and_restores_unix_execution(self):
        output = self.seal()
        extracted = self.root / 'extracted'
        extracted.mkdir()
        record, archive_hash = linux_matrix.unpack_candidate(output, extracted)
        self.assertEqual(archive_hash, delivery.digest(output / 'NOH-0.1.0-linux-x64.zip'))
        self.assertEqual(delivery.inventory(extracted), record['bundle_files'])
        for executable in (self.app, self.launcher, *self.workers):
            path = extracted / executable.relative_to(self.bundle)
            self.assertEqual(path.stat().st_mode & 0o111, 0o111)

    def test_missing_worker_blocks_seal_even_with_consistent_manifest(self):
        self.workers[0].unlink()
        self.update_manifest()
        with self.assertRaisesRegex(ValueError, 'missing executables'):
            self.seal()
        self.assertFalse((self.root / 'assets').exists())

    def test_host_library_blocks_seal_even_with_consistent_manifest(self):
        self.library.with_name('libasound.so.2').write_bytes(
            elf_fixtures.fixture(soname='libasound.so.2', interpreter=None))
        self.update_manifest()
        with self.assertRaisesRegex(ValueError, 'must not be bundled'):
            self.seal()
        self.assertFalse((self.root / 'assets').exists())

    def test_unexecutable_launcher_is_rejected_even_with_consistent_hashes(self):
        self.launcher.chmod(0o644)
        with self.assertRaisesRegex(ValueError, 'AppRun lost'):
            self.seal()

    def test_even_unlisted_private_library_blocks_seal(self):
        self.library.write_bytes(elf_fixtures.fixture(rpath=None, soname='libnative.so.1'))
        self.update_manifest()
        with self.assertRaisesRegex(ValueError, 'private ELF'):
            self.seal()

    def test_changed_host_contract_blocks_seal(self):
        delivery.write(self.app.parent / 'HOST-REQUIREMENTS.json', {})
        self.update_manifest()
        with self.assertRaisesRegex(ValueError, 'host requirements differ'):
            self.seal()

    def test_host_contract_cannot_be_removed_or_changed_in_rehashed_archive(self):
        output = self.seal()
        archive = output / 'NOH-0.1.0-linux-x64.zip'
        original = archive.read_bytes()
        record_original = (output / 'DELIVERY.json').read_bytes()
        contract = 'NOH.AppDir/usr/lib/noh/HOST-REQUIREMENTS.json'
        for replacement in (None, b'{}'):
            with self.subTest(replacement=replacement):
                archive.write_bytes(original)
                record = json.loads(record_original)
                with zipfile.ZipFile(archive) as zipped:
                    contents = {info.filename: (info, zipped.read(info)) for info in zipped.infolist()}
                if replacement is None:
                    del contents[contract]
                    del record['bundle_files'][contract]
                else:
                    contents[contract] = (contents[contract][0], replacement)
                    record['bundle_files'][contract] = {
                        'size': len(replacement), 'sha256': hashlib.sha256(replacement).hexdigest()}
                manifest = json.loads(contents['manifest.json'][1])
                manifest['sha256'] = {n: hashlib.sha256(data).hexdigest()
                                      for n, (_, data) in contents.items() if n != 'manifest.json'}
                data = json.dumps(manifest).encode()
                contents['manifest.json'] = (contents['manifest.json'][0], data)
                record['bundle_files']['manifest.json'] = {
                    'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                with zipfile.ZipFile(archive, 'w') as zipped:
                    for info, data in contents.values():
                        zipped.writestr(info, data)
                record['assets'][archive.name] = {
                    'sha256': delivery.digest(archive), 'size': archive.stat().st_size}
                delivery.write(output / 'DELIVERY.json', record)
                (output / 'SHA256SUMS').write_text(''.join(
                    f'{delivery.digest(p)}  {p.name}\n' for p in sorted(output.iterdir()) if p.name != 'SHA256SUMS'))
                with self.assertRaisesRegex(ValueError, 'host requirements differ'):
                    delivery.verify_envelope(output)

    def test_host_installation_links_resolve_in_staged_documentation(self):
        home = self.app.parent
        linux_package.stage_host_documentation(home)
        for name in ('INSTALLING.md', 'BUILDING.md'):
            path = home / 'docs' / name
            shutil.copy2(delivery.ROOT / 'docs' / name, path)
            links = re.findall(r'\]\((\.\./assets/linux-README\.md)\)', path.read_text())
            self.assertTrue(links, name)
            for link in links:
                self.assertEqual((path.parent / link).read_bytes(),
                                 (delivery.ROOT / 'assets/linux-README.md').read_bytes())

    def test_tampered_zip_mode_is_rejected_after_updating_outer_checksums(self):
        output = self.seal()
        archive = output / 'NOH-0.1.0-linux-x64.zip'
        with zipfile.ZipFile(archive) as zipped:
            contents = [(info, zipped.read(info)) for info in zipped.infolist()]
        with zipfile.ZipFile(archive, 'w') as zipped:
            for info, data in contents:
                if info.filename.endswith('/noh-app'):
                    info.external_attr = 0o100644 << 16
                zipped.writestr(info, data)
        record = delivery.read(output / 'DELIVERY.json')
        record['assets'][archive.name] = {'sha256': delivery.digest(archive), 'size': archive.stat().st_size}
        delivery.write(output / 'DELIVERY.json', record)
        (output / 'SHA256SUMS').write_text(''.join(
            f'{delivery.digest(p)}  {p.name}\n' for p in sorted(output.iterdir()) if p.name != 'SHA256SUMS'))
        with self.assertRaisesRegex(ValueError, 'ELF lost'):
            delivery.verify_envelope(output)


if __name__ == '__main__':
    unittest.main()
