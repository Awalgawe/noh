# Project composition and timeline contracts

Shared implementation for the single-screen desktop, CLI and local MCP server.
This document describes the added request; the existing export, transcription,
caption and file-based short requests retain their meanings.

## Selected project output

`project::ProjectRequest` contains the ordinary `ExportRequest` as `montage`, an
optional `ProjectShort`, and optional reviewed `ShortCaptions`. A missing short
means the full montage. Missing captions means no text is burned. Generation and
import do not implicitly enable burning. `Job::project` owns the same cancellation,
input identity checks, private workspace and exclusive publication as other jobs.

CLI example (paths are local; flag order retains media order):

```powershell
noh --project --soundtrack voice.wav --video A.mp4 --video B.mp4 `
  --start-ms 13000 --end-ms 18000 --output short.mp4 `
  --restart-loops --srt reviewed.srt
```

Omit `--restart-loops` to retain the montage's visual phase. Omit the start/end
pair for the full montage. Use `--preview` for reduced resolution and `--framing
crop` for explicit centered cropping of a short. Padding is the default.

MCP adds `noh_export_project` with `request` (the existing render-input object),
optional `short: {start_ms, end_ms, restart_loops, framing}`, optional
`captions: {subtitles, size, placement}`, and `preview`. It returns a job ID for
the same `noh_job`/`noh_cancel` lifecycle. JSON fields, paths and intervals are
validated; no diagnosis token or user-managed intermediate is required. The
existing `noh_make_short` still consumes an already rendered source video.

## Clocks, boundaries and fades

The WAV defines the project clock and duration. A short is the half-open interval
`[start_ms, end_ms)`. At local output time `u`, WAV time is always `start + u`.
Visual time is `(start + u) mod loop_duration`, or `u mod loop_duration` when
restart is enabled. Restart begins at the first ordered item, including images.
Original clip audio follows the chosen pictures; it never moves the WAV range.
Mixing retains the existing equal-input normalization.

SRT input is validated against the whole WAV duration. Cues are intersected with
the short range, clipped at its edges and shifted by subtracting its start.
Framing precedes text rendering, so captions are neither cropped nor burned twice.
Visual fades keep their original project/WAV times, including partially elapsed
fades. An interior short boundary does not introduce a new fade. Restart does not
restart these fades or the soundtrack.

Pictures follow the montage's frame grid; millisecond selection does not create
new source frames between existing frames. Actual output bounds are verified
before publication, and reported boundary/AAC padding warnings remain visible.
Audio codec padding is distinct from the requested WAV interval.
Piece normalization can place the first selected picture at output zero with a
residual smaller than one frame; WAV and SRT selection retains millisecond times.

## Work and resource bounds

Plain full montage export still delegates to the existing engine and preserves
its partial-copy behavior. Composed output prepares only contributing visual
pieces in private lossless FFV1 files, reuses repeated pieces and performs one
final lossy H.264 encode. It does not render the complete uncaptioned project as
a prerequisite for a short. Large selected pieces can require substantial
temporary disk space. Failure/cancellation removes the owned workspace.

Composed output requires MP4, a positive WAV of at most two hours, no more than
200,000 planned pieces, at most 240 fps, 16,384 pixels per dimension and 64 million
pixels per source canvas. Existing supported SDR/orientation and subtitle glyph/
layout limits apply. Unsupported inputs fail explicitly.

## Waveform and desktop state

`waveform::WaveformWorker` coalesces requests, checks WAV/FFmpeg identities off the
UI thread and retains one cache entry. FFmpeg emits native-rate float channels;
reduction takes the maximum absolute channel magnitude, so opposite-phase stereo
cannot cancel itself out. Stream buffers and diagnostics are bounded. The
amplitude pyramid contains at most 262,144 base bins and less than 2 MiB across
its levels; this does not bound FFmpeg's own process memory. Finest precision is
reported and becomes coarser for long sources. Queries reuse the pyramid without
decoding; display columns are capped at 16,384. Cancellation and a 15-minute total
deadline terminate the owned decoder tree. Waveform failure never prevents export.

The shared desktop state retains this waveform and the attached track when media
order, image durations or the short selection change. WAV/FFmpeg metadata refresh
discards obsolete replies and revalidates subtitles against the current WAV.
SRT reads are bounded to one MiB, occur off-thread, and cannot revive a removed
track. Range dragging preserves its initial grab offset; whole-range movement
clamps without shrinking and handles cannot cross. Viewport/timecode operations
are pure; the desktop limits zoom to a two-second window on sufficiently long projects.

File identities follow the existing metadata-stamp contract. Changes preserving
all recorded metadata cannot be detected by this mechanism; it is not a file lock
or content-hash promise. Every export repeats its own source checks.
