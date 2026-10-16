"""Check candidate immutability, native-case setup and real preview-audio evidence."""
import argparse
import contextlib
import io
import json
import math
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import wave

import linux_desktop_candidate as desktop
from delivery import inventory


class DesktopEvidenceTests(unittest.TestCase):
    @unittest.skipUnless(all(shutil.which(name) for name in
        ('debconf-set-selections', 'debconf-communicate', 'openssl', 'sh')),
        'Requires the Debian installer tools used by the native Linux CI job')
    def test_mint_commands_survive_real_debconf_storage_and_protocol(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            config = root / 'debconf.conf'
            config.write_text('Config: configdb\nTemplates: templatedb\n\n' + '\n'.join(
                f'Name: {name}\nDriver: File\nFilename: {root / name}\nMode: 600\n'
                for name in ('configdb', 'templatedb')))
            # Isolate both databases; never modify the runner's debconf state.
            env = {key: value for key, value in os.environ.items() if not key.startswith('DEBCONF_')}
            env['DEBCONF_SYSTEMRC'] = str(config)
            key, seed = root / 'key.pub', root / 'preseed.cfg'
            key.write_text('ssh-ed25519 Zml4dHVyZQ==')
            desktop.mint_preseed(argparse.Namespace(public_key=key, output=seed))
            subprocess.run(['debconf-set-selections', str(seed)], env=env, check=True,
                           capture_output=True, text=True, timeout=20)
            for line in seed.read_text().splitlines():
                if '_command string ' not in line:
                    continue
                _, question, _, expected = line.split(' ', 3)
                result = subprocess.run(['debconf-communicate', '-fnoninteractive', 'ubiquity'],
                    input=f'GET {question}\n', env=env, check=True,
                    capture_output=True, text=True, timeout=20)
                self.assertEqual(result.stdout[:2], '0 ')
                actual = result.stdout[2:].removesuffix('\n')
                self.assertTrue(actual == expected,
                    f'{question}: debconf changed/truncated {len(expected)} bytes to {len(actual)}')
                subprocess.run(['sh', '-n'], input=actual, check=True,
                               capture_output=True, text=True, timeout=20)

    def test_acquisition_cleanup_preserves_verified_payload_and_local_envelope(self):
        def envelope_at(path):
            path.mkdir()
            (path / 'DELIVERY.json').write_text('{}')
            return path

        def acquire(args, destination):
            (destination / 'transport.zip').write_bytes(b'fixture transport')
            return envelope_at(destination / 'envelope')

        def unpack(envelope, payload):
            app = payload / 'NOH.AppDir'
            app.mkdir()
            (app / 'AppRun').write_text('fixture executable')
            return {'source_commit': 'a' * 40, 'run_id': '1', 'run_attempt': '1',
                    'bundle_files': inventory(payload)}, 'b' * 64

        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            external = envelope_at(root / 'caller-envelope')
            for source in (None, external):
                output = root / ('acquired' if source is None else 'local')
                args = argparse.Namespace(commit='a' * 40, run=1, attempt=1, workflow=1,
                                          output=output, envelope=source)
                with patch.object(desktop, 'acquire', acquire), patch.object(desktop, 'unpack_candidate', unpack), \
                        contextlib.redirect_stdout(io.StringIO()):
                    desktop.prepare(args)
                self.assertTrue((external / 'DELIVERY.json').is_file())
                self.assertTrue((output / 'CANDIDATE.json').is_file())
                with desktop.tarfile.open(output / 'payload.tar') as archive:
                    self.assertEqual(archive.extractfile('NOH.AppDir/AppRun').read(), b'fixture executable')
                if source is None:
                    self.assertFalse((output / 'envelope').exists())
                    self.assertFalse((output / 'payload').exists())
                    self.assertFalse((output / 'transport.zip').exists())
                else:
                    self.assertTrue((output / 'payload/NOH.AppDir/AppRun').is_file())

    def test_xorg_uses_observed_shell_environment_without_inventing_desktop_values(self):
        observed = desktop.desktop_environment('DISPLAY=:1\nUNRELATED=excluded\n',
                    b'DISPLAY=:0\0XAUTHORITY=/run/user/1000/gdm/Xauthority\0XDG_CURRENT_DESKTOP=GNOME\0')
        self.assertEqual(observed, {'DISPLAY': ':0', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                                    'XDG_CURRENT_DESKTOP': 'GNOME'})
        self.assertNotIn('XDG_CURRENT_DESKTOP', desktop.desktop_environment('', b'DISPLAY=:0\0'))
        kde = desktop.desktop_environment('KDE_SESSION_VERSION=5\n',
                    b'XDG_CURRENT_DESKTOP=KDE\0KDE_SESSION_VERSION=6\0KDE_FULL_SESSION=true\0')
        self.assertEqual(kde, {'XDG_CURRENT_DESKTOP': 'KDE', 'KDE_SESSION_VERSION': '6', 'KDE_FULL_SESSION': 'true'})
        self.assertNotIn('KDE_SESSION_VERSION', desktop.desktop_environment('', b'DISPLAY=:0\0'))

    def test_configured_images_match_required_cases(self):
        for name in ('ubuntu-24.04', 'ubuntu-26.04', 'debian-13', 'fedora-44', 'opensuse-tumbleweed', 'arch', 'mint-22.3'):
            row, image = desktop.case_inputs(name)
            self.assertEqual(row['id'], name)
            expected = 'KDE' if name in ('arch', 'opensuse-tumbleweed') else ('Cinnamon' if name == 'mint-22.3' else 'GNOME')
            self.assertEqual(row['desktop'], expected)
            self.assertTrue(image['url'].startswith('https://'))
            self.assertEqual(image['version_id'] is None, name in ('arch', 'opensuse-tumbleweed'))
        for name in ('not-a-case',):
            with self.assertRaises(ValueError):
                desktop.case_inputs(name)

    def test_mint_rejects_live_boot_and_requires_installer_success(self):
        for mount, marker in (('overlay overlay', True), ('squashfs /dev/loop0', True),
                              ('ext4 /dev/vda2', False), ('', True)):
            with self.assertRaisesRegex(ValueError, 'installed disk'):
                desktop.require_installed_mint(mount, marker)
        desktop.require_installed_mint('ext4 /dev/vda2', True)
        with tempfile.TemporaryDirectory() as folder:
            key, output = Path(folder) / 'key.pub', Path(folder) / 'user-data'
            key.write_text('ssh-ed25519 fixture-only')
            desktop.cloud_config(argparse.Namespace(case='mint-22.3', public_key=key, output=output))
            config = json.loads(output.read_text().split('\n', 1)[1])
            self.assertEqual(config['write_files'][0]['path'], '/etc/lightdm/lightdm.conf.d/90-noh-test.conf')
            self.assertIn('autologin-session=cinnamon', config['write_files'][0]['content'])
            self.assertIn('pipewire-audio', config['packages'])
            self.assertNotIn('gdm3', config['packages'])

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
                self.assertIn('linux-image-generic', config['packages'])
                self.assertTrue(json.loads(output.with_suffix('.image.json').read_text())['reboot_after_provisioning'])
                self.assertNotIn('uname -r', json.dumps(config))
                self.assertNotIn(['systemctl', 'restart', 'gdm3'], config['runcmd'])

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
                    self.assertNotIn('packages', config)  # An empty list violates cloud-init's schema.
                    self.assertEqual(config['runcmd'][0], 'set -eu')
                    self.assertNotIn('groups', config['users'][0])
                    self.assertEqual(config['write_files'][0]['path'], path)
                    self.assertIn(f'Session={session}', config['write_files'][0]['content'])
                    enable = ['systemctl', 'enable', *(['--force'] if name == 'opensuse-tumbleweed' else []), manager]
                    self.assertIn(enable, config['runcmd'])
                    self.assertFalse(any(cmd[:2] == ['systemctl', 'restart'] for cmd in config['runcmd']))
                    self.assertTrue(any(kernel in cmd for cmd in config['runcmd']))
                    if name == 'opensuse-tumbleweed':
                        self.assertIn('-kernel-default-base', config['runcmd'][1])
                    if name == 'fedora-44':
                        self.assertTrue(config['preserve_hostname'])
                        self.assertNotIn('hostname', config)
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
