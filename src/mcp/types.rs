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
/// When the .srt and its NOH lyrics file differ: which one is the source of
/// truth. lyrics rewrites the .srt from the lyrics file; srt replaces the
/// lyrics file with the .srt's lines. The replaced file is kept beside as
/// `<name>.replaced…`. Ignored when they do not differ.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TruthInput {
    Lyrics,
    Srt,
}
impl From<TruthInput> for crate::lyrics_input::Truth {
    fn from(value: TruthInput) -> Self {
        match value {
            TruthInput::Lyrics => Self::Lyrics,
            TruthInput::Srt => Self::Srt,
        }
    }
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
    /// Absolute path to reviewed UTF-8 plain-text SRT, or to its NOH lyrics file
    /// (`.noh-lyrics.json`). The lyrics file is created beside an .srt given
    /// alone; when the two differ, `truth` is required.
    #[schemars(length(min = 1))]
    pub subtitles: String,
    /// Required when the .srt and its lyrics file differ (see TruthInput).
    #[serde(default)]
    pub truth: Option<TruthInput>,
    /// Absolute free MP4 destination. The source is never replaced.
    #[schemars(length(min = 1))]
    pub output: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub ffmpeg: Option<String>,
    /// Classique's size; absent means medium. Refused with another effect.
    #[serde(default)]
    pub size: Option<CaptionSize>,
    /// Classique's placement; absent means bottom. Refused with another effect.
    #[serde(default)]
    pub placement: Option<CaptionPlacement>,
    /// A caption effect instead of Classique (see noh_list_effects).
    #[serde(default)]
    pub effect: Option<EffectInput>,
    /// Render the same complete composition, then reduce resolution and quality.
    #[serde(default)]
    pub preview: bool,
}
/// A caption effect chosen by id; its folder comes from the effect folders NOH
/// reads, never from the request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectInput {
    /// An effect id from noh_list_effects (noh.classic is Classique).
    #[schemars(length(min = 1))]
    pub id: String,
    /// One of the effect's presets.
    #[serde(default)]
    pub preset: Option<String>,
    /// The effect's settings by id, each checked against its declared type before
    /// the job starts. A font setting takes a bundled face or
    /// "system:<installed family>"; a family missing on this computer is drawn in
    /// the effect's own font with warning.effect_font_missing.
    #[serde(default)]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The caption style and the effect plugin (None: Classique, its values folded
/// into the style), resolved before any job starts. `project_preview`: a project
/// preview, which draws no effect outside the app.
fn caption_choice(
    safe_area: CaptionSafeArea,
    size: Option<CaptionSize>,
    placement: Option<CaptionPlacement>,
    effect: Option<EffectInput>,
    project_preview: bool,
) -> Result<
    (
        crate::captions::CaptionStyle,
        Option<crate::project::EffectChoice>,
    ),
    SessionError,
> {
    use crate::captions::{CaptionPlacement as Placement, CaptionSize as Size, CaptionStyle};
    use crate::effect_choice::{Chosen, Flags, Given, Wanted};
    let style = CaptionStyle {
        line_width: None,
        safe_area: safe_area.into(),
        size: match size.unwrap_or_default() {
            CaptionSize::Small => Size::Small,
            CaptionSize::Medium => Size::Medium,
            CaptionSize::Large => Size::Large,
        },
        placement: match placement.unwrap_or_default() {
            CaptionPlacement::Top => Placement::Top,
            CaptionPlacement::Bottom => Placement::Bottom,
        },
    };
    let Some(effect) = effect else {
        return Ok((style, None));
    };
    let wanted = Wanted {
        id: effect.id,
        preset: effect.preset,
        values: effect
            .values
            .into_iter()
            .map(|(k, v)| (k, Given::Json(v)))
            .collect(),
    };
    let flags = Flags {
        size: size.is_some(),
        placement: placement.is_some(),
    };
    match crate::effect_choice::resolve(&wanted, style, flags, project_preview)
        .map_err(SessionError::from)?
    {
        Chosen::Classic(style) => Ok((style, None)),
        Chosen::Plugin(choice) => Ok((style, Some(choice))),
    }
}

impl CaptionsInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::captions::CaptionRequest, SessionError> {
        use crate::captions::CaptionRequest;
        let (style, effect) = caption_choice(
            self.safe_area,
            self.size,
            self.placement,
            self.effect,
            false,
        )?;
        let request = CaptionRequest {
            effect,
            source: absolute(&self.source)?,
            subtitles: absolute(&self.subtitles)?,
            output: absolute(&self.output)?,
            ffmpeg: self
                .ffmpeg
                .as_deref()
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| default_ffmpeg.to_owned()),
            style,
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
    /// Absolute reviewed UTF-8 SRT path, or its NOH lyrics file
    /// (`.noh-lyrics.json`); times refer to the original source. The lyrics file
    /// is created beside an .srt given alone; when the two differ, `truth` is
    /// required.
    #[schemars(length(min = 1))]
    pub subtitles: String,
    /// Required when the .srt and its lyrics file differ (see TruthInput).
    #[serde(default)]
    pub truth: Option<TruthInput>,
    /// Classique's size; absent means medium. Refused with another effect.
    #[serde(default)]
    pub size: Option<CaptionSize>,
    /// Classique's placement; absent means bottom. Refused with another effect.
    #[serde(default)]
    pub placement: Option<CaptionPlacement>,
    /// A caption effect instead of Classique (see noh_list_effects).
    #[serde(default)]
    pub effect: Option<EffectInput>,
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
    /// The captions and the effect plugin chosen for them (None: Classique).
    fn into_request(
        self,
        project_preview: bool,
    ) -> Result<
        (
            crate::shorts::ShortCaptions,
            Option<crate::project::EffectChoice>,
        ),
        SessionError,
    > {
        let subtitles = absolute(&self.subtitles)?;
        let (style, effect) = caption_choice(
            self.safe_area,
            self.size,
            self.placement,
            self.effect,
            project_preview,
        )?;
        Ok((crate::shorts::ShortCaptions { subtitles, style }, effect))
    }
}

impl ProjectInput {
    pub(crate) fn into_request(
        self,
        default_ffmpeg: &Path,
    ) -> Result<crate::project::ProjectRequest, SessionError> {
        let preview = self.preview;
        let (captions, effect) = match self.captions {
            Some(input) => {
                let (captions, effect) = input.into_request(preview)?;
                (Some(captions), effect)
            }
            None => (None, None),
        };
        let request = crate::project::ProjectRequest {
            montage: self.request.into_engine(default_ffmpeg, preview)?,
            short: self.short.map(|s| crate::project::ProjectShort {
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                restart_loops: s.restart_loops,
                framing: match s.framing {
                    ShortFraming::Pad => crate::shorts::Framing::Pad,
                    ShortFraming::Crop => crate::shorts::Framing::Crop,
                },
            }),
            effect,
            captions,
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
        let (captions, effect) = match self.captions {
            Some(input) => {
                let (captions, effect) = input.into_request(false)?;
                (Some(captions), effect)
            }
            None => (None, None),
        };
        let request = crate::shorts::ShortRequest {
            effect,
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
/// noh_list_effects takes nothing.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEffectsInput {}

/// The caption effects a request can name (`effect.id`), from the folders NOH
/// reads, bundled first.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EffectsReply {
    pub version: u32,
    /// The folders read and whether each exists.
    pub folders: Vec<EffectFolder>,
    pub effects: Vec<EffectListed>,
    /// Folders that are not valid effects, and why.
    pub refused: Vec<EffectRefused>,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EffectFolder {
    pub path: String,
    pub exists: bool,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EffectListed {
    pub id: String,
    /// English name, or the id.
    pub name: String,
    /// Absent: built into NOH.
    pub folder: Option<String>,
    pub presets: Vec<String>,
    pub settings: Vec<EffectSetting>,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EffectSetting {
    pub id: String,
    /// color, number, bool, palette, choice, font or text.
    #[serde(rename = "type")]
    pub kind: String,
    pub default: serde_json::Value,
    /// A choice's texts; a font's bundled faces (a font also takes
    /// "system:<installed family>").
    pub choices: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// 1: first settings in the app; 2: under "more settings".
    pub level: u8,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct EffectRefused {
    pub folder: String,
    pub reason: String,
}
impl From<crate::effect_choice::Listing> for EffectsReply {
    fn from(l: crate::effect_choice::Listing) -> Self {
        let text = |p: &Path| p.display().to_string();
        Self {
            version: 1,
            folders: l
                .folders
                .iter()
                .map(|f| EffectFolder {
                    path: text(&f.path),
                    exists: f.exists,
                })
                .collect(),
            effects: l
                .effects
                .into_iter()
                .map(|e| EffectListed {
                    folder: e.folder.as_deref().map(text),
                    id: e.id,
                    name: e.name,
                    presets: e.presets,
                    settings: e
                        .settings
                        .into_iter()
                        .map(|s| EffectSetting {
                            id: s.id,
                            kind: s.kind,
                            default: s.default,
                            choices: s.choices,
                            min: s.min,
                            max: s.max,
                            level: s.level,
                        })
                        .collect(),
                })
                .collect(),
            refused: l
                .refused
                .iter()
                .map(|r| EffectRefused {
                    folder: text(&r.folder),
                    reason: r.reason.clone(),
                })
                .collect(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct StartReply {
    pub version: u32,
    pub job_id: String,
    pub state: JobState,
    pub revision: u64,
    /// What NOH did to the subtitles and their lyrics file before starting:
    /// a lyrics file created, an .srt rewritten, the replaced file kept as….
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lyrics: Vec<Phase>,
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

/// noh_app_open: a NOH window the agent drives.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppOpenInput {
    /// What the window is for, shown to the person in the window (one line,
    /// at most 80 characters kept).
    #[schemars(length(min = 1))]
    pub title: String,
}

/// noh_app_close: one of this session's windows.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppCloseInput {
    #[schemars(length(min = 1))]
    pub app_id: String,
}

/// Where a driven window stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    /// Started, not ready yet.
    Opening,
    /// Driven by the agent.
    Open,
    /// The person stopped the piloting: an ordinary NOH, theirs.
    Released,
    /// Not reading for 5 s: requests wait; its work may still run.
    Lost,
    /// Closed by the agent or by the person.
    Closed,
    /// Ended without closing (a crash, or killed).
    Exited,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppSummary {
    pub app_id: String,
    pub title: String,
    pub state: AppState,
    pub pid: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppOpenReply {
    pub version: u32,
    pub app_id: String,
    pub state: AppState,
    pub pid: u32,
    /// The NOH version the window runs.
    pub app_version: String,
    /// Every window of this session, this one included.
    pub windows: Vec<AppSummary>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppCloseReply {
    pub version: u32,
    pub app_id: String,
    pub state: AppState,
    /// The window's process had ended when the reply was sent.
    pub exited: bool,
}

/// noh_app_do: one gesture in a driven window.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppDoInput {
    #[schemars(length(min = 1))]
    pub app_id: String,
    pub action: crate::driven::Action,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppDoReply {
    pub version: u32,
    pub app_id: String,
    /// The window's state revision once the gesture was applied.
    pub revision: u64,
    /// What the window answered.
    pub result: serde_json::Value,
}

/// noh_app_state: what a driven window shows.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppStateInput {
    #[schemars(length(min = 1))]
    pub app_id: String,
    /// Return once the revision is later than this one.
    #[serde(default)]
    pub after_revision: Option<u64>,
    /// How long to wait for that, at most 30000.
    #[serde(default)]
    #[schemars(range(max = 30000))]
    pub wait_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppStateReply {
    pub version: u32,
    pub app_id: String,
    pub state: AppState,
    pub revision: u64,
    /// The window's state as it last sent it: clips, song, lyrics, effect,
    /// range, export readiness, job, open question, `user_position`.
    pub view: serde_json::Value,
}

/// noh_app_look: a PNG of a driven window.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppLookInput {
    #[schemars(length(min = 1))]
    pub app_id: String,
    /// `window` (default) or `monitor`.
    #[serde(default)]
    pub scope: Option<crate::driven::LookScope>,
    /// Move the playhead there first (as `seek`); absent: the screen as it is,
    /// where the person is.
    #[serde(default)]
    pub at_ms: Option<u64>,
    /// A free absolute path ending in .png.
    pub output: PathBuf,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct AppLookReply {
    pub version: u32,
    pub app_id: String,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    /// The playhead when the picture was taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
}

/// Why a window tool did nothing.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema, thiserror::Error)]
#[error("{detail}")]
pub struct AppError {
    pub code: String,
    pub detail: String,
    /// The figures of a refused open; the question a window has open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl AppError {
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            details: None,
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
    /// Step 7 B: an effect by id. Absent size and placement stay medium and
    /// bottom; Classique's values fold into the style; Classique's options with
    /// another effect, and a project preview with one, are refused at start.
    #[cfg(feature = "effects")]
    #[test]
    fn caption_effects_are_chosen_by_id_and_checked_at_start() {
        use crate::captions::{CaptionPlacement, CaptionSize};
        let folder = tempfile::tempdir().unwrap();
        let path = |name: &str| folder.path().join(name);
        let burn = serde_json::json!({
            "source": path("in.mp4"), "subtitles": path("x.srt"), "output": path("out.mp4")
        });
        let plain = serde_json::from_value::<CaptionsInput>(burn.clone())
            .unwrap()
            .into_request(Path::new("ffmpeg"))
            .unwrap();
        assert_eq!(plain.style.size, CaptionSize::Medium);
        assert_eq!(plain.style.placement, CaptionPlacement::Bottom);
        assert!(plain.effect.is_none());
        let mut classic = burn.clone();
        classic["effect"] =
            serde_json::json!({"id": "noh.classic", "values": {"size": "large", "width": 75.5}});
        let request = serde_json::from_value::<CaptionsInput>(classic.clone())
            .unwrap()
            .into_request(Path::new("ffmpeg"))
            .unwrap();
        assert_eq!(request.style.size, CaptionSize::Large);
        assert_eq!(request.style.line_width, Some(7_550));
        assert!(request.effect.is_none());
        classic["size"] = "small".into();
        let error = serde_json::from_value::<CaptionsInput>(classic)
            .unwrap()
            .into_request(Path::new("ffmpeg"))
            .unwrap_err();
        assert_eq!(error.code, "error.effect");
        assert!(error.detail.contains("given twice"), "{}", error.detail);
        let mut sized = burn;
        sized["placement"] = "top".into();
        sized["effect"] = serde_json::json!({"id": "noh.drop"});
        let error = serde_json::from_value::<CaptionsInput>(sized)
            .unwrap()
            .into_request(Path::new("ffmpeg"))
            .unwrap_err();
        assert!(
            error.detail.contains("Classique's options"),
            "{}",
            error.detail
        );
        let project = serde_json::json!({
            "request": {"videos": [path("v.mp4")], "wav": path("s.wav"), "output": path("o.mp4")},
            "captions": {"subtitles": path("x.srt"), "effect": {"id": "noh.drop"}},
            "preview": true
        });
        let error = serde_json::from_value::<ProjectInput>(project)
            .unwrap()
            .into_request(Path::new("ffmpeg"))
            .unwrap_err();
        assert_eq!(error.code, "error.effect_preview");
        for schema in [
            serde_json::to_value(schemars::schema_for!(CaptionsInput)).unwrap(),
            serde_json::to_value(schemars::schema_for!(ShortCaptionInput)).unwrap(),
        ] {
            assert!(schema["properties"]["effect"].is_object(), "{schema}");
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            for optional in ["size", "placement", "effect"] {
                assert!(!required.contains(&optional.into()), "{optional}: {schema}");
            }
        }
        let effect = serde_json::to_value(schemars::schema_for!(EffectInput)).unwrap();
        assert_eq!(effect["additionalProperties"], false);
        assert_eq!(effect["required"], serde_json::json!(["id"]));
    }
}
