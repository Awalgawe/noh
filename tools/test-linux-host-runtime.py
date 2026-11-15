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

    def test_a_capture_timeout_records_the_application_and_the_processes_it_started(self):
        spec = importlib.util.spec_from_file_location('probe_linux_host', Path(__file__).with_name('probe-linux-host.py'))
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        with tempfile.TemporaryDirectory() as folder:
            proc = Path(folder)

            def process(pid, parent, name, threads):
                base = proc / str(pid)
                (base / 'task').mkdir(parents=True)
                (base / 'stat').write_text(f'{pid} ({name}) S {parent} 1 1 0')
                (base / 'cmdline').write_text(name + '\0--flag\0')
                (base / 'status').write_text('State:\tS (sleeping)\n')
                for tid, (state, wait) in threads.items():
                    (base / 'task' / str(tid)).mkdir()
                    (base / 'task' / str(tid) / 'stat').write_text(f'{tid} ({name} x) {state} {parent}')
                    (base / 'task' / str(tid) / 'comm').write_text(name + '\n')
                    (base / 'task' / str(tid) / 'wchan').write_text(wait)

            process(1, 0, 'init', {1: ('S', 'do_wait')})
            process(100, 1, 'noh-app', {100: ('S', 'futex_wait_queue'), 101: ('R', '0')})
            process(200, 100, 'noh worker', {200: ('S', 'poll_schedule_timeout')})
            process(300, 1, 'other', {300: ('R', '0')})
            (proc / 'loadavg').write_text('3.10 2.00 1.00 2/300 999\n')
            state = probe.gui_timeout_state(100, 120.5, proc)
            self.assertEqual(list(state['application']), ['100', '200'])
            self.assertEqual(state['application']['100']['threads']['101'],
                             {'name': 'noh-app', 'state': 'R', 'wchan': '0'})
            self.assertEqual(state['application']['200']['parent'], 100)
            self.assertEqual(state['application']['200']['threads']['200']['wchan'], 'poll_schedule_timeout')
            self.assertEqual(state['application']['100']['cmdline'], 'noh-app --flag')
            self.assertEqual(state['seconds'], 120.5)
            # Every observation is optional: a missing /proc or tool is recorded, never raised.
            probe.gui_timeout_state(100, 1.0, proc / 'absent')


if __name__ == '__main__':
    unittest.main()
