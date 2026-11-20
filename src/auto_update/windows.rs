//! What only Windows can answer: is this copy the one Inno Setup installed,
//! with which profile; does anything else run from its folder; how much space
//! is free; is the session ending; and the launch of the installer itself.
//! Raw `extern "system"` declarations, as `src/i18n.rs` already does: no
//! dependency changes.

use super::{Error, Installation, Process, Profile, Result};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

/// The uninstall key the public installer writes (`AppId=NOH.Public`).
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NOH.Public_is1";

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
}

type Handle = *mut std::ffi::c_void;

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    // Same signatures as src/process.rs and src/jobs_tests.rs.
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetDiskFreeSpaceExW(
        path: *const u16,
        available: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// A string value of the installer's uninstall key, or `None` when absent.
fn installer_value(name: &str) -> Option<String> {
    const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const RRF_SUBKEY_WOW6464KEY: u32 = 0x0001_0000;
    let key = wide(OsStr::new(UNINSTALL_KEY));
    let value = wide(OsStr::new(name));
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let length = (size as usize / 2).min(buffer.len());
    let text = &buffer[..length];
    let text = text.split(|c| *c == 0).next().unwrap_or(&[]);
    Some(String::from_utf16_lossy(text))
}

/// Paths compared the way Windows resolves them: canonical when the file
/// exists, trailing separators dropped, case ignored.
pub fn same_path(a: &Path, b: &Path) -> bool {
    normalized(a) == normalized(b)
}

fn normalized(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && !rest.starts_with("UNC\\")
    {
        text = rest.to_owned();
    }
    while text.ends_with('\\') && text.len() > 3 {
        text.pop();
    }
    text.to_lowercase()
}

/// `Some` when the installer's `InstallLocation` is this executable's folder and
/// its recorded profile is one the installer accepts.
pub fn installation() -> Option<Installation> {
    let exe = std::env::current_exe().ok()?;
    let folder = exe.parent()?.to_path_buf();
    let location = installer_value("InstallLocation")?;
    if location.trim().is_empty() || !same_path(Path::new(&location), &folder) {
        return None;
    }
    let profile =
        Profile::from_installer(installer_value("Inno Setup CodeFile: ContentProfile").as_deref())?;
    Some(Installation { folder, profile })
}

/// `path` lies inside `folder` (not merely beside it: `NOH2` is not in `NOH`).
pub fn inside(folder: &Path, path: &Path) -> bool {
    let folder = normalized(folder);
    let path = normalized(path);
    path.len() > folder.len() + 1
        && path.starts_with(&folder)
        && path.as_bytes()[folder.len()] == b'\\'
}

/// Lowercase names of every executable under `folder`: the programs that hold
/// its files while they run (NOH, its CLI and MCP server, FFmpeg, whisper-cli,
/// the component installer, the uninstaller).
fn executable_names(folder: &Path) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if name.ends_with(".exe") {
                names.insert(name);
            }
        }
    }
    names
}

/// Processes other than `own` that run a program from inside `folder`. One
/// named like a program of the folder whose location Windows will not give is
/// counted too, so the check fails closed.
pub fn running(folder: &Path, own: u32) -> Result<Vec<Process>> {
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let names = executable_names(folder);
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot as isize == -1 || snapshot.is_null() {
        return Err(Error::Unsupported(format!(
            "process list unavailable: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut found = Vec::new();
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry
            .exe
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.exe.len());
        let name = String::from_utf16_lossy(&entry.exe[..length]).to_lowercase();
        if entry.pid != own && names.contains(&name) {
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.pid) };
            let mut image = None;
            if !process.is_null() {
                let mut buffer = vec![0u16; 32768];
                let mut size = buffer.len() as u32;
                let ok = unsafe {
                    QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size)
                };
                unsafe { CloseHandle(process) };
                if ok != 0 {
                    image = Some(PathBuf::from(OsString::from_wide(&buffer[..size as usize])));
                }
            }
            if image.is_none_or(|image| inside(folder, &image)) {
                found.push(Process {
                    pid: entry.pid,
                    parent: entry.parent,
                    name,
                });
            }
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Ok(found)
}

/// Bytes available to this user on the volume holding `path`.
pub fn free_space(path: &Path) -> Result<u64> {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    let target = wide(probe.as_os_str());
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    if unsafe { GetDiskFreeSpaceExW(target.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(Error::Storage(format!(
            "free space unknown on {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(available)
}

/// Windows is ending the session (log off, restart, shut down): no installer then.
pub fn session_ending() -> bool {
    const SM_SHUTTINGDOWN: i32 = 0x2000;
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

/// Opens the installer so nobody can write or delete it, checks its digest
/// through that handle, and keeps the handle until the installer has started.
/// After that, Windows refuses writes to a running executable but not a rename;
/// the updates folder belongs to the same user as the installed copy.
pub fn launch(setup: &Path, sha256: &str, arguments: &[String]) -> Result<()> {
    use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let held: File = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(setup)?;
    if super::sha256_reader(&held, &|| false)? != sha256 {
        return Err(Error::Integrity(
            "the installer changed after its download".into(),
        ));
    }
    let start = |flags: u32| {
        std::process::Command::new(setup)
            .args(arguments)
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // Outlive NOH even if a job object holds it; a job that forbids breaking away
    // refuses the flag, and the installer then starts inside it.
    let started = start(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| start(base));
    drop(held);
    started
        .map(|_| ())
        .map_err(|e| Error::Storage(format!("the installer did not start: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_compare_like_windows() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path();
        let upper = PathBuf::from(path.to_string_lossy().to_uppercase());
        let trailing = PathBuf::from(format!("{}\\", path.display()));
        assert!(same_path(path, &upper));
        assert!(same_path(path, &trailing));
        assert!(same_path(
            Path::new(r"C:\Missing\NOH\"),
            Path::new(r"c:\missing\noh")
        ));
        assert!(!same_path(path, &path.join("bin")));
    }

    fn ping() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\PING.EXE")
    }

    #[test]
    fn inside_means_below_the_folder() {
        let folder = Path::new(r"C:\Programs\NOH");
        assert!(inside(folder, Path::new(r"C:\Programs\NOH\noh.exe")));
        assert!(inside(
            folder,
            Path::new(r"c:\programs\noh\bin\speech\whisper-cli.exe")
        ));
        assert!(inside(
            Path::new(r"C:\Programs\NOH\"),
            Path::new(r"C:\Programs\NOH\bin\ffmpeg.exe")
        ));
        assert!(!inside(folder, Path::new(r"C:\Programs\NOH2\noh.exe")));
        assert!(!inside(folder, Path::new(r"C:\Programs\NOH")));
        assert!(!inside(folder, Path::new(r"C:\Programs\noh.exe")));
    }

    /// Every program of the folder counts — NOH, its CLI, FFmpeg, whisper-cli —
    /// except the caller; the same names elsewhere do not.
    #[test]
    fn programs_running_from_the_folder_are_found() {
        let folder = tempfile::tempdir().unwrap();
        let copies = [
            folder.path().join("noh.exe"),
            folder.path().join(r"bin\noh-cli.exe"),
            folder.path().join(r"bin\ffmpeg.exe"),
            folder.path().join(r"bin\speech\whisper-cli.exe"),
        ];
        std::fs::create_dir_all(folder.path().join(r"bin\speech")).unwrap();
        // Copies of a harmless system program, waiting a few seconds.
        for copy in &copies {
            std::fs::copy(ping(), copy).unwrap();
        }
        let mut children: Vec<_> = copies
            .iter()
            .map(|p| {
                std::process::Command::new(p)
                    .args(["-n", "30", "127.0.0.1"])
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let found = running(folder.path(), std::process::id());
        let without_first = running(folder.path(), children[0].id());
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::copy(ping(), elsewhere.path().join("noh.exe")).unwrap();
        let other = running(elsewhere.path(), 0);
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let found = found.unwrap();
        assert_eq!(found.len(), copies.len(), "{found:?}");
        for (copy, child) in copies.iter().zip(&children) {
            let name = copy.file_name().unwrap().to_string_lossy().to_lowercase();
            assert!(
                found.contains(&Process {
                    pid: child.id(),
                    parent: std::process::id(),
                    name,
                }),
                "{copy:?} in {found:?}"
            );
        }
        // The caller's own process is never reported.
        assert_eq!(without_first.unwrap().len(), copies.len() - 1);
        // The same names running from another folder do not count.
        assert!(other.unwrap().is_empty());
    }

    #[test]
    fn the_installer_is_refused_if_it_changed() {
        let folder = tempfile::tempdir().unwrap();
        let setup = folder.path().join("Setup.exe");
        std::fs::write(&setup, b"not the installer").unwrap();
        assert!(matches!(
            launch(&setup, &"0".repeat(64), &[]),
            Err(Error::Integrity(_))
        ));
    }

    /// The installer starts while NOH holds it open without write sharing.
    #[test]
    fn the_held_installer_starts() {
        let folder = tempfile::tempdir().unwrap();
        let setup = folder.path().join("Setup.exe");
        std::fs::copy(ping(), &setup).unwrap();
        let digest = super::super::sha256_file(&setup, &|| false).unwrap();
        let arguments = ["-n".to_owned(), "4".to_owned(), "127.0.0.1".to_owned()];
        launch(&setup, &digest, &arguments).unwrap();
        // The child handle is gone: find it by its program.
        let mut seen = false;
        for _ in 0..40 {
            if !running(folder.path(), std::process::id())
                .unwrap()
                .is_empty()
            {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(seen, "the installer copy never appeared");
        // Let it end before the folder goes.
        for _ in 0..200 {
            if running(folder.path(), std::process::id())
                .unwrap()
                .is_empty()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn a_held_file_cannot_be_written() {
        use std::os::windows::fs::OpenOptionsExt;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("held.bin");
        std::fs::write(&path, b"x").unwrap();
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&path)
            .unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
    }

    #[test]
    fn free_space_is_known_for_a_missing_subfolder() {
        let folder = tempfile::tempdir().unwrap();
        assert!(free_space(&folder.path().join("not/yet")).unwrap() > 0);
    }
}
