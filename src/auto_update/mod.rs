//! Automatic updates of an installed Windows copy, the way VS Code does it.
//!
//! NOH asks GitHub for the latest promoted release, downloads the release's
//! Minimal installer and every optional component archive whose installed files
//! differ, checks each size and SHA-256, and runs the installer silently when the
//! user restarts or closes NOH. The installer is the published Inno Setup
//! wizard: the silent upgrade it performs is the one the release CI tests
//! (`tools/installer/public-ci.py`, `test_upgrade`). A portable copy, Linux and
//! macOS are only told that a version exists.
//!
//! This module is the pure part: it reads text and files and decides. Network,
//! processes and the registry live in the submodules.

use serde::{Deserialize, Serialize};
use sha2_11::{Digest, Sha256};
use std::{
    cmp::Ordering,
    fmt,
    io::Read,
    path::{Path, PathBuf},
};

/// The repository whose promoted release is the update feed.
pub const REPOSITORY: &str = "Awalgawe/noh";
/// GitHub's latest promoted release: never a draft, never a pre-release.
pub const FEED: &str = "https://api.github.com/repos/Awalgawe/noh/releases/latest";
/// Every downloaded file must come from this prefix, followed by `v<version>/<name>`.
pub const DOWNLOADS: &str = "https://github.com/Awalgawe/noh/releases/download/";
/// The release's component catalog, published beside the installers.
pub const CATALOG: &str = "COMPONENTS.json";
/// Upper bound for the release description and catalog read into memory.
pub const MAX_TEXT: u64 = 16 * 1024 * 1024;
/// Upper bound for one downloaded file (the speech archive is 1.15 GB in 0.2.0).
pub const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, Error>;

/// Why an update step stopped. Each kind has its own message in the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// GitHub could not be reached or answered with an error.
    Network(String),
    /// A file differs from what the release declares (size, digest, address).
    Integrity(String),
    /// A file could not be written or read on this computer.
    Storage(String),
    /// Nothing usable for this copy: no curl, no matching asset, unknown profile.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network(s) => write!(f, "network: {s}"),
            Error::Integrity(s) => write!(f, "integrity: {s}"),
            Error::Storage(s) => write!(f, "storage: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

/// A release version: exactly `major.minor.patch`, digits only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `1.2.3`; anything else (pre-release text, build metadata, spaces) is refused.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let mut next = || -> Option<u64> {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if part.len() > 1 && part.starts_with('0') {
                return None;
            }
            part.parse().ok()
        };
        let version = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }
    /// A release tag: `v1.2.3`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::parse(tag.strip_prefix('v')?)
    }
    /// The version of this build.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("The package version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// One file the update needs, with what the release declares about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The newer release this copy can update to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page, for the "see the release" link.
    pub page: String,
    /// The Minimal installer: silent upgrades keep the installed profile.
    pub setup: Asset,
    pub catalog: Asset,
}

/// Name of the installer NOH runs to update itself.
pub fn setup_name(version: Version) -> String {
    format!("NOH-{version}-windows-x64-Minimal-Setup.exe")
}

/// The release page the user may open; built, never taken from the answer.
pub fn release_page(version: Version) -> String {
    format!("https://github.com/{REPOSITORY}/releases/tag/v{version}")
}

/// Where `name` of release `version` must be downloaded from.
pub fn download_url(base: &str, version: Version, name: &str) -> String {
    format!("{base}v{version}/{name}")
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    browser_download_url: String,
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Reads GitHub's answer for the latest release. `Ok(None)`: nothing newer than
/// `current`. `base` is [`DOWNLOADS`] except in tests.
pub fn parse_release(json: &str, current: Version, base: &str) -> Result<Option<Release>> {
    let release: GitHubRelease = serde_json::from_str(json)
        .map_err(|e| Error::Network(format!("unreadable release description: {e}")))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::from_tag(&release.tag_name) else {
        return Ok(None);
    };
    if version <= current {
        return Ok(None);
    }
    let pick = |name: &str| -> Result<Asset> {
        let mut found = release.assets.iter().filter(|a| a.name == name);
        let asset = found
            .next()
            .ok_or_else(|| Error::Unsupported(format!("{name} is not in release v{version}")))?;
        if found.next().is_some() {
            return Err(Error::Integrity(format!(
                "{name} appears twice in release v{version}"
            )));
        }
        let url = download_url(base, version, name);
        if asset.browser_download_url != url {
            return Err(Error::Integrity(format!(
                "{name} is not served from {base}v{version}/"
            )));
        }
        let sha256 = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|d| is_sha256(d))
            .ok_or_else(|| Error::Integrity(format!("{name} has no SHA-256 digest")))?;
        if asset.size == 0 || asset.size > MAX_FILE {
            return Err(Error::Integrity(format!("{name} has an implausible size")));
        }
        Ok(Asset {
            name: name.to_owned(),
            url,
            size: asset.size,
            sha256: sha256.to_owned(),
        })
    };
    Ok(Some(Release {
        version,
        page: release_page(version),
        setup: pick(&setup_name(version))?,
        catalog: pick(CATALOG)?,
    }))
}

/// The copy this process runs from, when Inno Setup installed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installation {
    pub folder: PathBuf,
    pub profile: Profile,
}

/// The installed copy, or `None` for a portable copy, Linux and macOS: those
/// are only told that a version exists.
pub fn installation() -> Option<Installation> {
    #[cfg(target_os = "windows")]
    return windows::installation();
    #[cfg(not(target_os = "windows"))]
    None
}

/// The content profile Inno Setup recorded for the installed copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Minimal,
    Standard,
    Complete,
}

impl Profile {
    /// The value stored by the installer; an absent value means `complete`, as
    /// in `tools/installer/public-profile.iss`. Anything else is refused there,
    /// so NOH refuses it too.
    pub fn from_installer(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("complete") {
            "minimal" => Some(Profile::Minimal),
            "standard" => Some(Profile::Standard),
            "complete" => Some(Profile::Complete),
            _ => None,
        }
    }
    /// The component groups the installer keeps for this profile.
    pub fn groups(self) -> &'static [&'static str] {
        match self {
            Profile::Minimal => &[],
            Profile::Standard => &["media"],
            Profile::Complete => &["media", "speech"],
        }
    }
}

#[derive(Deserialize)]
struct Catalog {
    schema_version: u32,
    version: String,
    target: String,
    groups: std::collections::BTreeMap<String, Group>,
}

#[derive(Deserialize)]
struct Group {
    name: String,
    size: u64,
    sha256: String,
    files: std::collections::BTreeMap<String, Member>,
    /// Everything the archive holds: `files` plus licence notices.
    archive_files: std::collections::BTreeMap<String, Member>,
}

#[derive(Deserialize, PartialEq, Eq)]
struct Member {
    sha256: String,
    size: u64,
}

/// A component archive the update must place beside the installer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub archive: Asset,
    /// Bytes of everything the archive extracts, licence notices included: the
    /// sum the installer itself uses (`ComponentInstalledBytes`, generated by
    /// `tools/public_components.py` from `archive_files`).
    pub installed_bytes: u64,
}

/// Free space the installer itself demands on the volume of its temporary folder
/// before it extracts anything: archive bytes plus installed bytes of every group
/// it installs, plus 64 MiB (`PrepareContent` in
/// `tools/installer/public-content.iss`). It counts an archive even when it sits
/// beside the installer, so NOH counts it too.
pub fn installer_space(components: &[Component]) -> u64 {
    components
        .iter()
        .map(|c| c.archive.size.saturating_add(c.installed_bytes))
        .fold(64 * 1024 * 1024, u64::saturating_add)
}

/// The component archives the installer would otherwise download itself, NOH
/// closed and no window shown: one per group of `profile` whose installed files
/// are missing or differ. This is the rule of `GroupReady` in
/// `tools/installer/public-content.iss`. `stop` aborts the hashing.
pub fn components_to_download(
    catalog_json: &str,
    release: Version,
    installed: &Path,
    profile: Profile,
    base: &str,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<Component>> {
    let catalog: Catalog = serde_json::from_str(catalog_json)
        .map_err(|e| Error::Integrity(format!("unreadable {CATALOG}: {e}")))?;
    if catalog.schema_version != 1 {
        return Err(Error::Integrity(format!(
            "{CATALOG} schema {} is not understood",
            catalog.schema_version
        )));
    }
    if Version::parse(&catalog.version) != Some(release) || catalog.target != "windows-x64" {
        return Err(Error::Integrity(format!(
            "{CATALOG} describes {} {}, not v{release} windows-x64",
            catalog.version, catalog.target
        )));
    }
    let mut needed = Vec::new();
    for &group in profile.groups() {
        let spec = catalog
            .groups
            .get(group)
            .ok_or_else(|| Error::Integrity(format!("{CATALOG} has no {group} group")))?;
        let expected = format!("NOH-windows-x64-{group}-");
        if !spec.name.starts_with(&expected)
            || !spec.name.ends_with(".zip")
            || spec.name.contains(['/', '\\'])
            || !is_sha256(&spec.sha256)
            || spec.size == 0
            || spec.size > MAX_FILE
            || spec.files.is_empty()
            || spec
                .files
                .iter()
                .any(|(n, m)| spec.archive_files.get(n) != Some(m))
        {
            return Err(Error::Integrity(format!(
                "{CATALOG} describes {group} oddly"
            )));
        }
        let installed_bytes = spec
            .archive_files
            .values()
            .try_fold(0u64, |sum, m| sum.checked_add(m.size))
            .filter(|sum| *sum <= 4 * MAX_FILE)
            .ok_or_else(|| Error::Integrity(format!("{CATALOG} sizes of {group} overflow")))?;
        if !group_installed(installed, &spec.files, stop)? {
            needed.push(Component {
                archive: Asset {
                    name: spec.name.clone(),
                    url: download_url(base, release, &spec.name),
                    size: spec.size,
                    sha256: spec.sha256.clone(),
                },
                installed_bytes,
            });
        }
    }
    Ok(needed)
}

fn group_installed(
    installed: &Path,
    files: &std::collections::BTreeMap<String, Member>,
    stop: &dyn Fn() -> bool,
) -> Result<bool> {
    // Sizes first: a different size settles it without reading a byte.
    let mut paths = Vec::with_capacity(files.len());
    for (name, member) in files {
        let path = member_path(installed, name)?;
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() == member.size => paths.push((path, member)),
            _ => return Ok(false),
        }
    }
    for (path, member) in paths {
        if sha256_file(&path, stop)? != member.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `installed` joined with a catalog path, which must stay inside it.
fn member_path(installed: &Path, name: &str) -> Result<PathBuf> {
    let parts: Vec<&str> = name.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(['\\', ':']))
    {
        return Err(Error::Integrity(format!(
            "{CATALOG} names an unsafe path: {name}"
        )));
    }
    Ok(parts
        .iter()
        .fold(installed.to_path_buf(), |path, p| path.join(p)))
}

/// SHA-256 of a whole file, lowercase hexadecimal. `stop` aborts between blocks.
pub fn sha256_file(path: &Path, stop: &dyn Fn() -> bool) -> Result<String> {
    let file = std::fs::File::open(path)?;
    sha256_reader(file, stop)
}

/// SHA-256 of everything `reader` yields.
pub fn sha256_reader(mut reader: impl Read, stop: &dyn Fn() -> bool) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if stop() {
            return Err(Error::Storage("cancelled".into()));
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The user's update choices, saved beside the language preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Look for a new version at start and every 24 hours.
    pub check: bool,
    /// Install a downloaded update when NOH closes normally.
    pub install_on_exit: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            check: true,
            install_on_exit: true,
        }
    }
}

impl Preferences {
    pub fn path() -> Option<PathBuf> {
        Some(crate::i18n::preferences_path()?.with_file_name("updates.json"))
    }
    /// Missing or unreadable file: the defaults.
    pub fn read(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            serde_json::to_string(&self).expect("Preferences serialize"),
        )
    }
}

/// What the previous runs left: when GitHub was last asked, and which
/// installation was launched before NOH closed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Seconds since the Unix epoch.
    pub last_check: u64,
    pub attempt: Option<Attempt>,
    /// Why the installer did not start when NOH last closed, shown once at the
    /// next start. Text for the user only; nothing acts on it.
    pub not_started: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    /// The version whose installer was launched. Its files are in
    /// [`version_folder`]; no path is stored, so a damaged state file can never
    /// name a folder to delete.
    pub version: String,
}

/// What the start after an installation attempt tells the user. The files of
/// `version` are in [`version_folder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// This build is the version that was installed: its files can go.
    Updated(Version),
    /// The installation did not replace this build; its files are kept.
    Failed(Version),
    /// A newer version was installed by hand: forget the attempt and its files.
    Superseded(Version),
}

/// The download folder of `version` inside the updates folder `root`: only
/// ever a direct child named as a plain version.
pub fn version_folder(root: &Path, version: Version) -> PathBuf {
    root.join(version.to_string())
}

impl State {
    /// The folder holding downloads and `state.json`.
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
    pub fn read(folder: &Path) -> Self {
        std::fs::read_to_string(folder.join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// Written through a temporary file and a rename, so a crash leaves the old one.
    pub fn save(&self, folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        let temporary = folder.join("state.json.partial");
        std::fs::write(
            &temporary,
            serde_json::to_string(self).expect("State serializes"),
        )?;
        std::fs::rename(temporary, folder.join("state.json"))
    }
    /// A check is due when none was ever made or the last one is a day old.
    pub fn check_due(&self, now: u64) -> bool {
        self.last_check == 0
            || now.saturating_sub(self.last_check) >= 24 * 60 * 60
            || now < self.last_check
    }
    /// The verdict on the last installation attempt, if there was one.
    pub fn outcome(&self, current: Version) -> Option<Outcome> {
        let version = Version::parse(&self.attempt.as_ref()?.version)?;
        Some(match version.cmp(&current) {
            Ordering::Equal => Outcome::Updated(version),
            Ordering::Greater => Outcome::Failed(version),
            Ordering::Less => Outcome::Superseded(version),
        })
    }
}

/// The installer's command line for a silent update of the copy in `installed`.
/// `/NOCLOSEAPPLICATIONS`: fail rather than close a running `noh-cli` or
/// `noh-mcp`. `/WAITPID`: wait until this NOH has exited. `/RELAUNCH`: start NOH
/// again at the end, only when the user asked for a restart (see
/// `tools/installer/public.iss`): closing NOH must not reopen it.
pub fn setup_arguments(installed: &Path, log: &Path, pid: u32, relaunch: bool) -> Vec<String> {
    // The registry's InstallLocation ends with a separator. Before a closing
    // quote, Windows argument quoting doubles a trailing backslash, and Inno's
    // parser is not known to undo it: never pass one.
    let folder = installed.to_string_lossy();
    let folder = folder.trim_end_matches(['\\', '/']);
    let mut arguments = vec![
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/CURRENTUSER".into(),
        "/NOCLOSEAPPLICATIONS".into(),
        "/PROFILE=minimal".into(),
        format!("/DIR={folder}"),
        format!("/LOG={}", log.display()),
        format!("/WAITPID={pid}"),
    ];
    if relaunch {
        arguments.push("/RELAUNCH".into());
    }
    arguments
}

/// Why the installer was not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A program of the installed folder runs (another NOH window, an export,
    /// the MCP server, FFmpeg, whisper-cli): installing now would fail or cut it
    /// off. What runs, for the message.
    Busy(Vec<String>),
    Failed(Error),
}

/// Starts the silent installation of `ready` and records the attempt; the
/// caller then closes NOH. `relaunch`: the user asked for a restart.
#[cfg(target_os = "windows")]
pub fn apply(
    ready: &worker::Ready,
    root: &Path,
    relaunch: bool,
) -> std::result::Result<(), Refusal> {
    let own = std::process::id();
    let busy = windows::running(&ready.installation.folder, own).map_err(Refusal::Failed)?;
    if !busy.is_empty() {
        return Err(Refusal::Busy(busy.iter().map(|r| r.to_string()).collect()));
    }
    worker::require_space(&std::env::temp_dir(), installer_space(&ready.components))
        .map_err(Refusal::Failed)?;
    let log = version_folder(root, ready.version).join("install.log");
    let arguments = setup_arguments(&ready.installation.folder, &log, own, relaunch);
    let mut state = State::read(root);
    state.attempt = Some(Attempt {
        version: ready.version.to_string(),
    });
    state.save(root).map_err(|e| Refusal::Failed(e.into()))?;
    if let Err(error) = windows::launch(&ready.setup, &ready.setup_sha256, &arguments) {
        state.attempt = None;
        let _ = state.save(root);
        return Err(Refusal::Failed(error));
    }
    Ok(())
}

pub mod fetch;
#[cfg(target_os = "windows")]
pub mod windows;
pub mod worker;

#[cfg(test)]
mod tests;
