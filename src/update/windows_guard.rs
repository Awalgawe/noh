//! Guarded helper handoff, restricted to disposable qualification for now.
use super::{
    windows::{ProtectedFile, SupervisedProcess},
    *,
};
use std::ffi::OsStr;
use windows_sys::Win32::{Security as security, System::Threading as threading};

const HELPER_SHA256: &str = "acefb4a2cb46cc77ed3c2364db17f8bd2a25e2197cfeae56cd85e88a7ac21ac5";

#[derive(Serialize)]
pub struct ApplyOutcome {
    pub version: Version,
    pub helper_elapsed_ms: u128,
    pub helper_job_peak_commit_bytes: Option<u64>,
}

pub struct GuardedUpdate {
    package: ProtectedFile,
    helper: ProtectedFile,
    root_marker: ProtectedFile,
    package_path: PathBuf,
    helper_path: PathBuf,
    root: PathBuf,
    release: VerifiedRelease,
    package_id: String,
}
impl GuardedUpdate {
    /// Experimental primitive: caller must restrict use to an isolated qualification
    /// installation; delivered Controller::apply remains disabled.
    pub fn prepare(
        envelope: &[u8],
        keys: &[TrustKey],
        package_id: &str,
        channel: Channel,
        package_path: &Path,
        helper_path: &Path,
        installation: &Path,
    ) -> Result<Self> {
        if is_elevated()? {
            return Err(Error::InstallationUnavailable);
        }
        let root_marker = ProtectedFile::open(&installation.join(".portable"))?;
        let root = std::fs::canonicalize(installation)?;
        let executable = std::fs::canonicalize(std::env::current_exe()?)?;
        let helper_path = std::fs::canonicalize(helper_path)?;
        if under_root(&executable, &root) || under_root(&helper_path, &root) {
            return Err(Error::InstallationUnavailable);
        }
        let mut helper = ProtectedFile::open(&helper_path)?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 32768];
        loop {
            let count = helper.file().read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != HELPER_SHA256 {
            return Err(Error::Integrity);
        }
        let release = VerifiedRelease::verify(envelope, keys, package_id)?;
        let current = installed_version(&root, package_id)?;
        let target = Target::native()?;
        if target
            != (Target {
                os: Os::Windows,
                arch: Architecture::X64,
            })
        {
            return Err(Error::Target);
        }
        let artifact = release
            .select(target, channel, &current)?
            .ok_or(Error::Target)?;
        let mut package = ProtectedFile::open(package_path)?;
        let package_path = std::fs::canonicalize(package_path)?;
        if package_path.file_name() != Some(OsStr::new(&artifact.file_name)) {
            return Err(Error::Metadata);
        }
        verify_package(package.file(), artifact)?;
        validate_package_manifest(
            package.file(),
            package_id,
            &release.release().version,
            channel,
        )?;
        // User-writable root required before the official helper's elevation fallback.
        let writable = tempfile::NamedTempFile::new_in(&root)?;
        drop(writable);
        Ok(Self {
            package,
            helper,
            root_marker,
            package_path,
            helper_path,
            root,
            release,
            package_id: package_id.into(),
        })
    }
    pub fn apply(
        self,
        activity_timeout: Duration,
        helper_timeout: Duration,
        log: &Path,
    ) -> Result<Version> {
        self.apply_measured(activity_timeout, helper_timeout, log)
            .map(|outcome| outcome.version)
    }
    pub fn apply_measured(
        mut self,
        activity_timeout: Duration,
        helper_timeout: Duration,
        log: &Path,
    ) -> Result<ApplyOutcome> {
        self.apply_measured_inner(activity_timeout, helper_timeout, log)
    }

    /// Installation and relaunch are separate outcomes. A launch failure must
    /// never describe an already replaced installation as unchanged.
    pub fn apply_with_restart(
        mut self,
        activity_timeout: Duration,
        helper_timeout: Duration,
        log: &Path,
    ) -> Result<(ApplyOutcome, Result<u32>)> {
        let outcome = self.apply_measured_inner(activity_timeout, helper_timeout, log)?;
        let restart = self.restart_installed()?;
        Ok((outcome, restart))
    }

    fn restart_installed(&mut self) -> Result<Result<u32>> {
        let lease = super::windows::RuntimeLease::acquire(&self.root)?.into_protection();
        if installed_version(&self.root, &self.package_id)? != self.release.release().version {
            return Err(Error::Integrity);
        }
        let current = self.root.join("current");
        // Verify every installed application file directly against the signed,
        // protected full package, not an unsigned manifest from the installation.
        let inputs = protect_installed_package(self.package.file(), &current, &self.package_id)?;
        let mut protections: Vec<_> = inputs.iter().map(|(_, input)| input).collect();
        protections.push(&lease);
        let child = SupervisedProcess::spawn_protected(
            &current.join("noh.exe"),
            &[],
            &current,
            &protections,
        )
        .map_err(Error::from);
        // A PID establishes launch only; readiness is not inferred from spawn.
        Ok(child.and_then(|child| child.detach().map_err(Error::from)))
    }

    fn apply_measured_inner(
        &mut self,
        activity_timeout: Duration,
        helper_timeout: Duration,
        log: &Path,
    ) -> Result<ApplyOutcome> {
        // Stable lock outside current/. Requires every installed NOH version to
        // participate. Direct third-party runtime executions are not covered.
        let start = std::time::Instant::now();
        let lease = loop {
            match super::windows::ExclusiveLease::acquire(&self.root) {
                Ok(lease) => break lease.into_protection(),
                Err(error)
                    if error.raw_os_error() == Some(32) && start.elapsed() < activity_timeout =>
                {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(error) => {
                    return Err(Error::Io(std::io::Error::new(
                        error.kind(),
                        format!("Cannot acquire exclusive update lease: {error}"),
                    )));
                }
            }
        };
        // Recheck installed identity/version after clients are gone, before apply.
        let current =
            installed_version(&self.root, &self.package_id).map_err(|error| match error {
                Error::Io(error) => Error::Io(std::io::Error::new(
                    error.kind(),
                    format!("Cannot reread installed identity: {error}"),
                )),
                other => other,
            })?;
        self.release
            .select(Target::native()?, self.release.release().channel, &current)?
            .ok_or(Error::Target)?;
        wait_for_installed_process_exit(&self.root, start, activity_timeout).map_err(|error| {
            Error::Io(std::io::Error::other(format!(
                "Cannot establish installed process inactivity: {error}"
            )))
        })?;
        let root = self.root.as_os_str();
        let packages = self
            .package_path
            .parent()
            .ok_or(Error::Metadata)?
            .as_os_str();
        let args: Vec<&OsStr> = vec![
            OsStr::new("--silent"),
            OsStr::new("--rootDir"),
            root,
            OsStr::new("--packageDir"),
            packages,
            OsStr::new("--log"),
            log.as_os_str(),
            OsStr::new("apply"),
            OsStr::new("--norestart"),
            OsStr::new("--package"),
            self.package_path.as_os_str(),
        ];
        let helper_start = std::time::Instant::now();
        let consumer = SupervisedProcess::spawn_protected(
            &self.helper_path,
            &args,
            self.helper_path.parent().ok_or(Error::Metadata)?,
            &[&self.package, &self.helper, &self.root_marker, &lease],
        )
        .map_err(|error| {
            Error::Io(std::io::Error::new(
                error.kind(),
                format!("Cannot launch protected update helper: {error}"),
            ))
        })?;
        let exit = consumer.wait(helper_timeout).map_err(|error| {
            Error::Io(std::io::Error::new(
                error.kind(),
                format!("Cannot observe update helper exit: {error}"),
            ))
        })?;
        // Explicitly inspect tree termination, even if the helper leader succeeded.
        consumer
            .terminate_tree(Duration::from_secs(5))
            .map_err(|error| {
                Error::Io(std::io::Error::new(
                    error.kind(),
                    format!("Cannot stop update helper process tree: {error}"),
                ))
            })?;
        let peak = consumer.peak_committed_bytes().ok();
        let helper_elapsed_ms = helper_start.elapsed().as_millis();
        drop(consumer);
        if exit != Some(0) {
            return Err(Error::Io(std::io::Error::other(
                "Update helper failed or timed out; recovery required",
            )));
        }
        let actual = installed_version(&self.root, &self.package_id)?;
        if actual != self.release.release().version {
            return Err(Error::Integrity);
        }
        drop(lease);
        Ok(ApplyOutcome {
            version: actual,
            helper_elapsed_ms,
            helper_job_peak_commit_bytes: peak,
        })
    }
}

fn protect_installed_package(
    package: &mut File,
    current: &Path,
    package_id: &str,
) -> Result<Vec<(PathBuf, ProtectedFile)>> {
    protect_installed_package_cancellable(package, current, package_id, &AtomicBool::new(false))
}
/// Pinned Velopack 1.2.161 installs engine members outside current, renaming
/// the execution stub. Verify those destinations rather than ignoring members.
fn validate_engine_member_path(name: &str, package_id: &str) -> Result<()> {
    // The two pinned patterns each contain exactly one dot wildcard. Match
    // their complete-path suffix semantics without a new runtime dependency.
    let matches_reserved_pattern = |before: &str, after: &str| {
        name.strip_suffix(after).is_some_and(|prefix| {
            prefix
                .char_indices()
                .next_back()
                .is_some_and(|(last, character)| {
                    character != '\n' && prefix[..last].ends_with(before)
                })
        })
    };
    let leaf = name.rsplit('/').next().unwrap_or_default();
    if leaf.ends_with("Squirrel.exe") && name != "lib/app/Squirrel.exe" {
        return Err(Error::Metadata);
    }
    // The pinned extractor's stub regex has an unescaped dot. Refuse its
    // noncanonical matches rather than treating omitted files as app members.
    // Match the whole archive path, as the pinned helper does: the wildcard
    // can also consume a directory separator, not just a filename character.
    if matches_reserved_pattern("_ExecutionStub", "exe")
        && name != format!("lib/app/{package_id}_ExecutionStub.exe")
    {
        return Err(Error::Metadata);
    }
    if matches_reserved_pattern("", "__symlink") {
        return Err(Error::Metadata);
    }
    Ok(())
}
pub(crate) fn installed_member_path(
    current: &Path,
    relative: &str,
    package_id: &str,
) -> Result<PathBuf> {
    validate_engine_member_path(&format!("lib/app/{relative}"), package_id)?;
    let member = Path::new(relative);
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' '])
        })
        || member
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(Error::Metadata);
    }
    let name = member
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::Metadata)?;
    let root = current.parent().ok_or(Error::Metadata)?;
    if name == "Squirrel.exe" {
        if member.components().count() != 1 {
            return Err(Error::Metadata);
        }
        Ok(root.join("Update.exe"))
    } else if let Some(stub) = name.strip_suffix("_ExecutionStub.exe") {
        // Our packaging contract omits PackTitle: the only supported stub base
        // is the independently expected package identity, not arbitrary aliases.
        if member.components().count() != 1 || stub != package_id {
            return Err(Error::Metadata);
        }
        Ok(root.join(format!("{stub}.exe")))
    } else {
        Ok(current.join(member))
    }
}
pub(crate) fn protect_installed_package_cancellable(
    package: &mut File,
    current: &Path,
    package_id: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<(PathBuf, ProtectedFile)>> {
    package.seek(SeekFrom::Start(0))?;
    let mut archive = zip::ZipArchive::new(package).map_err(|_| Error::Metadata)?;
    let mut protected = Vec::new();
    let mut destinations = std::collections::HashSet::new();
    let mut has_gui = false;
    for index in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut entry = archive.by_index(index).map_err(|_| Error::Metadata)?;
        validate_engine_member_path(entry.name(), package_id)?;
        let Some(relative) = entry.name().strip_prefix("lib/app/").map(str::to_owned) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let destination = installed_member_path(current, &relative, package_id)?;
        if !destinations.insert(destination.to_string_lossy().to_lowercase()) {
            return Err(Error::Metadata);
        }
        let mut input = ProtectedFile::open(&destination)?;
        if input.file().metadata()?.len() != entry.size() {
            return Err(Error::Integrity);
        }
        let mut expected = Sha256::new();
        let mut actual = Sha256::new();
        let mut buffer = [0u8; 32 * 1024];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            expected.update(&buffer[..count]);
        }
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let count = input.file().read(&mut buffer)?;
            if count == 0 {
                break;
            }
            actual.update(&buffer[..count]);
        }
        if expected.finalize() != actual.finalize() {
            return Err(Error::Integrity);
        }
        has_gui |= relative == "noh.exe";
        protected.push((PathBuf::from(relative), input));
    }
    if !has_gui {
        return Err(Error::Metadata);
    }
    Ok(protected)
}

fn under_root(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase();
    let root = root.to_string_lossy().to_lowercase();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    let root = root.strip_prefix(r"\\?\").unwrap_or(&root);
    path == root || path.starts_with(&(root.trim_end_matches('\\').to_string() + "\\"))
}

#[cfg(test)]
fn ensure_no_running_installed_processes(root: &Path) -> Result<()> {
    match running_installed_process(root)? {
        None => Ok(()),
        Some((pid, path)) => Err(Error::Io(std::io::Error::other(format!(
            "An installed application or runtime process is still active: pid={pid} path={}",
            path.display()
        )))),
    }
}

pub(crate) fn wait_for_installed_process_exit(
    root: &Path,
    start: std::time::Instant,
    timeout: Duration,
) -> Result<()> {
    wait_for_installed_process_exit_cancellable(root, start, timeout, &AtomicBool::new(false))
}

pub(crate) fn wait_for_installed_process_exit_cancellable(
    root: &Path,
    start: std::time::Instant,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        match running_installed_process(root)? {
            None => return Ok(()),
            Some((pid, path)) if start.elapsed() >= timeout => {
                return Err(Error::Io(std::io::Error::other(format!(
                    "Installed process exit deadline: pid={pid} path={}",
                    path.display()
                ))));
            }
            Some(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

pub(crate) fn running_installed_process(root: &Path) -> Result<Option<(u32, PathBuf)>> {
    use windows_sys::Win32::{Foundation as foundation, System::Diagnostics::ToolHelp as snapshot};
    let mut names = std::collections::HashSet::new();
    fn executables(
        folder: &Path,
        names: &mut std::collections::HashSet<String>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(std::io::Error::other(
                    "Symlink runtime layout is unsupported",
                ));
            }
            if kind.is_dir() {
                executables(&entry.path(), names)?;
            } else if entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            {
                names.insert(entry.file_name().to_string_lossy().to_lowercase());
            }
        }
        Ok(())
    }
    names.extend(["noh.exe", "noh-cli.exe", "noh-mcp.exe", "ffmpeg.exe"].map(str::to_owned));
    match executables(&root.join("current"), &mut names) {
        Ok(()) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && !root.join("current").exists() => {}
        Err(error) => return Err(error.into()),
    }
    let processes = unsafe { snapshot::CreateToolhelp32Snapshot(snapshot::TH32CS_SNAPPROCESS, 0) };
    if processes == foundation::INVALID_HANDLE_VALUE {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    let result = (|| {
        let mut entry: snapshot::PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut valid = unsafe { snapshot::Process32FirstW(processes, &mut entry) };
        while valid != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase();
            let process = unsafe {
                threading::OpenProcess(
                    threading::PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ProcessID,
                )
            };
            if !process.is_null() {
                let mut path = vec![0u16; 32768];
                let mut length = path.len() as u32;
                let read = unsafe {
                    threading::QueryFullProcessImageNameW(
                        process,
                        0,
                        path.as_mut_ptr(),
                        &mut length,
                    )
                };
                let query_error = (read == 0).then(std::io::Error::last_os_error);
                let exit_status = if read != 0 || names.contains(&name) {
                    let mut code = 259;
                    if unsafe { threading::GetExitCodeProcess(process, &mut code) } != 0 {
                        Some(Ok(code))
                    } else {
                        Some(Err(std::io::Error::last_os_error()))
                    }
                } else {
                    None
                };
                unsafe { foundation::CloseHandle(process) };
                if read != 0 {
                    let path = PathBuf::from(String::from_utf16_lossy(&path[..length as usize]));
                    if under_root(&path, root) {
                        if let Some(Err(error)) = exit_status {
                            return Err(Error::Io(std::io::Error::new(
                                error.kind(),
                                format!(
                                    "Cannot inspect installed process exit: pid={} path={}: {error}",
                                    entry.th32ProcessID,
                                    path.display()
                                ),
                            )));
                        }
                    }
                    if under_root(&path, root)
                        && !matches!(exit_status, Some(Ok(code)) if code != foundation::STILL_ACTIVE as u32)
                    {
                        // Keep the exclusive lease while a closing process still
                        // exists. Releasing its Rust lease precedes OS exit.
                        return Ok(Some((entry.th32ProcessID, path)));
                    }
                } else if names.contains(&name)
                    // A completed GUI can remain in the snapshot while another
                    // process retains its handle. Only this same handle's
                    // successful termination status authorizes ignoring it.
                    && !matches!(exit_status, Some(Ok(code)) if code != foundation::STILL_ACTIVE as u32)
                {
                    return Err(Error::Io(std::io::Error::other(format!(
                        "Cannot establish installed runtime inactivity: QueryFullProcessImageNameW pid={} name={name}: {}; GetExitCodeProcess={exit_status:?}",
                        entry.th32ProcessID,
                        query_error.unwrap()
                    ))));
                }
            } else if names.contains(&name) {
                let error = std::io::Error::last_os_error();
                return Err(Error::Io(std::io::Error::other(format!(
                    "Cannot inspect a possible installed runtime process: OpenProcess pid={} name={name}: {error}",
                    entry.th32ProcessID
                ))));
            }
            valid = unsafe { snapshot::Process32NextW(processes, &mut entry) };
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(foundation::ERROR_NO_MORE_FILES as i32) {
            return Err(Error::Io(error));
        }
        Ok(None)
    })();
    unsafe { foundation::CloseHandle(processes) };
    result
}
pub(crate) fn is_elevated() -> std::io::Result<bool> {
    let mut token = std::ptr::null_mut();
    if unsafe {
        threading::OpenProcessToken(
            threading::GetCurrentProcess(),
            security::TOKEN_QUERY,
            &mut token,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut elevation: security::TOKEN_ELEVATION = unsafe { std::mem::zeroed() };
    let mut returned = 0;
    let success = unsafe {
        security::GetTokenInformation(
            token,
            security::TokenElevation,
            &mut elevation as *mut _ as _,
            std::mem::size_of_val(&elevation) as u32,
            &mut returned,
        )
    };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    if success == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}
pub(crate) fn installed_version(root: &Path, expected_id: &str) -> Result<Version> {
    let mut manifest = ProtectedFile::open(&root.join("current/sq.version"))?;
    let mut text = String::new();
    manifest.file().take(16385).read_to_string(&mut text)?;
    if text.len() > 16384 {
        return Err(Error::Metadata);
    }
    let fields = manifest_fields(&text)?;
    if fields.get("id").map(String::as_str) != Some(expected_id) {
        return Err(Error::Metadata);
    }
    Version::parse(fields.get("version").ok_or(Error::Metadata)?).map_err(|_| Error::Metadata)
}

fn manifest_fields(text: &str) -> Result<std::collections::HashMap<String, String>> {
    let mut reader = quick_xml::Reader::from_str(text);
    let mut fields = std::collections::HashMap::new();
    loop {
        match reader.read_event().map_err(|_| Error::Metadata)? {
            quick_xml::events::Event::Start(element)
                if matches!(
                    element.name().as_ref(),
                    b"id" | b"version" | b"os" | b"machineArchitecture" | b"channel" | b"mainExe"
                ) =>
            {
                let name = std::str::from_utf8(element.name().as_ref())
                    .map_err(|_| Error::Metadata)?
                    .to_string();
                if fields.contains_key(&name) {
                    return Err(Error::Metadata);
                }
                fields.insert(
                    name,
                    reader
                        .read_text(element.name())
                        .map_err(|_| Error::Metadata)?
                        .decode()
                        .map_err(|_| Error::Metadata)?
                        .into_owned(),
                );
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(fields)
}

pub(crate) fn validate_package_manifest(
    file: &mut File,
    package_id: &str,
    version: &Version,
    channel: Channel,
) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| Error::Metadata)?;
    let mut seen = std::collections::HashSet::new();
    for name in archive.file_names() {
        validate_engine_member_path(name, package_id)?;
        if name.starts_with('/')
            || name.contains(['\\', ':'])
            || name.ends_with(".__symlink")
            || !seen.insert(name.to_lowercase())
            || name.trim_end_matches('/').split('/').any(|part| {
                part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' '])
            })
        {
            return Err(Error::Metadata);
        }
    }
    for index in 0..archive.len() {
        if archive
            .by_index(index)
            .map_err(|_| Error::Metadata)?
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(Error::Metadata);
        }
    }
    let manifests: Vec<_> = archive
        .file_names()
        .filter(|name| name.ends_with(".nuspec"))
        .map(str::to_owned)
        .collect();
    if manifests.len() != 1 {
        return Err(Error::Metadata);
    }
    let mut entry = archive
        .by_name(&manifests[0])
        .map_err(|_| Error::Metadata)?;
    if entry.size() > 16384 {
        return Err(Error::Metadata);
    }
    let mut text = String::new();
    entry.by_ref().take(16385).read_to_string(&mut text)?;
    if text.len() > 16384 {
        return Err(Error::Metadata);
    }
    let fields = manifest_fields(&text)?;
    validate_manifest_fields(&fields, package_id, version, channel)?;
    drop(entry);
    let embedded: Vec<_> = archive
        .file_names()
        .filter(|name| name.starts_with("lib/") && name.ends_with("/sq.version"))
        .map(str::to_owned)
        .collect();
    for name in embedded {
        let mut file = archive.by_name(&name).map_err(|_| Error::Metadata)?;
        let mut text = String::new();
        file.by_ref().take(16385).read_to_string(&mut text)?;
        if text.len() > 16384 {
            return Err(Error::Metadata);
        }
        validate_manifest_fields(&manifest_fields(&text)?, package_id, version, channel)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_members_are_authenticated_at_their_final_installed_destinations() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        let package = root.path().join("full.zip");
        let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
        for (relative, bytes) in [
            ("noh.exe", b"gui".as_slice()),
            ("Squirrel.exe", b"updater".as_slice()),
            ("NOH_ExecutionStub.exe", b"launcher".as_slice()),
        ] {
            writer
                .start_file(
                    format!("lib/app/{relative}"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
            std::fs::write(
                installed_member_path(&current, relative, "NOH").unwrap(),
                bytes,
            )
            .unwrap();
        }
        writer.finish().unwrap();
        let mut input = File::open(&package).unwrap();
        let held = protect_installed_package(&mut input, &current, "NOH").unwrap();
        assert_eq!(held.len(), 3);
        assert!(!current.join("Squirrel.exe").exists());
        assert!(!current.join("NOH_ExecutionStub.exe").exists());
        assert!(std::fs::write(root.path().join("Update.exe"), b"changed").is_err());
        assert!(
            std::fs::rename(root.path().join("NOH.exe"), root.path().join("other.exe")).is_err()
        );
        drop(held);
        std::fs::write(root.path().join("Update.exe"), b"changed").unwrap();
        assert!(matches!(
            protect_installed_package(&mut input, &current, "NOH"),
            Err(Error::Integrity)
        ));
        std::fs::write(root.path().join("Update.exe"), b"updater").unwrap();
        std::fs::write(root.path().join("NOH.exe"), b"tampered").unwrap();
        assert!(matches!(
            protect_installed_package(&mut input, &current, "NOH"),
            Err(Error::Integrity)
        ));
        assert!(installed_member_path(&current, "bin/Squirrel.exe", "NOH").is_err());
        assert!(installed_member_path(&current, "Foreign_ExecutionStub.exe", "NOH").is_err());
        assert!(installed_member_path(&current, "bin/NOH_ExecutionStub.exe", "NOH").is_err());
        assert!(installed_member_path(&current, "runtime.dll.", "NOH").is_err());
        assert!(installed_member_path(&current, "bin//runtime.dll", "NOH").is_err());
        assert!(installed_member_path(&current, "FooSquirrel.exe", "NOH").is_err());
        assert!(installed_member_path(&current, "Foo_ExecutionStubXexe", "NOH").is_err());
    }
    #[test]
    fn installed_members_reject_windows_destination_aliases() {
        for (identity, members) in [
            ("Update", vec!["Squirrel.exe", "Update_ExecutionStub.exe"]),
            ("NOH", vec!["runtime.dll", "RUNTIME.dll"]),
        ] {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("current");
            std::fs::create_dir(&current).unwrap();
            let package = root.path().join("full.zip");
            let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
            for relative in members {
                writer
                    .start_file(
                        format!("lib/app/{relative}"),
                        zip::write::SimpleFileOptions::default(),
                    )
                    .unwrap();
                writer.write_all(b"same bytes").unwrap();
                std::fs::write(
                    installed_member_path(&current, relative, identity).unwrap(),
                    b"same bytes",
                )
                .unwrap();
            }
            writer.finish().unwrap();
            assert!(matches!(
                protect_installed_package(&mut File::open(&package).unwrap(), &current, identity),
                Err(Error::Metadata)
            ));
        }
    }
    #[test]
    fn engine_interpreted_variants_and_members_outside_app_are_refused() {
        for name in [
            "lib/app/FooSquirrel.exe",
            "lib/app/Foo_ExecutionStubXexe",
            "tools/Squirrel.exe",
            "tools/NOH_ExecutionStub.exe",
            "lib/app/Foo_ExecutionStub/exe",
            "lib/app/FooX__symlink",
            "lib/app/Foo/__symlink",
        ] {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("current");
            std::fs::create_dir(&current).unwrap();
            std::fs::write(current.join("noh.exe"), b"gui").unwrap();
            let package = root.path().join("full.zip");
            let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
            for (member, bytes) in [
                ("lib/app/noh.exe", b"gui".as_slice()),
                (name, b"engine".as_slice()),
            ] {
                writer
                    .start_file(member, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
            assert!(
                matches!(
                    protect_installed_package(&mut File::open(&package).unwrap(), &current, "NOH"),
                    Err(Error::Metadata)
                ),
                "unsupported engine member {name}"
            );
        }
    }
    #[test]
    fn inactivity_scan_refuses_live_runtime_but_accepts_held_terminated_process() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        let executable = current.join("noh-inactivity-test.exe");
        std::fs::copy(
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe"),
            &executable,
        )
        .unwrap();
        let mut child = std::process::Command::new(&executable)
            .args(["/D", "/Q"])
            .creation_flags(threading::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let live = ensure_no_running_installed_processes(root.path());
        assert!(
            wait_for_installed_process_exit(root.path(), std::time::Instant::now(), Duration::ZERO)
                .is_err(),
            "A live process must not pass an expired activity deadline"
        );
        let cancelled = AtomicBool::new(false);
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                cancelled.store(true, Ordering::Relaxed);
            });
            assert!(matches!(
                wait_for_installed_process_exit_cancellable(
                    root.path(),
                    started,
                    Duration::from_secs(10),
                    &cancelled
                ),
                Err(Error::Cancelled)
            ));
        });
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            child.try_wait().unwrap().is_none(),
            "Cancellation does not terminate an application"
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"exit 0\r\n")
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Native inactivity fixture did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
        assert!(live.is_err(), "A directly launched live runtime must block");
        let mut code = 259;
        assert_ne!(
            unsafe { threading::GetExitCodeProcess(child.as_raw_handle(), &mut code) },
            0
        );
        assert_eq!(code, 0, "The real process handle proves termination");
        ensure_no_running_installed_processes(root.path()).unwrap();
        drop(child);
    }
    #[test]
    fn activity_wait_covers_exit_after_runtime_lease_release() {
        use std::os::windows::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        let executable = current.join("noh-delayed-exit-test.exe");
        std::fs::copy(
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe"),
            &executable,
        )
        .unwrap();
        let mut child = std::process::Command::new(&executable)
            .args(["/D", "/Q"])
            .creation_flags(threading::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // The process deliberately has no runtime lease: it models the short
        // teardown interval after a managed application's lease is dropped.
        let lease = super::super::windows::ExclusiveLease::acquire(root.path()).unwrap();
        assert!(running_installed_process(root.path()).unwrap().is_some());
        let exiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"exit 0\r\n")
                .unwrap();
            assert!(child.wait().unwrap().success());
        });
        let start = std::time::Instant::now();
        wait_for_installed_process_exit(root.path(), start, Duration::from_secs(5)).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(100));
        exiting.join().unwrap();
        drop(lease);
    }
    #[test]
    fn restart_verifies_all_application_files_and_holds_protection() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        let package = root.path().join("package.zip");
        let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
        for (name, bytes) in [
            ("noh.exe", b"gui".as_slice()),
            ("runtime.dll", b"dll".as_slice()),
        ] {
            writer
                .start_file(
                    format!("lib/app/{name}"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
            std::fs::write(current.join(name), bytes).unwrap();
        }
        writer.finish().unwrap();
        let mut source = File::open(&package).unwrap();
        let held = protect_installed_package(&mut source, &current, "NOH").unwrap();
        assert_eq!(held.len(), 2);
        assert!(std::fs::write(current.join("runtime.dll"), b"bad").is_err());
        assert!(std::fs::rename(&current, root.path().join("replaced")).is_err());
        drop(held);
        std::fs::write(current.join("runtime.dll"), b"bad").unwrap();
        assert!(matches!(
            protect_installed_package(&mut source, &current, "NOH"),
            Err(Error::Integrity)
        ));
        std::fs::remove_file(current.join("noh.exe")).unwrap();
        assert!(protect_installed_package(&mut source, &current, "NOH").is_err());
    }
    #[test]
    fn restart_classifies_integrity_failure_before_process_launch_failure() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        let package = root.path().join("package.zip");
        let mut writer = zip::ZipWriter::new(File::create(&package).unwrap());
        writer
            .start_file("lib/app/noh.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not a PE executable").unwrap();
        writer.finish().unwrap();
        std::fs::write(current.join("noh.exe"), b"not a PE executable").unwrap();
        std::fs::write(current.join("sq.version"), manifest("NOH", "2.0.0", "x64")).unwrap();
        std::fs::write(root.path().join(".portable"), b"").unwrap();
        std::fs::write(root.path().join("helper"), b"unused test helper").unwrap();
        let (envelope, keys) = super::super::tests::signed(&super::super::tests::release());
        let mut guard = GuardedUpdate {
            package: ProtectedFile::open(&package).unwrap(),
            helper: ProtectedFile::open(&root.path().join("helper")).unwrap(),
            root_marker: ProtectedFile::open(&root.path().join(".portable")).unwrap(),
            package_path: package,
            helper_path: root.path().join("helper"),
            root: root.path().to_owned(),
            release: VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap(),
            package_id: "NOH".into(),
        };
        assert!(matches!(guard.restart_installed(), Ok(Err(Error::Io(_)))));
        std::fs::write(current.join("noh.exe"), b"altered installation").unwrap();
        assert!(matches!(guard.restart_installed(), Err(Error::Integrity)));
    }
    use ring::signature::{Ed25519KeyPair, KeyPair};
    fn manifest(id: &str, version: &str, arch: &str) -> String {
        format!(
            "<package><metadata><id>{id}</id><version>{version}</version><mainExe>probe.exe</mainExe><os>win</os><machineArchitecture>{arch}</machineArchitecture><channel>win-x64-stable</channel></metadata></package>"
        )
    }
    fn package(path: &Path, spec: &str, executable: &[u8]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in [
            ("probe.nuspec", spec.as_bytes()),
            ("lib/app/probe.exe", executable),
            (
                "lib/app/marker.txt",
                b"authenticated new version".as_slice(),
            ),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn package_manifest_must_match_signed_identity_target_channel_and_version() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.nupkg");
        for (id, version, arch, expected) in [
            ("NOH", "2.0.0", "x64", true),
            ("Other", "2.0.0", "x64", false),
            ("NOH", "1.0.0", "x64", false),
            ("NOH", "2.0.0", "arm64", false),
        ] {
            package(&path, &manifest(id, version, arch), b"test");
            assert_eq!(
                validate_package_manifest(
                    &mut File::open(&path).unwrap(),
                    "NOH",
                    &Version::new(2, 0, 0),
                    Channel::Stable
                )
                .is_ok(),
                expected
            );
        }
        assert!(
            validate_package_manifest(
                &mut File::open(path).unwrap(),
                "NOH",
                &Version::new(2, 0, 0),
                Channel::Beta
            )
            .is_err()
        );
    }
    #[test]
    fn duplicate_manifest_fields_are_rejected() {
        assert!(manifest_fields("<package><id>NOH</id><id>Other</id></package>").is_err());
    }
    #[test]
    fn process_paths_match_canonical_installation_root_without_prefix_confusion() {
        assert!(under_root(
            Path::new(r"C:\Users\Fixture\current\noh.exe"),
            Path::new(r"\\?\C:\Users\Fixture")
        ));
        assert!(!under_root(
            Path::new(r"C:\Users\Fixture-other\noh.exe"),
            Path::new(r"\\?\C:\Users\Fixture")
        ));
    }
    #[test]
    #[ignore = "Explicit local helper qualification; requires fixture environment paths"]
    fn authenticated_official_helper_handoff() {
        let helper =
            PathBuf::from(std::env::var_os("NOH_UPDATE_HELPER").expect("NOH_UPDATE_HELPER"));
        let executable =
            std::fs::read(std::env::var_os("NOH_UPDATE_PROBE").expect("NOH_UPDATE_PROBE")).unwrap();
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".mcp-dev/update-feasibility");
        let fixture = tempfile::Builder::new()
            .prefix("rust-guardian-")
            .tempdir_in(base)
            .unwrap();
        let fixture_path = fixture.keep();
        let fixture = fixture_path.as_path();
        let root = fixture.join("installation");
        let current = root.join("current");
        let packages = root.join("packages");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir(&packages).unwrap();
        std::fs::write(root.join(".portable"), b"").unwrap();
        std::fs::copy(&helper, root.join("Update.exe")).unwrap();
        std::fs::write(
            current.join("sq.version"),
            manifest("NohUpdateBoundaryProbe", "1.0.0", "x64"),
        )
        .unwrap();
        std::fs::write(current.join("probe.exe"), &executable).unwrap();
        std::fs::write(current.join("marker.txt"), b"old").unwrap();
        let package_path = packages.join("NohUpdateBoundaryProbe-1.0.1-full.nupkg");
        package(
            &package_path,
            &manifest("NohUpdateBoundaryProbe", "1.0.1", "x64"),
            &executable,
        );
        let bytes = std::fs::read(&package_path).unwrap();
        let release = Release {
            schema_version: 1,
            profile: Profile::Complete,
            package_id: "NohUpdateBoundaryProbe".into(),
            version: Version::new(1, 0, 1),
            channel: Channel::Stable,
            notes: "fixture".into(),
            artifacts: vec![Artifact {
                target: Target {
                    os: Os::Windows,
                    arch: Architecture::X64,
                },
                kind: PackageKind::Full,
                setup: None,
                base_version: None,
                file_name: package_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                url: "https://github.com/example/noh/releases/download/fixture/fixture.nupkg"
                    .into(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            }],
        };
        let key = Ed25519KeyPair::from_seed_unchecked(&[11; 32]).unwrap();
        let payload = serde_json::to_vec(&release).unwrap();
        let envelope = serde_json::to_vec(&Envelope {
            key_id: "fixture".into(),
            payload: STANDARD.encode(&payload),
            signature: STANDARD.encode(key.sign(&payload).as_ref()),
        })
        .unwrap();
        let keys = [TrustKey {
            id: "fixture".into(),
            public_key: key.public_key().as_ref().try_into().unwrap(),
        }];
        let guard = GuardedUpdate::prepare(
            &envelope,
            &keys,
            "NohUpdateBoundaryProbe",
            Channel::Stable,
            &package_path,
            &helper,
            &root,
        )
        .unwrap();
        assert!(std::fs::write(&package_path, b"tampering").is_err());
        let runtime = super::super::windows::RuntimeLease::acquire(&root).unwrap();
        assert!(
            guard
                .apply(
                    Duration::from_millis(100),
                    Duration::from_secs(45),
                    &fixture.join("blocked-helper.log")
                )
                .is_err()
        );
        assert!(!fixture.join("blocked-helper.log").exists());
        assert_eq!(std::fs::read(current.join("marker.txt")).unwrap(), b"old");
        drop(runtime);
        // A direct runtime without the cooperative lease must be refused rather
        // than killed by the official helper's force-stop behavior.
        let mut active = std::process::Command::new(current.join("probe.exe"))
            .arg("--hold")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            ensure_no_running_installed_processes(&std::fs::canonicalize(&root).unwrap()).is_err()
        );
        assert!(active.wait().unwrap().success());
        let guard = GuardedUpdate::prepare(
            &envelope,
            &keys,
            "NohUpdateBoundaryProbe",
            Channel::Stable,
            &package_path,
            &helper,
            &root,
        )
        .unwrap();
        let version = guard
            .apply(
                Duration::from_secs(2),
                Duration::from_secs(45),
                &fixture.join("helper.log"),
            )
            .unwrap();
        assert_eq!(version, Version::new(1, 0, 1));
        assert_eq!(
            std::fs::read(current.join("marker.txt")).unwrap(),
            b"authenticated new version"
        );
        let evidence = fixture.to_path_buf();
        std::fs::write(evidence.join("result.json"),serde_json::to_vec_pretty(&serde_json::json!({"version":version.to_string(),"authenticated_helper_apply":true,"active_lease_refused_before_helper":true,"direct_active_runtime_refused_without_kill":true,"qualification":"tiny fixture only","helper":"1.2.161"})).unwrap()).unwrap();
        println!("Guardian evidence: {}", evidence.display());
    }
}
fn validate_manifest_fields(
    fields: &std::collections::HashMap<String, String>,
    package_id: &str,
    version: &Version,
    channel: Channel,
) -> Result<()> {
    let expected_channel = match channel {
        Channel::Stable => "win-x64-stable",
        Channel::Beta => "win-x64-beta",
    };
    let version = version.to_string();
    for (key, expected) in [
        ("id", package_id),
        ("version", version.as_str()),
        ("os", "win"),
        ("machineArchitecture", "x64"),
        ("channel", expected_channel),
    ] {
        if fields.get(key).map(String::as_str) != Some(expected) {
            return Err(Error::Metadata);
        }
    }
    let executable = fields.get("mainExe").ok_or(Error::Metadata)?;
    if !executable.ends_with(".exe")
        || !executable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(Error::Metadata);
    }
    Ok(())
}
