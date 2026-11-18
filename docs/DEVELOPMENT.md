# Development

Read the [build guide](BUILDING.md) for dependencies and the
[architecture](ARCHITECTURE.md) for component boundaries. Repository code,
comments, tools and documentation use English; the desktop is localized.

## Build and test

```sh
cargo check --locked --features gui --bin noh-app
cargo dev verify
cargo dev verify --media --mcp
```

Media checks need FFmpeg, supplied through `NOH_FFMPEG` if necessary. Add
`--offline` only with cached dependencies. Focused changes can use a matching
`cargo test` filter; use the complete media/MCP run for integration. Tests generate
synthetic fixtures and must not depend on personal media or machine paths.

The `cargo dev` helper uses the optimized QA profile. Its `build` subcommand
still produces release application binaries. For UI development:

```sh
cargo build --locked --profile qa --features gui --bin noh-app
```

Use one target directory and avoid concurrent heavy builds in the same checkout.
A running Windows portable must be closed before packaging replaces it. Missing
resources should fail explicitly rather than trigger unrecorded downloads or
fallback behavior.

For optional updater verification, build the matching GUI/CLI/MCP binaries with
`gui,mcp,updates`, then run `cargo dev verify --media --mcp --updates`.
Native installation qualification is separate; see [UPDATE_RECOVERY.md](UPDATE_RECOVERY.md).

## Media and transcription checks

The media suite covers copying, selective conversion, fades, frame timing,
geometry, cancellation and no-overwrite publication. It does not establish a
complete HDR/color contract or support for every native distribution.
Opt-in real transcription additionally needs local runtime/model paths and a
speech fixture:

```powershell
$env:NOH_WHISPER = 'C:\tools\whisper-cli.exe'
$env:NOH_WHISPER_MODEL = 'C:\models\ggml-small.bin'
$env:NOH_WHISPER_VAD = 'C:\models\ggml-silero-v6.2.0.bin'
$env:NOH_SUBTITLE_SPEECH_FR = 'C:\fixtures\speech-fr.wav'
cargo dev verify --media --mcp --transcription
```

Do not count an explicitly skipped media/transcription scenario as a pass.
Keep measurements tied to the exact inputs, build and machine; configuration
alone does not establish a performance improvement.

## UI captures

The app can capture its own real rendering and exit. Set `NOH_CAPTURE_UI` to a
new `.ppm` path, `NOH_CAPTURE_PROJECT` to a project request JSON, and optionally
`NOH_LANGUAGE`, `NOH_CAPTURE_WIDTH`, `NOH_CAPTURE_HEIGHT`, `NOH_CAPTURE_SCALE` and
`NOH_CAPTURE_THEME`. These overrides are capture-only and do not change saved
language or theme preferences; the saved theme is not read in capture mode
(Options shows the captured theme). Inspect the adjacent report: `ready`, `capture_ready`,
`timed_out` and diagnosis/error fields determine whether real media loaded.

`NOH_CAPTURE_STATE` can stage display-only states. Such screenshots are layout
fixtures, not proof that an export or interaction succeeded.
`NOH_CAPTURE_PROJECT_ACTION` runs real operations such as export, short-export,
preview, short-preview or generate; the last needs local speech resources.

The reusable campaign runner produces PNGs, reports and a contact sheet:

```sh
cargo dev fixtures layout path/to/new-fixtures --ffmpeg path/to/ffmpeg
cargo dev capture-ui --app target/qa/noh-app --ffmpeg path/to/ffmpeg --project path/to/new-fixtures/project.json --output path/to/new-captures
```

On Windows use `.exe` paths. Filters include `--states`, `--languages`, `--themes`,
`--scales` and `--sizes`; `--matrix layout` selects the fixed layout campaign.
Use a new output directory and visually inspect screenshots as well as reports.
Screenshot success is not a human interaction test.

### The live preview

`NOH_CAPTURE_PROJECT_ACTION=live-preview` (or `live-short`) presses Play and
ends at `NOH_CAPTURE_PLAYBACK_UNTIL_MS`. On top of it, one of:

- `NOH_CAPTURE_SEEK_PLAYING=1` clicks on the time ruler while playing, at
  `NOH_CAPTURE_SEEK_TARGETS_MS`, holding the button `NOH_CAPTURE_SEEK_HOLD_MS`,
  and reports what the monitor showed after each press;
- `NOH_CAPTURE_DRAG_PLAYING=1` drags on the ruler while playing;
- `NOH_CAPTURE_TRANSPORT=1` walks through pause, frame steps, volume, the end
  of the montage, an edit and the short view.

Every report lists the player each Play went to (`engines`), with the reason
when it is not libmpv. `NOH_CAPTURE_REQUIRE_LIVE=1` waits for the light copies
before Play and fails unless libmpv played, with a sound output: without it a
capture passes just the same when the FFmpeg player played instead.
`NOH_PLAYER=mpv` or `ffmpeg` chooses the player; `NOH_CACHE_DIR` keeps the
light copies of a capture out of the user's cache.

`NOH_PROFILE=<file>` appends a journal of what the preview does: each move of
the playhead, each picture, each seek and its time, the player chosen at each
Play and why. It is for diagnosis, in a capture or in ordinary use. The file
holds the full paths of the media that were opened: read it before sending it
to anyone.

## Documentation and contributions

Keep the main README focused on the app and getting started. Detailed contracts
belong in these guides. Do not commit session reports, private media, credentials,
personal paths, local build artifacts or machine-specific diagnostic evidence.
Retain license notices and useful, reproducible technical constraints.
When changing translations, preserve every key and interpolation placeholder.
When changing packaging, verify the documentation allowlist and relative links
without rebuilding the whole runtime unless executable behavior also changes.

## Known limitations

- Windows installer and portable downloads are available on
  [GitHub Releases](https://github.com/Awalgawe/noh/releases/latest). Linux and
  macOS distributions remain unqualified; macOS signing/notarization is incomplete.
- Automatic installation of updates is disabled in ordinary builds.
- The desktop does not automatically restore a closed editing session.
- HDR tone mapping and a verified end-to-end color-management contract are absent.
- libmpv plays the live preview by default on Windows only. It has not been run
  on Linux, where the system's libmpv is loaded, nor on macOS.
- Once libmpv has given a montage up, the FFmpeg player plays until NOH is
  started again, even if the cause is gone (a sound device plugged in since).
- A window that is not redrawn while the system still reports it visible stops
  a montage played by libmpv at its next cut, sound included, until it is drawn
  again. A minimised or hidden window does not.

These limitations must inform release readiness.
[RELEASING.md](RELEASING.md) distinguishes source publication from a
supported binary release.
