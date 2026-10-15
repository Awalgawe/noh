# Linux candidate validation

The 0.1.4 candidate changes library ownership. It requires fresh qualification;
the 0.1.3 Ubuntu/WSLg report does not qualify it. No distribution is advertised
as qualified until its exact candidate, environment and results are reviewed.

## Build once, test the same archive

Release builds one Linux x86_64 AppDir on Ubuntu 24.04, checks its four-executable
ELF boundary and host contract, preserves notices/corresponding sources, and
seals the application and materials ZIPs with DELIVERY.json and SHA256SUMS.
The Linux candidate job then runs `tools/linux_matrix.py` against that sealed
envelope. It verifies and extracts the archive once, restores Unix modes and
mounts those same files read-only in every test case. There is no Cargo build
inside the distribution matrix. Cargo dependency and verified-download caches
are reused; compiled NOH executables are not accepted from the cache as evidence.

The [matrix contract](../assets/linux-validation-matrix.json) pins six x86_64
base image digests resolved on 2026-10-15. Installation uses each distribution's
package manager; Fedora uses RPM Fusion free and openSUSE uses Packman Essentials
for the required codecs. Test images are disposable and are never application
dependencies or redistributed runtime material. The recipe, exact prepared image
ID, creation time and installed package inventory are recorded for every case.
The local prepared image is reused only for identical base/setup inputs.

These are **identified image snapshots**, not a claim that mutable package
repositories can reconstruct identical images forever. Base digests alone do
not pin subsequent package downloads. Retain the prepared image when exact
environment replay is required; a newly prepared image needs its own results.

The bounded runner uses at most two concurrent image preparations and executes
media checks sequentially. Full logs and terminal errors are retained; a failed
or missing case fails the candidate job. Runtime containers have no network,
use an unprivileged user, and are removed at completion. The test scripts start
and reap only their own virtual display and audio services.

| Container | Virtual display | Synthetic audio |
| --- | --- | --- |
| Ubuntu 24.04 | Xvfb/X11 | PulseAudio null sink |
| Ubuntu 26.04 | Weston/Wayland | PipeWire null sink |
| Debian 13 | Xvfb/X11 | PulseAudio null sink |
| Fedora 44 | Weston/Wayland | PipeWire null sink |
| openSUSE Tumbleweed | Xvfb/X11 | PipeWire null sink |
| Arch Linux | Weston/Wayland | PipeWire null sink |

Checks include dependency/capability installation, GUI/CLI/MCP startup identity,
ALSA loaded before libmpv followed by default PCM output, actual GUI media import
and video/audio preview through NOH's existing capture mode, CLI export from `/`
without an explicit FFmpeg path, decode/no-overwrite, and real offline CPU
transcription of a synthetic spoken fixture. Preview must produce a decoded
frame and an available audio stream without a warning; an application mock
state cannot pass. GUI capture, service logs and per-case reports are retained.
This establishes software rendering and synthetic audio, not physical devices
or human listening. No personal screen content is captured.

Run the same runner locally on Linux with Docker (or `--engine podman`):

```sh
espeak-ng -v en-us -s 140 -w /tmp/noh-synthetic.wav 'This is a synthetic speech sample. The video and audio tools work together.'
python3 tools/linux_matrix.py --envelope /path/to/sealed-assets \
  --speech-fixture /tmp/noh-synthetic.wav --output /path/to/new-evidence-folder
```

`--prepare-only` prepares and identifies test environments without claiming any
application check. `--cases` narrows diagnosis; a subset cannot satisfy the full
qualification gate. Output folders are never overwritten.

## Desktop integration is a separate requirement

The matrix contract also names seven real desktop cases, including Mint 22.3
with Cinnamon. Ubuntu success cannot stand in for Mint. GNOME/KDE/Cinnamon,
Wayland/X11 and PipeWire/PulseAudio are represented without testing a Cartesian
product. Use a VM or physical machine with the named desktop; a container with
Weston or Xvfb is not that desktop. Record the OS image/snapshot, package
inventory, desktop/session/audio versions, GPU/renderer and audio device,
machine kind, time, reviewer, exact DELIVERY.json and four executable hashes.

On each case test a fresh extraction and prerequisites, launch, file chooser and
media import, video/audio preview, export/no-overwrite and transcription. Retain
observations and diagnostic/capture evidence. Review real playback and audible
output on the stated device. Record cancellation and ordinary desktop opening
of output files; retain hardware limits rather than generalizing to other GPUs.

Desktop evidence uses schema version 1, scope `desktop-integration`, status
`passed`, the exact `delivery_sha256` and `matrix_sha256`, and a `cases` object
keyed by every `required_desktops` ID. Each case records `status`, `elf_sha256`,
an `environment` containing the matrix fields plus `machine_kind` (`physical`
or `virtual-machine`), `os_image_identity`, `gpu`, `audio_device`, `tested_at`
and `reviewer`; `evidence` lists retained observation files with repository-relative
`path` and exact `sha256` (both are verified). Its `checks`
map must mark installation, startup, import, video_audio_preview, export and
transcription as passed. These are reviewed observations, not an automatic
attestation that an untested machine works.

## Release gate

The candidate job uploads matrix evidence separately as
`linux-validation-<run>-<attempt>`. Draft verification requires its exact
run/attempt artifact and a successful matrix step. Qualification additionally
requires independently reviewed `linux_runtime_matrix` and
`linux_desktop_matrix` evidence bound to DELIVERY.json and the complete matrix.
Missing distributions, different bytes, incomplete functional checks, or a
container presented as a desktop are rejected. Historical manifest verification
remains available; it does not authorize publishing a new host-runtime package.

Public upload still requires the existing exact-input redistribution review,
and final qualification requires the Critic's exact-artifact approval. The
workflow changes alone authorize neither pushing code nor publishing a release.
