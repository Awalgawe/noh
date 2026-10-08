//! Local and self-contained macOS application bundles.
use super::{Result, hash_bundle, publish, query_build, same_build};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command, time::Duration};
#[path = "macos_runtime.rs"]
mod runtime;

fn run(command: Command) -> Result<std::process::Output> {
    let description = format!("{command:?}");
    let output = crate::process::run(command, Duration::from_secs(30))?;
    if !output.status.success() {
        return Err(format!("{description}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output)
}

pub(super) fn bundle(
    root: &Path,
    release: &Path,
    ffmpeg: &Path,
    mcp: bool,
    portable: bool,
    speech: Option<&Path>,
) -> Result<()> {
    let ffmpeg = noh::inspection::resolve_ffmpeg(ffmpeg)?.canonicalize()?;
    let mut command = Command::new(&ffmpeg);
    command.arg("-version");
    let version = run(command)?;
    let ffmpeg_version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    if !ffmpeg_version.starts_with("ffmpeg version 7.1") {
        return Err("macOS packaging requires the qualified FFmpeg 7.1 runtime. Install brew install ffmpeg@7 or select it with --ffmpeg. FFmpeg 9 fails the export regressions.".into());
    }
    let mut command = Command::new(&ffmpeg);
    command.args(["-hide_banner", "-filters"]);
    let filters = run(command)?;
    if !String::from_utf8_lossy(&filters.stdout)
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some("subtitles"))
    {
        return Err("FFmpeg needs the subtitles filter (libass). Install brew install ffmpeg@7 or pass --ffmpeg with a complete native 7.1 build.".into());
    }

    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let staging = tempfile::Builder::new()
        .prefix(".noh-macos-")
        .tempdir_in(&dist)?;
    let ready = staging.path().join("ready");
    let app = if portable {
        ready.join("NOH.app")
    } else {
        ready.clone()
    };
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(macos.join("bin"))?;
    fs::create_dir_all(&resources)?;
    let mut executables = vec![("noh-app", "noh-app"), ("noh", "bin/noh-cli")];
    if mcp {
        executables.push(("noh-mcp", "bin/noh-mcp"));
    }
    for (source, destination) in &executables {
        fs::copy(release.join(source), macos.join(destination))?;
    }
    let build = query_build(&macos.join("noh-app"))?;
    let target = build["target"].as_str().ok_or("Missing build target")?;
    if target != format!("{}-apple-darwin", std::env::consts::ARCH) {
        return Err("Build a native macOS target before packaging; cross-compiled bundles are not qualified here.".into());
    }
    for (name, destination) in executables.iter().skip(1) {
        same_build(&build, &query_build(&macos.join(destination))?, name)?;
    }
    fs::copy(&ffmpeg, macos.join("bin/ffmpeg"))?;
    let mut command = Command::new(macos.join("bin/ffmpeg"));
    command.arg("-version");
    run(command)?;
    super::copy_docs(root, &resources, mcp)?;
    let speech_source = speech
        .map(Path::to_owned)
        .unwrap_or_else(|| root.join(".mcp-dev/macos-speech"));
    let speech_details = if portable {
        Some(copy_speech(&speech_source, &resources.join("speech"))?)
    } else {
        None
    };
    let portable_runtime = if portable {
        let mut roots: Vec<_> = executables
            .iter()
            .map(|(source, destination)| (release.join(source), macos.join(destination)))
            .collect();
        roots.push((ffmpeg.clone(), macos.join("bin/ffmpeg")));
        roots.push((
            speech_source.join("whisper-cli"),
            macos.join("bin/whisper-cli"),
        ));
        let details = runtime::embed(&app, &roots)?;
        fs::write(
            resources.join("PORTABLE-RUNTIME.json"),
            serde_json::to_vec_pretty(&details)?,
        )?;
        Some(details)
    } else {
        None
    };
    let portable_readme = portable_runtime.as_ref().map(|details| format!(
        "# NOH portable for macOS\n\nUnzip and open NOH.app. You may move the app to any folder or external disk.\nNo Homebrew, Rust, Python or media-tool installation is needed for editing,\npreview, playback, transcription and export. FFmpeg, libmpv, Whisper and their\nnative libraries are included, together with the multilingual Small model and\nSilero VAD model. The GUI detects the speech resources automatically.\nTranscription works offline, using Metal on supported Macs or the CPU.\n\nArchitecture: {}. Minimum macOS: {} (derived from all bundled Mach-O files).\nThis app has an ad-hoc integrity signature, not a Developer ID signature or\nApple notarization. It is a private portable build, not a public release.\n\nCLI: NOH.app/Contents/MacOS/bin/noh-cli\nMCP: NOH.app/Contents/MacOS/bin/noh-mcp (when requested at build time)\nWhisper: NOH.app/Contents/MacOS/bin/whisper-cli\nSpeech models: NOH.app/Contents/Resources/speech/\n\nRuntime provenance and third-party notices: NOH.app/Contents/Resources/ and\nthe speech folder. Full signed-file checksums: manifest.json alongside the app.\n",
        details["architecture"].as_str().unwrap_or("unknown"),
        details["minimum_macos"].as_str().unwrap_or("unknown")
    ));
    fs::write(
        resources.join("README.md"),
        portable_readme.as_deref().unwrap_or("# NOH for macOS\n\nOpen NOH.app in Finder. Keep ffmpeg@7 and mpv installed with Homebrew.\n\nThis local application is built for the native Mac architecture. It is not a\nself-contained, Developer ID-signed or notarized distribution. FFmpeg is copied\nwith its existing dynamic-library dependencies; libmpv is discovered locally.\n\nCLI: Contents/MacOS/bin/noh-cli\nMCP (when included): Contents/MacOS/bin/noh-mcp\n\nConfigure native whisper-cli and local models in Settings for transcription.\nSee docs/README.md for setup and verification.\n"),
    )?;
    let mut command = Command::new(macos.join("bin/ffmpeg"));
    command.args(["-hide_banner", "-L"]);
    let license = run(command)?;
    let mut license_bytes = license.stdout;
    license_bytes.extend(license.stderr);
    fs::write(resources.join("licenses/FFmpeg-LICENSE.txt"), license_bytes)?;
    icon(root, staging.path(), &resources.join("noh.icns"))?;
    let mut plist =
        include_str!("../assets/macos-Info.plist").replace("@VERSION@", env!("CARGO_PKG_VERSION"));
    if let Some(details) = &portable_runtime {
        plist = plist.replace("    <key>NSHighResolutionCapable", &format!(
            "    <key>LSMinimumSystemVersion</key><string>{}</string>\n    <key>NSHighResolutionCapable",
            details["minimum_macos"].as_str().ok_or("Missing minimum macOS")?
        ));
    }
    fs::write(contents.join("Info.plist"), plist)?;
    let mut command = Command::new("/usr/bin/plutil");
    command.args(["-lint"]).arg(contents.join("Info.plist"));
    run(command)?;
    if let Some(readme) = &portable_readme {
        fs::write(ready.join("README.md"), readme)?;
        // Seal resources last. The full hash manifest lives outside the signed
        // app to avoid a circular dependency with the main executable signature.
        runtime::sign(&app)?;
        let mut command = Command::new("/usr/bin/codesign");
        command.args(["--verify", "--deep", "--strict"]).arg(&app);
        run(command)?;
    }
    let hashes = hash_bundle(&ready)?;
    let ffmpeg_key = if portable {
        "NOH.app/Contents/MacOS/bin/ffmpeg"
    } else {
        "Contents/MacOS/bin/ffmpeg"
    };
    fs::write(
        if portable {
            ready.join("manifest.json")
        } else {
            resources.join("manifest.json")
        },
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2, "package": "noh", "version": build["package_version"],
            "build": build,
            "runtime_dependencies": {
                "distribution": if portable { "portable-macos" } else { "local-macos" },
                "homebrew_required": !portable,
                "portable": portable_runtime,
                "speech": speech_details
            },
            "tools": {"ffmpeg": {
                "path": ffmpeg_key, "source": ffmpeg,
                "details": ffmpeg_version, "sha256": hashes[ffmpeg_key]
            }},
            "sha256": hashes
        }))?,
    )?;
    if portable {
        let architecture = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            std::env::consts::ARCH
        };
        let name = format!("NOH-macos-{architecture}");
        let archive = tempfile::Builder::new()
            .prefix(".noh-archive-")
            .suffix(".zip")
            .tempfile_in(&dist)?;
        let mut command = Command::new("/usr/bin/ditto");
        command
            .args(["-c", "-k", "--sequesterRsrc"])
            .arg(&ready)
            .arg(archive.path());
        // Model weights dominate the archive; compression can exceed the
        // short timeout used for runtime probes and signing commands.
        let output = crate::process::run(command, Duration::from_secs(600))?;
        if !output.status.success() {
            return Err(format!(
                "Archive creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        publish(staging, &dist.join(&name))?;
        archive.persist(dist.join(format!("{name}.zip")))?;
        println!(
            "Portable macOS application: {}\nArchive: {}",
            dist.join(&name).display(),
            dist.join(format!("{name}.zip")).display()
        );
    } else {
        publish(staging, &dist.join("NOH.app"))?;
        println!(
            "macOS application: {} (requires local Homebrew media libraries)",
            dist.join("NOH.app").display()
        );
    }
    Ok(())
}

fn copy_speech(source: &Path, destination: &Path) -> Result<serde_json::Value> {
    let manifest: serde_json::Value =
        serde_json::from_slice(include_bytes!("../assets/speech-bundle.json"))?;
    let provenance = source.join("WHISPER-BUILD.json");
    let build: serde_json::Value = serde_json::from_slice(&fs::read(&provenance).map_err(|error| {
        format!("Missing native speech bundle: {error}. Run python3 tools/prepare-macos-speech.py, or pass --speech with its output folder. The existing portable is unchanged.")
    })?)?;
    let executable = fs::read(source.join("whisper-cli"))?;
    if build["executable_sha256"].as_str()
        != Some(format!("{:x}", Sha256::digest(&executable)).as_str())
    {
        return Err("Native Whisper checksum mismatch; prepare the speech bundle again.".into());
    }
    fs::create_dir_all(destination)?;
    for name in [
        "ggml-small.bin",
        "ggml-silero-v6.2.0.bin",
        "WHISPER-MODEL-LICENSE.txt",
        "SILERO-LICENSE.txt",
    ] {
        let bytes = fs::read(source.join(name))?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if manifest["sha256"][name].as_str() != Some(&actual) {
            return Err(format!(
                "Speech checksum mismatch: {name}; prepare the speech bundle again."
            )
            .into());
        }
        fs::write(destination.join(name), bytes)?;
    }
    for name in ["WHISPER-CPP-LICENSE.txt", "WHISPER-BUILD.json"] {
        fs::copy(source.join(name), destination.join(name))?;
    }
    let details = serde_json::json!({
        "runtime": build, "executable": "Contents/MacOS/bin/whisper-cli",
        "models": "Contents/Resources/speech",
        "model": manifest["model"], "vad": manifest["vad"],
        "model_sha256": manifest["sha256"]["ggml-small.bin"],
        "vad_sha256": manifest["sha256"]["ggml-silero-v6.2.0.bin"],
        "offline": true
    });
    fs::write(
        destination.join("SPEECH-BUNDLE.json"),
        serde_json::to_vec_pretty(&details)?,
    )?;
    Ok(details)
}

fn icon(root: &Path, staging: &Path, destination: &Path) -> Result<()> {
    let iconset = staging.join("noh.iconset");
    fs::create_dir(&iconset)?;
    for size in [16, 32, 128, 256, 512] {
        for scale in [1, 2] {
            let pixels = (size * scale).to_string();
            let suffix = if scale == 2 { "@2x" } else { "" };
            let mut command = Command::new("/usr/bin/sips");
            command
                .args(["-z", &pixels, &pixels])
                .arg(root.join("assets/noh-icon.png"))
                .arg("--out")
                .arg(iconset.join(format!("icon_{size}x{size}{suffix}.png")));
            run(command)?;
        }
    }
    let mut command = Command::new("/usr/bin/iconutil");
    command
        .args(["-c", "icns"])
        .arg(iconset)
        .arg("-o")
        .arg(destination);
    run(command)?;
    Ok(())
}
