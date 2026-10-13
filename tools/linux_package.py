#!/usr/bin/env python3
"""Embed and relocate the locked Linux media/speech runtime in a prepared AppDir."""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from delivery import ROOT, digest, inventory, read, relative, write
from linux_elf import audit_bundle


def embed(appdir, native, speech):
    receipt = read(native / 'LINUX-INPUTS.json')
    lock = read(ROOT / 'assets/linux-native.lock.json')
    if receipt.get('lock_sha256') != digest(ROOT / 'assets/linux-native.lock.json'):
        raise ValueError('Linux runtime was prepared from a different input lock')
    files = inventory(native / 'runtime')
    expected = {name: spec['sha256'] for name, spec in lock['files'].items()}
    if (files != receipt['runtime_files']
            or {name: spec['sha256'] for name, spec in files.items()} != expected):
        raise ValueError('Linux runtime inventory differs from the locked inputs')
    home = appdir / 'usr/lib/noh'
    for name in files:
        destination = home / str(relative(name))
        if destination.exists():
            raise ValueError(f'Runtime would overwrite a prepared file: {name}')
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(native / 'runtime' / name, destination)
    speech_build = read(speech / 'WHISPER-BUILD.json')
    spec = importlib.util.spec_from_file_location('linux_speech', ROOT / 'tools/prepare-linux-speech.py')
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    if (speech_build.get('version') != builder.VERSION
            or speech_build.get('source') != builder.SOURCE
            or speech_build.get('architecture') != 'x86_64'
            or speech_build.get('cmake_options') != builder.OPTIONS):
        raise ValueError('Speech receipt differs from the pinned Linux CPU build recipe')
    with tempfile.TemporaryDirectory() as tmp:
        commands = Path(tmp) / 'commands.json'
        write(commands, speech_build['compiler_commands'])
        builder.check_cpu_commands(commands)
    speech_files = inventory(speech)
    if {name: spec['sha256'] for name, spec in speech_files.items()
            if name != 'WHISPER-BUILD.json'} != speech_build['sha256']:
        raise ValueError('Speech inputs differ from their build receipt')
    models = read(ROOT / 'assets/speech-bundle.json')
    for name in ('ggml-small.bin', 'ggml-silero-v6.2.0.bin',
                 'WHISPER-MODEL-LICENSE.txt', 'SILERO-LICENSE.txt'):
        if speech_files[name]['sha256'] != models['sha256'][name]:
            raise ValueError(f'Linux speech input differs from pinned model/license: {name}')
    shutil.copytree(speech, home / 'bin/speech')
    shutil.copy2(native / 'LINUX-INPUTS.json', home / 'LINUX-INPUTS.json')
    library = home / 'lib'
    for path in sorted(home.rglob('*')):
        if not path.is_file():
            continue
        with path.open('rb') as stream:
            is_elf = stream.read(4) == b'\x7fELF'
        if is_elf:
            # Every object resolves its own imports. No global LD_LIBRARY_PATH
            # leaks this private closure into programs launched outside NOH.
            suffix = os.path.relpath(library, path.parent)
            rpath = '$ORIGIN' if suffix == '.' else '$ORIGIN/' + suffix
            subprocess.run(['patchelf', '--set-rpath', rpath, str(path)],
                           check=True, timeout=30)
            path.chmod(0o755)
    report = audit_bundle(appdir)
    write(home / 'ELF-IMPORTS.json', report)
    for program, arguments in ((home / 'bin/ffmpeg', ['-version']),
                               (home / 'bin/speech/whisper-cli', ['--help'])):
        subprocess.run([str(program), *arguments], check=True, timeout=30,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    print(f'Prepared Linux AppDir with {len(report["images"])} audited ELF images')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--appdir', required=True, type=Path)
    parser.add_argument('--native', required=True, type=Path)
    parser.add_argument('--speech', required=True, type=Path)
    args = parser.parse_args()
    embed(args.appdir.resolve(), args.native.resolve(), args.speech.resolve())


if __name__ == '__main__':
    main()
