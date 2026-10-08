//! Signed full-Setup contract. Extraction and publication belong to the engine.
use super::*;

pub const ENGINE: &str = "velopack-1.2.161";
pub const MAX_INSTALLED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_MANIFEST: usize = 128 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledFile {
    /// Path relative to current/, taken from the application payload before packing.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupContract {
    pub engine: String,
    /// Signer attestation; the producer must pack without installable prerequisites.
    /// Hash authentication alone does not inspect or sandbox executable behavior.
    pub runtime_dependencies: Vec<String>,
    pub build_fingerprint: String,
    pub installed_bytes: u64,
    pub files: Vec<InstalledFile>,
}

/// Producer inventory for an already validated complete application bundle.
/// The caller is responsible for packing with the pinned engine and no prerequisites.
pub fn describe_payload(directory: &Path, version: &Version) -> Result<SetupContract> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<InstalledFile>) -> Result<()> {
        #[cfg(windows)]
        let _directory = windows::ProtectedFile::directory(directory)?;
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(Error::Metadata);
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(Error::Metadata);
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| Error::Metadata)?
                .to_string_lossy()
                .replace('\\', "/");
            if !valid_relative_path(&relative) {
                return Err(Error::Metadata);
            }
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else if metadata.is_file() {
                if files.len() >= 4096 {
                    return Err(Error::Metadata);
                }
                let mut file = File::open(&path)?;
                let mut hasher = Sha256::new();
                let mut buffer = [0; 32768];
                let mut size = 0;
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    size += count as u64;
                    if size > MAX_INSTALLED_BYTES {
                        return Err(Error::Metadata);
                    }
                    hasher.update(&buffer[..count]);
                }
                files.push(InstalledFile {
                    path: relative,
                    size,
                    sha256: format!("{:x}", hasher.finalize()),
                });
            } else {
                return Err(Error::Metadata);
            }
        }
        Ok(())
    }
    let mut bytes = Vec::new();
    File::open(directory.join("manifest.json"))?
        .take(MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MANIFEST {
        return Err(Error::Metadata);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
    let build = &manifest["build"];
    if build["package_version"].as_str() != Some(&version.to_string())
        || build["target"].as_str() != Some("x86_64-pc-windows-gnu")
        || ["gui", "mcp", "updates"].iter().any(|feature| {
            !build["features"].as_array().is_some_and(|features| {
                features.iter().any(|value| value.as_str() == Some(feature))
            })
        })
    {
        return Err(Error::Metadata);
    }
    let fingerprint = build["build_fingerprint"]
        .as_str()
        .filter(|value| digest(value))
        .ok_or(Error::Metadata)?;
    let mut files = Vec::new();
    visit(directory, directory, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let installed_bytes = files.iter().try_fold(0u64, |total, file| {
        total.checked_add(file.size).ok_or(Error::Metadata)
    })?;
    Ok(SetupContract {
        engine: ENGINE.into(),
        runtime_dependencies: vec![],
        build_fingerprint: fingerprint.into(),
        installed_bytes,
        files,
    })
}

pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Refuse ambiguous Windows spellings, streams and device names even when a
/// producer runs on another OS. Native package names legitimately contain +/~;
/// installed directory enumeration additionally enforces the exact signed names.
pub fn valid_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._- ()+~".contains(&b))
                && {
                    let stem = part.split('.').next().unwrap().to_ascii_uppercase();
                    !matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    ) && !(stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                }
        })
}

/// Structural validation for producers; this does not authenticate any bytes.
pub fn validate_artifact(artifact: &Artifact) -> Result<()> {
    let contract = artifact.setup.as_ref().ok_or(Error::Metadata)?;
    if artifact.target
        != (Target {
            os: Os::Windows,
            arch: Architecture::X64,
        })
        || artifact.base_version.is_some()
        || !artifact.file_name.ends_with("-Setup.exe")
        || contract.engine != ENGINE
        || !contract.runtime_dependencies.is_empty()
        || !digest(&contract.build_fingerprint)
        || contract.files.is_empty()
        || contract.files.len() > 4096
        || contract.installed_bytes == 0
        || contract.installed_bytes > MAX_INSTALLED_BYTES
    {
        return Err(Error::Metadata);
    }
    let mut paths = HashSet::new();
    let mut total = 0u64;
    for file in &contract.files {
        if !valid_relative_path(&file.path)
            || (file.path.eq_ignore_ascii_case("manifest.json") && file.size > MAX_MANIFEST as u64)
            || file.path.eq_ignore_ascii_case("sq.version")
            || !digest(&file.sha256)
            || !paths.insert(file.path.to_ascii_lowercase())
        {
            return Err(Error::Metadata);
        }
        total = total.checked_add(file.size).ok_or(Error::Metadata)?;
    }
    // The engine owns sq.version. NOH supplies and authenticates all application
    // members, including the external recovery tools before they are copied out.
    for required in [
        "noh.exe",
        "bin/noh-cli.exe",
        "bin/noh-mcp.exe",
        "bin/noh-update-guard.exe",
        "bin/noh-update-repair.exe",
        "manifest.json",
    ] {
        if !paths.contains(required) {
            return Err(Error::Metadata);
        }
    }
    if total != contract.installed_bytes {
        return Err(Error::Metadata);
    }
    // A file cannot also be an ancestor directory of another file.
    for path in &paths {
        for (index, _) in path.match_indices('/') {
            if paths.contains(&path[..index]) {
                return Err(Error::Metadata);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release() -> Release {
        let mut release = super::super::tests::release();
        release.schema_version = 2;
        release.artifacts.truncate(1);
        let artifact = &mut release.artifacts[0];
        artifact.kind = PackageKind::WindowsSetup;
        artifact.file_name = "NOH-Setup.exe".into();
        artifact.setup = Some(SetupContract {
            engine: ENGINE.into(),
            runtime_dependencies: vec![],
            build_fingerprint: "a".repeat(64),
            installed_bytes: 6,
            files: [
                "noh.exe",
                "bin/noh-cli.exe",
                "bin/noh-mcp.exe",
                "bin/noh-update-guard.exe",
                "bin/noh-update-repair.exe",
                "manifest.json",
            ]
            .into_iter()
            .map(|path| InstalledFile {
                path: path.into(),
                size: 1,
                sha256: "b".repeat(64),
            })
            .collect(),
        });
        release
    }
    fn verify(release: &Release) -> Result<VerifiedRelease> {
        let (bytes, keys) = super::super::tests::signed(release);
        VerifiedRelease::verify(&bytes, &keys, "NOH")
    }
    #[test]
    fn profile_identity_is_signed_and_legacy_complete_metadata_stays_compatible() {
        let legacy = release();
        assert!(
            serde_json::to_value(&legacy)
                .unwrap()
                .get("profile")
                .is_none()
        );
        verify(&legacy)
            .unwrap()
            .require_profile(Profile::Complete)
            .unwrap();
        for profile in [Profile::Minimal, Profile::Standard] {
            let mut proposed = legacy.clone();
            proposed.profile = profile;
            assert!(verify(&proposed).is_err());
            proposed.schema_version = 3;
            let verified = verify(&proposed).unwrap();
            assert!(
                verified.is_setup(),
                "profile releases must use the Setup handoff"
            );
            verified.require_profile(profile).unwrap();
            assert!(verified.require_profile(Profile::Complete).is_err());
            let (bytes, keys) = super::super::tests::signed(&proposed);
            let mut envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
            let mut payload: serde_json::Value =
                serde_json::from_slice(&STANDARD.decode(&envelope.payload).unwrap()).unwrap();
            payload["profile"] = serde_json::json!("complete");
            envelope.payload = STANDARD.encode(serde_json::to_vec(&payload).unwrap());
            assert!(matches!(
                VerifiedRelease::verify(&serde_json::to_vec(&envelope).unwrap(), &keys, "NOH"),
                Err(Error::Signature)
            ));
        }
        let mut missing = legacy;
        missing.schema_version = 3;
        assert!(verify(&missing).is_err());
    }
    #[test]
    fn setup_is_explicitly_signed_and_cannot_enter_the_legacy_selection() {
        let release = release();
        let verified = verify(&release).unwrap();
        let target = release.artifacts[0].target;
        let previous = Version::new(1, 0, 0);
        assert!(verified.select(target, Channel::Stable, &previous).is_err());
        assert!(
            verified
                .select_setup(target, Channel::Stable, &previous)
                .unwrap()
                .is_some()
        );
        assert!(
            verified
                .select_setup(target, Channel::Beta, &previous)
                .is_err()
        );
        assert!(
            verified
                .select_setup(target, Channel::Stable, &release.version)
                .unwrap()
                .is_none()
        );
        let mut incompatible = release.clone();
        incompatible.schema_version = 1;
        assert!(verify(&incompatible).is_err());
        incompatible = release.clone();
        incompatible.artifacts[0].kind = PackageKind::Full;
        assert!(verify(&incompatible).is_err());
    }
    #[test]
    fn larger_setup_inventory_does_not_relax_legacy_bounds() {
        let mut release = release();
        let contract = release.artifacts[0].setup.as_mut().unwrap();
        for index in 0..700 {
            contract.files.push(InstalledFile {
                path: format!("models/file-{index}.bin"),
                size: 0,
                sha256: "c".repeat(64),
            });
        }
        assert!(serde_json::to_vec(&release).unwrap().len() > 64 * 1024);
        assert!(verify(&release).is_ok());
        let mut legacy = super::super::tests::release();
        legacy.notes = "x".repeat(65 * 1024);
        assert!(verify(&legacy).is_err());
    }
    #[test]
    fn signed_setup_accepts_locked_windows_runtime_names() {
        let mut release = release();
        let contract = release.artifacts[0].setup.as_mut().unwrap();
        for path in [
            "bin/libstdc++-6.dll",
            "bin/preview/libstdc++-6.dll",
            "licenses/native/mingw-w64-x86_64-spirv-cross-1~1.4.321.0-1/spirv-cross/LICENSE",
            "licenses/native/mingw-w64-x86_64-vulkan-loader-1~1.4.321.0-1/vulkan-loader/LICENSE.txt",
        ] {
            contract.files.push(InstalledFile {
                path: path.into(),
                size: 1,
                sha256: "b".repeat(64),
            });
            contract.installed_bytes += 1;
        }
        assert!(verify(&release).is_ok());
    }
    #[test]
    fn setup_refuses_ambiguous_paths_prerequisites_and_incomplete_inventory() {
        for bad in [
            "../escape",
            "C:/escape",
            "a\\b",
            "CON.txt",
            "bin/COM1",
            "a./b",
            "a:b",
            "a//b",
            "a/../b",
            "bin/libstdc++-6.dll:payload",
            "licenses/native/package~1./LICENSE",
        ] {
            assert!(!valid_relative_path(bad), "{bad}");
        }
        for mutation in 0..10 {
            let mut release = release();
            let artifact = &mut release.artifacts[0];
            let contract = artifact.setup.as_mut().unwrap();
            match mutation {
                0 => artifact.target.arch = Architecture::Arm64,
                1 => artifact.file_name = "NOH.exe".into(),
                2 => contract.runtime_dependencies.push("dotnet8".into()),
                3 => contract.engine = "unqualified".into(),
                4 => contract.installed_bytes += 1,
                5 => {
                    contract.files.pop();
                }
                6 => {
                    let mut file = contract.files[0].clone();
                    file.path = "NOH.EXE".into();
                    contract.files.push(file);
                    contract.installed_bytes += 1;
                }
                7 => {
                    contract.files[0].path = "sq.version".into();
                }
                8 => {
                    contract.files.push(InstalledFile {
                        path: "bin".into(),
                        size: 0,
                        sha256: "a".repeat(64),
                    });
                }
                _ => artifact.base_version = Some(Version::new(1, 0, 0)),
            }
            assert!(verify(&release).is_err(), "mutation {mutation}");
        }
    }
}
