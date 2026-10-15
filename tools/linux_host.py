"""Declared Linux host media requirements; no native libraries are bundled."""
import argparse
import ctypes
import json
from pathlib import Path
import re
import shutil
import subprocess

from delivery import ROOT, read, write

CONTRACT = ROOT / 'assets/linux-host.json'


def check_host(ffmpeg='ffmpeg'):
    contract = read(CONTRACT)
    executable = shutil.which(str(ffmpeg))
    if not executable:
        raise ValueError('Host FFmpeg is missing; install the Linux host requirements')
    version = subprocess.run([executable, '-version'], check=True, capture_output=True,
                             text=True, timeout=30).stdout.splitlines()[0]
    match = re.match(r'ffmpeg version (?:n)?(\d+)', version)
    if not match or int(match[1]) < contract['ffmpeg']['minimum_major']:
        raise ValueError(f'Unsupported host FFmpeg: {version}')
    encoders = subprocess.run([executable, '-hide_banner', '-encoders'], check=True,
                              capture_output=True, text=True, timeout=30).stdout
    available = set(re.findall(r'^ [A-Z.]{6}\s+(\S+)', encoders, re.M))
    missing = set(contract['ffmpeg']['encoders']) - available
    if missing:
        raise ValueError(f'Host FFmpeg lacks required encoders: {sorted(missing)}')
    mpv = ctypes.CDLL(contract['libmpv']['soname'])
    mpv.mpv_client_api_version.restype = ctypes.c_ulong
    api = mpv.mpv_client_api_version()
    if api >> 16 != contract['libmpv']['client_api_major']:
        raise ValueError(f'Unsupported host libmpv client API: {api}')
    return {'ffmpeg': str(Path(executable).resolve()), 'ffmpeg_version': version,
            'libmpv': contract['libmpv']['soname'], 'libmpv_client_api': api,
            'required_encoders': sorted(contract['ffmpeg']['encoders'])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', default='ffmpeg')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = check_host(args.ffmpeg)
    if args.output:
        if args.output.exists():
            raise ValueError('Refusing to overwrite host runtime evidence')
        write(args.output, result)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
