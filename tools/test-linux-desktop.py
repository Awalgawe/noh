"""Check candidate immutability, native-case setup and real preview-audio evidence."""
import argparse
import contextlib
import inspect
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
import linux_matrix
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

    def test_a_combined_build_run_names_every_platform_it_retained(self):
        def inventory_of(*names):
            return {'artifacts': [{'name': name} for name in names]}
        self.assertEqual(desktop.build_platforms(inventory_of('delivery-linux-x64', 'rust-audit-1-1')), ['linux-x64'])
        self.assertEqual(desktop.build_platforms(inventory_of(
            'delivery-windows-x64', 'delivery-linux-x64', 'linux-validation-1-1')), ['linux-x64', 'windows-x64'])

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
            self.assertEqual('rolling' in image, name == 'opensuse-tumbleweed')
            self.assertTrue((image['rolling']['listing'] if 'rolling' in image else image['url']).startswith('https://'))
            self.assertEqual(image['version_id'] is None, name in ('arch', 'opensuse-tumbleweed'))
        for name in ('not-a-case',):
            with self.assertRaises(ValueError):
                desktop.case_inputs(name)

    def test_a_rolling_image_is_the_single_listed_one_with_its_signed_digest(self):
        image = desktop.case_inputs('opensuse-tumbleweed')[1]
        listing = image['rolling']['listing']
        current = 'openSUSE-Tumbleweed-Minimal-VM.x86_64-1.0.0-Cloud-Snapshot20261117.qcow2'
        def served(names, checksum=None):
            files = {listing + '?jsontable': json.dumps({'data': [{'name': n} for n in names]}).encode(),
                     listing + current + '.sha256': checksum or ('f' * 64 + '  ' + current + '\n').encode(),
                     listing + current + '.sha256.asc': b'signature'}
            return lambda url, limit=1 << 20: files[url]
        signed = []
        def verify(data, signature, key, fingerprint):
            signed.append((data, signature, fingerprint))
            self.assertIn('BEGIN PGP PUBLIC KEY BLOCK', key)
        others = [current + '.sha256', current + '.sha256.asc', 'openSUSE-Tumbleweed-Minimal-VM.x86_64-Cloud.qcow2',
                  'openSUSE-Tumbleweed-Minimal-VM.x86_64-1.0.0-kvm-and-xen-Snapshot20261117.qcow2']
        resolved = desktop.resolve_image(image, served([current] + others), verify)
        self.assertEqual(resolved['url'], listing + current)
        self.assertEqual(resolved['digest'], 'f' * 64)
        self.assertEqual(resolved['resolved']['image'], current)
        self.assertEqual(resolved['resolved']['signed_by'], image['rolling']['signing_key_fingerprint'])
        self.assertEqual(signed, [(('f' * 64 + '  ' + current + '\n').encode(), b'signature',
                                   image['rolling']['signing_key_fingerprint'])])
        older = current.replace('20261117', '20261116')
        for names, checksum, message in (([], None, 'lists 0 images'), ([current, older], None, 'lists 2 images'),
                                         ([current], ('f' * 64 + '  ' + older + '\n').encode(), 'does not name'),
                                         ([current], ('f' * 63 + '  ' + current + '\n').encode(), 'does not name')):
            with self.subTest(message), self.assertRaisesRegex(ValueError, message):
                desktop.resolve_image(image, served(names, checksum), verify)
        def refused(*arguments):
            raise ValueError('not signed')
        with self.assertRaisesRegex(ValueError, 'not signed'):
            desktop.resolve_image(image, served([current]), refused)
        pinned = desktop.case_inputs('arch')[1]
        self.assertEqual(desktop.resolve_image(pinned, fetch=None, verify=None), pinned)

    def test_the_guest_records_the_image_the_runner_resolved(self):
        rolling, pinned = desktop.case_inputs('opensuse-tumbleweed')[1], desktop.case_inputs('arch')[1]
        listing = rolling['rolling']['listing']
        resolved = rolling | {'url': listing + 'openSUSE-Tumbleweed-Minimal-VM.x86_64-1.0.0-Cloud-Snapshot20261117.qcow2',
                              'digest': 'f' * 64, 'resolved': {'image': 'x'}}
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'IMAGE.json'
            for name, image in (('opensuse-tumbleweed', resolved), ('arch', pinned)):
                path.write_text(json.dumps(image))
                self.assertEqual(desktop.tested_image(name, path), image)
            for name, image in (('opensuse-tumbleweed', rolling), ('opensuse-tumbleweed', resolved | {'digest': 'F' * 64}),
                                ('opensuse-tumbleweed', resolved | {'url': 'https://example.org/x.qcow2'}),
                                ('opensuse-tumbleweed', resolved | {'session': 'other.desktop'}),
                                ('opensuse-tumbleweed', {k: v for k, v in resolved.items() if k != 'resolved'}),
                                ('arch', pinned | {'digest': 'f' * 64}), ('fedora-44', pinned)):
                path.write_text(json.dumps(image))
                with self.subTest(name=name, image=image), self.assertRaises(ValueError):
                    desktop.tested_image(name, path)

    def test_a_rolling_image_needs_a_listing_a_pattern_and_a_pinned_key(self):
        image = desktop.case_inputs('opensuse-tumbleweed')[1]
        for change in ({'url': 'https://example.com/x.qcow2'}, {'digest': 'f' * 64}, {'algorithm': 'sha512'},
                       {'rolling': image['rolling'] | {'listing': 'http://example.com/'}},
                       {'rolling': image['rolling'] | {'signing_key': '../key.asc'}},
                       {'rolling': image['rolling'] | {'signing_key_fingerprint': 'ad48'}},
                       {'rolling': image['rolling'] | {'extra': True}}):
            with self.subTest(change), self.assertRaises(ValueError):
                desktop.rolling_inputs(image | change)

    def test_only_a_good_signature_by_the_pinned_primary_key_is_accepted(self):
        key = (desktop.ROOT / 'assets/opensuse-signing-key.asc').read_text(encoding='ascii')
        self.assertEqual(desktop.dearmor_public_key(key)[0], 0x99)  # a public key packet
        with self.assertRaisesRegex(ValueError, 'checksum'):
            desktop.dearmor_public_key(key.replace('=is8v', '=AAAA'))
        fingerprint = 'AD485664E901B867051AB15F35A2F86E29B700A4'
        def gpgv(code, stdout):
            def run(arguments, **options):
                self.assertEqual(arguments[:4], ['gpgv', '--status-fd', '1', '--keyring'])
                return subprocess.CompletedProcess(arguments, code, stdout, '')
            return run
        good = ('[GNUPG:] GOODSIG 35A2F86E29B700A4 x\n[GNUPG:] VALIDSIG ' + 'F' * 40
                + ' 2026-11-18 1 0 4 0 1 10 00 ' + fingerprint + '\n')
        desktop.verify_signature(b'data', b'sig', key, fingerprint, gpgv(0, good))
        for code, stdout in ((1, good), (0, good.replace(fingerprint, 'E' * 40)), (0, '')):
            with self.subTest(code=code, stdout=stdout), self.assertRaisesRegex(ValueError, 'not signed'):
                desktop.verify_signature(b'data', b'sig', key, fingerprint, gpgv(code, stdout))

    @unittest.skipUnless(shutil.which('gpgv'), 'Requires gpgv, as the desktop runner has')
    def test_the_published_signature_of_a_real_snapshot_verifies_with_the_pinned_key(self):
        key = (desktop.ROOT / 'assets/opensuse-signing-key.asc').read_text(encoding='ascii')
        fingerprint = desktop.case_inputs('opensuse-tumbleweed')[1]['rolling']['signing_key_fingerprint']
        desktop.verify_signature(SNAPSHOT_CHECKSUM, SNAPSHOT_SIGNATURE, key, fingerprint)
        with self.assertRaisesRegex(ValueError, 'not signed'):
            desktop.verify_signature(SNAPSHOT_CHECKSUM.replace(b'fbdb', b'fbdc'), SNAPSHOT_SIGNATURE, key, fingerprint)
    def test_preflight_stops_only_on_an_image_its_publisher_says_is_gone(self):
        import urllib.error
        images = desktop.read(desktop.ROOT / 'assets/linux-desktop-images.json')['images']
        current = images['opensuse-tumbleweed']['rolling']['listing'] + 'Snapshot.qcow2'
        resolved = []
        def resolve(image):
            resolved.append(image)
            return image | {'url': current}
        addresses = {name: image.get('url', current) for name, image in images.items()}
        # A mirror reached by a redirect cannot say the image is gone: only the pinned host can.
        answers = {'arch': 404, 'opensuse-tumbleweed': 410, 'debian-13': 503, 'fedora-44': 405,
                   'ubuntu-24.04': (404, 'https://mirror.example.org/image.qcow2'),
                   'ubuntu-26.04': urllib.error.URLError('no route'), 'mint-22.3': TimeoutError('slow')}
        asked = []
        def opener(request, timeout):
            name = next(key for key, url in addresses.items() if url == request.full_url)
            asked.append((name, request.get_method(), timeout))
            answer = answers.get(name)
            if isinstance(answer, tuple):
                raise urllib.error.HTTPError(answer[1], answer[0], 'answer', {}, None)
            if isinstance(answer, int):
                raise urllib.error.HTTPError(request.full_url, answer, 'answer', {}, None)
            if answer is not None:
                raise answer
            return contextlib.nullcontext()
        gone, unsure = desktop.missing_desktop_images(opener, resolve=resolve)
        self.assertEqual(resolved, [images['opensuse-tumbleweed']])
        # A rolling image replaced between the listing and the HEAD has nothing to pin again.
        self.assertEqual(sorted(gone), ['arch'])
        self.assertEqual(sorted(unsure), ['debian-13', 'fedora-44', 'mint-22.3', 'opensuse-tumbleweed',
                                          'ubuntu-24.04', 'ubuntu-26.04'])
        self.assertIn('replaced since it was listed', unsure['opensuse-tumbleweed'])
        self.assertIn(images['arch']['url'], gone['arch'])
        self.assertIn('answered by https://mirror.example.org/image.qcow2', unsure['ubuntu-24.04'])
        self.assertEqual(sorted(name for name, _, _ in asked), sorted(images))
        self.assertEqual({(method, timeout) for _, method, timeout in asked}, {('HEAD', 10)})
        # A rolling image that cannot be resolved now is a warning, and its address is not asked.
        asked.clear()
        def unresolved(image):
            raise ValueError('The publisher lists 0 images')
        answers['opensuse-tumbleweed'] = None
        gone, unsure = desktop.missing_desktop_images(opener, resolve=unresolved)
        self.assertEqual(sorted(gone), ['arch'])
        self.assertIn('ValueError resolving the current image from https://download.opensuse.org/', unsure['opensuse-tumbleweed'])
        self.assertNotIn('opensuse-tumbleweed', [name for name, _, _ in asked])

    def test_preflight_image_check_is_bounded_as_a_whole(self):
        import threading
        import time
        release = threading.Event()
        def opener(request, timeout):
            release.wait(5)  # every publisher hangs longer than the budget
            return contextlib.nullcontext()
        started = time.monotonic()
        gone, unsure = desktop.missing_desktop_images(
            opener, timeout=10, budget=0.5, resolve=lambda image: image | {'url': 'https://example.org/x.qcow2'})
        elapsed = time.monotonic() - started
        release.set()
        self.assertEqual(gone, {})
        images = desktop.read(desktop.ROOT / 'assets/linux-desktop-images.json')['images']
        self.assertEqual(sorted(unsure), sorted(images))
        self.assertTrue(all('no answer within 0.5 s' in reason for reason in unsure.values()))
        self.assertLess(elapsed, 3)

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
                ('opensuse-tumbleweed', 'sddm', '/etc/sddm.conf.d/noh-test.conf', 'plasmawayland.desktop', 'kernel-default'),
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

    def test_a_desktop_refuses_a_probe_report_without_mcp(self):
        # The guest applies the matrix's rule to its own probe report, MCP job included.
        self.assertIs(desktop.require_complete_probe, linux_matrix.require_complete_probe)
        self.assertIn('    require_complete_probe(report)\n', inspect.getsource(desktop.guest))
        report = {'scope': 'host-runtime-smoke', 'gui': {'a': 1}, 'transcription': {'cues': 1}}
        with self.assertRaisesRegex(ValueError, 'MCP'):
            desktop.require_complete_probe(report)


# openSUSE's published checksum and signature for its 2026-11-17 Tumbleweed cloud image.
SNAPSHOT_CHECKSUM = b'fbdb555222faea0b07116c5af50e6dfa564afce2719ff3cfc458d585f9b9342b  openSUSE-Tumbleweed-Minimal-VM.x86_64-1.0.0-Cloud-Snapshot20261117.qcow2\n'
SNAPSHOT_SIGNATURE = b'''-----BEGIN PGP SIGNATURE-----
Version: GnuPG v2.4.4 (GNU/Linux)

iQIVAwUAav1bqzWi+G4ptwCkAQptKRAAkVdSR7YlvzOy86oggkDRW90cIDtBJgR5
BH/+a0vz2CfXIM8WkvwK3SS447cZuKssFE6z6TtJfFgCqfZTCC4NI30Xll7u+6ag
vOWcSu+ZlWf0PkHHuv91+fQkwST4YkzHtQlnkwip1GpNSv8XSaNQv7nTZLON6xCy
R4ktPQUPlOEeTXNM1AwVWmV7T3pR4Pt53XbJ6j6BPaCTmzHUxPlW1WPBJTEwdtI9
YRbeVk7WTow45enoab4N0oUAzPQfnvCo7CkM+3asNdQ0Ky07+IMxvphPjVzKRME8
dfTrhGtDfUxN4p/888I5BWI+Gr4SNHRkgP/YzAPAI9S1zb24ev3C3f/wEDlzHr9R
OJ1tBf99xYHyG2Jm+27hLf1W7sMqQEd1LJRORXF9zbuzc8RmyJqR+bV9WtDWPG+g
C2B+UgMLYnkO0FYMr2Ok/xRfyhcw7GNexTk0/+Y05w6I+/vxWnWFyP49bpv9On3T
qBOe3mUhkrsCxE5yEUF6d4Qc7ZMxvQ1IkfoZJYjdqLDHNTGXUn2P3DxVWB7pkBt3
4zrXaQy3UKfqi6RrQlFpjBX7zgbJPE2QTDM3V7piqX7Uds3rKO8flrU1E67xOd1M
GNZLcXaDyNRruFKyfOLbTzg8Qi05mRZDI5HekCXwGXli/oJKx9Dkc6F78rCAT+VH
5RPT0ULocNE=
=ZeOX
-----END PGP SIGNATURE-----
'''


if __name__ == '__main__':
    unittest.main()
