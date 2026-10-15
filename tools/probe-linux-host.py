#!/usr/bin/env python3
"""Exercise host graphics and ALSA -> libmpv -> default PCM in one process.

Run on each supported desktop against a prepared AppDir. This uses no window
and writes silence; it does not establish visual quality or audible playback.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import tempfile

from delivery import digest, write
from linux_elf import audit_bundle, require_host_runtime
from linux_host import check_host


def check_gui_report(report):
    preview = report.get('mini_preview', {})
    player = preview.get('player') or {}
    frame = player.get('frame') or {}
    if (report.get('fixture') or report.get('timed_out') or not report.get('capture_ready')
            or report.get('project_terminal') != 'finished' or report.get('diagnosis_error')
            or report.get('resource_issues') != 0 or preview.get('explicit_ffmpeg')
            or preview.get('error') or player.get('error') or player.get('warning')
            or not player.get('audio_available') or frame.get('position', 0) < 1
            or frame.get('width', 0) <= 0 or frame.get('height', 0) <= 0):
        raise ValueError('Actual NOH import/video/audio preview did not pass; inspect gui.json')


def gui_preview(appdir, clip, soundtrack, tmp, evidence):
    project = Path(tmp) / 'project.json'
    write(project, {'videos': [str(clip)], 'wav': str(soundtrack),
                    'output': str(Path(tmp) / 'gui-export.mp4'), 'ffmpeg': '',
                    'fade_in': 0, 'fade_out': 0, 'partial_fades': False,
                    'preview': False, 'clip_audio': False, 'force_encode': True})
    capture = evidence / 'gui.ppm'
    env = dict(os.environ, NOH_CAPTURE_UI=str(capture), NOH_CAPTURE_PROJECT=str(project),
               NOH_CAPTURE_PROJECT_ACTION='live-preview', NOH_CAPTURE_PLAYBACK_UNTIL_MS='1000')
    with (evidence / 'gui.log').open('w') as log:
        subprocess.run([str(appdir / 'AppRun')], check=True, stdout=log,
                       stderr=subprocess.STDOUT, env=env, cwd='/', timeout=120)
    report = json.loads(capture.with_suffix('.json').read_text())
    check_gui_report(report)
    if not capture.is_file() or capture.stat().st_size < 10000:
        raise ValueError('NOH GUI capture is missing')
    return {'report_sha256': digest(capture.with_suffix('.json')),
            'capture_sha256': digest(capture), 'preview': report['mini_preview']}


def transcribe(home, source, tmp):
    output = Path(tmp) / 'transcript.srt'
    speech = home / 'bin/speech'
    command = [str(home / 'bin/noh-cli'), '--transcribe', str(source), '--output', str(output),
               '--transcriber', str(speech / 'whisper-cli'), '--model', str(speech / 'ggml-small.bin'),
               '--vad-model', str(speech / 'ggml-silero-v6.2.0.bin'), '--language', 'en', '--json']
    run = subprocess.run(command, check=True, capture_output=True, text=True, cwd='/', timeout=300)
    record = json.loads(run.stdout)
    cues = record['result']['data']['Ok']['track']['cues']
    if record['result']['event'] != 'subtitled' or not cues or '-->' not in output.read_text():
        raise ValueError('Packaged CPU speech did not produce subtitle cues')
    return {'source_sha256': digest(source), 'subtitle_sha256': digest(output), 'cues': len(cues)}


def function(library, name, result, *arguments):
    fn = getattr(library, name)
    fn.restype, fn.argtypes = result, list(arguments)
    return fn


def graphics(backend):
    egl = C.CDLL('libEGL.so.1')
    native = None
    wayland = None
    if backend == 'wayland':
        wayland = C.CDLL('libwayland-client.so.0')
        native = function(wayland, 'wl_display_connect', C.c_void_p, C.c_char_p)(None)
        if not native:
            raise ValueError('Wayland connection failed; a real compositor socket is required')
    get_display = function(egl, 'eglGetPlatformDisplay', C.c_void_p, C.c_uint, C.c_void_p, C.c_void_p)
    display = get_display(0x31D8 if backend == 'wayland' else 0x31DD, native, None)
    major, minor = C.c_int(), C.c_int()
    initialize = function(egl, 'eglInitialize', C.c_uint, C.c_void_p, C.POINTER(C.c_int), C.POINTER(C.c_int))
    if not display or not initialize(display, C.byref(major), C.byref(minor)):
        raise ValueError('Host EGL initialization failed')
    context = None
    try:
        bind = function(egl, 'eglBindAPI', C.c_uint, C.c_uint)
        choose = function(egl, 'eglChooseConfig', C.c_uint, C.c_void_p, C.POINTER(C.c_int),
                          C.POINTER(C.c_void_p), C.c_int, C.POINTER(C.c_int))
        config, count = C.c_void_p(), C.c_int()
        attrs = (C.c_int * 5)(0x3040, 8, 0x3033, 0, 0x3038)
        if not bind(0x30A2) or not choose(display, attrs, C.byref(config), 1, C.byref(count)) or count.value < 1:
            raise ValueError('Host EGL has no OpenGL configuration')
        create = function(egl, 'eglCreateContext', C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p, C.POINTER(C.c_int))
        context = create(display, config, None, (C.c_int * 1)(0x3038))
        current = function(egl, 'eglMakeCurrent', C.c_uint, C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)
        if not context or not current(display, None, None, context):
            raise ValueError('Host OpenGL context creation failed')
        gl = C.CDLL('libGL.so.1')
        renderer = function(gl, 'glGetString', C.c_char_p, C.c_uint)(0x1F01)
        if not renderer:
            raise ValueError('Host OpenGL context has no renderer')
        return {'backend': backend, 'egl': f'{major.value}.{minor.value}', 'renderer': renderer.decode()}
    finally:
        if context:
            function(egl, 'eglMakeCurrent', C.c_uint, C.c_void_p, C.c_void_p, C.c_void_p, C.c_void_p)(display, None, None, None)
            function(egl, 'eglDestroyContext', C.c_uint, C.c_void_p, C.c_void_p)(display, context)
        function(egl, 'eglTerminate', C.c_uint, C.c_void_p)(display)
        if native:
            function(wayland, 'wl_display_disconnect', None, C.c_void_p)(native)


def audio(alsa):
    mpv = C.CDLL('libmpv.so.2', mode=C.RTLD_GLOBAL)
    context = function(mpv, 'mpv_create', C.c_void_p)()
    if not context:
        raise ValueError('Host libmpv creation failed')
    pcm = C.c_void_p()
    try:
        if function(mpv, 'mpv_initialize', C.c_int, C.c_void_p)(context) < 0:
            raise ValueError('Host libmpv initialization failed')
        opened = function(alsa, 'snd_pcm_open', C.c_int, C.POINTER(C.c_void_p), C.c_char_p, C.c_int, C.c_int)(C.byref(pcm), b'default', 0, 0)
        if opened < 0:
            raise ValueError(f'Host default PCM failed after libmpv: {opened}')
        params = function(alsa, 'snd_pcm_set_params', C.c_int, C.c_void_p, C.c_int, C.c_int, C.c_uint, C.c_uint, C.c_int, C.c_uint)
        if params(pcm, 2, 3, 2, 48000, 1, 100000) < 0:
            raise ValueError('Host PCM configuration failed')
        silence = (C.c_short * 1920)()
        frames = function(alsa, 'snd_pcm_writei', C.c_long, C.c_void_p, C.c_void_p, C.c_ulong)(pcm, silence, 960)
        if frames != 960:
            raise ValueError(f'Host PCM accepted {frames} frames instead of 960')
        paths = sorted({line.split()[-1] for line in Path('/proc/self/maps').read_text().splitlines()
                        if line.split()[-1].startswith('/')})
        return {'silent_frames': frames, 'loaded_paths': paths}
    finally:
        if pcm:
            function(alsa, 'snd_pcm_close', C.c_int, C.c_void_p)(pcm)
        function(mpv, 'mpv_terminate_destroy', None, C.c_void_p)(context)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--appdir', type=Path, required=True)
    parser.add_argument('--graphics', choices=('wayland', 'surfaceless'), required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--loader-only', action='store_true', help='Record an explicitly partial graphics/audio check without the FFmpeg export')
    parser.add_argument('--gui', action='store_true', help='Use the existing NOH capture path for actual import and video/audio preview')
    parser.add_argument('--speech-fixture', type=Path, help='Synthetic spoken WAV for actual packaged CPU transcription')
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError('Refusing to overwrite probe evidence')
    args.appdir = args.appdir.resolve()
    args.output = args.output.resolve()
    if args.loader_only and (args.gui or args.speech_fixture):
        parser.error('Loader-only mode cannot claim GUI or transcription coverage')
    for key in ('ALSA_PLUGIN_DIR', 'LD_LIBRARY_PATH', 'LD_PRELOAD', 'NOH_LIBMPV', 'NOH_FFMPEG'):
        if os.environ.get(key):
            raise ValueError(f'Run with the unmodified host discovery environment: {key}')
    report = audit_bundle(args.appdir, host_runtime=True)
    require_host_runtime(report)
    home = args.appdir / 'usr/lib/noh'
    result = {'scope': 'loader-only' if args.loader_only else 'host-runtime-smoke',
              'os_release': Path('/etc/os-release').read_text(),
              'elf_sha256': {n: i['sha256'] for n, i in report['images'].items()},
              'display_environment': {k: os.environ.get(k) for k in ('DISPLAY', 'WAYLAND_DISPLAY', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE')}}
    # Match the application: ALSA is already linked before graphics/libmpv load.
    alsa = C.CDLL('libasound.so.2', mode=C.RTLD_GLOBAL)
    result['graphics'] = graphics(args.graphics)
    result['audio'] = audio(alsa)
    if any(str(args.appdir.resolve()) in p for p in result['audio']['loaded_paths']):
        raise ValueError('The probe loaded an in-process library from the AppDir')
    result['workers'] = {}
    for name in ('noh-app', 'bin/noh-cli', 'bin/noh-mcp'):
        cmd = args.appdir / 'AppRun' if name == 'noh-app' else home / name
        run = subprocess.run([str(cmd), '--build-info'], check=True, capture_output=True, text=True, timeout=30)
        result['workers'][name] = json.loads(run.stdout)
    if not args.loader_only:
        result['host'] = check_host()
        with tempfile.TemporaryDirectory(prefix='noh-host-smoke-') as tmp:
            clip = Path(tmp) / 'synthetic.mp4'
            soundtrack = Path(tmp) / 'silence.wav'
            output = Path(tmp) / 'composition.mp4'
            ffmpeg = result['host']['ffmpeg']
            for command in ([ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=128x72:rate=12:duration=3',
                             '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-n', str(clip)],
                            [ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
                             '-t', '3', '-ac', '2', '-c:a', 'pcm_s16le', '-n', str(soundtrack)]):
                subprocess.run(command, check=True, capture_output=True, text=True, timeout=45)
            command = [str(home / 'bin/noh-cli'), str(clip), str(soundtrack), '--reencode',
                       '--fade-in', '0', '--fade-out', '0', '--output', str(output)]
            subprocess.run(command, check=True, capture_output=True, text=True, timeout=60, cwd='/')
            subprocess.run([ffmpeg, '-v', 'error', '-i', str(output), '-f', 'null', '-'],
                           check=True, capture_output=True, text=True, timeout=30)
            result['synthetic_export_sha256'] = digest(output)
            result['export_working_directory'] = '/'
            duplicate = subprocess.run(command, capture_output=True, text=True, timeout=30, cwd='/')
            if duplicate.returncode == 0 or digest(output) != result['synthetic_export_sha256']:
                raise ValueError('NOH did not preserve the existing export')
            result['no_overwrite_exit_code'] = duplicate.returncode
            if args.gui:
                result['gui'] = gui_preview(args.appdir, clip, soundtrack, tmp, args.output.parent)
            if args.speech_fixture:
                result['transcription'] = transcribe(home, args.speech_fixture.resolve(), tmp)
    write(args.output, result)
    print(json.dumps({'scope': result['scope'], 'graphics': result['graphics'],
                      'silent_frames': result['audio']['silent_frames'], 'output': str(args.output)}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(f'Probe command failed ({error.returncode}): {error.cmd}\n{error.stderr}')
