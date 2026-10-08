//! Authenticated, independently retained full packages. No installer side effects.
//! Run on a worker; callers must keep the archive outside the replaced installation.
use super::*;

const MAX_ARCHIVES: usize = 128;

/// Callers choose the format before reading a catalog or opening package bytes.
#[derive(Clone, Copy)]
pub struct Format(PackageKind, Profile);
pub const SETUP: Format = Format(PackageKind::WindowsSetup, Profile::Complete);
pub const NUPKG: Format = Format(PackageKind::Full, Profile::Complete);

pub fn find_candidate(
    archive: &Path,
    keys: &[TrustKey],
    package_id: &str,
    target: Target,
    channel: Channel,
    version: &Version,
    cancelled: &AtomicBool,
) -> Result<Option<PathBuf>> {
    NUPKG.find_candidate(
        archive, keys, package_id, target, channel, version, cancelled,
    )
}
pub fn retain_or_reuse(
    archive: &Path,
    source: &Path,
    envelope: &[u8],
    keys: &[TrustKey],
    package_id: &str,
    target: Target,
    channel: Channel,
    version: &Version,
    cancelled: &AtomicBool,
) -> Result<PathBuf> {
    NUPKG.retain_or_reuse(
        archive, source, envelope, keys, package_id, target, channel, version, cancelled,
    )
}
pub fn open(
    directory: &Path,
    keys: &[TrustKey],
    package_id: &str,
    target: Target,
    channel: Channel,
    version: &Version,
) -> Result<RetainedPackage> {
    NUPKG.open(directory, keys, package_id, target, channel, version)
}
pub fn open_cancellable(
    directory: &Path,
    keys: &[TrustKey],
    package_id: &str,
    target: Target,
    channel: Channel,
    version: &Version,
    cancelled: &AtomicBool,
) -> Result<RetainedPackage> {
    NUPKG.open_cancellable(
        directory, keys, package_id, target, channel, version, cancelled,
    )
}

impl Format {
    pub fn for_profile(self, profile: Profile) -> Self {
        Self(self.0, profile)
    }
    /// Discover a completed, signed exact-version record. This is metadata lookup,
    /// never package authority: callers must use open_cancellable before consuming it.
    /// Conflicting signed full digests for one identity/version fail closed.
    pub fn find_candidate(
        &self,
        archive: &Path,
        keys: &[TrustKey],
        package_id: &str,
        target: Target,
        channel: Channel,
        version: &Version,
        cancelled: &AtomicBool,
    ) -> Result<Option<PathBuf>> {
        let entries = match std::fs::read_dir(archive) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let entry = entry?;
            if paths.len() >= MAX_ARCHIVES {
                return Err(Error::Metadata);
            }
            paths.push(entry.path());
        }
        paths.sort();
        let mut selected: Option<(PathBuf, String)> = None;
        for directory in paths {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if !directory
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("retained-"))
            {
                continue;
            }
            let read = |name: &str, limit: u64| -> Result<Vec<u8>> {
                #[cfg(windows)]
                let mut protected = windows::ProtectedFile::open(&directory.join(name))?;
                #[cfg(windows)]
                let input = protected.file();
                #[cfg(not(windows))]
                let mut file = File::open(directory.join(name))?;
                #[cfg(not(windows))]
                let input = &mut file;
                let mut bytes = Vec::new();
                input.take(limit + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > limit {
                    return Err(Error::Metadata);
                }
                Ok(bytes)
            };
            if read("complete", 64).ok().as_deref() != Some(b"retained-full-package") {
                continue;
            }
            let Some(release) = read("envelope.json", MAX_ENVELOPE as u64)
                .ok()
                .and_then(|bytes| VerifiedRelease::verify(&bytes, keys, package_id).ok())
            else {
                continue;
            };
            let Ok(artifact) = self.exact_artifact(&release, target, channel, version) else {
                continue;
            };
            if let Some((_, digest)) = &selected {
                if digest != &artifact.sha256 {
                    return Err(Error::Integrity);
                }
            } else {
                selected = Some((directory, artifact.sha256.clone()));
            }
        }
        Ok(selected.map(|(path, _)| path))
    }

    /// Keep a verified existing copy or create a new immutable full archive.
    /// Corrupt payloads in selectable signed records fail. Invalid metadata is
    /// ignored and preserved; no existing record is overwritten or pruned.
    pub fn retain_or_reuse(
        &self,
        archive: &Path,
        source: &Path,
        envelope: &[u8],
        keys: &[TrustKey],
        package_id: &str,
        target: Target,
        channel: Channel,
        version: &Version,
        cancelled: &AtomicBool,
    ) -> Result<PathBuf> {
        std::fs::create_dir_all(archive)?;
        #[cfg(windows)]
        let _catalog_lock = windows::ExclusiveLease::acquire(archive)?;
        let requested = VerifiedRelease::verify(envelope, keys, package_id)?;
        let expected = self.exact_artifact(&requested, target, channel, version)?;
        if let Some(directory) = self.find_candidate(
            archive, keys, package_id, target, channel, version, cancelled,
        )? {
            let retained = self.open_cancellable(
                &directory, keys, package_id, target, channel, version, cancelled,
            )?;
            let actual = self.exact_artifact(retained.release(), target, channel, version)?;
            if actual.sha256 != expected.sha256 || actual.size != expected.size {
                return Err(Error::Integrity);
            }
            return Ok(directory);
        }
        self.retain(
            archive, source, envelope, keys, package_id, target, channel, version, cancelled,
        )
    }
}

/// Holds the authenticated archive inputs open for the consumer's lifetime.
/// On Windows, ancestor and file protections prevent replacement after hashing.
pub struct RetainedPackage {
    release: VerifiedRelease,
    #[cfg(windows)]
    package: windows::ProtectedFile,
    #[cfg(windows)]
    _envelope: windows::ProtectedFile,
    #[cfg(not(windows))]
    package: File,
}
impl RetainedPackage {
    pub fn release(&self) -> &VerifiedRelease {
        &self.release
    }
    pub fn file(&mut self) -> &mut File {
        #[cfg(windows)]
        {
            self.package.file()
        }
        #[cfg(not(windows))]
        {
            &mut self.package
        }
    }
}

/// Reauthenticate retained inputs before repair, even when a completion marker
/// exists. Trust keys are supplied independently, never read from the archive.
impl Format {
    pub fn open(
        &self,
        directory: &Path,
        keys: &[TrustKey],
        package_id: &str,
        target: Target,
        channel: Channel,
        version: &Version,
    ) -> Result<RetainedPackage> {
        self.open_cancellable(
            directory,
            keys,
            package_id,
            target,
            channel,
            version,
            &AtomicBool::new(false),
        )
    }

    pub fn open_cancellable(
        &self,
        directory: &Path,
        keys: &[TrustKey],
        package_id: &str,
        target: Target,
        channel: Channel,
        version: &Version,
        cancelled: &AtomicBool,
    ) -> Result<RetainedPackage> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        #[cfg(windows)]
        let mut envelope = windows::ProtectedFile::open(&directory.join("envelope.json"))?;
        #[cfg(windows)]
        let input = envelope.file();
        #[cfg(not(windows))]
        let mut envelope = File::open(directory.join("envelope.json"))?;
        #[cfg(not(windows))]
        let input = &mut envelope;
        let mut bytes = Vec::new();
        input
            .take(MAX_ENVELOPE as u64 + 1)
            .read_to_end(&mut bytes)?;
        let release = VerifiedRelease::verify(&bytes, keys, package_id)?;
        let artifact = self.exact_artifact(&release, target, channel, version)?;
        #[cfg(windows)]
        let mut package = windows::ProtectedFile::open(&directory.join(&artifact.file_name))?;
        #[cfg(windows)]
        verify_package_cancellable(package.file(), artifact, cancelled)?;
        #[cfg(windows)]
        package.file().seek(SeekFrom::Start(0))?;
        #[cfg(not(windows))]
        let mut package = File::open(directory.join(&artifact.file_name))?;
        #[cfg(not(windows))]
        verify_package_cancellable(&mut package, artifact, cancelled)?;
        #[cfg(not(windows))]
        package.seek(SeekFrom::Start(0))?;
        Ok(RetainedPackage {
            release,
            package,
            #[cfg(windows)]
            _envelope: envelope,
        })
    }

    /// Every use authenticates the original envelope and hashes the complete package.
    /// Exact-version recovery is deliberately separate from newer-version discovery.
    fn exact_artifact<'a>(
        &self,
        release: &'a VerifiedRelease,
        target: Target,
        channel: Channel,
        version: &Version,
    ) -> Result<&'a Artifact> {
        release.require_profile(self.1)?;
        if release.release().version != *version || release.release().channel != channel {
            return Err(Error::Target);
        }
        release
            .release()
            .artifacts
            .iter()
            .find(|artifact| artifact.target == target && artifact.kind == self.0)
            .ok_or(Error::Target)
    }

    /// A fresh archive is published only after both synced inputs pass authentication.
    /// No existing archive is overwritten or pruned, including after a failed update.
    fn retain(
        &self,
        archive: &Path,
        source: &Path,
        envelope: &[u8],
        keys: &[TrustKey],
        package_id: &str,
        target: Target,
        channel: Channel,
        version: &Version,
        cancelled: &AtomicBool,
    ) -> Result<PathBuf> {
        let release = VerifiedRelease::verify(envelope, keys, package_id)?;
        let artifact = self.exact_artifact(&release, target, channel, version)?;
        #[cfg(windows)]
        let mut protected = windows::ProtectedFile::open(source)?;
        #[cfg(windows)]
        let input = protected.file();
        #[cfg(not(windows))]
        let mut source_file = File::open(source)?;
        #[cfg(not(windows))]
        let input = &mut source_file;
        // Authenticate the stream again after copying; mutable Unix inputs cannot
        // turn an earlier successful hash into authority for a different copy.
        std::fs::create_dir_all(archive)?;
        if std::fs::read_dir(archive)?.take(MAX_ARCHIVES).count() >= MAX_ARCHIVES {
            return Err(Error::Metadata);
        }
        let directory = tempfile::Builder::new()
            .prefix("retained-")
            .tempdir_in(archive)?;
        let package = directory.path().join(&artifact.file_name);
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&package)?;
        let mut buffer = [0u8; 32 * 1024];
        let mut total = 0u64;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > artifact.size {
                return Err(Error::Integrity);
            }
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()?;
        verify_package_cancellable(&mut output, artifact, cancelled)?;
        let mut metadata = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.path().join("envelope.json"))?;
        metadata.write_all(envelope)?;
        metadata.sync_all()?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Marker means complete persistence, never authority to install. Readers
        // still authenticate envelope, exact identity and bytes on every use.
        let mut marker = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.path().join("complete"))?;
        marker.write_all(b"retained-full-package")?;
        marker.sync_all()?;
        drop(marker);
        drop(metadata);
        drop(output);
        Ok(directory.keep())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_keeps_both_generations_and_preserves_untrusted_records() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let archive = root.path().join("archive");
        std::fs::write(&source, b"package").unwrap();
        std::fs::create_dir_all(archive.join("retained-incomplete")).unwrap();
        std::fs::write(
            archive.join("retained-incomplete/envelope.json"),
            b"untrusted",
        )
        .unwrap();
        let mut release = super::super::tests::release();
        let target = Target {
            os: Os::Windows,
            arch: Architecture::X64,
        };
        let cancel = AtomicBool::new(false);
        let mut saved = Vec::new();
        for version in [Version::new(2, 0, 0), Version::new(3, 0, 0)] {
            release.version = version.clone();
            let (envelope, keys) = super::super::tests::signed(&release);
            let directory = retain_or_reuse(
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
            saved.push((version, directory, keys));
        }
        for (version, directory, keys) in saved {
            assert_eq!(
                find_candidate(
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
            assert!(open(&directory, &keys, "NOH", target, Channel::Stable, &version).is_ok());
        }
        assert_eq!(
            std::fs::read(archive.join("retained-incomplete/envelope.json")).unwrap(),
            b"untrusted"
        );
        assert_eq!(
            std::fs::read_dir(&archive)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            3
        );
    }
    #[test]
    fn catalog_selects_exact_version_reuses_good_copies_and_refuses_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, b"package").unwrap();
        let archive = root.path().join("archive");
        let mut release = super::super::tests::release();
        let (envelope, keys) = super::super::tests::signed(&release);
        let target = Target {
            os: Os::Windows,
            arch: Architecture::X64,
        };
        let cancel = AtomicBool::new(false);
        let first = retain_or_reuse(
            &archive,
            &source,
            &envelope,
            &keys,
            "NOH",
            target,
            Channel::Stable,
            &release.version,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            )
            .unwrap(),
            Some(first.clone())
        );
        assert!(
            find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &Version::new(1, 0, 0),
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert!(
            find_candidate(
                &archive,
                &keys,
                "Other",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert!(
            find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Beta,
                &release.version,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            retain_or_reuse(
                &archive,
                &source,
                &envelope,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            )
            .unwrap(),
            first
        );
        assert_eq!(
            std::fs::read_dir(&archive)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            1
        );
        std::fs::write(first.join("noh-Windows.nupkg"), b"changed").unwrap();
        assert!(matches!(
            retain_or_reuse(
                &archive,
                &source,
                &envelope,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            ),
            Err(Error::Integrity)
        ));
        std::fs::write(first.join("noh-Windows.nupkg"), b"package").unwrap();
        std::fs::write(&source, b"changed").unwrap();
        release
            .artifacts
            .iter_mut()
            .for_each(|artifact| artifact.sha256 = format!("{:x}", Sha256::digest(b"changed")));
        let (other_envelope, _) = super::super::tests::signed(&release);
        NUPKG
            .retain(
                &archive,
                &source,
                &other_envelope,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel,
            )
            .unwrap();
        assert!(matches!(
            find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            ),
            Err(Error::Integrity)
        ));
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            find_candidate(
                &archive,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            ),
            Err(Error::Cancelled)
        ));
    }
    #[test]
    fn retention_checks_exact_identity_and_cleans_failed_or_cancelled_copies() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let archive = root.path().join("archive");
        std::fs::write(&source, b"package").unwrap();
        let release = super::super::tests::release();
        let (envelope, keys) = super::super::tests::signed(&release);
        let target = Target {
            os: Os::Windows,
            arch: Architecture::X64,
        };
        let cancel = AtomicBool::new(false);
        let run = |version: &Version| {
            NUPKG.retain(
                &archive,
                &source,
                &envelope,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                version,
                &cancel,
            )
        };
        assert!(matches!(run(&Version::new(1, 0, 0)), Err(Error::Target)));
        let saved = run(&release.version).unwrap();
        assert_eq!(
            std::fs::read(saved.join("noh-Windows.nupkg")).unwrap(),
            b"package"
        );
        assert_eq!(
            std::fs::read(saved.join("envelope.json")).unwrap(),
            envelope
        );
        let mut retained = open(
            &saved,
            &keys,
            "NOH",
            target,
            Channel::Stable,
            &release.version,
        )
        .unwrap();
        let mut restored = Vec::new();
        retained.file().read_to_end(&mut restored).unwrap();
        assert_eq!(restored, b"package");
        drop(retained);
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            open_cancellable(
                &saved,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version,
                &cancel
            ),
            Err(Error::Cancelled)
        ));
        let verified = VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap();
        let artifact = NUPKG
            .exact_artifact(&verified, target, Channel::Stable, &release.version)
            .unwrap();
        let mut verifying = File::open(&source).unwrap();
        assert!(matches!(
            verify_package_cancellable(&mut verifying, artifact, &cancel),
            Err(Error::Cancelled)
        ));
        assert!(matches!(run(&release.version), Err(Error::Cancelled)));
        cancel.store(false, Ordering::Relaxed);
        std::fs::write(&source, b"changed").unwrap();
        assert!(matches!(run(&release.version), Err(Error::Integrity)));
        assert_eq!(std::fs::read_dir(&archive).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(saved.join("noh-Windows.nupkg")).unwrap(),
            b"package"
        );
        std::fs::write(saved.join("noh-Windows.nupkg"), b"changed").unwrap();
        assert!(matches!(
            open(
                &saved,
                &keys,
                "NOH",
                target,
                Channel::Stable,
                &release.version
            ),
            Err(Error::Integrity)
        ));
    }
}
