# Local MCP server

Caption requests and short/project `captions` accept an optional `safe_area`:
`none` (default), `youtube_shorts`, `tiktok`, `reels`, or `universal`.
See [Short presets](SHORT_PRESETS.md) for the shared protected text regions.

The implementation uses the official Rust SDK behind the
optional `mcp` feature and the same isolated job controller as the CLI and GUI.
It exposes local jobs over stdio; client compatibility should be checked with
the chosen client and version.

## Build and start

```sh
cargo build --locked --features mcp --bin noh-mcp
target/debug/noh-mcp --help
```

Use `.exe` paths on Windows. A fresh build fetches Cargo dependencies; use
`--offline` after they are cached. No Rust installation is needed to run the
compiled server. See [BUILDING.md](BUILDING.md) for portable builds.

`noh-mcp --version` prints the shared build label; `--build-info` prints the
version-1 build JSON and exits without starting stdio or finding FFmpeg. During
normal protocol negotiation, `serverInfo.version` contains the same compact
identity. These diagnostics do not change the version-1 media/job contracts.

Start `noh-mcp` with stdio connected to the harness. `--ffmpeg <absolute-path>`
or `NOH_FFMPEG` selects FFmpeg; ordinary application discovery remains available.
Individual requests can override the engine with an absolute `ffmpeg` path.
Protocol discovery works even when no default engine is available; media work
then reports the corresponding engine error. stdout contains JSON-RPC only.
There is no HTTP listener, upload or remote job store. No tool opens a player
or a window, except `noh_app_open`, which opens on request a NOH window driven
by the agent: a child of the server, reached only through its standard pipes.

To build a Windows portable including all three entry points:

```powershell
cargo --offline dev build --gui --mcp --offline --ffmpeg dist/noh/bin/ffmpeg.exe
cargo --offline dev verify --media --mcp --offline
```

## Tools and workflow

| Tool | Required arguments | Result |
| --- | --- | --- |
| `noh_inspect` | `path`, `kind` (`video`, `wav` or `image`) | Metadata job ID |
| `noh_diagnose` | `request`; optional `level` (`exact` by default, or `quick`) | Diagnosis job ID |
| `noh_export` | `request`, `diagnosis_job_id` | Checked export job ID |
| `noh_preview` | `request` | Preview job ID; full montage at reduced quality |
| `noh_export_project` | `request`; optional `short`, `captions`, `preview` | Full montage or selected project interval using the current WAV/media/SRT |
| `noh_transcribe` | `source`, `output`, `transcriber`, `model`, `vad_model`; optional `language` and `ffmpeg` | Local transcription job ID |
| `noh_burn_subtitles` | `source`, `subtitles`, `output`; optional `ffmpeg`, `size`, `placement`, `effect`, `preview` | Caption render job ID; result is an exported MP4 |
| `noh_make_short` | `source`, `output`, `start_ms`, `end_ms`; optional `ffmpeg`, `framing`, `captions`, `preview` | Vertical clip job ID; result includes the measured duration and exported MP4 |
| `noh_job` | `job_id` | Current status, optionally waiting for a change and returning the result |
| `noh_cancel` | `job_id` | Cancellation request/current status |
| `noh_list_effects` | none | The caption effects `effect.id` can name, their presets and settings, the folders read and refused |
| `noh_app_open` | `title` | A NOH window the agent drives: `app_id`, its state and every window of this session |
| `noh_app_close` | `app_id` | The window closed, or why nothing was closed |

`noh_transcribe` processes an explicitly selected local audio file or the first
audio stream in a selected video using local CPU transcription. All paths are
absolute; the optional `ffmpeg` uses the session default when omitted. The
Whisper.cpp executable, transcription
model and Silero VAD model are required explicit local paths. No executable or
model is downloaded automatically. VAD is required to reduce silence transcription.
The spoken-language `language` defaults to `auto`; an explicit value is 2–3
lowercase ASCII letters (for example, `fr`). It selects the spoken language and
does not request translation. Source audio is limited to two hours. The output
must be a free `.srt` path, and publication never overwrites an existing file.
Recognized text and timings should be reviewed before use. A valid empty track is
returned when no speech is detected.

`noh_burn_subtitles` accepts an explicit source video, reviewed plain UTF-8 SRT
and free MP4 destination, all absolute local paths. `size` is `small`, `medium`
(default) or `large`; `placement` is `bottom` (default) or `top`; `preview` defaults
to false. It shares the controller's validation, cancellation and publication,
and returns `kind: "export"` through `noh_job`. No montage diagnosis is needed
for this separate operation. Video is re-encoded; audio treatment, timestamp
rounding and unknown final-frame duration are reported as stable warning IDs.
The source is never modified. SRT input is capped at 1 MiB and source
duration at two hours. Unsupported color/rotation cases fail explicitly.

Caption effects (`noh_burn_subtitles`, and `captions` in `noh_make_short` and
`noh_export_project`): captions are drawn in Classique unless `effect`
(`{"id": …, "preset": …, "values": {…}}`) names another effect from
`noh_list_effects`. The id is looked up in the folders NOH reads (the `effects`
folder beside `noh.exe`, then the user's `noh/effects`), never given as a path.
The preset and each value are checked against the effect before the job starts;
a wrong one is refused (`error.effect`) with what was found, nothing is
corrected. A font setting takes a bundled face or `"system:<installed family>"`.
`size` and `placement` are Classique's (absent: `medium`, `bottom`); with another
effect they are refused, and with `noh.classic` a setting given both as a field
and as a value is refused. An effect other than Classique needs OpenGL (no libass
fallback: `error.effect_gl_unavailable`, nothing published) and is refused with a
project `preview` (`error.effect_preview`). An installed family missing on this
computer is drawn in the effect's own font; the job's warnings then hold
`{"code": "warning.effect_font_missing", "args": [family]}`.
Captions wrap between words only (every effect, GPU and libass): a word wider
than the text area stays whole on its own line, centred, and the job warns with
`{"code": "warning.effect_word_too_wide", "args": ["m:ss"]}`; the overflow
warning may come too when the word also leaves the safe area (any effect), and
the off-frame warning when it leaves the picture (Classique only).
When libass draws the captions (Classique without OpenGL, or no effects
engine) in a BT.709 export and the FFmpeg in use blends them in BT.601 (FFmpeg
older than 7.1, such as Ubuntu 24.04's 6.1), the job warns with
`warning.caption_colors_old_ffmpeg`: the colours may be slightly off; nothing is
corrected. NOH measures this by drawing one red box with that FFmpeg.

Subtitles and their lyrics file (`noh_burn_subtitles`, and `captions` in
`noh_make_short` and `noh_export_project`): `subtitles` is an `.srt` or its NOH
lyrics file `<name>.noh-lyrics.json` (the lyrics with their word times; the
`.srt` mirrors it). Before the job starts NOH keeps the two identical: an `.srt`
given alone gets its lyrics file created beside it (never over an existing file),
a lyrics file given uses the `.srt` beside it (rebuilt when missing), and a lyrics
file NOH cannot use leaves the `.srt` in use. When the two differ, `truth` is
required: `"lyrics"` rewrites the `.srt`, `"srt"` replaces the lyrics (unchanged
lines keep their word times); the replaced file is first kept beside it under the
next free `<name>.replaced.srt` or `<name>.noh-lyrics.replaced.json`. Without
`truth` the start is refused before anything is written, with code
`error.lyrics_truth_required`; its `technical` is a JSON object with `srt`,
`lyrics`, `differing_lines`, `srt_usable` and `accepted` (`["lyrics","srt"]`,
or `["lyrics"]` when the `.srt` cannot be read). The files are unified before the
media is checked: a job that then fails leaves them unified, with the replaced
file beside.
`truth` is ignored when the two do not differ. What NOH did (a file created, an
`.srt` rewritten, the replaced file kept as …) is listed in the start reply's
`lyrics` array of `{code, args}`.

`noh_export_project` is the additive project-aware path. Its `request` uses the
existing render-input object; optional `short` contains `start_ms`, `end_ms`,
`restart_loops` and `framing`; optional `captions` contains the reviewed SRT path,
size and placement. A missing short exports the full montage, and captions are
never burned unless explicitly supplied. The WAV clock remains fixed to the selected
interval; restart changes only the visual loop phase. Cue times are clipped to the
short and shifted to its start. It returns the same job lifecycle and exclusive
publication behavior. See [PROJECT_EXPORT.md](PROJECT_EXPORT.md) for clocks, fades
and frame-boundary details.

`noh_make_short` uses source playback milliseconds and a half-open interval.
`framing` is `pad` (default) or centered `crop`. Optional `captions` is
`{"subtitles":"C:/media/reviewed.srt","size":"medium","placement":"bottom"}`
with source-timed SRT cues; style defaults match caption rendering. Full output is
1080x1920 and `preview: true` produces the same composition at 360x640. It shares
job cancellation, identity checks and no-overwrite publication, returning
`kind: "export"`. Read job warnings for re-encoding, omitted streams and frame
boundaries. Sources are limited to two hours and SRT input to 1 MiB. Millisecond
selection does not create new pictures between source frames. Protocol tests
do not establish compatibility with every external client.

Input objects reject unknown properties. Every media path is a nonempty absolute
local path. A render request has this shape:

```json
{
  "videos": ["C:/media/a.mp4", "C:/media/b.webm"],
  "wav": "C:/media/soundtrack.wav",
  "output": "C:/media/montage.mp4",
  "fade_in": 1.0,
  "fade_out": 2.0,
  "partial_fades": true,
  "clip_audio": false,
  "force_encode": false
}
```

`wav`, `output` and exactly one of `videos` or `items` are required. The optional `ffmpeg` is absolute.
Defaults: zero fades, partial fades enabled, clip audio and forced conversion
disabled. Up to 4096 ordered clips are accepted. Fade durations must be finite
and nonnegative. The server selects preview mode; clients cannot supply it.
Preview destinations must be MP4. Publication never replaces an existing file.

For images or mixed sources, replace `videos` with ordered tagged entries:

```json
"items": [
  {"kind": "image", "path": "C:/media/photo.png", "duration": 2.5},
  {"kind": "video", "path": "C:/media/clip.mp4"},
  {"kind": "image", "path": "C:/media/photo.png", "duration": 1.0}
]
```

Image duration is finite and positive, and belongs to the occurrence. PNG/JPEG
images share the engine's frame rounding, orientation, black alpha compositing
and aspect-preserving framing; see [CLI.md](CLI.md). Image metadata has no intrinsic
duration; the rendering request provides it. Images require MP4 output. Changing
an image duration invalidates a previous exact diagnosis approval. Unknown item
properties, null lists and simultaneous videos/items are rejected. Existing
video-only requests/replies retain their version-1 JSON shape.

Start with inspection or exact diagnosis. The start response contains
`version`, `job_id`, `state` and `revision`. Call `noh_job` with
`after_revision` and `wait_ms` (0–30000) to wait for a change without frequent
polling. Request `include_result: true` for complete terminal output. An unchanged
cursor returns `changed: false` with only identity/state/revision; omit the
cursor to fetch a retained result again.

Exact diagnosis describes output target, source copy/conversion decisions and
reasons. Use its completed job ID for export. The server retains the authoritative
diagnosis; clients do not send an edited plan back. A rename or folder change
within the same container can reuse it. Render-setting, media or engine changes
invalidate it; checked export repeats the core validation. Quick diagnosis
provides metadata only and cannot approve an export.

Job states are `running`, `cancelling`, `succeeded`, `failed` and `cancelled`.
Progress is coalesced and monotonic; 100% follows publication. Results are tagged
`metadata`, `diagnosis`, `export` or `subtitles`. Metadata/diagnosis embed the
existing version-1 engine JSON objects; export reports path and duration. A
subtitle result includes the published `output` path and validated `track` with
`language`, `duration_ms` and `cues`; each cue has `start_ms`, `end_ms` and `text`.
No source media bytes are returned. Responses provide both
`structuredContent` and equivalent JSON text. Terminal engine failures retain
`code`, `operation`, `path`, `detail` and bounded `technical` diagnostics and use
MCP's `isError`. Adapter failures include busy/unknown/expired/oversize errors.

Transcription uses the same one-active-job limit and `noh_job`/`noh_cancel`
lifecycle as other work. Cancellation is cooperative and terminal only when the
owned process tree has stopped; a committed output remains successful. Empty
tracks are successful results, not failures.

## Session and lifetime rules

One job may run at a time per server process; further starts return a busy error.
There is no persistent queue or database. IDs belong to this session and cannot
be transferred to a newly started server. At most 16 terminal records and 32 MiB
of retained result data are kept; old records expire explicitly. Media bytes
are never returned. Input frames are limited to 4 MiB and complete output
envelopes to 8 MiB. Oversize results fail explicitly rather than truncating a plan.

Cancellation is confirmed only by a terminal `cancelled` result. If publication
has already committed, success and the output remain. EOF or broken protocol
pipes close admission, cancel active work and wait for the owned controller's
cleanup. Published outputs and unrelated files are preserved.

An installed Windows copy that installs an update asks its servers to stop (see
[UPDATES.md](UPDATES.md)). From then on a server refuses new jobs with
`mcp.updating`. It exits once no job runs and the last one ended at least 15 s
ago, and never in the middle of a job. Its job IDs end with it. A client that
does not restart stdio servers by itself (interactive Claude Code) needs to
reconnect it after the update.

## Driven windows

`noh_app_open` starts the NOH application installed beside the server
(`noh.exe` in the Windows bundle, `noh-app` elsewhere; never from PATH) with
`--driven`. The window and the server talk over the window's standard input
and output, one JSON message per line; nothing listens and no other program
can reach it. The person sees the window and the agent's badge, can play,
pause and move through the video, and cannot edit: they ask the agent. A click
on a clip or a lyrics line designates it. **Stop driving** in the badge, or
closing the window, gives the window back to the person as an ordinary NOH;
the agent can then no longer act in it (`app.released_by_user`, `app.closed`).

A window opens only if the machine has room for it. Free memory, and on
Windows free graphics memory, are measured at each open; each window already
open keeps a margin up to its budget (550 MiB of memory, 250 MiB of graphics
memory), and 2 GiB or 15 % of the memory (1 GiB of graphics memory) stays free
for other programs. At most 8 windows per server, 4 when graphics memory cannot
be read. A refusal (`app.cap_reached`) gives the figures. Windows the person took
back or closed by hand still count while they run: they are still this server's
children.

`noh_app_close` writes pending lyrics changes first and never discards
anything: with a question open in the window, or one the write needs (the first
change of an .srt), nothing closes and `app.question_open` names it; a job
running refuses the close (`app.job_running`). When the session ends the
server tells its windows and closes their pipes; the windows stay open as
ordinary NOH windows, theirs to close.

## Client configuration

Configure a stdio server in your MCP client with an absolute executable path.
For example, clients using a `mcpServers` JSON configuration commonly accept:

```json
{
  "mcpServers": {
    "noh": {
      "command": "C:/tools/noh/bin/noh-mcp.exe",
      "args": ["--ffmpeg", "C:/tools/noh/bin/ffmpeg.exe"]
    }
  }
}
```

Adapt this to your client's configuration format. The server inherits that
client's access permissions. Media paths refer to local files; there is no
HTTP endpoint, upload service or remote media storage.
