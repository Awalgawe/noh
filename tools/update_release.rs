//! Offline release signing tool. Only public keys belong in application builds.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use noh::update::*;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::PathBuf,
};

#[path = "setup_profiles.rs"]
mod setup_profiles;

#[derive(Parser)]
struct Options {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Derive an exact content profile from one validated complete application build.
    PrepareProfile {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_enum)]
        profile: Profile,
    },
    /// Confirm the signer matches independently configured public trust before building.
    CheckSigningKey {
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        public_keys: PathBuf,
        #[arg(long)]
        key_id: String,
    },
    /// Describe a full Setup produced with the pinned engine without prerequisites.
    DescribeSetup {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        setup: PathBuf,
        #[arg(long)]
        package_id: String,
        #[arg(long)]
        version: semver::Version,
        #[arg(long, value_enum, default_value = "complete")]
        profile: Profile,
        #[arg(long)]
        url: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Authenticate every package against an independently reviewed publication policy.
    VerifyRelease {
        #[arg(long)]
        public_keys: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        packages: PathBuf,
        #[arg(long)]
        expected_version: semver::Version,
        #[arg(long)]
        tag: String,
    },
    /// Authenticate and retain a full package for recovery; installs nothing.
    Retain {
        #[arg(long, value_enum, default_value = "complete")]
        profile: Profile,
        #[arg(long, default_value="nupkg", value_parser=["nupkg", "windows-setup"])]
        format: String,
        #[arg(long)]
        public_keys: PathBuf,
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        package_id: String,
        #[arg(long)]
        expected_version: semver::Version,
        #[arg(long, default_value="stable", value_parser=["stable", "beta"])]
        channel: String,
    },
    /// Independently authenticate retained recovery inputs without signing or installing.
    Verify {
        #[arg(long, value_enum, default_value = "complete")]
        profile: Profile,
        #[arg(long, default_value="nupkg", value_parser=["nupkg", "windows-setup"])]
        format: String,
        #[arg(long)]
        public_keys: PathBuf,
        #[arg(long)]
        envelope: PathBuf,
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        package_id: String,
        #[arg(long)]
        expected_version: semver::Version,
    },
    Keygen {
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        public_keys: PathBuf,
        #[arg(long)]
        key_id: String,
    },
    Sign {
        #[arg(long)]
        private_key: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        release: PathBuf,
        #[arg(long)]
        packages: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationPolicy {
    repository: String,
    package_id: String,
    channel: Channel,
    controlled_windows_trial: bool,
    targets: Vec<Target>,
}

fn read_bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Metadata);
    }
    Ok(bytes)
}

fn verify_complete_release(
    policy: &PublicationPolicy,
    verified: &VerifiedRelease,
    packages: &std::path::Path,
    expected_version: &semver::Version,
    tag: &str,
) -> Result<()> {
    let release = verified.release();
    let repository_parts: Vec<_> = policy.repository.split('/').collect();
    let safe_component = |value: &str| {
        !value.is_empty()
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    let targets: HashSet<_> = policy.targets.iter().copied().collect();
    if repository_parts.len() != 2
        || !repository_parts.iter().all(|part| safe_component(part))
        || !safe_component(tag)
        || targets.is_empty()
        || targets.len() != policy.targets.len()
        || release.package_id != policy.package_id
        || release.version != *expected_version
        || release.channel != policy.channel
        || release.artifacts.len() != targets.len()
    {
        return Err(Error::Target);
    }
    if policy.controlled_windows_trial {
        let trial_target = Target {
            os: Os::Windows,
            arch: Architecture::X64,
        };
        if policy.package_id != "NohGuiUpdateQualification"
            || targets != HashSet::from([trial_target])
        {
            return Err(Error::Target);
        }
    } else if targets
        .iter()
        .any(|target| !target.installation_qualified())
    {
        return Err(Error::Target);
    }
    let mut names = HashSet::new();
    let mut seen_targets = HashSet::new();
    for artifact in &release.artifacts {
        if artifact.kind != PackageKind::Full
            || artifact.base_version.is_some()
            || !targets.contains(&artifact.target)
            || !seen_targets.insert(artifact.target)
            || !names.insert(artifact.file_name.clone())
            || artifact.url
                != format!(
                    "https://github.com/{}/releases/download/{tag}/{}",
                    policy.repository, artifact.file_name
                )
        {
            return Err(Error::Target);
        }
        let path = packages.join(&artifact.file_name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::Metadata);
        }
        verify_package(&mut std::fs::File::open(path)?, artifact)?;
    }
    // The package directory contains packages only; envelopes/policy live outside it.
    let mut actual_names = HashSet::new();
    for entry in std::fs::read_dir(packages)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err(Error::Metadata);
        }
        actual_names.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Metadata)?,
        );
    }
    if actual_names != names || seen_targets != targets {
        return Err(Error::Target);
    }
    Ok(())
}
fn write_new(path: &std::path::Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    if private {
        let mut file = create_private_key(path)?;
        file.write_all(bytes)?;
        return file.sync_all();
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if private {
            options.mode(0o600);
        }
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
#[cfg(windows)]
fn create_private_key(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let sid_result = (|| {
        let mut size = 0;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
        if size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid_text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_text) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut length = 0;
        unsafe {
            while *sid_text.add(length) != 0 {
                length += 1;
            }
        }
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_text, length) });
        unsafe { LocalFree(sid_text.cast()) };
        Ok(sid)
    })();
    unsafe { CloseHandle(token) };
    let sid = sid_result?;
    // Protected DACL: only the current user and SYSTEM may access the key.
    // Install this at creation, before any secret bytes reach the file.
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let error = if handle == INVALID_HANDLE_VALUE {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    unsafe { LocalFree(descriptor) };
    if let Some(error) = error {
        return Err(error);
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}
fn run(options: Options) -> Result<()> {
    match options.command {
        Command::PrepareProfile {
            source,
            output,
            profile,
        } => {
            setup_profiles::prepare(&source, &output, profile)?;
            println!(
                "Prepared {} distribution from shared application binaries.",
                profile.as_str()
            );
        }
        Command::CheckSigningKey {
            private_key,
            public_keys,
            key_id,
        } => {
            let bytes = read_bounded(&private_key, 8192)?;
            let pair = Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| Error::Signature)?;
            let keys: Vec<TrustKey> = serde_json::from_slice(&read_bounded(&public_keys, 16384)?)
                .map_err(|_| Error::Metadata)?;
            trust::validate_keys(&keys)?;
            if !keys.iter().any(|key| {
                key.id == key_id && key.public_key.as_slice() == pair.public_key().as_ref()
            }) {
                return Err(Error::Signature);
            }
            println!("Signing key matches independent public trust.");
        }
        Command::DescribeSetup {
            bundle,
            setup: package,
            package_id,
            version,
            profile,
            url,
            output,
        } => {
            use sha2::{Digest, Sha256};
            let bundle = std::fs::canonicalize(bundle)?;
            let manifest: serde_json::Value = serde_json::from_slice(&read_bounded(
                &bundle.join("manifest.json"),
                setup::MAX_MANIFEST as u64,
            )?)
            .map_err(|_| Error::Metadata)?;
            if Profile::from_manifest(&manifest)? != profile {
                return Err(Error::Target);
            }
            let contract = setup::describe_payload(&bundle, &version)?;
            setup_profiles::validate_content(profile, &contract)?;
            let mut input = std::fs::File::open(&package)?;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut buffer = [0; 32768];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                size += count as u64;
                hasher.update(&buffer[..count]);
            }
            let artifact = Artifact {
                target: Target {
                    os: Os::Windows,
                    arch: Architecture::X64,
                },
                kind: PackageKind::WindowsSetup,
                base_version: None,
                file_name: package
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or(Error::Metadata)?
                    .into(),
                url,
                size,
                sha256: format!("{:x}", hasher.finalize()),
                setup: Some(contract),
            };
            setup::validate_artifact(&artifact)?;
            let release = Release {
                schema_version: if profile.is_complete() { 2 } else { 3 },
                profile,
                package_id,
                version,
                channel: Channel::Stable,
                notes: "Full Windows Setup".into(),
                artifacts: vec![artifact],
            };
            write_new(
                &output,
                &serde_json::to_vec_pretty(&release).map_err(|_| Error::Metadata)?,
                false,
            )?;
            println!("Wrote unsigned Setup metadata; independent signing is still required.");
        }
        Command::VerifyRelease {
            public_keys,
            policy,
            envelope,
            packages,
            expected_version,
            tag,
        } => {
            let policy: PublicationPolicy = serde_json::from_slice(&read_bounded(&policy, 16384)?)
                .map_err(|_| Error::Metadata)?;
            let keys: Vec<TrustKey> = serde_json::from_slice(&read_bounded(&public_keys, 16384)?)
                .map_err(|_| Error::Metadata)?;
            trust::validate_keys(&keys)?;
            let verified = VerifiedRelease::verify(
                &read_bounded(&envelope, 131072)?,
                &keys,
                &policy.package_id,
            )?;
            verify_complete_release(&policy, &verified, &packages, &expected_version, &tag)?;
            println!("All expected release packages authenticated; no publication performed.");
        }
        Command::Retain {
            profile,
            format,
            public_keys,
            envelope,
            package,
            archive,
            package_id,
            expected_version,
            channel,
        } => {
            let read = |path: PathBuf, limit: u64| -> Result<Vec<u8>> {
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(limit + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > limit {
                    return Err(Error::Metadata);
                }
                Ok(bytes)
            };
            let keys: Vec<TrustKey> =
                serde_json::from_slice(&read(public_keys, 16384)?).map_err(|_| Error::Metadata)?;
            trust::validate_keys(&keys)?;
            let storage = if format == "windows-setup" {
                retention::SETUP.for_profile(profile)
            } else {
                if profile != Profile::Complete {
                    return Err(Error::Target);
                }
                retention::NUPKG
            };
            let saved = storage.retain_or_reuse(
                &archive,
                &package,
                &read(envelope, MAX_ENVELOPE as u64)?,
                &keys,
                &package_id,
                Target::native()?,
                if channel == "stable" {
                    Channel::Stable
                } else {
                    Channel::Beta
                },
                &expected_version,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            println!(
                "{}",
                serde_json::json!({"retained_directory":saved,"version":expected_version.to_string(),"installed":false})
            );
        }
        Command::Verify {
            profile,
            format,
            public_keys,
            envelope,
            package,
            package_id,
            expected_version,
        } => {
            let read_bounded = |path: PathBuf, limit: u64| -> Result<Vec<u8>> {
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(limit + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > limit {
                    return Err(Error::Metadata);
                }
                Ok(bytes)
            };
            let keys: Vec<TrustKey> = serde_json::from_slice(&read_bounded(public_keys, 16384)?)
                .map_err(|_| Error::Metadata)?;
            trust::validate_keys(&keys)?;
            let verified = VerifiedRelease::verify(
                &read_bounded(envelope, MAX_ENVELOPE as u64)?,
                &keys,
                &package_id,
            )?;
            let release = verified.release();
            verified.require_profile(profile)?;
            if release.version != expected_version || release.channel != Channel::Stable {
                return Err(Error::Target);
            }
            let target = Target::native()?;
            let artifacts: Vec<_> = release
                .artifacts
                .iter()
                .filter(|artifact| {
                    artifact.target == target
                        && artifact.kind
                            == if format == "windows-setup" {
                                PackageKind::WindowsSetup
                            } else {
                                PackageKind::Full
                            }
                })
                .collect();
            if artifacts.len() != 1
                || package.file_name() != Some(std::ffi::OsStr::new(&artifacts[0].file_name))
            {
                return Err(Error::Target);
            }
            verify_package(&mut std::fs::File::open(package)?, artifacts[0])?;
            println!(
                "Authenticated retained full package {} for {:?}; no installation performed.",
                release.version, target
            );
        }
        Command::Keygen {
            private_key,
            public_keys,
            key_id,
        } => {
            let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .map_err(|_| Error::Signature)?;
            let pair =
                Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|_| Error::Signature)?;
            let public = [TrustKey {
                id: key_id,
                public_key: pair
                    .public_key()
                    .as_ref()
                    .try_into()
                    .map_err(|_| Error::Signature)?,
            }];
            trust::validate_keys(&public)?;
            if private_key.exists() || public_keys.exists() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "Key output already exists",
                )));
            }
            write_new(&private_key, document.as_ref(), true)?;
            write_new(
                &public_keys,
                &serde_json::to_vec_pretty(&public).map_err(|_| Error::Metadata)?,
                false,
            )?;
            println!("Created private signing key and public trust configuration.");
        }
        Command::Sign {
            private_key,
            key_id,
            release,
            packages,
            output,
        } => {
            let mut secret = Vec::new();
            std::fs::File::open(private_key)?
                .take(8193)
                .read_to_end(&mut secret)?;
            if secret.len() > 8192 {
                return Err(Error::Signature);
            }
            let pair = Ed25519KeyPair::from_pkcs8(&secret).map_err(|_| Error::Signature)?;
            let mut payload = Vec::new();
            std::fs::File::open(release)?
                .take((MAX_PAYLOAD + 1) as u64)
                .read_to_end(&mut payload)?;
            if payload.len() > MAX_PAYLOAD {
                return Err(Error::Metadata);
            }
            let value: Release = serde_json::from_slice(&payload).map_err(|_| Error::Metadata)?;
            let envelope = serde_json::to_vec(&Envelope {
                key_id: key_id.clone(),
                payload: STANDARD.encode(&payload),
                signature: STANDARD.encode(pair.sign(&payload).as_ref()),
            })
            .map_err(|_| Error::Metadata)?;
            let keys = [TrustKey {
                id: key_id,
                public_key: pair
                    .public_key()
                    .as_ref()
                    .try_into()
                    .map_err(|_| Error::Signature)?,
            }];
            trust::validate_keys(&keys)?;
            let verified = VerifiedRelease::verify(&envelope, &keys, &value.package_id)?;
            for artifact in &verified.release().artifacts {
                verify_package(
                    &mut std::fs::File::open(packages.join(&artifact.file_name))?,
                    artifact,
                )?;
            }
            write_new(&output, &envelope, false)?;
            println!("Signed release metadata after verifying all package digests.");
        }
    }
    Ok(())
}
fn main() {
    if let Err(error) = run(Options::parse()) {
        eprintln!("update-release: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    struct Fixture {
        directory: tempfile::TempDir,
        policy: PublicationPolicy,
        release: Release,
        pair: Ed25519KeyPair,
        keys: Vec<TrustKey>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let bytes = b"synthetic full package";
            std::fs::write(directory.path().join("fixture.nupkg"), bytes).unwrap();
            let target = Target {
                os: Os::Windows,
                arch: Architecture::X64,
            };
            let policy = PublicationPolicy {
                repository: "example/noh".into(),
                package_id: "NohGuiUpdateQualification".into(),
                channel: Channel::Stable,
                controlled_windows_trial: true,
                targets: vec![target],
            };
            let release = Release {
                schema_version: 1,
                profile: Profile::Complete,
                package_id: policy.package_id.clone(),
                version: semver::Version::new(1, 2, 3),
                channel: Channel::Stable,
                notes: "Synthetic publication verifier fixture".into(),
                artifacts: vec![Artifact {
                    target,
                    kind: PackageKind::Full,
                    setup: None,
                    base_version: None,
                    file_name: "fixture.nupkg".into(),
                    url: "https://github.com/example/noh/releases/download/trial-1/fixture.nupkg"
                        .into(),
                    size: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                }],
            };
            let pair = Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap();
            let keys = vec![TrustKey {
                id: "fixture".into(),
                public_key: pair.public_key().as_ref().try_into().unwrap(),
            }];
            Self {
                directory,
                policy,
                release,
                pair,
                keys,
            }
        }
        fn verify(&self) -> Result<()> {
            let payload = serde_json::to_vec(&self.release).unwrap();
            let envelope = serde_json::to_vec(&Envelope {
                key_id: "fixture".into(),
                payload: STANDARD.encode(&payload),
                signature: STANDARD.encode(self.pair.sign(&payload).as_ref()),
            })
            .unwrap();
            let verified = VerifiedRelease::verify(&envelope, &self.keys, &self.policy.package_id)?;
            verify_complete_release(
                &self.policy,
                &verified,
                self.directory.path(),
                &semver::Version::new(1, 2, 3),
                "trial-1",
            )
        }
    }

    #[test]
    fn complete_signed_trial_is_accepted_but_production_is_not() {
        let mut f = Fixture::new();
        f.verify().unwrap();
        f.policy.controlled_windows_trial = false;
        assert!(f.verify().is_err());
    }
    #[test]
    fn changed_or_missing_package_is_refused() {
        let f = Fixture::new();
        std::fs::write(f.directory.path().join("fixture.nupkg"), b"corrupted").unwrap();
        assert!(f.verify().is_err());
        std::fs::remove_file(f.directory.path().join("fixture.nupkg")).unwrap();
        assert!(f.verify().is_err());
    }
    #[test]
    fn extra_file_or_target_is_refused() {
        let mut f = Fixture::new();
        std::fs::write(f.directory.path().join("extra.nupkg"), b"extra").unwrap();
        assert!(f.verify().is_err());
        std::fs::remove_file(f.directory.path().join("extra.nupkg")).unwrap();
        f.release.artifacts.push(f.release.artifacts[0].clone());
        f.release.artifacts[1].target.os = Os::Linux;
        assert!(f.verify().is_err());
    }
    #[test]
    fn signed_wrong_repository_tag_channel_version_or_key_is_refused() {
        let mut f = Fixture::new();
        f.release.artifacts[0].url =
            "https://github.com/other/noh/releases/download/trial-1/fixture.nupkg".into();
        assert!(f.verify().is_err());
        f.release.artifacts[0].url =
            "https://github.com/example/noh/releases/download/other-tag/fixture.nupkg".into();
        assert!(f.verify().is_err());
        let mut f = Fixture::new();
        f.release.channel = Channel::Beta;
        assert!(f.verify().is_err());
        let mut f = Fixture::new();
        f.release.version = semver::Version::new(1, 2, 4);
        assert!(f.verify().is_err());
        let mut f = Fixture::new();
        f.keys[0].public_key = [0; 32];
        assert!(f.verify().is_err());
    }
    #[test]
    fn duplicate_or_unqualified_policy_targets_are_refused() {
        let mut f = Fixture::new();
        f.policy.targets.push(f.policy.targets[0]);
        assert!(f.verify().is_err());
        let mut f = Fixture::new();
        f.policy.targets[0].os = Os::Macos;
        assert!(f.verify().is_err());
    }
}
