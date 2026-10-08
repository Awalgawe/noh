//! Adapter-independent requests and execution. No process arguments or console IO.
pub use crate::input::MediaItem;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportRequest {
    #[serde(flatten, with = "crate::input::item_list")]
    pub items: Vec<MediaItem>,
    pub wav: PathBuf,
    pub output: PathBuf,
    pub ffmpeg: PathBuf,
    pub fade_in: f64,
    pub fade_out: f64,
    pub partial_fades: bool,
    pub preview: bool,
    pub clip_audio: bool,
    pub force_encode: bool,
}

impl ExportRequest {
    /// Compare render inputs and options, ignoring the destination folder and stem.
    /// Container extensions are case insensitive, as they are during rendering.
    pub fn same_render_settings(&self, other: &Self) -> bool {
        self.items == other.items
            && self.wav == other.wav
            && self.ffmpeg == other.ffmpeg
            && self.fade_in == other.fade_in
            && self.fade_out == other.fade_out
            && self.partial_fades == other.partial_fades
            && self.preview == other.preview
            && self.clip_audio == other.clip_audio
            && self.force_encode == other.force_encode
            && self
                .output
                .extension()
                .map(|extension| extension.to_ascii_lowercase())
                == other
                    .output
                    .extension()
                    .map(|extension| extension.to_ascii_lowercase())
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        if self.items.is_empty() || self.items.len() > 4096 {
            return Err(EngineError::new(
                "error.select_files",
                "validate",
                None,
                "Provide between 1 and 4096 media items.",
            ));
        }
        for item in &self.items {
            if let MediaItem::Image { path, duration } = item {
                crate::images::validate(path, *duration)?;
            }
        }
        if [self.fade_in, self.fade_out]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(EngineError::new(
                "error.fades",
                "validate",
                None,
                "Fade duration must be finite and nonnegative.",
            ));
        }
        Ok(())
    }
}

/// Stable fields survive the worker boundary; the technical excerpt is bounded.
#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error, PartialEq, Eq)]
#[error("{detail}")]
pub struct EngineError {
    pub code: String,
    pub operation: String,
    pub path: Option<PathBuf>,
    pub detail: Box<str>,
    pub technical: Box<str>,
}
impl EngineError {
    /// Preserve the desktop's existing seven-language error catalogs.
    pub fn message_code(&self) -> &str {
        match self.code.as_str() {
            "error.video"
            | "error.image"
            | "error.image_duration"
            | "error.wav"
            | "error.fades"
            | "error.output_exists"
            | "error.output_folder"
            | "error.ffmpeg"
            | "error.start"
            | "error.dependencies"
            | "error.stale_inspection"
            | "error.select_files" => &self.code,
            "error.subtitle_input"
            | "error.subtitle_config"
            | "error.subtitle_backend"
            | "error.subtitle_result"
            | "error.subtitle_limit" => &self.code,
            "error.caption_input"
            | "error.caption_config"
            | "error.caption_track"
            | "error.caption_layout"
            | "error.caption_backend"
            | "error.caption_limit"
            | "error.caption_result" => &self.code,
            "error.short_input"
            | "error.short_config"
            | "error.short_range"
            | "error.short_backend"
            | "error.short_limit"
            | "error.short_result" => &self.code,
            _ => "error.engine",
        }
    }
    pub fn new(code: &str, operation: &str, path: Option<&Path>, detail: impl ToString) -> Self {
        Self {
            code: code.into(),
            operation: operation.into(),
            path: path.map(Path::to_path_buf),
            detail: bounded(&detail.to_string(), 4096).into(),
            technical: "".into(),
        }
    }
    pub(crate) fn wrap(
        operation: &str,
        path: Option<&Path>,
        error: &(dyn std::error::Error + 'static),
    ) -> Self {
        if let Some(error) = error.downcast_ref::<Self>() {
            return error.clone();
        }
        let code = crate::error_key(error).unwrap_or("error.engine");
        Self::new(code, operation, path, error)
    }
}
pub(crate) fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[truncated] {}", &text[start..])
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportResult {
    pub output: PathBuf,
    pub duration: f64,
}

/// Structured message ID and arguments; adapters choose language/presentation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub code: String,
    pub args: Vec<String>,
}
impl Message {
    pub(crate) fn parse(value: &str) -> Self {
        let mut fields = value.split('|');
        Self {
            code: fields.next().unwrap_or_default().into(),
            args: fields.map(str::to_owned).collect(),
        }
    }
    pub fn to_wire(&self) -> String {
        std::iter::once(self.code.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("|")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Progress { percent: u32, phase: Message },
    Warning(Message),
    Diagnostic(String),
    Plan(crate::plan::RenderPlan),
    Done(Result<ExportResult, EngineError>),
    Inspected(Result<crate::inspection::InspectionResult, EngineError>),
    Subtitled(Result<crate::subtitles::SubtitleResult, EngineError>),
    Cancelled,
}
impl Event {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done(_) | Self::Inspected(_) | Self::Subtitled(_) | Self::Cancelled
        )
    }
}

pub(crate) struct Reporter<'a> {
    emit: &'a mut dyn FnMut(Event),
    percent: u32,
}
impl<'a> Reporter<'a> {
    pub fn new(emit: &'a mut dyn FnMut(Event)) -> Self {
        Self { emit, percent: 0 }
    }
    pub fn progress(&mut self, percent: u32, phase: &str) {
        self.percent = self.percent.max(percent.min(99));
        tracing::debug!(percent = self.percent, phase, "export progress");
        (self.emit)(Event::Progress {
            percent: self.percent,
            phase: Message::parse(phase),
        });
    }
    pub fn warning(&mut self, reason: &str) {
        (self.emit)(Event::Warning(Message::parse(reason)));
    }
    pub fn diagnostic(&mut self, detail: impl ToString) {
        (self.emit)(Event::Diagnostic(bounded(&detail.to_string(), 4096)));
    }
    pub fn plan(&mut self, plan: crate::plan::RenderPlan) {
        (self.emit)(Event::Plan(plan));
    }
}

/// Execute in a private, supervisor-owned workspace. The caller publishes only
/// after success. Forced worker termination cannot destroy a user's destination.
pub fn execute(
    request: ExportRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    execute_checked(request, workspace, None, emit)
}

pub(crate) fn execute_checked(
    request: ExportRequest,
    workspace: &Path,
    expected: Option<&crate::inspection::Diagnosis>,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    request.validate()?;
    if let Some(expected) = expected {
        expected.validate(&request)?;
    }
    let snapshot = crate::inspection::Snapshot::project(&request)?;
    let span = tracing::info_span!("export", output = %request.output.display());
    let _entered = span.enter();
    let path = request.output.clone();
    let result = render(request, workspace, expected, &mut Reporter::new(emit))
        .map_err(|e| EngineError::wrap("export", Some(&path), e.as_ref()))?;
    snapshot.verify()?;
    Ok(result)
}

/// Exact, potentially expensive source diagnosis. Call from a background task.
/// Final fade segments are refined after preparation; this does not claim that
/// a not-yet-created intermediate has passed timestamp/header verification.
pub fn inspect_and_plan(
    request: &ExportRequest,
    emit: &mut dyn FnMut(Event),
) -> Result<crate::plan::RenderPlan, EngineError> {
    request.validate()?;
    let duration = wav_duration(&request.wav)
        .map_err(|e| EngineError::new("error.wav", "read_soundtrack", Some(&request.wav), e))?;
    if request.fade_in + request.fade_out > duration {
        return Err(EngineError::new(
            "error.fades",
            "plan",
            Some(&request.wav),
            "Combined fades exceed the WAV duration.",
        ));
    }
    let has_fades = request.fade_in > 0.0 || request.fade_out > 0.0;
    let partial = request.partial_fades && has_fades && !request.preview && !request.force_encode;
    let encode = request.force_encode || request.preview || (!request.partial_fades && has_fades);
    let inspection = sequence::inspect(
        &crate::inspection::resolve_ffmpeg(&request.ffmpeg)?,
        &request.items,
        encode,
        request.preview,
        partial,
        &mut Reporter::new(emit),
    )
    .map_err(|e| EngineError::wrap("inspect", None, e.as_ref()))?;
    if request.items.len() > 1 || request.items[0].is_image() || request.clip_audio || partial {
        Ok(inspection.decision.public)
    } else {
        Ok(direct_plan(request, &inspection.info[0], duration))
    }
}

fn direct_plan(
    request: &ExportRequest,
    info: &crate::media::MediaInfo,
    duration: f64,
) -> crate::plan::RenderPlan {
    use crate::plan::{ClipPlan, RenderPlan, Segment, Treatment};
    let encode =
        request.force_encode || request.preview || request.fade_in > 0.0 || request.fade_out > 0.0;
    let treatment = if encode {
        Treatment::Convert
    } else {
        Treatment::Copy
    };
    let mut target = crate::plan::target(info, request.preview);
    if !request.preview {
        target.width = if encode && info.rotation.rem_euclid(180.0).abs() > 0.01 {
            info.height
        } else {
            info.width
        };
        target.height = if encode && info.rotation.rem_euclid(180.0).abs() > 0.01 {
            info.width
        } else {
            info.height
        };
        target.pixel_format = "source-dependent".into();
    }
    if !encode {
        target.codec = info.codec.clone();
        target.pixel_format = "preserved".into();
    }
    RenderPlan {
        exact_inspection: true,
        target,
        stage: "direct_export".into(),
        clips: vec![ClipPlan {
            path: request.items[0].path().clone(),
            treatment,
            reasons: if encode {
                vec![
                    if request.preview {
                        "preview"
                    } else if request.force_encode {
                        "requested_encoding"
                    } else {
                        "fades"
                    }
                    .into(),
                ]
            } else {
                Vec::new()
            },
            timestamp_normalization: false,
            source_seconds: info.seconds,
        }],
        segments: vec![Segment {
            start: 0.0,
            end: duration,
            treatment,
        }],
    }
}

use crate::{coded_error, command, hybrid, progress::ProgressRange, sequence, wav_duration};
use std::{ffi::OsString, fs};
fn render(
    args: ExportRequest,
    workspace: &Path,
    expected: Option<&crate::inspection::Diagnosis>,
    report: &mut Reporter,
) -> crate::Result<ExportResult> {
    let mut items = args.items.clone();
    for item in &mut items {
        let code = if item.is_image() {
            "error.image"
        } else {
            "error.video"
        };
        *item.path_mut() = input_path(item.path(), code)?;
        if !item.path().is_file() {
            return Err(EngineError::new(
                code,
                "validate_input",
                Some(item.path()),
                "Media file not found",
            )
            .into());
        }
    }
    let mut video = items[0].path().clone();
    let mut wav = input_path(&args.wav, "error.wav")?;
    if !video.is_file() || !wav.is_file() {
        return Err("Provide two existing files.".into());
    }
    let duration = wav_duration(&wav)
        .map_err(|e| EngineError::new("error.wav", "read_soundtrack", Some(&wav), e))?;
    if args.fade_in + args.fade_out > duration {
        return Err(coded_error(
            "error.fades",
            format!("Combined fades exceed the WAV duration ({duration:.3} s)."),
        ));
    }
    let output = args.output.clone();
    let output = std::path::absolute(output)?;
    if output.exists() {
        return Err(coded_error(
            "error.output_exists",
            format!(
                "Output already exists: {}. Choose another name.",
                output.display()
            ),
        ));
    }
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "mp4" && ext != "mkv" {
        return Err("Output must be an .mp4 or .mkv file.".into());
    }
    let parent = output.parent().ok_or("Missing output folder")?;
    if !parent.is_dir() {
        return Err(coded_error(
            "error.output_folder",
            "Output folder does not exist.",
        ));
    }
    let ffmpeg = crate::inspection::resolve_ffmpeg(&args.ffmpeg)?;
    let temp_file = workspace.join(format!("export.{ext}"));
    let has_fades = args.fade_in > 0.0 || args.fade_out > 0.0;
    let mut prepared_encoded = false;
    let mut prepared_parameter_sets = 0;
    let partial_fades = args.partial_fades && has_fades && !args.preview && !args.force_encode;
    let needs_preparation =
        items.len() > 1 || items[0].is_image() || args.clip_audio || partial_fades;
    let render_progress = ProgressRange::new(if needs_preparation { 40 } else { 0 }, 99);
    let encode = args.force_encode || args.preview || (!args.partial_fades && has_fades);
    let mut inspection =
        sequence::inspect(&ffmpeg, &items, encode, args.preview, partial_fades, report)?;
    // Keep the caller's path spelling in public plans; execution uses resolved paths.
    for (clip, path) in inspection.decision.public.clips.iter_mut().zip(&args.items) {
        clip.path = path.path().clone();
    }
    let source_plan = if needs_preparation {
        inspection.decision.public.clone()
    } else {
        direct_plan(&args, &inspection.info[0], duration)
    };
    if let Some(expected) = expected {
        expected.validate(&args)?;
        // Reinspect at export; never execute a stale or merely quick copy promise.
        if expected.plan.as_ref() != Some(&source_plan) {
            return Err(crate::inspection::stale(None).into());
        }
    }
    if !needs_preparation {
        report.plan(direct_plan(&args, &inspection.info[0], duration));
    }
    if needs_preparation {
        if ext != "mp4" {
            return Err("A clip sequence or audio mixing requires MP4 output.".into());
        }
        let prepared = sequence::prepare(
            sequence::Preparation {
                ffmpeg: &ffmpeg,
                items: &items,
                wav: &wav,
                folder: workspace,
                duration,
                encode,
                preview: args.preview,
                audio: args.clip_audio,
            },
            inspection,
            report,
        )?;
        video = prepared.video;
        wav = prepared.wav;
        prepared_encoded = prepared.encoded;
        prepared_parameter_sets = prepared.parameter_sets;
    }
    if has_fades
        && ((args.partial_fades && !args.preview && !args.force_encode) || prepared_encoded)
    {
        if ext != "mp4" {
            return Err("Partial fades require MP4 output.".into());
        }
        if hybrid::render(
            hybrid::FadeRender {
                ffmpeg: &ffmpeg,
                source: &video,
                wav: &wav,
                output: &temp_file,
                duration,
                fade_in: args.fade_in,
                fade_out: args.fade_out,
                preview: args.preview,
                progress: render_progress,
                reserved_parameter_sets: prepared_parameter_sets,
            },
            report,
        )? {
            return Ok(ExportResult { output, duration });
        }
    }
    let mut cmd = command(&ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-n", "-stream_loop", "-1", "-i"])
        .arg(&video)
        .arg("-i")
        .arg(&wav)
        .args(["-map", "0:v:0", "-map", "1:a:0"]);

    let encode_final = has_fades || ((args.preview || args.force_encode) && !prepared_encoded);
    let mode = if args.preview {
        RenderMode::Preview
    } else if encode_final || prepared_encoded {
        RenderMode::Encode
    } else {
        RenderMode::Copy
    };
    if encode_final {
        let mut filters = vec!["setpts=PTS-STARTPTS".to_owned()];
        if args.preview {
            filters.push("scale=w='max(2,trunc(min(640,min(iw*sar,360*dar))/2)*2)':h='max(2,trunc(min(360,min(ih,640/dar))/2)*2)',setsar=1".into());
        }
        if args.fade_in > 0.0 {
            filters.push(format!("fade=t=in:st=0:d={:.9}", args.fade_in));
        }
        if args.fade_out > 0.0 {
            filters.push(format!(
                "fade=t=out:st={:.9}:d={:.9}",
                duration - args.fade_out,
                args.fade_out
            ));
        }
        cmd.args([
            "-vf",
            &filters.join(","),
            "-c:v",
            "libx264",
            "-crf",
            if args.preview { "28" } else { "16" },
            "-preset",
            if args.preview { "ultrafast" } else { "medium" },
        ]);
        if args.preview {
            cmd.args(["-pix_fmt", "yuv420p"]);
        }
    } else {
        cmd.args(["-c:v", "copy"]);
    }
    if ext == "mkv" {
        cmd.args(["-c:a", "copy"]);
    } else {
        cmd.args([
            "-c:a",
            "aac",
            "-b:a",
            if args.preview { "160k" } else { "320k" },
            "-movflags",
            "+faststart",
        ]);
    }
    // The explicit limit prevents extra loops when copying. When encoding,
    // -shortest can truncate audio because of video frame reordering delay.
    cmd.args(["-t", &format!("{duration:.9}")]);
    if !has_fades && !args.preview && !args.force_encode {
        cmd.arg("-shortest");
    }
    cmd.arg(&temp_file);
    report.diagnostic(format!(
        "WAV duration: {duration:.6} s. Video loops; WAV plays once."
    ));
    report.diagnostic(format!("Video: {}", mode.description()));
    report.diagnostic(format!(
        "Audio: {}",
        if ext == "mkv" {
            "lossless copy"
        } else if args.preview {
            "AAC 160 kbit/s (preview)"
        } else {
            "AAC 320 kbit/s"
        }
    ));
    let command_args = cmd.get_args().map(OsString::from).collect();
    hybrid::stage(
        &ffmpeg,
        command_args,
        duration,
        render_progress,
        mode.phase(),
        report,
    )?;
    if fs::metadata(&temp_file)?.len() == 0 {
        return Err("Empty export".into());
    }

    Ok(ExportResult { output, duration })
}

fn input_path(path: &Path, code: &str) -> Result<PathBuf, EngineError> {
    let resolved = path
        .canonicalize()
        .map_err(|e| EngineError::new(code, "validate_input", Some(path), e))?;
    if !resolved.is_file() {
        return Err(EngineError::new(
            code,
            "validate_input",
            Some(path),
            "Input is not a regular file",
        ));
    }
    Ok(resolved)
}

#[derive(Clone, Copy)]
enum RenderMode {
    Copy,
    Encode,
    Preview,
}
impl RenderMode {
    fn description(self) -> &'static str {
        match self {
            Self::Copy => "copy without re-encoding",
            Self::Encode => "H.264 CRF 16 encoding",
            Self::Preview => "lightweight H.264 preview, up to 640 x 360",
        }
    }
    fn phase(self) -> &'static str {
        match self {
            Self::Copy => "progress.copy",
            Self::Encode => "progress.encode",
            Self::Preview => "progress.preview",
        }
    }
}
