# Windows 0.1.0 installer qualification

The ordinary per-user installer wraps the already qualified Windows portable.
It adds the installation wizard, Start-menu shortcut and uninstaller. The
application is byte-identical to the [portable qualification](WINDOWS_0_1_0_QUALIFICATION.md).

## Exact identity

| Item | Identity |
| --- | --- |
| Application source and release tag | `df41ed864aef4aef4bf8df0da2460cd7173e2cf1` |
| Wrapper and workflow source | `156c9095bbcb6a8bbcce50ad9213e1863a518d56` |
| Hosted build | [Release 37901158103, attempt 1](https://github.com/Awalgawe/noh/actions/runs/37901158103), `stage=installer`, `target=windows-x64` |
| Installer | `NOH-0.1.0-windows-x64-Setup.exe`, 1,217,701,760 bytes |
| Installer SHA-256 | `30ad594726f4e598b155564cc38573f2c9c7d1e1209a87826c68bda1208b9165` |
| Wrapper sources SHA-256 | `72a713d663c7db9820a624a54d7cfa4d63b81403e69afbbed98c8429b4a502aa` |
| Portable manifest SHA-256 | `27afb3ce2f57a4f3d59e6afbee6354cc50c0c37a61cdf6c0e299e9ec11a04b6d` |

The compiler is pinned Inno Setup 7.1.0. The wrapper runs natively as x64; both
its PE header and the actual local installation log confirm this. The workflow
verifies the released portable/tag/manifest before packaging and retains an
unpublished review draft. The ordinary release receives those same tested bytes.
No application rebuild or tag movement accompanies the installer addition.

The release's `INSTALLER-PROVENANCE.json` binds the hosted and local evidence
hashes. The existing `-materials.zip` provides application/native/Rust sources
and notices; `-installer-sources.zip` adds the wrapper, translations, builder,
icon and licence files.

## Observed acceptance

On the disposable Windows Server 2025 runner:

- Silent installation and reinstallation into a path containing spaces passed
  in 90.512 and 89.663 seconds. All 568 portable files matched their expected hashes.
- The installed CLI passed. GUI startup reported the precise unavailable OpenGL
  2.0 renderer error; this is recorded as unavailable, not a pass.
- Uninstallation removed the registration and application files while preserving
  a user-created file.

On Windows 10 Pro 22H2 x64, build 19045:

- The exact hosted installer completed per-user installation in 86.360 seconds
  without administrator rights. The 567 manifest-listed payload files and the
  manifest itself matched the qualified portable.
- With a fresh application profile and a PATH containing only Windows system
  directories, the installed French GUI reached its ready state and produced a
  capture in 3.718 seconds. The capture was visually inspected. The installed
  CLI returned the original clean application identity.
- The Start-menu shortcut was created. Uninstallation removed it, the application
  files and the registration while preserving the test's user-created file.

The downloaded installer retained its SHA-256 after Attachment Services and
Microsoft Defender checks. Attachment Save returned zero and preserved Internet
zone 3; CheckPolicy returned 1 (a prompt would be required). Defender's explicit
scan returned zero with no threats in 16.385 seconds, no matching detections and
unchanged bytes. Antivirus and real-time protection remained enabled. An initial
scan command using forward slashes failed immediately; the native Windows path
succeeded. Neither failure nor retry is represented as a security exemption.

## Limits

- The local environment is an existing Windows host with a new application
  profile, not a freshly provisioned consumer OS.
- The exact hosted installer's interactive wizard could not be opened through
  the desktop tool: its launch timed out twice, and no wizard window appeared.
  The earlier local x86 wrapper was navigated to Ready and cancelled; it is not
  treated as interactive acceptance of this final x64 artifact.
- The x64 Microsoft runtime 14.51.36247.0 and all four required System32 DLLs
  were present. The corrected x64 wrapper and reviewed predicate support the
  expected page-skipping behavior, but that page transition was not observed on
  the final artifact. No fresh-system Microsoft download/install is claimed.
- Silent tests do not install the optional Microsoft prerequisite. The wizard
  downloads it directly from Microsoft, with a pinned hash and Microsoft's own
  licence/install UI; no Microsoft installer or DLL is redistributed by NOH.
- The installer and application are unsigned. These checks do not establish
  Authenticode identity, browser SmartScreen reputation or universal hardware
  compatibility. No security setting or protection was disabled.
- Media, audio, CPU/CUDA transcription and seven-language rendering results are
  reused from the byte-identical portable; they were not rerun for this wrapper.

See [Release readiness](RELEASING.md) for the standard build/review/publication
procedure and [installation instructions](INSTALLING.md) for users.
