//! The background part: one thread at a time asks GitHub, downloads and checks,
//! and publishes its progress in a shared [`Phase`] the interface reads each
//! frame. Nothing here touches the interface or blocks it.

use super::{
    Asset, Component, Error, Installation, Release, Result, State, Version, components_to_download,
    fetch, installation, installer_space, parse_release, version_folder,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

/// Where the update stands, as the interface shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    /// A newer version exists but this copy cannot install it itself (portable
    /// copy, Linux, unknown profile): the interface shows the release link.
    Available(Release),
    /// Comparing the installed components with the release's catalog.
    Preparing(Version),
    Downloading {
        version: Version,
        done: u64,
        total: u64,
    },
    /// Everything is downloaded and checked; the installer can run.
    Ready(Ready),
    Failed(Error),
}

/// A verified installer and the components beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    pub version: Version,
    pub page: String,
    pub setup: PathBuf,
    pub setup_sha256: String,
    pub components: Vec<Component>,
    pub installation: Installation,
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Owns the background thread; dropping it stops the thread and waits for it.
pub struct Worker {
    phase: Arc<Mutex<Phase>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            phase: Arc::new(Mutex::new(Phase::Idle)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

impl Worker {
    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn busy(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
    /// Starts a check (and, for an installed copy, the download) unless one runs.
    /// `wake` asks the interface for a new frame.
    pub fn start(&mut self, root: PathBuf, wake: impl Fn() + Send + 'static) {
        if self.busy() {
            return;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.stop.store(false, Ordering::SeqCst);
        // An update already downloaded and checked is not prepared again while
        // the user postpones the restart: only a newer release restarts the work.
        let known = match self.phase() {
            Phase::Ready(ready) => Some(ready),
            _ => None,
        };
        let phase = self.phase.clone();
        let stop = self.stop.clone();
        let set = move |value: Phase| {
            *phase.lock().unwrap_or_else(|e| e.into_inner()) = value;
            wake();
        };
        set(Phase::Checking);
        self.thread = Some(std::thread::spawn(move || {
            let stopped = || stop.load(Ordering::SeqCst);
            let result = run(&root, known, &set, &stopped);
            if !stopped() {
                set(result.unwrap_or_else(Phase::Failed));
            }
        }));
    }
    /// Puts a phase back, for "Réessayer" after a refused installation.
    pub fn show(&self, value: Phase) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One check: ask, compare, and for an installed copy download and verify.
fn run(
    root: &Path,
    known: Option<Ready>,
    set: &dyn Fn(Phase),
    stop: &dyn Fn() -> bool,
) -> Result<Phase> {
    let Some(source) = fetch::Source::configured() else {
        return Err(Error::Unsupported(format!(
            "{} must be a file:/// folder",
            fetch::TRIAL
        )));
    };
    let Some(curl) = fetch::curl() else {
        return Err(Error::Unsupported("no system curl".into()));
    };
    let mut state = State::read(root);
    let description = fetch::release_description(&curl, &source, stop)?;
    state.last_check = now();
    let _ = state.save(root);
    let Some(release) = parse_release(&description, Version::current(), &source.base)? else {
        return Ok(Phase::UpToDate);
    };
    if let Some(ready) = known.filter(|r| r.version == release.version) {
        return Ok(Phase::Ready(ready));
    }
    let Some(installation) = installation() else {
        return Ok(Phase::Available(release));
    };
    set(Phase::Preparing(release.version));
    let folder = version_folder(root, release.version);
    remove_other_versions(root, release.version);
    let catalog = fetch::release_text(&curl, &source, &release.catalog, stop)?;
    let components = components_to_download(
        &catalog,
        release.version,
        &installation.folder,
        installation.profile,
        &source.base,
        stop,
    )?;
    let files: Vec<&Asset> = std::iter::once(&release.setup)
        .chain(components.iter().map(|c| &c.archive))
        .collect();
    let missing: u64 = files
        .iter()
        .filter(|a| std::fs::metadata(folder.join(&a.name)).map_or(true, |m| m.len() != a.size))
        .map(|a| a.size)
        .sum();
    require_space(&folder, missing)?;
    let total: u64 = files.iter().map(|a| a.size).sum();
    let mut finished = 0u64;
    let mut setup = PathBuf::new();
    for asset in &files {
        let report = |done: u64| {
            set(Phase::Downloading {
                version: release.version,
                done: finished + done.min(asset.size),
                total,
            })
        };
        let path = fetch::download(&curl, &source, asset, &folder, &report, stop)?;
        if asset.name == release.setup.name {
            setup = path;
        }
        finished += asset.size;
    }
    require_space(&std::env::temp_dir(), installer_space(&components))?;
    Ok(Phase::Ready(Ready {
        version: release.version,
        page: release.page,
        setup,
        setup_sha256: release.setup.sha256.clone(),
        components,
        installation,
    }))
}

/// Refuses when the volume of `path` has less than `bytes` free for this user.
pub fn require_space(path: &Path, bytes: u64) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let free = super::windows::free_space(path)?;
        if free < bytes {
            return Err(Error::Storage(format!(
                "{} MB needed on the volume of {}, {} MB free",
                bytes.div_ceil(1_000_000),
                path.display(),
                free / 1_000_000
            )));
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (path, bytes);
    Ok(())
}

/// Download folders of other versions: only direct children named as a plain
/// version, never anything else in the updates folder.
fn remove_other_versions(root: &Path, keep: Version) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(version) = name.to_str().and_then(Version::parse) else {
            continue;
        };
        if version != keep && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(version_folder(root, version));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_version_folders_are_cleaned() {
        let root = tempfile::tempdir().unwrap();
        for name in ["0.2.0", "0.3.0", "0.4.0", "notes", "0.3"] {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
        }
        std::fs::write(root.path().join("state.json"), "{}").unwrap();
        remove_other_versions(root.path(), Version(0, 3, 0));
        let mut left: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["0.3", "0.3.0", "notes", "state.json"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn space_is_refused_beyond_the_free_bytes() {
        let root = tempfile::tempdir().unwrap();
        assert!(require_space(root.path(), 1).is_ok());
        assert!(matches!(
            require_space(root.path(), u64::MAX),
            Err(Error::Storage(_))
        ));
    }
}
