# Getting started with NOH

NOH turns an ordered sequence of clips and photos into a video that follows a
song (MP3, WAV, M4A, FLAC or OGG). The sequence repeats for the length of the song. You can also
export a selected moment as a vertical short.

## Open the app

Install the Windows app from the [latest release](https://github.com/Awalgawe/noh/releases/latest),
then open **NOH** from the Start menu. With a portable copy, extract the complete
ZIP and open **noh.exe**. Choose the Standard or Complete profile for editing;
Minimal can download the media tools when you need them.

On Linux x86_64, extract the complete Linux ZIP and launch `NOH.AppDir/AppRun`.
The desktop must provide the file chooser, audio and graphics services described
in the [installation guide](INSTALLING.md#linux-x86_64). Linux 0.1.3 uses
`xdg-open` and Linux default applications to open exported media,
folders and lyrics, including under WSL. Associate `.srt` files with
a Linux text editor to review and save corrections. Upgrade older Linux
packages to obtain the corrected desktop-opening behavior.

See the [installation guide](INSTALLING.md). macOS currently requires
a [source build](BUILDING.md).

## Make your first video

1. Drop videos or PNG/JPEG photos into the window, or click **Add**. Arrange the
   items in the strip at the top. Each photo has an adjustable duration.
2. Add your **song**: MP3, WAV, M4A, FLAC or OGG. Its waveform appears below the
   monitor and determines the full video's length. Original clip audio is off by
   default. A song that is not a WAV is decoded once, when you add it, into a WAV
   kept in NOH's cache folder: the player, the lyrics and the export all hear that
   copy. Lyrics and scene files stay beside your own song file.
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
no audio is uploaded by this feature. In **Settings**, the Whisper menu on the
**Lyrics** row chooses the model; **Automatic** takes the most accurate one that
runs at a usable speed here (see [Installing](INSTALLING.md)).

Attaching or generating a track does not automatically place text on the video.
Enable **Show on the video** in the lyrics menu when you want burned-in text.
Caption placement and size can be adjusted. Text that cannot fit is reported
instead of silently cropped.

The Complete installer and portable ZIP include the speech runtime and models.
Other installed profiles can add them through **Resolve missing resources →
Add or repair components**. Source builds may require paths in
**Settings → Advanced lyrics settings**.

## A window the agent drives

An agent connected to NOH's MCP server can open NOH windows itself and work in
them while you watch. Such a window shows the agent's badge at the top and says
so in its title. You can play, pause, move through the video on the ruler, zoom
the timeline and change the volume; a click on a clip or a lyrics line points it
out to the agent, which also knows where you are on the timeline, so « here »
means your position. While you move through the video the agent does not move
the playhead. Everything else is the agent's: ask it in the conversation.
**History** in the badge lists the agent's last twenty gestures with their time
(what it added, removed, moved, changed, where it put the playhead, the exports
it started). While another window of the same agent exports or prepares, this
one waits its turn: its clips show a still mark and its preview waits
until then.
**Stop driving** in the badge, or closing the window, makes it an ordinary NOH
window again, yours. When the agent's session ends the window stays open as an
ordinary NOH. If such a window fails, it shows no error dialog (nobody may be
there to close it): the agent is told why, and `startup.log` keeps the reason.

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
In an installed Windows copy, choose **Add or repair components** to open the
download wizard. You can add content without uninstalling. After restoring files
or changing paths, use **Check again**. The check runs in
the background; other features remain available when their dependencies work.
Restoring the fast-preview library requires restarting NOH.

- **Export is unavailable:** wait for source checking to finish, or read the
  message in the bottom bar. Check the soundtrack, media paths and destination.
- **The destination already exists:** choose another name; NOH preserves it.
- **No transcription engine:** add the Complete profile from the missing-resource
  settings, use the complete portable ZIP, or configure existing tools and models
  in advanced settings.
- **A preview takes time:** let the first preparation finish. Preview working
  copies do not replace the original export sources.
- **The preview stutters, stops or sounds wrong while playing:** start NOH with
  the environment variable `NOH_PLAYER=ffmpeg`. It keeps the former player,
  which is slower to follow a click on the time ruler. Please report what you
  saw.
- **The preview starts, hesitates about a second in, then goes on:** the fast
  player found no sound output and the former player took over. It keeps
  playing until NOH is started again; connect the sound output first.

For automation, see the [CLI guide](CLI.md) and [MCP server](MCP.md). Developers
can consult the [architecture](ARCHITECTURE.md) and [known limitations](DEVELOPMENT.md#known-limitations).
