//! Experimental ownership transfer. Ordinary installation remains disabled.
use super::{
    controller::PreparedDownload,
    windows::{ProtectedFile, RuntimeLease},
    *,
};
use std::{
    process::{Child, Command, Stdio},
    time::Instant,
};

/// Holds an additional activity lease until explicit shutdown handoff.
/// Drop cancels the waiting guardian; it never silently authorizes installation.
pub struct WindowsHandoff {
    child: Option<Child>,
    lease: Option<RuntimeLease>,
    directory: Option<tempfile::TempDir>,
    transaction: PathBuf,
    expected_version: Version,
}
impl WindowsHandoff {
    /// Desktop path: trust the installed guardian only through the independently
    /// authenticated exact previous full package. No runtime guardian picker.
    pub fn prepare_installed(
        download: PreparedDownload,
        previous_archive: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        if !trust::setup_enabled() && option_env!("NOH_UPDATE_QUALIFICATION_ROOT").is_none() {
            return Err(Error::InstallationUnavailable);
        }
        let incoming = VerifiedRelease::verify(
            &download.envelope,
            &trust::compiled_keys()?,
            trust::package_id(),
        )?;
        if incoming.is_setup() {
            return Self::prepare_setup(download, previous_archive, timeout, cancelled);
        }
        let executable = std::fs::canonicalize(std::env::current_exe()?)?;
        let current = executable.parent().ok_or(Error::Metadata)?;
        if current.file_name() != Some(std::ffi::OsStr::new("current")) {
            return Err(Error::InstallationUnavailable);
        }
        let root = current.parent().ok_or(Error::Metadata)?;
        let allowed = std::fs::canonicalize(
            option_env!("NOH_UPDATE_QUALIFICATION_ROOT").ok_or(Error::InstallationUnavailable)?,
        )?;
        let previous_archive = std::fs::canonicalize(previous_archive)?;
        let staging = std::fs::canonicalize(download.directory.path())?;
        if root == allowed
            || !root.starts_with(&allowed)
            || !previous_archive.starts_with(&allowed)
            || previous_archive.starts_with(root)
            || root.starts_with(&previous_archive)
            || previous_archive.starts_with(&staging)
            || staging.starts_with(&previous_archive)
        {
            return Err(Error::InstallationUnavailable);
        }
        let lease = RuntimeLease::acquire(root)?;
        let mut previous = super::retention::open_cancellable(
            &previous_archive,
            &trust::compiled_keys()?,
            trust::package_id(),
            Target::native()?,
            download.channel,
            &trust::compiled_version()?,
            cancelled,
        )?;
        let mut installed = super::windows_guard::protect_installed_package_cancellable(
            previous.file(),
            current,
            trust::package_id(),
            cancelled,
        )?;
        let guardian = download.directory.path().join("noh-update-guard.exe");
        let helper = download.directory.path().join("Update.exe");
        let _guardian = copy_installed_guardian(&mut installed, &guardian, cancelled)?;
        let _helper = copy_protected(&root.join("Update.exe"), &helper, cancelled)?;
        let archive = staging.parent().ok_or(Error::Metadata)?.join("retained");
        std::fs::create_dir_all(&archive)?;
        let archive = std::fs::canonicalize(archive)?;
        if !archive.starts_with(&allowed)
            || archive.starts_with(root)
            || root.starts_with(&archive)
            || archive.starts_with(&staging)
            || staging.starts_with(&archive)
        {
            return Err(Error::InstallationUnavailable);
        }
        let keys = trust::compiled_keys()?;
        let incoming = VerifiedRelease::verify(&download.envelope, &keys, trust::package_id())?;
        let artifact = incoming
            .select(
                Target::native()?,
                download.channel,
                &trust::compiled_version()?,
            )?
            .ok_or(Error::Target)?;
        let mut package = ProtectedFile::open(&download.package)?;
        verify_package_cancellable(package.file(), artifact, cancelled)?;
        let unpacked = application_size(package.file())?;
        // Preflight is a conservative estimate, not a reservation or guarantee:
        // two full-package copies and two application trees, plus actual tools.
        let required = space_requirement(
            artifact.size,
            unpacked,
            std::fs::metadata(&guardian)?.len(),
            std::fs::metadata(&helper)?.len(),
            download.envelope.len() as u64,
        )?;
        let available = super::windows::available_disk_bytes(&staging)?;
        if available < required {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Insufficient space for full update and retained recovery inputs",
            )));
        }
        let mut space_plan = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(staging.join("space-plan.json"))?;
        space_plan.write_all(&serde_json::to_vec(&serde_json::json!({"required_bytes":required,"available_bytes":available,"package_bytes":artifact.size,"application_bytes":unpacked,"reservation":false,"policy":"two packages and two application trees plus tools/envelope"})).map_err(|_| Error::Metadata)?)?;
        space_plan.sync_all()?;
        drop(space_plan);
        super::retention::retain_or_reuse(
            &archive,
            &download.package,
            &download.envelope,
            &keys,
            trust::package_id(),
            Target::native()?,
            download.channel,
            &download.version,
            cancelled,
        )?;
        let handoff =
            Self::prepare_inner(download, &guardian, &helper, root, timeout, cancelled, true)?;
        drop(lease);
        Ok(handoff)
    }
    fn prepare_setup(
        download: PreparedDownload,
        previous_archive: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        use super::{windows_anchor::InstallationAnchor, windows_setup};
        let keys = trust::compiled_keys()?;
        let anchor =
            InstallationAnchor::for_executable(&std::env::current_exe()?, trust::package_id())?
                .ok_or(Error::InstallationUnavailable)?;
        trust::validate_setup_base(anchor.base())?;
        let staging = std::fs::canonicalize(download.directory.path())?;
        if !staging.starts_with(anchor.coordination().join("downloads"))
            || std::fs::canonicalize(&download.package)?.parent() != Some(staging.as_path())
        {
            return Err(Error::InstallationUnavailable);
        }
        let lease = RuntimeLease::acquire(&anchor.coordination())?;
        let previous = super::retention::SETUP
            .for_profile(anchor.profile())
            .open_cancellable(
                previous_archive,
                &keys,
                trust::package_id(),
                Target::native()?,
                download.channel,
                &trust::compiled_version()?,
                cancelled,
            )?;
        let contract = windows_setup::exact_setup(
            previous.release(),
            download.channel,
            &trust::compiled_version()?,
        )?
        .setup
        .as_ref()
        .ok_or(Error::Metadata)?;
        if contract.build_fingerprint != crate::build_info::current().build_fingerprint {
            return Err(Error::Integrity);
        }
        let (guardian, _guardian) = windows_setup::open_recovery_tool(
            &anchor,
            &keys,
            trust::package_id(),
            &trust::compiled_version()?,
            "noh-update-guard.exe",
            cancelled,
        )?;
        let incoming = VerifiedRelease::verify(&download.envelope, &keys, trust::package_id())?;
        incoming.require_profile(anchor.profile())?;
        let artifact = incoming
            .select_setup(
                Target::native()?,
                download.channel,
                &trust::compiled_version()?,
            )?
            .ok_or(Error::Target)?;
        let mut package = ProtectedFile::open(&download.package)?;
        verify_package_cancellable(package.file(), artifact, cancelled)?;
        Self::start_guardian(
            download,
            &guardian,
            None,
            &anchor.installation(),
            Some(anchor.base()),
            lease,
            timeout,
            cancelled,
            true,
        )
    }
    /// Run on a worker. The independently trusted guardian must live outside root.
    /// This remains restricted to the compile-time disposable qualification root.
    pub fn prepare(
        download: PreparedDownload,
        guardian: &Path,
        helper: &Path,
        root: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        Self::prepare_inner(download, guardian, helper, root, timeout, cancelled, false)
    }
    fn prepare_inner(
        download: PreparedDownload,
        guardian: &Path,
        helper: &Path,
        root: &Path,
        timeout: Duration,
        cancelled: &AtomicBool,
        restart_gui: bool,
    ) -> Result<Self> {
        let allowed = std::fs::canonicalize(
            option_env!("NOH_UPDATE_QUALIFICATION_ROOT").ok_or(Error::InstallationUnavailable)?,
        )?;
        let root = std::fs::canonicalize(root)?;
        let guardian = std::fs::canonicalize(guardian)?;
        let staging = std::fs::canonicalize(download.directory.path())?;
        let package = std::fs::canonicalize(&download.package)?;
        if root == allowed
            || !root.starts_with(&allowed)
            || !staging.starts_with(&allowed)
            || staging.starts_with(&root)
            || root.starts_with(&staging)
            || guardian.starts_with(&root)
            || package.parent() != Some(staging.as_path())
        {
            return Err(Error::InstallationUnavailable);
        }
        let _guardian_protection = ProtectedFile::open(&guardian)?;
        let lease = RuntimeLease::acquire(&root)?;
        Self::start_guardian(
            download,
            &guardian,
            Some(helper),
            &root,
            None,
            lease,
            timeout,
            cancelled,
            restart_gui,
        )
    }

    fn start_guardian(
        download: PreparedDownload,
        guardian: &Path,
        helper: Option<&Path>,
        root: &Path,
        setup_base: Option<&Path>,
        lease: RuntimeLease,
        timeout: Duration,
        cancelled: &AtomicBool,
        restart_gui: bool,
    ) -> Result<Self> {
        let staging = std::fs::canonicalize(download.directory.path())?;
        let package = std::fs::canonicalize(&download.package)?;
        let envelope_path = staging.join("handoff-envelope.json");
        let mut envelope = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&envelope_path)?;
        envelope.write_all(&download.envelope)?;
        envelope.sync_all()?;
        drop(envelope);
        let transaction = staging.join("transaction");
        let stderr = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.join("guardian-stderr.log"))?;
        let mut command = guardian_command(&guardian, &staging);
        command
            .args(["--root"])
            .arg(&root)
            .arg("--package")
            .arg(&package)
            .arg("--envelope")
            .arg(&envelope_path)
            .arg("--transaction")
            .arg(&transaction)
            .arg("--channel")
            .arg(match download.channel {
                Channel::Stable => "stable",
                Channel::Beta => "beta",
            })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        if let Some(helper) = helper {
            command.arg("--helper").arg(helper);
        }
        if let Some(base) = setup_base {
            command
                .arg("--setup-base")
                .arg(base)
                .arg("--from-version")
                .arg(trust::compiled_version()?.to_string());
        }
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        if restart_gui {
            command.arg("--restart-gui");
        }
        let child = command.spawn()?;
        let mut handoff = Self {
            child: Some(child),
            lease: Some(lease),
            directory: Some(download.directory),
            transaction,
            expected_version: download.version,
        };
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if handoff.child.as_mut().unwrap().try_wait()?.is_some() {
                return Err(Error::Io(std::io::Error::other(
                    "Guardian exited before responsibility transfer",
                )));
            }
            match File::open(handoff.transaction.join("ready")) {
                Ok(file) => {
                    let mut marker = Vec::new();
                    file.take(64).read_to_end(&mut marker)?;
                    if marker == b"authenticated-and-protected" {
                        // Ready is sufficient for ownership transfer, never success.
                        return Ok(handoff);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= deadline {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Guardian readiness timed out",
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    pub fn transaction(&self) -> &Path {
        &self.transaction
    }
    #[cfg(test)]
    pub(crate) fn child_id(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    /// Persist BEFORE dropping our activity lease and detaching the guardian.
    /// The application's own managed lease must still span normal GUI shutdown.
    /// The returned directory is untrusted on restart until independently checked.
    pub fn release_for_shutdown(mut self) -> Result<DetachedHandoff> {
        if self.child.as_mut().unwrap().try_wait()?.is_some() {
            return Err(Error::Io(std::io::Error::other(
                "Guardian exited before shutdown",
            )));
        }
        let directory = self.directory.take().unwrap().keep();
        let temporary_commit = self.transaction.join("commit.tmp");
        let mut commit = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_commit)?;
        commit.write_all(b"authorize-shutdown")?;
        commit.sync_all()?;
        drop(commit);
        std::fs::rename(temporary_commit, self.transaction.join("commit"))?;
        let child = self.child.take().unwrap();
        self.lease.take();
        Ok(DetachedHandoff {
            child,
            directory,
            transaction: self.transaction.clone(),
            expected_version: self.expected_version.clone(),
        })
    }
}

fn guardian_command(executable: &Path, staging: &Path) -> Command {
    // A GUI normally runs with current/ as its working directory. Inheriting
    // that directory would prevent its replacement despite an external image.
    let mut command = Command::new(executable);
    command.current_dir(staging);
    command
}

fn space_requirement(
    package: u64,
    application: u64,
    guardian: u64,
    helper: u64,
    envelope: u64,
) -> Result<u64> {
    package
        .checked_mul(2)
        .and_then(|size| size.checked_add(application.checked_mul(2)?))
        .and_then(|size| size.checked_add(guardian))
        .and_then(|size| size.checked_add(helper))
        .and_then(|size| size.checked_add(envelope))
        .ok_or(Error::Metadata)
}

fn application_size(package: &mut File) -> Result<u64> {
    package.seek(SeekFrom::Start(0))?;
    let mut archive = zip::ZipArchive::new(package).map_err(|_| Error::Metadata)?;
    if archive.len() > 4096 {
        return Err(Error::Metadata);
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| Error::Metadata)?;
        if entry.name().starts_with("lib/app/") && !entry.is_dir() {
            total = total.checked_add(entry.size()).ok_or(Error::Metadata)?;
        }
        if total > 4 * 1024 * 1024 * 1024 {
            return Err(Error::Metadata);
        }
    }
    if total == 0 {
        return Err(Error::Metadata);
    }
    Ok(total)
}

fn copy_protected(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<ProtectedFile> {
    let mut input = ProtectedFile::open(source)?;
    copy_authenticated(input.file(), destination, cancelled)
}
fn copy_installed_guardian(
    installed: &mut [(PathBuf, ProtectedFile)],
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<ProtectedFile> {
    let source = installed
        .iter_mut()
        .find(|(relative, _)| relative == Path::new("bin/noh-update-guard.exe"))
        .ok_or(Error::Integrity)?;
    copy_authenticated(source.1.file(), destination, cancelled)
}
fn copy_authenticated(
    input: &mut File,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<ProtectedFile> {
    input.seek(SeekFrom::Start(0))?;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut buffer = [0u8; 32 * 1024];
    let mut expected = Sha256::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        expected.update(&buffer[..count]);
    }
    output.sync_all()?;
    drop(output);
    let mut protection = ProtectedFile::open(destination)?;
    let mut actual = Sha256::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = protection.file().read(&mut buffer)?;
        if count == 0 {
            break;
        }
        actual.update(&buffer[..count]);
    }
    if expected.finalize() != actual.finalize() {
        return Err(Error::Integrity);
    }
    Ok(protection)
}
impl Drop for WindowsHandoff {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Our lease prevents helper launch while cancellation stops the guardian.
            // On failed termination retain the directory rather than deleting inputs.
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            let stopped = loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Err(_) => break false,
                    Ok(None) if Instant::now() >= deadline => break false,
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                }
            };
            if !stopped {
                if let Some(directory) = self.directory.take() {
                    let _ = directory.keep();
                }
            }
        }
        self.lease.take();
        // TempDir cleanup happens only after guardian termination and lock release.
    }
}

/// A surviving guardian and persistent inputs. Dropping this closes the process
/// handle without killing the guardian or removing its staging directory.
pub struct DetachedHandoff {
    child: Child,
    directory: PathBuf,
    transaction: PathBuf,
    expected_version: Version,
}
impl DetachedHandoff {
    pub fn transaction(&self) -> &Path {
        &self.transaction
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    /// Observe an actual process handle, not a stale ready marker.
    pub fn wait(&mut self, timeout: Duration) -> Result<Version> {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Guardian result remains unknown",
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let mut completed = Vec::new();
        File::open(self.transaction.join("completed"))?
            .take(64)
            .read_to_end(&mut completed)?;
        if completed != b"result-flushed" {
            return Err(Error::Metadata);
        }
        let mut bytes = Vec::new();
        File::open(self.transaction.join("result.json"))?
            .take(16385)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16384 {
            return Err(Error::Metadata);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
        if !status.success() || value.get("success").and_then(|v| v.as_bool()) != Some(true) {
            return Err(Error::Io(std::io::Error::other(
                "Guardian reported update failure",
            )));
        }
        let version = Version::parse(
            value
                .get("version")
                .and_then(|v| v.as_str())
                .ok_or(Error::Metadata)?,
        )
        .map_err(|_| Error::Metadata)?;
        if version != self.expected_version {
            return Err(Error::Integrity);
        }
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guardian_launch_uses_external_cwd_and_does_not_pin_current() {
        use std::os::windows::process::CommandExt;
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let staging = root.path().join("external-stage");
        let backup = root.path().join("backup");
        std::fs::create_dir(&current).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(current.join("app.txt"), b"intact").unwrap();
        let executable = std::env::current_exe().unwrap();
        let start = |directory: &Path| {
            let mut command = guardian_command(&executable, directory);
            assert_eq!(command.get_current_dir(), Some(directory));
            command
                .args([
                    "--ignored",
                    "--exact",
                    "update::windows_handoff::tests::guardian_cwd_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("NOH_GUARDIAN_CWD_CHILD", directory)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x08000000);
            let mut child = OwnedChild(command.spawn().unwrap());
            let stdout = child.0.stdout.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                use std::io::BufRead;
                let mut reader = std::io::BufReader::new(stdout);
                let mut line = Vec::new();
                let found = loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break false,
                        Ok(_)
                            if line
                                .windows(b"NOH-CWD-READY".len())
                                .any(|value| value == b"NOH-CWD-READY") =>
                        {
                            break true;
                        }
                        Ok(_) => {}
                    }
                };
                let _ = tx.send(found);
                reader.into_inner()
            });
            assert!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                "Native child must confirm its actual CWD before rename"
            );
            child.0.stdout = Some(reader.join().unwrap());
            child
        };
        let mut pinned = start(&current);
        assert!(pinned.0.try_wait().unwrap().is_none());
        assert_eq!(
            std::fs::rename(&current, &backup)
                .unwrap_err()
                .raw_os_error(),
            Some(32)
        );
        drop(pinned);
        let mut external = start(&staging);
        assert!(external.0.try_wait().unwrap().is_none());
        std::fs::rename(&current, &backup).unwrap();
        assert_eq!(std::fs::read(backup.join("app.txt")).unwrap(), b"intact");
        drop(external);
    }
    #[test]
    #[ignore = "Native child entry point, launched by the CWD regression"]
    fn guardian_cwd_child() {
        let Some(expected) = std::env::var_os("NOH_GUARDIAN_CWD_CHILD") else {
            return;
        };
        assert_eq!(
            std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap(),
            std::fs::canonicalize(expected).unwrap()
        );
        println!("NOH-CWD-READY");
        std::io::stdout().flush().unwrap();
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().read(&mut byte);
    }
    #[test]
    fn full_space_preflight_counts_signed_application_and_rejects_overflow() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package.zip");
        let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
        writer
            .start_file("lib/app/noh.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"application").unwrap();
        writer
            .start_file("metadata", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not extracted into current").unwrap();
        writer.finish().unwrap();
        assert_eq!(
            application_size(&mut File::open(&package).unwrap()).unwrap(),
            11
        );
        // Multi-gigabyte package and application sizes: preflight includes retention
        // and a second application tree as well as the installation delta.
        assert_eq!(
            space_requirement(1_266_107_201, 1_987_172_278, 10, 20, 30).unwrap(),
            6_506_559_018
        );
        assert!(matches!(
            space_requirement(u64::MAX, 1, 0, 0, 0),
            Err(Error::Metadata)
        ));
        assert!(matches!(
            space_requirement(1, u64::MAX, 0, 0, 0),
            Err(Error::Metadata)
        ));
        assert!(super::super::windows::available_disk_bytes(root.path()).unwrap() > 0);
        let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
        writer
            .start_file("metadata", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"no application").unwrap();
        writer.finish().unwrap();
        assert!(matches!(
            application_size(&mut File::open(&package).unwrap()),
            Err(Error::Metadata)
        ));
    }
    #[test]
    fn installed_guardian_must_be_a_protected_signed_package_member() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir_all(current.join("bin")).unwrap();
        std::fs::write(current.join("noh.exe"), b"gui").unwrap();
        std::fs::write(current.join("bin/noh-update-guard.exe"), b"guardian").unwrap();
        let package = root.path().join("package.zip");
        let cancel = AtomicBool::new(false);
        for included in [false, true] {
            let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
            writer
                .start_file("lib/app/noh.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"gui").unwrap();
            if included {
                writer
                    .start_file(
                        "lib/app/bin/noh-update-guard.exe",
                        zip::write::SimpleFileOptions::default(),
                    )
                    .unwrap();
                writer.write_all(b"guardian").unwrap();
            }
            writer.finish().unwrap();
            let mut installed = super::super::windows_guard::protect_installed_package_cancellable(
                &mut File::open(&package).unwrap(),
                &current,
                "NOH",
                &cancel,
            )
            .unwrap();
            let copied = root.path().join("external-guardian.exe");
            let result = copy_installed_guardian(&mut installed, &copied, &cancel);
            if !included {
                assert!(matches!(result, Err(Error::Integrity)));
                assert!(
                    !copied.exists(),
                    "an extra unsigned installed guardian is never copied"
                );
            } else {
                let protection = result.unwrap();
                assert_eq!(std::fs::read(&copied).unwrap(), b"guardian");
                assert!(std::fs::write(&copied, b"altered").is_err());
                cancel.store(true, Ordering::Relaxed);
                assert!(matches!(
                    copy_installed_guardian(
                        &mut installed,
                        &root.path().join("cancelled"),
                        &cancel
                    ),
                    Err(Error::Cancelled)
                ));
                drop(protection);
            }
        }
    }
}
