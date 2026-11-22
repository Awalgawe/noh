//! What a motion scene job asks (check a scene, look at frames of it, or draw it
//! in a caption burn), as plain data: compiled in every build, so the requests
//! cross the worker boundary unchanged; only a build with the `motion` feature
//! runs them (`crate::motion::run`). Nothing here names the script language: a
//! scene is a file, and its first line names its format.
use crate::engine::EngineError;
use crate::safe_area::SafeArea;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Frames a `stills` or `measure` look may ask for.
pub const MAX_STILLS: usize = 16;
/// Frames of a `strip`: the second that starts at its instant.
pub const STRIP_FRAMES: usize = 8;
/// Frames a `sheet` may spread over the interval.
pub const MAX_SHEET: usize = 24;
/// Longest scene job (a check evaluates every frame of the interval twice).
pub const TOTAL_TIMEOUT_SECS: u64 = 900;
/// The folder a scene job writes in its workspace, published whole.
pub const STAGED: &str = "scene";
/// Files a scene job may publish, and their total size.
const MAX_FILES: usize = 64;
const MAX_BYTES: u64 = 1 << 30;

/// The staged folder before publication: a real folder of regular files,
/// within bounds. A worker's word is not trusted.
pub(crate) fn check_staged(dir: &Path) -> Result<(), EngineError> {
    let bad = |detail: &str| {
        EngineError::new(
            "error.scene_output",
            "validate_scene_output",
            Some(dir),
            detail,
        )
    };
    let meta = std::fs::symlink_metadata(dir).map_err(|e| bad(&e.to_string()))?;
    if !meta.file_type().is_dir() {
        return Err(bad("The scene job wrote no folder."));
    }
    let (mut files, mut bytes) = (0usize, 0u64);
    for entry in std::fs::read_dir(dir).map_err(|e| bad(&e.to_string()))? {
        let entry = entry.map_err(|e| bad(&e.to_string()))?;
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| bad(&e.to_string()))?;
        if !meta.file_type().is_file() {
            return Err(bad("The scene folder holds something other than files."));
        }
        files += 1;
        bytes += meta.len();
    }
    if files == 0 || files > MAX_FILES || bytes > MAX_BYTES {
        return Err(bad("The scene folder is empty or too large."));
    }
    Ok(())
}

/// Publish the staged folder under its final name: a rename in the same folder,
/// refused if anything has taken the name meanwhile.
pub(crate) fn publish(staged: &Path, output: &Path) -> Result<(), EngineError> {
    if output.exists() {
        return Err(EngineError::new(
            "error.output_exists",
            "publish",
            Some(output),
            "Output already exists. Choose another folder name.",
        ));
    }
    std::fs::rename(staged, output).map_err(|e| {
        EngineError::new(
            if output.exists() {
                "error.output_exists"
            } else {
                "error.publish"
            },
            "publish",
            Some(output),
            e,
        )
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SceneMode {
    /// Validate the scene and write `report.json`.
    Check,
    /// The frames at these instants (source milliseconds).
    Stills { at_ms: Vec<u64> },
    /// Eight frames over the second that starts at `at_ms`.
    Strip { at_ms: u64 },
    /// `count` frames spread evenly over the interval.
    Sheet { count: usize },
    /// Where each text of the frames at these instants falls, as JSON.
    Measure { at_ms: Vec<u64> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneRequest {
    /// The scene file.
    pub scene: PathBuf,
    /// The video: its sound drives the scene's energy and onsets, its picture is
    /// under the looks, its size is the canvas.
    pub source: PathBuf,
    /// Reviewed SRT (word times read from the lyrics file beside it).
    pub subtitles: PathBuf,
    pub ffmpeg: PathBuf,
    #[serde(default)]
    pub safe_area: SafeArea,
    /// The interval checked or looked at, in source milliseconds (default: all).
    #[serde(default)]
    pub from_ms: Option<u64>,
    #[serde(default)]
    pub to_ms: Option<u64>,
    #[serde(flatten)]
    pub mode: SceneMode,
    /// Looks: the video alone, without the scene's draws (to compose on it).
    #[serde(default)]
    pub bare: bool,
    /// Looks: the frame's edges, a line every tenth and the safe area drawn over.
    #[serde(default)]
    pub grid: bool,
    /// A folder that does not exist yet: the files are written there.
    pub output: PathBuf,
}

/// A scene drawn instead of the captions in a caption burn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneChoice {
    pub scene: PathBuf,
    /// Only this interval of the source, in milliseconds (a short clip to judge
    /// a rhythm); the scene still sees the source's clock.
    #[serde(default)]
    pub range_ms: Option<[u64; 2]>,
}

fn invalid(code: &str, detail: impl ToString) -> EngineError {
    EngineError::new(code, "validate_scene", None, detail)
}

fn absolute(path: &Path, what: &str) -> Result<(), EngineError> {
    if !path.is_absolute() {
        return Err(invalid(
            "error.scene_request",
            format!("The {what} path must be absolute."),
        ));
    }
    Ok(())
}

impl SceneRequest {
    /// Syntax only: absolute paths, an interval in order, counts within bounds.
    pub fn validate(&self) -> Result<(), EngineError> {
        absolute(&self.scene, "scene")?;
        absolute(&self.source, "source")?;
        absolute(&self.subtitles, "subtitles")?;
        absolute(&self.output, "output")?;
        if let (Some(a), Some(b)) = (self.from_ms, self.to_ms)
            && a >= b
        {
            return Err(invalid(
                "error.scene_request",
                "from_ms must come before to_ms.",
            ));
        }
        match &self.mode {
            SceneMode::Check => {}
            SceneMode::Stills { at_ms } | SceneMode::Measure { at_ms } => {
                if at_ms.is_empty() || at_ms.len() > MAX_STILLS {
                    return Err(invalid(
                        "error.scene_request",
                        format!("Ask for 1 to {MAX_STILLS} instants."),
                    ));
                }
            }
            SceneMode::Strip { .. } => {}
            SceneMode::Sheet { count } => {
                if *count == 0 || *count > MAX_SHEET {
                    return Err(invalid(
                        "error.scene_request",
                        format!("A sheet takes 1 to {MAX_SHEET} frames."),
                    ));
                }
            }
        }
        if self.output.exists() {
            return Err(EngineError::new(
                "error.output_exists",
                "validate_scene",
                Some(&self.output),
                "Output already exists. Choose another folder name.",
            ));
        }
        Ok(())
    }
}

impl SceneChoice {
    pub fn validate(&self) -> Result<(), EngineError> {
        absolute(&self.scene, "scene")?;
        if let Some([a, b]) = self.range_ms
            && a >= b
        {
            return Err(invalid(
                "error.scene_request",
                "The clip's start must come before its end.",
            ));
        }
        if !cfg!(feature = "motion") {
            return Err(invalid(
                "error.scene_unavailable",
                "This build of NOH cannot draw scenes.",
            ));
        }
        Ok(())
    }
}

/// A scene needs a chosen safe area on a vertical picture: NOH cannot know the
/// platform, so the agent asks the user and says it.
pub fn safe_area_required(width: u32, height: u32, safe: SafeArea) -> Result<(), EngineError> {
    if height > width && safe == SafeArea::None {
        return Err(invalid(
            "error.safe_area_required",
            "This video is vertical: choose the safe area of the platform it is for (ask the user; youtube_shorts, tiktok, reels or universal).",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(mode: SceneMode) -> SceneRequest {
        let root = std::env::temp_dir();
        SceneRequest {
            scene: root.join("a.noh-motion.rhai"),
            source: root.join("a.mp4"),
            subtitles: root.join("a.srt"),
            ffmpeg: PathBuf::new(),
            safe_area: SafeArea::None,
            from_ms: None,
            to_ms: None,
            mode,
            bare: false,
            grid: false,
            output: root.join("noh-scene-request-test-that-does-not-exist"),
        }
    }

    #[test]
    fn a_scene_request_is_checked_before_any_work() {
        assert!(request(SceneMode::Check).validate().is_ok());
        let mut r = request(SceneMode::Check);
        r.scene = PathBuf::from("relative.rhai");
        assert_eq!(r.validate().unwrap_err().code, "error.scene_request");
        let mut r = request(SceneMode::Check);
        (r.from_ms, r.to_ms) = (Some(2000), Some(1000));
        assert!(r.validate().is_err());
        assert!(
            request(SceneMode::Stills { at_ms: vec![] })
                .validate()
                .is_err()
        );
        assert!(
            request(SceneMode::Stills {
                at_ms: vec![0; MAX_STILLS + 1]
            })
            .validate()
            .is_err()
        );
        assert!(
            request(SceneMode::Sheet {
                count: MAX_SHEET + 1
            })
            .validate()
            .is_err()
        );
        let mut r = request(SceneMode::Check);
        r.output = std::env::temp_dir();
        assert_eq!(r.validate().unwrap_err().code, "error.output_exists");
    }

    #[test]
    fn a_vertical_picture_needs_a_chosen_safe_area() {
        assert!(safe_area_required(1920, 1080, SafeArea::None).is_ok());
        assert_eq!(
            safe_area_required(1080, 1920, SafeArea::None)
                .unwrap_err()
                .code,
            "error.safe_area_required"
        );
        assert!(safe_area_required(1080, 1920, SafeArea::YoutubeShorts).is_ok());
    }

    #[test]
    fn the_mode_travels_flat_in_the_request() {
        let r = request(SceneMode::Strip { at_ms: 1500 });
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["mode"], "strip");
        assert_eq!(v["at_ms"], 1500);
        let back: SceneRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}
