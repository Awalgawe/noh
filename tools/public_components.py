"""Seal optional Windows content from verified portable files, without rebuilding it."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile

import delivery

APP_FILES = {'bin/noh-cli.exe', 'bin/noh-mcp.exe'}
GROUPS = ('media', 'speech')


def group_for(name):
    if name.startswith('bin/speech/'):
        return 'speech'
    if name.startswith('bin/') and name not in APP_FILES:
        return 'media'
    return None


def prepare(bundle, commit, output):
    manifest = delivery.verify_bundle(bundle, 'x86_64-pc-windows-gnu', commit)
    delivery.authorize_redistribution('windows-x64')
    files = delivery.inventory(bundle)
    delivery.reject_microsoft_payloads(files)
    output.mkdir(parents=True, exist_ok=True)
    groups = {}
    for group in GROUPS:
        payload = {name: item for name, item in files.items() if group_for(name) == group}
        if not payload:
            raise ValueError('Missing component payload: ' + group)
        # Each independently downloadable archive carries the original notices.
        archived = {**payload, **{n: s for n, s in files.items() if n.startswith('licenses/')}}
        identity = hashlib.sha256(json.dumps(archived, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        name = f'NOH-windows-x64-{group}-{identity[:16]}.zip'
        path = output / name
        if not path.exists():
            temporary = path.with_suffix('.partial')
            try:
                with zipfile.ZipFile(temporary, 'w', zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
                    for member in sorted(archived):
                        info = zipfile.ZipInfo(member, date_time=(2026, 1, 1, 0, 0, 0))
                        info.compress_type = zipfile.ZIP_DEFLATED
                        info.external_attr = 0o644 << 16
                        with archive.open(info, 'w', force_zip64=True) as sink, (bundle / member).open('rb') as source:
                            while chunk := source.read(1024 * 1024):
                                sink.write(chunk)
                temporary.rename(path)
            finally:
                temporary.unlink(missing_ok=True)
        delivery.verify_zip(path, archived)
        if path.stat().st_size >= delivery.LIMIT:
            raise ValueError('Component exceeds the asset size limit')
        groups[group] = {'name': name, 'sha256': delivery.digest(path), 'size': path.stat().st_size,
                         'inventory_sha256': identity, 'files': payload, 'archive_files': archived,
                         'installed_bytes': sum(item['size'] for item in payload.values())}
    catalog = {'schema_version': 1, 'target': 'windows-x64', 'source_commit': commit,
               'portable_manifest_sha256': delivery.digest(bundle / 'manifest.json'),
               'version': manifest['version'], 'groups': groups}
    delivery.write(output / 'COMPONENTS.json', catalog)
    return catalog


def validate(catalog, folder):
    if catalog.get('schema_version') != 1 or catalog.get('target') != 'windows-x64' or set(catalog['groups']) != set(GROUPS):
        raise ValueError('Unsupported component catalog')
    for group, item in catalog['groups'].items():
        if not re.fullmatch('NOH-windows-x64-' + group + r'-[0-9a-f]{16}\.zip', item['name']):
            raise ValueError('Unsafe component archive name')
        path = folder / item['name']
        if path.stat().st_size != item['size'] or delivery.digest(path) != item['sha256']:
            raise ValueError('Component archive changed')
        if not item['files'] or any(group_for(name) != group for name in item['files']):
            raise ValueError('Mixed component content')
        if {name for name in item['archive_files'] if not name.startswith('licenses/')} != set(item['files']):
            raise ValueError('Unexpected component archive content')
        identity = hashlib.sha256(json.dumps(item['archive_files'], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        if (identity != item['inventory_sha256'] or identity[:16] not in item['name']
                or item['installed_bytes'] != sum(spec['size'] for spec in item['files'].values())):
            raise ValueError('Component inventory identity differs')
        delivery.reject_microsoft_payloads(item['archive_files'])
        delivery.verify_zip(path, item['archive_files'])
        if any(item['archive_files'].get(name) != spec for name, spec in item['files'].items()):
            raise ValueError('Component payload is absent from its archive')


def script(catalog):
    """Emit an inert, exact inventory for Inno's shared acquisition code."""
    lines = []
    for function, key, kind in (('ComponentName', 'name', 'String'), ('ComponentHash', 'sha256', 'String'),
                                ('ComponentBytes', 'size', 'Int64'), ('ComponentInstalledBytes', 'installed_bytes', 'Int64')):
        lines += [f'function {function}(Group: String): {kind};', 'begin']
        for index, group in enumerate(GROUPS):
            item = catalog['groups'][group]
            value = sum(spec['size'] for spec in item['archive_files'].values()) if key == 'installed_bytes' else item[key]
            value = repr(value) if kind == 'String' else str(value)
            lines.append(('  if' if index == 0 else '  else if') + f" Group = '{group}' then Result := {value}")
        lines += ["  else RaiseException('Unknown content group');", 'end;']
    lines += ['procedure ComponentFiles(Group: String; Files: TStringList);', 'begin']
    for index, group in enumerate(GROUPS):
        lines.append(('  if' if index == 0 else '  else if') + f" Group = '{group}' then begin")
        for name, item in catalog['groups'][group]['files'].items():
            if any(c in name for c in "'\r\n={}"):
                raise ValueError('Unsafe component path')
            relative = name.replace('/', '\\')
            lines.append(f"    Files.Add('{relative}={item['sha256']}');")
        lines.append('  end')
    lines += ["  else RaiseException('Unknown content group');", 'end;']
    return lines


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    catalog = prepare(args.bundle.resolve(), args.commit, args.output.resolve())
    print(json.dumps({name: {k: v[k] for k in ('name', 'sha256', 'size')} for name, v in catalog['groups'].items()}, indent=2))
