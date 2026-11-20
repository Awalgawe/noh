# Update architecture

## Automatic updates in published builds

Published builds update with `src/auto_update` (Settings section in
`src/app_autoupdate.rs`). An installed Windows copy reads GitHub's latest release,
downloads the release's Minimal Inno Setup installer and the component archives
whose installed files differ from `COMPONENTS.json`, checks each size and SHA-256
against the digests GitHub reports, and runs the installer silently with
`/NOCLOSEAPPLICATIONS`, `/WAITPID=<pid>` and, after "Restart to update",
`/RELAUNCH` (`tools/installer/public.iss`). It refuses while any program of the
installation folder runs. Network access goes through the system `curl.exe`
started with `--disable` (no `.curlrc`), HTTPS only. Portable copies and Linux
only report a new version. The daily check is on by default and can be switched
off in Settings, as can the installation when NOH closes.

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

## History

Until 0.3.0 the repository also carried an unused update prototype: the
`updates` Cargo feature (`src/update/`, the `noh-update-guard` and
`noh-update-repair` executables, the `update-release` producer), its private
Velopack Setup delivery and qualification scripts, and their documents
(`UPDATE_RECOVERY.md`, `UPDATE_NATIVE_GATES.md`, `UPDATE_RELEASE.md`). No
published build ever enabled it. It was removed after 0.3.0 shipped
`src/auto_update`; its sources remain in Git history.
