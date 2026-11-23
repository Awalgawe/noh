# Changelog

User-visible changes are recorded here. Published versions and their downloads
are available on [GitHub Releases](https://github.com/Awalgawe/noh/releases).
This changelog starts with the initial public source version; earlier private
builds are not public releases.

## 0.4.0

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.4.0).

### Caption styles

- Seven new caption styles: Readable band, Highlighter, Lit word, Punched
  words, Stickers, Scrolling lyrics and Fade to dust. NOH now ships thirteen.
  Each has its own settings in the style panel.

### Caption effects

- The caption effect format gains what new styles need: a timed return when a
  word or syllable has been sung, rounded plates behind a line, word or letter
  (animated in opacity, margin, width and sweep), a soft edge on the colour
  sweep, a window of lines that glides to its new place, the scale, rotation and
  rank of a line or chunk, left or right alignment, and, for shader hooks, the
  pixel's place in the frame, the letter's number, the line's time, the pass
  being drawn, the letter's row, the time since the line was sung and the
  effect's own settings. The bundled styles draw exactly as before. The format is described
  in the [effect format guide](https://github.com/Awalgawe/noh/blob/v0.4.0/docs/EFFECTS.md).

### Lyrics

- The lyrics can say who sings each line. On the lyrics screen, **Voices**
  above the text declares two voices; you can rename them, add up to four and
  remove the last one. Each line has a badge with its voices: a click opens a
  menu to set them, Ctrl+1 to Ctrl+4 give the current line one voice,
  Ctrl+Shift+1 to Ctrl+Shift+4 add or remove one, and Ctrl+0 removes them all.
  Both tracks show a line's voices as a coloured band. No caption style draws
  the voices yet.
- Words between parentheses are backing vocals. The lyrics screen draws them
  dimmer and underlined, and gives them their own band on the track. Select
  some words and type "(" to put them between parentheses. The sync game gains
  a **Backing** pass that times only those words; the other passes skip them.
- A lyrics file that holds voices, or a word held past backing vocals, is
  written as version 3 of the format; 0.3.1 and older versions see it as newer
  and do not save over it. Every other lyrics file is written as before.
- When you choose the .srt over the lyrics file, a line keeps its voices from
  the line with the same times or, failing that, the same text. NOH says how
  many lines lose theirs. The .srt itself holds no voices.
- Exports, shorts and burned captions read the voices from the lyrics file
  beside the .srt.
- The lyrics screen fits in every language at both window sizes: the footer
  takes a second row when its text needs one, a long line to sing is written
  smaller and slides so that the word to sing stays in view, and the speeds
  read 1×, 0.75×, 0.5×.

### Updates

- An update can install while an assistant such as Claude keeps NOH's MCP
  server open. When only such servers hold the NOH folder, they stop by
  themselves once they have no job left, and refuse new jobs meanwhile.
  **Restart to update** waits for them, with a button to cancel. Afterwards,
  reconnect the server in the assistant (Claude Code: `/mcp`, then
  Reconnect): NOH says so.
- When something else holds the NOH folder, the message says what, once per
  kind of program, instead of repeating the same path.
- NOH looks for a new version every 4 hours instead of once a day. The header
  shows **Update ready** or **New version** as a button.

### MCP

- An agent can open NOH windows and drive them while you watch
  (`noh_app_open`, `noh_app_do`, `noh_app_state`, `noh_app_look`,
  `noh_app_close`): it adds, removes and orders media, sets fades, edits lyrics
  lines, chooses the effect and its settings, the short's range, framing and safe
  area, the destination, exports or cancels, moves the playhead and looks at the
  window. It also declares the voices of the lyrics and sets who sings each
  line. It answers NOH's questions, which it reads in the window's state. One
  heavy job runs at a time across the agent's windows and its other tools, and
  the windows prepare their previews one after another. It
  knows where you are on the timeline, and never takes the playhead from you
  while you navigate. In such a window you can play, pause and
  move through the video, and point out a clip or a lyrics line; editing is the
  agent's. **History** in its badge lists what the agent did there, and
  **Stop driving** gives the window back to you. A window opens only
  if the computer has the memory for it, and stays open after the agent's
  session as an ordinary NOH window.

### Fixes

- NOH keeps up while its window is minimized. Before, the window took in
  nothing until it was shown again: an export's progress stood still and its
  end was shown only then, a short or an export with captions on the video
  that was still being prepared started only then, and a lyrics change was
  written only then. An export already running went on to the end in every
  case.
- Closing NOH no longer leaves a video decoder running behind it: closing
  waits up to two seconds for the decoders still running to be stopped. On
  Windows the decoders, the encoders and the downloads NOH starts also end
  with it when NOH is killed or crashes. The installer of an update is not
  concerned: it goes on after NOH closes, as before.

### Known issues

- The update from 0.3.1 to this version still waits for the idle MCP servers
  of 0.3.1, which do not know how to stop for an update. Close the assistant's
  session that uses NOH, or cancel and update later; after 20 seconds NOH says
  that a server which does not stop is probably from an older version.
- No caption style draws the voices yet: setting who sings a line changes
  nothing on the video in this version.
- Two lines still cannot be sung at the same time: a lyrics file or an .srt
  whose lines overlap is refused, as before. Words sung together go on one
  line, with both voices.
- When the main words of a line are timed again, its backing words are spread
  again, and backing words timed by hand are reset without warning if they
  end up out of order.
- Updates carry no signature of their own. NOH trusts GitHub's HTTPS and the
  digests GitHub reports for the files of the latest release. The Windows
  installers are not Authenticode-signed.
- The other known issues of [0.3.1](https://github.com/Awalgawe/noh/releases/tag/v0.3.1)
  remain, including those it carries from 0.3.0 and 0.2.0.
- The automated tests use software rendering and virtual audio. They test no
  real graphics card, sound card, NVENC or CUDA device.

## 0.3.1

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.3.1).

### Lyrics

- The sync game no longer ends at its first take when "Captions on the
  video" is on. In 0.3.0 each take edited the lyrics, which stopped the
  song, and the game took that for the end of the song. Without captions
  on the video the game was not affected. The captions on the monitor are
  redrawn with the new times when the game ends.

### Known issues

- Installed 0.3.0 copies on Windows are offered this version by their
  automatic update. It is the first published release they can receive that
  way: no installed copy had yet updated itself from a published release.
- If the caption effect fails during a sync game, the preview falls back to
  the player's own captions and stops the song, which ends the game, as in
  0.3.0.
- That the captions drawn over the picture follow the new times during a
  game was read in the code, not measured.
- The other known issues of [0.3.0](https://github.com/Awalgawe/noh/releases/tag/v0.3.0)
  remain, including those it carries from 0.2.0: nothing else in this
  version was written to correct them.

## 0.3.0

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.3.0).

### Lyrics

- Edit the lyrics on a screen of their own: "Edit" on the lyrics chip, or
  a click on the lyrics lane, opens it on the line under the pointer or the
  playhead. The text of every line is typed there like in a document (Enter
  cuts a line at the cursor, Backspace at its start joins it to the line
  above), beside the video, with a zoomed track under them where the lines,
  the words and the syllables are dragged.
- Set the times by ear with a sync game: while the song plays, hold Space for
  as long as each line, word or syllable is sung. The song can play slower,
  at the same pitch. A whole game is one undo step.
- Lines, words and syllables keep their own end time, so a breath between two
  words stays a silence in the caption effects. Lyrics files keep these ends;
  0.2.0 opens such a file without them and drops them if it saves it.
- The lyrics lane of the main screen no longer edits: its bubble, its word
  mode and its drags are replaced by the lyrics screen. Hovering a line shows
  its times and its text.
- While an export or another job runs, the lyrics are read-only, on the main
  screen and on the lyrics screen.

### Updates

- An installed copy on Windows updates itself, the way VS Code does: once a day
  it looks for a new release on GitHub, downloads its installer and the changed
  components in the background, checks every file, and installs silently when
  you restart or close NOH. Copies installed before this version must install it
  once by hand. Portable copies and Linux are told that a new version exists.
  Settings has two switches to turn checking and installing at close off. See
  the [installation guide](https://github.com/Awalgawe/noh/blob/v0.3.0/docs/INSTALLING.md#update-or-remove-noh).

### Known issues

- Automatic updates start with this version: an installed 0.2.0 or older copy
  never looks for one, so 0.3.0 is installed over it once by hand. The first
  automatic update a copy can receive is the version after 0.3.0.
- No installed copy has yet updated itself from a published release. The
  download and the checks were tested against stand-in releases, and the
  release's installer tests upgrade an installed 0.2.0 with the silent command
  NOH starts, but without NOH's own download before it.
- Updates carry no signature of their own. NOH trusts GitHub's HTTPS and the
  digests GitHub reports for the files of the latest release: a file replaced
  on the release page after publication would reach installed copies. The
  Windows installers are not Authenticode-signed.
- Update checks use Windows' `curl.exe`, which ignores the Windows proxy
  settings: behind a proxy configured only there, checks fail.
- Nobody has yet listened to whether the voice keeps its pitch when the song
  plays slower on the lyrics screen; the player is asked to keep it. Slower
  playback is offered only while the fast-preview library plays the montage,
  so not by default on Linux.
- The sync game reads a clock that moves by one picture of the montage: 40 ms
  on a 25 pictures per second montage. There, a time was taken within about
  30 ms of the key 95 times in 100, and up to 50 ms off, measured against the
  application's clock, not against physical key presses.
- The takes of a sync game are written to disk when the game ends: a crash
  during a game loses them.
- The known issues of [0.2.0](https://github.com/Awalgawe/noh/releases/tag/v0.2.0)
  remain: nothing in this version was written to correct them.
- The automated tests use software rendering and virtual audio. They test no
  real graphics card, sound card, NVENC or CUDA device.

## 0.2.0

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.2.0).

### Preview and playback

- On Windows, play the montage through the fast-preview library. A click on
  the time ruler while the montage plays shows the picture of the new position
  at once, and the playing goes on from there.
- Prepare a light copy of each video as soon as it is added. Copies are kept in
  the user's cache folder and found again by the content of a file, not by its
  name. A tile shows that its copy is being prepared, and the preview uses the
  copy as soon as it is ready.
- React to a press on the time ruler at once, instead of waiting to tell a
  click from a drag. While the montage plays, the preview pauses for as long as
  the button is down, shows the picture under the pointer and plays again at
  the release.
- Keep a picture on the monitor while playback stops, starts or is moved.
- Stop the preview from keeping a little more memory with each picture it
  shows. Versions 0.40 and 0.41 of the fast-preview library never give back a
  small object they ask the graphics driver for at each picture. Earlier
  Windows packages, which carry version 0.40, have this in the paused preview,
  which shows few pictures; on Linux it depends on the version the system
  provides. A montage played through the library shows 24 to 60 a second.
- The former player still plays what the fast-preview library does not: clip
  sound mixed in, a still image in the montage, captions drawn without the
  effect engine, a video whose copy is not ready yet. On Linux it remains the
  default player.
- When the library gives a montage up (it finds no sound output, or does not
  start within three seconds), the former player takes over from the same
  position: without a sound output, the montage starts and the other player
  takes over about a second later. From then on the former player plays until
  NOH is started again.
- Start NOH with the environment variable `NOH_PLAYER=ffmpeg` to keep the
  former player, or `NOH_PLAYER=mpv` to ask for the fast-preview library where
  it is not the default.

### Start-up

- Write each start to a small `startup.log` file beside the language
  preference, and show a message when the window cannot open. Until now the
  Windows interface closed without a word when it could not start. See the
  [installation guide](https://github.com/Awalgawe/noh/blob/v0.2.0/docs/INSTALLING.md#the-start-up-file) for where the file is
  and what it holds.

### Known issues

- Playing a montage through the fast-preview library was tried on one Windows
  computer, with one graphics card and one sound output. On Linux it was not
  tried: the former player is the default there.
- A montage played through the fast-preview library stops at its next cut,
  sound included, when NOH's window is not redrawn although the system reports
  it visible, and goes on when the window is drawn again. A minimised window
  and a window covered by another one did not stop it. Which situations do was
  not established; a locked session was not tried.
- With a project open, the memory NOH uses grew by about 2 MB a minute when
  nothing played and about 3 MB a minute while a montage played, with either
  player, in ten-minute measurements on one Windows computer. Without a
  project it did not grow. The cause is not established, and 0.1.6 was not
  measured.
- The message shown when the window cannot open was displayed in no test: the
  automated test is a capture run, which writes `startup.log` and never shows
  the message.
- On one Windows 10 tablet, Windows refused to run `noh.exe` ("Access is
  denied"). The cause is not established. This version does not fix it, and no
  `startup.log` is written in that case: nothing of NOH runs. See the
  [installation guide](https://github.com/Awalgawe/noh/blob/v0.2.0/docs/INSTALLING.md#the-start-up-file).
- On Debian 13 with GNOME, the preview's picture stopped updating after about
  one second in 3 of 23 automated test runs made while preparing 0.1.6. The
  cause is still not established, and nothing in this version was written to
  correct it: on Linux the montage is still played by the former player. It is
  not known whether a real computer is affected.
- The automated tests use software rendering and virtual audio. They test no
  real graphics card, sound card, NVENC or CUDA device.

## 0.1.6

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.6).

### Captions

- Draw captions with a GPU effect engine, in the preview and in the export.
  Six effects are built in: Classic (the default, which reproduces the former
  captions), Simple karaoke, Falling words, Sung sweep, Breath and Type block.
- Choose an effect and its settings in a style panel beside the monitor. The
  five effects other than Classic can draw with a font installed on the
  computer, chosen in a list that shows each font in its own type; a missing
  font is reported and replaced by the effect's own typeface.
- Wipe karaoke captions syllable by syllable, with syllables proposed from
  language packs.
- Report a caption that leaves the safe area or that is too wide, instead of
  cutting a word between letters.
- Choose a caption effect from the command line and from the MCP server.
- Without OpenGL 3.3, Classic falls back to the former FFmpeg captions and
  the other effects are refused with a message.

### Lyrics and subtitles

- Edit the subtitles in the lyrics lane, place the words of a line by hand,
  and keep Whisper's word times beside the subtitles.
- Keep every cue in a lyrics file beside the `.srt`. Edits are written as they
  are made; NOH asks once before modifying an `.srt`, and asks which file is
  the reference when the two differ. Read-only files are left untouched.
- Choose the Whisper model, with an automatic default that prefers the most
  accurate model present. The lyrics remember which model produced them.
- Keep the lyrics when Whisper's last segment runs past the end of the song.

### Export and playback

- Encode composed project exports and caption burns with NVENC when available.
- Use one colour matrix per export, read untagged video as players do, and
  show a colour matrix conflict in the simple view.
- Move the playhead only from the time ruler.
- On Linux with FFmpeg 6.1, say when FFmpeg draws caption colours in the
  older standard.

### Interface

- Choose the interface theme in Options: system, dark or light.

### MCP

- Serve tool schemas without the `uint` formats that JSON Schema does not
  define.

### Dependencies

- Update the dependencies, which clears
  five vulnerabilities and two unsound advisories reported by the RustSec
  audit. rmcp moves to 3.10.0 and ZIP to 9.5.1.

### Linux desktops tested

- openSUSE Tumbleweed is now tested with KDE on Wayland: its main repository
  no longer has the package of the KDE X11 session. KDE on X11 is no longer tested; X11 remains tested with
  GNOME on Debian 13 and Cinnamon on Mint 22.3.

### Known issues

- On Debian 13 with GNOME, in 3 of 23 automated test runs made while
  preparing this release, the preview's picture stopped updating after about
  one second. In the one occurrence that was recorded in detail, the player
  reported the end of the clip at that moment. A possible cause is being
  investigated: the preview ending when its audio queue is momentarily empty;
  the same condition is already in 0.1.5. It is not known whether a real computer
  is affected.
- In the desktop test run of this release, Debian 13 passed on a second
  attempt after that failure, and Mint 22.3 passed on a second attempt after
  its first one timed out downloading its test image, before any test started.
- The automated tests use software rendering and virtual audio. No real
  graphics card, sound card, NVENC or CUDA device was tested.

## 0.1.5 — 2026-10-17, Windows packages added 2026-10-18

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.5).
The original Linux archives and tag are preserved. The Windows build has its
own recorded commit, differing only in release documentation and policy;
each platform's materials archive contains its actual compiled sources.

- Fix native Wayland picture, video and WAV drops, while preserving clipboard
  integration, cancellation, path validation and imports through the file chooser.
- Update ZIP to 9.0.0 for its upstream security fixes. Preserve Windows archive
  member and traversal validation.
- Include Windows Minimal, Standard, Complete and Web installers, the complete
  portable archive and corresponding sources for version 0.1.5.
- Correct the installation guide for the host-owned Linux media runtime and
  link the seven-desktop qualification results.

## 0.1.4 — Linux package

The [published Linux release](https://github.com/Awalgawe/noh/releases/tag/v0.1.4)
contains the host-runtime packaging correction below.

- Linux packages now require host FFmpeg and libmpv, and ship no shared
  libraries. This keeps the media, graphics and audio stack with the host's
  drivers/plugins and removes the forced ALSA plugin directory. CPU speech
  remains included. Install the declared host dependencies and extract into a
  fresh directory. The [candidate report](LINUX_0_1_4_CANDIDATE.md) records the
  seven desktop results, exact files and independent-review boundary.
- Find the Linux distribution's FFmpeg when launched from `/`, `/usr` or
  `/usr/bin`, while continuing to reject implicit working-directory lookup.
- Keep Linux preview audio active while ALSA recovers from a temporary buffer
  underrun, instead of permanently switching to silent preview. Device loss,
  recovery failures and a stalled audio clock still trigger silent fallback.
- Verify the installed-dependency contract in final archives and retain the
  linked Linux installation guide. Test one sealed candidate across the
  [Linux validation matrix](LINUX_VALIDATION.md), with separate desktop gates.
- Update the locked Rustls dependency to 0.23.46 for the four security fixes
  described in its [upstream release](https://github.com/rustls/rustls/releases/tag/v%2F0.23.46).

## 0.1.3 — 2026-10-14

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.3).
Both application packages come from the same CI build; Windows installers reuse
its application bytes. See the [Linux qualification report](LINUX_0_1_3_QUALIFICATION.md)
for actual desktop checks and the Ubuntu/WSLg limits.

- Treat cancelling the native file chooser as cancellation, without a false error.
- Keep the sung-language choices open inside the lyrics menu.
- Open Linux media, folders and SRT files with Linux default applications under
  WSL as well, and keep external launch waits off the interface thread.
- Translate opening confirmations and export/preview cancellation in all seven
  interface languages.
- Show export success and its Open buttons after a previous refused file drop.
- Document the Linux host file chooser and default-application prerequisites,
  including the Windows Explorer-to-WSLg drop limitation.

## Linux package addition — 2026-10-13

The [0.1.2 release](https://github.com/Awalgawe/noh/releases/tag/v0.1.2) now also
provides a complete Linux x86_64 AppDir ZIP with media tools and CPU transcription
models. Its CI-built bytes were validated on the recorded Ubuntu 24.04/WSLg
baseline, with software graphics and PulseAudio. See the
[qualification report](LINUX_0_1_2_QUALIFICATION.md) for limits and exact hashes.
Existing Windows assets are unchanged. The Release workflow selects Windows and
Linux together by default and retains individual platform targets.

## 0.1.2 — 2026-10-10

Download the [Windows installers and portable ZIP](https://github.com/Awalgawe/noh/releases/tag/v0.1.2).

- Use one Windows web installation wizard: embed the small application payload
  and download only the selected optional components, without launching another
  NOH installer.
- Show cancellable verification progress for large component archives and existing
  content. Display the temporary location before installation and clean owned
  downloads/extracted files on success, failure or normal cancellation.
- Prevent installation after cancelling extraction, and retain the existing
  content profile when rerunning Web setup.

## 0.1.1 — 2026-10-09

Download the [Windows installers and portable ZIP](https://github.com/Awalgawe/noh/releases/tag/v0.1.1).

- Restore Minimal, Standard and Complete Windows installers and a small web selector.
- Offer missing media tools and transcription downloads during setup and from
  NOH's resource settings. Existing installations can add or repair content
  without uninstalling, while preserving user files and already installed content.
- Keep downloads outside the GUI thread, check exact archive hashes and retain
  the seven interface languages. Restart NOH to initialize newly added preview support.
- Consolidate CI around PR validation and Release; remove obsolete measurement
  and private installer workflows. Package installers from an identified successful
  Release build without compiling the application again.

## 0.1.0 — 2026-10-09

The first public Windows x64 release, with an installer and a portable ZIP, is available from
[GitHub Releases](https://github.com/Awalgawe/noh/releases/tag/v0.1.0).

### Added

- A desktop workflow for ordering video clips and PNG/JPEG photos, repeating the
  sequence to match a WAV soundtrack, previewing the result and exporting a video.
- Waveform range selection and vertical short exports with fit or crop controls.
- SRT lyrics/captions and local transcription with the bundled speech runtime;
  generated text and timing can be reviewed before rendering it into a video.
- Background media processing, cancellation and protection against overwriting
  existing files.
- English, French, German, Spanish, Japanese, Korean and Simplified Chinese interfaces.
- A command-line interface and local MCP server sharing the desktop's media core.
- Public GPL-3.0-only sources, contribution guidance and private security reporting.

### Distribution

- Native CI validation on Windows x64, Linux, macOS Apple Silicon and Intel Mac.
- PR checks that select affected platforms and avoid native builds for documentation.
- A separate manual pipeline for package verification, candidate builds and release drafts.
- A Windows x64 portable package with bundled media tools and local speech models.
- A per-user Windows installer, Start-menu shortcut and uninstaller, built and
  tested through the standard Release pipeline from the qualified portable bytes.
- An optional direct Microsoft prerequisite download in the installation wizard.
- A direct Microsoft download action when the optional transcription prerequisite is missing.
- Corresponding native/Rust source archives, build recipes and original licence notices.

### Known limitations

- macOS and Linux packages are not available; these targets require source builds.
- macOS distribution signing and notarization are not complete.
- Automatic update installation is disabled in ordinary builds.
- The desktop does not restore the editing session after closing.
- Verified HDR tone mapping and end-to-end color management are not provided.

See the [desktop guide](APP.md) for supported workflows and the
[release readiness guide](RELEASING.md) for packaging status.
