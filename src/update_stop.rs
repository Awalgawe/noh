//! The request an updating NOH leaves for the MCP servers of its installed copy.
//!
//! The installer cannot replace a running `noh-mcp.exe`, and an MCP client
//! keeps one alive for as long as its session lasts. When NOH is about to run
//! the installer, it writes this request in the updates folder; every server
//! of that copy then refuses new jobs and exits as soon as it has none (see
//! `mcp::serve_io`). A server is never stopped in the middle of a job.
//!
//! The request names the installation folder, so a portable copy's servers are
//! left alone, and it lapses [`LIFETIME`] seconds after NOH last renewed it, so
//! a NOH that crashed or gave up never leaves servers refusing work.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Name of the request inside the updates folder.
pub const FILE: &str = "stop-servers.json";
/// Seconds a request stays in force after NOH last wrote it.
pub const LIFETIME: u64 = 30;

/// The folder holding update downloads, `state.json` and the request.
pub fn folder() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Caches"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")));
    base.map(|p| p.join("noh").join("updates"))
}

/// "Stop the servers running from `installation`, until `until`."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRequest {
    pub installation: PathBuf,
    /// Seconds since the Unix epoch.
    pub until: u64,
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl StopRequest {
    /// Writes (or renews) the request through a temporary file and a rename,
    /// so a server never reads half of it.
    pub fn write(folder: &Path, installation: &Path, now: u64) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        let request = StopRequest {
            installation: installation.to_path_buf(),
            until: now.saturating_add(LIFETIME),
        };
        let temporary = folder.join(format!("{FILE}.partial"));
        std::fs::write(
            &temporary,
            serde_json::to_string(&request).expect("StopRequest serializes"),
        )?;
        std::fs::rename(temporary, folder.join(FILE))
    }
    /// Withdraws the request: the servers accept jobs again.
    pub fn clear(folder: &Path) {
        let _ = std::fs::remove_file(folder.join(FILE));
    }
    pub fn read(folder: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(folder.join(FILE)).ok()?).ok()
    }
    /// In force at `now` for a program running from `program_folder`. An
    /// `until` further ahead than one lifetime (a clock set back) is not.
    pub fn applies(&self, program_folder: &Path, now: u64) -> bool {
        now <= self.until
            && self.until <= now.saturating_add(LIFETIME)
            && same_folder(&self.installation, program_folder)
    }
}

/// Whether a request is in force for the copy this process runs from.
pub fn requested_here() -> bool {
    let Some(folder) = folder() else {
        return false;
    };
    let Some(request) = StopRequest::read(&folder) else {
        return false;
    };
    let Some(program) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return false;
    };
    request.applies(&program, now())
}

/// Folders compared the way the system resolves them: canonical when they
/// exist, trailing separators dropped, case ignored on Windows.
fn same_folder(a: &Path, b: &Path) -> bool {
    fn key(path: &Path) -> String {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let text = path.to_string_lossy();
        let text = text.trim_end_matches(['\\', '/']);
        if cfg!(windows) {
            text.replace('/', "\\").to_lowercase()
        } else {
            text.to_owned()
        }
    }
    key(a) == key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_applies_to_its_own_folder_until_it_lapses() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("NOH");
        let other = root.path().join("NOH2");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let updates = root.path().join("updates");
        assert!(StopRequest::read(&updates).is_none());
        StopRequest::write(&updates, &installed, 1_000).unwrap();
        let request = StopRequest::read(&updates).unwrap();
        assert_eq!(request.until, 1_000 + LIFETIME);
        assert!(request.applies(&installed, 1_000));
        assert!(request.applies(&installed.join("."), 1_000 + LIFETIME));
        // Expired, a portable copy beside it, a clock set back.
        assert!(!request.applies(&installed, 1_001 + LIFETIME));
        assert!(!request.applies(&other, 1_000));
        assert!(!request.applies(&installed, 1_000 - 1));
        // Renewing moves the deadline; clearing withdraws it.
        StopRequest::write(&updates, &installed, 1_020).unwrap();
        assert!(
            StopRequest::read(&updates)
                .unwrap()
                .applies(&installed, 1_040)
        );
        StopRequest::clear(&updates);
        assert!(StopRequest::read(&updates).is_none());
    }

    #[test]
    fn an_unreadable_request_is_none() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), "{").unwrap();
        assert!(StopRequest::read(root.path()).is_none());
    }
}
