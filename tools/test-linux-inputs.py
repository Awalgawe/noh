"""Reject changed native inputs, unsafe payload paths and host-specific CPU builds."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

import linux_runtime

spec = importlib.util.spec_from_file_location('linux_speech', Path(__file__).with_name('prepare-linux-speech.py'))
speech = importlib.util.module_from_spec(spec)
spec.loader.exec_module(speech)


def archive_bytes(entries):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w') as archive:
        for name, data, kind in entries:
            member = tarfile.TarInfo(name)
            member.mode = 0o644
            if kind in ('symlink', 'hardlink'):
                member.type = tarfile.SYMTYPE if kind == 'symlink' else tarfile.LNKTYPE
                member.linkname = data
                archive.addfile(member)
            else:
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
    return buffer.getvalue()


class NativeInputsTests(unittest.TestCase):
    def test_only_official_pool_urls_are_frozen(self):
        self.assertEqual(linux_runtime.ubuntu_url('http://archive.ubuntu.com/ubuntu/pool/a.deb'),
                         'https://archive.ubuntu.com/ubuntu/pool/a.deb')
        for url in ('https://example.com/ubuntu/pool/a.deb',
                    'https://archive.ubuntu.com.evil/ubuntu/pool/a.deb',
                    'https://user@archive.ubuntu.com/ubuntu/pool/a.deb',
                    'https://archive.ubuntu.com:443/ubuntu/pool/a.deb',
                    'https://archive.ubuntu.com/ubuntu/pool/a.deb?key=x',
                    'https://archive.ubuntu.com/elsewhere/a.deb'):
            with self.subTest(url=url), self.assertRaises(ValueError):
                linux_runtime.ubuntu_url(url)

    def copy(self, entries, members, output):
        data = archive_bytes(entries)

        def unpack(command, **kwargs):
            self.assertEqual(command[:2], ['dpkg-deb', '--fsys-tarfile'])
            kwargs['stdout'].write(data)

        with mock.patch.object(linux_runtime.subprocess, 'run', side_effect=unpack):
            linux_runtime.copy_members(Path('input.deb'), members, output)

    @staticmethod
    def member(name='usr/lib/libtest.so.1', data=b'native bytes', mode=0o755):
        return {'member': name, 'sha256': hashlib.sha256(data).hexdigest(), 'mode': mode}

    def test_copy_only_selected_exact_regular_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            self.copy([('./usr/lib/libtest.so.1', b'native bytes', 'file'),
                       ('./unselected', b'not included', 'file')],
                      {'lib/libtest.so.1': self.member()}, output)
            self.assertEqual((output / 'lib/libtest.so.1').read_bytes(), b'native bytes')
            self.assertFalse((output / 'unselected').exists())

    def test_copy_rejects_checksum_mismatch_and_special_modes(self):
        for entry in (self.member(data=b'changed'), self.member(mode=0o4755)):
            with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
                self.copy([('usr/lib/libtest.so.1', b'native bytes', 'file')],
                          {'lib/test': entry}, Path(tmp))

    def test_archive_and_destination_paths_cannot_escape(self):
        for bad in ('../escape', '/escape', 'a/../../escape', 'a\\escape'):
            with self.subTest(path=bad), tempfile.TemporaryDirectory() as tmp:
                with self.assertRaises(ValueError):
                    self.copy([(bad, b'native bytes', 'file')], {}, Path(tmp))
                with self.assertRaises(ValueError):
                    self.copy([('usr/lib/libtest.so.1', b'native bytes', 'file')],
                              {bad: self.member()}, Path(tmp))

    def test_duplicate_members_and_symlinks_are_not_payloads(self):
        duplicate = [('usr/lib/libtest.so.1', b'native bytes', 'file')] * 2
        symlink = [('usr/lib/libtest.so.1', '/etc/passwd', 'symlink')]
        for entries in (duplicate, symlink):
            with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
                self.copy(entries, {'lib/test': self.member()}, Path(tmp))

    def test_hardlinks_are_resolved_without_extracting_links(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.copy([('usr/lib/libtest.so.1', b'native bytes', 'file'),
                       ('usr/lib/alias.so', './usr/lib/libtest.so.1', 'hardlink')],
                      {'lib/test': self.member('usr/lib/alias.so')}, Path(tmp))
            self.assertEqual((Path(tmp) / 'lib/test').read_bytes(), b'native bytes')
        with tempfile.TemporaryDirectory() as tmp, self.assertRaisesRegex(ValueError, 'cyclic'):
            self.copy([('usr/lib/libtest.so.1', 'usr/lib/libtest.so.1', 'hardlink')],
                      {'lib/test': self.member()}, Path(tmp))

    def test_existing_prepared_files_are_preserved(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            (output / 'keep').write_bytes(b'old content')
            with self.assertRaises(ValueError):
                self.copy([('usr/lib/libtest.so.1', b'native bytes', 'file')],
                          {'keep': self.member()}, output)
            self.assertEqual((output / 'keep').read_bytes(), b'old content')

    def test_producer_evidence_is_required_and_preserved_without_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record = root / 'producer'
            record.write_bytes(b'producer bytes')
            records = {'files': {'rav1e.buildinfo': {'size': 14}}}
            with mock.patch.object(linux_runtime, 'fetch', return_value=record):
                linux_runtime.copy_build_records(records, root / 'cache', root / 'materials')
                target = root / 'materials/ubuntu-build-records/rav1e.buildinfo'
                self.assertEqual(target.read_bytes(), record.read_bytes())
                with self.assertRaises(FileExistsError):
                    linux_runtime.copy_build_records(records, root / 'cache', root / 'materials')
                with self.assertRaisesRegex(ValueError, 'size differs'):
                    linux_runtime.copy_build_records({'files': {'other': {'size': 1}}}, root, root)
                with self.assertRaises(ValueError):
                    linux_runtime.copy_build_records({'files': {'../escape': {'size': 14}}}, root, root)
            with self.assertRaises(KeyError):
                linux_runtime.copy_build_records({}, root, root)


class SpeechInputsTests(unittest.TestCase):
    def test_mutated_or_extra_cached_source_is_rejected(self):
        for change in ('modified', 'extra', 'missing'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = root / 'source.tar'
                archive.write_bytes(archive_bytes([('source/main.c', b'original', 'file')]))
                cache = root / 'source'
                speech.source_tree(archive, cache)
                if change == 'modified':
                    (cache / 'source/main.c').write_bytes(b'changed')
                elif change == 'extra':
                    (cache / 'injected').write_bytes(b'new source')
                else:
                    (cache / 'source/main.c').unlink()
                with self.assertRaisesRegex(ValueError, 'differs'):
                    speech.source_tree(archive, cache)

    def test_generated_metadata_must_match_verified_template(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root / 'source.tar'
            folder = f'whisper.cpp-{speech.VERSION}/bindings/javascript/'
            template = b'{"version":"@WHISPER_VERSION@"}\n'
            archive.write_bytes(archive_bytes([(folder + 'package-tmpl.json', template, 'file')]))
            cache = root / 'source'
            speech.source_tree(archive, cache)
            generated = cache / folder / 'package.json'
            for version in (speech.VERSION, speech.VERSION + '-dev'):
                generated.write_bytes(template.replace(b'@WHISPER_VERSION@', version.encode()))
                speech.source_tree(archive, cache)
            generated.write_bytes(b'{"injected":true}')
            with self.assertRaisesRegex(ValueError, 'metadata differs'):
                speech.source_tree(archive, cache)

    def test_source_archive_rejects_links_and_duplicates_before_extraction(self):
        for entries in ([('source/link', '/etc/passwd', 'symlink')],
                        [('source/main.c', b'one', 'file'), ('source/main.c', b'two', 'file')]):
            with tempfile.TemporaryDirectory() as tmp:
                archive = Path(tmp) / 'source.tar'
                archive.write_bytes(archive_bytes(entries))
                with self.assertRaises(ValueError):
                    speech.source_tree(archive, Path(tmp) / 'unpacked')
                self.assertFalse((Path(tmp) / 'unpacked').exists())

    def test_actual_compiler_flags_must_keep_the_baseline(self):
        base = '/usr/bin/c++ -march=x86-64 -mtune=generic -O3 -c file.cpp'
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'commands.json'
            path.write_text(json.dumps([{'command': base}]))
            self.assertEqual(len(speech.check_cpu_commands(path)), 1)
            for flag in ('-mbmi2', '-mavx', '-mavx2', '-mfma', '-mf16c',
                         '-msse4.2', '-mssse3', '-mamx-tile', '-march=native', '-march=haswell'):
                for entry in ({'command': base + ' ' + flag},
                              {'command': base + " '" + flag + "'"},
                              {'command': base + ' "' + flag + '"'},
                              {'command': base, 'arguments': base.split() + [flag]}):
                    path.write_text(json.dumps([entry]))
                    with self.subTest(flag=flag, entry=entry), self.assertRaises(ValueError):
                        speech.check_cpu_commands(path)
            path.write_text(json.dumps([{'arguments': base.split()}]))
            self.assertEqual(len(speech.check_cpu_commands(path)), 1)
        self.assertIn('-DGGML_BMI2=OFF', speech.OPTIONS)
        self.assertIn('-DGGML_NATIVE=OFF', speech.OPTIONS)


if __name__ == '__main__':
    unittest.main()
