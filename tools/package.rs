use super::Result;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[cfg(target_os = "macos")]
#[path = "package_macos.rs"]
mod macos;

// These two runtime descriptions are embedded in the helper. Explicit-root
// Windows packaging may reuse it only when the selected tree has the same bytes.
pub fn validate_source_root(root: &Path) -> Result<()> {
    for (name, embedded) in [
        (
            "assets/speech-bundle.json",
            include_bytes!("../assets/speech-bundle.json").as_slice(),
        ),
        (
            "assets/preview-runtime.json",
            include_bytes!("../assets/preview-runtime.json").as_slice(),
        ),
    ] {
        if fs::read(root.join(name))? != embedded {
            return Err(format!("Source root differs from the helper's embedded {name}; rebuild the helper from that tree.").into());
        }
    }
    Ok(())
}

fn query_build(executable: &Path) -> Result<serde_json::Value> {
    let mut command = Command::new(executable);
    command.arg("--build-info");
    let output = super::process::run(command, Duration::from_secs(15))?;
    if !output.status.success() {
        return Err(format!("Build identity query failed for {}", executable.display()).into());
    }
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Invalid build identity from {}: {e}", executable.display()))?;
    validate_build(&info)?;
    Ok(info)
}

fn validate_build(info: &serde_json::Value) -> Result<()> {
    let hash = |field: &str| {
        info[field]
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    if info["schema_version"] != 1
        || !info["package_version"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        || !hash("source_sha256")
        || !hash("build_fingerprint")
        || !hash("options_sha256")
        || !info["target"].as_str().is_some_and(|s| !s.is_empty())
        || !info["profile"].as_str().is_some_and(|s| !s.is_empty())
        || !info["features"]
            .as_array()
            .is_some_and(|a| a.iter().all(serde_json::Value::is_string))
    {
        return Err(
            "Incomplete or unsupported binary build identity; rebuild the executables.".into(),
        );
    }
    Ok(())
}

fn same_build(expected: &serde_json::Value, actual: &serde_json::Value, name: &str) -> Result<()> {
    validate_build(actual)?;
    if actual != expected {
        return Err(format!("Mismatched build identity for {name}; rebuild all requested binaries together. The previous distribution is unchanged.").into());
    }
    Ok(())
}

pub fn bundle(
    root: &Path,
    release: &Path,
    ffmpeg: &Path,
    gui: bool,
    mcp: bool,
    updates: bool,
    speech: Option<&Path>,
    portable: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if updates {
            return Err("Update-enabled macOS bundles are not qualified.".into());
        }
        if !gui {
            return Err("macOS packaging requires --gui.".into());
        }
        return macos::bundle(root, release, ffmpeg, mcp, portable, speech);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = portable;
        bundle_windows(root, release, ffmpeg, gui, mcp, updates, speech)
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn bundle_windows(
    root: &Path,
    release: &Path,
    ffmpeg: &Path,
    gui: bool,
    mcp: bool,
    updates: bool,
    speech: Option<&Path>,
) -> Result<()> {
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let staging = tempfile::Builder::new()
        .prefix(".noh-package-")
        .tempdir_in(&dist)?;
    let ready = staging.path().join("ready");
    fs::create_dir(&ready)?;
    for folder in ["bin", "docs", "licenses"] {
        fs::create_dir(ready.join(folder))?;
    }
    if gui {
        fs::copy(root.join("assets/noh-icon.ico"), ready.join("noh.ico"))?;
        copy_speech(speech.unwrap_or(&dist.join("noh")), &ready)?;
        let runtime = std::env::var_os("NOH_LIBMPV")
            .map(PathBuf::from)
            .unwrap_or_else(|| dist.join("noh/bin/preview/libmpv-2.dll"));
        copy_preview(root, &runtime, &ready)?;
    }
    let mut executables = vec![("noh.exe", "bin/noh-cli.exe")];
    if gui {
        executables.push(("noh-app.exe", "noh.exe"));
    }
    if mcp {
        executables.push(("noh-mcp.exe", "bin/noh-mcp.exe"));
    }
    if updates {
        executables.push(("noh-update-guard.exe", "bin/noh-update-guard.exe"));
        executables.push(("noh-update-repair.exe", "bin/noh-update-repair.exe"));
    }
    for (name, destination) in &executables {
        fs::copy(release.join(name), ready.join(destination))?;
    }
    // Read the staged binaries, not this development tool's debug build metadata.
    let build = query_build(&ready.join("bin/noh-cli.exe"))?;
    for (name, destination) in executables.iter().skip(1) {
        same_build(&build, &query_build(&ready.join(destination))?, name)?;
    }
    // PATH resolution is left to the OS; copying requires an actual resolved path.
    let ffmpeg = if ffmpeg.is_file() {
        ffmpeg.to_path_buf()
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(ffmpeg).with_extension("exe"))
            .find(|p| p.is_file())
            .ok_or("FFmpeg not found; pass --ffmpeg with its full path")?
    };
    let native = native_windows_lock(root)?;
    copy_native_role(
        ffmpeg.parent().ok_or("FFmpeg has no parent directory")?,
        &ready.join("bin"),
        &native["roles"]["export"],
    )?;
    let native_notices = std::env::var_os("NOH_NATIVE_NOTICES")
        .map(PathBuf::from)
        .unwrap_or_else(|| dist.join("noh/licenses/native"));
    verify_native_notices(
        &native_notices,
        &format!(
            "{:x}",
            Sha256::digest(fs::read(root.join("assets/windows-native.lock.json"))?)
        ),
    )?;
    copy_native_notice_tree(&native_notices, &ready.join("licenses/native"))?;
    copy_docs(root, &ready, mcp)?;
    fs::copy(
        root.join("assets/portable-README.md"),
        ready.join("README.md"),
    )?;
    let mut command = Command::new(ready.join("bin/ffmpeg.exe"));
    command.args(["-hide_banner", "-L"]);
    let license = super::process::run(command, Duration::from_secs(15))?;
    if !license.status.success() {
        return Err("FFmpeg license query failed".into());
    }
    let mut bytes = license.stdout;
    bytes.extend(license.stderr);
    fs::write(ready.join("licenses/FFmpeg-LICENSE.txt"), bytes)?;
    let mut command = Command::new(ready.join("bin/ffmpeg.exe"));
    command.arg("-version");
    let version = super::process::run(command, Duration::from_secs(15))?;
    if !version.status.success() {
        return Err("FFmpeg version query failed".into());
    }
    let ffmpeg_version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    if !ffmpeg_version.starts_with("ffmpeg version ") {
        return Err("FFmpeg returned no recognizable version identity.".into());
    }
    let hashes = hash_bundle(&ready)?;
    fs::write(
        ready.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2, "package": "noh", "version": build["package_version"],
            "build": build,
            "tools": {"ffmpeg": {"path": "bin/ffmpeg.exe", "version": ffmpeg_version.lines().next(), "details": ffmpeg_version, "sha256": hashes["bin/ffmpeg.exe"]}},
            "sha256": hashes
        }))?,
    )?;
    publish(staging, &dist.join("noh"))?;
    println!("Portable build: {}", dist.join("noh").display());
    Ok(())
}

fn copy_docs(root: &Path, ready: &Path, _mcp: bool) -> Result<()> {
    fs::create_dir_all(ready.join("docs"))?;
    fs::create_dir_all(ready.join("licenses"))?;
    // Keep packaged guides limited to the documented allowlist.
    fs::write(
        ready.join("docs/README.md"),
        "# NOH guides\n\n[Installation](INSTALLING.md) · [Getting started](APP.md) · [Command line](CLI.md) · [MCP](MCP.md)\n\n[Build from source](BUILDING.md) · [Development](DEVELOPMENT.md) · [Third-party notices](THIRD-PARTY.md)\n\nNOH's code is licensed under GNU GPLv3 (GPL-3.0-only). The full text is in ../licenses/NOH-LICENSE.txt.\n",
    )?;
    for name in [
        "APP.md",
        "INSTALLING.md",
        "CLI.md",
        "BUILDING.md",
        "DEVELOPMENT.md",
        "ARCHITECTURE.md",
        "RELEASING.md",
        "MACOS_SIGNING.md",
        "MCP.md",
        "SHORT_PRESETS.md",
        "PROJECT_EXPORT.md",
        "THIRD-PARTY.md",
        "UPDATES.md",
        "UPDATE_RECOVERY.md",
        "UPDATE_RELEASE.md",
        "UPDATE_NATIVE_GATES.md",
    ] {
        fs::copy(root.join("docs").join(name), ready.join("docs").join(name))?;
    }
    fs::copy(root.join("LICENSE"), ready.join("licenses/NOH-LICENSE.txt"))?;
    fs::copy(
        root.join("assets/OFL.txt"),
        ready.join("licenses/FONT-LICENSE.txt"),
    )?;
    fs::copy(
        root.join("assets/NotoSans-OFL.txt"),
        ready.join("licenses/FONT-NOTO-SANS-LICENSE.txt"),
    )?;
    fs::copy(
        root.join("assets/Jost-OFL.txt"),
        ready.join("licenses/FONT-JOST-LICENSE.txt"),
    )?;
    fs::copy(
        root.join("assets/NOTICE.md"),
        ready.join("licenses/FONT-NOTICE.md"),
    )?;
    if let Some(source) = std::env::var_os("NOH_RUST_NOTICES") {
        copy_rust_notices(Path::new(&source), ready)?;
    }
    Ok(())
}

fn copy_rust_notices(source: &Path, ready: &Path) -> Result<()> {
    let notice = fs::read(source.join("RUST-NOTICES.txt"))?;
    let inventory = fs::read(source.join("RUST-NOTICE-INVENTORY.json"))?;
    let report: serde_json::Value = serde_json::from_slice(&inventory)?;
    let checksum = format!("{:x}", Sha256::digest(&notice));
    if report["schema_version"] != 1
        || report["status"] != "unreviewed-notice-materials"
        || report["notice_sha256"].as_str() != Some(checksum.as_str())
    {
        return Err("Rust notice input does not match its inventory".into());
    }
    let standard_library = if let Some(record) = report.get("standard_library") {
        if record["status"] != "unreviewed-original-standard-library-notice"
            || record["path"] != "RUST-STANDARD-LIBRARY-COPYRIGHT.html"
        {
            return Err("Unexpected standard-library notice input".into());
        }
        let data = fs::read(source.join("RUST-STANDARD-LIBRARY-COPYRIGHT.html"))?;
        let checksum = format!("{:x}", Sha256::digest(&data));
        if record["sha256"].as_str() != Some(checksum.as_str()) {
            return Err("Standard-library notice input does not match its inventory".into());
        }
        Some(data)
    } else {
        None
    };
    let destination = ready.join("licenses/rust");
    fs::create_dir(&destination)?;
    fs::write(destination.join("RUST-NOTICES.txt"), notice)?;
    fs::write(destination.join("RUST-NOTICE-INVENTORY.json"), inventory)?;
    if let Some(data) = standard_library {
        fs::write(
            destination.join("RUST-STANDARD-LIBRARY-COPYRIGHT.html"),
            data,
        )?;
    }
    Ok(())
}

fn copy_speech(source: &Path, destination: &Path) -> Result<()> {
    let manifest = include_bytes!("../assets/speech-bundle.json");
    let value: serde_json::Value = serde_json::from_slice(manifest)?;
    copy_speech_files(source, destination, &value)?;
    fs::write(destination.join("bin/speech/SPEECH-BUNDLE.json"), manifest)?;
    Ok(())
}

fn copy_preview(root: &Path, runtime: &Path, destination: &Path) -> Result<()> {
    let manifest = include_bytes!("../assets/preview-runtime.json");
    let value: serde_json::Value = serde_json::from_slice(manifest)?;
    let bytes=fs::read(runtime).map_err(|e|format!("Missing qualified preview runtime: {e}. Set NOH_LIBMPV to the DLL recorded in assets/preview-runtime.json; the existing portable is unchanged."))?;
    verify_preview_hash(&bytes, &value)?;
    let native = native_windows_lock(root)?;
    copy_native_role(
        runtime
            .parent()
            .ok_or("Preview runtime has no parent directory")?,
        &destination.join("bin/preview"),
        &native["roles"]["preview"],
    )?;
    fs::write(destination.join("bin/PREVIEW-RUNTIME.json"), manifest)?;
    Ok(())
}
fn native_windows_lock(root: &Path) -> Result<serde_json::Value> {
    let spec: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("assets/delivery-windows.lock.json"))?)?;
    let bytes = fs::read(root.join("assets/windows-native.lock.json"))?;
    if spec["native_runtime"]["sha256"].as_str() != Some(&format!("{:x}", Sha256::digest(&bytes))) {
        return Err("Native runtime lock hash mismatch".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn copy_native_role(source: &Path, destination: &Path, files: &serde_json::Value) -> Result<()> {
    fs::create_dir_all(destination)?;
    for (name, item) in files.as_object().ok_or("Missing native file inventory")? {
        if name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            return Err("Unsafe native filename".into());
        }
        let path = source.join(name);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err("Native input symlinks are not accepted".into());
        }
        let bytes = fs::read(path)?;
        if item["sha256"].as_str() != Some(&format!("{:x}", Sha256::digest(&bytes))) {
            return Err(format!("Native file checksum mismatch: {name}").into());
        }
        fs::write(destination.join(name), bytes)?;
    }
    Ok(())
}
fn copy_native_notice_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_native_notice_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err("Native notice links/special files are not accepted".into());
        }
    }
    Ok(())
}
fn verify_native_notices(source: &Path, native_lock_sha256: &str) -> Result<()> {
    let provenance: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join("PROVENANCE.json"))?)?;
    if provenance["native_lock_sha256"].as_str() != Some(native_lock_sha256)
        || provenance["provider"] != "MSYS2"
        || provenance["status"] != "unreviewed-original-package-notices"
    {
        return Err("Native notices do not match the selected runtime lock".into());
    }
    let expected = provenance["files"]
        .as_object()
        .ok_or("Missing native notice inventory")?;
    let mut actual = hash_bundle(source)?;
    actual.remove("PROVENANCE.json");
    if actual.len() != expected.len() || actual.is_empty() {
        return Err("Native notice inventory coverage mismatch".into());
    }
    for (name, hash) in actual {
        if expected[&name]["sha256"] != hash {
            return Err(format!("Native notice checksum mismatch: {name}").into());
        }
    }
    Ok(())
}
fn verify_preview_hash(bytes: &[u8], manifest: &serde_json::Value) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if manifest["sha256"].as_str() != Some(&actual) {
        return Err("Preview runtime hash mismatch; supply the locked producer input.".into());
    }
    Ok(())
}

fn copy_speech_files(
    source: &Path,
    destination: &Path,
    manifest: &serde_json::Value,
) -> Result<()> {
    let files = manifest["sha256"]
        .as_object()
        .ok_or("Missing speech bundle hashes")?;
    if files.is_empty() {
        return Err("Empty speech bundle".into());
    }
    let organized = source.join("bin/speech").is_dir();
    for (name, expected) in files {
        let path = Path::new(name);
        if path.components().count() != 1
            || path.file_name().is_none_or(|file| file != name.as_str())
        {
            return Err("Speech bundle paths must be simple filenames".into());
        }
        let relative = if name.ends_with("-LICENSE.txt") {
            Path::new("licenses").join(path)
        } else {
            Path::new("bin/speech").join(path)
        };
        let source_file = source.join(if organized { &relative } else { path });
        let bytes = fs::read(source_file).map_err(|error| {
            format!("Missing speech bundle file {name}: {error}. Supply the verified local files with --speech; the existing portable is unchanged.")
        })?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        if expected.as_str() != Some(&hash) {
            return Err(format!("Speech bundle hash mismatch: {name}").into());
        }
        let target = destination.join(relative);
        fs::create_dir_all(
            target
                .parent()
                .ok_or("Missing speech destination directory")?,
        )?;
        fs::write(target, bytes)?;
    }
    Ok(())
}

/// Collect ordinary files only; do not follow links out of a portable folder.
fn bundle_files(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            } else {
                return Err(
                    format!("Unsupported portable entry: {}", entry.path().display()).into(),
                );
            }
        }
    }
    files.sort();
    Ok(files)
}

fn hash_bundle(folder: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut hashes = serde_json::Map::new();
    for path in bundle_files(folder)? {
        let relative = path
            .strip_prefix(folder)?
            .to_string_lossy()
            .replace('\\', "/");
        hashes.insert(
            relative,
            format!("{:x}", Sha256::digest(fs::read(path)?)).into(),
        );
    }
    Ok(hashes)
}

fn publish(staging: tempfile::TempDir, destination: &Path) -> Result<()> {
    let ready = staging.path().join("ready");
    let previous = staging.path().join("previous");
    // Detect Windows image/file locks before moving either version.
    #[cfg(windows)]
    if destination.exists() {
        use std::os::windows::fs::OpenOptionsExt;
        for path in bundle_files(destination)? {
            fs::OpenOptions::new().read(true).write(true).share_mode(0).open(&path)
                .map_err(|e|format!("Cannot update {}: {e}. Close NOH and retry; the previous distribution is unchanged.",path.display()))?;
        }
    }
    let had_previous = destination.exists();
    if had_previous {
        fs::rename(destination, &previous)?;
    }
    if let Err(error) = fs::rename(&ready, destination) {
        if had_previous && let Err(restore) = fs::rename(&previous, destination) {
            let recovery = staging.keep();
            return Err(format!("Publish failed: {error}; restoration failed: {restore}. Previous files retained in {}",recovery.display()).into());
        }
        return Err(error.into());
    }
    staging.close()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_source_root_rejects_changed_embedded_runtime_inputs() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("assets")).unwrap();
        let inputs = [
            (
                "assets/speech-bundle.json",
                include_bytes!("../assets/speech-bundle.json").as_slice(),
            ),
            (
                "assets/preview-runtime.json",
                include_bytes!("../assets/preview-runtime.json").as_slice(),
            ),
        ];
        for (name, bytes) in inputs {
            fs::write(root.path().join(name), bytes).unwrap();
        }
        validate_source_root(root.path()).unwrap();
        for (name, bytes) in inputs {
            fs::write(root.path().join(name), b"{}").unwrap();
            assert!(validate_source_root(root.path()).is_err());
            fs::write(root.path().join(name), bytes).unwrap();
        }
    }

    #[test]
    fn rust_notices_are_checked_before_packaged_inventory_and_signing() {
        let source = tempfile::tempdir().unwrap();
        let ready = tempfile::tempdir().unwrap();
        fs::create_dir(ready.path().join("licenses")).unwrap();
        let notice = b"Original dependency terms and attribution\n";
        let standard_library = b"<html>Original standard-library terms</html>\n";
        fs::write(source.path().join("RUST-NOTICES.txt"), notice).unwrap();
        fs::write(
            source.path().join("RUST-STANDARD-LIBRARY-COPYRIGHT.html"),
            standard_library,
        )
        .unwrap();
        fs::write(
            source.path().join("RUST-NOTICE-INVENTORY.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "status": "unreviewed-notice-materials",
                "notice_sha256": format!("{:x}", Sha256::digest(notice)),
                "standard_library": {
                    "status": "unreviewed-original-standard-library-notice",
                    "path": "RUST-STANDARD-LIBRARY-COPYRIGHT.html",
                    "sha256": format!("{:x}", Sha256::digest(standard_library)),
                },
            }))
            .unwrap(),
        )
        .unwrap();
        copy_rust_notices(source.path(), ready.path()).unwrap();
        assert_eq!(
            fs::read(ready.path().join("licenses/rust/RUST-NOTICES.txt")).unwrap(),
            notice
        );
        assert_eq!(
            fs::read(
                ready
                    .path()
                    .join("licenses/rust/RUST-STANDARD-LIBRARY-COPYRIGHT.html")
            )
            .unwrap(),
            standard_library
        );
        let changed_library = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("RUST-STANDARD-LIBRARY-COPYRIGHT.html"),
            b"altered library terms",
        )
        .unwrap();
        assert!(copy_rust_notices(source.path(), changed_library.path()).is_err());
        assert!(!changed_library.path().join("licenses/rust").exists());
        let altered = tempfile::tempdir().unwrap();
        fs::write(source.path().join("RUST-NOTICES.txt"), b"altered terms").unwrap();
        assert!(copy_rust_notices(source.path(), altered.path()).is_err());
        assert!(!altered.path().join("licenses/rust").exists());
    }
    use super::*;
    #[test]
    fn native_roles_keep_conflicting_dlls_isolated_and_reject_changed_inputs() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        for (role, bytes) in [
            ("export", b"export".as_slice()),
            ("preview", b"preview".as_slice()),
        ] {
            fs::create_dir(source.path().join(role)).unwrap();
            fs::write(source.path().join(role).join("same.dll"), bytes).unwrap();
            let files =
                serde_json::json!({"same.dll": {"sha256": format!("{:x}",Sha256::digest(bytes))}});
            copy_native_role(&source.path().join(role), &output.path().join(role), &files).unwrap();
            assert_eq!(
                fs::read(output.path().join(role).join("same.dll")).unwrap(),
                bytes
            );
            fs::write(source.path().join(role).join("same.dll"), b"changed").unwrap();
            assert!(
                copy_native_role(
                    &source.path().join(role),
                    &output.path().join("changed"),
                    &files
                )
                .is_err()
            );
        }
        assert!(
            copy_native_role(
                source.path(),
                output.path(),
                &serde_json::json!({"../escape.dll": {}})
            )
            .is_err()
        );
        assert!(
            copy_native_role(
                source.path(),
                output.path(),
                &serde_json::json!({"missing.dll": {}})
            )
            .is_err()
        );
    }
    #[test]
    fn native_notices_require_exact_inventory_and_original_bytes() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("LICENSE"), b"original terms").unwrap();
        fs::write(
            source.path().join("PROVENANCE.json"),
            serde_json::to_vec(&serde_json::json!({
                "provider":"MSYS2", "status":"unreviewed-original-package-notices", "native_lock_sha256":"locked", "files": {
                "LICENSE": {"sha256":format!("{:x}",Sha256::digest(b"original terms"))}
            }}))
            .unwrap(),
        )
        .unwrap();
        verify_native_notices(source.path(), "locked").unwrap();
        assert!(verify_native_notices(source.path(), "another lock").is_err());
        fs::write(source.path().join("extra"), b"extra").unwrap();
        assert!(verify_native_notices(source.path(), "locked").is_err());
        fs::remove_file(source.path().join("extra")).unwrap();
        fs::write(source.path().join("LICENSE"), b"altered terms").unwrap();
        assert!(verify_native_notices(source.path(), "locked").is_err());
    }
    #[test]
    fn packaged_docs_include_only_public_guides() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let output = tempfile::tempdir().unwrap();
        copy_docs(root, output.path(), false).unwrap();
        let mut names: Vec<_> = fs::read_dir(output.path().join("docs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 17);
        assert!(names.contains(&"APP.md".into()));
        assert!(names.contains(&"CLI.md".into()));
        assert_eq!(
            names,
            [
                "APP.md",
                "ARCHITECTURE.md",
                "BUILDING.md",
                "CLI.md",
                "DEVELOPMENT.md",
                "INSTALLING.md",
                "MACOS_SIGNING.md",
                "MCP.md",
                "PROJECT_EXPORT.md",
                "README.md",
                "RELEASING.md",
                "SHORT_PRESETS.md",
                "THIRD-PARTY.md",
                "UPDATES.md",
                "UPDATE_NATIVE_GATES.md",
                "UPDATE_RECOVERY.md",
                "UPDATE_RELEASE.md",
            ]
        );
        let index = fs::read_to_string(output.path().join("docs/README.md")).unwrap();
        assert!(index.contains("](APP.md)"));
        assert!(index.contains("](INSTALLING.md)"));
        assert!(index.contains("](CLI.md)"));
        assert!(!index.contains("docs/images"));
        assert_eq!(
            fs::read(output.path().join("licenses/NOH-LICENSE.txt")).unwrap(),
            fs::read(root.join("LICENSE")).unwrap()
        );
    }
    #[test]
    fn preview_runtime_requires_the_qualified_hash() {
        let manifest =
            serde_json::json!({"sha256":format!("{:x}",Sha256::digest(b"qualified LGPL library"))});
        assert!(verify_preview_hash(b"qualified LGPL library", &manifest).is_ok());
        assert!(verify_preview_hash(b"different build", &manifest).is_err());
        assert!(verify_preview_hash(b"qualified LGPL library", &serde_json::json!({})).is_err());
    }
    #[test]
    fn speech_packaging_rejects_missing_changed_and_escaping_files() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let hash = format!("{:x}", Sha256::digest(b"qualified"));
        let manifest = serde_json::json!({"sha256":{"model.bin":hash}});
        assert!(copy_speech_files(source.path(), dest.path(), &manifest).is_err());
        fs::write(source.path().join("model.bin"), b"qualified").unwrap();
        copy_speech_files(source.path(), dest.path(), &manifest).unwrap();
        assert_eq!(
            fs::read(dest.path().join("bin/speech/model.bin")).unwrap(),
            b"qualified"
        );
        fs::write(source.path().join("model.bin"), b"changed").unwrap();
        assert!(copy_speech_files(source.path(), dest.path(), &manifest).is_err());
        let escape = serde_json::json!({"sha256":{"../outside.bin":hash}});
        assert!(copy_speech_files(source.path(), dest.path(), &escape).is_err());
    }
    #[test]
    fn organized_speech_can_be_reused_and_nested_hashes_keep_distinct_paths() {
        let source = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let model = format!("{:x}", Sha256::digest(b"model"));
        let license = format!("{:x}", Sha256::digest(b"license"));
        let manifest = serde_json::json!({"sha256": {
            "model.bin": model, "MODEL-LICENSE.txt": license
        }});
        fs::write(source.path().join("model.bin"), b"model").unwrap();
        fs::write(source.path().join("MODEL-LICENSE.txt"), b"license").unwrap();
        copy_speech_files(source.path(), first.path(), &manifest).unwrap();
        copy_speech_files(first.path(), second.path(), &manifest).unwrap();
        fs::write(second.path().join("model.bin"), b"other").unwrap();
        let hashes = hash_bundle(second.path()).unwrap();
        assert_eq!(hashes.len(), 3);
        assert_eq!(hashes["bin/speech/model.bin"], model);
        assert_eq!(hashes["licenses/MODEL-LICENSE.txt"], license);
        assert_ne!(hashes["model.bin"], model);
        fs::write(first.path().join("licenses/MODEL-LICENSE.txt"), b"changed").unwrap();
        assert!(copy_speech_files(first.path(), second.path(), &manifest).is_err());
    }
    #[test]
    fn provenance_rejects_mixed_or_incomplete_binaries_before_publication() {
        let info = serde_json::json!({
            "schema_version":1, "package_version":"0.1.0", "source_sha256":"a".repeat(64),
            "build_fingerprint":"b".repeat(64), "options_sha256":"c".repeat(64),
            "git_revision":null,"git_dirty":null,"target":"x86_64-pc-windows-gnu","profile":"release","features":["gui"]
        });
        same_build(&info, &info, "noh-app.exe").unwrap();
        for field in ["source_sha256", "build_fingerprint", "profile", "features"] {
            let mut different = info.clone();
            different[field] = if field.ends_with("sha256") || field == "build_fingerprint" {
                "d".repeat(64).into()
            } else {
                "different".into()
            };
            assert!(
                same_build(&info, &different, "noh-app.exe").is_err(),
                "{field}"
            );
        }
        for invalid in [
            serde_json::json!({}),
            serde_json::json!({"schema_version":2}),
            {
                let mut invalid = info;
                invalid["source_sha256"] = "not-a-hash".into();
                invalid
            },
        ] {
            assert!(validate_build(&invalid).is_err());
        }
    }
    #[test]
    fn publishing_replaces_the_whole_bundle() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("noh");
        fs::create_dir(&dest).unwrap();
        fs::create_dir_all(dest.join("bin/speech")).unwrap();
        fs::write(dest.join("bin/speech/old.dll"), b"old").unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        fs::create_dir(stage.path().join("ready")).unwrap();
        fs::write(stage.path().join("ready/new.exe"), b"new").unwrap();
        publish(stage, &dest).unwrap();
        assert_eq!(fs::read(dest.join("new.exe")).unwrap(), b"new");
        assert!(!dest.join("bin").exists());
    }
    #[test]
    #[cfg(windows)]
    fn locked_file_keeps_the_previous_bundle_intact() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("noh");
        fs::create_dir(&dest).unwrap();
        fs::create_dir_all(dest.join("bin/speech")).unwrap();
        fs::write(dest.join("bin/speech/old.dll"), b"old").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(dest.join("bin/speech/old.dll"))
            .unwrap();
        let stage = tempfile::tempdir_in(root.path()).unwrap();
        fs::create_dir(stage.path().join("ready")).unwrap();
        fs::write(stage.path().join("ready/new.exe"), b"new").unwrap();
        assert!(
            publish(stage, &dest)
                .unwrap_err()
                .to_string()
                .contains("previous distribution is unchanged")
        );
        assert_eq!(fs::read(dest.join("bin/speech/old.dll")).unwrap(), b"old");
        assert!(!dest.join("new.exe").exists());
        drop(lock);
    }
}
