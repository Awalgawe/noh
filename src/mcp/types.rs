//! Versioned MCP DTOs; media core types do not depend on schema generation.
use crate::{
    engine::{EngineError, ExportRequest, MediaItem, Message},
    subtitles::SubtitleRequest,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct RenderRequest(RenderRequestFields);

impl<'de> Deserialize<'de> for RenderRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = RenderRequestFields::deserialize(deserializer)?;
        if fields.videos.is_some() == fields.items.is_some() {
            return Err(serde::de::Error::custom(
                "Provide exactly one of videos or items",
            ));
        }
        Ok(Self(fields))
    }
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Vec::deserialize(deserializer).map(Some)
}

fn exclusive_inputs(schema: &mut schemars::Schema) {
    schema.insert(
        "oneOf".into(),
        serde_json::json!([
            {"required":["videos"],"not":{"required":["items"]}},
            {"required":["items"],"not":{"required":["videos"]}}
        ]),
    );
}

fn positive_duration(schema: &mut schemars::Schema) {
    schema.insert("exclusiveMinimum".into(), serde_json::json!(0));
}
fn subtitle_language(schema: &mut schemars::Schema) {
    schema.insert("pattern".into(), serde_json::json!("^(auto|[a-z]{2,3})$"));
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RenderRequest", transform = exclusive_inputs)]
struct RenderRequestFields {
    /// Video-only inputs. Provide exactly one of videos or items.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(
        with = "Vec<String>",
        length(min = 1, max = 4096),
        inner(length(min = 1))
    )]
    videos: Option<Vec<String>>,
    /// Ordered video/image occurrences. Image durations belong to individual entries.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Vec<MediaInput>", length(min = 1, max = 4096))]
    items: Option<Vec<MediaInput>>,
    /// Absolute path to a PCM or floating-point WAV soundtrack.
    #[schemars(length(min = 1))]
    pub wav: String,
    /// Absolute destination path. Existing files are never overwritten.
    #[schemars(length(min = 1))]
    pub output: String,
    /// Absolute FFmpeg executable path; omitted uses the session's engine.
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
    /// Finite, nonnegative seconds.
    #[serde(default)]
    #[schemars(range(min = 0))]
    pub fade_in: f64,
    /// Finite, nonnegative seconds.
    #[serde(default)]
    #[schemars(range(min = 0))]
    pub fade_out: f64,
    #[serde(default = "yes")]
    pub partial_fades: bool,
    #[serde(default)]
    pub clip_audio: bool,
    #[serde(default)]
    pub force_encode: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaInput {
    Video {
        #[schemars(length(min = 1))]
        path: String,
    },
    Image {
        #[schemars(length(min = 1))]
        path: String,
        /// Finite positive seconds; the same path may occur with different durations.
        #[schemars(transform = positive_duration)]
        duration: f64,
    },
}

impl MediaInput {
    fn into_engine(self) -> Result<MediaItem, SessionError> {
        match self {
            Self::Video { path } => Ok(MediaItem::Video {
                path: absolute(&path)?,
            }),
            Self::Image { path, duration } if duration.is_finite() && duration > 0.0 => {
                Ok(MediaItem::Image {
                    path: absolute(&path)?,
                    duration,
                })
            }
            Self::Image { .. } => Err(SessionError::new(
                "mcp.invalid_request",
                "Image duration must be finite and positive.",
            )),
        }
    }
}
fn yes() -> bool {
    true
}
impl RenderRequest {
    pub(crate) fn into_engine(
        self,
        default_ffmpeg: &Path,
        preview: bool,
    ) -> Result<ExportRequest, SessionError> {
        let fields = self.0;
        let items = match (fields.videos, fields.items) {
            (Some(paths), None) => paths
                .iter()
                .map(|path| absolute(path).map(MediaItem::from))
                .collect::<Result<_, _>>()?,
            (None, Some(items)) => items
                .into_iter()
                .map(MediaInput::into_engine)
                .collect::<Result<_, _>>()?,
            _ => {
                return Err(SessionError::new(
                    "mcp.invalid_request",
                    "Provide exactly one of videos or items.",
                ));
            }
        };
        let request = ExportRequest {
            items,
            wav: absolute(&fields.wav)?,
            output: absolute(&fields.output)?,
            ffmpeg: fields
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| default_ffmpeg.to_owned()),
            fade_in: fields.fade_in,
            fade_out: fields.fade_out,
            partial_fades: fields.partial_fades,
            preview,
            clip_audio: fields.clip_audio,
            force_encode: fields.force_encode,
        };
        request.validate().map_err(SessionError::from)?;
        if preview
            && request
                .output
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
        {
            return Err(SessionError::new(
                "mcp.invalid_request",
                "Preview requires an MP4 destination.",
            ));
        }
        Ok(request)
    }
}
pub(crate) fn absolute(value: &str) -> Result<PathBuf, SessionError> {
    let path = Path::new(value);
    if value.is_empty() || value.contains('\0') || !path.is_absolute() {
        return Err(SessionError::new(
            "mcp.invalid_request",
            "Paths must be nonempty absolute local paths without NUL bytes.",
        ));
    }
    Ok(path.to_owned())
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Wav,
    Image,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectInput {
    #[schemars(length(min = 1))]
    pub path: String,
    pub kind: MediaKind,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisLevel {
    Quick,
    #[default]
    Exact,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnoseInput {
    pub request: RenderRequest,
    #[serde(default)]
    pub level: DiagnosisLevel,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    pub request: RenderRequest,
    /// Completed exact diagnosis ID from this same session.
    #[schemars(length(min = 1))]
    pub diagnosis_job_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub request: RenderRequest,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscribeInput {
    /// Explicit local audio file or video whose first audio stream is used.
    #[schemars(length(min = 1))]
    pub source: String,
    /// Explicit free .srt destination; existing files are never overwritten.
    #[schemars(length(min = 1))]
    pub output: String,
    /// Absolute local whisper.cpp CLI executable.
    #[schemars(length(min = 1))]
    pub transcriber: String,
    /// Absolute local transcription model path.
    #[schemars(length(min = 1))]
    pub model: String,
    /// Absolute local Silero VAD model path.
    #[schemars(length(min = 1))]
    pub vad_model: String,
    /// Spoken language tag, or "auto" for backend detection.
    #[serde(default = "auto_language")]
    #[schemars(length(min = 2, max = 4), transform = subtitle_language)]
    pub language: String,
    /// Absolute FFmpeg path; omitted uses the session default.
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
}
fn auto_language() -> String {
    "auto".into()
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionSize {
    Small,
    #[default]
    Medium,
    Large,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPlacement {
    #[default]
    Bottom,
    Top,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionSafeArea {
    #[default]
    None,
    YoutubeShorts,
    Tiktok,
    Reels,
    Universal,
}
impl From<CaptionSafeArea> for crate::safe_area::SafeArea {
    fn from(value: CaptionSafeArea) -> Self {
        match value {
            CaptionSafeArea::None => Self::None,
            CaptionSafeArea::YoutubeShorts => Self::YoutubeShorts,
            CaptionSafeArea::Tiktok => Self::Tiktok,
            CaptionSafeArea::Reels => Self::Reels,
            CaptionSafeArea::Universal => Self::Universal,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptionsInput {
    /// Conservative UI-safe text region; none preserves the original margins.
    #[serde(default)]
    pub safe_area: CaptionSafeArea,
    /// Absolute path to a local video, at most two hours long.
    #[schemars(length(min = 1))]
    pub source: String,
    /// Absolute path to reviewed UTF-8 plain-text SRT.
    #[schemars(length(min = 1))]
    pub subtitles: String,
    /// Absolute free MP4 destination. The source is never replaced.
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
    #[serde(default)]
    pub size: CaptionSize,
    #[serde(default)]
    pub placement: CaptionPlacement,
    /// Render the same complete composition, then reduce resolution and quality.
    #[serde(default)]
    pub preview: bool,
}
impl CaptionsInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::captions::CaptionRequest, SessionError> {
        use crate::captions::{
            CaptionPlacement as Placement, CaptionRequest, CaptionSize as Size, CaptionStyle,
        };
        let request = CaptionRequest {
            source: absolute(&self.source)?,
            subtitles: absolute(&self.subtitles)?,
            output: absolute(&self.output)?,
            ffmpeg: self
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| default_ffmpeg.to_owned()),
            style: CaptionStyle {
                safe_area: self.safe_area.into(),
                size: match self.size {
                    CaptionSize::Small => Size::Small,
                    CaptionSize::Medium => Size::Medium,
                    CaptionSize::Large => Size::Large,
                },
                placement: match self.placement {
                    CaptionPlacement::Top => Placement::Top,
                    CaptionPlacement::Bottom => Placement::Bottom,
                },
            },
            preview: self.preview,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShortFraming {
    #[default]
    Pad,
    Crop,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShortCaptionInput {
    /// Platform UI guide: none, youtube_shorts, tiktok, reels or universal.
    #[serde(default)]
    pub safe_area: CaptionSafeArea,
    /// Absolute reviewed UTF-8 SRT path; times refer to the original source.
    #[schemars(length(min = 1))]
    pub subtitles: String,
    #[serde(default)]
    pub size: CaptionSize,
    #[serde(default)]
    pub placement: CaptionPlacement,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShortInput {
    #[schemars(length(min = 1))]
    pub source: String,
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
    /// Inclusive start in source playback milliseconds; must precede end_ms.
    #[schemars(range(min = 0, max = 7200000))]
    pub start_ms: u64,
    /// Exclusive end in source playback milliseconds; within two hours.
    #[schemars(range(min = 1, max = 7200000))]
    pub end_ms: u64,
    #[serde(default)]
    pub framing: ShortFraming,
    #[serde(default)]
    pub captions: Option<ShortCaptionInput>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectShortInput {
    /// Inclusive start on the full WAV clock.
    #[schemars(range(min = 0, max = 7200000))]
    pub start_ms: u64,
    /// Exclusive end on the full WAV clock.
    #[schemars(range(min = 1, max = 7200000))]
    pub end_ms: u64,
    /// Restart visuals only; WAV and SRT remain on their selected project clock.
    #[serde(default)]
    pub restart_loops: bool,
    #[serde(default)]
    pub framing: ShortFraming,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectInput {
    pub request: RenderRequest,
    #[serde(default)]
    pub short: Option<ProjectShortInput>,
    /// Reviewed SRT on the full WAV clock; presence explicitly requests burning.
    #[serde(default)]
    pub captions: Option<ShortCaptionInput>,
    #[serde(default)]
    pub preview: bool,
}

impl ShortCaptionInput {
    fn into_request(self) -> Result<crate::shorts::ShortCaptions, SessionError> {
        use crate::captions::{CaptionPlacement as Placement, CaptionSize as Size, CaptionStyle};
        Ok(crate::shorts::ShortCaptions {
            subtitles: absolute(&self.subtitles)?,
            style: CaptionStyle {
                safe_area: self.safe_area.into(),
                size: match self.size {
                    CaptionSize::Small => Size::Small,
                    CaptionSize::Medium => Size::Medium,
                    CaptionSize::Large => Size::Large,
                },
                placement: match self.placement {
                    CaptionPlacement::Top => Placement::Top,
                    CaptionPlacement::Bottom => Placement::Bottom,
                },
            },
        })
    }
}

impl ProjectInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::project::ProjectRequest, SessionError> {
        let request = crate::project::ProjectRequest {
            montage: self.request.into_engine(default_ffmpeg, self.preview)?,
            short: self.short.map(|s| crate::project::ProjectShort {
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                restart_loops: s.restart_loops,
                framing: match s.framing {
                    ShortFraming::Pad => crate::shorts::Framing::Pad,
                    ShortFraming::Crop => crate::shorts::Framing::Crop,
                },
            }),
            captions: self
                .captions
                .map(ShortCaptionInput::into_request)
                .transpose()?,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}

impl ShortInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::shorts::ShortRequest, SessionError> {
        use crate::captions::{CaptionPlacement as Placement, CaptionSize as Size, CaptionStyle};
        let captions = self
            .captions
            .map(|input| {
                Ok::<_, SessionError>(crate::shorts::ShortCaptions {
                    subtitles: absolute(&input.subtitles)?,
                    style: CaptionStyle {
                        safe_area: input.safe_area.into(),
                        size: match input.size {
                            CaptionSize::Small => Size::Small,
                            CaptionSize::Medium => Size::Medium,
                            CaptionSize::Large => Size::Large,
                        },
                        placement: match input.placement {
                            CaptionPlacement::Top => Placement::Top,
                            CaptionPlacement::Bottom => Placement::Bottom,
                        },
                    },
                })
            })
            .transpose()?;
        let request = crate::shorts::ShortRequest {
            source: absolute(&self.source)?,
            output: absolute(&self.output)?,
            ffmpeg: self
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| default_ffmpeg.to_owned()),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            framing: match self.framing {
                ShortFraming::Pad => crate::shorts::Framing::Pad,
                ShortFraming::Crop => crate::shorts::Framing::Crop,
            },
            captions,
            preview: self.preview,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}

impl TranscribeInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<SubtitleRequest, SessionError> {
        if !self.output.to_ascii_lowercase().ends_with(".srt") {
            return Err(SessionError::new(
                "mcp.invalid_request",
                "Subtitle output must end in .srt.",
            ));
        }
        let request = SubtitleRequest {
            source: absolute(&self.source)?,
            output: absolute(&self.output)?,
            ffmpeg: self
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| default_ffmpeg.to_owned()),
            transcriber: absolute(&self.transcriber)?,
            model: absolute(&self.model)?,
            vad_model: absolute(&self.vad_model)?,
            language: self.language,
        };
        request.validate().map_err(SessionError::from)?;
        Ok(request)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobInput {
    #[schemars(length(min = 1))]
    pub job_id: String,
    #[serde(default)]
    #[schemars(range(min = 0))]
    pub after_revision: Option<u64>,
    /// Event-based wait, from zero to 30000 milliseconds.
    #[serde(default)]
    #[schemars(range(min = 0, max = 30000))]
    pub wait_ms: u32,
    #[serde(default)]
    pub include_result: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}
impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StartReply {
    pub version: u32,
    pub job_id: String,
    pub state: JobState,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Phase {
    pub code: String,
    pub args: Vec<String>,
}
impl From<Message> for Phase {
    fn from(message: Message) -> Self {
        Self {
            code: message.code,
            args: message.args,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Progress {
    pub percent: u32,
    pub phase: Phase,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobResult {
    Metadata {
        /// Version 1 serialized noh::media::MediaInfo. Contains metadata, never media bytes.
        info: serde_json::Value,
    },
    Diagnosis {
        /// Version 1 serialized noh::inspection::Diagnosis, including its original request,
        /// snapshot, exact plan or quick metadata, reasons and notes. Server owns approval.
        diagnosis: serde_json::Value,
    },
    Export {
        output: String,
        duration: f64,
    },
    Subtitles {
        output: String,
        /// Validated subtitle track metadata and cues; no source media is returned.
        track: serde_json::Value,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct JobReply {
    pub version: u32,
    pub job_id: String,
    pub state: JobState,
    pub revision: u64,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Phase>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SessionError>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{detail}")]
pub struct SessionError {
    pub code: String,
    pub operation: String,
    pub path: Option<String>,
    pub detail: String,
    pub technical: String,
}
impl SessionError {
    pub fn new(code: &str, detail: &str) -> Self {
        Self {
            code: code.into(),
            operation: "mcp".into(),
            path: None,
            detail: detail.into(),
            technical: String::new(),
        }
    }
}
impl From<EngineError> for SessionError {
    fn from(error: EngineError) -> Self {
        Self {
            code: error.code,
            operation: error.operation,
            path: error.path.map(|p| p.to_string_lossy().into_owned()),
            detail: error.detail.into(),
            technical: error.technical.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_defaults_and_unknown_fields() {
        let request: RenderRequest = serde_json::from_value(serde_json::json!({
            "videos": ["clip"], "wav": "music", "output": "output"
        }))
        .unwrap();
        assert!(request.0.partial_fades);
        assert!(!request.0.force_encode);
        assert_eq!(request.0.fade_in, 0.0);
        assert!(
            serde_json::from_value::<RenderRequest>(serde_json::json!({
                "videos": [], "wav": "music", "output": "output", "preview": true
            }))
            .is_err()
        );
        assert!(request.into_engine(Path::new("ffmpeg"), false).is_err());
        let input: DiagnoseInput = serde_json::from_value(serde_json::json!({
            "request": {"videos": ["clip"], "wav": "music", "output": "output"}
        }))
        .unwrap();
        assert!(matches!(input.level, DiagnosisLevel::Exact));
    }

    #[test]
    fn schemas_expose_runtime_bounds_and_closed_input_objects() {
        let request = serde_json::to_value(schemars::schema_for!(RenderRequest)).unwrap();
        assert_eq!(request["additionalProperties"], false);
        assert_eq!(request["properties"]["videos"]["minItems"], 1);
        assert_eq!(request["properties"]["videos"]["maxItems"], 4096);
        assert_eq!(request["properties"]["videos"]["items"]["minLength"], 1);
        assert_eq!(request["properties"]["videos"]["type"], "array");
        assert_eq!(request["properties"]["items"]["type"], "array");
        assert_eq!(request["properties"]["items"]["minItems"], 1);
        assert_eq!(request["properties"]["items"]["maxItems"], 4096);
        assert_eq!(
            request["oneOf"],
            serde_json::json!([
                {"required":["videos"],"not":{"required":["items"]}},
                {"required":["items"],"not":{"required":["videos"]}}
            ])
        );
        assert_eq!(request["properties"]["output"]["minLength"], 1);
        assert_eq!(request["properties"]["fade_in"]["minimum"], 0);
        let job = serde_json::to_value(schemars::schema_for!(JobInput)).unwrap();
        assert_eq!(job["properties"]["wait_ms"]["maximum"], 30000);
        assert_eq!(job["properties"]["job_id"]["minLength"], 1);
    }

    #[test]
    fn input_lists_reject_missing_ambiguous_null_duplicate_and_unknown_fields() {
        for fields in [
            r#""videos":[],"items":[]"#,
            r#""videos":null"#,
            r#""items":null"#,
            r#""videos":null,"items":[]"#,
            r#""videos":[],"videos":[]"#,
            r#""items":[],"items":[]"#,
            r#""items":[{"kind":"video","path":"a.mp4","duration":2}]"#,
            r#""items":[{"kind":"image","path":"a.png"}]"#,
            r#""items":[{"kind":"image","path":"a.png","duration":2,"extra":true}]"#,
            r#""videos":[],"extra":true"#,
        ] {
            let json = format!("{{{fields},\"wav\":\"music.wav\",\"output\":\"out.mp4\"}}");
            assert!(
                serde_json::from_str::<RenderRequest>(&json).is_err(),
                "{json}"
            );
        }
        assert!(
            serde_json::from_str::<RenderRequest>(r#"{"wav":"music.wav","output":"out.mp4"}"#)
                .is_err()
        );
    }

    #[test]
    fn video_only_and_typed_inputs_normalize_to_one_ordered_collection() {
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        let base = serde_json::json!({"wav":path("music.wav"),"output":path("out.mp4")});
        let mut video_only = base.clone();
        video_only["videos"] = serde_json::json!([path("video.mp4")]);
        let video_only: RenderRequest = serde_json::from_value(video_only.clone()).unwrap();
        let video_only = video_only.into_engine(Path::new("ffmpeg"), false).unwrap();
        assert_eq!(video_only.items, vec![MediaItem::from(path("video.mp4"))]);
        let serialized = serde_json::to_value(&video_only).unwrap();
        assert!(serialized.get("videos").is_some());
        assert!(serialized.get("items").is_none());
        let mut mixed = base;
        mixed["items"] = serde_json::json!([
            {"kind":"image","path":path("same.png"),"duration":1.0},
            {"kind":"video","path":path("video.mp4")},
            {"kind":"image","path":path("same.png"),"duration":2.0}
        ]);
        let mixed: RenderRequest = serde_json::from_value(mixed).unwrap();
        let mixed = mixed.into_engine(Path::new("ffmpeg"), false).unwrap();
        assert_eq!(mixed.items.len(), 3);
        assert_ne!(mixed.items[0], mixed.items[2]);
        assert_eq!(mixed.items[0].path(), mixed.items[2].path());
        let serialized = serde_json::to_value(&mixed).unwrap();
        assert!(serialized.get("videos").is_none());
        assert_eq!(serialized["items"][2]["duration"], 2.0);
    }

    #[test]
    fn invalid_image_duration_and_path_never_reach_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png").to_string_lossy().into_owned();
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = MediaInput::Image {
                path: path.clone(),
                duration,
            }
            .into_engine()
            .unwrap_err();
            assert_eq!(error.code, "mcp.invalid_request");
        }
        assert!(
            MediaInput::Image {
                path: "relative.png".into(),
                duration: 1.0
            }
            .into_engine()
            .is_err()
        );
        let schema = serde_json::to_value(schemars::schema_for!(MediaInput)).unwrap();
        let image = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["kind"]["const"] == "image")
            .unwrap();
        assert_eq!(image["properties"]["duration"]["exclusiveMinimum"], 0);
        assert_eq!(image["additionalProperties"], false);
    }

    #[test]
    fn transcription_input_is_strict_explicit_and_defaults_only_language() {
        let input: TranscribeInput = serde_json::from_value(serde_json::json!({
            "source":"C:/media/source.wav",
            "output":"C:/media/captions.srt",
            "transcriber":"C:/tools/whisper-cli.exe",
            "model":"C:/models/ggml-tiny.bin",
            "vad_model":"C:/models/vad.bin"
        }))
        .unwrap();
        assert_eq!(input.language, "auto");
        assert!(input.ffmpeg.is_none());
        let schema = serde_json::to_value(schemars::schema_for!(TranscribeInput)).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        for field in ["source", "output", "transcriber", "model", "vad_model"] {
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(field))
            );
            assert_eq!(schema["properties"][field]["minLength"], 1);
        }
        assert_eq!(
            schema["properties"]["language"]["pattern"],
            "^(auto|[a-z]{2,3})$"
        );
        for invalid in [
            serde_json::json!({"source":"x","output":"x.srt","transcriber":"x","model":"x"}),
            serde_json::json!({"source":"x","output":"x.srt","transcriber":"x","model":"x","vad_model":"x","shell":"-f lavfi"}),
        ] {
            assert!(serde_json::from_value::<TranscribeInput>(invalid).is_err());
        }
        let relative: TranscribeInput = serde_json::from_value(serde_json::json!({
            "source":"relative.wav","output":"relative.srt",
            "transcriber":"whisper-cli","model":"model.bin","vad_model":"vad.bin"
        }))
        .unwrap();
        let error = relative.into_request(Path::new("ffmpeg.exe")).unwrap_err();
        assert_eq!(error.code, "mcp.invalid_request");
    }
    #[test]
    fn short_inputs_are_strict_absolute_and_map_to_the_shared_request() {
        let folder = tempfile::tempdir().unwrap();
        let json = serde_json::json!({
            "source":folder.path().join("source.mp4"),"output":folder.path().join("short.mp4"),
            "start_ms":135,"end_ms":1775
        });
        let input: ShortInput = serde_json::from_value(json.clone()).unwrap();
        let request = input
            .into_request(&folder.path().join("ffmpeg.exe"))
            .unwrap();
        assert_eq!((request.start_ms, request.end_ms), (135, 1775));
        assert_eq!(request.framing, crate::shorts::Framing::Pad);
        assert!(request.captions.is_none() && !request.preview);
        let mut styled = json.clone();
        styled["framing"] = "crop".into();
        styled["preview"] = true.into();
        styled["captions"] = serde_json::json!({"subtitles":folder.path().join("reviewed.srt"),"size":"large","placement":"top"});
        let request = serde_json::from_value::<ShortInput>(styled.clone())
            .unwrap()
            .into_request(&request.ffmpeg)
            .unwrap();
        assert_eq!(request.framing, crate::shorts::Framing::Crop);
        assert!(request.preview);
        let captions = request.captions.unwrap();
        assert_eq!(captions.style.size, crate::captions::CaptionSize::Large);
        assert_eq!(
            captions.style.placement,
            crate::captions::CaptionPlacement::Top
        );
        styled["captions"]["arbitrary_filter"] = "drawtext".into();
        assert!(serde_json::from_value::<ShortInput>(styled).is_err());
        for (key, value) in [
            ("framing", serde_json::json!("automatic")),
            ("start_ms", serde_json::json!(-1)),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut bad = json.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<ShortInput>(bad).is_err(), "{key}");
        }
        let mut invalid = json.clone();
        invalid["end_ms"] = 135.into();
        assert_eq!(
            serde_json::from_value::<ShortInput>(invalid)
                .unwrap()
                .into_request(Path::new("ffmpeg"))
                .unwrap_err()
                .code,
            "error.short_range"
        );
        let mut invalid = json;
        invalid["source"] = "relative.mp4".into();
        assert_eq!(
            serde_json::from_value::<ShortInput>(invalid)
                .unwrap()
                .into_request(Path::new("ffmpeg"))
                .unwrap_err()
                .code,
            "mcp.invalid_request"
        );
        let schema = serde_json::to_value(schemars::schema_for!(ShortInput)).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["start_ms"]["maximum"], 7200000);
        assert_eq!(schema["properties"]["end_ms"]["minimum"], 1);
    }
}
