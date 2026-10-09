# Windows 0.1.1 portable qualification

This record covers the exact Windows x64 portable application and corresponding
materials produced by the standard Release workflow. Installer wrappers,
on-demand component downloads and publication have separate acceptance gates.

## Source and provenance

- Compiled source: `9a8ee751d95c5950c3dffab0eb34ca8493d8e8f0`, clean, with `gui,mcp`.
- Successful [Release build 37917999373](https://github.com/Awalgawe/noh/actions/runs/37917999373), attempt 1.
- DELIVERY SHA-256: `9f6b97caf15502c46336f71cc7af5b68f9aacefc313f395cdf366de9758c162a`.
- Portable SHA-256: `3c722a53cdd47e1fa37548726682a06c80df5e531d064ce9c102ecdbb46d4d96`.
- Materials SHA-256: `a53179947452befbacfdddf484de9dedbd324cce25391f023c79ba25a5902094`.

The downloaded Actions envelope matched GitHub's artifact digest. Its portable,
materials, file inventories and import closure were verified before native tests.
The running CLI reports `noh 0.1.1 (9a8ee751 clean; build ee69a7e676ea)`.
[Machine-readable evidence](release-evidence/windows-0.1.1.json) records the
artifact identity, results and limits.

All 321 application source files in `NOH-source.zip` match the compiled Git tree.
The ZIP timestamps differ between the hosted and local Git archive commands;
the file bytes and embedded commit identity agree. Compared with 0.1.0, 25,489
material files are unchanged. Only the application source archive, build
environment, PE report and materials README differ; no material files were
added or removed. Original bundled third-party licence files are unchanged.
The project's font notice differs only in line endings. See the independently
reviewed [redistribution record](WINDOWS_0_1_1_REDISTRIBUTION.md).

## Native results

The build's integration suite passed 477 tests with zero failures and 12 explicit
ignores. It exercises media/export and MCP with the packaged workers. The package
contains 265 PE images with no unresolved imports outside its recorded external
system/prerequisite boundary.

On Windows 10 Pro 22H2 x64, build 19045, with an RTX 4060 and the Microsoft runtime
already installed, fresh application configuration and a system-only PATH passed:

- Native preview playback with bundled libmpv, 294 observed frames and six
  play/pause/seek handoffs, including playback with a real audio device.
- 84 ready/resource-panel captures: seven languages, two sizes and three scales,
  dark theme. French, Japanese and German resource panels and the French ready
  screen were visually inspected.
- CPU-only and actual CUDA0 transcription of the known synthetic French speech,
  with the expected words recognized in both cases. No user media was used.

The native tools and models are byte-identical to the previously qualified
0.1.0 payload. The checks above use the new package and application identity;
the installer and its resource-recovery interaction require their own tests.

## Download protection and limits

The portable came from the successful GitHub Actions artifact. Windows Attachment
Services recorded its HTTPS origin and Internet zone for the nested portable and
extracted executable. Save returned `S_OK`; CheckPolicy returned `S_FALSE`, meaning
an interactive confirmation would be required. No such prompt was accepted.

Microsoft Defender scanned the portable with normal protection/remediation,
returned zero and reported no threats. The package hash remained unchanged.
Antivirus and real-time protection stayed enabled. The exact definition version
and evidence are in the JSON record. This is an Actions-download and native
policy check, not a browser or interactive SmartScreen reputation approval.

The application is unsigned. These results do not establish universal hardware
compatibility, a fresh Windows installation, fresh Microsoft prerequisite setup
or human listening. The portable's packaged guides reflect its build-source
snapshot; current online release instructions can be updated independently.
