# Windows 0.1.1 installer qualification

The four setup choices and installed component helper reuse the exact qualified
[0.1.1 portable](WINDOWS_0_1_1_QUALIFICATION.md). The application was built once
from `9a8ee751d95c5950c3dffab0eb34ca8493d8e8f0` in
[Release run 37917999373](https://github.com/Awalgawe/noh/actions/runs/37917999373).
[Release run 37921521530](https://github.com/Awalgawe/noh/actions/runs/37921521530),
attempt 1, built and tested the wrappers from that same source identity.

## Exact packages

| Installer | Bytes | SHA-256 |
| --- | ---: | --- |
| Minimal | 38,926,497 | `000e381fd83a879bf4f8923bbebd799bb9650472f80b608e702fd7c6fac6d813` |
| Standard | 148,119,772 | `e6aaf7ca56b09c8be1fac51163f74a743990109569da73dd2c76deb20dccea88` |
| Complete | 1,220,689,036 | `9faea9f14128f88c8fff0b730c901d1cf2f1c4574976537b446c828315e20a27` |
| Web | 2,830,098 | `0219c6fc83763a0e20e837a6f003fef7d59b5257a6512a2461723f06063eddd0` |

The embedded helper is 3,368,220 bytes, SHA-256
`fc2652a10420fcb768e74593c9af530ff131f315354f2cde61a24a6c4361a0cc`.
The release's `SHA256SUMS`, `INSTALLER-PROVENANCE.json`, component catalog and
DELIVERY record bind the complete asset inventory.

The media/speech archives preserve the native payload and notices from the
portable. Five wrapper-source ZIPs accompany the three offline installers,
Web selector and maintenance helper. Inno and its LGPL decoder replacement
materials are included as recorded in [the reconstruction guide](INSTALLER_REBUILD.md).
The Microsoft prerequisite is obtained directly from Microsoft when requested;
its installer and runtime DLLs are not redistributed in these packages.

## Native prepublication checks

On the GitHub-hosted Windows Server 2025 runner (build 26100), the exact wrappers
passed media/speech additions, repair of missing content, retention of acquired
components, corrupt-archive and unknown-profile rejection, and removal while
preserving a user-file sentinel. These results are recorded in
`INSTALLER-TESTS.json` from run 37921521530.

The same downloaded wrappers then passed these additional checks on Windows 10
x64 22H2, build 19045:

- Minimal, Standard and Complete installation through the cached Web selector,
  repair with the corresponding offline installer, profile retention and
  rejection of a different installation directory.
- Installed GUI/CLI execution for each profile. Hosted GUI checks lacked
  OpenGL 2.0; these exact local checks supply the missing native evidence.
- Upgrade a real 0.1.0 Complete installation using the 0.1.1 Minimal installer,
  retaining Complete content and the user-file sentinel.
- Traverse the French Minimal wizard with keyboard/accessibility controls,
  choose its profile/destination, complete installation and launch installed NOH.
- Cancel the helper before transfer, observe a real unavailable HTTPS URL
  failure without installed-file changes, and retry successfully using a
  hash-checked adjacent component archive.
- Add speech while NOH stays open with bundled libmpv/preview DLLs loaded,
  using the app's `/PROFILE`, `/LANG` and `/DIR` arguments plus silent/logging flags.
  The complete installed inventory and user-file sentinel matched afterwards.

All five executable wrappers passed native Attachment Services checks with
their actual GitHub staging-asset origins and Internet zone. Defender reported
no threats. File hashes remained unchanged and normal protection stayed active.

## Limits and publication path

The binaries are unsigned; no Authenticode or interactive SmartScreen reputation
approval is claimed. Tests used an existing Microsoft runtime, not a fresh OS or
a fresh Microsoft prerequisite installation.

Desktop screenshot capture failed in the native automation tool. Inno keyboard
and accessibility observations worked. The live egui button click and subsequent
resource refresh were not observed: their source/tests, native interface captures
and equivalent helper invocation provide the explicitly bounded evidence.
Restart NOH to initialize newly added preview support.

The standard Release workflow produced the application and all installer bytes.
Its separate draft stage expects a protected GitHub environment that is not
configured on this repository. This release therefore uses the independently
reviewed local draft preparer, retaining the exact qualification, source, hash
and merged-PR gates. No environment or repository protection was changed.
The ordinary release was first published without changing `latest`. Public HTTPS
acquisition subsequently passed. Latest promotion retains the validation limits
below and preserves the old 0.1.0 assets.

## Public delivery checkpoint

Ordinary v0.1.1 is published at the compiled source with all 17 frozen assets.
Anonymous HEAD requests and GitHub asset digests agree with the complete inventory;
the checksum file and Web executable were fetched and hashed directly.
The exact Web installer downloaded and installed Minimal from the public URL.
The installed helper then downloaded media to reach Standard, followed by speech
to reach Complete. Exact inventories, installed GUI/CLI, FFmpeg and actual CPU
transcription passed. Test removal preserved the unrelated user-file sentinel.

In-flight cancellation remains unqualified. The first attempt reached extraction
before the stop action, and the automation tool could not operate its confirmation.
The second test used silent mode, which hid the download UI and completed the
installation. Neither is counted as cancellation success. A future qualification
needs a normal-mode Stop/Yes action before the transfer completes, followed by
process closure, unchanged inventory and retry verification. The separate capture
diagnostic did not establish a correction for the automation failure.

Latest promotion does not assert that this scenario passed. It makes the already
published, CI-produced release the default download with this validation limit
explicitly retained in its public notes. The earlier requirement to finish this
manual scenario before promotion is revised in this review; neither a successful
retry after interrupted public transfer nor unrestricted installer acceptance is
claimed. No binary, tag, source identity or repository protection changes.
