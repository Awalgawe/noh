# Windows 0.1.3 exact-byte qualification

The Windows and Linux applications were produced by the same [Release build](https://github.com/Awalgawe/noh/actions/runs/38481645547),
attempt 1, from `6b8c8054b93169fd455d86d16af8aeb92fec1a0e`. This report covers the exact
windows x64 portable and corresponding materials. Publication and Windows
installer wrappers are separately checked; a future build does not inherit this acceptance.

The full [evidence record](release-evidence/windows-0.1.3.json) is bound in
`assets/delivery-policy.json` to DELIVERY.json SHA-256
`01b7f97e7cceb00731c51024ceab654ebb48a4aedb58f4f54feed77b11fc3df5`. The original CI manifest retains its `unqualified`
producer status; the separate exact-byte record defines the reviewed scope.

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.3-windows-x64-materials.zip` | 1277444531 | `bda9cd862be7a0488a9760f44da80d4a625827e804c75e900e3b374efbfde4b9` |
| `NOH-0.1.3-windows-x64.zip` | 1312787060 | `69ffecd05d9ba7ce1c592c34916bfd3f9f0b8026b8e7f1ac89151784d81a1045` |

CI completed 481 tests, zero failures and 12 existing explicit
ignores across 22 suites. The shared RustSec audit and delivery
controls passed. Downloaded artifact digests, archive inventories, source
identity and native import closure were checked before native acceptance.

## Native acceptance

The downloaded portable ran with fresh application settings and Windows system
directories only on PATH. Native preview/playback, play/pause/seek transitions,
loaded bundled runtime modules, CPU and CUDA transcription passed. The capture
matrix completed 84 cases across seven languages,
two sizes, three scales and ready/resource-options states. Four representative
captures were visually inspected, including narrow French and Japanese options.
Resource-options intentionally simulates missing components; this is not a
missing-runtime result for the complete package. These automated captures do
not constitute live mouse/keyboard interaction coverage.

Package identity and every corresponding source file were checked. Third-party
native/runtime payload hashes match qualified 0.1.2, and bundled notices retain
the same normalized text. PE import closure has no undeclared missing imports.
Windows Attachment Services recorded the actual GitHub origin and ZoneId=3;
Defender scanning reported no threat and the downloaded bytes remained unchanged.

## Limits

The host is the existing Windows 10 Pro x64 22H2 build 19045 machine with a
NVIDIA GPU and an installed Microsoft runtime. This does not claim a fresh OS,
new prerequisite installation, every GPU or a human listening check. Packages
are unsigned; no Authenticode or SmartScreen reputation approval is claimed.
The external Microsoft speech prerequisite and its installer are not redistributed.
Installer profiles and real public HTTPS component acquisition have separate
receipts in the release provenance and publication review.
