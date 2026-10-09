"""Wrap an exact reviewed Windows portable in an ordinary per-user installer.

This never rebuilds NOH or acquires/embeds Microsoft runtime files. The generated
wizard offers a direct, hash-checked Microsoft download on the user's computer.
"""
import argparse
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

import delivery

ROOT = Path(__file__).resolve().parents[1]
COMPILER = {
    "ISCC.exe": "d06ebd38f38e3cee60a3c50cc45bd449d77e0bc6a5cabc607ea9886808e4de1a",
    "ISPP.dll": "f875ddf920f17dceaaad05280dafd6d5376a1a4111cbd5fe97bfc47c286b5a41",
    "license.txt": "2e5346868c2a18434489824e11d65c3031620f792fefc415d05f19cd441abf5c",
}


def quoted(value):
    value = str(value)
    if any(c in value for c in ('"', '\r', '\n', '{', '}')):
        raise ValueError("Unsafe installer input path")
    return '"' + value + '"'


def build(bundle, commit, compiler, output):
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
    output.mkdir(parents=True, exist_ok=False)
    source = output / "source"
    source.mkdir()
    for name in ("public.iss", "public-messages.iss"):
        shutil.copy2(ROOT / "tools/installer" / name, source / name)
    shutil.copy2(compiler / "license.txt", source / "INNO-SETUP-LICENSE.txt")
    shutil.copy2(ROOT / "assets/noh-icon.ico", source / "noh.ico")
    # Enumerating exact manifest members excludes unrelated media, caches and
    # Microsoft installers; Inno's compile-time Hash also binds every payload.
    lines = ['#define NohVersion ' + quoted(version),
             '#define OutputDirectory ' + quoted(output),
             '#define NohIcon ' + quoted(source / "noh.ico"), '[Files]']
    files = delivery.inventory(bundle)
    for name, spec in files.items():
        relative = Path(name)
        destination = '{app}' + (('\\' + str(relative.parent)) if relative.parent != Path('.') else '')
        lines.append('Source: ' + quoted(bundle / relative) + '; DestDir: "' + destination
                     + '"; Hash: "' + spec['sha256'] + '"; Flags: ignoreversion')
    lines.append('Source: ' + quoted(source / "INNO-SETUP-LICENSE.txt")
                 + '; DestDir: "{app}\\licenses"; Flags: ignoreversion')
    (source / "public-payload.iss").write_text('\n'.join(lines)+'\n', encoding='utf-8-sig', newline='\n')
    with (output / "compiler.log").open('wb') as log:
        subprocess.run([str(compiler / "ISCC.exe"), str(source / "public.iss")],
                       check=True, stdout=log, stderr=subprocess.STDOUT, timeout=1200)
    asset = output / f"NOH-{version}-windows-x64-Setup.exe"
    if not asset.is_file() or asset.stat().st_size >= delivery.LIMIT:
        raise ValueError("Installer is missing or exceeds the release asset limit")
    delivery.verify_bundle(bundle, "x86_64-pc-windows-gnu", commit)
    # Retain a portable description instead of leaking local build paths in the
    # public source materials. The builder regenerates public-payload.iss.
    materials = output / f"NOH-{version}-windows-x64-installer-sources.zip"
    with zipfile.ZipFile(materials, 'x', zipfile.ZIP_DEFLATED) as archive:
        for name in ("public.iss", "public-messages.iss", "INNO-SETUP-LICENSE.txt", "noh.ico"):
            archive.write(source / name, name)
        archive.write(Path(__file__), 'public-installer.py')
        archive.write(ROOT / 'LICENSE', 'NOH-LICENSE.txt')
        archive.writestr('README.txt', 'NOH Windows installer wrapper sources.\n'
            'Use tools/public-installer.py from the matching NOH source tree.\n'
            'Build from the verified portable folder with pinned Inno Setup 7.1.0.\n'
            'The builder creates public-payload.iss from the exact portable manifest.\n'
            'Microsoft supplies the optional runtime directly to each user; it is not embedded.\n'
            'Inno Setup: https://github.com/jrsoftware/issrc/tree/is-7_1_0\n')
    record = {"schema_version": 1, "status": "unqualified-installer-wrapper", "source_commit": commit,
              "portable_manifest_sha256": delivery.digest(bundle / "manifest.json"),
              "installer": {"name": asset.name, "size": asset.stat().st_size, "sha256": delivery.digest(asset)},
              "materials": {"name": materials.name, "size": materials.stat().st_size, "sha256": delivery.digest(materials)},
              "compiler": {"version": "7.1.0", "pins": COMPILER},
              "wrapper_sources": {name: delivery.digest(ROOT / "tools/installer" / name)
                                  for name in ("public.iss", "public-messages.iss")},
              "microsoft_runtime": "Downloaded directly from Microsoft after interactive user selection; not redistributed."}
    delivery.write(output / "INSTALLER.json", record)
    print("Installer prepared; native wizard/install/uninstall qualification remains required.")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--compiler', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    build(args.bundle, args.commit, args.compiler, args.output)
