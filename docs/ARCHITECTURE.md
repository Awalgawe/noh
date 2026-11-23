# Architecture

One Rust media core serves the desktop, command line and optional local MCP
server. FFmpeg performs media inspection and rendering; libmpv supports the
timeline preview, paused and, where it is the default player, playing.
Rendering is isolated from the UI thread.

| Boundary | Responsibility |
| --- | --- |
| `engine`, `input`, `plan`, `inspection` | Typed requests, source validation, exact diagnosis and copy/conversion decisions |
| `jobs`, `process`, `storage` | Isolated workers, bounded progress, cancellation, owned temporary files and no-overwrite publication |
| `media`, `sequence`, `hybrid`, `h264`, `timing` | Media preparation, compatible copying, selective conversion, fades and timestamp verification |
| `project`, `shorts`, `captions`, `subtitle_*` | Composed output, selected intervals, caption layout and local transcription |
| `app_*`, `playback`, `preview`, `scrub`, `waveform` | Desktop state, presentation, asynchronous metadata and monitor/timeline playback |
| `cli`, `mcp` | CLI parsing and local stdio protocol adapters |
| `update` | Optional authenticated update contract and controlled Windows installation prototype |
| `tools` | Build, packaging, capture and qualification utilities |

## Worker and output ownership

The GUI and MCP adapter use the same job controller as the CLI. Workers report
typed events rather than changing UI state directly. Progress is coalesced;
slow consumers do not authorize losing the terminal result. Cancellation waits
for the owned process tree and cleanup. A committed output remains a successful
publication even if a later cancellation arrives.

Temporary work belongs to one operation. Final publication does not replace an
existing destination. Exact diagnosis runs independently of export, and export
revalidates the relevant inputs before using a prior decision. Metadata stamps
are invalidation hints, not locked snapshots or a content-integrity guarantee.

## Desktop state

`app_state` derives action availability from current project revisions and job
state. Destination identity is separate from render-setting identity.
`app_diagnosis` coalesces background checks; obsolete replies cannot replace a
newer request. `app_ui` composes media, lyrics, monitor, timeline, selection and
export controls on one screen. Settings holds technical configuration.
Each frame first takes the job's events, starts a prepared export, runs the
checks and follows the copies (`advance`), then draws. A window eframe sees as
minimized, or as occluded where winit reports it (macOS and X11), draws
nothing: eframe then runs `App::logic` alone, which does the same work and
writes a due lyrics change, so a prepared export still starts and its progress
and end are taken while the window is away. Checked on Windows, minimized.

A window opened by `noh-mcp` (`noh_app_open`) runs with `--driven`: the
server is its parent and the window's standard input and output are the only
channel (`driven`, one JSON message per line, bounded lines, a writer thread
that never blocks its caller). `app_driven` serves the channel from `advance`,
so a minimized window answers, and filters the person's input in
`raw_input_hook` by an allow-list built from the rectangles the previous drawn
frame registered (navigation, the ruler, designation, the badge): keys, drops
and every other press are taken out before the interface sees them. The server
side (`mcp::apps`) keeps each window's state, never waits on a window, and
computes the cap from the machine's free memory (`mcp::machine`).

The desktop preview can prepare lightweight working copies. Export uses the
original sources. Waveform decoding and reductions run in a bounded worker;
changing a view does not require decoding the soundtrack again.
Silent timeline seeking uses indexed presentation timestamps from the preview
copy, selects a representable frame and keeps a five-second deadline through
frame delivery. Newer seek requests can supersede an outstanding target.

Play asks the same libmpv instance to play the montage as one virtual file: the
light copies cut as the montage cuts them, with the song as a second stream.
It keeps that file open while paused, so a click or a drag on the time ruler is
a seek in it. `playback::Player` forwards to that engine (`playback::Live`);
the interface drives either player the same way. `preview::play_plan` refuses
what the file cannot express (clip sound, a still image, captions drawn by
libass, a copy that is not ready); a refusal, a decoder
error, no picture within three seconds or no sound output within one second of
playing sends the same Play to the FFmpeg player. A refusal comes at once and
is not remembered. A montage given up after libmpv took it is: the FFmpeg
player then plays for the rest of the session, rather than paying the same wait
at each Play. A seek goes to the time of the frame the playhead is in, counted
as the paused preview counts it (`preview::PlayGrid`): given the time itself,
libmpv shows the next frame. libmpv is the default player on Windows only;
`NOH_PLAYER` chooses otherwise.

Three threads meet there: the controller, which talks to libmpv; libmpv's own;
and the interface's, which renders libmpv's pictures. libmpv waits for the
render thread, so nothing may be asked of libmpv while a lock the interface
takes is held. `scrub::live` holds the state both sides share and knows
nothing of libmpv: a call into libmpv under its lock cannot be written. While
the window is hidden no ui runs and nothing is painted; `App::logic` then also
serves libmpv's render context, or the playing would stop at the next cut.

libmpv 0.40 and 0.41 ask OpenGL for a sync object after each picture drawn
through the render API and never free it (fixed in 0.42). NOH gives libmpv the
OpenGL functions by name, so it gives it a `glFenceSync` that makes none, and
turns off the decoder's direct rendering (`vd-lavc-dr`), the only other part of
libmpv that needs real ones.

`assets/ui-tokens.json` records the tested design tokens. Seven translation
catalogs retain matching keys and placeholders.

## Media contracts

Compatible H.264 pictures can be copied through central loop intervals.
Incompatible sources, image intervals, fades and composition can require
encoding. PTS, DTS, frame-grid checks and partial-fade joins have distinct roles;
output duration alone does not prove picture preservation.

Loop preparation keeps the global format and timing decisions, but prepares
only the first-cycle prefix needed by the soundtrack and reuses equivalent
preparations within an operation. Encoded video, images and PCM can be bounded
to the required duration. A terminal compressed-video copy can retain the
complete source for GOP/B-frame dependencies; presentation is cut later.

[PROJECT_EXPORT.md](PROJECT_EXPORT.md) describes project clocks, selected ranges
and composition bounds. [SHORT_PRESETS.md](SHORT_PRESETS.md) describes caption
safe-area guides. HDR tone mapping and a full ICC/HDR color-management contract
are not implemented.

## Build identity

Every executable embeds a source hash and build fingerprint. The source hash
covers build inputs; the fingerprint also includes compiler, target, profile,
features and a hash of relevant build options. Missing Git metadata stays
explicitly unavailable. A fingerprint is not an executable checksum or a promise
of bit-for-bit reproducible builds.

Packaging rejects mixed CLI/GUI/MCP identities and writes a relative-path file
hash manifest. Optional updater identities also bind their compiled trust and
qualification configuration; private signing keys are never application inputs.
See [UPDATES.md](UPDATES.md).

## Dependencies and platform scope

Infrastructure uses maintained libraries where practical. Native installation
and runtime relocation still require platform-specific work and verification.
See [BUILDING.md](BUILDING.md) for supported build paths and
[THIRD-PARTY.md](THIRD-PARTY.md) for exact runtime/font provenance.
Python/fontTools is used only for optional asset regeneration and macOS speech
preparation; ordinary Rust builds use the checked-in assets.
