#!/usr/bin/env python3
"""Run a bounded synthetic desktop/audio session inside a disposable container."""
import argparse
import os
from pathlib import Path
import subprocess
import time


def ready(check, children, description):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        if any(child.poll() is not None for child in children):
            raise RuntimeError(f'Test service exited before {description}')
        if check():
            return
        time.sleep(.25)
    raise TimeoutError(f'Test service did not become ready: {description}')


def command_contains(command, text):
    result = subprocess.run(command, capture_output=True, text=True, timeout=3)
    return result.returncode == 0 and text in result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--display', choices=('x11', 'wayland'), required=True)
    parser.add_argument('--audio', choices=('pulseaudio', 'pipewire'), required=True)
    args = parser.parse_args()
    runtime, config = Path('/tmp/noh-runtime'), Path('/tmp/noh-config')
    runtime.mkdir(mode=0o700)
    config.mkdir()
    Path('/tmp/noh-home').mkdir()
    os.environ.update(HOME='/tmp/noh-home', XDG_RUNTIME_DIR=str(runtime),
                      XDG_CONFIG_HOME=str(config), LIBGL_ALWAYS_SOFTWARE='1',
                      XDG_SESSION_TYPE=args.display)
    children, logs = [], []
    def start(name, command):
        log = Path('/evidence', name + '.log').open('wb')
        logs.append(log)
        child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
    try:
        if args.display == 'x11':
            os.environ['DISPLAY'] = ':99'
            start('xvfb', ['Xvfb', ':99', '-screen', '0', '1280x1024x24', '-nolisten', 'tcp'])
            ready(lambda: Path('/tmp/.X11-unix/X99').exists(), children, 'X11 socket')
        else:
            os.environ['WAYLAND_DISPLAY'] = 'wayland-noh'
            start('weston', ['weston', '--backend=headless', '--renderer=pixman',
                             '--socket=wayland-noh', '--idle-time=0', '--width=1280', '--height=1024'])
            ready(lambda: (runtime / 'wayland-noh').exists(), children, 'Wayland socket')
        if args.audio == 'pipewire':
            settings = config / 'pipewire/pipewire.conf.d'
            settings.mkdir(parents=True)
            (settings / '10-noh-null.conf').write_text('''context.objects = [
 { factory = adapter args = { factory.name = support.null-audio-sink
   node.name = noh_test_sink media.class = "Audio/Sink" audio.position = [ FL FR ] } }
]\n''')
            start('pipewire', ['pipewire'])
            ready(lambda: command_contains(['pw-dump'], 'noh_test_sink'), children, 'PipeWire sink')
            start('wireplumber', ['wireplumber'])
            ready(lambda: command_contains(['wpctl', 'inspect', '@DEFAULT_AUDIO_SINK@'], 'noh_test_sink'),
                  children, 'default PipeWire sink')
        else:
            start('pulseaudio', ['pulseaudio', '--daemonize=no', '--exit-idle-time=-1',
                                '--load=module-null-sink sink_name=noh_test_sink'])
            ready(lambda: command_contains(['pactl', 'info'], 'noh_test_sink'), children, 'PulseAudio sink')
        subprocess.run(['python3', '/work/tools/probe-linux-host.py', '--appdir', '/candidate/NOH.AppDir',
                        '--graphics', 'wayland' if args.display == 'wayland' else 'surfaceless',
                        '--gui', '--speech-fixture', '/speech.wav', '--output', '/evidence/result.json'],
                       check=True, timeout=480)
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in reversed(children):
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
