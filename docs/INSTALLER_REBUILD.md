# Rebuild the Windows installer with a modified archive library

The profile/maintenance installer sources ZIP includes the complete Inno Setup
7.1.0 source archive, its build scripts and precompiled objects, the is7z 26.02
source archive with Inno changes, original licences, the NOH installer scripts,
and `installer-extraction.lock.json` identifying both upstream revisions and hashes.
The matching NOH materials asset supplies the application sources, vendor tree,
native sources and build recipes; the portable asset supplies the application
payload wrapped by these scripts. These materials support modifying and rebuilding
the installer with a changed library under LGPL 2.1 section 6a. No restriction on
reverse engineering for debugging such modifications is imposed by NOH.

This is a reconstruction procedure, not a claim of a bit-for-bit reproduced
upstream DLL or installer. Upstream signing keys are neither supplied nor required.
Use your own isolated build directories and retain the original licence notices.
The official release pipeline continues to require its published input hashes.

1. Extract `innosetup-source.zip` and `is7z-source.zip`. Verify their SHA-256 values
   against `installer-extraction.lock.json`. Read each archive's `README.md` and
   build scripts, which document the toolchain requirements of these exact revisions.
   Inno's upstream build uses Delphi 12.3 Athens with its May patch; its Community
   Edition can build through the IDE as explained in `build-ce.bat`. The is7z
   scripts use Visual Studio 2022 C++ tools. Obtain any development tools under
   their own applicable terms; NOH does not redistribute those proprietary toolchains.
2. Modify the library's sources. In the is7z root, create `compilesettings.bat`
   with `VSBUILDROOT` pointing to your Visual Studio `VC\Auxiliary\Build` directory.
   Run `compile.bat x64`. The archive's script builds the Inno-compatible library
   in `CPP\7zip\Bundles\Format7zFInno\x64\7z.dll`.
3. Set up Inno's Delphi/GetIt dependencies as its README describes. Build the
   `Projects/ISSigTool.dproj` project first (or `compile.bat x64 ISSigTool` with
   command-line Delphi); this creates `Files/ISSigTool.exe` used by the next steps.
   Copy your DLL into the Inno source tree as `Files\is7z-x64.dll`. To let your
   compiler trust your changed library, use Inno's existing personal-key mechanism:
   configure `ISSIGTOOL_KEY_FILE` in `compilesettings.bat`, with the private key
   outside the source tree; run `issig.bat embed` and then
   `issig.bat sign Files\is7z-x64.dll`. The scripts generate a personal key when
   needed and embed its public half in `Components\TrustFunc.AllowedPublicKeys.inc`.
   Build the complete Release64 group in `Projects/Projects.groupproj` (or
   `compile.bat x64` with command-line Delphi), including ISCC, ISCmplr, ISPP,
   Setup and SetupLdr, with that public key. Follow the upstream signing steps
   for the rebuilt compiler modules and setup stubs. Retain verification of upstream files; do not
   disable Windows security settings or Inno's signature checks.
4. In your own NOH source copy, update `COMPILER` in `tools/public-installer.py`
   to the hashes of your rebuilt compiler/ISPP/library and adjust
   `assets/installer-extraction.lock.json` to your modified library/source material.
   This creates a separately identified derivative, not an official NOH release.
   These editable source checks prevent accidental mixing of inputs; they do not
   require a maintainer secret or prohibit library replacement.
5. From the matching verified portable, run `tools/public_components.py` to create
   the component ZIPs/catalog. Build the helper with `tools/public-installer.py
   --maintenance --profile minimal`, then the three offline profiles using
   `--helper <new-helper>`. Supply `--bundle`, `--commit`, `--components`,
   `--compiler` and separate `--output` paths as documented in [RELEASING.md](RELEASING.md).
   The source package includes these scripts; full paths to your extracted inputs
   are generated locally, so no maintainer filesystem path is required.
6. If distributing your derivative, provide its modified sources and build
   materials, update its download locations/catalog and notices, and test the
   resulting installations. Build a matching web selector from those new offline
   installers with `tools/installer/public-web.py`. Their hashes necessarily differ
   from official NOH artifacts; do not replace or relabel the originals.

The source archives include the code which embeds and loads the decoder, not
just a link to upstream. Inno's `Components/TrustFunc.pas`, `issig.bat` and
`compile.bat` show how its self-build keys and compiler work together. Neither
the official source archive nor NOH's installer imposes a private update-signing
key on this reconstruction.
