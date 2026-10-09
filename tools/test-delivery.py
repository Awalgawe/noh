#!/usr/bin/env python3
"""Meaningful delivery rejection checks, using only small synthetic fixtures."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import delivery
import rust_notices
import macos_signing
import windows_runtime
import windows_test_runtime
import plistlib

spec = importlib.util.spec_from_file_location("delivery_release", Path(__file__).with_name("delivery-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
spec = importlib.util.spec_from_file_location("public_installer", Path(__file__).with_name("public-installer.py"))
public_installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(public_installer)
COMMIT = "a" * 40


def pe_fixture(name=None, delay=False):
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3c, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<HH", data, 68, 0x8664, 1)
    struct.pack_into("<H", data, 84, 240)
    optional = 88
    struct.pack_into("<H", data, optional, 0x20b)
    struct.pack_into("<Q", data, optional + 24, 0x140000000)
    struct.pack_into("<I", data, optional + 108, 16)
    struct.pack_into("<IIII", data, optional + 240 + 8, 512, 0x1000, 512, 512)
    if name is not None:
        index, size = (13, 32) if delay else (1, 20)
        struct.pack_into("<II", data, optional + 112 + index * 8, 0x1000, size * 2)
        if delay:
            struct.pack_into("<II", data, 512, 1, 0x1080)
        else:
            struct.pack_into("<I", data, 524, 0x1080)
        data[640:640 + len(name) + 1] = name + b"\0"
    return data


class DeliveryTests(unittest.TestCase):
    def test_public_installer_rejects_changed_portable_before_compiler(self):
        (self.bundle / 'noh.exe').write_bytes(b'changed after qualification')
        with patch.object(public_installer.subprocess, 'run') as compile_process:
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                public_installer.build(self.bundle, COMMIT, self.root / 'compiler', self.root / 'installer')
            compile_process.assert_not_called()
        self.assertFalse((self.root / 'installer').exists())

    def test_public_installer_rejects_microsoft_payload_even_with_updated_manifest(self):
        (self.bundle / 'vc_redist.x64.exe').write_bytes(b'not a distributable NOH input')
        self.manifest['sha256']['vc_redist.x64.exe'] = delivery.digest(self.bundle / 'vc_redist.x64.exe')
        delivery.write(self.bundle / 'manifest.json', self.manifest)
        with patch.object(delivery, 'authorize_redistribution'), patch.object(public_installer.subprocess, 'run') as compile_process:
            with self.assertRaisesRegex(ValueError, 'must not be redistributed'):
                public_installer.build(self.bundle, COMMIT, self.root / 'compiler', self.root / 'installer')
            compile_process.assert_not_called()

    def test_public_installer_rejects_changed_compiler_before_output(self):
        compiler = self.root / 'compiler'
        compiler.mkdir()
        (compiler / 'ISCC.exe').write_bytes(b'wrong compiler')
        with patch.object(delivery, 'authorize_redistribution'), patch.object(public_installer.subprocess, 'run') as compile_process:
            with self.assertRaisesRegex(ValueError, 'compiler pin mismatch'):
                public_installer.build(self.bundle, COMMIT, compiler, self.root / 'installer')
            compile_process.assert_not_called()
        self.assertFalse((self.root / 'installer').exists())

    def test_public_installer_paths_cannot_inject_script_directives(self):
        for value in ('bad"; Flags: external', 'bad\n[Run]', 'bad{code:Injected}'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                public_installer.quoted(value)

    def test_native_notices_hash_all_sources_but_materialize_only_notice_inputs(self):
        crates = self.root / 'native-crates'
        crates.mkdir()
        files = {'src/lib.rs': b'pub fn fixture() {}\n',
                 'fonts/body/terms.txt': b'Original font terms\n',
                 'legal/custom.txt': b'Original explicit terms\n',
                 'LICENSE-MIT': b'Original MIT terms\n',
                 '.cargo_vcs_info.json': b'{"git":{"sha1":"' + COMMIT.encode() + b'"}}',
                 'Cargo.toml': b'[package]\nname="tiny"\nversion="1.0.0"\nlicense="MIT"\nlicense-file="./legal//custom.txt"\n'}
        archive = crates / 'tiny-1.0.0.crate'
        # Put the manifest last: license-file need not be known when its member
        # is encountered, and member order must not lose explicit terms.
        with tarfile.open(archive, 'w:gz') as tar:
            for name, data in files.items():
                member = tarfile.TarInfo('tiny-1.0.0/' + name)
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        original_generate = rust_notices.generate
        def inspect_then_generate(materials, cache, supplements, fetcher, evidence_root):
            folder = materials / 'vendor/tiny-1.0.0'
            checksums = delivery.read(folder / '.cargo-checksum.json')
            self.assertEqual(checksums['package'], delivery.digest(archive))
            self.assertEqual(checksums['files'], {name: hashlib.sha256(data).hexdigest()
                                               for name, data in files.items()})
            self.assertFalse((folder / 'src/lib.rs').exists())
            for name in set(files) - {'src/lib.rs'}:
                self.assertEqual((folder / name).read_bytes(), files[name])
            return original_generate(materials, cache, supplements, fetcher, evidence_root)
        notices = self.root / 'notices'
        notices.mkdir()
        with patch.object(rust_notices, 'generate', side_effect=inspect_then_generate):
            self.assertEqual(windows_runtime.native_rust_notices(crates, notices, self.root / 'cache'), [])
        text = (notices / 'rust-native/RUST-NOTICES.txt').read_bytes()
        inventory = delivery.read(notices / 'rust-native/RUST-NOTICE-INVENTORY.json')
        self.assertEqual(inventory['crates'][0]['license_file'], './legal//custom.txt')
        for name in ('fonts/body/terms.txt', 'legal/custom.txt', 'LICENSE-MIT'):
            self.assertIn(files[name], text)

    def test_native_notices_retain_all_sources_for_complete_correspondence(self):
        crates = self.root / 'correspondence-crates'
        crates.mkdir()
        files = {'src/lib.rs': b'pub fn fixture() {}\n',
                 'Cargo.toml': b'[package]\nname="tiny"\nversion="1.0.0"\nlicense="MIT"\n'}
        with tarfile.open(crates / 'tiny-1.0.0.crate', 'w') as tar:
            for name, data in files.items():
                member = tarfile.TarInfo('tiny-1.0.0/' + name)
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        supplements = {'crates': {'tiny-1.0.0': {'source_correspondence': []}}}
        def inspect(materials, *_):
            folder = materials / 'vendor/tiny-1.0.0'
            for name, data in files.items():
                self.assertEqual((folder / name).read_bytes(), data)
            (materials / 'rust-notices').mkdir()
            return {'missing_notice_texts': []}
        notices = self.root / 'correspondence-notices'
        notices.mkdir()
        with patch.object(delivery, 'read', return_value=supplements), \
                patch.object(rust_notices, 'generate', side_effect=inspect):
            windows_runtime.native_rust_notices(crates, notices, self.root / 'cache')

    def test_native_notices_reject_unsafe_unselected_members_before_generation(self):
        license_paths = {'license-path': '../outside', 'license-absolute': '/outside',
                         'license-backslash': 'legal\\custom.txt', 'license-drive': 'C:/outside'}
        for case in ('root', 'root-file', 'parent-file', 'link', 'duplicate', 'checksum', 'traversal', *license_paths):
            with self.subTest(case=case):
                crates = self.root / (case + '-crates')
                crates.mkdir()
                with tarfile.open(crates / 'tiny-1.0.0.crate', 'w') as tar:
                    data = b'[package]\nname="tiny"\nversion="1.0.0"\n'
                    if case in license_paths:
                        data += ('license-file=' + json.dumps(license_paths[case]) + '\n').encode()
                    manifest = tarfile.TarInfo('tiny-1.0.0/Cargo.toml')
                    manifest.size = len(data)
                    tar.addfile(manifest, io.BytesIO(data))
                    name = {'root': 'foreign/src/lib.rs', 'root-file': 'tiny-1.0.0',
                            'checksum': 'tiny-1.0.0/.cargo-checksum.json',
                            'traversal': 'tiny-1.0.0/../outside'}.get(case, 'tiny-1.0.0/src/lib.rs')
                    member = tarfile.TarInfo(name)
                    if case == 'link':
                        member.type = tarfile.SYMTYPE
                        member.linkname = 'outside'
                    tar.addfile(member)
                    if case == 'duplicate':
                        tar.addfile(member)
                    if case == 'parent-file':
                        tar.addfile(tarfile.TarInfo('tiny-1.0.0/SRC'))
                notices = self.root / (case + '-notices')
                notices.mkdir()
                with patch.object(rust_notices, 'generate') as generate, self.assertRaises(ValueError):
                    windows_runtime.native_rust_notices(crates, notices, self.root / 'cache')
                generate.assert_not_called()
                self.assertFalse((notices / 'rust-native').exists())

    def test_ci_backend_uses_only_locked_export_packages_and_exact_file_bytes(self):
        binary = bytes(pe_fixture(b'helper.dll'))
        helper = bytes(pe_fixture())
        package = self.root / 'backend.tar'
        files = {'ffmpeg.exe': binary, 'helper.dll': helper}
        with tarfile.open(package, 'w') as tar:
            for name, data in files.items():
                member = tarfile.TarInfo('mingw64/bin/' + ('Helper.dll' if name == 'helper.dll' else name))
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        lock = {'roles': {'export': {
            name: {'package': 'backend', 'member': 'mingw64/bin/' + ('Helper.dll' if name == 'helper.dll' else name),
                   'sha256': hashlib.sha256(data).hexdigest()}
            for name, data in files.items()},
            'preview': {'libmpv.dll': {'package': 'unused-preview'}}},
            'packages': {'backend': {'binary': {'url': 'https://example.invalid/backend.tar'}}}}
        output = self.root / 'test-backend'
        with patch.object(delivery, 'fetch', return_value=package) as fetch, \
                patch.object(delivery, 'validate_ffmpeg') as validate:
            executable = windows_test_runtime.acquire(lock, self.root / 'cache', output)
        self.assertEqual(fetch.call_count, 1)
        validate.assert_called_once_with(executable)
        for name, data in files.items():
            self.assertEqual((output / name).read_bytes(), data)
        self.assertEqual(delivery.read(output / 'PE-IMPORTS.json')['missing'], [])

    def test_ci_backend_rejects_changed_linked_and_incomplete_runtime_inputs(self):
        data = bytes(pe_fixture(b'missing.dll'))
        for case in ('hash', 'link', 'missing-dependency', 'duplicate', 'path', 'collision'):
            with self.subTest(case=case):
                package = self.root / (case + '.tar')
                with tarfile.open(package, 'w') as tar:
                    member = tarfile.TarInfo('mingw64/bin/ffmpeg.exe')
                    member.size = len(data)
                    if case == 'link':
                        member.type = tarfile.SYMTYPE
                        member.linkname = 'foreign.exe'
                        tar.addfile(member)
                    else:
                        tar.addfile(member, io.BytesIO(data))
                        if case == 'duplicate':
                            tar.addfile(member, io.BytesIO(data))
                name = '../ffmpeg.exe' if case == 'path' else 'ffmpeg.exe'
                lock = {'roles': {'export': {name: {'package': 'backend',
                    'member': 'mingw64/bin/ffmpeg.exe',
                    'sha256': '0' * 64 if case == 'hash' else hashlib.sha256(data).hexdigest()} }},
                    'packages': {'backend': {'binary': {'url': 'https://example.invalid/backend.tar'}}}}
                if case == 'collision':
                    lock['roles']['export']['FFMPEG.exe'] = copy.deepcopy(lock['roles']['export']['ffmpeg.exe'])
                with patch.object(delivery, 'fetch', return_value=package), \
                        patch.object(delivery, 'validate_ffmpeg') as validate, \
                        self.assertRaises(ValueError):
                    windows_test_runtime.acquire(lock, self.root / 'cache', self.root / case)
                validate.assert_not_called()

    def test_original_epoch_dated_vendor_files_survive_material_zip_roundtrip(self):
        import os
        folder = self.root / 'epoch-vendor'
        folder.mkdir()
        original = folder / 'original-license.txt'
        original.write_bytes(b'Original terms\r\n')
        os.utime(original, (1000, 1000))
        expected = delivery.inventory(folder)
        output = self.root / 'epoch-materials.zip'
        delivery.zip_folder(folder, output)
        delivery.verify_zip(output, expected)
        with zipfile.ZipFile(output) as archive:
            self.assertEqual(archive.read(original.name), original.read_bytes())
            self.assertEqual(archive.getinfo(original.name).date_time[0], 1980)

    def test_source_notices_retain_pinned_original_bytes_and_reject_changes(self):
        content = b'Original upstream terms\r\n'
        inner_data = io.BytesIO()
        with tarfile.open(fileobj=inner_data, mode='w:gz') as inner:
            member = tarfile.TarInfo('component/LICENSE')
            member.size = len(content)
            inner.addfile(member, io.BytesIO(content))
        source = self.root / 'source.tar'
        with tarfile.open(source, 'w') as outer:
            payload = inner_data.getvalue()
            member = tarfile.TarInfo('sources/upstream.tar.gz')
            member.size = len(payload)
            outer.addfile(member, io.BytesIO(payload))
        import hashlib
        spec = {'component': 'component', 'source_archive_member': member.name,
                'source_member': 'component/LICENSE', 'sha256': hashlib.sha256(content).hexdigest()}
        notices = self.root / 'notices'
        windows_runtime.source_notices(source, 'component', [spec], notices)
        self.assertEqual((notices / 'component/upstream/component/LICENSE').read_bytes(), content)
        spec['sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'checksum'):
            windows_runtime.source_notices(source, 'component', [spec], notices)

    def test_retained_git_notice_binds_source_and_text_without_checkout(self):
        source = self.root / 'source.tar'
        source.write_bytes(b'Locked original source archive')
        original = self.root / 'COPYING'
        original.write_bytes(b'Original terms\r\n')
        spec = {'component': 'component', 'repository_file': 'COPYING',
                'source_archive_sha256': delivery.digest(source),
                'source_member': 'librtmp/COPYING', 'sha256': delivery.digest(original)}
        notices = self.root / 'notices'
        with patch.object(delivery, 'ROOT', self.root):
            windows_runtime.source_notices(source, 'component', [spec], notices)
            self.assertEqual((notices / 'component/upstream/librtmp/COPYING').read_bytes(), original.read_bytes())
            original.write_bytes(b'Changed terms')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                windows_runtime.source_notices(source, 'component', [spec], notices)
            spec['source_archive_sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'archive checksum'):
                windows_runtime.source_notices(source, 'component', [spec], notices)

    def test_native_toolchain_notices_reject_recipe_and_notice_mismatch(self):
        recipe = self.root / 'PKGBUILD'
        recipe.write_bytes(b'Original producer recipe')
        buildinfo = ('pkgbuild_sha256sum = ' + delivery.digest(recipe) + '\n').encode()
        content = b'Original generated standard-library notices'
        package = self.root / 'rust.tar'
        with tarfile.open(package, 'w') as tar:
            for name, data in [('.BUILDINFO', buildinfo), ('doc/COPYRIGHT-library.html', content)]:
                member = tarfile.TarInfo(name)
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        pinned = {'url': 'https://example.invalid/rust.tar', 'sha256': delivery.digest(package),
                  'buildinfo_sha256': hashlib.sha256(buildinfo).hexdigest(),
                  'notices': [{'member': 'doc/COPYRIGHT-library.html', 'sha256': hashlib.sha256(content).hexdigest()}]}
        spec = {'version': '1.87.0-2', 'binary': pinned, 'standard_library_source': pinned,
                'recipe_sha256': delivery.digest(recipe),
                'recipe_files': [{'path': 'PKGBUILD', 'sha256': delivery.digest(recipe)}]}
        sources, notices = self.root / 'sources', self.root / 'notices'
        with patch.object(delivery, 'ROOT', self.root), patch.object(delivery, 'fetch', return_value=package):
            windows_runtime.native_toolchain_materials([spec], self.root, sources, notices)
            self.assertEqual((notices / 'rust-toolchains/1.87.0-2/doc/COPYRIGHT-library.html').read_bytes(), content)
            self.assertEqual((sources / 'rust-toolchains/rust.tar').read_bytes(), package.read_bytes())
            spec['recipe_sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'recipe mismatch'):
                windows_runtime.native_toolchain_materials([spec], self.root, sources, notices)
            spec['recipe_sha256'] = delivery.digest(recipe)
            pinned['notices'][0]['sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'notice checksum'):
                windows_runtime.native_toolchain_materials([spec], self.root, sources, notices)

    def test_std_dependency_graph_cannot_omit_registry_crates(self):
        content = ('[[package]]\nname = "dep"\nversion = "1.0.0"\n'
                   'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                   'checksum = "' + 'a' * 64 + '"\n').encode()
        retained = self.root / 'library-Cargo.lock'
        retained.write_bytes(content)
        source = self.root / 'rust.tar'
        with tarfile.open(source, 'w') as tar:
            member = tarfile.TarInfo('rust/library/Cargo.lock')
            member.size = len(content)
            tar.addfile(member, io.BytesIO(content))
        crate = self.root / 'dep-1.0.0.crate'
        crate.write_bytes(b'Fixture crate handled by the separately tested notice collector')
        graph = {'source': {'url': 'https://example.invalid/rust.tar'},
                 'member': member.name, 'path': retained.name,
                 'sha256': hashlib.sha256(content).hexdigest(), 'packages': {'dep-1.0.0': 'a' * 64}}
        lock = {'standard_library_dependency_locks': [graph],
                'standard_library_crates': {'dep-1.0.0': {'sha256': 'a' * 64}}}
        fetch = lambda spec, cache, name: source if name == source.name else crate
        with patch.object(delivery, 'ROOT', self.root), patch.object(delivery, 'fetch', side_effect=fetch), \
                patch.object(windows_runtime, 'native_rust_notices', return_value=[]) as notices:
            windows_runtime.standard_library_dependency_materials(lock, self.root, self.root / 'sources', self.root / 'notices')
            self.assertEqual(notices.call_args.kwargs['output_name'], 'rust-standard-libraries')
            graph['packages'] = {}
            with self.assertRaisesRegex(ValueError, 'Incomplete Rust std dependency graph'):
                windows_runtime.standard_library_dependency_materials(lock, self.root, self.root / 'sources2', self.root / 'notices2')
            graph['packages'] = {'dep-1.0.0': 'a' * 64}
            lock['standard_library_crates'] = {}
            with self.assertRaisesRegex(ValueError, 'exact dependency graphs'):
                windows_runtime.standard_library_dependency_materials(lock, self.root, self.root / 'sources3', self.root / 'notices3')

    def mac_bundle(self):
        bundle = self.root / 'mac-bundle'
        app = bundle / 'NOH.app'
        paths = ['Contents/MacOS/noh-app', 'Contents/MacOS/bin/ffmpeg', 'Contents/Frameworks/libmpv.2.dylib']
        for name in paths:
            path = app / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(bytes.fromhex('cffaedfe') + b'fixture code')
        (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'noh-app', 'CFBundlePackageType': 'APPL'}))
        resources = app / 'Contents/Resources'
        resources.mkdir()
        delivery.write(resources / 'PORTABLE-RUNTIME.json', {'signature': 'ad-hoc'})
        (resources / 'README.md').write_text('Private fixture')
        (bundle / 'README.md').write_text('Private fixture')
        manifest = {'schema_version': 2, 'version': '0.1.0',
                    'build': {'target': 'aarch64-apple-darwin', 'profile': 'release', 'git_revision': COMMIT, 'git_dirty': False, 'features': ['gui', 'mcp']},
                    'runtime_dependencies': {'portable': {'mach_o': [{'path': name} for name in paths]}},
                    'tools': {'ffmpeg': {'path': 'NOH.app/Contents/MacOS/bin/ffmpeg'}},
                    'sha256': {name: item['sha256'] for name, item in delivery.inventory(bundle).items()}}
        delivery.write(bundle / 'manifest.json', manifest)
        return bundle, paths

    def test_macos_signing_plan_requires_exact_recorded_code_without_running_tools(self):
        bundle, paths = self.mac_bundle()
        with patch.object(macos_signing, 'run', side_effect=AssertionError('No native tools in plan')):
            planned = macos_signing.plan(bundle, COMMIT)
        self.assertEqual(planned['status'], 'unexecuted-signing-plan')
        self.assertEqual(set(planned['code']), set(paths))
        manifest = delivery.read(bundle / 'manifest.json')
        manifest['runtime_dependencies']['portable']['mach_o'].pop()
        delivery.write(bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(ValueError, 'exact recorded Mach-O'):
            macos_signing.plan(bundle, COMMIT)

    def test_macos_signing_refuses_other_hosts_before_credential_access(self):
        with patch.object(macos_signing.platform, 'system', return_value='Windows'), patch.object(macos_signing, 'plan') as planned:
            with self.assertRaisesRegex(ValueError, 'Apple Silicon GitHub runner'):
                macos_signing.execute(self.root, COMMIT, 'a' * 40, 'ABCDEFGHIJ')
            planned.assert_not_called()

    def test_macos_signing_redacts_credential_command_failures(self):
        failure = macos_signing.subprocess.CompletedProcess(['security', 'secret-password'], 1, '', 'private-password')
        with patch.object(macos_signing.subprocess, 'run', return_value=failure):
            with self.assertRaises(ValueError) as error:
                macos_signing.run(['security', 'private-password'])
            self.assertNotIn('private-password', str(error.exception))
        with patch.object(macos_signing.subprocess, 'run', side_effect=macos_signing.subprocess.TimeoutExpired(['security', 'private-password'], 1)):
            with self.assertRaisesRegex(ValueError, '^security timed out$'):
                macos_signing.run(['security', 'private-password'])

    def test_macos_signature_checks_real_code_directory_flags_and_identity(self):
        details = ('Authority=Developer ID Application: Fixture\nTeamIdentifier=ABCDEFGHIJ\n'
                   'CodeDirectory v=20500 size=1234 flags=0x10000(runtime) hashes=12+3 location=embedded\n'
                   'Timestamp=Oct 5, 2026 at 10:00:00 AM')
        with patch.object(macos_signing, 'run', return_value=details):
            macos_signing.signature_check(self.root, [], 'ABCDEFGHIJ')
        invalid = [details.replace('0x10000(runtime)', '0x0(runtime)'),
                   details.replace('0x10000(runtime)', '0x0(none)'),
                   details.replace('CodeDirectory v=20500 size=1234 flags=0x10000(runtime)', 'flags=runtime'),
                   details.replace('TeamIdentifier=ABCDEFGHIJ', 'TeamIdentifier=OTHERTEAM0'),
                   details.replace('Timestamp=Oct 5, 2026 at 10:00:00 AM', 'Timestamp=none'),
                   details.replace('Authority=Developer ID Application: Fixture', 'Authority=Apple Development: Fixture')]
        for output in invalid:
            with self.subTest(output=output), patch.object(macos_signing, 'run', return_value=output):
                with self.assertRaisesRegex(ValueError, 'verification failed'):
                    macos_signing.signature_check(self.root, [], 'ABCDEFGHIJ')

    def test_macos_notarization_rejects_mismatched_pending_and_warning_logs(self):
        submission = '12345678-1234-1234-1234-123456789abc'
        accepted = {'id': submission, 'status': 'Accepted'}
        log = {'jobId': submission.upper(), 'status': 'Accepted', 'issues': None}
        self.assertEqual(macos_signing.accepted_submission(accepted, log), submission)
        for result, report in [(accepted | {'status': 'In Progress'}, log),
                               (accepted, log | {'jobId': 'another-job'}),
                               (accepted, log | {'issues': [{'severity': 'warning'}]})]:
            with self.assertRaises(ValueError):
                macos_signing.accepted_submission(result, report)

    def test_macos_signing_orders_native_steps_rehashes_final_bytes_and_cleans_keychain(self):
        bundle, paths = self.mac_bundle()
        app = bundle / 'NOH.app'
        original = delivery.digest(app / paths[1])
        identity, team = 'a' * 40, 'ABCDEFGHIJ'
        submission = '12345678-1234-1234-1234-123456789abc'
        commands = []
        def native(command, **kwargs):
            args = [str(x) for x in command]
            commands.append(args)
            if 'find-identity' in args:
                return identity + ' "Developer ID Application: Fixture (' + team + ')"'
            if '--display' in args:
                return 'Authority=Developer ID Application: Fixture\nTeamIdentifier=' + team + '\nCodeDirectory v=20500 size=1234 flags=0x10000(runtime) hashes=12+3 location=embedded\nTimestamp=fixture'
            if '--force' in args and args[-1] != str(app):
                path = Path(args[-1]); path.write_bytes(path.read_bytes() + b'signature')
            if 'submit' in args:
                return json.dumps({'id': submission, 'status': 'Accepted'})
            if 'log' in args:
                delivery.write(Path(args[-1]), {'jobId': submission, 'status': 'Accepted', 'issues': None})
            if 'staple' in args:
                (app / 'Contents/ticket').write_bytes(b'ticket')
            return ''
        env = {'RUNNER_TEMP': str(self.root), 'NOH_MACOS_CERTIFICATE_BASE64': 'ZmFrZQ==', 'NOH_MACOS_CERTIFICATE_PASSWORD': 'fake-cert-password',
               'NOH_APPLE_ID': 'fixture@example.invalid', 'NOH_APPLE_APP_PASSWORD': 'fake-notary-password'}
        with patch.object(macos_signing, 'require_ci_host'), patch.dict(macos_signing.os.environ, env), patch.object(macos_signing, 'run', side_effect=native):
            macos_signing.execute(bundle, COMMIT, identity, team)
        report = delivery.read(bundle / 'MACOS-SIGNING.json')
        self.assertEqual(report['status'], 'signed-notarized-stapled-candidate')
        self.assertEqual(report['app_files'], delivery.inventory(app))
        manifest = delivery.verify_bundle(bundle, 'aarch64-apple-darwin', COMMIT)
        self.assertNotEqual(original, manifest['tools']['ffmpeg']['sha256'])
        self.assertEqual(manifest['tools']['ffmpeg']['sha256'], delivery.digest(app / paths[1]))
        signing = [args for args in commands if '--force' in args]
        self.assertEqual(signing[-1][-1], str(app))
        self.assertFalse(any('--deep' in args for args in signing))
        self.assertLess(next(i for i,c in enumerate(commands) if 'submit' in c), next(i for i,c in enumerate(commands) if 'staple' in c))
        self.assertEqual(commands[-1][1], 'delete-keychain')
        self.assertFalse(any(p.name.startswith('noh-signing-') for p in self.root.iterdir()))

    def test_macos_signing_cleans_credentials_on_identity_rejection(self):
        bundle, _ = self.mac_bundle()
        original = delivery.inventory(bundle)
        commands = []
        def native(command, **kwargs):
            commands.append([str(x) for x in command])
            return ''
        env = {'RUNNER_TEMP': str(self.root), 'NOH_MACOS_CERTIFICATE_BASE64': 'ZmFrZQ==', 'NOH_MACOS_CERTIFICATE_PASSWORD': 'fake',
               'NOH_APPLE_ID': 'fixture@example.invalid', 'NOH_APPLE_APP_PASSWORD': 'fake'}
        with patch.object(macos_signing, 'require_ci_host'), patch.dict(macos_signing.os.environ, env), patch.object(macos_signing, 'run', side_effect=native):
            with self.assertRaisesRegex(ValueError, 'certificate/team'):
                macos_signing.execute(bundle, COMMIT, 'a' * 40, 'ABCDEFGHIJ')
        self.assertEqual(commands[-1][1], 'delete-keychain')
        self.assertEqual(delivery.inventory(bundle), original)
        self.assertFalse(any(p.name.startswith('noh-signing-') for p in self.root.iterdir()))

    def test_macos_final_zip_roundtrip_requires_ticket_and_unchanged_app_bytes(self):
        import shutil
        bundle, code = self.mac_bundle()
        app = bundle / 'NOH.app'
        report = {'code': code, 'team_id': 'ABCDEFGHIJ', 'app_files': delivery.inventory(app)}
        delivery.write(bundle / 'MACOS-SIGNING.json', report)
        archive = self.root / 'final.zip'
        archive.write_bytes(b'fixture; extraction is mocked')
        lose_ticket = False
        corrupt = False
        def native(command, **kwargs):
            args = [str(x) for x in command]
            if '-x' in args:
                target = Path(args[-1]) / 'NOH.app'
                shutil.copytree(app, target)
                if corrupt:
                    (target / 'Contents/MacOS/noh-app').write_bytes(b'changed')
            if '--display' in args:
                return 'Authority=Developer ID Application: Fixture\nTeamIdentifier=ABCDEFGHIJ\nCodeDirectory v=20500 size=1234 flags=0x10000(runtime) hashes=12+3 location=embedded\nTimestamp=fixture'
            if 'validate' in args and lose_ticket:
                raise ValueError('Stapled ticket did not survive final ZIP')
            return ''
        with patch.object(macos_signing, 'require_ci_host'), patch.dict(macos_signing.os.environ, {'RUNNER_TEMP': str(self.root)}), patch.object(macos_signing, 'run', side_effect=native):
            macos_signing.verify_archive(archive, bundle)
            lose_ticket = True
            with self.assertRaisesRegex(ValueError, 'ticket did not survive'):
                macos_signing.verify_archive(archive, bundle)
            corrupt = True
            with self.assertRaisesRegex(ValueError, 'changes the signed application'):
                macos_signing.verify_archive(archive, bundle)

    def test_rust_notice_inventory_cannot_omit_vendored_crates(self):
        root = self.root / 'inventory-materials'
        vendor = root / 'vendor'
        folder = vendor / 'tiny-1.0.0'
        folder.mkdir(parents=True)
        (folder / 'Cargo.toml').write_text('[package]\nname="tiny"\nversion="1.0.0"\nlicense="MIT"\n')
        delivery.write(folder / '.cargo-checksum.json', {'package': 'a' * 64, 'files': {}})
        lock = self.root / 'fixture.lock'
        lock.write_text('[[package]]\nname="tiny"\nversion="1.0.0"\nsource="registry+fixture"\nchecksum="' + 'a' * 64 + '"\n')
        crates = rust_notices.inventory_vendor(vendor, lock)
        self.assertEqual(len(crates), 1)
        delivery.write(root / 'RUST-INVENTORY.json', [])
        with self.assertRaisesRegex(ValueError, 'exact vendor graph'):
            rust_notices.generate(root, self.root / 'cache', {'crates': {}, 'unresolved': []}, lambda *args: self.fail('Network forbidden'))
        self.assertFalse((root / 'rust-notices').exists())
        delivery.write(folder / '.cargo-checksum.json', {'package': 'b' * 64, 'files': {}})
        with self.assertRaisesRegex(ValueError, 'checksum differs'):
            rust_notices.inventory_vendor(vendor, lock)

    def test_rust_notices_preserve_font_and_upstream_text_and_report_gaps(self):
        root = self.root / 'rust-materials'
        folder = root / 'vendor/tiny-1.0.0'
        (folder / 'fonts').mkdir(parents=True)
        font = b'Original font attribution and terms\n'
        (folder / 'fonts/OFL.txt').write_bytes(font)
        original = b'Original upstream license and copyright\n'
        downloaded = self.root / 'upstream.txt'
        downloaded.write_bytes(original)
        delivery.write(folder / '.cargo-checksum.json', {'package': 'a' * 64, 'files': {'fonts/OFL.txt': delivery.hashlib.sha256(font).hexdigest()}})
        delivery.write(folder / '.cargo_vcs_info.json', {'git': {'sha1': COMMIT}})
        delivery.write(root / 'RUST-INVENTORY.json', [{'name': 'tiny', 'version': '1.0.0', 'license': 'MIT AND OFL-1.1', 'source': 'registry+fixture', 'license_file': None}])
        item = {'url': 'https://raw.githubusercontent.com/owner/project/' + COMMIT + '/LICENSE',
                'path': 'LICENSE', 'sha256': delivery.digest(downloaded),
                'git_blob': delivery.hashlib.sha1(b'blob ' + str(len(original)).encode() + b'\0' + original).hexdigest()}
        supplements = {'crates': {'tiny-1.0.0': {'package_sha256': 'a' * 64, 'revision': COMMIT,
                       'repository': 'https://github.com/owner/project', 'files': [item]}}, 'unresolved': []}
        report = rust_notices.generate(root, self.root / 'cache', supplements, lambda *args: downloaded)
        notice = (root / 'rust-notices/RUST-NOTICES.txt').read_bytes()
        self.assertIn(font, notice)
        self.assertIn(original, notice)
        self.assertEqual(report['missing_notice_texts'], [])
        self.assertEqual(report['status'], 'unreviewed-notice-materials')
        self.assertEqual(report['notice_sha256'], delivery.digest(root / 'rust-notices/RUST-NOTICES.txt'))
        missing = self.root / 'missing-materials'
        (missing / 'vendor/tiny-1.0.0').mkdir(parents=True)
        delivery.write(missing / 'vendor/tiny-1.0.0/.cargo-checksum.json', {'package': 'a' * 64, 'files': {}})
        delivery.write(missing / 'RUST-INVENTORY.json', delivery.read(root / 'RUST-INVENTORY.json'))
        report = rust_notices.generate(missing, self.root / 'cache', {'crates': {}, 'unresolved': []}, lambda *args: self.fail('Network forbidden'))
        self.assertEqual(report['missing_notice_texts'], ['tiny-1.0.0'])
        self.assertIn('MISSING ORIGINAL NOTICE', (missing / 'rust-notices/RUST-NOTICES.txt').read_text())

    def test_explicit_license_declaration_preserves_origin_and_rejects_changes(self):
        root = self.root / 'declared-materials'
        folder = root / 'vendor/tiny-1.0.0'
        folder.mkdir(parents=True)
        manifest = b'[package]\nname="tiny"\nversion="1.0.0"\nlicense="MIT OR Apache-2.0"\nauthors=["Original author"]\n'
        (folder / 'Cargo.toml').write_bytes(manifest)
        delivery.write(folder / '.cargo-checksum.json', {'package': 'a' * 64,
                       'files': {'Cargo.toml': hashlib.sha256(manifest).hexdigest()}})
        delivery.write(root / 'RUST-INVENTORY.json', [{'name': 'tiny', 'version': '1.0.0',
                       'license': 'MIT OR Apache-2.0', 'source': 'registry+fixture', 'license_file': None}])
        text_path = self.root / 'assets/license-texts/Apache-2.0.txt'
        text_path.parent.mkdir(parents=True)
        text_path.write_bytes(b'Synthetic standard license, not an original copyright notice')
        declaration = {'package_sha256': 'a' * 64, 'declared_license': 'MIT OR Apache-2.0',
                       'selected_license': 'Apache-2.0', 'authors': ['Original author'],
                       'text': {'path': 'assets/license-texts/Apache-2.0.txt', 'sha256': delivery.digest(text_path)},
                       'basis': 'Explicit fixture declaration'}
        spec = {'crates': {}, 'unresolved': [], 'declared_license_supplements': {'tiny-1.0.0': declaration}}
        report = rust_notices.generate(root, self.root / 'cache', spec, lambda *args: self.fail('No download'), self.root)
        self.assertEqual(report['missing_original_notice_texts'], ['tiny-1.0.0'])
        self.assertEqual(report['missing_notice_texts'], [])
        self.assertIn(manifest, (root / 'rust-notices/RUST-NOTICES.txt').read_bytes())
        self.assertIn(b'standard-license-text', (root / 'rust-notices/RUST-NOTICES.txt').read_bytes())
        import shutil
        mutations = [('package_sha256', 'b' * 64), ('selected_license', 'GPL-3.0-only'),
                     ('authors', ['Invented author']), ('declared_license', 'MIT')]
        for key, value in mutations:
            shutil.rmtree(root / 'rust-notices')
            changed = copy.deepcopy(spec)
            changed['declared_license_supplements']['tiny-1.0.0'][key] = value
            with self.assertRaisesRegex(ValueError, 'does not match'):
                rust_notices.generate(root, self.root / 'cache', changed, lambda *args: self.fail('No download'), self.root)
        shutil.rmtree(root / 'rust-notices')
        text_path.write_bytes(b'changed standard terms')
        with self.assertRaisesRegex(ValueError, 'Standard-license text changed'):
            rust_notices.generate(root, self.root / 'cache', spec, lambda *args: self.fail('No download'), self.root)

    def test_rust_notices_reject_modified_vendor_license(self):
        root = self.root / 'rust-materials'
        folder = root / 'vendor/tiny-1.0.0'
        folder.mkdir(parents=True)
        (folder / 'LICENSE').write_bytes(b'altered terms')
        delivery.write(folder / '.cargo-checksum.json', {'package': 'a' * 64, 'files': {'LICENSE': '0' * 64}})
        delivery.write(root / 'RUST-INVENTORY.json', [{'name': 'tiny', 'version': '1.0.0', 'license': 'MIT', 'source': 'registry+fixture', 'license_file': None}])
        with self.assertRaisesRegex(ValueError, 'Vendor notice checksum mismatch'):
            rust_notices.generate(root, self.root / 'cache', {'crates': {}, 'unresolved': []}, lambda *args: self.fail('Network forbidden'))

    def test_old_crate_notice_requires_complete_published_source_and_tree_correspondence(self):
        root = self.root / 'old-crate-materials'
        folder = root / 'vendor/tiny-1.0.0'
        folder.mkdir(parents=True)
        published = {'Cargo.toml.orig': b'[package]\nname="tiny"\nversion="1.0.0"\n',
                     'src/lib.rs': b'pub fn example() {}\n'}
        entries, matches = [], []
        for name, data in published.items():
            path = folder / name
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(data)
            blob = delivery.hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
            upstream = 'Cargo.toml' if name == 'Cargo.toml.orig' else name
            matches.append({'path': name, 'upstream_path': upstream, 'sha256': delivery.digest(path), 'git_blob': blob})
            entries.append({'path': upstream, 'type': 'blob', 'sha': blob})
        checksums = {'package': 'a' * 64, 'files': {item['path']: item['sha256'] for item in matches}}
        delivery.write(folder / '.cargo-checksum.json', checksums)
        original = self.root / 'upstream-license.txt'
        original.write_bytes(b'Original upstream terms and attribution\n')
        notice = {'path': 'LICENSE', 'url': 'https://raw.githubusercontent.com/owner/project/' + COMMIT + '/LICENSE',
                  'sha256': delivery.digest(original),
                  'git_blob': delivery.hashlib.sha1(b'blob ' + str(original.stat().st_size).encode() + b'\0' + original.read_bytes()).hexdigest()}
        entries.append({'path': 'LICENSE', 'type': 'blob', 'sha': notice['git_blob']})
        tree_path = self.root / 'assets/rust-notice-evidence/tiny.json'
        tree_path.parent.mkdir(parents=True)
        tree = {'url': 'https://api.github.com/repos/owner/project/git/trees/' + COMMIT, 'tree': entries, 'truncated': False}
        delivery.write(tree_path, tree)
        supplement = {'repository': 'https://github.com/owner/project', 'revision': COMMIT,
                      'package_sha256': 'a' * 64, 'files': [notice], 'source_correspondence': matches,
                      'source_tree': {'path': 'assets/rust-notice-evidence/tiny.json', 'sha256': delivery.digest(tree_path)}}
        delivery.write(root / 'RUST-INVENTORY.json', [{'name': 'tiny', 'version': '1.0.0', 'license': 'MIT', 'source': 'registry+fixture'}])
        report = rust_notices.generate(root, self.root / 'cache', {'crates': {'tiny-1.0.0': supplement}, 'unresolved': []},
                                       lambda *args: original, evidence_root=self.root)
        self.assertEqual(report['missing_notice_texts'], [])
        proof = report['crates'][0]['source_correspondence']
        self.assertEqual(proof['method'], 'complete-published-content-match')
        self.assertEqual(delivery.digest(root / 'rust-notices' / proof['source_tree']['path']), delivery.digest(tree_path))
        self.assertEqual(proof['source_tree']['original_text'].encode('utf-8'), tree_path.read_bytes())
        self.assertIn(original.read_bytes(), (root / 'rust-notices/RUST-NOTICES.txt').read_bytes())
        with self.assertRaisesRegex(ValueError, 'omits published files'):
            rust_notices.source_correspondence(folder, checksums, supplement | {'source_correspondence': matches[:-1]}, self.root, root / 'rust-notices')
        (folder / 'src/lib.rs').write_bytes(b'changed code')
        with self.assertRaisesRegex(ValueError, 'differs from pinned upstream'):
            rust_notices.source_correspondence(folder, checksums, supplement, self.root, root / 'rust-notices')
        (folder / 'src/lib.rs').write_bytes(published['src/lib.rs'])
        delivery.write(tree_path, tree | {'url': tree['url'] + '-other'})
        with self.assertRaisesRegex(ValueError, 'tree evidence changed'):
            rust_notices.source_correspondence(folder, checksums, supplement, self.root, root / 'rust-notices')
        changed = copy.deepcopy(supplement)
        changed['source_tree']['sha256'] = delivery.digest(tree_path)
        with self.assertRaisesRegex(ValueError, 'different revision'):
            rust_notices.source_correspondence(folder, checksums, changed, self.root, root / 'rust-notices')

    def test_standard_library_notice_requires_exact_release_component_and_bytes(self):
        sysroot = self.root / 'sysroot'
        html = sysroot / 'share/doc/rust/COPYRIGHT-library.html'
        html.parent.mkdir(parents=True)
        original = b'<html>Original Rust library copyright and third-party terms</html>\n'
        html.write_bytes(original)
        manifest = sysroot / 'lib/rustlib/manifest-rustc-fixture-target'
        manifest.parent.mkdir(parents=True)
        manifest.write_text('file:share/doc/rust/COPYRIGHT-library.html\n')
        spec = {'version': '1.98.1', 'source_commit': COMMIT, 'sha256': delivery.digest(html)}
        compiler = 'release: 1.98.1\ncommit-hash: ' + COMMIT + '\nhost: fixture-target\n'
        output = self.root / 'notices'
        output.mkdir()
        record = rust_notices.standard_library_notices(spec, output, sysroot, compiler)
        self.assertEqual((output / record['path']).read_bytes(), original)
        self.assertEqual(record['status'], 'unreviewed-original-standard-library-notice')
        with self.assertRaisesRegex(ValueError, 'pinned Rust release/commit'):
            rust_notices.standard_library_notices(spec, output, sysroot, compiler.replace(COMMIT, 'b' * 40))
        html.write_bytes(b'altered copyright')
        with self.assertRaisesRegex(ValueError, 'differs from the pinned original'):
            rust_notices.standard_library_notices(spec, output, sysroot, compiler)
        html.write_bytes(original)
        manifest.write_text('file:share/doc/rust/another.html\n')
        with self.assertRaisesRegex(ValueError, 'not recorded'):
            rust_notices.standard_library_notices(spec, output, sysroot, compiler)

    def test_macos_sources_preserve_resources_and_all_patch_forms(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            recipe = b'class Tiny\nend\n__END__\nembedded patch\n'
            local = b'local patch\n'
            core = root / "core.tar"
            with tarfile.open(core, "w") as archive:
                for name, data in {"Formula/t/tiny.rb": recipe, "Patches/tiny.diff": local}.items():
                    member = tarfile.TarInfo("homebrew-core-" + COMMIT + "/" + name)
                    member.size = len(data)
                    archive.addfile(member, io.BytesIO(data))
            payload = root / "download"
            payload.write_bytes(b'pinned source bytes')
            pinned = {"url": "https://example.invalid/source", "sha256": delivery.digest(payload)}
            formula = {"recipe": "Formula/t/tiny.rb", "recipe_sha256": delivery.hashlib.sha256(recipe).hexdigest(),
                       "bottle": {}, "source": {"url": pinned["url"], "checksum": pinned["sha256"]},
                       "resources": [pinned | {"name": "dependency"}, pinned | {"name": "Test::Harness"}],
                       "patches": [pinned | {"strip": "p1"},
                                   {"file": "Patches/tiny.diff", "sha256": delivery.hashlib.sha256(local).hexdigest()},
                                   {"data": True, "sha256": delivery.hashlib.sha256(b'embedded patch\n').hexdigest()}]}
            lock = {"core_commit": COMMIT, "core": {"sha256": delivery.digest(core)}, "formulae": {"tiny": formula}}
            incomplete = copy.deepcopy(lock)
            incomplete['formulae']['tiny'].pop('resources')
            with self.assertRaisesRegex(ValueError, "Incomplete Homebrew source inputs"):
                delivery.macos_source_materials(incomplete, core, root / "cache", root / "incomplete")
            self.assertFalse((root / "incomplete").exists())
            with patch.object(delivery, "fetch", return_value=payload):
                output = root / "materials"
                delivery.macos_source_materials(lock, core, root / "cache", output)
                record = delivery.read(output / "SOURCES.json")
                self.assertEqual(record["status"], "unreviewed-source-materials")
                self.assertEqual((output / "tiny/resource-dependency.archive").read_bytes(), payload.read_bytes())
                self.assertEqual((output / "tiny/resource-Test%3A%3AHarness.archive").read_bytes(), payload.read_bytes())
                self.assertEqual(record['formulae']['tiny']['resources'][1]['name'], 'Test::Harness')
                self.assertEqual((output / "tiny/patch-1.patch").read_bytes(), local)
                self.assertEqual(record['formulae']['tiny']['patches'][1]['source_file'], 'Patches/tiny.diff')
                self.assertEqual((output / "tiny/patch-2.patch").read_bytes(), b'embedded patch\n')
                bad = copy.deepcopy(lock)
                bad["formulae"]["tiny"]["patches"][1]["sha256"] = "0" * 64
                with self.assertRaisesRegex(ValueError, "patch checksum"):
                    delivery.macos_source_materials(bad, core, root / "cache", root / "bad")
                bad = copy.deepcopy(lock)
                bad["formulae"]["tiny"]["recipe_sha256"] = "0" * 64
                with self.assertRaisesRegex(ValueError, "Formula changed"):
                    delivery.macos_source_materials(bad, core, root / "cache", root / "bad-recipe")
                with self.assertRaises(FileExistsError):
                    delivery.macos_source_materials(lock, core, root / "cache", output)

    def test_git_sources_require_an_immutable_public_commit(self):
        for source in [{"url": "https://example.invalid/code.git", "revision": "main"},
                       {"url": "https://user:secret@example.invalid/code.git", "revision": COMMIT},
                       {"url": "file:///local/code", "revision": COMMIT},
                       {"url": "https://example.invalid/code.git", "revision": None}]:
            with self.subTest(source=source), self.assertRaises(ValueError), patch.object(delivery, "checked") as run:
                delivery.git_source_archive(source, Path(self.temp.name), Path(self.temp.name) / "source.tar")
                run.assert_not_called()

    def test_git_export_keeps_ignored_sources_and_rejects_submodules(self):
        root = Path(self.temp.name)
        repository = root / "fixture"
        delivery.checked(["git", "init", "--bare", "--template=", repository], capture_output=True)
        def blob(data):
            return delivery.checked(["git", "-C", repository, "hash-object", "-w", "--stdin"],
                                    input=data, capture_output=True).stdout.decode().strip()
        tree = delivery.checked(["git", "-C", repository, "mktree"], input=(
            "100644 blob " + blob(b'kept source\n') + "\tkept.txt\n" +
            "100644 blob " + blob(b'kept.txt export-ignore\n') + "\t.gitattributes\n").encode(),
            capture_output=True).stdout.decode().strip()
        env = delivery.os.environ | {"GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                                      "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"}
        def commit(tree):
            return delivery.checked(["git", "-C", repository, "commit-tree", tree], input=b'fixture\n',
                                    env=env, capture_output=True).stdout.decode().strip()
        revision = commit(tree)
        original = delivery.checked
        def local_fetch(command, **kwargs):
            command = [str(part) for part in command]
            if "fetch" in command:
                command[command.index("protocol.file.allow=never")] = "protocol.file.allow=always"
                command[-2] = str(repository)
            return original(command, **kwargs)
        with patch.object(delivery, "checked", side_effect=local_fetch):
            output = root / "source.tar"
            record = delivery.git_source_archive({"url": "https://example.invalid/code.git", "revision": revision},
                                                 root / "cache", output)
            self.assertEqual(record["revision"], revision)
            with tarfile.open(output) as archive:
                self.assertEqual(archive.extractfile("source/kept.txt").read(), b'kept source\n')
            linked = original(["git", "-C", repository, "mktree"],
                              input=f"160000 commit {revision}\tdependency\n".encode(), capture_output=True).stdout.decode().strip()
            with self.assertRaisesRegex(ValueError, "submodules"):
                delivery.git_source_archive({"url": "https://example.invalid/code.git", "revision": commit(linked)},
                                            root / "cache", root / "submodules.tar")
            self.assertFalse((root / "submodules.tar").exists())

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        (self.bundle / "noh.exe").write_bytes(pe_fixture())
        self.materials = self.root / "materials"
        self.materials.mkdir()
        (self.materials / "LICENSE.txt").write_text("synthetic notice", encoding="utf-8")
        self.manifest = {"schema_version": 2, "version": "0.1.0", "build": {
            "target": "x86_64-pc-windows-gnu", "profile": "release", "features": ["gui", "mcp"],
            "git_revision": COMMIT, "git_dirty": False},
            "sha256": {"noh.exe": delivery.digest(self.bundle / "noh.exe")}}
        delivery.write(self.bundle / "manifest.json", self.manifest)

    def seal(self):
        output = self.root / "assets"
        delivery.seal(self.bundle, self.materials, output, "windows-x64", COMMIT)
        return output

    def test_valid_envelope_and_exact_sizes(self):
        output = self.seal()
        record = delivery.verify_envelope(output)
        self.assertEqual(record["status"], "unqualified")
        self.assertEqual(record["source_commit"], COMMIT)
        self.assertEqual(record["extracted_bytes"], sum(p.stat().st_size for p in self.bundle.iterdir()))

    def test_missing_runtime_blocks_seal_and_wrong_directory_does_not_satisfy_import(self):
        (self.bundle / "noh.exe").write_bytes(pe_fixture(b"vcruntime140.dll", delay=True))
        self.manifest["sha256"]["noh.exe"] = delivery.digest(self.bundle / "noh.exe")
        (self.bundle / "bin").mkdir()
        (self.bundle / "bin/vcruntime140.dll").write_bytes(pe_fixture())
        self.manifest["sha256"]["bin/vcruntime140.dll"] = delivery.digest(self.bundle / "bin/vcruntime140.dll")
        delivery.write(self.bundle / "manifest.json", self.manifest)
        with self.assertRaisesRegex(ValueError, "Missing bundled.*vcruntime140"):
            self.seal()
        self.assertFalse((self.root / "assets").exists())

    def test_pe_regular_delay_and_transitive_dependencies(self):
        for delay in (False, True):
            report = delivery.audit_pe_images([
                ("bin/whisper-cli.exe", pe_fixture(b"GGML.DLL", delay)),
                ("bin/ggml.dll", pe_fixture(b"vcomp140.dll"))])
            self.assertEqual(report["missing"], [{"image": "bin/ggml.dll", "library": "vcomp140.dll"}])
            self.assertEqual(report["images"]["bin/whisper-cli.exe"]["imports"]["delay" if delay else "normal"], ["ggml.dll"])
            complete = delivery.audit_pe_images([
                ("bin/whisper-cli.exe", pe_fixture(b"GGML.DLL", delay)),
                ("bin/ggml.dll", pe_fixture(b"vcomp140.dll")),
                ("bin/vcomp140.dll", pe_fixture(b"kernel32.dll"))])
            delivery.require_pe_closure(complete)

    def test_envelope_rechecks_imports_even_with_consistent_producer_hashes(self):
        output = self.seal()
        record = delivery.read(output / "DELIVERY.json")
        app = next(p for p in output.glob("*.zip") if not p.name.endswith("-materials.zip"))
        (self.bundle / "noh.exe").write_bytes(pe_fixture(b"vcruntime140.dll"))
        self.manifest["sha256"]["noh.exe"] = delivery.digest(self.bundle / "noh.exe")
        delivery.write(self.bundle / "manifest.json", self.manifest)
        app.unlink()
        delivery.zip_folder(self.bundle, app)
        record["bundle_files"] = delivery.inventory(self.bundle)
        record["assets"][app.name] = {"sha256": delivery.digest(app), "size": app.stat().st_size}
        delivery.write(output / "DELIVERY.json", record)
        (output / "SHA256SUMS").write_text("".join(
            f"{delivery.digest(p)}  {p.name}\n" for p in sorted(output.iterdir())
            if p.name != "SHA256SUMS"), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Missing bundled.*vcruntime140"):
            delivery.verify_envelope(output)

    def test_pe_windows_driver_and_crt_classification(self):
        for name in (b"d2d1.dll", b"api-ms-win-core-file-l1-1-0.dll"):
            delivery.require_pe_closure(delivery.audit_pe_images([("bin/libmpv-2.dll", pe_fixture(name))]))
        report = delivery.audit_pe_images([("bin/ggml-cuda.dll", pe_fixture(b"nvcuda.dll"))])
        self.assertEqual(report["images"]["bin/ggml-cuda.dll"]["dependencies"]["nvcuda.dll"]["kind"], "external_nvidia_driver")
        delivery.require_pe_closure(report)
        for name in (b"msvcp140.dll", b"vcruntime140.dll", b"vcruntime140_1.dll", b"vcomp140.dll", b"nvcuda.dll"):
            with self.assertRaisesRegex(ValueError, "Missing bundled"):
                delivery.require_pe_closure(delivery.audit_pe_images([("noh.exe", pe_fixture(name))]))

    def test_external_crt_is_limited_to_exact_speech_images(self):
        locked = delivery.read(delivery.ROOT / "assets/speech-bundle.json")
        for dependency in locked["windows_prerequisite"]["files"]:
            for delay in (False, True):
                data = pe_fixture(dependency.encode(), delay)
                spec = copy.deepcopy(locked)
                spec["sha256"]["whisper-cli.exe"] = hashlib.sha256(data).hexdigest()
                with patch.object(delivery, "read", return_value=spec):
                    report = delivery.audit_pe_images([("bin/speech/whisper-cli.exe", data)])
                    delivery.require_pe_closure(report)
                    self.assertEqual(report["images"]["bin/speech/whisper-cli.exe"]["dependencies"][dependency]["kind"],
                                     "external_visual_cpp_runtime")
                    self.assertEqual(report["external_prerequisites"], [spec["windows_prerequisite"]])
                    for name in ("noh.exe", "bin/whisper-cli.exe", "bin/speech/unrecognised.exe"):
                        with self.assertRaisesRegex(ValueError, "Missing bundled"):
                            delivery.require_pe_closure(delivery.audit_pe_images([(name, data)]))
                spec["sha256"]["whisper-cli.exe"] = "0" * 64
                with patch.object(delivery, "read", return_value=spec):
                    with self.assertRaisesRegex(ValueError, "Missing bundled"):
                        delivery.require_pe_closure(delivery.audit_pe_images([("bin/speech/whisper-cli.exe", data)]))

    def test_windows_materials_cannot_reintroduce_microsoft_payloads(self):
        for name in ("native/visual-cpp/payload.cab", "native/VC_redist.x64.exe", "speech/VCOMP140.DLL"):
            with self.subTest(name=name):
                bad = self.materials / name
                bad.parent.mkdir(parents=True, exist_ok=True)
                bad.write_bytes(b"not redistributable by this package")
                with self.assertRaisesRegex(ValueError, "Microsoft runtime payload"):
                    self.seal()
                bad.unlink()
        self.assertFalse((self.root / "assets").exists())

    def test_empty_default_feature_matches_cargo_build_identity(self):
        self.manifest["build"]["features"].append("default")
        delivery.write(self.bundle / "manifest.json", self.manifest)
        self.seal()

    def test_pe_malformed_images_fail_without_execution(self):
        for data in (b"not PE", pe_fixture()[:80], pe_fixture()[:350], pe_fixture(b"../runtime.dll")):
            with self.assertRaises(ValueError):
                delivery.pe_imports(data)
        unterminated = pe_fixture(b"runtime.dll")
        struct.pack_into("<I", unterminated, 88 + 112 + 8 + 4, 20)
        with self.assertRaisesRegex(ValueError, "Unterminated PE import table"):
            delivery.pe_imports(unterminated)

    def test_extra_missing_and_mutated_files_are_rejected(self):
        output = self.seal()
        (output / "surprise.txt").write_bytes(b"extra")
        with self.assertRaisesRegex(ValueError, "extra"):
            delivery.verify_envelope(output)
        (output / "surprise.txt").unlink()
        app = next(p for p in output.glob("*.zip") if not p.name.endswith("-materials.zip"))
        app.write_bytes(app.read_bytes() + b"modified")
        with self.assertRaisesRegex(ValueError, "integrity"):
            delivery.verify_envelope(output)
        app.unlink()
        with self.assertRaisesRegex(ValueError, "extra"):
            delivery.verify_envelope(output)

    def test_bundle_manifest_rejects_extra_corrupt_and_mixed_builds(self):
        (self.bundle / "other.dll").write_bytes(b"unknown")
        with self.assertRaisesRegex(ValueError, "inventory"):
            delivery.verify_bundle(self.bundle, self.manifest["build"]["target"], COMMIT)
        (self.bundle / "other.dll").unlink()
        (self.bundle / "noh.exe").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "checksum"):
            delivery.verify_bundle(self.bundle, self.manifest["build"]["target"], COMMIT)
        for field, value in [("git_dirty", True), ("git_revision", "b" * 40),
                             ("features", ["gui", "mcp", "updates"]), ("profile", "qa")]:
            modified = copy.deepcopy(self.manifest)
            modified["build"][field] = value
            delivery.write(self.bundle / "manifest.json", modified)
            with self.assertRaisesRegex(ValueError, "identity"):
                delivery.verify_bundle(self.bundle, self.manifest["build"]["target"], COMMIT)

    def test_cache_checksum_is_revalidated_without_network(self):
        cache = self.root / "cache"
        cache.mkdir()
        path = cache / "a.bin"
        path.write_bytes(b"good")
        source = {"url": "https://example.invalid/a.bin", "sha256": delivery.digest(path)}
        with patch.object(delivery, "checked", side_effect=AssertionError("Network forbidden")):
            self.assertEqual(delivery.fetch(source, cache, "a.bin"), path)
            path.write_bytes(b"wrong")
            with self.assertRaisesRegex(ValueError, "Corrupt cached"):
                delivery.fetch(source, cache, "a.bin")

    def test_explicit_source_download_url_stays_https_and_checksum_pinned(self):
        cache = self.root / 'cache'
        data = b'original pinned bytes'
        source = {'url': 'https://ftpmirror.gnu.org/project/archive.tar',
                  'download_url': 'https://ftp.gnu.org/gnu/project/archive.tar',
                  'sha256': delivery.hashlib.sha256(data).hexdigest()}
        def download(command, **kwargs):
            self.assertEqual(command[-1], source['download_url'])
            self.assertIn('--proto-redir', command)
            Path(command[command.index('--output') + 1]).write_bytes(data)
        with patch.object(delivery, 'checked', side_effect=download):
            self.assertEqual(delivery.fetch(source, cache, 'a.tar').read_bytes(), data)
        source['download_url'] = 'http://example.invalid/archive.tar'
        with self.assertRaisesRegex(ValueError, 'insecure'), patch.object(delivery, 'checked') as run:
            delivery.fetch(source, cache, 'b.tar')
            run.assert_not_called()

    def test_unsafe_paths_duplicates_and_symlinks_are_rejected(self):
        for name in ("../outside", "/absolute", "C:/path", "a\\b", "a/./b", "NUL.txt", "name.", "name "):
            with self.subTest(name=name), self.assertRaises(ValueError):
                delivery.relative(name)
        for names, symbolic in [(["a", "A"], False), (["../escape"], False), (["link"], True)]:
            data = io.BytesIO()
            with zipfile.ZipFile(data, "w") as z:
                for name in names:
                    info = zipfile.ZipInfo(name)
                    if symbolic:
                        info.external_attr = (stat.S_IFLNK | 0o777) << 16
                    z.writestr(info, b"payload")
            data.seek(0)
            with zipfile.ZipFile(data) as z, self.assertRaises(ValueError):
                delivery.zip_members(z)

    def test_zip_internal_inventory_and_content_are_rechecked(self):
        output = self.seal()
        app = next(p for p in output.glob("*.zip") if not p.name.endswith("-materials.zip"))
        expected = delivery.inventory(self.bundle)
        expected["noh.exe"]["sha256"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "ZIP checksum"):
            delivery.verify_zip(app, expected)
        expected.pop("noh.exe")
        with self.assertRaisesRegex(ValueError, "ZIP inventory"):
            delivery.verify_zip(app, expected)

    def test_limit_and_no_overwrite(self):
        path = self.root / "output.zip"
        with patch.object(delivery, "LIMIT", 1), self.assertRaisesRegex(ValueError, "2 GiB"):
            delivery.zip_folder(self.bundle, path)
        with self.assertRaisesRegex(ValueError, "overwrite"):
            delivery.zip_folder(self.bundle, path)

    def test_existing_gaps_and_no_authority_cannot_be_self_qualified(self):
        output = self.seal()
        policy = copy.deepcopy(delivery.read(delivery.ROOT / "assets/delivery-policy.json"))
        original = delivery.read
        with patch.object(delivery, "authorize_redistribution"), patch.object(delivery, "read", side_effect=lambda p: policy if Path(p).name == "delivery-policy.json" else original(p)):
            policy["platforms"]["windows-x64"]["unresolved"] = ["Unverified native fixture"]
            with self.assertRaisesRegex(ValueError, "Public draft blocked"):
                delivery.qualify(output)
            policy["platforms"]["windows-x64"]["unresolved"] = []
            policy["qualification_records"] = []
            with self.assertRaisesRegex(ValueError, "independently reviewed"):
                delivery.qualify(output)

    def test_reviewed_record_requires_exact_final_hash_and_evidence(self):
        output = self.seal()
        policy = copy.deepcopy(delivery.read(delivery.ROOT / "assets/delivery-policy.json"))
        policy["platforms"]["windows-x64"]["unresolved"] = []
        policy["qualification_records"] = [{"source_commit": COMMIT, "platform": "windows-x64",
                                           "delivery_sha256": delivery.digest(output / "DELIVERY.json"), "evidence": {}}]
        original = delivery.read
        with patch.object(delivery, "authorize_redistribution"), patch.object(delivery, "read", side_effect=lambda p: policy if Path(p).name == "delivery-policy.json" else original(p)):
            with self.assertRaisesRegex(ValueError, "Incomplete reviewed"):
                delivery.qualify(output)

    def test_vendor_configuration_is_portable_and_keeps_source_tables(self):
        raw = '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "C:/producer/build/materials/vendor"\n'
        result = delivery.portable_vendor_config(raw)
        self.assertIn('directory = "vendor"', result)
        self.assertIn('[source.crates-io]', result)
        self.assertNotIn('producer', result)
        self.assertNotIn('C:', result)
        with self.assertRaises(ValueError):
            delivery.portable_vendor_config('no source replacement')

    def test_public_artifacts_require_independent_redistribution_authority(self):
        original = delivery.read
        policy = copy.deepcopy(original(delivery.ROOT / "assets/delivery-policy.json"))
        with patch.object(delivery, "read", side_effect=lambda p: policy if Path(p).name == "delivery-policy.json" else original(p)):
            policy["platforms"]["windows-x64"]["redistribution_unresolved"] = ["Unreviewed fixture"]
            with self.assertRaisesRegex(ValueError, "Public artifact upload blocked"):
                delivery.authorize_redistribution("windows-x64")
            policy["platforms"]["windows-x64"]["redistribution_unresolved"] = []
            policy["redistribution_records"] = []
            with self.assertRaisesRegex(ValueError, "reviewed redistribution"):
                delivery.authorize_redistribution("windows-x64")

    def test_json_metadata_hashes_survive_git_lf_checkout(self):
        output = self.root / 'locked.json'
        delivery.write(output, {'fixture': ['first', 'second']})
        self.assertNotIn(b'\r\n', output.read_bytes())
        self.assertEqual(delivery.digest(output), hashlib.sha256(output.read_bytes().replace(b'\r\n', b'\n')).hexdigest())

    def test_provenance_run_boundaries(self):
        run = {"repository": {"full_name": "owner/noh"}, "head_repository": {"full_name": "owner/noh"},
               "workflow_id": 123, "head_sha": COMMIT, "head_branch": "main", "event": "workflow_dispatch",
               "status": "completed", "conclusion": "success", "run_attempt": 1,
               "path": ".github/workflows/delivery-candidates.yml"}
        release.validate_run(run, "owner/noh", 123, COMMIT, 1)
        release.validate_run(run | {"path": ".github/workflows/release.yml"}, "owner/noh", 123, COMMIT, 1)
        for field, value in [("workflow_id", 124), ("head_sha", "b" * 40), ("head_branch", "other"),
                             ("event", "pull_request"), ("run_attempt", 2), ("conclusion", "failure"),
                             ("status", "in_progress"), ("path", ".github/workflows/other.yml"),
                             ("repository", {"full_name": "other/noh"})]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                release.validate_run(run | {field: value}, "owner/noh", 123, COMMIT, 1)
        with self.assertRaises(ValueError):
            release.validate_run(run | {"head_repository": {"full_name": "fork/noh"}}, "owner/noh", 123, COMMIT, 1)

    def test_provenance_requires_successful_controls_audit_and_exact_platform_jobs(self):
        platforms = ["windows-x64", "macos-arm64"]
        jobs = [{"name": name, "conclusion": "success"} for name in
                ["controls", "rust-audit / Audit Cargo.lock against RustSec",
                 "Candidate windows-x64", "Candidate macos-arm64"]]
        workflow = ".github/workflows/delivery-candidates.yml"
        release.validate_jobs({"total_count": len(jobs), "jobs": jobs}, platforms, workflow)
        for index in range(len(jobs)):
            for conclusion in ["failure", "skipped", "cancelled", None]:
                changed = [job | {"conclusion": conclusion} if i == index else job
                           for i, job in enumerate(jobs)]
                with self.subTest(index=index, conclusion=conclusion), self.assertRaises(ValueError):
                    release.validate_jobs({"total_count": len(changed), "jobs": changed}, platforms, workflow)
            missing = jobs[:index] + jobs[index + 1:]
            with self.subTest(missing=index), self.assertRaises(ValueError):
                release.validate_jobs({"total_count": len(missing), "jobs": missing}, platforms, workflow)
        for changed in [jobs + [jobs[0]], jobs + [{"name": "unexpected", "conclusion": "success"}]]:
            with self.assertRaises(ValueError):
                release.validate_jobs({"total_count": len(changed), "jobs": changed}, platforms, workflow)
        with self.assertRaises(ValueError):
            release.validate_jobs({"total_count": len(jobs) + 1, "jobs": jobs}, platforms, workflow)
        with self.assertRaises(ValueError):
            release.validate_jobs({"total_count": len(jobs), "jobs": jobs}, ["windows-x64"], workflow)

    def test_release_wrapper_requires_preflight_and_only_skipped_unused_draft(self):
        workflow = ".github/workflows/release.yml"
        jobs = [{"name": name, "conclusion": "success"} for name in
                ["preflight", "build / controls", "build / rust-audit / Audit Cargo.lock against RustSec",
                 "build / Candidate windows-x64"]]
        def validate(items):
            release.validate_jobs({"total_count": len(items), "jobs": items}, ["windows-x64"], workflow)
        validate(jobs)
        validate(jobs + [{"name": "draft", "conclusion": "skipped"}])
        children = [{"name": name, "conclusion": "skipped"} for name in ["draft / verify", "draft / draft"]]
        validate(jobs + children)
        for changed in [jobs[1:], jobs + [{"name": "draft", "conclusion": "success"}],
                        jobs + [{"name": "draft / verify", "conclusion": "failure"}],
                        jobs + [{"name": "untrusted", "conclusion": "skipped"}],
                        jobs + [jobs[0]],
                        [job | {"name": job["name"].removeprefix("build / ")} for job in jobs]]:
            with self.assertRaises(ValueError):
                validate(changed)

    def test_optional_audit_artifact_is_exact_and_never_staged_as_a_package(self):
        artifact = {"name": "delivery-windows-x64", "expired": False, "digest": "sha256:" + "a" * 64}
        audit = artifact | {"name": "rust-audit-123-2"}
        def validate(items):
            return release.validate_artifacts({"total_count": len(items), "artifacts": items},
                                              ["windows-x64"], "rust-audit-123-2")
        self.assertEqual(validate([artifact]), [artifact])
        self.assertEqual(validate([artifact, audit]), [artifact])
        for changed in [[audit], [artifact, audit, audit], [artifact, artifact],
                        [artifact, audit | {"name": "rust-audit-123-1"}],
                        [artifact, audit | {"name": "rust-audit-124-2"}],
                        [artifact, audit | {"expired": True}],
                        [artifact, audit | {"digest": None}],
                        [artifact, audit | {"name": "unknown"}]]:
            with self.assertRaises(ValueError):
                validate(changed)
        with self.assertRaises(ValueError):
            release.validate_artifacts({"total_count": 3, "artifacts": [artifact, audit]},
                                       ["windows-x64"], "rust-audit-123-2")

    def test_artifact_inventory_missing_extra_expired_and_digest(self):
        artifact = {"name": "delivery-windows-x64", "expired": False, "digest": "sha256:" + "a" * 64}
        release.validate_artifacts({"total_count": 1, "artifacts": [artifact]}, ["windows-x64"])
        for data in ({"total_count": 0, "artifacts": []},
                     {"total_count": 2, "artifacts": [artifact, artifact]},
                     {"total_count": 1, "artifacts": [artifact | {"expired": True}]},
                     {"total_count": 1, "artifacts": [artifact | {"digest": None}]}):
            with self.assertRaises(ValueError):
                release.validate_artifacts(data, ["windows-x64"])
        with self.assertRaises(ValueError):
            release.validate_artifacts({"total_count": 1, "artifacts": [artifact]}, ["windows-x64", "macos-arm64"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
