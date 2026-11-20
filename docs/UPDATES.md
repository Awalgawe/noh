# Update architecture

## Automatic updates in published builds

Published builds update with `src/auto_update` (Settings section in
`src/app_autoupdate.rs`). An installed Windows copy reads GitHub's latest release,
downloads the release's Minimal Inno Setup installer and the component archives
whose installed files differ from `COMPONENTS.json`, checks each size and SHA-256
against the digests GitHub reports, and runs the installer silently with
`/NOCLOSEAPPLICATIONS`, `/WAITPID=<pid>` and, after "Restart to update",
`/RELAUNCH` (`tools/installer/public.iss`). Network access goes through the
system `curl.exe` started with `--disable` (no `.curlrc`), HTTPS only. Portable
copies and Linux only report a new version. The check runs at start and every
4 hours (`CHECK_INTERVAL`); it is on by default and can be switched off in
Settings, as can the installation when NOH closes.

The installer cannot replace a running program, so NOH refuses while any program
of the installation folder runs, grouped in the message by what started it (an
export's FFmpeg counts as that export). MCP servers are the exception, since an
assistant keeps them open for its whole session. When they alone hold the
folder (anything else is refused first, so no server stops for an update that
cannot install), NOH writes a stop request (`src/update_stop.rs`,
`stop-servers.json` in the updates folder) naming the installation folder. Each
`noh-mcp` of that folder checks it every second. While the request stands, a
server refuses new jobs (`mcp.updating`), and it exits once no job runs and none
ended in the last 15 s. A server is never stopped mid-job. "Restart to update"
waits for the servers, with a Cancel button; the header badge reads "Restart
pending" meanwhile, so the wait stays visible with Settings closed. Closing NOH
waits up to 5 s. The request lapses 30 s after NOH last renewed it, so a NOH
that crashed never leaves servers refusing work; after the installer starts, it
keeps newly started servers off the files only for the rest of that lifetime.
Interactive Claude Code does not restart a stdio server that exited: the user
reconnects it (`/mcp`, then Reconnect). The waiting message says so, and so does
the "updated" notice at the next start when servers were stopped
(`Attempt::servers_stopped`).

The release CI test `test_upgrade` (`tools/installer/public-ci.py`) exercises the
refusal, the wait and the relaunch. The installer side lives in
`tools/installer/public-update.iss`; the native test
`tools/installer/update-native.py` compiles it into a synthetic silent setup and
checks the measured wait, the 60 s refusal (with its log), an invalid value and
the relaunch with the inherited environment, without installing anything.
The trial switch `NOH_AUTOUPDATE_TRIAL=file:///<folder>/` points an installed
copy at a local release folder (`latest.json`, `v<version>/…`) for manual tests.

## Known limits

- **No signature beyond GitHub's.** The update trusts GitHub's HTTPS and the
  digest GitHub reports for each file of the latest release. Anyone who can write
  to the repository can replace a release file after promotion, and that file then
  reaches installed copies (see the trust note in [RELEASING.md](RELEASING.md)).
  A separate signature, for example Ed25519 with a key kept off GitHub, would
  close this, but it needs that key on the maintainer's machine and a signing step
  at every promotion. It is left for a later decision.
- Copies installed before 0.3.0 cannot detect anything: they must install a newer
  version by hand once.
- `curl.exe` ignores the Windows system proxy settings.
- MCP servers from before this stop request (0.3.1 and earlier) do not know it:
  the first update from such a copy still waits until their assistant session
  closes or reconnects them; after 20 s the message says so.

## History

Until 0.3.0 the repository also carried an unused update prototype: the
`updates` Cargo feature (`src/update/`, the `noh-update-guard` and
`noh-update-repair` executables, the `update-release` producer), its private
Velopack Setup delivery and qualification scripts, and their documents
(`UPDATE_RECOVERY.md`, `UPDATE_NATIVE_GATES.md`, `UPDATE_RELEASE.md`). No
published build ever enabled it. It was removed after 0.3.0 shipped
`src/auto_update`; its sources remain in Git history.
