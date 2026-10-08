# macOS and Linux update readiness

The shared signed metadata, target/channel selection, streaming download,
cancellation and independent archive verification are implemented. Native
installation is disabled on both platforms. Windows source checks do not compile
their `cfg` branches or establish native compatibility.

The existing native CI runner now builds with `gui,mcp,updates` and runs the
common updater tests within the media/MCP verification run. It provides native
compile feedback when those jobs run. These workflow
changes have not been executed on this Windows machine; even a successful native
CI run would not establish authenticated installation or recovery.

The inspected engine is Velopack 1.2.161, upstream revision
`92d6a1c91716729d449034df5c50307dcce39493`. These contracts prepare native
qualification; they do not authorize a release or replace its missing evidence.

| Contract | macOS | Linux |
| --- | --- | --- |
| Initial target | Apple Silicon / aarch64 | x86_64 |
| Engine channel | `osx-arm64-stable` or `osx-arm64-beta` | `linux-x64-stable` or `linux-x64-beta` |
| Signed NOH target | `macos` / `aarch64` | `linux` / `x86_64` |
| Application layout | Existing `NOH.app`: GUI/CLI/MCP in Contents/MacOS, portable runtime libraries in Contents/Frameworks, models in Contents/Resources/speech | Relocatable AppDir converted to one AppImage; no qualified NOH AppDir/AppImage packager exists yet |
| Current native packaging | Implemented by tools/package_macos.rs and tools/macos_runtime.rs | Missing |
| Baseline | Existing arm64 portable requires macOS 26.6.2, derived from bundled Mach-O requirements; this is not a broader compatibility promise | Distribution/glibc baseline unqualified; must be established from actual bundled ELF dependencies |
| Distribution signing | Existing ad-hoc integrity signature; Developer ID and notarization pending | Independent NOH envelope required; AppImage signing alone is not installer authentication |
| Authenticated native handoff | Designed, not implemented or qualified | Designed, not implemented or qualified |
| Production installation | Disabled | Disabled |

Other architectures require separate native packages and acceptance. Do not
advertise a universal macOS bundle or ARM Linux support from metadata parsing.

## Adapter contract before enabling installation

1. Resolve the actual running app bundle/AppImage and independent data/cache
   roots. Stage and retain signed full packages outside the replaced application.
   Authenticate identity, native target, channel, newer version and exact full
   bytes again in an external guardian before any engine extraction.
2. Establish an OS-native handoff preserving those authenticated bytes until the
   engine consumes them. Windows sharing handles are not a Unix solution. An
   open file descriptor alone does not make a separately reopened pathname
   immutable. Prove the actual consumer path or reject that integration.
3. Keep automatic SDK/startup cache application disabled. Fail closed on
   unsupported permissions and privilege escalation. The pinned Linux helper
   falls back from `mv` to `pkexec`; the macOS helper can invoke privileged
   `osascript`. Those paths are outside the nonprivileged qualification and
   cannot be silently enabled by launching the stock helper.
4. Account for the current pinned replacement behavior: macOS attempts an atomic
   bundle swap then falls back to two renames with restoration; Linux moves an
   extracted AppImage and may cross filesystems. Prove recovery under failure
   rather than inferring atomicity from the success path.
5. Verify installed signed members and actual version, then launch a fixed GUI
   path. Record installation, launch and readiness separately. Coordinate GUI,
   CLI and MCP lifetimes on these OSs before replacing native runtime libraries.
6. Retain a verified previous full package and a separately usable repair entry
   point; test it with the GUI unavailable. Define the space preflight from the
   native measured layout. Keep data and project files outside replacement.

These requirements follow the inspected apply_osx_impl.rs and
apply_linux_impl.rs paths. Neither stock entry point establishes the independent
NOH publisher signature policy. No maintained fork or replacement privileged
installer is approved by this contract.

## Commands to execute on each native host

First install the project's required native build dependencies and prepare the
verified media/speech inputs. Read BUILDING.md and the platform packaging
instructions before running these commands.

```sh
# Common update code, GUI and non-Windows guardian refusal branch.
cargo build --locked --offline --features gui,mcp,updates --bins

# Actual runtime integration on each available native host.
cargo dev verify --media --mcp --updates --offline
```

For macOS, separately build the existing self-contained application using
`cargo dev build --gui --mcp --portable --offline --ffmpeg <verified-path>
--speech <prepared-speech-folder>`. This ordinary packaging command does not
enable update installation. Check every nested Mach-O architecture, dependency
and minimum OS, relocation, final signature and later notarization on the real
bundle. Do not copy the Windows DLL/runtime tree into a Mac bundle.

For Linux, prepare real native FFmpeg, libmpv, Whisper and models, inventory their
licenses and ELF dependencies, and implement/qualify AppDir/AppImage packaging
before running an engine package command. There is deliberately no fictitious
Linux packaging success command here.

For each supported native target, execute signed A-to-B installation and repair
with invalid signature/target/version, changed bytes/path, cancellation, low
space, permission/locked file, concurrent media/CLI/MCP, abrupt helper/guardian
termination and failed relaunch cases. macOS also needs relocation/quarantine
and external-volume cases. Linux needs FUSE and extraction fallback, executable
permissions, mount/filesystem cases and actual X11/Wayland/audio/video checks on
every declared distribution baseline. Keep architecture-specific logs and final
bundle identities. Exclude targets without that evidence from release feeds.
