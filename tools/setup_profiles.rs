//! Prepare profile payloads from one common validated build, before Setup packing.
use noh::update::{Error, Profile, Result, setup};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

fn includes(profile: Profile, path: &str) -> bool {
    match profile {
        Profile::Complete => true,
        Profile::Standard => !path.starts_with("bin/speech/"),
        Profile::Minimal => {
            !path.starts_with("bin/")
                || matches!(
                    path,
                    "bin/noh-cli.exe"
                        | "bin/noh-mcp.exe"
                        | "bin/noh-update-guard.exe"
                        | "bin/noh-update-repair.exe"
                )
        }
    }
}

pub fn validate_content(profile: Profile, contract: &setup::SetupContract) -> Result<()> {
    if contract
        .files
        .iter()
        .any(|file| !includes(profile, &file.path))
    {
        return Err(Error::Target);
    }
    if profile == Profile::Standard
        && ["bin/ffmpeg.exe", "bin/preview/libmpv-2.dll"]
            .iter()
            .any(|path| !contract.files.iter().any(|file| file.path == *path))
    {
        return Err(Error::Metadata);
    }
    Ok(())
}

pub fn prepare(source: &Path, output: &Path, profile: Profile) -> Result<()> {
    let mut bytes = Vec::new();
    std::fs::File::open(source.join("manifest.json"))?
        .take(setup::MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > setup::MAX_MANIFEST {
        return Err(Error::Metadata);
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
    if Profile::from_manifest(&manifest)? != Profile::Complete {
        return Err(Error::Target);
    }
    let source_hashes = manifest["sha256"].as_object().ok_or(Error::Metadata)?;
    if source_hashes.is_empty() || source_hashes.len() > 4096 || output.try_exists()? {
        return Err(Error::Metadata);
    }
    let parent = output.parent().ok_or(Error::Metadata)?;
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".noh-profile-")
        .tempdir_in(parent)?;
    let mut hashes = serde_json::Map::new();
    for (relative, expected) in source_hashes {
        if !setup::valid_relative_path(relative) || relative == "manifest.json" {
            return Err(Error::Metadata);
        }
        if !includes(profile, relative) {
            continue;
        }
        let expected = expected.as_str().ok_or(Error::Metadata)?;
        let path = source.join(relative);
        #[cfg(windows)]
        let mut input = noh::update::windows::ProtectedFile::open(&path)?;
        #[cfg(windows)]
        let file = input.file();
        #[cfg(not(windows))]
        let mut input = std::fs::File::open(&path)?;
        #[cfg(not(windows))]
        let file = &mut input;
        let destination = staging.path().join(relative);
        std::fs::create_dir_all(destination.parent().ok_or(Error::Metadata)?)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 32768];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
        }
        output.sync_all()?;
        if format!("{:x}", digest.finalize()) != expected {
            return Err(Error::Integrity);
        }
        hashes.insert(relative.clone(), expected.into());
    }
    for path in [
        "noh.exe",
        "bin/noh-cli.exe",
        "bin/noh-mcp.exe",
        "bin/noh-update-guard.exe",
        "bin/noh-update-repair.exe",
    ] {
        if !hashes.contains_key(path) {
            return Err(Error::Metadata);
        }
    }
    if profile != Profile::Minimal
        && ["bin/ffmpeg.exe", "bin/preview/libmpv-2.dll"]
            .iter()
            .any(|path| !hashes.contains_key(*path))
    {
        return Err(Error::Metadata);
    }
    if profile == Profile::Complete && !hashes.keys().any(|path| path.starts_with("bin/speech/")) {
        return Err(Error::Metadata);
    }
    manifest["distribution_profile"] = profile.as_str().into();
    manifest["sha256"] = hashes.into();
    if profile == Profile::Minimal {
        manifest["tools"] = serde_json::json!({});
    }
    let encoded = serde_json::to_vec_pretty(&manifest).map_err(|_| Error::Metadata)?;
    if encoded.len() > setup::MAX_MANIFEST {
        return Err(Error::Metadata);
    }
    std::fs::write(staging.path().join("manifest.json"), encoded)?;
    // Output is new; keep the caller's existing distributions untouched.
    if output.try_exists()? {
        return Err(Error::Metadata);
    }
    std::fs::rename(staging.path(), output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path) {
        let mut hashes = serde_json::Map::new();
        for path in [
            "noh.exe",
            "noh.ico",
            "bin/noh-cli.exe",
            "bin/noh-mcp.exe",
            "bin/noh-update-guard.exe",
            "bin/noh-update-repair.exe",
            "bin/ffmpeg.exe",
            "bin/avcodec.dll",
            "bin/preview/libmpv-2.dll",
            "bin/preview/avcodec.dll",
            "bin/speech/whisper.exe",
            "bin/speech/model.bin",
            "docs/INSTALLING.md",
            "licenses/NOH-LICENSE.txt",
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, path.as_bytes()).unwrap();
            hashes.insert(
                path.into(),
                format!("{:x}", Sha256::digest(path.as_bytes())).into(),
            );
        }
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(
                &serde_json::json!({"sha256":hashes,"tools":{"ffmpeg":{"path":"bin/ffmpeg.exe"}}}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn profiles_preserve_common_bytes_and_media_dependency_groups() {
        let root = tempfile::tempdir().unwrap();
        let full = root.path().join("source");
        source(&full);
        for profile in [Profile::Minimal, Profile::Standard, Profile::Complete] {
            let output = root.path().join(profile.as_str());
            prepare(&full, &output, profile).unwrap();
            assert_eq!(std::fs::read(output.join("noh.exe")).unwrap(), b"noh.exe");
            assert!(output.join("licenses/NOH-LICENSE.txt").is_file());
            assert_eq!(
                output.join("bin/avcodec.dll").is_file(),
                profile != Profile::Minimal
            );
            assert_eq!(
                output.join("bin/preview/avcodec.dll").is_file(),
                profile != Profile::Minimal
            );
            assert_eq!(
                output.join("bin/speech/model.bin").is_file(),
                profile == Profile::Complete
            );
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap())
                    .unwrap();
            assert_eq!(Profile::from_manifest(&manifest).unwrap(), profile);
            assert_eq!(
                manifest["sha256"].as_object().unwrap().len(),
                if profile == Profile::Minimal {
                    8
                } else if profile == Profile::Standard {
                    12
                } else {
                    14
                }
            );
            assert!(prepare(&full, &output, profile).is_err());
        }
        std::fs::write(full.join("noh.exe"), b"tampered").unwrap();
        let output = root.path().join("bad");
        assert!(prepare(&full, &output, Profile::Minimal).is_err());
        assert!(!output.exists());
    }
}
