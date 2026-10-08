//! Nonprivileged Windows protection primitives. No installation qualification implied.
use super::*;
use std::{
    ffi::OsStr,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
};
use windows_sys::Win32::{
    Foundation as wf,
    System::{JobObjects as wj, Threading as wt},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess,
            PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);
impl Handle {
    fn checked(handle: HANDLE) -> std::io::Result<Self> {
        if handle.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Owns a kill-on-close job BEFORE its child starts executing. Handles are not inherited.
pub struct SupervisedProcess {
    job: Handle,
    process: Handle,
    pid: u32,
    detached: bool,
}
impl SupervisedProcess {
    pub fn spawn(executable: &Path, arguments: &[&OsStr], cwd: &Path) -> std::io::Result<Self> {
        Self::spawn_protected(executable, arguments, cwd, &[])
    }
    pub fn spawn_protected(
        executable: &Path,
        arguments: &[&OsStr],
        cwd: &Path,
        protections: &[&ProtectedFile],
    ) -> std::io::Result<Self> {
        // Copy protection directly into the suspended child. No globally inheritable
        // handles: concurrent unrelated spawns cannot accidentally inherit locks.
        let job = Handle::checked(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let executable_wide = wide(executable.as_os_str())?;
        let cwd = wide(cwd.as_os_str())?;
        let mut command = quote(executable.as_os_str())?;
        for argument in arguments {
            command.push(b' ' as u16);
            command.extend(quote(argument)?);
        }
        command.push(0);
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of_val(&startup) as u32;
        let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            CreateProcessW(
                executable_wide.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_SUSPENDED | CREATE_NO_WINDOW,
                std::ptr::null(),
                cwd.as_ptr(),
                &startup,
                &mut information,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let process = Handle(information.hProcess);
        let thread = Handle(information.hThread);
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                TerminateProcess(process.0, 1);
                WaitForSingleObject(process.0, 5000);
            }
            return Err(error);
        }
        for protection in protections {
            if let Err(error) = protection.copy_into(process.0) {
                drop(job);
                unsafe {
                    WaitForSingleObject(process.0, 5000);
                }
                return Err(error);
            }
        }
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            let error = std::io::Error::last_os_error();
            drop(job); // Kills the child, including failure paths before construction.
            unsafe {
                WaitForSingleObject(process.0, 5000);
            }
            return Err(error);
        }
        Ok(Self {
            job,
            process,
            pid: information.dwProcessId,
            detached: false,
        })
    }
    pub fn id(&self) -> u32 {
        self.pid
    }
    /// Check that an observed consumer belongs to this supervised process tree.
    pub fn contains_process(&self, pid: u32) -> std::io::Result<bool> {
        let process = Handle::checked(unsafe {
            wt::OpenProcess(wt::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
        })?;
        let mut member = 0;
        if unsafe { wj::IsProcessInJob(process.0, self.job.0, &mut member) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(member != 0)
    }
    /// Bounded diagnostic snapshot; returns an error rather than truncating a large tree.
    pub fn process_ids(&self) -> std::io::Result<Vec<u32>> {
        #[repr(C)]
        struct ProcessList {
            assigned: u32,
            count: u32,
            ids: [usize; 64],
        }
        let mut list = ProcessList {
            assigned: 0,
            count: 0,
            ids: [0; 64],
        };
        if unsafe {
            wj::QueryInformationJobObject(
                self.job.0,
                wj::JobObjectBasicProcessIdList,
                &mut list as *mut _ as _,
                std::mem::size_of_val(&list) as u32,
                std::ptr::null_mut(),
            )
        } == 0
            || list.count as usize > list.ids.len()
        {
            return Err(std::io::Error::last_os_error());
        }
        list.ids[..list.count as usize]
            .iter()
            .map(|pid| {
                u32::try_from(*pid).map_err(|_| std::io::Error::other("Invalid process identifier"))
            })
            .collect()
    }
    /// Transfer a restarted GUI to its normal application lifetime. Protections
    /// copied into the child remain alive until that process exits. Failure keeps
    /// the kill-on-close job, so an unowned child cannot escape supervision.
    pub fn detach(mut self) -> std::io::Result<u32> {
        let limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            SetInformationJobObject(
                self.job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        self.detached = true;
        Ok(self.pid)
    }
    pub fn wait(&self, timeout: Duration) -> std::io::Result<Option<u32>> {
        let milliseconds = timeout.as_millis().min((u32::MAX - 1) as u128) as u32;
        match unsafe { WaitForSingleObject(self.process.0, milliseconds) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(Some(code))
                }
            }
            _ => Err(std::io::Error::last_os_error()),
        }
    }
    pub fn active_processes(&self) -> std::io::Result<u32> {
        let mut information: wj::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
            unsafe { std::mem::zeroed() };
        if unsafe {
            wj::QueryInformationJobObject(
                self.job.0,
                wj::JobObjectBasicAccountingInformation,
                &mut information as *mut _ as _,
                std::mem::size_of_val(&information) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(information.ActiveProcesses)
    }
    /// Windows continuously tracks this job-wide peak; no model/process polling.
    /// This is committed virtual memory, not resident working-set memory.
    pub fn peak_committed_bytes(&self) -> std::io::Result<u64> {
        let mut information: wj::JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
            unsafe { std::mem::zeroed() };
        if unsafe {
            wj::QueryInformationJobObject(
                self.job.0,
                wj::JobObjectExtendedLimitInformation,
                &mut information as *mut _ as _,
                std::mem::size_of_val(&information) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(information.PeakJobMemoryUsed as u64)
    }
    pub fn terminate_tree(&self, timeout: Duration) -> std::io::Result<()> {
        if unsafe { wj::TerminateJobObject(self.job.0, 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let start = std::time::Instant::now();
        if self.wait(timeout)?.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Update leader did not terminate",
            ));
        }
        while self.active_processes()? != 0 {
            if start.elapsed() >= timeout {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Update process tree did not terminate",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}
impl Drop for SupervisedProcess {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        // Best-effort fallback. Callers use terminate_tree and inspect its result.
        // Consumer-owned protection handles stay alive even if this cleanup fails.
        let _ = self.terminate_tree(Duration::from_secs(5));
    }
}

fn wide(value: &OsStr) -> std::io::Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "NUL in process argument",
        ));
    }
    value.push(0);
    Ok(value)
}
pub fn available_disk_bytes(path: &Path) -> std::io::Result<u64> {
    let path = wide(path.as_os_str())?;
    let mut available = 0u64;
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(available)
}
fn quote(value: &OsStr) -> std::io::Result<Vec<u16>> {
    let raw = wide(value)?;
    let mut output = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in &raw[..raw.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(output)
}

/// Prevents file mutation and ancestor replacement while the consumer uses this path.
pub struct ProtectedFile {
    file: File,
    _ancestors: Vec<File>,
}
impl ProtectedFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_kind(path, false)
    }
    /// Pin a directory and reject reparse ancestors before creating child files.
    pub fn directory(path: &Path) -> std::io::Result<Self> {
        Self::open_kind(path, true)
    }
    fn open_kind(path: &Path, directory: bool) -> std::io::Result<Self> {
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Absolute staging path required",
            ));
        }
        if path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Staging path traversal rejected",
            ));
        }
        let mut ancestors = Vec::new();
        // Root-to-leaf avoids checking through an ancestor that has not been protected.
        let directories: Vec<_> = path
            .parent()
            .ok_or_else(|| std::io::Error::other("Package parent missing"))?
            .ancestors()
            .collect();
        for directory in directories.into_iter().rev() {
            let handle = std::fs::OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(directory)?;
            if handle.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(std::io::Error::other(
                    "Reparse points are unsupported in update staging paths",
                ));
            }
            ancestors.push(handle);
        }
        let file = if directory {
            std::fs::OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?
        } else {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?
        };
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::other("Reparse package rejected"));
        }
        Ok(Self {
            file,
            _ancestors: ancestors,
        })
    }
    pub fn file(&mut self) -> &mut File {
        &mut self.file
    }
    fn copy_into(&self, consumer: HANDLE) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        self._ancestors
            .iter()
            .chain(std::iter::once(&self.file))
            .map(|file| {
                let mut duplicate = std::ptr::null_mut();
                let process = unsafe { wt::GetCurrentProcess() };
                if unsafe {
                    wf::DuplicateHandle(
                        process,
                        file.as_raw_handle(),
                        consumer,
                        &mut duplicate,
                        0,
                        0,
                        wf::DUPLICATE_SAME_ACCESS,
                    )
                } == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                // This value belongs to the other process; never CloseHandle here.
                Ok(())
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map(|_| ())
    }
}

/// Shared lifetime lease for every executable in a managed portable installation.
/// A guardian's exclusive lease fails while any application/CLI/MCP worker is alive.
pub struct RuntimeLease {
    _file: File,
    _anchor: Option<ProtectedFile>,
}
impl RuntimeLease {
    /// Transfer the activity lock into a suspended restarted process. Ancestor
    /// protection is supplied separately by the authenticated installed files.
    pub fn into_protection(self) -> ProtectedFile {
        let mut ancestors = Vec::new();
        if let Some(anchor) = self._anchor {
            ancestors.push(anchor.file);
            ancestors.extend(anchor._ancestors);
        }
        ProtectedFile {
            file: self._file,
            _ancestors: ancestors,
        }
    }
    pub fn acquire(installation: &Path) -> std::io::Result<Self> {
        Ok(Self {
            _anchor: None,
            _file: open_runtime_lock(installation, FILE_SHARE_READ | FILE_SHARE_WRITE)?,
        })
    }
    pub fn for_current_executable() -> std::io::Result<Option<Self>> {
        let executable = std::env::current_exe()?;
        Self::for_executable(&executable, crate::build_info::current().build_fingerprint)
    }
    pub(crate) fn for_executable(
        executable: &Path,
        fingerprint: &str,
    ) -> std::io::Result<Option<Self>> {
        let directory = executable
            .parent()
            .ok_or_else(|| std::io::Error::other("Executable directory missing"))?;
        // GUI in current/, CLI/MCP in current/bin/.
        let current = if directory.file_name() == Some(OsStr::new("bin")) {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        // Recognize the stable installation root even if the loaded image has
        // moved to a backup directory or current/ is between replacement renames.
        let setup_anchor = super::windows_anchor::InstallationAnchor::for_executable(
            executable,
            super::trust::package_id(),
        )
        .map_err(std::io::Error::other)?;
        let root = executable
            .ancestors()
            .skip(1)
            .take(5)
            .find(|candidate| candidate.join(".portable").is_file());
        let setup_root = setup_anchor.as_ref().map(|anchor| anchor.installation());
        let setup_profile = setup_anchor.as_ref().map(|anchor| anchor.profile());
        let Some(root) = setup_root.as_deref().or(root) else {
            if current.file_name() == Some(OsStr::new("current")) {
                return Err(std::io::Error::other("Managed installation marker missing"));
            }
            return Ok(None);
        };
        let lease = if let Some(anchor) = setup_anchor {
            let mut lease = Self::acquire(&anchor.coordination())?;
            lease._anchor = Some(anchor.into_protection());
            lease
        } else {
            Self::acquire(root)?
        };
        if std::fs::canonicalize(current)? != std::fs::canonicalize(root)?.join("current")
            || !current.join("sq.version").is_file()
        {
            return Err(std::io::Error::other(
                "Managed installation is being replaced or executable is obsolete",
            ));
        }
        // An image loaded before replacement may still report its original path.
        // Its embedded identity must match the newly installed package generation.
        let mut manifest = File::open(current.join("manifest.json"))?;
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut manifest)
            .take(super::setup::MAX_MANIFEST as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > super::setup::MAX_MANIFEST {
            return Err(std::io::Error::other(
                "Managed package manifest exceeds limit",
            ));
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        if let Some(expected) = setup_profile {
            if super::Profile::from_manifest(&manifest).map_err(std::io::Error::other)? != expected
            {
                return Err(std::io::Error::other(
                    "Managed installation content profile differs from its anchor",
                ));
            }
        }
        if manifest["build"]["build_fingerprint"].as_str() != Some(fingerprint) {
            return Err(std::io::Error::other(
                "Loaded executable does not match the managed package generation",
            ));
        }
        Ok(Some(lease))
    }
}

/// Exclusive activity lease. Qualification tools may use a stable external directory.
/// Every participating client must use the same directory; this is not access control.
pub struct ExclusiveLease {
    file: File,
}
impl ExclusiveLease {
    pub fn acquire(installation: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: open_runtime_lock(installation, 0)?,
        })
    }
    pub fn into_protection(self) -> ProtectedFile {
        ProtectedFile {
            file: self.file,
            _ancestors: Vec::new(),
        }
    }
}

fn open_runtime_lock(directory: &Path, sharing: u32) -> std::io::Result<File> {
    use std::os::windows::fs::MetadataExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.join(".noh-update-runtime.lock"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::other(
            "Reparse or non-file runtime lock refused",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_package_denies_write_and_ancestor_replacement() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("packages");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("fixture.nupkg");
        std::fs::write(&path, b"trusted").unwrap();
        let mut protection = ProtectedFile::open(&path).unwrap();
        assert!(std::fs::write(&path, b"forged").is_err());
        assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
        assert!(std::fs::rename(&directory, root.path().join("moved")).is_err());
        let mut bytes = Vec::new();
        protection.file().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"trusted");
        drop(protection);
        std::fs::rename(&directory, root.path().join("moved")).unwrap();
    }
    #[test]
    fn process_deadline_and_drop_terminate_only_owned_child() {
        let exe = std::env::current_exe().unwrap();
        let root = tempfile::tempdir().unwrap();
        let child = SupervisedProcess::spawn(
            &exe,
            &[
                OsStr::new("--ignored"),
                OsStr::new("--exact"),
                OsStr::new("update::windows::tests::sleeping_child"),
            ],
            root.path(),
        )
        .unwrap();
        assert_eq!(child.wait(Duration::from_millis(50)).unwrap(), None);
        let peak = child.peak_committed_bytes().unwrap();
        assert!(peak > 0);
        child.terminate_tree(Duration::from_secs(5)).unwrap();
        assert!(child.peak_committed_bytes().unwrap() >= peak);
        let start = std::time::Instant::now();
        drop(child);
        assert!(start.elapsed() < Duration::from_secs(6));
    }
    #[test]
    #[ignore = "Invoked only by the isolated supervisor test"]
    fn sleeping_child() {
        std::fs::write("child.pid", std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }
    #[test]
    #[ignore = "Invoked only by the guardian crash test"]
    fn guardian_child() {
        let exe = std::env::current_exe().unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::fs::write(cwd.join("guarded.nupkg"), b"authenticated").unwrap();
        let protection = ProtectedFile::open(&cwd.join("guarded.nupkg")).unwrap();
        let child = SupervisedProcess::spawn_protected(
            &exe,
            &[
                OsStr::new("--ignored"),
                OsStr::new("--exact"),
                OsStr::new("update::windows::tests::sleeping_child"),
            ],
            &cwd,
            &[&protection],
        )
        .unwrap();
        std::fs::write("guard.ready", child.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        drop(child);
    }
    #[test]
    fn abrupt_guardian_death_closes_job_and_kills_consumer() {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let root = tempfile::tempdir().unwrap();
        let mut guardian = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "update::windows::tests::guardian_child",
            ])
            .current_dir(root.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while !root.path().join("child.pid").exists() && started.elapsed() < Duration::from_secs(10)
        {
            if guardian.try_wait().unwrap().is_some() {
                panic!("Guardian exited before fixture consumer started");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid = std::fs::read_to_string(root.path().join("child.pid"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let process = Handle::checked(unsafe {
            OpenProcess(0x00100000 | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
        })
        .unwrap();
        assert!(std::fs::write(root.path().join("guarded.nupkg"), b"forged").is_err());
        guardian.kill().unwrap();
        guardian.wait().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(process.0, 5000) },
            WAIT_OBJECT_0
        );
        std::fs::write(root.path().join("guarded.nupkg"), b"consumer terminated").unwrap();
    }
    #[test]
    fn junction_ancestor_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let link = root.path().join("junction");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("a.nupkg"), b"fixture").unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(ProtectedFile::open(&link.join("a.nupkg")).is_err());
        assert_eq!(std::fs::read(real.join("a.nupkg")).unwrap(), b"fixture");
    }
    #[test]
    fn runtime_lease_rejects_reparse_lock_entry() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let lock = root.path().join(".noh-update-runtime.lock");
        std::fs::write(&target, b"unchanged").unwrap();
        match std::os::windows::fs::symlink_file(&target, &lock) {
            Ok(()) => {
                assert!(RuntimeLease::acquire(root.path()).is_err());
                assert!(ExclusiveLease::acquire(root.path()).is_err());
                assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
                std::fs::remove_file(&lock).unwrap();
            }
            Err(error) if error.raw_os_error() == Some(1314) => {
                // Windows without Developer Mode cannot create a file symlink.
                // Exercise the unprivileged directory-reparse rejection instead;
                // do not report that fallback as a file-symlink reproduction.
                eprintln!("File symlink unavailable (1314); testing directory junction lock entry");
                let directory = root.path().join("target-directory");
                std::fs::create_dir(&directory).unwrap();
                let status = std::process::Command::new("cmd.exe")
                    .args(["/c", "mklink", "/J"])
                    .arg(&lock)
                    .arg(&directory)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap();
                assert!(status.success());
                assert!(RuntimeLease::acquire(root.path()).is_err());
                assert!(ExclusiveLease::acquire(root.path()).is_err());
                std::fs::remove_dir(&lock).unwrap();
            }
            Err(error) => panic!("Cannot create lock fixture: {error}"),
        }
        let shared = RuntimeLease::acquire(root.path()).unwrap();
        assert!(ExclusiveLease::acquire(root.path()).is_err());
        drop(shared);
        let exclusive = ExclusiveLease::acquire(root.path()).unwrap();
        assert!(RuntimeLease::acquire(root.path()).is_err());
        drop(exclusive);
        assert!(RuntimeLease::acquire(root.path()).is_ok());
    }
    #[test]
    fn runtime_leases_exclude_application_and_update_concurrency() {
        let root = tempfile::tempdir().unwrap();
        let first = RuntimeLease::acquire(root.path()).unwrap();
        let second = RuntimeLease::acquire(root.path()).unwrap();
        assert!(ExclusiveLease::acquire(root.path()).is_err());
        drop(first);
        assert!(ExclusiveLease::acquire(root.path()).is_err());
        drop(second);
        let update = ExclusiveLease::acquire(root.path()).unwrap();
        assert!(RuntimeLease::acquire(root.path()).is_err());
        assert!(ExclusiveLease::acquire(root.path()).is_err());
        drop(update);
        assert!(RuntimeLease::acquire(root.path()).is_ok());
    }
    #[test]
    fn managed_startup_fails_closed_during_rename_and_for_obsolete_loaded_image() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        std::fs::create_dir(&current).unwrap();
        std::fs::write(root.path().join(".portable"), b"").unwrap();
        let exe = current.join("noh.exe");
        assert!(RuntimeLease::for_executable(&exe, "old-build").is_err());
        let update = ExclusiveLease::acquire(root.path()).unwrap();
        assert!(RuntimeLease::for_executable(&exe, "old-build").is_err());
        let backup = root.path().join("backup");
        std::fs::rename(&current, &backup).unwrap();
        assert!(RuntimeLease::for_executable(&backup.join("noh.exe"), "old-build").is_err());
        std::fs::create_dir(&current).unwrap();
        std::fs::write(current.join("sq.version"), b"new manifest").unwrap();
        std::fs::write(
            current.join("manifest.json"),
            br#"{"build":{"build_fingerprint":"new-build"}}"#,
        )
        .unwrap();
        assert!(RuntimeLease::for_executable(&exe, "new-build").is_err());
        drop(update);
        assert!(RuntimeLease::for_executable(&backup.join("noh.exe"), "old-build").is_err());
        assert!(RuntimeLease::for_executable(&exe, "old-build").is_err());
        assert!(
            RuntimeLease::for_executable(&exe, "new-build")
                .unwrap()
                .is_some()
        );
    }
    #[test]
    #[ignore = "Invoked only by the native loaded-image startup test"]
    fn loaded_runtime_participant() {
        let cwd = std::env::current_dir().unwrap();
        std::fs::write(cwd.join("loaded"), b"ready").unwrap();
        let start = std::time::Instant::now();
        while !cwd.join("go").exists() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(cwd.join("go").exists());
        match RuntimeLease::for_current_executable() {
            Ok(Some(_lease)) => {
                std::fs::write(cwd.join("application-entered"), b"entered").unwrap();
            }
            Err(_) => {
                std::fs::write(cwd.join("startup-refused"), b"refused").unwrap();
            }
            Ok(None) => panic!("Managed loaded image must never skip its lease"),
        }
    }
    #[test]
    fn native_loaded_image_cannot_enter_application_during_or_after_replacement() {
        for during_update in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("current");
            std::fs::create_dir(&current).unwrap();
            std::fs::write(root.path().join(".portable"), b"").unwrap();
            let executable = current.join("participant.exe");
            std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
            std::fs::write(current.join("sq.version"), b"fixture").unwrap();
            std::fs::write(current.join("manifest.json"),serde_json::to_vec(&serde_json::json!({"build":{"build_fingerprint":crate::build_info::current().build_fingerprint}})).unwrap()).unwrap();
            let participant = SupervisedProcess::spawn(
                &executable,
                &[
                    OsStr::new("--ignored"),
                    OsStr::new("--exact"),
                    OsStr::new("update::windows::tests::loaded_runtime_participant"),
                ],
                root.path(),
            )
            .unwrap();
            let start = std::time::Instant::now();
            while !root.path().join("loaded").exists() && start.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(root.path().join("loaded").exists());
            let update = ExclusiveLease::acquire(root.path()).unwrap();
            std::fs::rename(&current, root.path().join("backup")).unwrap();
            if !during_update {
                std::fs::create_dir(&current).unwrap();
                std::fs::write(current.join("sq.version"), b"fixture B").unwrap();
                std::fs::write(
                    current.join("manifest.json"),
                    br#"{"build":{"build_fingerprint":"replacement-generation"}}"#,
                )
                .unwrap();
                drop(update);
            }
            std::fs::write(root.path().join("go"), b"continue").unwrap();
            assert_eq!(participant.wait(Duration::from_secs(10)).unwrap(), Some(0));
            assert!(root.path().join("startup-refused").exists());
            assert!(!root.path().join("application-entered").exists());
        }
    }
    #[test]
    fn consumer_owns_protection_after_supervisor_releases_its_copy() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.nupkg");
        std::fs::write(&path, b"authenticated").unwrap();
        let protection = ProtectedFile::open(&path).unwrap();
        let child = SupervisedProcess::spawn_protected(
            &std::env::current_exe().unwrap(),
            &[
                OsStr::new("--ignored"),
                OsStr::new("--exact"),
                OsStr::new("update::windows::tests::sleeping_child"),
            ],
            root.path(),
            &[&protection],
        )
        .unwrap();
        drop(protection);
        assert!(std::fs::write(&path, b"forged").is_err());
        assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
        child.terminate_tree(Duration::from_secs(5)).unwrap();
        assert_eq!(child.active_processes().unwrap(), 0);
        std::fs::write(&path, b"after consumer death").unwrap();
    }
    #[test]
    #[ignore = "Invoked only by the detached GUI lifetime test"]
    fn detached_child() {
        std::thread::sleep(Duration::from_millis(750));
    }
    #[test]
    fn detached_child_survives_supervisor_and_keeps_protections_until_exit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("protected.dll");
        std::fs::write(&path, b"authenticated").unwrap();
        let protection = ProtectedFile::open(&path).unwrap();
        let lease = RuntimeLease::acquire(root.path())
            .unwrap()
            .into_protection();
        let child = SupervisedProcess::spawn_protected(
            &std::env::current_exe().unwrap(),
            &[
                OsStr::new("--ignored"),
                OsStr::new("--exact"),
                OsStr::new("update::windows::tests::detached_child"),
            ],
            root.path(),
            &[&protection, &lease],
        )
        .unwrap();
        let process =
            Handle::checked(unsafe { wt::OpenProcess(0x00100000, 0, child.id()) }).unwrap();
        let pid = child.id();
        assert_eq!(child.detach().unwrap(), pid);
        drop(protection);
        drop(lease);
        assert_eq!(unsafe { WaitForSingleObject(process.0, 0) }, WAIT_TIMEOUT);
        assert!(std::fs::write(&path, b"forged").is_err());
        assert!(ExclusiveLease::acquire(root.path()).is_err());
        assert_eq!(
            unsafe { WaitForSingleObject(process.0, 5000) },
            WAIT_OBJECT_0
        );
        std::fs::write(&path, b"after exit").unwrap();
        assert!(ExclusiveLease::acquire(root.path()).is_ok());
    }
    #[test]
    #[ignore = "Invoked only by the surviving-descendant test"]
    fn parent_with_surviving_child() {
        let cwd = std::env::current_dir().unwrap();
        let _child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "update::windows::tests::sleeping_child",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        while !cwd.join("child.pid").exists() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(cwd.join("child.pid").exists());
        std::process::exit(0);
    }
    #[test]
    fn termination_covers_descendants_after_leader_exit() {
        let root = tempfile::tempdir().unwrap();
        let child = SupervisedProcess::spawn(
            &std::env::current_exe().unwrap(),
            &[
                OsStr::new("--ignored"),
                OsStr::new("--exact"),
                OsStr::new("update::windows::tests::parent_with_surviving_child"),
            ],
            root.path(),
        )
        .unwrap();
        assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), Some(0));
        assert!(child.active_processes().unwrap() >= 1);
        child.terminate_tree(Duration::from_secs(5)).unwrap();
        assert_eq!(child.active_processes().unwrap(), 0);
    }
}
