# Getting started with NOH

NOH turns an ordered sequence of clips and photos into a video that follows a
WAV soundtrack. The sequence repeats for the length of the song. You can also
export a selected moment as a vertical short.

## Open the app

Start with the [installation guide](INSTALLING.md) to extract and place a complete
portable build. Then open **noh.exe** on Windows or **NOH.app** on Mac.
Windows downloads are available from the [latest release](https://github.com/Awalgawe/noh/releases/latest).
See the [installation guide](INSTALLING.md); other platforms require a [source build](BUILDING.md).

## Make your first video

1. Drop videos or PNG/JPEG photos into the window, or click **Add**. Arrange the
   items in the strip at the top. Each photo has an adjustable duration.
2. Add a **WAV soundtrack**. Its waveform appears below the monitor and determines
   the full video's length. Original clip audio is off by default.
3. Press **Play** to see and hear the result. Use the monitor controls or the
   waveform to move through the project. **Fades** adjusts the beginning and end.
4. Choose a destination in the bottom bar, then click **Export the video** once
   the app reports that it is ready. Existing files are never overwritten.

Your original files remain unchanged. Export runs in the background and can be
cancelled. A first preview may need a little preparation; exports use originals.
Keep source files in place while a project is open or an operation is running.

## Export a short

Select a range on the waveform, or choose **Choose a short**. Adjust the start
and end, then preview that selection on the monitor. Shorts use a vertical
1080 × 1920 frame. Choose whether to fit the picture with padding or fill the
frame with a centered crop.

The short uses the selected part of the same soundtrack. You can keep the
montage's visual timing or restart the visual sequence at the selection's start.
[Short presets](SHORT_PRESETS.md) provide caption guides for common social
platform layouts; always check the destination platform's upload preview.

## Add lyrics or captions

Use **Lyrics** to attach a reviewed UTF-8 `.srt` file. With the local speech
runtime available, you can also generate a transcript from the soundtrack.
Review the words and timing, particularly for singing. Recognition is local;
no audio is uploaded by this feature.

Attaching or generating a track does not automatically place text on the video.
Enable **Show on the video** in the lyrics menu when you want burned-in text.
Caption placement and size can be adjusted. Text that cannot fit is reported
instead of silently cropped.

Complete portable builds include their speech runtime and models. Source builds
may require paths in **Settings → Advanced lyrics settings**.

## Before closing

The desktop does not automatically restore the editing session after closing.
Export the result you need and keep your original media available.

## Settings

Open the sliders button in the upper-right corner to change language, export
mode, the media engine or transcription paths. The app follows your system
language unless you choose another. Seven interface languages are included.

**Automatic** export copies compatible video where possible and converts what
needs it. **Convert everything** forces encoding. Captions, image rendering and
composed shorts require encoding. NOH does not promise lossless output for every
composition or verified HDR/color-management fidelity.

The processing log provides technical details if an operation fails.
**Build information** identifies the running version for a bug report. Automatic
installation of updates is currently disabled in ordinary builds.

## If something does not work

If NOH reports missing resources, choose **Resolve missing resources**. Options
shows which tool or model is unavailable, its expected path and repair guidance.
After restoring files or changing paths, use **Check again**. The check runs in
the background; other features remain available when their dependencies work.
Restoring the fast-preview library requires restarting NOH.

- **Export is unavailable:** wait for source checking to finish, or read the
  message in the bottom bar. Check the soundtrack, media paths and destination.
- **The destination already exists:** choose another name; NOH preserves it.
- **No transcription engine:** use a complete portable build or configure your
  local runtime and models in advanced settings.
- **A preview takes time:** let the first preparation finish. Preview working
  copies do not replace the original export sources.

For automation, see the [CLI guide](CLI.md) and [MCP server](MCP.md). Developers
can consult the [architecture](ARCHITECTURE.md) and [known limitations](DEVELOPMENT.md#known-limitations).
