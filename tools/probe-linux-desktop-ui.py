#!/usr/bin/env python3
"""Drive a bounded real chooser sequence through the private QEMU keyboard.

Screens are observations requiring independent review, never automatic UI passes.
Only the disposable guest receives input; no host desktop is controlled.
"""
import argparse
import json
from pathlib import Path
import socket
import time


def observe(socket_path, output, phase, desktop='GNOME'):
    output.mkdir(parents=True, exist_ok=False)
    with socket.socket(socket.AF_UNIX) as connection, (output / 'actions.jsonl').open('x') as log:
        connection.settimeout(15)
        connection.connect(str(socket_path))
        stream = connection.makefile('rwb', buffering=0)
        if 'QMP' not in json.loads(stream.readline()):
            raise ValueError('Missing QMP greeting')

        def execute(name, arguments=None):
            request = {'execute': name}
            if arguments is not None:
                request['arguments'] = arguments
            log.write(json.dumps(request) + '\n')
            log.flush()
            stream.write(json.dumps(request).encode() + b'\n')
            while True:
                reply = json.loads(stream.readline())
                if 'error' in reply:
                    raise RuntimeError(reply)
                if 'return' in reply:
                    return reply['return']

        def key(*codes):
            execute('send-key', {'keys': [{'type': 'qcode', 'data': code} for code in codes], 'hold-time': 30})
            time.sleep(.12)

        def screen(name, delay=1):
            time.sleep(delay)
            execute('screendump', {'filename': str(output.resolve() / f'{name}.ppm')})

        execute('qmp_capabilities')
        if phase == 'chooser':
            key('esc')
            # Focus the observed application area on the 1280x1024 guest. This
            # also selects its window when GNOME initially shows the overview.
            execute('input-send-event', {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': 16384}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': 9830}},
                {'type': 'btn', 'data': {'down': True, 'button': 'left'}}]})
            time.sleep(.1)
            execute('input-send-event', {'events': [{'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})
            screen('01-normal')
            key('shift', 'tab')
            key('ret')
            screen('02-chooser', 3)
            key('esc')
            screen('03-cancelled')
            key('ret')
            time.sleep(2)
            # KDE's location bar navigates directories; its Name mnemonic
            # targets the actual file selection entry, including full paths.
            if desktop == 'KDE':
                key('alt', 'n')
            else:
                key('ctrl', 'l')
            key('ctrl', 'a')
            # Fixed ASCII fixture path; no clipboard, capture-project injection,
            # or compositor-specific input API is involved.
            for char in '/home/noh/noh-evidence/media/synthetic.mp4':
                key({'/': 'slash', '-': 'minus', '.': 'dot'}.get(char, char))
            screen('04-location')
            key('ret')
            screen('05-selection')
            key('ret')
            # Newer GNOME's chooser first navigates to/selects the pathname;
            # use its Open mnemonic to confirm the remaining selection. This
            # has no binding in NOH when an older chooser already accepted it.
            time.sleep(1)
            key('alt', 'o')
            screen('06-imported', 5)
        else:
            screen('boot' if phase == 'boot' else '07-output-opened', 3)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--socket', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phase', choices=('chooser', 'output', 'boot'), required=True)
    parser.add_argument('--desktop', choices=('GNOME', 'KDE', 'Cinnamon'), default='GNOME')
    args = parser.parse_args()
    observe(args.socket, args.output, args.phase, args.desktop)
