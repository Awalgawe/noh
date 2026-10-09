"""Build a small selector bound to the exact three offline profile installers."""
import argparse
import importlib.util
from pathlib import Path
import re
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
import delivery
spec = importlib.util.spec_from_file_location('offline', ROOT / 'tools/public-installer.py')
offline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(offline)


def build(profiles, compiler, output):
    records = [delivery.read(profiles / profile / 'INSTALLER.json') for profile in offline.PROFILES]
    versions = set()
    for profile, record in zip(offline.PROFILES, records):
        asset = record['installer']
        match = re.fullmatch(r'NOH-(\d+\.\d+\.\d+)-windows-x64-' + profile.title() + r'-Setup\.exe', asset['name'])
        if not match or record['distribution_profile'] != profile:
            raise ValueError('Wrong profile installer identity')
        versions.add(match[1])
        path = profiles / profile / asset['name']
        if path.stat().st_size != asset['size'] or delivery.digest(path) != asset['sha256']:
            raise ValueError('Offline installer bytes changed')
    if len(versions) != 1 or len({r['source_commit'] for r in records}) != 1 or len({r['portable_manifest_sha256'] for r in records}) != 1:
        raise ValueError('Mixed application versions or portable inputs')
    for name, expected in offline.COMPILER.items():
        if delivery.digest(compiler / name) != expected:
            raise ValueError('Compiler pin mismatch: ' + name)
    version = versions.pop()
    output.mkdir(parents=True, exist_ok=False)
    source = output / 'source'
    source.mkdir()
    for name in ('public-web.iss', 'public-messages.iss', 'public-profile.iss'):
        shutil.copy2(ROOT / 'tools/installer' / name, source / name)
    shutil.copy2(ROOT / 'assets/noh-icon.ico', source / 'noh.ico')
    lines = ['#define NohVersion ' + offline.quoted(version),
             '#define OutputDirectory ' + offline.quoted(output.resolve()),
             '#define NohIcon ' + offline.quoted(source.resolve() / 'noh.ico')]
    (source / 'web-payload.iss').write_text('\n'.join(lines) + '\n', encoding='utf-8-sig')
    lines = []
    for function, key in (('ProfileName', 'name'), ('ProfileHash', 'sha256'), ('ProfileSize', 'size')):
        lines += [f'function {function}(Index: Integer): String;', 'begin', "  Result := '';", '  case Index of']
        for index, record in enumerate(records):
            value = record['installer'][key]
            if key == 'size':
                value = f'{value / 1024 / 1024:.1f} MiB'
            # Names, hashes and sizes come from validated identities above.
            lines.append(f"    {index}: Result := '{value}';")
        lines += ['  end;', 'end;']
    (source / 'web-catalog.iss').write_text('\n'.join(lines) + '\n', encoding='utf-8-sig')
    with (output / 'compiler.log').open('wb') as log:
        subprocess.run([str(compiler / 'ISCC.exe'), str(source / 'public-web.iss')],
                       check=True, stdout=log, stderr=subprocess.STDOUT, timeout=180)
    asset = output / f'NOH-{version}-windows-x64-Web-Setup.exe'
    if not asset.is_file() or asset.stat().st_size >= delivery.LIMIT:
        raise ValueError('Web installer missing or oversized')
    materials = output / f'NOH-{version}-windows-x64-web-installer-sources.zip'
    with zipfile.ZipFile(materials, 'x', zipfile.ZIP_DEFLATED) as archive:
        for path in source.iterdir():
            if path.name not in ('web-payload.iss', 'web-catalog.iss'):
                archive.write(path, path.name)
        archive.write(Path(__file__), 'public-web.py')
        archive.write(ROOT / 'tools/public-installer.py', 'public-installer.py')
        archive.write(compiler / 'license.txt', 'INNO-SETUP-LICENSE.txt')
        archive.write(ROOT / 'LICENSE', 'NOH-LICENSE.txt')
        archive.writestr('README.txt', 'Build with tools/installer/public-web.py from the matching NOH tree, pinned Inno Setup 7.1.0 and the three exact profile output folders.\n')
    record = {'schema_version': 1, 'status': 'unqualified-web-installer',
              'source_commit': records[0]['source_commit'],
              'portable_manifest_sha256': records[0]['portable_manifest_sha256'],
              'profiles': {p: r['installer'] for p, r in zip(offline.PROFILES, records)},
              'installer': {'name': asset.name, 'size': asset.stat().st_size, 'sha256': delivery.digest(asset)},
              'materials': {'name': materials.name, 'size': materials.stat().st_size, 'sha256': delivery.digest(materials)},
              'compiler': {'version': '7.1.0', 'pins': offline.COMPILER}}
    delivery.write(output / 'INSTALLER.json', record)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profiles', required=True, type=Path)
    parser.add_argument('--compiler', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    build(args.profiles, args.compiler, args.output)
