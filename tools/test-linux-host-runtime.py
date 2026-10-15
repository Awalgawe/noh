"""Check the host-only package boundary and actionable dependency failures."""
import importlib.util
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

from linux_elf import audit_images, require_host_runtime
import linux_host

spec = importlib.util.spec_from_file_location('elf_fixture', Path(__file__).with_name('test-linux-elf.py'))
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
NAMES = ('noh-app', 'bin/noh-cli', 'bin/noh-mcp', 'bin/speech/whisper-cli')


class HostRuntimeTests(unittest.TestCase):
    def images(self):
        return [('usr/lib/noh/' + n, fixtures.fixture(('libc.so.6',), rpath=None)) for n in NAMES]

    def test_executable_only_boundary_accepts_system_imports(self):
        images = self.images()
        images[0] = (images[0][0], fixtures.fixture(('libasound.so.2',), rpath=None))
        require_host_runtime(audit_images(images, host_runtime=True))

    def test_unknown_shared_library_and_renamed_provider_are_rejected(self):
        for name, soname in [('lib/libfuture-driver.so', 'libfuture-driver.so'), ('bin/extra', None)]:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'private ELF'):
                require_host_runtime(audit_images(self.images() + [('usr/lib/noh/' + name,
                    fixtures.fixture(rpath=None, soname=soname))], host_runtime=True))

    def test_private_search_path_or_missing_worker_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'missing executables'):
            require_host_runtime(audit_images(self.images()[:-1], host_runtime=True))
        images = self.images()
        images[0] = (images[0][0], fixtures.fixture(rpath='$ORIGIN/lib'))
        with self.assertRaisesRegex(ValueError, 'private ELF'):
            require_host_runtime(audit_images(images, host_runtime=True))

    def test_host_ffmpeg_missing_encoder_fails_before_libmpv(self):
        responses = [mock.Mock(stdout='ffmpeg version 8.1.3\n'),
                     mock.Mock(stdout=' A..... aac AAC\n A..... pcm_s16le PCM\n')]
        with mock.patch.object(linux_host.shutil, 'which', return_value='/usr/bin/ffmpeg'), \
                mock.patch.object(linux_host.subprocess, 'run', side_effect=responses), \
                mock.patch.object(linux_host.ctypes, 'CDLL') as mpv:
            with self.assertRaisesRegex(ValueError, 'libx264'):
                linux_host.check_host()
            mpv.assert_not_called()

    def test_missing_ffmpeg_has_an_installation_diagnostic(self):
        with mock.patch.object(linux_host.shutil, 'which', return_value=None), \
                self.assertRaisesRegex(ValueError, 'install the Linux host requirements'):
            linux_host.check_host()

    @unittest.skipUnless(platform.system() == 'Linux', 'Requires a POSIX launcher')
    def test_launcher_preserves_environment_and_arguments(self):
        with tempfile.TemporaryDirectory(prefix='noh launcher ') as tmp:
            appdir = Path(tmp)
            home = appdir / 'usr/lib/noh'
            home.mkdir(parents=True)
            app = home / 'noh-app'
            app.write_text('#!/bin/sh\nprintf "%s\\n" "${ALSA_PLUGIN_DIR-unset}" '
                           '"${LD_LIBRARY_PATH-unset}" "$@"\n')
            app.chmod(0o755)
            launcher = appdir / 'AppRun'
            shutil.copyfile(Path(__file__).resolve().parents[1] / 'assets/linux-AppRun', launcher)
            launcher.chmod(0o755)
            env = {k: v for k, v in os.environ.items()
                   if k not in ('LD_LIBRARY_PATH', 'LD_PRELOAD', 'ALSA_PLUGIN_DIR')}
            for override in (None, '/host custom/plugins'):
                if override:
                    env['ALSA_PLUGIN_DIR'] = override
                result = subprocess.run([str(launcher), 'argument with spaces', ''], env=env,
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines(),
                                 [override or 'unset', 'unset', 'argument with spaces', ''])


if __name__ == '__main__':
    unittest.main()
