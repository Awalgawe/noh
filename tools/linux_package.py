#!/usr/bin/env python3
"""Package NOH and CPU speech; the distribution owns the entire media stack."""
import argparse
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile

from delivery import ROOT, inventory, read, write
from linux_elf import audit_bundle, require_host_runtime
from linux_host import CONTRACT


def stage_host_documentation(home):
    (home / 'assets').mkdir(parents=True, exist_ok=True)
    (home / 'docs').mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / 'assets/linux-README.md', home / 'assets/linux-README.md')
    shutil.copy2(ROOT / 'assets/linux-validation-matrix.json', home / 'assets/linux-validation-matrix.json')
    for name in ('LINUX_REDISTRIBUTION.md', 'LINUX_0_1_3_QUALIFICATION.md', 'LINUX_VALIDATION.md'):
        shutil.copy2(ROOT / 'docs' / name, home / 'docs' / name)


def embed(appdir, speech):
    home = appdir / 'usr/lib/noh'
    if (home / 'HOST-REQUIREMENTS.json').exists() or (home / 'bin/speech').exists():
        raise ValueError('Refusing to overwrite a prepared Linux runtime')
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
    shutil.copy2(CONTRACT, home / 'HOST-REQUIREMENTS.json')
    stage_host_documentation(home)
    for path in sorted(home.rglob('*')):
        if not path.is_file():
            continue
        with path.open('rb') as stream:
            is_elf = stream.read(4) == b'\x7fELF'
        if is_elf:
            # No private search directory: all dynamic imports use the host.
            subprocess.run(['patchelf', '--remove-rpath', str(path)],
                           check=True, timeout=30)
            path.chmod(0o755)
    report = audit_bundle(appdir, host_runtime=True)
    require_host_runtime(report)
    write(home / 'ELF-IMPORTS.json', report)
    for program, arguments in ((home / 'bin/speech/whisper-cli', ['--help']),):
        subprocess.run([str(program), *arguments], check=True, timeout=30,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    print(f'Prepared Linux AppDir with {len(report["images"])} audited ELF images')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--appdir', required=True, type=Path)
    parser.add_argument('--speech', required=True, type=Path)
    args = parser.parse_args()
    embed(args.appdir.resolve(), args.speech.resolve())


if __name__ == '__main__':
    main()
