//! Authenticated update discovery and staging. Installation is a separate adapter.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::signature;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const MAX_ENVELOPE: usize = 1024 * 1024;
pub const MAX_PAYLOAD: usize = 768 * 1024;
const MAX_PACKAGE: u64 = 2 * 1024 * 1024 * 1024 - 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Unsupported update metadata")]
    Metadata,
    #[error("Update signature is not trusted")]
    Signature,
    #[error("No compatible update package")]
    Target,
    #[error("Update cancelled")]
    Cancelled,
    #[error("Update package size or digest does not match")]
    Integrity,
    #[error("Update network request failed")]
    Network,
    #[error("Update server returned HTTP {0}")]
    Http(u16),
    #[error("Update source rate limited; retry in {0} seconds")]
    RateLimited(u64),
    #[error("Update installation is not qualified on this platform")]
    InstallationUnavailable,
    #[error("Unsafe update URL or redirect")]
    Url,
    #[error("Update filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

pub mod profile;
pub use profile::Profile;
#[cfg(windows)]
pub mod bootstrap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum Architecture {
    #[serde(rename = "x86_64")]
    X64,
    #[serde(rename = "aarch64")]
    Arm64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub os: Os,
    pub arch: Architecture,
}
impl Target {
    /// Qualification is deliberately separate from metadata/download support.
    pub fn installation_qualified(self) -> bool {
        false
    }
    pub fn native() -> Result<Self> {
        let os = match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            "linux" => Os::Linux,
            _ => return Err(Error::Target),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Architecture::X64,
            "aarch64" => Architecture::Arm64,
            _ => return Err(Error::Target),
        };
        Ok(Self { os, arch })
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Full,
    Delta,
    #[serde(rename = "windows-setup")]
    WindowsSetup,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub target: Target,
    pub kind: PackageKind,
    pub base_version: Option<Version>,
    pub file_name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<setup::SetupContract>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Profile::is_complete")]
    pub profile: Profile,
    pub package_id: String,
    pub version: Version,
    pub channel: Channel,
    pub notes: String,
    pub artifacts: Vec<Artifact>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub key_id: String,
    pub payload: String,
    pub signature: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustKey {
    pub id: String,
    pub public_key: [u8; 32],
}

/// Cannot be constructed from unauthenticated JSON by an installation caller.
pub struct VerifiedRelease {
    release: Release,
    envelope: Vec<u8>,
}
impl VerifiedRelease {
    pub fn verify(bytes: &[u8], keys: &[TrustKey], package_id: &str) -> Result<Self> {
        if bytes.len() > MAX_ENVELOPE {
            return Err(Error::Metadata);
        }
        let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| Error::Metadata)?;
        let key = keys
            .iter()
            .find(|key| key.id == envelope.key_id)
            .ok_or(Error::Signature)?;
        let payload = STANDARD
            .decode(envelope.payload)
            .map_err(|_| Error::Metadata)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::Metadata);
        }
        let sig = STANDARD
            .decode(envelope.signature)
            .map_err(|_| Error::Signature)?;
        signature::UnparsedPublicKey::new(&signature::ED25519, key.public_key)
            .verify(&payload, &sig)
            .map_err(|_| Error::Signature)?;
        let release: Release = serde_json::from_slice(&payload).map_err(|_| Error::Metadata)?;
        if release.schema_version == 1 && (bytes.len() > 128 * 1024 || payload.len() > 64 * 1024) {
            return Err(Error::Metadata);
        }
        if !matches!(release.schema_version, 1 | 2 | 3)
            || (release.schema_version < 3 && release.profile != Profile::Complete)
            || (release.schema_version == 3 && release.profile == Profile::Complete)
            || release.package_id != package_id
            || release.artifacts.is_empty()
            || release.artifacts.len() > 32
            || (release.channel == Channel::Stable && !release.version.pre.is_empty())
        {
            return Err(Error::Metadata);
        }
        let mut seen = HashSet::new();
        for artifact in &release.artifacts {
            let key = (
                artifact.target,
                artifact.kind as u8,
                artifact.base_version.clone(),
            );
            if !seen.insert(key)
                || artifact.size == 0
                || artifact.size > MAX_PACKAGE
                || artifact.sha256.len() != 64
                || !artifact
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || artifact.file_name.is_empty()
                || !artifact
                    .file_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || (artifact.kind != PackageKind::WindowsSetup
                    && (!artifact.file_name.ends_with(".nupkg") || artifact.setup.is_some()))
                || (artifact.kind == PackageKind::Full && artifact.base_version.is_some())
                || (artifact.kind == PackageKind::Delta
                    && artifact
                        .base_version
                        .as_ref()
                        .is_none_or(|v| !v.cmp_precedence(&release.version).is_lt()))
            {
                return Err(Error::Metadata);
            }
            match (release.schema_version, artifact.kind) {
                (1, PackageKind::Full | PackageKind::Delta) => {}
                (2 | 3, PackageKind::WindowsSetup) => {
                    setup::validate_artifact(artifact)?;
                }
                _ => return Err(Error::Metadata),
            }
            validate_url(&artifact.url)?;
        }
        Ok(Self {
            release,
            envelope: bytes.to_vec(),
        })
    }
    pub fn release(&self) -> &Release {
        &self.release
    }
    pub fn require_profile(&self, expected: Profile) -> Result<()> {
        if self.release.profile != expected {
            return Err(Error::Target);
        }
        Ok(())
    }
    pub(crate) fn is_setup(&self) -> bool {
        self.release
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == PackageKind::WindowsSetup)
    }
    /// Original signed proof for independent guardian verification.
    pub fn envelope(&self) -> &[u8] {
        &self.envelope
    }
    /// Normal checks never downgrade, laterally replace, or switch channels.
    pub fn select(
        &self,
        target: Target,
        channel: Channel,
        current: &Version,
    ) -> Result<Option<&Artifact>> {
        self.select_kind(target, channel, current, PackageKind::Full)
    }
    /// Explicit opt-in: legacy .nupkg consumers never receive an executable.
    pub fn select_setup(
        &self,
        target: Target,
        channel: Channel,
        current: &Version,
    ) -> Result<Option<&Artifact>> {
        self.select_kind(target, channel, current, PackageKind::WindowsSetup)
    }
    fn select_kind(
        &self,
        target: Target,
        channel: Channel,
        current: &Version,
        kind: PackageKind,
    ) -> Result<Option<&Artifact>> {
        if self.release.channel != channel {
            return Err(Error::Target);
        }
        if !self.release.version.cmp_precedence(current).is_gt() {
            return Ok(None);
        }
        self.release
            .artifacts
            .iter()
            .find(|a| a.target == target && a.kind == kind)
            .map(Some)
            .ok_or(Error::Target)
    }
}

fn validate_url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(|_| Error::Url)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "api.github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
    {
        return Err(Error::Url);
    }
    Ok(url)
}

/// Token is private and intentionally has no Debug implementation.
pub struct GithubTransport {
    client: reqwest::Client,
    token: Option<String>,
    #[cfg(test)]
    test_endpoint: Option<reqwest::Url>,
    #[cfg(test)]
    test_storage_full: bool,
}
impl GithubTransport {
    pub fn new(token: Option<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .user_agent("NOH-updater")
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self {
            client,
            token,
            #[cfg(test)]
            test_endpoint: None,
            #[cfg(test)]
            test_storage_full: false,
        })
    }
    async fn request(&self, value: &str, cancelled: &AtomicBool) -> Result<reqwest::Response> {
        let mut url = validate_url(value)?;
        for _ in 0..6 {
            let request = self.build_request(&url);
            let response = cancellable(request.send(), cancelled).await?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(Error::Url)?;
                url = redirect_url(&url, location)?;
                continue;
            }
            if !response.status().is_success() {
                if let Some(seconds) = retry_delay(response.status().as_u16(), response.headers()) {
                    return Err(Error::RateLimited(seconds));
                }
                return Err(Error::Http(response.status().as_u16()));
            }
            return Ok(response);
        }
        Err(Error::Url)
    }
    fn build_request(&self, url: &reqwest::Url) -> reqwest::RequestBuilder {
        #[cfg(test)]
        let destination = self.test_endpoint.as_ref().unwrap_or(url);
        #[cfg(not(test))]
        let destination = url;
        let request = self
            .client
            .get(destination.clone())
            .header("Accept", "application/octet-stream");
        // Rebuild each redirected request; never forward an Authorization header.
        match token_for(url, self.token.as_deref()) {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
    pub async fn envelope(&self, url: &str, cancelled: &AtomicBool) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut response = self.request(url, cancelled).await?;
        while let Some(chunk) = cancellable(response.chunk(), cancelled).await? {
            if bytes.len() + chunk.len() > MAX_ENVELOPE {
                return Err(Error::Metadata);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn stage(
        &self,
        release: &VerifiedRelease,
        target: Target,
        channel: Channel,
        current: &Version,
        folder: &Path,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        self.stage_kind(
            release,
            target,
            channel,
            current,
            PackageKind::Full,
            folder,
            cancelled,
            progress,
        )
        .await
    }
    pub(crate) async fn stage_kind(
        &self,
        release: &VerifiedRelease,
        target: Target,
        channel: Channel,
        current: &Version,
        kind: PackageKind,
        folder: &Path,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        let artifact = release
            .select_kind(target, channel, current, kind)?
            .ok_or(Error::Target)?;
        self.stage_artifact(artifact, &artifact.url, folder, cancelled, progress)
            .await
    }

    /// A bootstrapper installs an exact signed version, including first install.
    /// An optional private-trial mirror is restricted to the same GitHub repository's
    /// asset API. Its response must still match the signed artifact size and digest.
    #[cfg(windows)]
    pub async fn stage_setup(
        &self,
        release: &VerifiedRelease,
        profile: Profile,
        version: &Version,
        source: Option<&str>,
        folder: &Path,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        release.require_profile(profile)?;
        let artifact = windows_setup::exact_setup(release, Channel::Stable, version)?;
        let source = setup_download_source(&artifact.url, source)?;
        self.stage_artifact(artifact, &source, folder, cancelled, progress)
            .await
    }

    async fn stage_artifact(
        &self,
        artifact: &Artifact,
        source: &str,
        folder: &Path,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut response = self.request(source, cancelled).await?;
        let mut staged = Staging::new(artifact, folder)?;
        #[cfg(test)]
        {
            staged.test_storage_full = self.test_storage_full;
        }
        let mut progress = progress;
        while let Some(chunk) = cancellable(response.chunk(), cancelled).await? {
            staged.write(&chunk)?;
            progress(staged.total, artifact.size);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        staged.finish()
    }
}

#[cfg(any(windows, test))]
fn setup_download_source(signed: &str, mirror: Option<&str>) -> Result<String> {
    let original = validate_url(signed)?;
    let Some(mirror) = mirror else {
        return Ok(original.into());
    };
    let destination = validate_url(mirror)?;
    let parts: Vec<_> = original.path_segments().ok_or(Error::Url)?.collect();
    if original.host_str() != Some("github.com")
        || original.query().is_some()
        || parts.len() != 6
        || parts[2..4] != ["releases", "download"]
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(Error::Url);
    }
    let prefix = format!("/repos/{}/{}/releases/assets/", parts[0], parts[1]);
    let asset = destination.path().strip_prefix(&prefix).ok_or(Error::Url)?;
    if destination.host_str() != Some("api.github.com")
        || destination.query().is_some()
        || asset.is_empty()
        || !asset.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::Url);
    }
    Ok(destination.into())
}

fn retry_delay(status: u16, headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let exhausted = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0");
    if status != 429 && !(status == 403 && (exhausted || headers.contains_key("retry-after"))) {
        return None;
    }
    let seconds = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| {
            headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|reset| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    reset.saturating_sub(now)
                })
        })
        .unwrap_or(60);
    Some(seconds.clamp(1, 86400))
}

fn token_for<'a>(url: &reqwest::Url, token: Option<&'a str>) -> Option<&'a str> {
    if url.host_str() == Some("api.github.com") {
        token
    } else {
        None
    }
}
fn redirect_url(base: &reqwest::Url, location: &str) -> Result<reqwest::Url> {
    validate_url(base.join(location).map_err(|_| Error::Url)?.as_str())
}

/// Bounds lack of network progress, not total package transfer duration.
/// Dropping an in-flight request/body read stops it when cancellation is observed.
async fn cancellable<T>(
    future: impl std::future::Future<Output = std::result::Result<T, reqwest::Error>>,
    cancelled: &AtomicBool,
) -> Result<T> {
    tokio::pin!(future);
    let start = tokio::time::Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if start.elapsed() >= Duration::from_secs(30) {
            return Err(Error::Network);
        }
        tokio::select! {
            result = &mut future => return result.map_err(|_| Error::Network),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

struct Staging<'a> {
    file: tempfile::NamedTempFile,
    artifact: &'a Artifact,
    destination: PathBuf,
    hasher: Sha256,
    total: u64,
    #[cfg(test)]
    test_storage_full: bool,
}
impl<'a> Staging<'a> {
    fn new(artifact: &'a Artifact, folder: &Path) -> Result<Self> {
        std::fs::create_dir_all(folder)?;
        Ok(Self {
            file: tempfile::NamedTempFile::new_in(folder)?,
            artifact,
            destination: folder.join(&artifact.file_name),
            hasher: Sha256::new(),
            total: 0,
            #[cfg(test)]
            test_storage_full: false,
        })
    }
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.total = self
            .total
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Integrity)?;
        if self.total > self.artifact.size {
            return Err(Error::Integrity);
        }
        #[cfg(test)]
        if self.test_storage_full {
            // Simulate a partial filesystem write followed by disk-full failure.
            // The user's volume is never filled and this hook is absent in builds.
            self.file.write_all(&bytes[..bytes.len() / 2])?;
            return Err(std::io::Error::from(std::io::ErrorKind::StorageFull).into());
        }
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        Ok(())
    }
    fn finish(self) -> Result<PathBuf> {
        if self.total != self.artifact.size
            || format!("{:x}", self.hasher.finalize()) != self.artifact.sha256
        {
            return Err(Error::Integrity);
        }
        self.file.as_file().sync_all()?;
        self.file
            .persist_noclobber(&self.destination)
            .map_err(|e| Error::Io(e.error))?;
        Ok(self.destination)
    }
}

#[cfg(test)]
fn stage_reader(
    reader: &mut impl Read,
    artifact: &Artifact,
    folder: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let mut staged = Staging::new(artifact, folder)?;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        staged.write(&buffer[..count])?;
        progress(staged.total, artifact.size);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    staged.finish()
}

/// Reverification is required in the guardian, under its held package lock.
pub fn verify_package(file: &mut File, artifact: &Artifact) -> Result<()> {
    verify_package_cancellable(file, artifact, &AtomicBool::new(false))
}

pub fn verify_package_cancellable(
    file: &mut File,
    artifact: &Artifact,
    cancelled: &AtomicBool,
) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0u8; 32 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > artifact.size {
            return Err(Error::Integrity);
        }
        hasher.update(&bytes[..count]);
    }
    if total != artifact.size || format!("{:x}", hasher.finalize()) != artifact.sha256 {
        return Err(Error::Integrity);
    }
    Ok(())
}

#[cfg(test)]
mod network_tests;
#[cfg(test)]
mod tests;

mod controller;
pub use controller::{Controller, PreparedDownload, State};
pub mod desktop;
pub mod retention;

pub mod lifecycle;
pub mod setup;
pub mod trust;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub mod windows_anchor;
#[cfg(windows)]
pub mod windows_ready;
#[cfg(windows)]
pub mod windows_setup;

#[cfg(windows)]
pub mod windows_guard;
#[cfg(windows)]
pub mod windows_handoff;
#[cfg(windows)]
pub mod windows_repair;
