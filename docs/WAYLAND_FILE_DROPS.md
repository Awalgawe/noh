# Native Wayland file drops

NOH 0.1.4 could import through the file chooser but received no external file
drops on native Wayland. Winit 0.30.13 did not implement the Wayland file-drop
protocol. Enabling the viewport's drag-and-drop flag did not add that backend
support. This explains [issue 25](https://github.com/Awalgawe/noh/issues/25);
working X11 or Wine imports do not establish native Wayland support.

## Temporary dependency backports

NOH 0.1.5 pins two source changes until compatible releases contain them:

- [winit PR 4729](https://github.com/rust-windowing/winit/pull/4729), revision
  `7a962fd46470a137d9a7a1945bf5b5d0b95a002c`, implements local `text/uri-list`
  transfers, hover/cancellation events and a shared Wayland clipboard.
- [egui PR 8657](https://github.com/emilk/egui/pull/8657)'s clipboard change,
  backported to the published egui-winit 0.36.2 source in
  [revision 3c7cc546](https://github.com/Awalgawe/egui/commit/3c7cc5468ca8de52f20ba5ac0110d3f53c29439a).
  It uses winit's data device instead of opening a competing smithay clipboard
  device. The 0.36.2 API and registry egui dependency are preserved. The crate
  includes the unchanged original Apache and MIT notices.

The registry DPI dependency remains at 0.1.2. The lockfile pins both Git revisions;
delivery materials include their sources through the existing Cargo vendor
workflow. Do not update only one half of the clipboard/data-device integration.
Remove these patches together once a released combination provides both features
and passes the native regression probe. Media import validation, worker threads
and translations continue to use the existing application code.

## Reproduce the protocol boundary

`tools/probe-wayland-dnd.py` creates its own Xvfb display and a nested Weston
compositor. A separate GTK application offers real file URIs, and pointer input
goes only to that private display. The probe retains protocol logs and screenshots;
it never injects egui events or imports via command-line file arguments.

Install Weston, Xwayland, Xvfb, xdotool, GTK 3 introspection, Python Xlib and Pillow.
Use synthetic PNG, MP4 and WAV fixtures, including a filename with spaces, Unicode,
`#` and `%`. For the default 1600x1050 test display:

```sh
timeout 90 python3 tools/probe-wayland-dnd.py \
  --app target/debug/noh-app --output logs/wayland-native \
  --backend wayland --source-backend wayland --destination 800 400 \
  --cancel-first --change-button 350 903 \
  'fixtures/image space é #%.png' fixtures/video.mp4 fixtures/sound.wav
```

The cancellation case leaves the target before release, then retries a fresh
drag. The clipboard case pastes from GTK into the output-name draft, copies and
pastes within NOH, then reads the selection back in GTK. No export is started.
Protocol assertions verify that native transfers occurred; independently inspect
the cancelled, hover, imported and clipboard screenshots for actual UI behavior.
These virtual-display observations do not qualify a named KDE/GNOME release or
physical hardware. The release desktop matrix remains a separate requirement.
