//! Manual repair of the controlled prototype, not an ordinary installer.
//! Caller must establish owned guardian/job termination and prevent new actors.
use super::{
    windows::{ExclusiveLease, ProtectedFile},
    windows_guard as guard, *,
};
use std::{collections::HashSet, os::windows::ffi::OsStrExt};
use windows_sys::Win32::{
    Foundation as foundation,
    Storage::FileSystem::MoveFileExW,
    System::{Diagnostics::ToolHelp as snapshot, Threading as threading},
};

const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;

pub fn restore(
    root: &Path,
    archive: &Path,
    version: &Version,
    cancelled: &AtomicBool,
) -> Result<serde_json::Value> {
    let allowed = std::fs::canonicalize(
        option_env!("NOH_UPDATE_QUALIFICATION_ROOT").ok_or(Error::InstallationUnavailable)?,
    )?;
    let root = std::fs::canonicalize(root)?;
    let archive = std::fs::canonicalize(archive)?;
    let executable = std::fs::canonicalize(std::env::current_exe()?)?;
    let working_directory = std::fs::canonicalize(std::env::current_dir()?)?;
    if guard::is_elevated()?
        || root == allowed
        || !root.starts_with(&allowed)
        || !archive.starts_with(&allowed)
        || archive.starts_with(&root)
        || executable.starts_with(&root)
        || working_directory.starts_with(&root)
        || root.starts_with(&archive)
        || Target::native()?
            != (Target {
                os: Os::Windows,
                arch: Architecture::X64,
            })
    {
        return Err(Error::InstallationUnavailable);
    }
    let _marker = ProtectedFile::open(&root.join(".portable"))?;
    refuse_update_owners()?;
    let _lease = ExclusiveLease::acquire(&root)?;
    refuse_update_owners()?;
    if let Some((pid, path)) = guard::running_installed_process(&root)? {
        return Err(Error::Io(std::io::Error::other(format!(
            "Installed process is active: pid={pid} path={}",
            path.display()
        ))));
    }
    let keys = trust::compiled_keys()?;
    let id = trust::package_id();
    // Exact full version is independent of an absent/broken current tree.
    let mut retained = retention::open_cancellable(
        &archive,
        &keys,
        id,
        Target::native()?,
        Channel::Stable,
        version,
        cancelled,
    )?;
    guard::validate_package_manifest(retained.file(), id, version, Channel::Stable)?;
    let stage = tempfile::Builder::new()
        .prefix("manual-repair-")
        .tempdir_in(&root)?;
    let expanded = extract(retained.file(), stage.path(), id, cancelled)?;
    let held = guard::protect_installed_package_cancellable(
        retained.file(),
        &stage.path().join("current"),
        id,
        cancelled,
    )?;
    drop(held);
    check_cancel(cancelled)?;
    refuse_update_owners()?;
    if guard::running_installed_process(&root)?.is_some() {
        return Err(Error::InstallationUnavailable);
    }
    // From here cancellation may leave a partial installation. Retain all trees,
    // record intent first, and never claim automatic rollback or readiness.
    let stage = stage.keep();
    let mut intent = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.join("restore-intent.json"))?;
    intent.write_all(&serde_json::to_vec(&serde_json::json!({"version":version,"package_id":id,"root":root,"archive":archive,"prototype":true})).map_err(|_| Error::Metadata)?)?;
    intent.sync_all()?;
    publish(&stage, &root, id, cancelled)?;
    let _installed = guard::protect_installed_package_cancellable(
        retained.file(),
        &root.join("current"),
        id,
        cancelled,
    )?;
    let report = serde_json::json!({"restored_files_verified":true,"version":version,"package_id":id,"preserved_evidence":stage,"expanded_bytes":expanded,"runtime_readiness":"unverified; caller must run CLI and saved-project GUI","qualification":"controlled manual portable prototype","automatic_rollback":false});
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.join("restored-files.json"))?;
    output.write_all(&serde_json::to_vec_pretty(&report).map_err(|_| Error::Metadata)?)?;
    output.sync_all()?;
    Ok(report)
}

fn check_cancel(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn extract(package: &mut File, stage: &Path, id: &str, cancelled: &AtomicBool) -> Result<u64> {
    package.seek(SeekFrom::Start(0))?;
    let mut zip = zip::ZipArchive::new(package).map_err(|_| Error::Metadata)?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|_| Error::Metadata)?;
        if entry.name().starts_with("lib/app/") && !entry.is_dir() {
            total = total
                .checked_add(entry.size())
                .filter(|value| *value <= MAX_EXPANDED)
                .ok_or(Error::Metadata)?;
        }
    }
    if total == 0 || windows::available_disk_bytes(stage)? < total {
        return Err(Error::Io(std::io::Error::other(
            "Insufficient staging space or empty application",
        )));
    }
    let current = stage.join("current");
    std::fs::create_dir(&current)?;
    let mut destinations = HashSet::new();
    let mut buffer = [0u8; 32768];
    for i in 0..zip.len() {
        check_cancel(cancelled)?;
        let mut entry = zip.by_index(i).map_err(|_| Error::Metadata)?;
        let Some(relative) = entry.name().strip_prefix("lib/app/") else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let destination = guard::installed_member_path(&current, relative, id)?;
        if !destinations.insert(destination.to_string_lossy().to_lowercase()) {
            return Err(Error::Metadata);
        }
        std::fs::create_dir_all(destination.parent().ok_or(Error::Metadata)?)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut copied = 0u64;
        loop {
            check_cancel(cancelled)?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            copied = copied
                .checked_add(n as u64)
                .filter(|value| *value <= entry.size())
                .ok_or(Error::Integrity)?;
            output.write_all(&buffer[..n])?;
        }
        if copied != entry.size() {
            return Err(Error::Integrity);
        }
        output.sync_all()?;
    }
    Ok(total)
}

fn move_no_replace(source: &Path, destination: &Path) -> Result<()> {
    let wide = |p: &Path| {
        let mut v: Vec<u16> = p.as_os_str().encode_wide().collect();
        v.push(0);
        v
    };
    let source = wide(source);
    let destination = wide(destination);
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn publish(stage: &Path, root: &Path, id: &str, cancelled: &AtomicBool) -> Result<()> {
    // Refuse malformed full contents before moving the interrupted application.
    for name in [
        "current/sq.version".to_owned(),
        "Update.exe".to_owned(),
        format!("{id}.exe"),
    ] {
        if !stage.join(name).is_file() {
            return Err(Error::Metadata);
        }
    }
    std::fs::create_dir(stage.join("interrupted"))?;
    for name in [
        "current".to_owned(),
        "Update.exe".to_owned(),
        format!("{id}.exe"),
    ] {
        check_cancel(cancelled)?;
        let existing = root.join(&name);
        match std::fs::symlink_metadata(&existing) {
            Ok(metadata) => {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(Error::Metadata);
                }
                move_no_replace(&existing, &stage.join("interrupted").join(&name))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    for name in [
        "Update.exe".to_owned(),
        format!("{id}.exe"),
        "current".to_owned(),
    ] {
        check_cancel(cancelled)?;
        move_no_replace(&stage.join(&name), &root.join(&name))?;
    }
    Ok(())
}

// Conservative global-name refusal is not an atomic exclusion protocol.
#[cfg(test)]
mod tests {
    use super::*;
    fn staged(parent: &Path) -> PathBuf {
        let stage = parent.join("stage");
        std::fs::create_dir_all(stage.join("current")).unwrap();
        for (name, bytes) in [
            ("current/sq.version", b"spec".as_slice()),
            ("current/noh.exe", b"new"),
            ("Update.exe", b"helper"),
            ("NOH.exe", b"stub"),
        ] {
            std::fs::write(stage.join(name), bytes).unwrap();
        }
        stage
    }
    #[test]
    fn preserves_interrupted_current_and_engine_files() {
        let root = tempfile::tempdir().unwrap();
        let stage = staged(root.path());
        std::fs::create_dir(root.path().join("current")).unwrap();
        std::fs::write(root.path().join("current/noh.exe"), b"broken").unwrap();
        std::fs::write(root.path().join("Update.exe"), b"old-helper").unwrap();
        std::fs::write(root.path().join("NOH.exe"), b"old-stub").unwrap();
        publish(&stage, root.path(), "NOH", &AtomicBool::new(false)).unwrap();
        assert_eq!(
            std::fs::read(stage.join("interrupted/current/noh.exe")).unwrap(),
            b"broken"
        );
        assert_eq!(
            std::fs::read(stage.join("interrupted/Update.exe")).unwrap(),
            b"old-helper"
        );
        assert_eq!(
            std::fs::read(stage.join("interrupted/NOH.exe")).unwrap(),
            b"old-stub"
        );
        assert_eq!(
            std::fs::read(root.path().join("current/noh.exe")).unwrap(),
            b"new"
        );
    }
    #[test]
    fn refuses_incomplete_stage_and_keeps_inputs_on_rename_failure() {
        let root = tempfile::tempdir().unwrap();
        let stage = staged(root.path());
        std::fs::create_dir(root.path().join("current")).unwrap();
        std::fs::write(root.path().join("current/noh.exe"), b"broken").unwrap();
        std::fs::remove_file(stage.join("Update.exe")).unwrap();
        assert!(publish(&stage, root.path(), "NOH", &AtomicBool::new(false)).is_err());
        assert!(!stage.join("interrupted").exists());
        std::fs::write(stage.join("Update.exe"), b"helper").unwrap();
        let _held = ProtectedFile::open(&root.path().join("current/noh.exe")).unwrap();
        assert!(publish(&stage, root.path(), "NOH", &AtomicBool::new(false)).is_err());
        assert_eq!(
            std::fs::read(root.path().join("current/noh.exe")).unwrap(),
            b"broken"
        );
        assert!(stage.join("current/noh.exe").exists());
    }
    #[test]
    fn accepts_missing_current_and_preserves_stage_on_cancellation() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            guard::running_installed_process(root.path())
                .unwrap()
                .is_none()
        );
        let stage = staged(root.path());
        assert!(matches!(
            publish(&stage, root.path(), "NOH", &AtomicBool::new(true)),
            Err(Error::Cancelled)
        ));
        assert!(!root.path().join("current").exists());
        assert!(stage.join("current/noh.exe").exists());
        let other = tempfile::tempdir().unwrap();
        let stage = staged(other.path());
        publish(&stage, other.path(), "NOH", &AtomicBool::new(false)).unwrap();
        assert!(other.path().join("current/noh.exe").is_file());
    }
    #[test]
    fn interrupted_publication_retains_both_old_and_authenticated_staged_trees() {
        let root = tempfile::tempdir().unwrap();
        let stage = staged(root.path());
        std::fs::create_dir(root.path().join("current")).unwrap();
        std::fs::write(root.path().join("current/noh.exe"), b"broken").unwrap();
        std::fs::write(root.path().join("Update.exe"), b"old-helper").unwrap();
        let held = ProtectedFile::open(&root.path().join("Update.exe")).unwrap();
        assert!(publish(&stage, root.path(), "NOH", &AtomicBool::new(false)).is_err());
        assert!(!root.path().join("current").exists());
        assert_eq!(
            std::fs::read(stage.join("interrupted/current/noh.exe")).unwrap(),
            b"broken"
        );
        assert_eq!(
            std::fs::read(stage.join("current/noh.exe")).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(root.path().join("Update.exe")).unwrap(),
            b"old-helper"
        );
        drop(held);
    }
}

// The controlled caller also prevents creation of new updater actors.
fn refuse_update_owners() -> Result<()> {
    let processes = unsafe { snapshot::CreateToolhelp32Snapshot(snapshot::TH32CS_SNAPPROCESS, 0) };
    if processes == foundation::INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut entry: snapshot::PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut valid = unsafe { snapshot::Process32FirstW(processes, &mut entry) };
        while valid != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|u| *u == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase();
            if [
                "noh-update-guard.exe",
                "update.exe",
                "update_x64.exe",
                "update-measure.exe",
            ]
            .contains(&name.as_str())
            {
                let handle = unsafe {
                    threading::OpenProcess(
                        threading::PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        entry.th32ProcessID,
                    )
                };
                if handle.is_null() {
                    return Err(std::io::Error::other(format!(
                        "Cannot inspect possible updater owner: pid={} name={name}",
                        entry.th32ProcessID
                    ))
                    .into());
                }
                let mut status = foundation::STILL_ACTIVE as u32;
                let read = unsafe { threading::GetExitCodeProcess(handle, &mut status) };
                unsafe { foundation::CloseHandle(handle) };
                if read == 0 || status == foundation::STILL_ACTIVE as u32 {
                    return Err(std::io::Error::other(format!(
                        "Possible updater owner remains active: pid={} name={name}",
                        entry.th32ProcessID
                    ))
                    .into());
                }
            }
            valid = unsafe { snapshot::Process32NextW(processes, &mut entry) };
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(foundation::ERROR_NO_MORE_FILES as i32) {
            return Err(error.into());
        }
        Ok(())
    })();
    unsafe { foundation::CloseHandle(processes) };
    result
}
