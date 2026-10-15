"""Check candidate immutability, native-case setup and real preview-audio evidence."""
import argparse
import contextlib
import io
import json
import math
from pathlib import Path
import struct
import tempfile
import unittest
import wave

import linux_desktop_candidate as desktop
from delivery import inventory


class DesktopEvidenceTests(unittest.TestCase):
    def test_xorg_uses_observed_shell_environment_without_inventing_desktop_values(self):
        observed = desktop.desktop_environment('DISPLAY=:1\nUNRELATED=excluded\n',
                    b'DISPLAY=:0\0XAUTHORITY=/run/user/1000/gdm/Xauthority\0XDG_CURRENT_DESKTOP=GNOME\0')
        self.assertEqual(observed, {'DISPLAY': ':0', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                                    'XDG_CURRENT_DESKTOP': 'GNOME'})
        self.assertNotIn('XDG_CURRENT_DESKTOP', desktop.desktop_environment('', b'DISPLAY=:0\0'))

    def test_configured_images_match_required_cases(self):
        for name in ('ubuntu-24.04', 'ubuntu-26.04', 'debian-13', 'fedora-44', 'opensuse-tumbleweed', 'arch'):
            row, image = desktop.case_inputs(name)
            self.assertEqual(row['id'], name)
            self.assertEqual(row['desktop'], 'KDE' if name in ('arch', 'opensuse-tumbleweed') else 'GNOME')
            self.assertTrue(image['url'].startswith('https://'))
            self.assertEqual(image['version_id'] is None, name in ('arch', 'opensuse-tumbleweed'))
        for name in ('mint-22.3', 'not-a-case'):
            with self.assertRaises(ValueError):
                desktop.case_inputs(name)

    def test_debian_uses_xorg_session_and_pulseaudio(self):
        with tempfile.TemporaryDirectory() as folder:
            key, output = Path(folder) / 'key.pub', Path(folder) / 'user-data'
            key.write_text('ssh-ed25519 fixture-only')
            desktop.cloud_config(argparse.Namespace(case='debian-13', public_key=key, output=output))
            config = json.loads(output.read_text().split('\n', 1)[1])
            self.assertNotIn('groups', config)  # These vendor groups already exist in all pinned images.
            self.assertTrue({'audio', 'video', 'render'}.issubset(config['users'][0]['groups']))
            self.assertEqual(config['write_files'][0]['path'], '/etc/gdm3/daemon.conf')
            self.assertIn('gnome-session-xsession', config['packages'])
            self.assertIn('pulseaudio', config['packages'])
            self.assertNotIn('pipewire-audio', config['packages'])
            self.assertIn('WaylandEnable=false', config['write_files'][0]['content'])
            self.assertIn('DefaultSession=gnome-xorg.desktop', config['write_files'][0]['content'])
            for name in ('ubuntu-24.04', 'ubuntu-26.04'):
                desktop.cloud_config(argparse.Namespace(case=name, public_key=key, output=output))
                config = json.loads(output.read_text().split('\n', 1)[1])
                self.assertNotIn('groups', config)
                self.assertEqual(config['write_files'][0]['path'], '/etc/gdm3/custom.conf')
                self.assertIn('WaylandEnable=true', config['write_files'][0]['content'])

    def test_guest_rejects_changed_or_added_candidate_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            app = root / 'NOH.AppDir'
            app.mkdir()
            file = app / 'AppRun'
            file.write_text('original')
            identity = {'bundle_files': inventory(root)}
            desktop.verify_bundle(root, identity)
            file.write_text('modified')
            with self.assertRaisesRegex(ValueError, 'changed'):
                desktop.verify_bundle(root, identity)
            file.write_text('original')
            (app / 'unexpected.so').write_bytes(b'extra')
            with self.assertRaisesRegex(ValueError, 'additional'):
                desktop.verify_bundle(root, identity)

    def test_rpm_arch_desktops_select_real_sessions_and_boot_installed_kernel(self):
        with tempfile.TemporaryDirectory() as folder:
            key, output = Path(folder) / 'key.pub', Path(folder) / 'user-data'
            key.write_text('ssh-ed25519 fixture-only')
            for name, manager, path, session, kernel in (
                ('fedora-44', 'gdm', '/etc/gdm/custom.conf', 'gnome.desktop', 'kernel-modules-extra'),
                ('opensuse-tumbleweed', 'sddm', '/etc/sddm.conf.d/noh-test.conf', 'plasma6.desktop', 'kernel-default'),
                ('arch', 'sddm', '/etc/sddm.conf.d/noh-test.conf', 'plasma.desktop', 'linux'),
            ):
                with self.subTest(name=name):
                    desktop.cloud_config(argparse.Namespace(case=name, public_key=key, output=output))
                    config = json.loads(output.read_text().split('\n', 1)[1])
                    image = json.loads(output.with_suffix('.image.json').read_text())
                    self.assertTrue(image['reboot_after_provisioning'])
                    self.assertNotIn('groups', config['users'][0])
                    self.assertEqual(config['write_files'][0]['path'], path)
                    self.assertIn(f'Session={session}', config['write_files'][0]['content'])
                    self.assertIn(['systemctl', 'enable', manager], config['runcmd'])
                    self.assertFalse(any(cmd[:2] == ['systemctl', 'restart'] for cmd in config['runcmd']))
                    self.assertTrue(any(kernel in cmd for cmd in config['runcmd']))
    def test_audio_rejects_silence_and_wrong_frequency(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for frequency in (0, 440, 1000):
                path = root / f'{frequency}.wav'
                with wave.open(str(path), 'wb') as output:
                    output.setparams((2, 2, 44100, 0, 'NONE', 'not compressed'))
                    output.writeframes(b''.join(struct.pack('<hh', *([int(5000 * math.sin(2 * math.pi * frequency * n / 44100))] * 2))
                                               for n in range(22050)))
                args = argparse.Namespace(input=path, output=root / f'{frequency}.json')
                with contextlib.redirect_stdout(io.StringIO()):
                    if frequency == 440:
                        desktop.audio_report(args)
                    else:
                        with self.assertRaisesRegex(ValueError, 'preview tone'):
                            desktop.audio_report(args)


if __name__ == '__main__':
    unittest.main()
