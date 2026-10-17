#!/usr/bin/env python3
"""Observe real GTK file drops into NOH on a private nested Weston display.

Requires Weston, Xvfb, Xwayland, xdotool, GTK 3 introspection, python3-xlib
and Pillow. Only the isolated X display receives input. Screens require review;
successful protocol assertions alone do not prove a successful media import.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import re
import subprocess
import sys
import time


def source(paths, receiver=False):
    import gi
    gi.require_version('Gtk', '3.0')
    gi.require_version('Gdk', '3.0')
    from gi.repository import Gdk, Gtk
    window = Gtk.Window(title='NOH drag fixture' if not receiver else 'GTK control target')
    window.set_default_size(980, 850) if receiver else window.set_default_size(280, 150)
    button = Gtk.Button(label='DRAG SYNTHETIC FILES' if not receiver else 'DROP CONTROL')
    css = Gtk.CssProvider()
    css.load_from_data(b'button {background-image:none;background-color:#0080ff;color:#000000;}' if receiver else b'button {background-image:none;background-color:#ec17db;color:#000000;}')
    button.get_style_context().add_provider(css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    targets = [Gtk.TargetEntry.new('text/uri-list', 0, 0)]
    if receiver:
        button.drag_dest_set(Gtk.DestDefaults.ALL, targets, Gdk.DragAction.COPY)
        def receive(widget, context, x, y, data, info, timestamp):
            print(json.dumps({'received': data.get_uris()}), flush=True)
            Gtk.drag_finish(context, True, False, timestamp)
        button.connect('drag-data-received', receive)
    else:
        button.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, targets, Gdk.DragAction.COPY)
        def get_data(widget, context, data, info, timestamp):
            data.set_uris([p.resolve().as_uri() for p in paths])
            print(json.dumps({'sent': [str(p) for p in paths]}), flush=True)
        button.connect('drag-data-get', get_data)
        button.connect('drag-begin', lambda *args: print('drag-begin', flush=True))
        button.connect('drag-end', lambda *args: print('drag-end', flush=True))
        button.connect('drag-failed', lambda widget, context, reason: print('drag-failed:' + str(reason), flush=True) or False)
    window.add(button)
    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    def key_press(widget, event):
        if event.keyval == Gdk.KEY_c:
            clipboard.set_text('NOH clipboard é #%.mp4', -1)
            print('clipboard-set', flush=True)
        elif event.keyval == Gdk.KEY_r:
            clipboard.request_text(lambda clipboard, text, data: print(
                json.dumps({'clipboard-read': text}), flush=True), None)
    window.connect('key-press-event', key_press)
    window.connect('destroy', Gtk.main_quit)
    window.show_all()
    Gtk.main()


def run(args):
    from PIL import Image
    from Xlib import display, X
    output = args.output.resolve()
    if not args.app.is_file() or any(not path.is_file() for path in args.files):
        raise ValueError('The executable and every synthetic fixture must exist')
    output.mkdir(parents=True, exist_ok=False)
    processes, logs = [], []
    runtime = Path('/tmp') / f'noh-wayland-probe-{os.getpid()}'
    runtime.mkdir(mode=0o700)
    env = os.environ.copy()
    for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'DBUS_SESSION_BUS_ADDRESS'):
        env.pop(name, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), LIBGL_ALWAYS_SOFTWARE='1')
    def launch(command, name, extra=None, **kwargs):
        log = (output / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, env=env | (extra or {}), stdout=log, stderr=subprocess.STDOUT, start_new_session=True, **kwargs)
        processes.append(process)
        return process
    def wait_for(predicate, seconds=15):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(.1)
        raise TimeoutError('Private display did not become ready')
    def mouse(*commands):
        subprocess.run(['xdotool', *map(str, commands)], env=env, check=True, timeout=5)
    connection = None
    try:
        read_fd, write_fd = os.pipe()
        try:
            launch(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1600x1050x24', '-nolisten', 'tcp'], 'xvfb', pass_fds=(write_fd,))
        finally:
            os.close(write_fd)
        import select
        try:
            if not select.select([read_fd], [], [], 15)[0]:
                raise TimeoutError('Xvfb did not allocate its private display')
            number = os.read(read_fd, 20).decode().strip()
        finally:
            os.close(read_fd)
        if not number.isdigit():
            raise RuntimeError('Invalid private X display number')
        env['DISPLAY'] = ':' + number
        launch(['weston', '--backend=x11-backend.so', '--socket=noh-wayland', '--width=1600', '--height=1000', '--use-pixman', '--xwayland', '--idle-time=0', '--no-config'], 'weston')
        wait_for(lambda: (runtime / 'noh-wayland').exists())
        env['WAYLAND_DISPLAY'] = 'noh-wayland'
        time.sleep(1)
        # Weston advertises its Xwayland display in its own launch log.
        def xdisplay():
            found = re.search(r'xserver listening on display (:\d+)', (output / 'weston.log').read_text())
            return found.group(1) if found else None
        wait_for(xdisplay)
        xwayland = xdisplay()
        target_env = {'DISPLAY': xwayland, 'WAYLAND_DEBUG': 'client', 'NOH_LANGUAGE': 'en'}
        if args.backend == 'x11':
            target_env['WAYLAND_DISPLAY'] = ''
        target = launch([sys.executable, __file__, '--receiver'] if args.control else [str(args.app.resolve())], 'noh', target_env)
        time.sleep(4)
        if target.poll() is not None:
            raise RuntimeError('NOH exited: ' + str(target.returncode))
        launch([sys.executable, __file__, '--source', *map(str, args.files)], 'source', {'DISPLAY': xwayland, 'GDK_BACKEND': args.source_backend, 'WAYLAND_DEBUG': 'client'})
        time.sleep(2)
        connection = display.Display(env['DISPLAY'])
        root = connection.screen().root
        def capture(name):
            geometry = root.get_geometry()
            raw = root.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
            picture = Image.frombytes('RGB', (geometry.width, geometry.height), raw.data, 'raw', 'BGRX')
            picture.save(output / (name + '.png'))
            return picture
        def source_points(picture):
            pixels = picture.load()
            points = [(x, y) for y in range(0, picture.height, 4) for x in range(0, picture.width, 4) if pixels[x, y][:3] == (236, 23, 219)]
            if not points:
                raise RuntimeError('GTK drag source not visible')
            return points
        points = source_points(capture('initial'))
        # Keep the sender beside NOH so target focus cannot hide it, and never
        # mistake a release over the sender for a cross-application drop.
        if args.source_backend == 'wayland' and min(x for x, y in points) < 1120:
            heading = (sum(x for x, y in points) // len(points), min(y for x, y in points) - 16)
            mouse('mousemove', *heading, 'mousedown', 1)
            for i in range(1, 21):
                mouse('mousemove', round(heading[0] + (1320 - heading[0]) * i / 20), round(heading[1] + (550 - heading[1]) * i / 20))
                time.sleep(.03)
            mouse('mouseup', 1)
            time.sleep(.5)
        points = source_points(capture('before'))
        if args.change_button and min(x for x, y in points) < 1120:
            raise RuntimeError('GTK source could not be positioned beside NOH')
        start = (sum(x for x, y in points) // len(points), sum(y for x, y in points) // len(points))
        end = None
        if args.destination:
            end = tuple(args.destination)
            def over_source(point):
                return (min(x for x, y in points) - 8 <= point[0] <= max(x for x, y in points) + 8
                        and min(y for x, y in points) - 40 <= point[1] <= max(y for x, y in points) + 8)
            if over_source(end):
                end = next(point for point in ((800, 400), (200, 250), (900, 800)) if not over_source(point))
            def drag():
                mouse('mousemove', *start)
                mouse('mousedown', 1)
                for i in range(1, 31):
                    mouse('mousemove', round(start[0] + (end[0] - start[0]) * i / 30), round(start[1] + (end[1] - start[1]) * i / 30))
                    time.sleep(.03)
                time.sleep(.7)
            if args.cancel_first:
                drag()
                capture('cancel-hover')
                # Leave the target before releasing; compositor-specific Escape
                # shortcuts are not part of this cancellation observation.
                mouse('mousemove', 1500, 950)
                time.sleep(.3)
                mouse('mouseup', 1)
                time.sleep(.8)
                capture('cancelled')
                if re.search(r'wl_data_device@\d+\.drop\(', (output / 'noh.log').read_text()):
                    raise AssertionError('A cancelled drag generated a native drop')
            drag()
            capture('hover')
            mouse('mouseup', 1)
            time.sleep(5)
            capture('after')
        if args.change_button:
            mouse('mousemove', *start, 'click', 1, 'key', 'c')
            time.sleep(.3)
            mouse('mousemove', *args.change_button, 'click', 1)
            time.sleep(.5)
            mouse('key', 'ctrl+a', 'ctrl+v')
            time.sleep(.5)
            capture('clipboard-paste')
            mouse('key', 'ctrl+a', 'ctrl+c')
            # Exercise our own selection without waiting on our event loop.
            mouse('type', 'replacement.mp4')
            mouse('key', 'ctrl+a', 'ctrl+v')
            time.sleep(.5)
            capture('clipboard-self-paste')
            mouse('key', 'ctrl+a', 'ctrl+c')
            mouse('mousemove', *start, 'click', 1, 'key', 'r')
            time.sleep(.8)
            readback = [json.loads(line)['clipboard-read'] for line in (output / 'source.log').read_text().splitlines() if line.startswith('{"clipboard-read":')]
            if readback != ['NOH clipboard é #%.mp4']:
                raise AssertionError('Native clipboard round trip failed: ' + repr(readback))
        protocol = (output / 'noh.log').read_text()
        if args.backend == 'wayland' and not args.control:
            for message in ('receive', 'finish'):
                if not re.search(r'wl_data_offer@\d+\.' + message + r'\(', protocol):
                    raise AssertionError('Missing native Wayland file transfer: ' + message)
            if args.change_button and not re.search(r'wl_data_device@\d+\.set_selection\(', protocol):
                raise AssertionError('NOH did not own the native clipboard')
        (output / 'run.json').write_text(json.dumps({'backend': args.backend, 'source_backend': args.source_backend, 'control': args.control, 'app': str(args.app), 'files': [str(p) for p in args.files], 'source': start, 'destination': end, 'cancel_first': args.cancel_first, 'clipboard_round_trip': bool(args.change_button), 'xwayland': xwayland, 'target_exit': target.poll(), 'visual_review_required': True}, indent=2))
        print(str(output / 'run.json'), flush=True)
    finally:
        if connection:
            connection.close()
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--receiver':
        source([], receiver=True)
    elif len(sys.argv) > 1 and sys.argv[1] == '--source':
        source([Path(p) for p in sys.argv[2:]])
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--app', type=Path, required=True)
        parser.add_argument('--output', type=Path, required=True)
        parser.add_argument('--backend', choices=('wayland', 'x11'), required=True)
        parser.add_argument('--source-backend', choices=('wayland', 'x11'), default='wayland')
        parser.add_argument('--control', action='store_true')
        parser.add_argument('--cancel-first', action='store_true')
        parser.add_argument('--change-button', type=int, nargs=2, help='NOH Change button coordinates; exercises native text clipboard')
        parser.add_argument('--destination', type=int, nargs=2)
        parser.add_argument('files', type=Path, nargs='+')
        run(parser.parse_args())
