#!/usr/bin/env python3
"""Drive real chooser and native file-drop observations through private QEMU input.

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

        def move(x, y):
            execute('input-send-event', {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': round(x * 32767 / 1279)}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': round(y * 32767 / 1023)}}]})

        def button(down):
            execute('input-send-event', {'events': [{'type': 'btn', 'data': {'down': down, 'button': 'left'}}]})

        def drag(start, end):
            move(*start)
            time.sleep(.3)
            button(True)
            for i in range(1, 21):
                move(*(round(a + (b - a) * i / 20) for a, b in zip(start, end)))
                time.sleep(.08)

        def sender_bounds(name):
            from PIL import Image
            with Image.open(output / f'{name}.ppm') as shot:
                if shot.size != (1280, 1024):
                    raise ValueError('Unexpected guest scanout dimensions')
                pixels = shot.convert('RGB')
                mask = Image.new('1', pixels.size)
                # This deliberately differs from the synthetic video's magenta.
                mask.putdata([pixel == (236, 23, 219) for pixel in pixels.getdata()])
                bounds = mask.getbbox()
                if bounds is None or sum(mask.getdata()) < 2000:
                    raise ValueError('No visible native GTK source button')
                return bounds

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
        elif phase == 'drop':
            screen('01-source', 3)
            left, top, right, bottom = sender_bounds('01-source')
            # Move the real sender by its title bar to expose the NOH target.
            # Re-observe the colored button before starting the actual file drag.
            start = ((left + right) // 2, top - 15)
            drag(start, (start[0] + 980 - left, start[1] + 150 - top))
            button(False)
            screen('02-positioned')
            left, top, right, bottom = sender_bounds('02-positioned')
            if left < 900:
                raise ValueError('Source did not move aside; refusing an ambiguous target')
            drag(((left + right) // 2, (top + bottom) // 2), (500, 400))
            try:
                screen('03-hover')
            finally:
                button(False)
            screen('04-imported', 6)
        else:
            screen('boot' if phase == 'boot' else '07-output-opened', 3)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--socket', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phase', choices=('chooser', 'drop', 'output', 'boot'), required=True)
    parser.add_argument('--desktop', choices=('GNOME', 'KDE', 'Cinnamon'), default='GNOME')
    args = parser.parse_args()
    observe(args.socket, args.output, args.phase, args.desktop)
