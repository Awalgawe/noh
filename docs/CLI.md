# Command-line guide

In a Windows portable, use `bin/noh-cli.exe`; a Cargo build produces `noh`.
Run `noh --help` for all options. Paths below are examples. Select FFmpeg with
`--ffmpeg <path>` or `NOH_FFMPEG` when needed.

## A soundtrack and an ordered sequence

```sh
noh first.mp4 song.wav --video second.mp4 --output video.mp4
noh first.mp4 song.wav --fade-in 1 --fade-out 2 --partial-fades --output video.mp4
noh --soundtrack song.wav --image photo.png 2.5 --video clip.mp4 --output video.mp4
```

The soundtrack is a WAV, MP3, M4A, FLAC or OGG song; a song that is not a WAV is
decoded once into a WAV that every step reads. The sequence repeats to the song's
duration. Named media arguments retain order.
Each `--image PATH SECONDS` sets that occurrence's duration. PNG/JPEG images fit
without cropping; transparent areas and padding become black. Animated PNG is
not supported. The first video sets dimensions and frame rate; image-only
projects use the first image's even dimensions at 25 fps.

Original clip audio is off; `--clip-audio` enables mixing. `--reencode` forces
encoding, while `--partial-fades` can preserve compatible central video pictures.
`--preview` produces a reduced-resolution output. Captions and composed shorts
require encoding. Conversion does not apply HDR tone mapping.

Existing files are never overwritten. The default filename ends in `_noh.mp4`.
Ctrl+C requests cancellation and cleanup of the owned process tree.

## Inspect before exporting

```sh
noh clip.mp4 song.wav --diagnose
noh clip.mp4 song.wav --diagnose=quick --json
```

Exact diagnosis checks source timing and copy/conversion decisions without
rendering. Quick diagnosis reads metadata and cannot approve compressed copying.
Export repeats validation. Keep sources stable during work; metadata stamps are
not locked or content-hashed snapshots.

## A short from the current composition

```sh
noh --project --soundtrack song.wav --video A.mp4 --video B.mp4 --start-ms 13000 --end-ms 18000 --output short.mp4
noh --project --soundtrack song.wav --video A.mp4 --start-ms 13000 --end-ms 18000 --restart-loops --srt reviewed.srt --output short.mp4
```

The selected soundtrack interval is retained. `--restart-loops` starts the visual
sequence again at that point; otherwise its original phase is preserved.
Omit the start/end pair to render the full composition. See the
[project contract](PROJECT_EXPORT.md).

To take a short from an already rendered video instead:

```sh
noh --short video.mp4 --start-ms 500 --end-ms 3500 --framing pad --output short.mp4
```

Shorts use 1080 × 1920. `pad` preserves the complete picture with black margins;
`crop` fills the frame with a centered crop. Source cadence is retained. Output
bounds can differ from a selected millisecond boundary by source-frame timing;
reported warnings describe that difference.

## Reviewed captions

```sh
noh --burn-subtitles video.mp4 --srt reviewed.srt --output captioned.mp4
noh --burn-subtitles video.mp4 --srt reviewed.srt --caption-placement top --caption-size large --preview --output preview.mp4
```

The renderer accepts plain UTF-8 SRT, uses bundled fonts and reports text that
cannot fit. Styles/positioning from HTML or ASS are not supported. Video is
re-encoded; audio handling and timing warnings remain explicit.
Use `--caption-safe-area youtube_shorts|tiktok|reels|universal|none` to apply the
[short caption guides](SHORT_PRESETS.md).

### Promote NOH

`--promote` (with `--project`, `--short` or `--burn-subtitles`) adds a small
"made with NOH" signature in a corner of the picture, inside its safe area (a
vertical picture without one keeps it inside the `universal` area):
`--promote-corner auto|top_left|top_right|bottom_left|bottom_right`, where
`auto` (the default) is the bottom right, or the top left on a vertical
picture. `--promote-color '#RRGGBB'` sets its colour (white by default; its
opacity and stroke stay). Captions on the signature's side keep clear of it. It is off by default. A project with no captions
and no short is then encoded instead of copied. If this FFmpeg cannot draw the
signature, the export is made again without it and a warning says so; the same
goes for a project with no captions and no short that the signature's
composition cannot take (more than two hours, or too fast or too large a
picture), which is then exported as before, copied. A scene short
(`--framing scene`) is exported without the signature, with a warning.

### Caption effects

Captions (burn, short, project) are drawn in Classique unless `--effect` names
another caption effect:

```sh
noh --list-effects
noh --burn-subtitles video.mp4 --srt reviewed.srt --effect noh.drop --effect-value font=system:Arial --output captioned.mp4
```

`--list-effects` (`--list-effects=json` for JSON) shows the folders read (the
`effects` folder beside `noh.exe`, then the user's `noh/effects`), each effect
with its presets and settings, and the folders refused with their reason. The CLI
takes an id only, never a folder. `--effect-preset NAME` picks a preset;
`--effect-value KEY=VALUE` (repeatable) sets one setting, read by its declared
type: a number (one decimal comma accepted), `true`/`false`, a palette as
comma-separated colors, a font as a bundled face or `system:<installed family>`,
a landscape/portrait pair as JSON. The id, the preset and every value are checked
before the job starts; a wrong one stops the command with what was found
(`error.effect`), nothing is corrected.

`--caption-size` and `--caption-placement` are Classique's: with another effect
they are refused. `--effect noh.classic` with values sets the same style (a
setting given both ways is refused). An effect other than Classique needs OpenGL:
without it the job fails (`error.effect_gl_unavailable`) and nothing is
published; it cannot be drawn by a project `--preview` (`error.effect_preview`;
burn and short previews draw it). An installed family missing on this computer
is drawn in the effect's own font, with the warning
`warning.effect_font_missing` naming the family.

Captions wrap between words only, with Classique and every effect, on the GPU and
with libass: a word wider than the text area is never cut between letters. It
stays whole on its own line, centred and wider than the area, and the command
says so with `warning.effect_word_too_wide` and the time it first shows. When the
word also leaves the safe area (any effect) or the picture (Classique only), that
warning comes as well.

A caption effect that draws a great deal in one frame (letters, surface or
blurs) is drawn all the same: the command says so with `warning.effect_heavy`
and the time it first happens, because the export may be slow.

### Motion scenes (experimental)

In a build with the `motion` feature, a scene (a script file, see
`docs/MOTION_AUTHORING.md`) can be checked, looked at and drawn:

```sh
noh --scene-check video.mp4 --scene song.noh-motion.rhai --srt reviewed.srt --output check
noh --scene-look video.mp4 --scene song.noh-motion.rhai --srt reviewed.srt --output frames --look strip --at-ms 12000 --grid
noh --burn-subtitles video.mp4 --srt reviewed.srt --scene song.noh-motion.rhai --output with-scene.mp4
noh --burn-subtitles video.mp4 --srt reviewed.srt --scene song.noh-motion.rhai --from-ms 12000 --to-ms 20000 --preview --output clip.mp4
noh --short video.mp4 --start-ms 12000 --end-ms 42000 --framing scene --scene song.noh-motion.rhai --srt reviewed.srt --caption-safe-area youtube_shorts --output short.mp4
```

`--scene-check` writes `report.json` into the new `--output` folder; a refused
scene stops with `error.scene_refused`, the instant and the reason, and nothing is
published. `--scene-look` checks the same way, then writes PNGs and `look.json`:
`--look stills` (default) at `--at-ms` (comma-separated), `strip` (eight frames
over the second at `--at-ms`), `sheet` (`--count` frames over `--from-ms`/`--to-ms`
or the whole video), `measure` (each text's box, JSON). `--grid` draws the frame's
edges, a line every tenth and the safe area; `--bare` shows the video alone. With
`--burn-subtitles`, `--scene` draws the scene instead of the captions (refused with
`--effect`, `--caption-size` or `--caption-placement`), with OpenGL only; with
`--from-ms` and `--to-ms` only that clip of the video is rendered, the scene keeping
the video's clock. `--vertical` draws the scene on a vertical Short (1080 x 1920)
made from the whole picture by its `video()` (covering and centred without one);
`--short --framing scene --scene` exports that Short for `--start-ms`..`--end-ms`. A
vertical frame needs `--caption-safe-area`.

### The lyrics file beside the subtitles

`--srt` (burn, short and project) accepts an `.srt` or its NOH lyrics file
`<name>.noh-lyrics.json`, which holds the lyrics with their word times; the `.srt`
mirrors it. Before the job starts the two are kept identical, as in the app:

- an `.srt` given alone gets its lyrics file created beside it (never over an
  existing file; a folder that cannot be written goes on from the `.srt`, said on
  stderr);
- a lyrics file given uses the `.srt` beside it, rebuilt from it when missing;
- when the two differ, the answer is required and unifies them:
  `--truth lyrics` rewrites the `.srt`, `--truth srt` replaces the lyrics (lines
  the `.srt` keeps unchanged keep their word times; a line keeps its voices when
  the `.srt` has a line with the same times, or else with the same text, and
  stderr counts the lines that lose them). The replaced file is first
  kept beside it (`<name>.replaced.srt` or `<name>.noh-lyrics.replaced.json`, the
  next free name), and stderr names it. In a terminal (stdin and stderr) NOH asks;
  otherwise, or with `--json`, a missing `--truth` stops the command before
  anything is written, with `error.lyrics_truth_required` (with `--json`: one
  `{"version":1,"error":{…}}` line whose `technical` gives both paths, the number
  of differing lines and the accepted values);
- a lyrics file NOH cannot use (damaged, from a newer NOH, locked) leaves the
  `.srt` in use, said on stderr.

The files are unified before the media is checked: a job that then fails (for
example subtitles longer than the video) leaves them unified, with the replaced
file beside. When the `.srt` cannot be read as SRT, only `--truth lyrics` is
accepted.

The lyrics file can also say who sings each line. Its top-level `voices` lists
the declared voices (at most four, each with an optional `name` of at most 32
characters), and a cue's `voices` is the set of voice numbers that sing it,
sorted (`[1]`, `[2]`, `[1, 2]` for both together; absent: not known). Words
between parentheses in a cue's text are backing vocals; a word's known `end_ms`
may then run past the backing vocals that follow it. A file holding voices, or
such an end, is written as version 3, which a NOH that only knows version 2
treats as newer and never writes; every other lyrics file stays version 2. The
`.srt` holds no voices.

## Local transcription

```powershell
noh --transcribe speech.wav --output reviewed.srt `
  --transcriber C:\tools\whisper-cli.exe --model C:\models\ggml-small.bin `
  --vad-model C:\models\ggml-silero-v6.2.0.bin --language en
```

Supply the local runtime and model paths; the CLI downloads nothing. Audio can
come from a WAV or the first audio stream of a video. Review recognized words
and timings, especially for songs. Language defaults to automatic detection;
an explicit code selects recognition language, not translation. Sources are
limited to two hours. An empty transcript is valid when no speech is detected.
The VAD path remains part of the request contract; generation uses the complete
audio clock rather than cutting sung passages with VAD.

## Automation and troubleshooting

`--json` requests machine-readable results; `--events` records progress events.
Only a terminal result establishes completion. CLI warnings go to stderr.
`--version` and `--build-info` identify a build without starting a media job.
The [local MCP server](MCP.md) exposes the same media core with job waiting and
cancellation. See [architecture](ARCHITECTURE.md) for the shared controller.
