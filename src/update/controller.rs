use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Checking,
    Available {
        version: Version,
        notes: String,
        size: u64,
    },
    Downloading {
        received: u64,
        total: u64,
    },
    Verifying,
    Ready {
        package: PathBuf,
        version: Version,
    },
    Applying,
    Failed {
        message: String,
    },
}

/// Run on the update worker, never on the native UI thread. No install side effects.
pub struct Controller {
    transport: GithubTransport,
    keys: Vec<TrustKey>,
    package_id: String,
    target: Target,
    channel: Channel,
    current: Version,
    kind: PackageKind,
    profile: Profile,
    release: Option<VerifiedRelease>,
    state: State,
    retry_at: Option<std::time::Instant>,
    staging_dir: Option<tempfile::TempDir>,
}
impl Controller {
    pub fn new(
        transport: GithubTransport,
        keys: Vec<TrustKey>,
        package_id: String,
        target: Target,
        channel: Channel,
        current: Version,
    ) -> Self {
        Self {
            transport,
            keys,
            package_id,
            target,
            channel,
            current,
            kind: PackageKind::Full,
            profile: Profile::Complete,
            release: None,
            state: State::Idle,
            retry_at: None,
            staging_dir: None,
        }
    }
    /// Opt into the Setup contract before starting an update operation.
    pub fn with_setup(mut self) -> Self {
        self.discard();
        self.kind = PackageKind::WindowsSetup;
        self
    }
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.discard();
        self.profile = profile;
        self
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    /// Discard an uncommitted download on the worker; retain any retry deadline.
    pub fn discard(&mut self) {
        self.staging_dir = None;
        self.release = None;
        self.state = State::Idle;
    }
    /// No platform may enter Applying before its authenticated installer gate passes.
    pub fn apply(&mut self) -> Result<()> {
        Err(Error::InstallationUnavailable)
    }
    /// Move the unique download owner to the platform handoff worker.
    /// A path alone cannot transfer responsibility for temporary-directory cleanup.
    pub fn take_ready(&mut self) -> Result<PreparedDownload> {
        let package = match &self.state {
            State::Ready { package, .. } => package.clone(),
            _ => return Err(Error::Target),
        };
        let release = self.release.as_ref().ok_or(Error::Target)?;
        let envelope = release.envelope().to_vec();
        let version = release.release().version.clone();
        let directory = self.staging_dir.take().ok_or(Error::Target)?;
        self.release = None;
        self.state = State::Idle;
        Ok(PreparedDownload {
            directory,
            package,
            envelope,
            channel: self.channel,
            version,
        })
    }
    pub fn retry_after(&self) -> Duration {
        self.retry_at
            .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_default()
    }
    pub(crate) fn retry_deadline(&self) -> Option<std::time::Instant> {
        self.retry_at
    }
    pub async fn check(&mut self, url: &str, cancelled: &AtomicBool) -> Result<()> {
        let delay = self.retry_after();
        if !delay.is_zero() {
            return Err(Error::RateLimited(delay.as_secs().max(1)));
        }
        self.release = None;
        self.staging_dir = None;
        self.state = State::Checking;
        let result = async {
            let bytes = self.transport.envelope(url, cancelled).await?;
            let release = VerifiedRelease::verify(&bytes, &self.keys, &self.package_id)?;
            release.require_profile(self.profile)?;
            let available =
                release.select_kind(self.target, self.channel, &self.current, self.kind)?;
            self.state = match available {
                Some(artifact) => State::Available {
                    version: release.release().version.clone(),
                    notes: release.release().notes.clone(),
                    size: artifact.size,
                },
                None => State::Idle,
            };
            self.release = Some(release);
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.failed(error);
        }
        result
    }
    pub async fn download(
        &mut self,
        folder: &Path,
        cancelled: &AtomicBool,
        mut notify: impl FnMut(&State),
    ) -> Result<()> {
        let delay = self.retry_after();
        if !delay.is_zero() {
            return Err(Error::RateLimited(delay.as_secs().max(1)));
        }
        if !matches!(self.state, State::Available { .. }) {
            return Err(Error::Target);
        }
        let release = self.release.as_ref().ok_or(Error::Target)?;
        let staging_dir = match attempt_directory(folder) {
            Ok(directory) => directory,
            Err(error) => {
                self.failed(&error);
                notify(&self.state);
                return Err(error);
            }
        };
        let version = release.release().version.clone();
        let total = release
            .select_kind(self.target, self.channel, &self.current, self.kind)?
            .ok_or(Error::Target)?
            .size;
        self.state = State::Downloading { received: 0, total };
        notify(&self.state);
        let state = &mut self.state;
        let result = self
            .transport
            .stage_kind(
                release,
                self.target,
                self.channel,
                &self.current,
                self.kind,
                staging_dir.path(),
                cancelled,
                |received, total| {
                    *state = if received == total {
                        State::Verifying
                    } else {
                        State::Downloading { received, total }
                    };
                    notify(state);
                },
            )
            .await;
        match result {
            Ok(package) => {
                self.staging_dir = Some(staging_dir);
                self.state = State::Ready { package, version };
                notify(&self.state);
                Ok(())
            }
            Err(error) => {
                self.failed(&error);
                notify(&self.state);
                Err(error)
            }
        }
    }
    /// Local full-package ingestion for disposable Windows qualification only.
    /// Uses the controller's independent trust, never an archive-provided key.
    #[cfg(windows)]
    pub fn import_qualification_archive(
        &mut self,
        archive: &Path,
        folder: &Path,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        self.discard();
        self.state = State::Verifying;
        let result = (|| {
            if !trust::qualification_enabled() {
                return Err(Error::InstallationUnavailable);
            }
            let allowed = if let Some(root) = option_env!("NOH_UPDATE_QUALIFICATION_ROOT") {
                std::fs::canonicalize(root)?
            } else {
                super::windows_anchor::InstallationAnchor::for_executable(
                    &std::env::current_exe()?,
                    &self.package_id,
                )?
                .ok_or(Error::InstallationUnavailable)?
                .base()
                .to_path_buf()
            };
            let archive = std::fs::canonicalize(archive)?;
            let parent =
                std::fs::canonicalize(folder.parent().ok_or(Error::InstallationUnavailable)?)?;
            let candidate = parent.join(folder.file_name().ok_or(Error::InstallationUnavailable)?);
            if archive == allowed
                || !archive.starts_with(&allowed)
                || !parent.starts_with(&allowed)
                || candidate.starts_with(&archive)
                || archive.starts_with(&candidate)
            {
                return Err(Error::InstallationUnavailable);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            std::fs::create_dir_all(folder)?;
            let folder = std::fs::canonicalize(folder)?;
            if archive == allowed
                || folder == allowed
                || !archive.starts_with(&allowed)
                || !folder.starts_with(&allowed)
                || archive.starts_with(&folder)
                || folder.starts_with(&archive)
            {
                return Err(Error::InstallationUnavailable);
            }
            let mut metadata = super::windows::ProtectedFile::open(&archive.join("envelope.json"))?;
            let mut envelope = Vec::new();
            metadata
                .file()
                .take((MAX_ENVELOPE + 1) as u64)
                .read_to_end(&mut envelope)?;
            let release = VerifiedRelease::verify(&envelope, &self.keys, &self.package_id)?;
            release.require_profile(self.profile)?;
            let artifact = release
                .select_kind(self.target, self.channel, &self.current, self.kind)?
                .ok_or(Error::Target)?;
            let storage = if self.kind == PackageKind::WindowsSetup {
                super::retention::SETUP.for_profile(self.profile)
            } else {
                super::retention::NUPKG
            };
            let mut retained = storage.open_cancellable(
                &archive,
                &self.keys,
                &self.package_id,
                self.target,
                self.channel,
                &release.release().version,
                cancelled,
            )?;
            let directory = attempt_directory(&folder)?;
            let package = directory.path().join(&artifact.file_name);
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&package)?;
            let mut block = [0u8; 32768];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let read = retained.file().read(&mut block)?;
                if read == 0 {
                    break;
                }
                output.write_all(&block[..read])?;
            }
            output.sync_all()?;
            drop(output);
            verify_package_cancellable(&mut File::open(&package)?, artifact, cancelled)?;
            let version = release.release().version.clone();
            self.release = Some(release);
            self.staging_dir = Some(directory);
            self.state = State::Ready { package, version };
            Ok(())
        })();
        if let Err(error) = &result {
            self.failed(error);
        }
        result
    }
    fn failed(&mut self, error: &Error) {
        if let Error::RateLimited(seconds) = error {
            self.retry_at = Some(std::time::Instant::now() + Duration::from_secs(*seconds));
        }
        self.state = if matches!(error, Error::Cancelled) {
            State::Idle
        } else {
            State::Failed {
                message: error.to_string(),
            }
        };
    }
}

/// Unique ownership of a verified download; dropping before handoff removes it.
pub struct PreparedDownload {
    pub(crate) directory: tempfile::TempDir,
    pub(crate) package: PathBuf,
    pub(crate) envelope: Vec<u8>,
    pub(crate) channel: Channel,
    pub(crate) version: Version,
}

fn attempt_directory(folder: &Path) -> Result<tempfile::TempDir> {
    std::fs::create_dir_all(folder)?;
    Ok(tempfile::Builder::new()
        .prefix("noh-update-")
        .tempdir_in(folder)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn ordinary_build_refuses_local_qualification_ingress_without_writing() {
        if option_env!("NOH_UPDATE_QUALIFICATION_ROOT").is_some() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("not-created");
        let mut updater = controller();
        assert!(matches!(
            updater.import_qualification_archive(root.path(), &cache, &AtomicBool::new(false)),
            Err(Error::InstallationUnavailable)
        ));
        assert!(!cache.exists());
        assert!(updater.take_ready().is_err());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires an explicit disposable compiled qualification root"]
    fn local_archive_qualification_owns_authenticated_newer_bytes() {
        let allowed =
            option_env!("NOH_UPDATE_QUALIFICATION_ROOT").expect("compiled qualification root");
        let root = tempfile::tempdir_in(allowed).unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, b"package").unwrap();
        let release = super::super::tests::release();
        let (envelope, keys) = super::super::tests::signed(&release);
        let mut updater = controller();
        updater.keys = keys.clone();
        let cancel = AtomicBool::new(false);
        let archive = super::super::retention::retain_or_reuse(
            &root.path().join("archive"),
            &source,
            &envelope,
            &keys,
            "NOH",
            updater.target,
            Channel::Stable,
            &release.version,
            &cancel,
        )
        .unwrap();
        let cache = root.path().join("cache");
        updater
            .import_qualification_archive(&archive, &cache, &cancel)
            .unwrap();
        let owned = updater.take_ready().unwrap();
        let stage = owned.directory.path().to_owned();
        assert_eq!(std::fs::read(&owned.package).unwrap(), b"package");
        assert_eq!(owned.envelope, envelope);
        drop(owned);
        assert!(!stage.exists());
        assert!(archive.exists());
        updater.current = release.version.clone();
        assert!(matches!(
            updater.import_qualification_archive(&archive, &cache, &cancel),
            Err(Error::Target)
        ));
        updater.current = Version::new(1, 0, 0);
        std::fs::write(archive.join("noh-Windows.nupkg"), b"changed").unwrap();
        assert!(matches!(
            updater.import_qualification_archive(&archive, &cache, &cancel),
            Err(Error::Integrity)
        ));
        assert!(updater.take_ready().is_err());
        std::fs::write(archive.join("noh-Windows.nupkg"), b"package").unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            updater.import_qualification_archive(&archive, &cache, &cancel),
            Err(Error::Cancelled)
        ));
        let outside = tempfile::tempdir().unwrap();
        let refused_cache = outside.path().join("not-created");
        assert!(matches!(
            updater.import_qualification_archive(&archive, &refused_cache, &AtomicBool::new(false)),
            Err(Error::InstallationUnavailable)
        ));
        assert!(!refused_cache.exists());
        let nested_cache = archive.join("not-created");
        assert!(matches!(
            updater.import_qualification_archive(&archive, &nested_cache, &AtomicBool::new(false)),
            Err(Error::InstallationUnavailable)
        ));
        assert!(!nested_cache.exists());
    }
    fn controller() -> Controller {
        Controller::new(
            GithubTransport::new(None).unwrap(),
            Vec::new(),
            "NOH".into(),
            Target {
                os: Os::Windows,
                arch: Architecture::X64,
            },
            Channel::Stable,
            Version::new(1, 0, 0),
        )
    }
    #[tokio::test]
    async fn cancelled_check_is_idle_and_invalid_source_is_failed() {
        let mut updater = controller();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            updater.check("https://api.github.com/a", &cancel).await,
            Err(Error::Cancelled)
        ));
        assert_eq!(updater.state(), &State::Idle);
        cancel.store(false, Ordering::Relaxed);
        assert!(matches!(
            updater.check("http://evil.example/a", &cancel).await,
            Err(Error::Url)
        ));
        assert!(matches!(updater.state(), State::Failed { .. }));
    }
    #[tokio::test]
    async fn throttling_prevents_network_retry_and_download_requires_availability() {
        let mut updater = controller();
        updater.failed(&Error::RateLimited(60));
        assert!(updater.retry_after() > Duration::from_secs(59));
        assert!(matches!(
            updater
                .check("http://evil.example/a", &AtomicBool::new(false))
                .await,
            Err(Error::RateLimited(_))
        ));
        let folder = tempfile::tempdir().unwrap();
        let mut updater = controller();
        assert!(matches!(
            updater
                .download(folder.path(), &AtomicBool::new(false), |_| {})
                .await,
            Err(Error::Target)
        ));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
    #[test]
    fn each_attempt_is_isolated_and_owned_temporary_data_is_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let first = attempt_directory(root.path()).unwrap();
        let old = first.path().join("noh.nupkg");
        std::fs::write(&old, b"altered existing cache").unwrap();
        let second = attempt_directory(root.path()).unwrap();
        assert_ne!(first.path(), second.path());
        assert!(!second.path().join("noh.nupkg").exists());
        assert_eq!(std::fs::read(&old).unwrap(), b"altered existing cache");
        drop(first);
        assert!(!old.exists());
        drop(second);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires independently configured standalone guardian and disposable signed fixture"]
    fn standalone_handoff_transfers_controller_ownership_and_cancellation_cleans() {
        use crate::update::{windows::RuntimeLease, windows_handoff::WindowsHandoff};
        let trial = PathBuf::from(
            std::env::var_os("NOH_UPDATE_HANDOFF_TRIAL").expect("NOH_UPDATE_HANDOFF_TRIAL"),
        );
        let guardian =
            PathBuf::from(std::env::var_os("NOH_UPDATE_GUARDIAN").expect("NOH_UPDATE_GUARDIAN"));
        let helper =
            PathBuf::from(std::env::var_os("NOH_UPDATE_HELPER").expect("NOH_UPDATE_HELPER"));
        let case = tempfile::Builder::new()
            .prefix("controller-handoff-")
            .tempdir_in(&trial)
            .unwrap()
            .keep();
        let root = case.join("installation");
        std::fs::create_dir_all(root.join("current")).unwrap();
        std::fs::write(root.join(".portable"), b"").unwrap();
        std::fs::copy(&helper, root.join("Update.exe")).unwrap();
        std::fs::copy(
            trial.join("installation/current/probe.exe"),
            root.join("current/probe.exe"),
        )
        .unwrap();
        let old_manifest = std::fs::read_to_string(trial.join("installation/current/sq.version"))
            .unwrap()
            .replace("<version>1.0.1</version>", "<version>1.0.0</version>");
        std::fs::write(root.join("current/sq.version"), &old_manifest).unwrap();
        let envelope = std::fs::read(trial.join("envelope.json")).unwrap();
        let keys: Vec<TrustKey> =
            serde_json::from_slice(&std::fs::read(trial.join("public-keys.json")).unwrap())
                .unwrap();
        let make_ready = || {
            let release =
                VerifiedRelease::verify(&envelope, &keys, "NohUpdateBoundaryProbe").unwrap();
            let staging = attempt_directory(&case).unwrap();
            let artifact = release
                .select(
                    Target {
                        os: Os::Windows,
                        arch: Architecture::X64,
                    },
                    Channel::Stable,
                    &Version::new(1, 0, 0),
                )
                .unwrap()
                .unwrap();
            let package = staging.path().join(&artifact.file_name);
            std::fs::copy(
                trial
                    .join("installation/packages")
                    .join(&artifact.file_name),
                &package,
            )
            .unwrap();
            verify_package(&mut File::open(&package).unwrap(), artifact).unwrap();
            let mut controller = controller();
            controller.release = Some(release);
            controller.state = State::Ready {
                package,
                version: Version::new(1, 0, 1),
            };
            controller.staging_dir = Some(staging);
            controller
        };
        // A cancelled handoff must reap the guardian before removing its inputs.
        let mut cancelled_controller = make_ready();
        let cancelled_download = cancelled_controller.take_ready().unwrap();
        let cancelled_directory = cancelled_download.directory.path().to_owned();
        assert!(matches!(
            WindowsHandoff::prepare(
                cancelled_download,
                &guardian,
                &helper,
                &root,
                Duration::from_secs(15),
                &AtomicBool::new(true)
            ),
            Err(Error::Cancelled)
        ));
        assert!(!cancelled_directory.exists());
        assert_eq!(
            std::fs::read_to_string(root.join("current/sq.version")).unwrap(),
            old_manifest
        );

        let mut cancelled_ready_controller = make_ready();
        let cancelled_ready_download = cancelled_ready_controller.take_ready().unwrap();
        let cancelled_ready_directory = cancelled_ready_download.directory.path().to_owned();
        let cancelled_ready = WindowsHandoff::prepare(
            cancelled_ready_download,
            &guardian,
            &helper,
            &root,
            Duration::from_secs(15),
            &AtomicBool::new(false),
        )
        .unwrap();
        drop(cancelled_ready);
        assert!(!cancelled_ready_directory.exists());
        assert_eq!(
            std::fs::read_to_string(root.join("current/sq.version")).unwrap(),
            old_manifest
        );

        // Abrupt client death after Ready must not imply consent to install.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "update::controller::tests::uncommitted_handoff_client_fixture",
                "--nocapture",
            ])
            .env("NOH_UPDATE_HANDOFF_CASE", &case)
            .status()
            .unwrap();
        assert!(status.success());
        let info: serde_json::Value =
            serde_json::from_slice(&std::fs::read(case.join("uncommitted-client.json")).unwrap())
                .unwrap();
        let transaction = PathBuf::from(info["transaction"].as_str().unwrap());
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, WaitForSingleObject,
            },
        };
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                info["pid"].as_u64().unwrap() as u32,
            )
        };
        assert!(!process.is_null());
        assert_eq!(
            unsafe { WaitForSingleObject(process, 35000) },
            WAIT_OBJECT_0
        );
        let mut exit_code = 0;
        assert_ne!(unsafe { GetExitCodeProcess(process, &mut exit_code) }, 0);
        unsafe { CloseHandle(process) };
        assert_ne!(exit_code, 0);
        assert!(transaction.join("completed").exists());
        assert!(!transaction.join("commit").exists());
        assert_eq!(
            std::fs::metadata(transaction.join("helper.log"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            std::fs::read_to_string(root.join("current/sq.version")).unwrap(),
            old_manifest
        );

        let client_lease = RuntimeLease::acquire(&root).unwrap();
        let mut controller = make_ready();
        let download = controller.take_ready().unwrap();
        assert_eq!(controller.state(), &State::Idle);
        assert!(matches!(controller.take_ready(), Err(Error::Target)));
        let directory = download.directory.path().to_owned();
        drop(controller);
        assert!(directory.exists());
        let handoff = WindowsHandoff::prepare(
            download,
            &guardian,
            &helper,
            &root,
            Duration::from_secs(15),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(handoff.transaction().join("helper.log"))
                .unwrap()
                .len(),
            0
        );
        let mut detached = handoff.release_for_shutdown().unwrap();
        assert!(detached.directory().exists());
        assert_eq!(
            std::fs::read_to_string(root.join("current/sq.version")).unwrap(),
            old_manifest
        );
        drop(client_lease);
        assert_eq!(
            detached.wait(Duration::from_secs(45)).unwrap(),
            Version::new(1, 0, 1)
        );
        let report_path = detached.transaction().join("result.json");
        let original_report = std::fs::read(&report_path).unwrap();
        let mut wrong_report: serde_json::Value = serde_json::from_slice(&original_report).unwrap();
        wrong_report["version"] = serde_json::json!("1.0.2");
        std::fs::write(&report_path, serde_json::to_vec(&wrong_report).unwrap()).unwrap();
        assert!(matches!(
            detached.wait(Duration::ZERO),
            Err(Error::Integrity)
        ));
        std::fs::write(&report_path, original_report).unwrap();
        drop(detached);
        assert!(
            directory.exists(),
            "Detached ownership must survive client/controller teardown"
        );
        std::fs::write(case.join("evidence.json"), br#"{"controller_drop_preserved_inputs":true,"cancelled_guardian_reaped_before_cleanup":true,"ready_abandonment_cleaned_without_apply":true,"client_death_without_commit_did_not_apply":true,"unexpected_result_version_refused":true,"persistent_transfer_before_lease_release":true,"authenticated_update":"1.0.1","fixture_only":true}"#).unwrap();
        println!("Controller handoff evidence: {}", case.display());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Invoked as an abruptly exiting client by the handoff parent test"]
    fn uncommitted_handoff_client_fixture() {
        use crate::update::windows_handoff::WindowsHandoff;
        let case = PathBuf::from(std::env::var_os("NOH_UPDATE_HANDOFF_CASE").unwrap());
        let trial = PathBuf::from(std::env::var_os("NOH_UPDATE_HANDOFF_TRIAL").unwrap());
        let directory = attempt_directory(&case).unwrap();
        let name = "NohUpdateBoundaryProbe-1.0.1-full.nupkg";
        let package = directory.path().join(name);
        std::fs::copy(trial.join("installation/packages").join(name), &package).unwrap();
        let download = PreparedDownload {
            directory,
            package,
            envelope: std::fs::read(trial.join("envelope.json")).unwrap(),
            channel: Channel::Stable,
            version: Version::new(1, 0, 1),
        };
        let handoff = WindowsHandoff::prepare(
            download,
            &PathBuf::from(std::env::var_os("NOH_UPDATE_GUARDIAN").unwrap()),
            &PathBuf::from(std::env::var_os("NOH_UPDATE_HELPER").unwrap()),
            &case.join("installation"),
            Duration::from_secs(15),
            &AtomicBool::new(false),
        )
        .unwrap();
        let info =
            serde_json::json!({"pid":handoff.child_id(),"transaction":handoff.transaction()});
        std::fs::write(
            case.join("uncommitted-client.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .unwrap();
        std::process::exit(0); // Bypass destructors; guardian must still refuse to apply.
    }
}
