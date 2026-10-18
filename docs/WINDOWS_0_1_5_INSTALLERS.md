# Windows 0.1.5 installer qualification

The exact installers from [run 39084998885, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39084998885)
wrap the [qualified Windows portable](WINDOWS_0_1_5_QUALIFICATION.md) without
rebuilding the application. Both application and wrapper source commit are
`a7a3e2cc0a4c337ed88699b0573f1689d0a5b1cc`.

The [evidence record](release-evidence/windows-0.1.5-installers.json) binds
every installer and wrapper-source archive, the portable delivery, component
catalogue, native results and hosted report. The [publication inventory](release-evidence/publication-0.1.5.json)
identifies every final asset and the exact release-note digest. Publication
requires independent approval of this complete version and successful PR checks.

## Completed checks

- Hosted Minimal, Standard and Complete install/reinstall, exact installed
  payload checks, CLI, uninstall and preservation of user-created files passed.
  Web installs, component additions, repair, profile retention and rejection of
  wrong/unknown profiles, conflicting installation paths and corrupt media passed.
- Hosted GUI startup was unavailable due to missing OpenGL 2.0. Each exact profile
  was therefore installed locally using Web with adjacent verified archives.
  All three installed GUI and CLI checks passed on Windows 10 x64 22H2, build
  19045, with an RTX 4060. Their French captures were visually inspected.
- An actual 0.1.3 Complete installation upgraded using the new Minimal installer
  retained Complete components and a user-created file. Every installed payload
  hash matched the expected 0.1.5 Complete profile. Test installations were removed;
  no registered test installation remained.
- The five executables, including the embedded maintenance helper, passed Windows
  Attachment Services and Defender checks without changing security settings or
  their bytes. They remain unsigned.
- All five wrapper-source archives retain the exact wrapper/scripts, original
  compiler/decoder sources and notices. Correspondence with the recorded Git
  commit and locked compiler/decoder hashes was verified.

## Publication boundary and limits

Add these exact Windows assets to existing release `415854483`, keeping the
`v0.1.5` tag and three existing Linux files byte-for-byte. Replace only the
global `SHA256SUMS`, then apply the reviewed [release notes](RELEASE_0_1_5.md).
The temporary installer review draft is not the public release.

Prepublication component acquisition used verified adjacent archives. After
publication, verify anonymous metadata and download sizes, fetch the public Web
installer/checksums, install default Standard through actual HTTPS, cancel a
live speech transfer, retry Complete, verify payloads and GUI/CLI/media/CPU
transcription, and remove the test installation while preserving user data.
These postpublication checks are pending in this frozen prepublication record.

The host already had Microsoft's runtime. No fresh OS, fresh prerequisite
installation, live in-app component-button click, Authenticode signing or
SmartScreen reputation is claimed. Existing exact-portable playback/export
and speech evidence applies to unchanged installed application bytes.
