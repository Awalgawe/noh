# Prepared macOS signing and notarization

See [code signing for GitHub releases](CODE_SIGNING.md) for the shared Windows/Mac
provider decision and current account prerequisites. Developer ID is for
distribution outside the Mac App Store, including GitHub Releases.

This procedure is prepared and tested with simulated commands on Windows. It has
not signed or submitted a real application. Native acceptance remains required
on the exact final Apple Silicon package. No Apple account, certificate or secret
has been created or configured by this preparation.

The Release workflow's `verify` and `build` stages request portable candidates;
`sign_macos=false` remains the default.
Signing requires `main`, `portable`, `macos-arm64` or `all`, and the protected
`noh-macos-signing` environment. Configure that environment with an independent
required reviewer, prevention of self-review and a main-only deployment policy.
Unsigned jobs use `noh-candidate-build` and receive no signing inputs.
The independent redistribution gate runs before acquisition/build/signing when
`upload_candidate=true`. With `upload_candidate=false`, the candidate remains on
the disposable runner; `sign_macos=true` still explicitly requests private
submission to Apple and requires the protected signing environment.

Owner-provided environment inputs:

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `NOH_MACOS_IDENTITY` | Exact SHA-1 fingerprint of the Developer ID Application certificate. |
| Variable | `NOH_MACOS_TEAM_ID` | Ten-character Apple Developer team identifier. |
| Secret | `NOH_MACOS_CERTIFICATE_BASE64` | Base64-encoded P12 containing that certificate and private key. |
| Secret | `NOH_MACOS_CERTIFICATE_PASSWORD` | P12 password. |
| Secret | `NOH_APPLE_ID` | Account authorized to use Apple's notary service. |
| Secret | `NOH_APPLE_APP_PASSWORD` | Apple app-specific password for that account. |

Do not put these secrets in repository files, workflow arguments, reports or
source materials. Build scripts, dependency tools and tests receive no signing
credentials; only the separate configuration check and signing steps receive them.
The helper captures credential command output and omits command
arguments/stderr from errors. The P12 and random-password keychain live in a
private temporary directory. Keychain deletion and temporary-file cleanup run on
success and failure; hosted cancellation/runner loss still needs native validation.

`tools/macos_signing.py` defaults to an unexecuted plan when given `--bundle` and
`--commit`. This checks the complete manifest and recorded Mach-O closure without
reading signing credentials or invoking Apple tools. `--preflight` checks host and
configuration syntax before expensive builds; it does not prove credentials valid.
`--execute` is restricted to main-branch Apple Silicon GitHub runners. The workflow
invokes execution only after the explicit reviewed signing choice.

Execution imports the specified identity, checks its Developer ID type and team,
and stores notary credentials in the temporary keychain. It signs every recorded
Mach-O component with hardened runtime and a secure timestamp, then the enclosing
application. Signing does not use `--deep`; recursive strict verification does.
No additional hardened-runtime exceptions or debugging entitlements are requested.
Actual library loading, Metal, preview, audio and worker behavior need Mac tests.

The helper uploads a private submission ZIP to Apple, waits for the native
`notarytool` result, retains the matching log and refuses rejected/pending results
or logs with issues requiring review. It staples and validates the ticket, then
checks signatures and Gatekeeper assessment. This private transfer to Apple is
part of the explicit signing choice; it is not a GitHub release publication.

`MACOS-SIGNING.json` and `MACOS-NOTARY-LOG.json` remain outside `NOH.app`.
Application resources are finalized before signing; file hashes and the outer
manifest are refreshed only after signing/stapling. The normal delivery seal then
creates the final distribution ZIP. On the native runner it extracts those exact
ZIP bytes with `ditto` and rechecks application hashes, Developer ID signatures,
the stapled ticket and Gatekeeper. Failure stops artifact delivery. This check is
required even if the earlier submission ZIP passed.

These commands are preparation, not current proof of Apple acceptance. A future
native run must verify CLI options against the installed tools, identity import,
cleanup, all nested signatures, secure timestamps, notarization logs, stapling,
final ZIP ticket/permissions transport and packaged GUI/CLI/MCP/media behavior.
Clean-profile downloaded-file Gatekeeper acceptance, offline behavior, physical
audio/GPU and the supported OS baseline remain independent final-byte tests.
Signing/notarization alone do not clear source/license or qualification gates.

Primary references: [Apple notarization requirements](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution),
[custom notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow),
[inside-out code signing](https://developer.apple.com/library/archive/technotes/tn2206/_index.html).
