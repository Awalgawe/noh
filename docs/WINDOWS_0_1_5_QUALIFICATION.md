# Windows 0.1.5 exact-byte qualification

The Windows portable and corresponding materials come from [Release run 39082899722, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39082899722), compiled from `a7a3e2cc0a4c337ed88699b0573f1689d0a5b1cc`.

This supplies the Windows packages omitted from the initial 0.1.5 publication.
The published Linux archives and `v0.1.5` tag remain unchanged at
`1e2bbbd94c578115d45a750ab61493f7a1e48cae`. The tag-to-Windows-build diff
contains only release documentation, evidence and delivery-policy changes.
The Windows source archive contains its own actual compiled commit, not the
older tag revision.

The [evidence record](release-evidence/windows-0.1.5.json) binds DELIVERY.json SHA-256 `7702c908af3b1ae28f91e2fcd62bc43e3ca1080d97e03de47e381284dedab766`.
The original CI delivery retains its producer status; final qualification is
a separate record accepted only after independent review.

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.5-windows-x64-materials.zip` | 1277685694 | `7035cbc7d5b473f4d36296fbb6379f45f5ddf802b30ab76553ac81a2f31b8235` |
| `NOH-0.1.5-windows-x64.zip` | 1312790113 | `183f3adc0b54f2f1ce46e07efb509f2eaa93c44dd6f38242a310584202a44d4d` |

## Verified behavior

The exact build passed 482 Cargo tests, zero failures and 12 explicit ignores
across 22 suites, including GUI/CLI/media/export/MCP checks. Its RustSec
audit and delivery controls passed.

Fresh application settings and Windows system directories only on PATH were used
on Microsoft Windows 10 Professionnel 10.0.19045 build 19045 with NVIDIA GeForce RTX 4060. Native preview
produced 259 observed frames, six play/pause/seek handoffs and
2 playing audio-device observations. Both CPU and CUDA transcription
recognized the expected synthetic French speech.

The 84-case capture matrix passed across seven languages, two viewport sizes
and three scales. Four representative French, Japanese and German captures
were visually inspected. Resource-options intentionally simulates missing
components; its warnings do not describe the complete package.

Windows Attachment Services recorded the actual GitHub download origin and
Internet zone. Defender reported a successful scan; archive and executable
hashes remained unchanged. No security setting was disabled.

## Source and notice correspondence

All 395 source files match Git's Windows archive of the compiled
commit. 190 files use CRLF; their normalized contents match the LF
Git archive exactly. The 265 PE images have no undeclared missing imports.

Windows native materials are byte-identical to qualified 0.1.3. The complete
486-crate vendor graph corresponds to the already reviewed 0.1.5 graph:
206 winit text files use CRLF, with their actual Windows checksum map verified.
The vendor configuration parses to the same TOML. No additional source change
was accepted.

The original Rust notices are complete and unchanged in substance. Windows
orders one emoji-font notice block earlier than Linux; the complete remaining
text is identical. The Windows standard-library receipt matches qualified
0.1.3. The bundled notices equal the corresponding materials. Their actual
aggregate SHA-256 is `a34c6222dfbf3e86197ac9ccd767c9138c2c3265c9cdbd2568e6d9e518b17b2e`.
The existing macOS-only dispatch declaration supplement remains explicitly
distinguished from an original upstream notice.

## Limits

This is a fresh application profile on the recorded existing host, not a fresh
OS or universal hardware qualification. Microsoft runtime was already installed.
Audio-device operation was observed; no human listening test is claimed.
Packages are unsigned and have no claimed Authenticode or SmartScreen reputation.
Installer profiles and actual public HTTPS component acquisition have separate
evidence. Linux runtime results do not qualify Windows.
