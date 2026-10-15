//! Linux AppDir with declared host media dependencies and private CPU speech.
use super::{Result, hash_bundle, publish, query_build, same_build};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

pub(super) fn bundle(root: &Path, release: &Path, ffmpeg: &Path, speech: &Path) -> Result<()> {
    let mut preflight = Command::new("python3");
    preflight
        .arg(root.join("tools/linux_host.py"))
        .arg("--ffmpeg")
        .arg(ffmpeg);
    let output = crate::process::run(preflight, Duration::from_secs(90))?;
    if !output.status.success() {
        return Err(format!(
            "Linux host requirements failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let staging = tempfile::Builder::new()
        .prefix(".noh-linux-")
        .tempdir_in(&dist)?;
    let ready = staging.path().join("ready");
    let appdir = ready.join("NOH.AppDir");
    let home = appdir.join("usr/lib/noh");
    fs::create_dir_all(home.join("bin"))?;
    let executables = [
        ("noh-app", "noh-app"),
        ("noh", "bin/noh-cli"),
        ("noh-mcp", "bin/noh-mcp"),
    ];
    for (source, destination) in executables {
        fs::copy(release.join(source), home.join(destination))?;
        fs::set_permissions(home.join(destination), fs::Permissions::from_mode(0o755))?;
    }
    let build = query_build(&home.join("noh-app"))?;
    if build["target"] != "x86_64-unknown-linux-gnu" {
        return Err("Linux AppDir packaging requires native Linux x86_64 GNU executables.".into());
    }
    for (_, destination) in executables.iter().skip(1) {
        same_build(&build, &query_build(&home.join(destination))?, destination)?;
    }
    super::copy_docs(root, &home, true)?;
    fs::copy(root.join("assets/noh-icon.png"), appdir.join("noh.png"))?;
    fs::copy(root.join("assets/linux-AppRun"), appdir.join("AppRun"))?;
    fs::set_permissions(appdir.join("AppRun"), fs::Permissions::from_mode(0o755))?;
    fs::copy(root.join("assets/noh.desktop"), appdir.join("noh.desktop"))?;
    fs::copy(root.join("assets/linux-README.md"), ready.join("README.md"))?;
    let mut command = Command::new("python3");
    command
        .arg(root.join("tools/linux_package.py"))
        .arg("--appdir")
        .arg(&appdir)
        .arg("--speech")
        .arg(speech);
    let output = crate::process::run(command, Duration::from_secs(600))?;
    if !output.status.success() {
        return Err(format!(
            "Linux runtime packaging failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    // Re-query the relocated workers before sealing their final bytes.
    for (_, destination) in executables {
        same_build(&build, &query_build(&home.join(destination))?, destination)?;
    }
    let hashes = hash_bundle(&ready)?;
    fs::write(
        ready.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2, "package": "noh", "version": build["package_version"],
            "build": build, "runtime_dependencies": {
                "distribution": "linux-host-runtime", "architecture": "x86_64",
                "minimum_glibc": "2.39", "speech_backend": "cpu", "updates": false,
                "contract": "NOH.AppDir/usr/lib/noh/HOST-REQUIREMENTS.json"
            }, "tools": {"ffmpeg": {"provider": "host", "command": "ffmpeg"},
                         "libmpv": {"provider": "host", "soname": "libmpv.so.2"}},
            "sha256": hashes
        }))?,
    )?;
    publish(staging, &dist.join("NOH-linux-x64"))?;
    println!(
        "Linux AppDir: {}",
        dist.join("NOH-linux-x64/NOH.AppDir").display()
    );
    Ok(())
}
