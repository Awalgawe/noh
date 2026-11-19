"""Exercise the actual Inno /WAITPID and /RELAUNCH code on a synthetic setup (no app install)."""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent


def sleeper(seconds):
    return subprocess.Popen([sys.executable, '-c', f'import time; time.sleep({seconds})'])


def verify(compiler, output):
    output.mkdir(parents=True, exist_ok=False)
    source = output / 'update.iss'
    # The [Run] entry mirrors public.iss's relaunch: same Check, run by a silent
    # setup. It writes a variable from Setup's environment, so the file proves
    # both the relaunch and what the relaunched program inherits.
    source.write_text(f'''[Setup]
AppName=NOH update test
AppVersion=1
DefaultDirName={{tmp}}\\unused
CreateAppDir=no
Uninstallable=no
PrivilegesRequired=lowest
SetupArchitecture=x64
OutputDir=.
OutputBaseFilename=update
[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
[Run]
Filename: "{{cmd}}"; Parameters: "/c echo %NOH_UPDATE_NATIVE_MARK%> ""{{param:MARKER}}"""; Flags: runhidden; Check: RelaunchRequested
[Code]
#include "{ROOT / 'public-update.iss'}"
''', encoding='utf-8-sig')
    with (output / 'compiler.log').open('wb') as log:
        subprocess.run([str(compiler / 'ISCC.exe'), str(source)], stdout=log,
                       stderr=subprocess.STDOUT, check=True, timeout=60)
    token = secrets.token_hex(8)
    env = {**os.environ, 'NOH_UPDATE_NATIVE_MARK': token}

    def setup(name, extra):
        log = output / (name + '.log')
        marker = output / (name + '.marker')
        started = time.monotonic()
        result = subprocess.run([str(output / 'update.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
                                 '/MARKER=' + str(marker), '/LOG=' + str(log), *extra],
                                env=env, timeout=180)
        seconds = time.monotonic() - started
        text = log.read_text(encoding='utf-8-sig') if log.exists() else ''
        if 'Runtime error' in text:
            raise ValueError('Pascal runtime error in ' + name)
        marked = marker.read_text(encoding='utf-8', errors='replace').strip() if marker.exists() else None
        return result.returncode, text, marked, seconds, log.exists()

    results = []
    # The launching NOH exits a few seconds later: Setup waits, measures, relaunches.
    leaving = sleeper(6)
    code, text, marked, seconds, _ = setup('relaunch-after-wait',
                                           ['/WAITPID=' + str(leaving.pid), '/RELAUNCH'])
    waited = re.search(r'WAITPID ' + str(leaving.pid) + r' exited after (\d+) ms', text)
    if code != 0 or not waited or int(waited.group(1)) < 3000 or leaving.poll() is None:
        raise ValueError(f'Setup did not wait for the launching process (exit {code})')
    if marked != token:
        raise ValueError(f'The relaunch did not run or did not inherit the environment: {marked!r}')
    results.append({'case': 'relaunch-after-wait', 'exit': code, 'waited_ms': int(waited.group(1)),
                    'relaunched_with_environment': True})

    # Without /RELAUNCH (closing NOH installs without reopening it).
    code, text, marked, _, _ = setup('no-relaunch', [])
    if code != 0 or marked is not None:
        raise ValueError('A silent setup without /RELAUNCH ran the relaunch entry')
    results.append({'case': 'no-relaunch', 'exit': code, 'relaunched': False})

    # A process already gone. While this script still holds its handle, Windows
    # keeps the exited process object: Setup then sees it signalled at once.
    gone = sleeper(0)
    gone.wait()
    code, text, marked, _, _ = setup('already-exited', ['/WAITPID=' + str(gone.pid)])
    instant = re.search(r'WAITPID ' + str(gone.pid) + r' exited after (\d+) ms', text)
    if code != 0 or not (f'WAITPID {gone.pid} had already exited' in text
                         or (instant and int(instant.group(1)) < 1000)):
        raise ValueError('An exited process was not recognised')
    results.append({'case': 'already-exited', 'exit': code})

    # An invalid value changes nothing.
    code, text, marked, _, logged = setup('invalid', ['/WAITPID=abc', '/RELAUNCH'])
    if code == 0 or not logged or 'Invalid WAITPID value: abc' not in text or marked is not None:
        raise ValueError('An invalid WAITPID was accepted')
    results.append({'case': 'invalid', 'exit': code, 'log_written': logged})

    # A NOH that never exits: Setup gives up after 60 s, writes its log, changes nothing.
    stuck = sleeper(120)
    try:
        code, text, marked, seconds, logged = setup('still-running', ['/WAITPID=' + str(stuck.pid), '/RELAUNCH'])
    finally:
        stuck.kill()
        stuck.wait()
    if (code == 0 or not logged or 'still runs after 60 s; nothing was changed' not in text
            or 'Starting the installation process.' in text or marked is not None or seconds < 59):
        raise ValueError(f'A still-running process did not stop Setup (exit {code}, {seconds:.0f} s)')
    results.append({'case': 'still-running', 'exit': code, 'seconds': round(seconds), 'log_written': logged})

    report = {'status': 'passed', 'cases': results,
              'scope': 'Native /WAITPID wait, refusal and /RELAUNCH check of public-update.iss in a synthetic silent setup; '
                       'the real upgrade is tested by test_upgrade.'}
    (output / 'UPDATE.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.compiler.resolve(), args.output.resolve())
