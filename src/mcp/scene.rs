//! The motion scene tools' inputs (noh_scene_check, noh_scene_look,
//! noh_scene_clip) and the `motion` field of noh_burn_subtitles. A scene is a
//! file; nothing here names the language it is written in.
use super::types::{CaptionSafeArea, SessionError, TruthInput, absolute};
use crate::scene_request::{SceneChoice, SceneMode, SceneRequest};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A motion scene drawn instead of the captions.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MotionInput {
    /// Absolute path of the scene file (its first line names its format; see
    /// noh_list_effects, `motion`).
    #[schemars(length(min = 1))]
    pub scene: String,
}

impl MotionInput {
    pub(crate) fn choice(self) -> Result<SceneChoice, SessionError> {
        Ok(SceneChoice {
            scene: absolute(&self.scene)?,
            range_ms: None,
            vertical: false,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LookMode {
    /// The frames at `at_ms` (up to 16), one PNG each.
    #[default]
    Stills,
    /// Eight frames over the second that starts at `at_ms[0]`, and a sheet of them.
    Strip,
    /// `count` frames (up to 24) spread over the interval, and a sheet of them.
    Sheet,
    /// Where each text of the frames at `at_ms` falls, as JSON (no picture).
    Measure,
}

/// noh_scene_check.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneCheckInput {
    /// Absolute path of the scene file.
    #[schemars(length(min = 1))]
    pub scene: String,
    /// Absolute path of the video the scene is drawn over: its sound gives the
    /// scene's energy and onsets, its size is the canvas.
    #[schemars(length(min = 1))]
    pub source: String,
    /// Reviewed SRT or its NOH lyrics file (.noh-lyrics.json), source-timed;
    /// kept identical as in noh_burn_subtitles (`truth` when they differ).
    #[schemars(length(min = 1))]
    pub subtitles: String,
    #[serde(default)]
    pub truth: Option<TruthInput>,
    /// The platform's safe area. Required for a vertical video: ask the user
    /// which platform it is for (propose YouTube Shorts).
    #[serde(default)]
    pub safe_area: CaptionSafeArea,
    /// Interval in source milliseconds (default: the whole video).
    #[serde(default)]
    #[schemars(range(min = 0, max = 7200000))]
    pub from_ms: Option<u64>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 7200000))]
    pub to_ms: Option<u64>,
    /// Draw on a vertical frame (a Short, 1080 x 1920) made from the video by
    /// the scene's framing (`video()`; covering and centred without one), as
    /// noh_make_short with framing "scene" exports it. Needs a safe_area.
    #[serde(default)]
    pub vertical: bool,
    /// Absolute path of a folder that does not exist yet: report.json is
    /// written there.
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
}

/// noh_scene_look.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneLookInput {
    #[schemars(length(min = 1))]
    pub scene: String,
    #[schemars(length(min = 1))]
    pub source: String,
    #[schemars(length(min = 1))]
    pub subtitles: String,
    #[serde(default)]
    pub truth: Option<TruthInput>,
    #[serde(default)]
    pub safe_area: CaptionSafeArea,
    #[serde(default)]
    pub mode: LookMode,
    /// Instants in source milliseconds: stills and measure take 1 to 16,
    /// strip takes one (its first frame).
    #[serde(default)]
    pub at_ms: Vec<u64>,
    /// A sheet's number of frames, 1 to 24 (default 12).
    #[serde(default)]
    #[schemars(range(min = 1, max = 24))]
    pub count: Option<u32>,
    /// A sheet's interval in source milliseconds (default: the whole video).
    #[serde(default)]
    #[schemars(range(min = 0, max = 7200000))]
    pub from_ms: Option<u64>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 7200000))]
    pub to_ms: Option<u64>,
    /// The video alone, without the scene's draws: to compose on it (the scene
    /// is still checked).
    #[serde(default)]
    pub bare: bool,
    /// The frame's edges, a line every tenth of the frame and the safe area
    /// drawn over each frame.
    #[serde(default)]
    pub grid: bool,
    /// A NOH window you drive (noh_app_open): it shows the person where you
    /// look. Its playhead moves to the first instant (a sheet: from_ms) as
    /// seek does, waiting while the person navigates, and its monitor shows
    /// the frame's grid (with `grid`) and, on the short, the safe area, while
    /// the playhead stays there. The window's clock is its montage's: the same
    /// as the video's when the window holds that video or its song. The frames
    /// are still drawn by the export's engine, as a job of the heavy-job slot.
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub app_id: Option<String>,
    /// Draw on a vertical frame (a Short, 1080 x 1920) made from the video by
    /// the scene's framing (`video()`; covering and centred without one), as
    /// noh_make_short with framing "scene" exports it. Needs a safe_area.
    #[serde(default)]
    pub vertical: bool,
    /// Absolute path of a folder that does not exist yet.
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
}

/// noh_scene_clip.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneClipInput {
    #[schemars(length(min = 1))]
    pub scene: String,
    #[schemars(length(min = 1))]
    pub source: String,
    #[schemars(length(min = 1))]
    pub subtitles: String,
    #[serde(default)]
    pub truth: Option<TruthInput>,
    #[serde(default)]
    pub safe_area: CaptionSafeArea,
    /// Inclusive start in source milliseconds.
    #[schemars(range(min = 0, max = 7200000))]
    pub start_ms: u64,
    /// Exclusive end in source milliseconds, at most 30 s after the start.
    #[schemars(range(min = 1, max = 7200000))]
    pub end_ms: u64,
    /// Draw on a vertical frame (a Short, 1080 x 1920) made from the video by
    /// the scene's framing (`video()`; covering and centred without one), as
    /// noh_make_short with framing "scene" exports it. Needs a safe_area.
    #[serde(default)]
    pub vertical: bool,
    /// Absolute free MP4 destination.
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
}

/// Longest clip: a clip is for judging a rhythm, an export is for the rest.
pub const MAX_CLIP_MS: u64 = 30_000;

fn ffmpeg(value: Option<&str>, default: &Path) -> Result<std::path::PathBuf, SessionError> {
    Ok(value
        .map(absolute)
        .transpose()?
        .unwrap_or_else(|| default.to_owned()))
}

fn invalid(detail: &str) -> SessionError {
    SessionError::new("mcp.invalid_request", detail)
}

impl SceneCheckInput {
    pub(crate) fn into_request(self, default_ffmpeg: &Path) -> Result<SceneRequest, SessionError> {
        let request = SceneRequest {
            project: None,
            scene: absolute(&self.scene)?,
            source: absolute(&self.source)?,
            subtitles: absolute(&self.subtitles)?,
            ffmpeg: ffmpeg(self.ffmpeg.as_deref(), default_ffmpeg)?,
            safe_area: self.safe_area.into(),
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            mode: SceneMode::Check,
            vertical: self.vertical,
            bare: false,
            grid: false,
            output: absolute(&self.output)?,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}

impl SceneLookInput {
    /// What the driven window `app_id` is sent: the instant looked at first.
    pub(crate) fn marker(&self) -> crate::driven::Action {
        let at_ms = match self.mode {
            LookMode::Sheet => self.from_ms,
            _ => self.at_ms.first().copied(),
        };
        crate::driven::Action::ShowSceneLook {
            at_ms: at_ms.unwrap_or(0),
            safe_area: self.safe_area.into(),
            grid: self.grid,
        }
    }

    pub(crate) fn into_request(self, default_ffmpeg: &Path) -> Result<SceneRequest, SessionError> {
        let mode = match self.mode {
            LookMode::Stills => SceneMode::Stills { at_ms: self.at_ms },
            LookMode::Measure => SceneMode::Measure { at_ms: self.at_ms },
            LookMode::Strip => match self.at_ms.as_slice() {
                [at] => SceneMode::Strip { at_ms: *at },
                _ => return Err(invalid("A strip takes one instant in at_ms.")),
            },
            LookMode::Sheet => SceneMode::Sheet {
                count: self.count.unwrap_or(12) as usize,
            },
        };
        let request = SceneRequest {
            project: None,
            scene: absolute(&self.scene)?,
            source: absolute(&self.source)?,
            subtitles: absolute(&self.subtitles)?,
            ffmpeg: ffmpeg(self.ffmpeg.as_deref(), default_ffmpeg)?,
            safe_area: self.safe_area.into(),
            from_ms: self.from_ms,
            to_ms: self.to_ms,
            mode,
            vertical: self.vertical,
            bare: self.bare,
            grid: self.grid,
            output: absolute(&self.output)?,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}

impl SceneClipInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::captions::CaptionRequest, SessionError> {
        if self.start_ms >= self.end_ms || self.end_ms - self.start_ms > MAX_CLIP_MS {
            return Err(invalid(
                "A clip runs from start_ms to end_ms, at most 30000 ms long.",
            ));
        }
        let request = crate::captions::CaptionRequest {
            source: absolute(&self.source)?,
            subtitles: absolute(&self.subtitles)?,
            output: absolute(&self.output)?,
            ffmpeg: ffmpeg(self.ffmpeg.as_deref(), default_ffmpeg)?,
            style: crate::captions::CaptionStyle {
                safe_area: self.safe_area.into(),
                ..Default::default()
            },
            preview: true,
            effect: None,
            scene: Some(SceneChoice {
                scene: absolute(&self.scene)?,
                range_ms: Some([self.start_ms, self.end_ms]),
                vertical: self.vertical,
            }),
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}

/// What an agent needs before writing a scene, served by noh_list_effects.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct MotionInfo {
    /// Experimental: the format may change without notice.
    pub status: String,
    /// The first line every scene must start with.
    pub first_line: String,
    /// Where a scene for a song lives.
    pub file_name: String,
    /// The language scenes are written in.
    pub language: String,
    /// Every function a scene can call.
    pub functions: Vec<String>,
    /// Functions refused for now ("not in NOH yet").
    pub not_yet: Vec<String>,
    /// The counted caps, checked on every frame.
    pub limits: std::collections::BTreeMap<String, f64>,
    /// Font faces `.face(name)` takes.
    pub faces: Vec<String>,
    /// Safe areas the scene tools take (required for a vertical video).
    pub safe_areas: Vec<String>,
    /// The author guide (Markdown): the language, every builder, the limits.
    pub guide: String,
}

/// The motion section of noh_list_effects, in a build that draws scenes.
#[cfg(feature = "motion")]
pub fn motion_info() -> Option<MotionInfo> {
    use crate::fx::{draw, layer_shader as shader, shapes};
    use crate::motion::script;
    let words = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
    let limits = [
        ("operations_per_frame", script::MAX_OPS as f64),
        ("draws_per_frame", draw::MAX_DRAWS as f64),
        ("passes_per_frame", draw::MAX_PASSES as f64),
        (
            "particles_per_emitter",
            draw::MAX_PARTICLES_PER_EMITTER as f64,
        ),
        ("particles_on_screen", draw::MAX_PARTICLES as f64),
        ("letters_per_frame", draw::MAX_GLYPHS as f64),
        ("surface_per_frame_in_frames", f64::from(draw::MAX_AREA)),
        ("shadow_blurs_per_frame", draw::MAX_BLURS as f64),
        ("different_texts_per_scene", draw::MAX_BLOCKS as f64),
        ("text_characters", draw::MAX_TEXT_CHARS as f64),
        ("text_scale", f64::from(draw::MAX_TEXT_SCALE)),
        ("paths_per_frame", shapes::MAX_PATHS as f64),
        ("points_per_path", shapes::MAX_POINTS as f64),
        ("filled_points_per_path", shapes::MAX_FILL_POINTS as f64),
        ("filled_points_per_frame", shapes::MAX_FILL_TOTAL as f64),
        ("segments_per_frame", shapes::MAX_SEGMENTS as f64),
        (
            "shape_boxes_per_frame_in_frames",
            f64::from(shapes::MAX_SHAPE_AREA),
        ),
        ("gradient_stops", shapes::MAX_STOPS as f64),
        ("costly_calls_per_frame", f64::from(script::MAX_CALLS)),
        ("song_reads_per_frame", f64::from(script::MAX_READS)),
        ("string_bytes", script::MAX_STRING as f64),
        ("call_levels", script::MAX_CALL_LEVELS as f64),
        ("scene_file_bytes", script::MAX_SOURCE as f64),
        ("shader_files", shader::MAX_FILES as f64),
        ("shader_file_bytes", shader::MAX_SOURCE as f64),
        ("shader_layers_per_frame", shader::MAX_SHADER_LAYERS as f64),
        (
            "shader_pixels_per_frame_in_frames",
            f64::from(shader::MAX_SHADER_AREA),
        ),
        ("shader_reads_per_pixel", shader::MAX_READS as f64),
        ("shader_cost_per_pixel", shader::MAX_COST as f64),
        ("shader_helpers", shader::MAX_HELPERS as f64),
        (
            "video_scale_max",
            f64::from(crate::fx::reframe::MAX_VIDEO_SCALE),
        ),
        (
            "video_rot_max",
            f64::from(crate::fx::reframe::MAX_VIDEO_ROT),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    Some(MotionInfo {
        status: "experimental: the format may change without notice".into(),
        first_line: format!("// noh-motion {}", script::FORMAT),
        file_name: "<song>.noh-motion.rhai, beside the song and its lyrics; its layer shaders in <song>.noh-motion.shaders/".into(),
        language: "a small subset of Rhai (see the guide)".into(),
        functions: words(script::FUNCTIONS),
        not_yet: words(script::NOT_YET),
        limits,
        faces: words(&crate::fx::text::FACE_NAMES),
        safe_areas: ["none", "youtube_shorts", "tiktok", "reels", "universal"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        guide: include_str!("../../docs/MOTION_AUTHORING.md").into(),
    })
}

/// A build without motion scenes lists none.
#[cfg(not(feature = "motion"))]
pub fn motion_info() -> Option<MotionInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> String {
        std::env::temp_dir().join(name).display().to_string()
    }

    #[test]
    fn scene_inputs_are_refused_before_any_job() {
        let clip = |start_ms, end_ms| SceneClipInput {
            scene: root("a.noh-motion.rhai"),
            source: root("a.mp4"),
            subtitles: root("a.srt"),
            truth: None,
            safe_area: CaptionSafeArea::None,
            start_ms,
            end_ms,
            vertical: false,
            output: root("noh-scene-clip-test.mp4"),
            ffmpeg: None,
        };
        let ffmpeg = Path::new("ffmpeg");
        assert!(clip(0, MAX_CLIP_MS + 1).into_request(ffmpeg).is_err());
        assert!(clip(2000, 1000).into_request(ffmpeg).is_err());
        if cfg!(feature = "motion") {
            let r = clip(1000, 3000).into_request(ffmpeg).unwrap();
            assert_eq!(r.scene.unwrap().range_ms, Some([1000, 3000]));
            assert!(r.preview);
        }
        let look = SceneLookInput {
            scene: root("a.noh-motion.rhai"),
            source: root("a.mp4"),
            subtitles: root("a.srt"),
            truth: None,
            safe_area: CaptionSafeArea::None,
            mode: LookMode::Strip,
            at_ms: vec![0, 1000],
            count: None,
            from_ms: None,
            to_ms: None,
            bare: false,
            grid: false,
            app_id: None,
            vertical: false,
            output: root("noh-scene-look-test"),
            ffmpeg: None,
        };
        assert!(
            look.clone().into_request(ffmpeg).is_err(),
            "a strip takes one instant"
        );
        // What a driven window is sent: the first instant looked at.
        let marker = |mode, at_ms: Vec<u64>, from_ms| {
            SceneLookInput {
                mode,
                at_ms,
                from_ms,
                grid: true,
                safe_area: CaptionSafeArea::Tiktok,
                ..look.clone()
            }
            .marker()
        };
        let shown = |a| match a {
            crate::driven::Action::ShowSceneLook {
                at_ms,
                safe_area,
                grid,
            } => (at_ms, safe_area, grid),
            other => panic!("{other:?}"),
        };
        let tiktok = crate::safe_area::SafeArea::Tiktok;
        assert_eq!(
            shown(marker(LookMode::Stills, vec![1500, 900], None)),
            (1500, tiktok, true)
        );
        assert_eq!(
            shown(marker(LookMode::Sheet, vec![], Some(4000))),
            (4000, tiktok, true)
        );
        assert_eq!(shown(marker(LookMode::Sheet, vec![], None)).0, 0);
    }

    #[test]
    fn a_burn_with_a_scene_takes_no_caption_style() {
        let input: crate::mcp::types::CaptionsInput = serde_json::from_value(serde_json::json!({
            "source": root("a.mp4"), "subtitles": root("a.srt"), "output": root("o.mp4"),
            "motion": {"scene": root("a.noh-motion.rhai")}, "size": "large"
        }))
        .unwrap();
        let e = input.into_request(Path::new("ffmpeg")).unwrap_err();
        assert_eq!(e.code, "mcp.invalid_request");
    }
}
