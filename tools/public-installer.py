"""Wrap an exact reviewed Windows portable in an ordinary per-user installer.

This never rebuilds NOH or acquires/embeds Microsoft runtime files. The generated
wizard offers a direct, hash-checked Microsoft download on the user's computer.
"""
import argparse
import copy
from pathlib import Path
import re
import shutil
import subprocess
import zipfile
import urllib.request

import delivery
import public_components

ROOT = Path(__file__).resolve().parents[1]
COMPILER = {
    "ISCC.exe": "d06ebd38f38e3cee60a3c50cc45bd449d77e0bc6a5cabc607ea9886808e4de1a",
    "ISPP.dll": "f875ddf920f17dceaaad05280dafd6d5376a1a4111cbd5fe97bfc47c286b5a41",
    "license.txt": "2e5346868c2a18434489824e11d65c3031620f792fefc415d05f19cd441abf5c",
    "is7z-x64.dll": "822206aa54b53516c336300329f87236b83bf55562e63bc9a25b1ebf064e903b",
}
PROFILES = ('minimal', 'standard', 'complete')
WRAPPERS = ('public.iss', 'public-messages.iss', 'public-profile.iss', 'public-content.iss')


def includes(profile, name):
    """Keep the established setup_profiles.rs content boundaries."""
    if profile not in PROFILES:
        raise ValueError('Unknown installation profile')
    if profile == 'complete':
        return True
    if profile == 'standard':
        return not name.startswith('bin/speech/')
    return not name.startswith('bin/') or name in ('bin/noh-cli.exe', 'bin/noh-mcp.exe')


def profile_payload(bundle, manifest, profile, source, extras=None, inventory=None):
    files = {name: spec for name, spec in (inventory or delivery.inventory(bundle)).items() if includes(profile, name)}
    for name, path in (extras or {}).items():
        files[name] = {'sha256': delivery.digest(path), 'size': path.stat().st_size}
    required = {'noh.exe', 'bin/noh-cli.exe', 'bin/noh-mcp.exe'}
    if profile != 'minimal':
        required |= {'bin/ffmpeg.exe', 'bin/preview/libmpv-2.dll'}
    if not required <= files.keys():
        raise ValueError('Profile is missing required application/media files')
    if profile == 'complete' and not any(name.startswith('bin/speech/') for name in files):
        raise ValueError('Complete profile has no speech payload')
    selected = copy.deepcopy(manifest)
    selected['distribution_profile'] = profile
    selected['sha256'] = {name: spec['sha256'] for name, spec in files.items() if name != 'manifest.json'}
    if profile == 'minimal':
        selected['tools'] = {}
    description = {
        'minimal': 'Application only: no FFmpeg, preview runtime, Whisper or speech models. Configure external media tools to use their features.',
        'standard': 'Application, FFmpeg and video preview. No Whisper or speech models; automatic transcription is not included.',
        'complete': 'Application, FFmpeg, video preview, Whisper and speech models. The optional Microsoft speech prerequisite is downloaded directly from Microsoft.'}[profile]
    readme = source / 'README.md'
    readme.write_text(f'# NOH {manifest["version"]} — {profile.title()}\n\n{description}\n\n'
                      'Open noh.exe or use the Start menu.\n'
                      'Use Add or repair components in NOH to add media tools or transcription later.\n'
                      'The installer preserves added content when repairing or updating the application.\n'
                      'Downloads and source materials: https://github.com/Awalgawe/noh/releases/latest\n'
                      'Third-party notices are in licenses/. NOH is GPL-3.0-only.\n', encoding='utf-8')
    selected['sha256']['README.md'] = delivery.digest(readme)
    delivery.write(source / 'manifest.json', selected)
    paths = {name: bundle / name for name in files}
    paths.update(extras or {})
    paths.update({'README.md': readme, 'manifest.json': source / 'manifest.json'})
    expected = dict(files)
    for name in ('README.md', 'manifest.json'):
        path = paths[name]
        expected[name] = {'size': path.stat().st_size, 'sha256': delivery.digest(path)}
    return paths, expected


def quoted(value):
    value = str(value)
    if any(c in value for c in ('"', '\r', '\n', '{', '}')):
        raise ValueError("Unsafe installer input path")
    return '"' + value + '"'


def decoder_sources(cache):
    spec = delivery.read(ROOT / 'assets/installer-extraction.lock.json')
    cache.mkdir(parents=True, exist_ok=True)
    inputs = {'is7z-source.zip': {'url': spec['source_url'], 'sha256': spec['source_sha256']},
              'innosetup-source.zip': spec['installer_source']}
    paths = []
    for name, item in inputs.items():
        path = cache / name
        if not path.exists():
            partial = path.with_suffix('.partial')
            try:
                with urllib.request.urlopen(item['url'], timeout=90) as response, partial.open('wb') as sink:
                    shutil.copyfileobj(response, sink, length=1024 * 1024)
                if delivery.digest(partial) != item['sha256']:
                    raise ValueError('Installer source digest differs: ' + name)
                partial.replace(path)
            finally:
                partial.unlink(missing_ok=True)
        if delivery.digest(path) != item['sha256']:
            raise ValueError('Installer source digest differs: ' + name)
        paths.append(path)
    return paths


def build(bundle, commit, compiler, output, profile='complete', components=None, helper=None, maintenance=False):
    bundle, compiler, output = (p.resolve() for p in (bundle, compiler, output))
    manifest = delivery.verify_bundle(bundle, "x86_64-pc-windows-gnu", commit)
    delivery.authorize_redistribution("windows-x64")
    delivery.reject_microsoft_payloads(delivery.inventory(bundle))
    for name, expected in COMPILER.items():
        if delivery.digest(compiler / name) != expected:
            raise ValueError("Inno Setup 7.1.0 compiler pin mismatch: " + name)
    version = manifest["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("An ordinary release version is required")
    if components is None:
        raise ValueError('The verified optional-content catalog is required')
    catalog = delivery.read(components / 'COMPONENTS.json')
    public_components.validate(catalog, components)
    input_files = delivery.inventory(bundle)
    for group in public_components.GROUPS:
        expected = {name: spec for name, spec in input_files.items() if public_components.group_for(name) == group}
        if catalog['groups'][group]['files'] != expected:
            raise ValueError('Component catalog differs from the portable payload')
        notices = {name: spec for name, spec in input_files.items() if name.startswith('licenses/')}
        if catalog['groups'][group]['archive_files'] != {**expected, **notices}:
            raise ValueError('Component notices differ from the portable payload')
    if not maintenance:
        if helper is None:
            raise ValueError('The matching maintenance helper is required')
        helper_record = delivery.read(helper.parent / 'INSTALLER.json')
        if (helper_record['source_commit'] != commit or not helper_record.get('maintenance')
                or delivery.digest(helper) != helper_record['installer']['sha256']):
            raise ValueError('Maintenance helper identity differs')
    extraction_sources = decoder_sources(output.parent / 'decoder-sources')
    output.mkdir(parents=True, exist_ok=False)
    source = output / "source"
    source.mkdir()
    for name in WRAPPERS:
        shutil.copy2(ROOT / "tools/installer" / name, source / name)
    shutil.copy2(compiler / "license.txt", source / "INNO-SETUP-LICENSE.txt")
    shutil.copy2(ROOT / "assets/noh-icon.ico", source / "noh.ico")
    # Enumerating exact manifest members excludes unrelated media, caches and
    # Microsoft installers; Inno's compile-time Hash also binds every payload.
    lines = ['#define NohVersion ' + quoted(version),
             '#define NohProfile ' + quoted(profile),
             '#define NohProfileTitle ' + quoted(profile.title()),
             '#define NohMaintenance ' + str(int(maintenance)),
             '#define NohApplicationHash ' + quoted(delivery.digest(bundle / 'noh.exe')),
             '#define EmbeddedMedia ' + quoted(str(not maintenance and profile != 'minimal')),
             '#define EmbeddedSpeech ' + quoted(str(not maintenance and profile == 'complete')),
             '#define ComponentBaseUrl ' + quoted('https://github.com/Awalgawe/noh/releases/download/v' + version + '/'),
             '#define OutputDirectory ' + quoted(output),
             '#define NohIcon ' + quoted(source / "noh.ico"), '[Files]']
    extras = {'licenses/INNO-SETUP-LICENSE.txt': source / 'INNO-SETUP-LICENSE.txt'}
    for name in ('7ZIP-LICENSE.txt', 'LGPL-2.1.txt'):
        extras['licenses/installer/' + name] = ROOT / 'assets/installer-licenses' / name
    if helper is not None:
        extras['noh-components.exe'] = helper
    profiles = {}
    for content in PROFILES:
        folder = source / content
        folder.mkdir()
        profiles[content] = profile_payload(bundle, manifest, content, folder, extras, input_files)
    paths, files = profiles[profile]
    delivery.require_pe_closure(delivery.audit_pe_images(
        (name, path.read_bytes()) for name, path in paths.items()
        if path.suffix.lower() in ('.exe', '.dll')))
    all_paths, all_files = profiles['complete']
    for name, spec in all_files.items():
        if name in ('manifest.json', 'README.md'):
            continue
        group = public_components.group_for(name)
        if maintenance and group is None:
            continue
        relative = Path(name)
        destination = '{app}' + (('\\' + str(relative.parent)) if relative.parent != Path('.') else '')
        external = group is not None and (maintenance or not includes(profile, name))
        src = ('"{tmp}\\noh-' + group + '\\' + str(relative) + '"') if external else quoted(all_paths[name])
        line = 'Source: ' + src + '; DestDir: "' + destination + '"; Hash: "' + spec['sha256'] + '"; Flags: '
        line += 'external ignoreversion; ExternalSize: ' + str(spec['size']) if external else 'ignoreversion'
        if group is not None:
            line += '; Check: Install' + group.title()
        lines.append(line)
    for content, (content_paths, _) in profiles.items():
        for name in ('manifest.json', 'README.md'):
            base = name.split('.')[0] + '-' + content + '.' + name.split('.')[1]
            if not maintenance:
                lines.append('Source: ' + quoted(content_paths[name]) + '; DestDir: "{app}\\components"; DestName: '
                             + quoted(base) + '; Flags: ignoreversion')
            src = ('"{app}\\components\\' + base + '"') if maintenance else quoted(content_paths[name])
            flags = 'external ignoreversion' if maintenance else 'ignoreversion'
            lines.append('Source: ' + src + '; DestDir: "{app}"; DestName: ' + quoted(name)
                         + '; Flags: ' + flags + '; Check: Is' + content.title())
    # Templates are installer support files, outside the application manifest to
    # avoid recursive hashes. Qualification still checks their exact bytes.
    support = {}
    for content, (content_paths, _) in profiles.items():
        for name in ('manifest.json', 'README.md'):
            stem, suffix = name.split('.')
            path = content_paths[name]
            support[f'components/{stem}-{content}.{suffix}'] = {
                'size': path.stat().st_size, 'sha256': delivery.digest(path)}
    for _, expected in profiles.values():
        expected.update(support)
    (source / 'public-catalog.iss').write_text('\n'.join(public_components.script(catalog)) + '\n', encoding='utf-8-sig')
    (source / "public-payload.iss").write_text('\n'.join(lines)+'\n', encoding='utf-8-sig', newline='\n')
    with (output / "compiler.log").open('wb') as log:
        subprocess.run([str(compiler / "ISCC.exe"), str(source / "public.iss")],
                       check=True, stdout=log, stderr=subprocess.STDOUT, timeout=1200)
    asset = output / ('noh-components.exe' if maintenance else f"NOH-{version}-windows-x64-{profile.title()}-Setup.exe")
    if not asset.is_file() or asset.stat().st_size >= delivery.LIMIT:
        raise ValueError("Installer is missing or exceeds the release asset limit")
    delivery.verify_bundle(bundle, "x86_64-pc-windows-gnu", commit)
    # Retain a portable description instead of leaking local build paths in the
    # public source materials. The builder regenerates public-payload.iss.
    label = 'maintenance' if maintenance else profile
    materials = output / f"NOH-{version}-windows-x64-{label}-installer-sources.zip"
    with zipfile.ZipFile(materials, 'x', zipfile.ZIP_DEFLATED) as archive:
        for name in WRAPPERS + ("INNO-SETUP-LICENSE.txt", "noh.ico"):
            archive.write(source / name, name)
        archive.write(Path(__file__), 'public-installer.py')
        archive.write(ROOT / 'tools/public_components.py', 'public_components.py')
        archive.write(components / 'COMPONENTS.json', 'COMPONENTS.json')
        for path in extraction_sources:
            archive.write(path, path.name)
        archive.write(ROOT / 'docs/INSTALLER_REBUILD.md', 'INSTALLER_REBUILD.md')
        archive.write(ROOT / 'assets/installer-extraction.lock.json', 'installer-extraction.lock.json')
        for name in ('7ZIP-LICENSE.txt', 'LGPL-2.1.txt'):
            archive.write(ROOT / 'assets/installer-licenses' / name, name)
        archive.write(ROOT / 'LICENSE', 'NOH-LICENSE.txt')
        archive.writestr('README.txt', 'NOH Windows installer wrapper sources.\n'
            'Use tools/public-installer.py from the matching NOH source tree.\n'
            'Build from the verified portable folder with pinned Inno Setup 7.1.0.\n'
            'The builder creates public-payload.iss from the exact portable manifest.\n'
            'Microsoft supplies the optional runtime directly to each user; it is not embedded.\n'
            'Inno Setup: https://github.com/jrsoftware/issrc/tree/is-7_1_0\n')
    record = {"schema_version": 1, "status": "unqualified-installer-wrapper", "source_commit": commit,
              "distribution_profile": profile, "installed_files": files,
              "maintenance": maintenance,
              "profile_files": {content: result[1] for content, result in profiles.items()},
              "portable_manifest_sha256": delivery.digest(bundle / "manifest.json"),
              "installer": {"name": asset.name, "size": asset.stat().st_size, "sha256": delivery.digest(asset)},
              "materials": {"name": materials.name, "size": materials.stat().st_size, "sha256": delivery.digest(materials)},
              "compiler": {"version": "7.1.0", "pins": COMPILER},
              "wrapper_sources": {name: delivery.digest(ROOT / "tools/installer" / name)
                                  for name in WRAPPERS},
              "microsoft_runtime": "Downloaded directly from Microsoft after interactive user selection; not redistributed."}
    delivery.write(output / "INSTALLER.json", record)
    print("Installer prepared; native wizard/install/uninstall qualification remains required.")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--compiler', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--profile', choices=PROFILES, default='complete')
    parser.add_argument('--components', type=Path, required=True)
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--maintenance', action='store_true')
    args = parser.parse_args()
    build(args.bundle, args.commit, args.compiler, args.output, args.profile, args.components, args.helper, args.maintenance)
