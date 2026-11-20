//! Network access through the system's curl: no HTTP or TLS library enters the
//! packaged build. Windows 10 1803 and later ship `curl.exe` in System32 (it
//! uses Schannel, so the Windows certificate store); on Linux `/usr/bin/curl` is
//! used when it exists, and nothing is checked otherwise.

use super::{Asset, DOWNLOADS, Error, FEED, MAX_TEXT, Result};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Where the release description and its files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub feed: String,
    /// Prefix of every file: `<base>v<version>/<name>`.
    pub base: String,
    /// Only for the trial switch: the files are local.
    pub local: bool,
}

/// The trial switch: run an installed copy against a release that is not
/// published, prepared in a local folder (`latest.json` and `v<version>/…`).
pub const TRIAL: &str = "NOH_AUTOUPDATE_TRIAL";

impl Source {
    pub fn github() -> Self {
        Source {
            feed: FEED.into(),
            base: DOWNLOADS.into(),
            local: false,
        }
    }
    /// GitHub, or the trial folder when the switch is set. `None`: the switch
    /// holds something other than a `file://` folder, and nothing is checked.
    pub fn configured() -> Option<Self> {
        match std::env::var(TRIAL) {
            Ok(value) if !value.is_empty() => Self::trial(&value),
            _ => Some(Self::github()),
        }
    }
    pub fn trial(value: &str) -> Option<Self> {
        if !value.starts_with("file:///") || value.contains(['?', '#']) {
            return None;
        }
        let base = if value.ends_with('/') {
            value.to_owned()
        } else {
            format!("{value}/")
        };
        Some(Source {
            feed: format!("{base}latest.json"),
            base,
            local: true,
        })
    }
}

/// The curl this platform provides, by absolute path (never a `PATH` lookup).
pub fn curl() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let path = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32").join("curl.exe"));
    #[cfg(target_os = "linux")]
    let path = Some(PathBuf::from("/usr/bin/curl"));
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let path: Option<PathBuf> = None;
    path.filter(|p| p.is_file())
}

fn command(curl: &Path, source: &Source) -> Command {
    let mut command = Command::new(curl);
    let scheme = if source.local { "=file" } else { "=https" };
    command
        // Must come first: ignore the user's .curlrc/_curlrc, which could turn
        // off certificate checks, add a proxy or redirect the output.
        .arg("--disable")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--proto", scheme, "--proto-redir", scheme])
        .args(["--connect-timeout", "20"])
        .args(["--user-agent", concat!("NOH/", env!("CARGO_PKG_VERSION"))])
        .stdin(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// curl's exit status, sorted into the kinds the interface tells apart.
fn failure(code: Option<i32>, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let detail = if detail.is_empty() {
        format!("curl exited with {code:?}")
    } else {
        detail
    };
    match code {
        // Write error, cannot create the output file.
        Some(23) => Error::Storage(detail),
        // Larger than declared.
        Some(63) => Error::Integrity(detail),
        // Unsupported or refused protocol, including a redirect off HTTPS.
        Some(1) => Error::Integrity(detail),
        _ => Error::Network(detail),
    }
}

/// The release description, held in memory.
pub fn release_description(
    curl: &Path,
    source: &Source,
    stop: &dyn Fn() -> bool,
) -> Result<String> {
    let mut command = command(curl, source);
    if !source.local {
        command.args(["--header", "Accept: application/vnd.github+json"]);
    }
    text(command, &source.feed, stop)
}

/// A small text file of the release (its component catalog).
pub fn release_text(
    curl: &Path,
    source: &Source,
    asset: &Asset,
    stop: &dyn Fn() -> bool,
) -> Result<String> {
    let text = text(command(curl, source), &asset.url, stop)?;
    if text.len() as u64 != asset.size {
        return Err(Error::Integrity(format!(
            "{} has the wrong size",
            asset.name
        )));
    }
    let digest = super::sha256_reader(text.as_bytes(), &|| false)?;
    if digest != asset.sha256 {
        return Err(Error::Integrity(format!(
            "{} has the wrong digest",
            asset.name
        )));
    }
    Ok(text)
}

/// Runs curl to memory; `stop` kills it within a quarter of a second, so
/// closing NOH never waits for a slow answer.
fn text(mut command: Command, url: &str, stop: &dyn Fn() -> bool) -> Result<String> {
    use std::io::Read;
    let mut child = command
        .args(["--max-time", "60", "--max-filesize"])
        .arg(MAX_TEXT.to_string())
        .arg("--")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Unsupported(format!("curl did not start: {e}")))?;
    crate::process::adopt(&child);
    // Pipes are drained on their own threads so a full pipe never blocks curl.
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.read_to_end(&mut bytes);
        bytes
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = err.read_to_end(&mut bytes);
        bytes
    });
    let status = loop {
        if stop() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Storage("cancelled".into()));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = errors.join().unwrap_or_default();
    if !status.success() {
        return Err(failure(status.code(), &stderr));
    }
    String::from_utf8(stdout).map_err(|_| Error::Integrity(format!("{url} is not text")))
}

/// Downloads `asset` into `folder`, checks its size and SHA-256, and returns its
/// final path. A file already there with the right size and digest is kept
/// (a download interrupted by closing NOH resumes from scratch, a finished one
/// is not repeated). `progress` receives the bytes written so far; `stop` kills
/// curl.
pub fn download(
    curl: &Path,
    source: &Source,
    asset: &Asset,
    folder: &Path,
    progress: &dyn Fn(u64),
    stop: &dyn Fn() -> bool,
) -> Result<PathBuf> {
    if asset.name.contains(['/', '\\', ':']) || asset.name.starts_with('.') {
        return Err(Error::Integrity(format!("unsafe file name {}", asset.name)));
    }
    std::fs::create_dir_all(folder)?;
    let target = folder.join(&asset.name);
    if verified(&target, asset, stop)? {
        progress(asset.size);
        return Ok(target);
    }
    let partial = folder.join(format!("{}.partial", asset.name));
    let _ = std::fs::remove_file(&partial);
    let mut child = command(curl, source)
        // A stalled transfer (under 1 KiB/s for a minute) fails instead of hanging.
        .args([
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "--retry",
            "2",
        ])
        .arg("--max-filesize")
        .arg(asset.size.to_string())
        .arg("--output")
        .arg(&partial)
        .arg("--")
        .arg(&asset.url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Unsupported(format!("curl did not start: {e}")))?;
    crate::process::adopt(&child);
    let status = loop {
        if stop() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&partial);
            return Err(Error::Storage("cancelled".into()));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if let Ok(meta) = std::fs::metadata(&partial) {
            progress(meta.len());
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    if !status.success() {
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_end(&mut stderr);
        }
        let _ = std::fs::remove_file(&partial);
        return Err(failure(status.code(), &stderr));
    }
    if !verified(&partial, asset, stop)? {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Integrity(format!(
            "{} differs from what the release declares",
            asset.name
        )));
    }
    std::fs::rename(&partial, &target)?;
    progress(asset.size);
    Ok(target)
}

/// The file exists with the declared size and digest.
fn verified(path: &Path, asset: &Asset, stop: &dyn Fn() -> bool) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == asset.size => {
            Ok(super::sha256_file(path, stop)? == asset.sha256)
        }
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_local_folder_can_replace_github() {
        assert_eq!(Source::trial("https://example.com/"), None);
        assert_eq!(Source::trial("file://server/share/"), None);
        assert_eq!(Source::trial("file:///C:/trial/?x"), None);
        let trial = Source::trial("file:///C:/trial").unwrap();
        assert_eq!(trial.feed, "file:///C:/trial/latest.json");
        assert_eq!(trial.base, "file:///C:/trial/");
        assert!(trial.local);
        let github = Source::github();
        assert_eq!(github.feed, FEED);
        assert_eq!(github.base, DOWNLOADS);
        assert!(!github.local);
    }

    #[test]
    fn curl_failures_are_sorted() {
        assert!(matches!(
            failure(Some(6), b"Could not resolve host"),
            Error::Network(_)
        ));
        assert!(matches!(failure(Some(22), b"404"), Error::Network(_)));
        assert!(matches!(failure(Some(23), b""), Error::Storage(_)));
        assert!(matches!(failure(Some(63), b""), Error::Integrity(_)));
        assert!(matches!(failure(Some(1), b""), Error::Integrity(_)));
        assert_eq!(
            failure(None, b"").to_string(),
            "network: curl exited with None"
        );
    }

    /// The real system curl against files in a temporary folder.
    #[test]
    fn downloads_are_checked_before_they_are_kept() {
        let Some(curl) = curl() else {
            eprintln!("No system curl; skipped");
            return;
        };
        let folder = tempfile::tempdir().unwrap();
        let release = folder.path().join("release");
        std::fs::create_dir_all(release.join("v0.3.0")).unwrap();
        let payload = b"installer bytes".repeat(1000);
        std::fs::write(release.join("v0.3.0/NOH-Setup.exe"), &payload).unwrap();
        std::fs::write(release.join("latest.json"), "{\"tag_name\": \"v0.3.0\"}").unwrap();
        let url = |p: &Path| {
            let text = p.to_string_lossy().replace('\\', "/");
            format!("file:///{}", text.trim_start_matches('/'))
        };
        let source = Source::trial(&url(&release)).unwrap();
        assert!(
            release_description(&curl, &source, &|| false)
                .unwrap()
                .contains("v0.3.0")
        );
        // Asked to stop: curl is killed and nothing is returned.
        assert!(matches!(
            release_description(&curl, &source, &|| true),
            Err(Error::Storage(_))
        ));

        let asset = Asset {
            name: "NOH-Setup.exe".into(),
            url: format!("{}v0.3.0/NOH-Setup.exe", source.base),
            size: payload.len() as u64,
            sha256: crate::auto_update::sha256_reader(&payload[..], &|| false).unwrap(),
        };
        let downloads = folder.path().join("downloads");
        let seen = std::cell::Cell::new(0);
        let path = download(
            &curl,
            &source,
            &asset,
            &downloads,
            &|n| seen.set(n),
            &|| false,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        assert_eq!(seen.get(), asset.size);
        assert!(!downloads.join("NOH-Setup.exe.partial").exists());

        // A wrong digest: nothing is kept.
        let wrong = Asset {
            name: "Other.exe".into(),
            sha256: "0".repeat(64),
            ..asset.clone()
        };
        assert!(matches!(
            download(&curl, &source, &wrong, &downloads, &|_| {}, &|| false),
            Err(Error::Integrity(_))
        ));
        assert!(!downloads.join("Other.exe").exists());
        assert!(!downloads.join("Other.exe.partial").exists());

        // Larger than declared: curl stops it.
        let short = Asset {
            name: "Short.exe".into(),
            size: 10,
            ..asset.clone()
        };
        assert!(download(&curl, &source, &short, &downloads, &|_| {}, &|| false).is_err());
        assert!(!downloads.join("Short.exe").exists());

        // The user's curl configuration is ignored. Control first: without
        // `--disable` this configuration file is read.
        let home = folder.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        for name in ["_curlrc", ".curlrc"] {
            std::fs::write(home.join(name), "write-out = \"CURLRC-APPLIED\\n\"\n").unwrap();
        }
        let plain = Command::new(&curl)
            .args(["--silent", "--"])
            .arg(&source.feed)
            .env("CURL_HOME", &home)
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&plain.stdout).contains("CURLRC-APPLIED"));
        let mut ours = command(&curl, &source);
        ours.env("CURL_HOME", &home).env("HOME", &home);
        let fetched = text(ours, &source.feed, &|| false).unwrap();
        assert!(!fetched.contains("CURLRC-APPLIED"), "{fetched}");

        // The local switch never reaches the network, and GitHub never reads files.
        let github = Source::github();
        let local_file = Asset {
            url: asset.url.clone(),
            name: "Third.exe".into(),
            ..asset.clone()
        };
        assert!(matches!(
            download(&curl, &github, &local_file, &downloads, &|_| {}, &|| false),
            Err(Error::Integrity(_))
        ));
    }
}
