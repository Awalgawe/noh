#!/usr/bin/env python3
"""Freeze Ubuntu native inputs and acquire their exact binaries and sources."""
import argparse
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.parse

from delivery import digest, fetch, inventory, read, relative, write
from linux_elf import SYSTEM_LIBRARIES, elf_info

DL_LIBRARIES = (
    "libasound.so.2", "libX11.so.6", "libXcursor.so.1", "libXrandr.so.2",
    "libXi.so.6", "libXinerama.so.1", "libXrender.so.1", "libXfixes.so.3",
    "libxcb.so.1", "libxcb-render.so.0", "libxcb-shape.so.0", "libxcb-xfixes.so.0",
    "libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libwayland-client.so.0",
    "libwayland-cursor.so.0", "libwayland-egl.so.1", "libdecor-0.so.0",
)


def native_host():
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("Native input preparation requires Linux x86_64")
    release = dict(line.split("=", 1) for line in Path('/etc/os-release').read_text().splitlines()
                   if "=" in line)
    if release.get("ID", "").strip('"') != "ubuntu" or release.get("VERSION_ID", "").strip('"') != "24.04":
        raise ValueError("Native input preparation requires the Ubuntu 24.04 baseline")


def ubuntu_url(url):
    parts = urllib.parse.urlsplit(url)
    if (parts.scheme not in ("http", "https") or parts.hostname not in
            ("archive.ubuntu.com", "security.ubuntu.com") or parts.username or parts.password
            or parts.port or parts.query or parts.fragment or not parts.path.startswith('/ubuntu/pool/')):
        raise ValueError(f"Expected an official Ubuntu pool URL: {url}")
    return urllib.parse.urlunsplit(("https", parts.netloc, parts.path, "", ""))


def owners(paths):
    """Account for Ubuntu's merged /usr while keeping exact dpkg ownership."""
    paths = {Path(p).resolve() for p in paths}
    queries = set(map(str, paths))
    for path in list(queries):
        if path.startswith(("/usr/lib/", "/usr/bin/", "/usr/sbin/")):
            queries.add(path[4:])
    result = subprocess.run(["dpkg-query", "--search", "--", *sorted(queries)],
                            capture_output=True, text=True, timeout=60)
    if result.returncode not in (0, 1):
        raise ValueError(f"dpkg ownership query failed: {result.stderr}")
    found = {}
    for line in result.stdout.splitlines():
        if ": " not in line:
            continue
        package, member = line.split(": ", 1)
        if not member.startswith('/') or ',' in package:
            continue
        actual = Path(member).resolve()
        if actual not in paths:
            continue
        if actual in found and found[actual] != (package, member.lstrip('/')):
            raise ValueError(f"Ambiguous native-file ownership: {actual}")
        found[actual] = (package, member.lstrip('/'))
    missing = paths - set(found)
    if missing:
        raise ValueError(f"No package owns native input(s): {sorted(map(str, missing))}")
    return found


def freeze(output):
    native_host()
    if output.exists():
        raise ValueError("Refusing to overwrite the native input lock")
    # python-apt is needed only to create a lock, not to acquire locked inputs.
    import apt
    import apt_pkg
    cache = apt.Cache()
    ldconfig = subprocess.check_output(["/sbin/ldconfig", "-p"], text=True)
    libraries = {}
    for name, path in re.findall(r'^\s*(\S+) \([^\n]*x86-64[^\n]*\) => (/\S+)$', ldconfig, re.M):
        libraries.setdefault(name, Path(path))
    roots = [(Path('/usr/bin/ffmpeg'), 'bin/ffmpeg'),
             (Path('/usr/lib/x86_64-linux-gnu/libmpv.so.2'), 'bin/libmpv.so.2')]
    for name in DL_LIBRARIES:
        if name not in libraries:
            raise ValueError(f"Missing native GUI library: {name}")
        roots.append((libraries[name], 'lib/' + name))
    for name in ('libasound_module_pcm_pulse.so', 'libasound_module_ctl_pulse.so'):
        roots.append((Path('/usr/lib/x86_64-linux-gnu/alsa-lib') / name, 'lib/alsa-lib/' + name))
    selected = {}

    def add(path, destination):
        path = path.resolve(strict=True)
        relative(destination)
        if destination in selected and selected[destination] != path:
            raise ValueError(f"Conflicting native providers: {destination}")
        elf_info(path.read_bytes())
        selected[destination] = path

    for path, destination in roots:
        add(path, destination)
        result = subprocess.run(["ldd", str(path)], capture_output=True, text=True, timeout=30)
        if result.returncode or 'not found' in result.stdout:
            raise ValueError(f"Unresolved installed native runtime: {path}: {result.stdout}{result.stderr}")
        for name, provider in re.findall(r'^\s*(\S+) => (/\S+)', result.stdout, re.M):
            if name not in SYSTEM_LIBRARIES:
                add(Path(provider), 'lib/' + name)
    ownership = owners(selected.values())
    unavailable = []
    for package in sorted({item[0] for item in ownership.values()}):
        version = cache[package].installed
        if not version or not version.downloadable:
            unavailable.append(package)
    if unavailable:
        raise ValueError("Update these native packages to repository versions before freezing: " + ' '.join(unavailable))
    packages = {}
    sources = {}
    source_records = apt_pkg.SourceRecords()

    def package_record(package):
        version = cache[package].installed
        if not version or not version.downloadable or version.architecture not in ('amd64', 'all'):
            raise ValueError(f"Installed package has no matching x86_64 repository input: {package}")
        key = version.package.name
        if key in packages:
            return key
        if not any(origin.trusted for origin in version.origins):
            raise ValueError(f"Package index is not trusted: {package}")
        packages[key] = {
            'version': version.version, 'architecture': version.architecture,
            'source': version.source_name, 'source_version': version.source_version,
            'binary': {'url': ubuntu_url(version.uri), 'sha256': version.record['SHA256']},
        }
        name = version.source_name
        if name in sources:
            if sources[name]['version'] != version.source_version:
                raise ValueError(f"Conflicting source versions: {name}")
            return key
        source_records.restart()
        while source_records.lookup(name):
            if source_records.version != version.source_version:
                continue
            if not source_records.index.is_trusted:
                raise ValueError(f"Source index is not trusted: {name}")
            section = apt_pkg.TagSection(source_records.record)
            files = {}
            for line in section['Checksums-Sha256'].strip().splitlines():
                checksum, size, filename = line.split()
                if relative(filename).name != filename:
                    raise ValueError("Source archive must have a bare filename")
                location = section['Directory'] + '/' + filename
                files[filename] = {'url': ubuntu_url(source_records.index.archive_uri(location)),
                                   'sha256': checksum, 'size': int(size)}
            if len([n for n in files if n.endswith('.dsc')]) != 1:
                raise ValueError(f"Missing unique Debian source descriptor: {name}")
            sources[name] = {'version': version.source_version, 'files': files,
                             'repository_record': source_records.record}
            break
        else:
            raise ValueError(f"Corresponding source unavailable: {name}={version.source_version}")
        return key

    files = {}
    for destination, path in sorted(selected.items()):
        package, member = ownership[path]
        key = package_record(package)
        files[destination] = {'package': key, 'member': member, 'sha256': digest(path), 'mode': 0o755}
    # Preserve the exact distributed copyright files, including references to
    # common license texts. Their owning packages also get matching sources.
    notice_paths = {}
    for key in list(packages):
        name = key.split(':')[0]
        path = Path('/usr/share/doc') / name / 'copyright'
        if not path.is_file():
            raise ValueError(f"Missing package copyright notice: {name}")
        notice_paths['licenses/native/' + name + '/copyright'] = path.resolve()
    for path in Path('/usr/share/common-licenses').iterdir():
        if path.is_file():
            notice_paths['licenses/common/' + path.name] = path.resolve()
    notice_owners = owners(notice_paths.values())
    for destination, path in sorted(notice_paths.items()):
        package, member = notice_owners[path]
        key = package_record(package)
        files[destination] = {'package': key, 'member': member, 'sha256': digest(path), 'mode': 0o644}
    write(output, {'schema_version': 1, 'status': 'unqualified-candidate-inputs',
                   'baseline': {'distribution': 'Ubuntu', 'version': '24.04', 'architecture': 'x86_64', 'glibc': '2.39'},
                   'host_libraries': sorted(SYSTEM_LIBRARIES), 'dynamic_gui_libraries': list(DL_LIBRARIES),
                   'packages': packages, 'sources': sources, 'files': files})
    print(f"Locked {len(files)} files from {len(packages)} binaries and {len(sources)} corresponding sources: {output}")


def copy_members(archive, members, output):
    """Read named regular files only; never extract upstream links or devices."""
    with tempfile.TemporaryFile() as raw:
        subprocess.run(['dpkg-deb', '--fsys-tarfile', str(archive)], stdout=raw, check=True, timeout=120)
        raw.seek(0)
        with tarfile.open(fileobj=raw) as tar:
            table = {}
            for item in tar.getmembers():
                name = item.name.removeprefix('./').rstrip('/')
                if not name or name == '.':
                    continue
                relative(name)
                if name in table:
                    raise ValueError(f"Duplicate Debian payload member: {name}")
                table[name] = item
            for destination, spec in members.items():
                relative(destination)
                name = str(relative(spec['member']))
                seen = set()
                while True:
                    if name in seen or name not in table:
                        raise ValueError(f"Missing/cyclic Debian payload member: {name}")
                    seen.add(name)
                    item = table[name]
                    if item.isreg():
                        break
                    if not item.islnk():
                        raise ValueError(f"Expected a regular native input, not a symlink/device: {name}")
                    name = str(relative(item.linkname.removeprefix('./')))
                target = output / destination
                if target.exists() or spec['mode'] not in (0o644, 0o755):
                    raise ValueError(f"Invalid or duplicate native destination: {destination}")
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(item) as source, target.open('xb') as dest:
                    shutil.copyfileobj(source, dest)
                if digest(target) != spec['sha256']:
                    raise ValueError(f"Native payload identity mismatch: {destination}")
                target.chmod(spec['mode'])


def copy_build_records(records, cache, materials):
    """Retain producer evidence for Rust embedded in the native libraries."""
    for filename, spec in records['files'].items():
        if relative(filename).name != filename:
            raise ValueError("Expected a bare native build-record filename")
        record = fetch(spec, cache, 'ubuntu-build-records/' + filename)
        if record.stat().st_size != spec['size']:
            raise ValueError(f"Native build-record size differs: {filename}")
        target = materials / 'ubuntu-build-records' / filename
        target.parent.mkdir(parents=True, exist_ok=True)
        with record.open('rb') as source, target.open('xb') as destination:
            shutil.copyfileobj(source, destination)


def acquire(lock_path, cache, output):
    native_host()
    lock = read(lock_path)
    if (lock.get('schema_version') != 1 or lock.get('baseline') !=
            {'distribution': 'Ubuntu', 'version': '24.04', 'architecture': 'x86_64', 'glibc': '2.39'}
            or lock.get('host_libraries') != sorted(SYSTEM_LIBRARIES)):
        raise ValueError("Unsupported native input policy")
    if output.exists():
        raise ValueError("Refusing to overwrite prepared native inputs")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.linux-inputs-', dir=output.parent) as tmp:
        staged = Path(tmp) / 'ready'
        runtime = staged / 'runtime'
        materials = staged / 'materials'
        runtime.mkdir(parents=True)
        materials.mkdir()
        for package, spec in lock['packages'].items():
            archive_name = PurePosixPath(urllib.parse.urlsplit(spec['binary']['url']).path).name
            binary = fetch(spec['binary'], cache, 'ubuntu/' + archive_name)
            members = {name: f for name, f in lock['files'].items() if f['package'] == package}
            copy_members(binary, members, runtime)
        expected = {name: spec['sha256'] for name, spec in lock['files'].items()}
        if {name: f['sha256'] for name, f in inventory(runtime).items()} != expected:
            raise ValueError("Prepared native inputs differ from the locked inventory")
        for name, source in lock['sources'].items():
            relative(name)
            destination = materials / 'ubuntu-sources' / name
            destination.mkdir(parents=True)
            (destination / 'SOURCE-RECORD.txt').write_text(source['repository_record'], encoding='utf-8')
            for filename, spec in source['files'].items():
                if relative(filename).name != filename:
                    raise ValueError("Expected a bare source filename")
                archive = fetch(spec, cache, 'ubuntu-source/' + filename)
                if archive.stat().st_size != spec['size']:
                    raise ValueError(f"Source size differs: {filename}")
                shutil.copy2(archive, destination / filename)
        copy_build_records(lock['native_rust_builds'], cache, materials)
        shutil.copy2(lock_path, materials / 'linux-native.lock.json')
        write(staged / 'LINUX-INPUTS.json', {'schema_version': 1, 'lock_sha256': digest(lock_path),
                                           'runtime_files': inventory(runtime),
                                           'materials_files': inventory(materials)})
        os.rename(staged, output)
    print(f"Prepared locked Linux runtime and corresponding sources: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    tasks = parser.add_subparsers(dest='command', required=True)
    freezing = tasks.add_parser('freeze')
    freezing.add_argument('--output', type=Path, required=True)
    acquisition = tasks.add_parser('acquire')
    acquisition.add_argument('--lock', type=Path, required=True)
    acquisition.add_argument('--cache', type=Path, required=True)
    acquisition.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'freeze':
        freeze(args.output.resolve())
    else:
        acquire(args.lock.resolve(), args.cache.resolve(), args.output.resolve())


if __name__ == '__main__':
    main()
