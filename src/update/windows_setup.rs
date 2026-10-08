//! Full official Setup adapter. NOH authenticates, coordinates and verifies;
//! Setup alone extracts, saves the old root and publishes application files.
use super::{
    setup::{InstalledFile, SetupContract},
    windows::{ExclusiveLease, ProtectedFile, SupervisedProcess},
    windows_anchor::InstallationAnchor,
    *,
};
use std::{ffi::OsStr, time::Instant};

#[derive(Clone, Debug)]
pub enum Action {
    Initialize,
    Update {
        from: Version,
    },
    /// Exact signed version, explicitly requested, also when application is absent.
    Repair,
    /// Ordinary double-click installation/repair must never silently downgrade.
    InstallOrRepair,
}

pub struct PreparedSetup {
    anchor: InstallationAnchor,
    release: VerifiedRelease,
    package: ProtectedFile,
    package_path: PathBuf,
    action: Action,
    keys: Vec<TrustKey>,
}

#[derive(Serialize)]
pub struct Outcome {
    pub version: Version,
    pub profile: Profile,
    pub retained_archive: PathBuf,
    pub recovery_tools: PathBuf,
    pub setup_elapsed_ms: u128,
    pub job_peak_commit_bytes: Option<u64>,
}

impl PreparedSetup {
    pub fn prepare(
        envelope: &[u8],
        keys: &[TrustKey],
        package_id: &str,
        channel: Channel,
        base: &Path,
        package: &Path,
        version: &Version,
        action: Action,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        if super::windows_guard::is_elevated()? {
            return Err(Error::InstallationUnavailable);
        }
        trust::validate_keys(keys)?;
        let release = VerifiedRelease::verify(envelope, keys, package_id)?;
        let artifact = exact_setup(&release, channel, version)?;
        let mut protected = ProtectedFile::open(package)?;
        verify_package_cancellable(protected.file(), artifact, cancelled)?;
        let package_path = std::fs::canonicalize(package)?;
        if package_path.file_name() != Some(OsStr::new(&artifact.file_name)) {
            return Err(Error::Target);
        }
        let base_protection = ProtectedFile::directory(base)?;
        let base = std::fs::canonicalize(base)?;
        let root = base.join("application");
        if package_path.starts_with(&root)
            || std::fs::canonicalize(std::env::current_exe()?)?.starts_with(&root)
        {
            return Err(Error::InstallationUnavailable);
        }
        let anchor = if matches!(action, Action::Initialize) {
            if root.try_exists()? {
                return Err(Error::InstallationUnavailable);
            }
            InstallationAnchor::create_for_profile(
                &base,
                package_id,
                channel,
                release.release().profile,
            )?
        } else {
            InstallationAnchor::open(&base, package_id, Some(channel))?
        };
        release.require_profile(anchor.profile())?;
        drop(base_protection);
        let prepared = Self {
            anchor,
            release,
            package: protected,
            package_path,
            action,
            keys: keys.to_vec(),
        };
        prepared.check_version()?;
        Ok(prepared)
    }

    pub fn anchor(&self) -> &InstallationAnchor {
        &self.anchor
    }
    pub fn release(&self) -> &VerifiedRelease {
        &self.release
    }

    fn check_version(&self) -> Result<()> {
        match &self.action {
            Action::Initialize => {
                if self.anchor.installation().try_exists()? {
                    return Err(Error::InstallationUnavailable);
                }
            }
            Action::Update { from } => {
                let mut manifest =
                    ProtectedFile::open(&self.anchor.installation().join("current/manifest.json"))?;
                let mut bytes = Vec::new();
                manifest
                    .file()
                    .take(setup::MAX_MANIFEST as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > setup::MAX_MANIFEST {
                    return Err(Error::Metadata);
                }
                let manifest: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
                if Profile::from_manifest(&manifest)? != self.anchor.profile() {
                    return Err(Error::Target);
                }
                let actual = super::windows_guard::installed_version(
                    &self.anchor.installation(),
                    &self.release.release().package_id,
                )?;
                if &actual != from
                    || !self
                        .release
                        .release()
                        .version
                        .cmp_precedence(&actual)
                        .is_gt()
                {
                    return Err(Error::Target);
                }
            }
            Action::Repair => {}
            Action::InstallOrRepair => {
                bootstrap::require_no_downgrade(
                    &self.anchor,
                    &self.release.release().version,
                    &self.keys,
                    &self.release.release().package_id,
                )?;
            }
        }
        Ok(())
    }

    pub fn apply(
        self,
        client_timeout: Duration,
        setup_timeout: Duration,
        log: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Outcome> {
        let started = Instant::now();
        let lease = loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            match ExclusiveLease::acquire(&self.anchor.coordination()) {
                Ok(lease) => break lease.into_protection(),
                Err(error)
                    if error.raw_os_error() == Some(32) && started.elapsed() < client_timeout =>
                {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(error) => return Err(error.into()),
            }
        };
        // The anchor remains pinned through exclusion, engine consumption and
        // verification; the installation root itself must remain renameable.
        self.check_version()?;
        let root = self.anchor.installation();
        if root.try_exists()? {
            drop(ProtectedFile::directory(&root)?);
        }
        super::windows_guard::wait_for_installed_process_exit_cancellable(
            &root,
            started,
            client_timeout,
            cancelled,
        )?;
        let artifact = exact_setup(
            &self.release,
            self.anchor.channel(),
            &self.release.release().version,
        )?;
        let contract = artifact.setup.as_ref().ok_or(Error::Metadata)?;
        let required = required_space(artifact.size, contract.installed_bytes)?;
        if super::windows::available_disk_bytes(self.anchor.base())? < required {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Insufficient space for Setup, retained package and application trees",
            )));
        }
        let retained = retention::SETUP
            .for_profile(self.anchor.profile())
            .retain_or_reuse(
                &self.anchor.coordination().join("retained"),
                &self.package_path,
                self.release.envelope(),
                &self.keys,
                &self.release.release().package_id,
                Target::native()?,
                self.anchor.channel(),
                &self.release.release().version,
                cancelled,
            )?;
        // Use only an external log; pin its directory before creating it. The log
        // handle denies replacement while allowing the engine to append.
        let _log_directory = ProtectedFile::directory(log.parent().ok_or(Error::Metadata)?)?;
        let log = std::fs::canonicalize(log.parent().ok_or(Error::Metadata)?)?
            .join(log.file_name().ok_or(Error::Metadata)?);
        if log.starts_with(&root) {
            return Err(Error::InstallationUnavailable);
        }
        use std::os::windows::fs::OpenOptionsExt;
        let _log = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .share_mode(3)
            .open(&log)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let engine_start = Instant::now();
        let consumer = SupervisedProcess::spawn_protected(
            &self.package_path,
            &[
                OsStr::new("--silent"),
                OsStr::new("--installto"),
                root.as_os_str(),
                OsStr::new("--log"),
                log.as_os_str(),
            ],
            self.anchor.base(),
            &[&self.package, self.anchor.protection(), &lease],
        )?;
        let consumed = (|| {
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if engine_start.elapsed() >= setup_timeout {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Setup deadline exceeded; retained recovery inputs preserved",
                    )));
                }
                if let Some(exit) = consumer.wait(Duration::from_millis(25))? {
                    if exit != 0 {
                        return Err(Error::Io(std::io::Error::other(
                            "Setup failed; retained recovery inputs preserved",
                        )));
                    }
                    if consumer.active_processes()? == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Ok(())
        })();
        // Failures do not release exclusion until every owned consumer terminates.
        // Consumer-owned protected handles cover abrupt guardian disappearance.
        if consumed.is_err() {
            consumer.terminate_tree(Duration::from_secs(5))?;
        }
        let peak = consumer.peak_committed_bytes().ok();
        consumed?;
        let elapsed = engine_start.elapsed().as_millis();
        drop(consumer);
        let mut installed = protect_installed(&self.anchor, &self.release, cancelled)?;
        let tools = provision_tools(&self.anchor, &self.release, &mut installed, cancelled)?;
        drop(installed);
        drop(lease);
        Ok(Outcome {
            version: self.release.release().version.clone(),
            profile: self.anchor.profile(),
            retained_archive: retained,
            recovery_tools: tools,
            setup_elapsed_ms: elapsed,
            job_peak_commit_bytes: peak,
        })
    }
}

pub fn exact_setup<'a>(
    release: &'a VerifiedRelease,
    channel: Channel,
    version: &Version,
) -> Result<&'a Artifact> {
    if Target::native()?
        != (Target {
            os: Os::Windows,
            arch: Architecture::X64,
        })
    {
        return Err(Error::Target);
    }
    if release.release().version != *version || release.release().channel != channel {
        return Err(Error::Target);
    }
    release
        .release()
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.kind == PackageKind::WindowsSetup
                && artifact.target
                    == (Target {
                        os: Os::Windows,
                        arch: Architecture::X64,
                    })
        })
        .ok_or(Error::Target)
}

fn required_space(package: u64, application: u64) -> Result<u64> {
    // Additional space estimate: retained copy + engine package/temp allowance,
    // two new application-sized trees and a tool/log margin. Existing saved roots
    // are already reflected in free space. No automatic backup pruning or reservation.
    package
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(application.checked_mul(2)?))
        .and_then(|bytes| bytes.checked_add(64 * 1024 * 1024))
        .ok_or(Error::Metadata)
}

pub fn verify_member(
    file: &mut File,
    member: &InstalledFile,
    cancelled: &AtomicBool,
) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    if file.metadata()?.len() != member.size {
        return Err(Error::Integrity);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0; 32768];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    if format!("{:x}", hasher.finalize()) != member.sha256 {
        return Err(Error::Integrity);
    }
    Ok(())
}

pub fn protect_installed(
    anchor: &InstallationAnchor,
    release: &VerifiedRelease,
    cancelled: &AtomicBool,
) -> Result<Vec<(String, ProtectedFile)>> {
    release.require_profile(anchor.profile())?;
    let artifact = exact_setup(release, anchor.channel(), &release.release().version)?;
    let contract = artifact.setup.as_ref().ok_or(Error::Metadata)?;
    let current = anchor.installation().join("current");
    let _directory = ProtectedFile::directory(&current)?;
    let version = ProtectedFile::open(&current.join("sq.version"))?;
    if super::windows_guard::installed_version(
        &anchor.installation(),
        &release.release().package_id,
    )? != release.release().version
    {
        return Err(Error::Integrity);
    }
    let mut protected = vec![("sq.version".into(), version)];
    for member in &contract.files {
        let mut file = ProtectedFile::open(&current.join(&member.path))?;
        verify_member(file.file(), member, cancelled)?;
        protected.push((member.path.clone(), file));
    }
    let (_, manifest) = protected
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("manifest.json"))
        .ok_or(Error::Metadata)?;
    manifest.file().seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    manifest
        .file()
        .take(setup::MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > setup::MAX_MANIFEST {
        return Err(Error::Metadata);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
    if Profile::from_manifest(&manifest)? != anchor.profile() {
        return Err(Error::Integrity);
    }
    if manifest["build"]["build_fingerprint"].as_str() != Some(&contract.build_fingerprint)
        || manifest["build"]["package_version"].as_str()
            != Some(&release.release().version.to_string())
    {
        return Err(Error::Integrity);
    }
    let expected: HashSet<_> = contract
        .files
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .chain(std::iter::once("sq.version".into()))
        .collect();
    reject_extra_files(&current, &current, &expected, cancelled)?;
    Ok(protected)
}

fn reject_extra_files(
    root: &Path,
    directory: &Path,
    expected: &HashSet<String>,
    cancelled: &AtomicBool,
) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let _directory = ProtectedFile::directory(directory)?;
    for entry in std::fs::read_dir(directory)? {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let path = entry?.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Error::Integrity);
        }
        if metadata.is_dir() {
            let prefix = path
                .strip_prefix(root)
                .map_err(|_| Error::Metadata)?
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase()
                + "/";
            if !expected.iter().any(|name| name.starts_with(&prefix)) {
                return Err(Error::Integrity);
            }
            reject_extra_files(root, &path, expected, cancelled)?;
        } else if !metadata.is_file()
            || !expected.contains(
                &path
                    .strip_prefix(root)
                    .map_err(|_| Error::Metadata)?
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_ascii_lowercase(),
            )
        {
            return Err(Error::Integrity);
        }
    }
    Ok(())
}

fn provision_tools(
    anchor: &InstallationAnchor,
    release: &VerifiedRelease,
    installed: &mut [(String, ProtectedFile)],
    cancelled: &AtomicBool,
) -> Result<PathBuf> {
    let parent = anchor.coordination().join("recovery");
    std::fs::create_dir_all(&parent)?;
    let _parent = ProtectedFile::directory(&parent)?;
    let directory = tempfile::Builder::new()
        .prefix("tools-")
        .tempdir_in(&parent)?;
    let contract: &SetupContract = release.release().artifacts[0]
        .setup
        .as_ref()
        .ok_or(Error::Metadata)?;
    for name in ["noh-update-guard.exe", "noh-update-repair.exe"] {
        let relative = format!("bin/{name}");
        let member = contract
            .files
            .iter()
            .find(|file| file.path.eq_ignore_ascii_case(&relative))
            .ok_or(Error::Metadata)?;
        let (_, source) = installed
            .iter_mut()
            .find(|(path, _)| path.eq_ignore_ascii_case(&relative))
            .ok_or(Error::Metadata)?;
        source.file().seek(SeekFrom::Start(0))?;
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.path().join(name))?;
        let mut buffer = [0; 32768];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let count = source.file().read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()?;
        verify_member(&mut output, member, cancelled)?;
    }
    write_new(&directory.path().join("envelope.json"), release.envelope())?;
    write_new(
        &directory.path().join("complete"),
        b"authenticated-recovery-tools",
    )?;
    Ok(directory.keep())
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Select an external tool using independent keys and an exact installed version.
/// Completion markers are never trust roots and no public keys come from disk.
pub fn open_recovery_tool(
    anchor: &InstallationAnchor,
    keys: &[TrustKey],
    package_id: &str,
    version: &Version,
    name: &str,
    cancelled: &AtomicBool,
) -> Result<(PathBuf, ProtectedFile)> {
    if !matches!(name, "noh-update-guard.exe" | "noh-update-repair.exe") {
        return Err(Error::Metadata);
    }
    let directory = anchor.coordination().join("recovery");
    let _directory = ProtectedFile::directory(&directory)?;
    let mut selected = None;
    let mut expected_digest = None;
    for (index, entry) in std::fs::read_dir(&directory)?.enumerate() {
        if index >= 128 {
            return Err(Error::Metadata);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let path = entry?.path();
        let candidate = (|| -> Result<_> {
            let mut envelope = ProtectedFile::open(&path.join("envelope.json"))?;
            let mut bytes = Vec::new();
            envelope
                .file()
                .take(MAX_ENVELOPE as u64 + 1)
                .read_to_end(&mut bytes)?;
            let release = VerifiedRelease::verify(&bytes, keys, package_id)?;
            release.require_profile(anchor.profile())?;
            let artifact = exact_setup(&release, anchor.channel(), version)?;
            let member = artifact
                .setup
                .as_ref()
                .ok_or(Error::Metadata)?
                .files
                .iter()
                .find(|file| file.path.eq_ignore_ascii_case(&format!("bin/{name}")))
                .ok_or(Error::Metadata)?;
            let executable = path.join(name);
            let mut protection = ProtectedFile::open(&executable)?;
            verify_member(protection.file(), member, cancelled)?;
            Ok((executable, protection, member.sha256.clone()))
        })();
        match candidate {
            Ok((executable, protection, digest)) => {
                if expected_digest
                    .as_ref()
                    .is_some_and(|expected| expected != &digest)
                {
                    return Err(Error::Integrity);
                }
                expected_digest = Some(digest);
                if selected.is_none() {
                    selected = Some((executable, protection));
                }
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => {} // Preserve invalid records, but never execute their bytes.
        }
    }
    selected.ok_or(Error::Integrity)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(base: &Path) -> (InstallationAnchor, VerifiedRelease, Vec<TrustKey>) {
        let anchor = InstallationAnchor::create(base, "NOH", Channel::Stable).unwrap();
        let current = anchor.installation().join("current");
        std::fs::create_dir_all(current.join("bin")).unwrap();
        let fingerprint = "a".repeat(64);
        let manifest = serde_json::to_vec(&serde_json::json!({"build":{"build_fingerprint":fingerprint,"package_version":"2.0.0"}})).unwrap();
        let mut files = Vec::new();
        for name in [
            "noh.exe",
            "bin/noh-cli.exe",
            "bin/noh-mcp.exe",
            "bin/noh-update-guard.exe",
            "bin/noh-update-repair.exe",
            "manifest.json",
        ] {
            let bytes = if name.eq_ignore_ascii_case("manifest.json") {
                manifest.as_slice()
            } else {
                b"fixture"
            };
            std::fs::write(current.join(name), bytes).unwrap();
            files.push(InstalledFile {
                path: name.into(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            });
        }
        std::fs::write(
            current.join("sq.version"),
            b"<package><id>NOH</id><version>2.0.0</version></package>",
        )
        .unwrap();
        let release = Release {
            schema_version: 2,
            profile: Profile::Complete,
            package_id: "NOH".into(),
            version: Version::new(2, 0, 0),
            channel: Channel::Stable,
            notes: "Fixture".into(),
            artifacts: vec![Artifact {
                target: Target {
                    os: Os::Windows,
                    arch: Architecture::X64,
                },
                kind: PackageKind::WindowsSetup,
                base_version: None,
                file_name: "NOH-Setup.exe".into(),
                url: "https://github.com/example/noh/releases/download/test/NOH-Setup.exe".into(),
                size: 7,
                sha256: format!("{:x}", Sha256::digest(b"package")),
                setup: Some(SetupContract {
                    engine: setup::ENGINE.into(),
                    runtime_dependencies: vec![],
                    build_fingerprint: fingerprint,
                    installed_bytes: files.iter().map(|file| file.size).sum(),
                    files,
                }),
            }],
        };
        let (envelope, keys) = super::super::tests::signed(&release);
        (
            anchor,
            VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap(),
            keys,
        )
    }
    #[test]
    fn inventory_and_external_tools_are_verified_and_survive_missing_root() {
        let base = tempfile::tempdir().unwrap();
        let (anchor, release, keys) = fixture(base.path());
        let cancel = AtomicBool::new(false);
        let mut installed = protect_installed(&anchor, &release, &cancel).unwrap();
        let tools = provision_tools(&anchor, &release, &mut installed, &cancel).unwrap();
        assert!(std::fs::write(anchor.installation().join("current/noh.exe"), b"forged!").is_err());
        drop(installed);
        std::fs::rename(anchor.installation(), base.path().join("application.saved")).unwrap();
        let (_, protection) = open_recovery_tool(
            &anchor,
            &keys,
            "NOH",
            &Version::new(2, 0, 0),
            "noh-update-guard.exe",
            &cancel,
        )
        .unwrap();
        assert!(std::fs::write(tools.join("noh-update-guard.exe"), b"forged!").is_err());
        drop(protection);
        std::fs::write(tools.join("noh-update-guard.exe"), b"forged!").unwrap();
        assert!(
            open_recovery_tool(
                &anchor,
                &keys,
                "NOH",
                &Version::new(2, 0, 0),
                "noh-update-guard.exe",
                &cancel
            )
            .is_err()
        );
        assert!(
            open_recovery_tool(
                &anchor,
                &keys,
                "NOH",
                &Version::new(1, 0, 0),
                "noh-update-repair.exe",
                &cancel
            )
            .is_err()
        );
    }
    #[test]
    fn unexpected_files_changed_generation_and_cancelled_verification_fail_closed() {
        let base = tempfile::tempdir().unwrap();
        let (anchor, release, _) = fixture(base.path());
        let current = anchor.installation().join("current");
        let cancel = AtomicBool::new(false);
        std::fs::write(current.join("unexpected.dll"), b"extra").unwrap();
        assert!(protect_installed(&anchor, &release, &cancel).is_err());
        std::fs::remove_file(current.join("unexpected.dll")).unwrap();
        std::fs::write(current.join("noh.exe"), b"forged!").unwrap();
        assert!(protect_installed(&anchor, &release, &cancel).is_err());
        std::fs::write(current.join("noh.exe"), b"fixture").unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            protect_installed(&anchor, &release, &cancel),
            Err(Error::Cancelled)
        ));
        assert!(required_space(u64::MAX, 1).is_err());
    }
    #[test]
    #[ignore = "Requires the confined official-Setup integration runner and disposable HKCU identity"]
    fn native_full_setup_initial_update_and_missing_root_repair() {
        let case = std::fs::canonicalize(
            std::env::var_os("NOH_SETUP_INTEGRATION_CASE").expect("Explicit integration case"),
        )
        .unwrap();
        let allowed = std::fs::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(".mcp-dev/setup-integration"),
        )
        .unwrap();
        assert!(case.starts_with(&allowed) && case != allowed);
        let base = case.join("installation-base");
        std::fs::create_dir(&base).unwrap();
        let keys: Vec<TrustKey> =
            serde_json::from_slice(&std::fs::read(case.join("keys.json")).unwrap()).unwrap();
        let id = std::fs::read_to_string(case.join("package-id.txt")).unwrap();
        let cancel = AtomicBool::new(false);
        let load = |label: &str, action: Action| {
            let bytes = std::fs::read(case.join(format!("envelope-{label}.json"))).unwrap();
            let release = VerifiedRelease::verify(&bytes, &keys, &id).unwrap();
            let artifact = &release.release().artifacts[0];
            PreparedSetup::prepare(
                &bytes,
                &keys,
                &id,
                Channel::Stable,
                &base,
                &case
                    .join(format!("packages-{label}"))
                    .join(&artifact.file_name),
                &release.release().version,
                action,
                &cancel,
            )
            .unwrap()
        };
        let run = |prepared: PreparedSetup, attempt: &str| {
            let outcome = prepared
                .apply(
                    Duration::ZERO,
                    Duration::from_secs(90),
                    &case.join(format!("{attempt}.log")),
                    &cancel,
                )
                .unwrap();
            write_new(
                &case.join(format!("{attempt}.json")),
                &serde_json::to_vec_pretty(&outcome).unwrap(),
            )
            .unwrap();
            outcome
        };
        let a = Version::new(1, 0, 0);
        let b = Version::new(1, 0, 1);
        let initial = run(load("A", Action::Initialize), "initial");
        assert_eq!(initial.version, a);
        let anchor = InstallationAnchor::open(&base, &id, Some(Channel::Stable)).unwrap();
        let client = super::super::windows::RuntimeLease::acquire(&anchor.coordination()).unwrap();
        assert!(
            load("B", Action::Update { from: a.clone() })
                .apply(
                    Duration::ZERO,
                    Duration::from_secs(90),
                    &case.join("refused-client.log"),
                    &cancel
                )
                .is_err()
        );
        assert!(!case.join("refused-client.log").exists());
        drop(client);
        std::fs::rename(
            anchor.installation(),
            base.join("application.saved-by-test"),
        )
        .unwrap();
        assert!(
            open_recovery_tool(&anchor, &keys, &id, &a, "noh-update-repair.exe", &cancel).is_ok()
        );
        let repaired = run(load("A", Action::Repair), "repair-missing-root");
        assert_eq!(repaired.version, a);
        assert_eq!(repaired.retained_archive, initial.retained_archive);
        let updated = run(load("B", Action::Update { from: a }), "update");
        assert_eq!(updated.version, b);
        assert!(
            base.join("application.saved-by-test/current/noh.exe")
                .is_file()
        );
        assert!(
            open_recovery_tool(&anchor, &keys, &id, &b, "noh-update-guard.exe", &cancel).is_ok()
        );
        let package = updated.retained_archive.join("envelope.json");
        let bytes = std::fs::read(package).unwrap();
        let release = VerifiedRelease::verify(&bytes, &keys, &id).unwrap();
        let mut installed = protect_installed(&anchor, &release, &cancel).unwrap();
        let model = installed
            .iter_mut()
            .find(|(name, _)| name == "models/model.bin")
            .unwrap();
        model.1.file().seek(SeekFrom::Start(0)).unwrap();
        let mut model_bytes = Vec::new();
        model.1.file().read_to_end(&mut model_bytes).unwrap();
        assert_eq!(model_bytes, b"offline-model-B");
        assert_eq!(
            std::fs::read(case.join("userdata/sentinel")).unwrap(),
            b"preserve-user-data"
        );
        write_new(&case.join("passed.json"), b"{\"passed\":true,\"scope\":\"production adapter with synthetic payload; no GUI readiness claim\"}").unwrap();
    }
    #[test]
    fn mixed_case_signed_members_use_windows_path_semantics_everywhere() {
        let base = tempfile::tempdir().unwrap();
        let (anchor, release, _) = fixture(base.path());
        let mut metadata = release.release().clone();
        for member in &mut metadata.artifacts[0].setup.as_mut().unwrap().files {
            member.path = member.path.to_ascii_uppercase();
        }
        let (envelope, keys) = super::super::tests::signed(&metadata);
        let release = VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap();
        let cancel = AtomicBool::new(false);
        let mut installed = protect_installed(&anchor, &release, &cancel).unwrap();
        provision_tools(&anchor, &release, &mut installed, &cancel).unwrap();
        assert!(
            open_recovery_tool(
                &anchor,
                &keys,
                "NOH",
                &metadata.version,
                "noh-update-guard.exe",
                &cancel
            )
            .is_ok()
        );
        assert!(
            open_recovery_tool(
                &anchor,
                &keys,
                "NOH",
                &metadata.version,
                "noh-update-repair.exe",
                &cancel
            )
            .is_ok()
        );
    }
    #[test]
    fn manifest_bound_matches_metadata_postverification_and_runtime_startup() {
        let base = tempfile::tempdir().unwrap();
        let (anchor, release, _) = fixture(base.path());
        let path = anchor.installation().join("current/manifest.json");
        let mut metadata = release.release().clone();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.resize(setup::MAX_MANIFEST, b' ');
        std::fs::write(&path, &bytes).unwrap();
        let contract = metadata.artifacts[0].setup.as_mut().unwrap();
        let member = contract
            .files
            .iter_mut()
            .find(|file| file.path == "manifest.json")
            .unwrap();
        contract.installed_bytes += bytes.len() as u64 - member.size;
        member.size = bytes.len() as u64;
        member.sha256 = format!("{:x}", Sha256::digest(&bytes));
        let (envelope, keys) = super::super::tests::signed(&metadata);
        let valid = VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap();
        assert!(protect_installed(&anchor, &valid, &AtomicBool::new(false)).is_ok());
        assert!(
            super::super::windows::RuntimeLease::for_executable(
                &anchor.installation().join("current/noh.exe"),
                &"a".repeat(64)
            )
            .is_ok()
        );
        let contract = metadata.artifacts[0].setup.as_mut().unwrap();
        contract
            .files
            .iter_mut()
            .find(|file| file.path == "manifest.json")
            .unwrap()
            .size += 1;
        contract.installed_bytes += 1;
        let (envelope, keys) = super::super::tests::signed(&metadata);
        assert!(matches!(
            VerifiedRelease::verify(&envelope, &keys, "NOH"),
            Err(Error::Metadata)
        ));
        bytes.push(b' ');
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            super::super::windows::RuntimeLease::for_executable(
                &anchor.installation().join("current/noh.exe"),
                &"a".repeat(64)
            )
            .is_err()
        );
        assert!(
            setup::describe_payload(&anchor.installation().join("current"), &metadata.version)
                .is_err()
        );
    }
    #[test]
    fn retained_setup_is_exact_and_legacy_selection_stays_closed() {
        let base = tempfile::tempdir().unwrap();
        let (_, release, keys) = fixture(base.path());
        let source = base.path().join("NOH-Setup.exe");
        std::fs::write(&source, b"package").unwrap();
        let cancel = AtomicBool::new(false);
        let version = Version::new(2, 0, 0);
        let directory = retention::SETUP
            .retain_or_reuse(
                &base.path().join("retained"),
                &source,
                release.envelope(),
                &keys,
                "NOH",
                Target::native().unwrap(),
                Channel::Stable,
                &version,
                &cancel,
            )
            .unwrap();
        let retained = retention::SETUP
            .open(
                &directory,
                &keys,
                "NOH",
                Target::native().unwrap(),
                Channel::Stable,
                &version,
            )
            .unwrap();
        assert!(exact_setup(retained.release(), Channel::Stable, &version).is_ok());
        assert!(
            retained
                .release()
                .select(
                    Target::native().unwrap(),
                    Channel::Stable,
                    &Version::new(1, 0, 0)
                )
                .is_err()
        );
        assert!(
            retention::SETUP
                .open(
                    &directory,
                    &keys,
                    "NOH",
                    Target::native().unwrap(),
                    Channel::Beta,
                    &version
                )
                .is_err()
        );
        let target = Target::native().unwrap();
        let archive = base.path().join("retained");
        assert!(
            retention::find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &version,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        let legacy = super::super::tests::release();
        let (legacy_envelope, _) = super::super::tests::signed(&legacy);
        let legacy_directory = retention::retain_or_reuse(
            &archive,
            &source,
            &legacy_envelope,
            &keys,
            "NOH",
            target,
            Channel::Stable,
            &version,
            &cancel,
        )
        .unwrap();
        assert_ne!(legacy_directory, directory);
        assert_eq!(
            retention::find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &version,
                &cancel
            )
            .unwrap(),
            Some(legacy_directory)
        );
        assert_eq!(
            retention::SETUP
                .find_candidate(
                    &archive,
                    &keys,
                    "NOH",
                    target,
                    Channel::Stable,
                    &version,
                    &cancel
                )
                .unwrap(),
            Some(directory.clone())
        );
        drop(retained);
        std::fs::rename(
            directory.join("NOH-Setup.exe"),
            directory.join("saved-Setup.exe"),
        )
        .unwrap();
        // Legacy rejects the format before opening even the now-absent payload.
        assert!(matches!(
            retention::open(&directory, &keys, "NOH", target, Channel::Stable, &version),
            Err(Error::Target)
        ));
        assert!(matches!(
            retention::SETUP.open(&directory, &keys, "NOH", target, Channel::Stable, &version),
            Err(Error::Io(_))
        ));
    }

    #[test]
    fn retained_profiles_coexist_but_repair_cannot_switch_the_installation() {
        let base = tempfile::tempdir().unwrap();
        let (anchor, release, keys) = fixture(base.path());
        let elevated = super::windows_guard::is_elevated().unwrap();
        let source = base.path().join("NOH-Setup.exe");
        let archive = anchor.coordination().join("retained");
        let target = Target::native().unwrap();
        let cancel = AtomicBool::new(false);
        let version = release.release().version.clone();
        let mut entries = Vec::new();
        for profile in [Profile::Minimal, Profile::Standard, Profile::Complete] {
            let bytes = profile.as_str().as_bytes();
            std::fs::write(&source, bytes).unwrap();
            let mut metadata = release.release().clone();
            metadata.profile = profile;
            metadata.schema_version = if profile.is_complete() { 2 } else { 3 };
            metadata.artifacts[0].size = bytes.len() as u64;
            metadata.artifacts[0].sha256 = format!("{:x}", Sha256::digest(bytes));
            let envelope = super::super::tests::signed(&metadata).0;
            let directory = retention::SETUP
                .for_profile(profile)
                .retain_or_reuse(
                    &archive,
                    &source,
                    &envelope,
                    &keys,
                    "NOH",
                    target,
                    Channel::Stable,
                    &version,
                    &cancel,
                )
                .unwrap();
            if profile != Profile::Complete {
                let verified = VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap();
                assert!(matches!(
                    verified.require_profile(anchor.profile()),
                    Err(Error::Target)
                ));
                let prepared = PreparedSetup::prepare(
                    &envelope,
                    &keys,
                    "NOH",
                    Channel::Stable,
                    base.path(),
                    &source,
                    &version,
                    Action::Repair,
                    &cancel,
                );
                if elevated {
                    assert!(matches!(prepared, Err(Error::InstallationUnavailable)));
                } else {
                    assert!(matches!(prepared, Err(Error::Target)));
                }
            }
            entries.push((profile, directory));
        }
        assert_eq!(anchor.profile(), Profile::Complete);
        let check_wizard = || {
            // Keep the profile/downgrade proof on elevated CI too. Public entry
            // points must still refuse an elevated installer before those checks.
            assert!(matches!(
                bootstrap::require_no_downgrade(&anchor, &Version::new(1, 0, 0), &keys, "NOH"),
                Err(Error::Target)
            ));
            assert_eq!(
                bootstrap::require_no_downgrade(&anchor, &version, &keys, "NOH").unwrap(),
                Some(version.clone())
            );
            let plan = bootstrap::inspect(base.path(), &version, &keys, "NOH");
            if elevated {
                assert!(matches!(plan, Err(Error::InstallationUnavailable)));
            } else {
                let plan = plan.unwrap();
                assert!(!plan.initialize);
                assert_eq!(plan.profile, Some(Profile::Complete));
                assert!(matches!(
                    bootstrap::inspect(base.path(), &Version::new(1, 0, 0), &keys, "NOH"),
                    Err(Error::Target)
                ));
            }
        };
        check_wizard();
        let saved = base.path().join("application.saved");
        std::fs::rename(anchor.installation(), &saved).unwrap();
        check_wizard();
        std::fs::rename(saved, anchor.installation()).unwrap();
        for (profile, expected) in &entries {
            let storage = retention::SETUP.for_profile(*profile);
            assert_eq!(
                storage
                    .find_candidate(
                        &archive,
                        &keys,
                        "NOH",
                        target,
                        Channel::Stable,
                        &version,
                        &cancel
                    )
                    .unwrap()
                    .as_ref(),
                Some(expected)
            );
            for (other, directory) in &entries {
                let result =
                    storage.open(directory, &keys, "NOH", target, Channel::Stable, &version);
                assert_eq!(result.is_ok(), profile == other);
            }
        }
        // Same profile/version but a different signed package remains a conflict.
        let mut metadata = release.release().clone();
        metadata.profile = Profile::Standard;
        metadata.schema_version = 3;
        let bytes = b"conflicting-standard";
        std::fs::write(&source, bytes).unwrap();
        metadata.artifacts[0].size = bytes.len() as u64;
        metadata.artifacts[0].sha256 = format!("{:x}", Sha256::digest(bytes));
        let envelope = super::super::tests::signed(&metadata).0;
        assert!(matches!(
            retention::SETUP
                .for_profile(Profile::Standard)
                .retain_or_reuse(
                    &archive,
                    &source,
                    &envelope,
                    &keys,
                    "NOH",
                    target,
                    Channel::Stable,
                    &version,
                    &cancel
                ),
            Err(Error::Integrity)
        ));
    }
}
