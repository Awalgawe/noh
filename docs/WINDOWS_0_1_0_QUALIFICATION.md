# Windows 0.1.0 portable qualification

This record covers the ordinary Windows x64 portable ZIP and its corresponding
source/materials ZIP. The installer is excluded: its source and wrapper compile
were reviewed, but the native UI tool refused the interactive test launch.
It is not an accepted installer or a published download for this release.

## Exact source and artifacts

- Compiled public source: `df41ed864aef4aef4bf8df0da2460cd7173e2cf1`.
- DELIVERY record: `83b85e6b4641a37e81b1a3f0e7633065f9509ebb8e979333954513aa7b379646`.
- Portable ZIP: `79eae3216d45a290ce5e47ae530e28530b66c0eb1ac6d1f58fee193c7cfd3765`.
- Materials ZIP: `cd2e0dd70793d159e748e8579c85a18ec4d1addf7fd4ffb522c71761025248df`.

[Machine-readable evidence](release-evidence/windows-0.1.0.json) records sizes,
native results, source-receipt hashes and validation limits. The clean local
release build uses Rust 1.98.1 and the repository's Windows GNU/LLD configuration
with `gui,mcp`. It is not represented as a hosted Actions build. The matching
source tree passed [PR CI on all four platforms](https://github.com/Awalgawe/noh/actions/runs/37884373742)
before squash merge; the merge tree was checked against that tested tree.

The materials ZIP contains the exact NOH Git source archive, locked vendor
sources, native sources/recipes/patches, standard-library sources/dependencies
and original notices. The applicable material decisions are documented in
[Windows redistribution](WINDOWS_REDISTRIBUTION.md) and
[Rust notices](RUST_NOTICE_DECISIONS.md). Microsoft runtime files are excluded;
users obtain the optional speech prerequisite directly from Microsoft.

## Native acceptance

On Windows 10 Pro 22H2 x64, build 19045, with an RTX 4060 and Microsoft v14 x64
runtime 14.51.36247.0, the exact portable passed:

- Normal/delay PE import audit: 265 images, no unresolved imports beyond the
  explicitly recorded external system/prerequisite boundary.
- Fresh extraction and new application configuration; isolated native GUI and
  direct speech probes use only Windows system directories in PATH.
- 476 integration-suite tests, with 14 existing ignores reported explicitly;
  the three real-transcription cases were then run explicitly and passed.
- CPU-only speech inference and actual CUDA0 inference on known synthetic French
  speech, including the expected words, with no media upload.
- 693 observed native GUI frames and six play/pause/seek handoffs, including two
  playing states with a real audio device. Bundled libmpv and NVIDIA's OpenGL
  driver were observed. Caption rendering and media export tests passed.
- 42 ready-state captures: seven languages, two sizes, three scales, dark theme.
  The full contact sheet, a native playback image and French/Japanese narrow
  images were visually inspected.

These are fresh application tests on the recorded host, not a fresh Windows
installation or human listening test. They do not establish universal GPU,
CPU, OS or driver compatibility. The published application is unsigned, with
no Authenticode identity or SmartScreen reputation claimed.

## Download protection

The portable was downloaded again from the unpublished GitHub asset API and its
SHA-256 matched the sealed archive. The extracted `noh.exe` matched the manifest.
Windows Attachment Services preserved Internet-zone evidence (`ZoneId=3`) and
returned `S_OK` for Save. CheckPolicy returned `S_FALSE` for both files: user
confirmation is required; no prompt was accepted and no executable was launched.
The exact executable reports `NotSigned`.

Microsoft Defender 4.18.26080.4-0 then scanned the downloaded ZIP and extracted
executable with default protection/remediation retained. The scan completed in
55.188 seconds, returned zero, reported no threats, and left both hashes unchanged.
There were zero matching threat-history detections. Antivirus and real-time
protection remained enabled. The recorded definition version/date is retained
in the JSON evidence rather than assumed current indefinitely.

These are native Windows attachment-policy and antivirus results, not a browser
or interactive SmartScreen reputation approval. No security settings were lowered.
See Microsoft's [CheckPolicy return values](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-iattachmentexecute-checkpolicy),
[Save behavior](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-iattachmentexecute-save),
and [Defender scan documentation](https://learn.microsoft.com/en-us/defender-endpoint/command-line-arguments-microsoft-defender-antivirus).

## Documentation and distribution

The application binaries and source snapshot are frozen at the compiled commit.
Current online installation instructions and release notes identify the
portable-only offering; packaged guides reflect the earlier build-source
snapshot and include generic installer instructions. This metadata update does
not rebuild or relabel the application. Automatic application updating is disabled.
