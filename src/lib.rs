//! Shared media library. Binary adapters own process arguments and exit codes.
use std::{
    env,
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub mod build_info;
mod caption_layout;
pub mod captions;
pub mod cli;
pub mod engine;
mod h264;
mod hybrid;
pub mod i18n;
mod images;
pub mod input;
pub mod inspection;
pub mod jobs;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod media;
pub mod plan;
#[cfg(feature = "gui")]
pub mod playback;
pub mod preview;
mod progress;
pub mod project;
pub mod resources;
pub mod safe_area;
#[cfg(feature = "gui")]
pub mod scrub;
mod sequence;
pub mod shorts;
mod storage;
pub mod subtitle_srt;
pub mod subtitle_track;
pub mod subtitles;
pub mod timeline;
mod timing;
#[cfg(feature = "updates")]
pub mod update;
mod wav;
pub mod waveform;
#[derive(Debug)]
struct CodedError {
    key: &'static str,
    detail: String,
}
impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for CodedError {}
fn coded_error(key: &'static str, detail: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(CodedError {
        key,
        detail: detail.into(),
    })
}
pub fn error_key(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    error.downcast_ref::<CodedError>().map(|e| e.key)
}
fn ffmpeg_error(error: std::io::Error) -> Box<dyn std::error::Error> {
    coded_error(
        if error.kind() == std::io::ErrorKind::NotFound {
            "error.ffmpeg"
        } else {
            "error.engine"
        },
        error.to_string(),
    )
}

/// Duration from PCM/float frames, without decoding or rounding to whole seconds.
pub fn wav_duration(path: &Path) -> Result<f64> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    wav::duration(&mut file, length)
}

pub fn find_ffmpeg(choice: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = choice.or_else(|| env::var_os("NOH_FFMPEG").filter(|p| !p.is_empty())) {
        return Ok(path.into());
    }
    let exe = env::current_exe()?;
    let folder = exe.parent().ok_or("Executable folder not found")?;
    if let Some(bundled) = bundled_ffmpeg(folder) {
        return Ok(bundled);
    }
    #[cfg(target_os = "macos")]
    // Prefer the qualified 7.1 runtime, including when Finder starts the app
    // without a shell PATH. Homebrew's default formula also omits libass.
    for path in [
        "/opt/homebrew/opt/ffmpeg@7/bin/ffmpeg",
        "/usr/local/opt/ffmpeg@7/bin/ffmpeg",
        "/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg",
        "/usr/local/opt/ffmpeg-full/bin/ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
    ] {
        if Path::new(path).is_file() {
            return Ok(path.into());
        }
    }
    system_ffmpeg(
        &env::var_os("PATH").unwrap_or_default(),
        &env::current_dir()?,
    )
    .ok_or_else(|| {
        ffmpeg_error(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "FFmpeg executable not found",
        ))
    })
}

/// Resolve discovery to an absolute executable before spawning. In particular,
/// Windows must never get a bare name and search the working directory first.
fn system_ffmpeg(path: &std::ffi::OsStr, cwd: &Path) -> Option<PathBuf> {
    let cwd = cwd.canonicalize().ok()?;
    let cwd_bin = cwd.join("bin").canonicalize().ok();
    env::split_paths(path)
        .filter(|p| p.is_absolute())
        .find_map(|folder| {
            let folder = folder.canonicalize().ok()?;
            if folder == cwd || Some(&folder) == cwd_bin.as_ref() {
                return None;
            }
            let candidate = folder.join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
            if !candidate.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if candidate.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            Some(candidate)
        })
}

fn bundled_ffmpeg(folder: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    [folder.join("bin").join(name), folder.join(name)]
        .into_iter()
        .find(|path| path.is_file())
}

#[cfg(test)]
mod portable_tests {
    #[test]
    fn system_discovery_skips_empty_relative_and_working_folders() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("cwd");
        let bin = cwd.join("bin");
        let system = root.path().join("system");
        for folder in [&cwd, &bin, &system] {
            std::fs::create_dir_all(folder).unwrap();
            let file = folder.join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
            std::fs::write(&file, []).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let unsafe_path = std::env::join_paths([
            std::path::Path::new(""),
            std::path::Path::new("bin"),
            &cwd,
            &bin,
        ])
        .unwrap();
        assert!(super::system_ffmpeg(&unsafe_path, &cwd).is_none());
        let path = std::env::join_paths([std::path::Path::new("."), &cwd, &bin, &system]).unwrap();
        let found = super::system_ffmpeg(&path, &cwd).unwrap();
        assert!(found.is_absolute());
        assert_eq!(found.parent().unwrap(), system.canonicalize().unwrap());
    }
    #[test]
    fn engine_discovery_supports_launcher_cli_and_flat_folders() {
        let folder = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        assert!(super::bundled_ffmpeg(folder.path()).is_none());
        let flat_runtime = folder.path().join(name);
        std::fs::write(&flat_runtime, []).unwrap();
        assert_eq!(super::bundled_ffmpeg(folder.path()), Some(flat_runtime));
        let bin = folder.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let engine = bin.join(name);
        std::fs::write(&engine, []).unwrap();
        assert_eq!(super::bundled_ffmpeg(folder.path()), Some(engine.clone()));
        assert_eq!(super::bundled_ffmpeg(&bin), Some(engine));
    }
}

/// Hide subprocess consoles without changing redirected output.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

mod process;
