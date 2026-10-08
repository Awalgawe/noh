//! Plain SRT captions rendered in a private worker workspace. Publication is
//! exclusively the controller's responsibility; every video frame is encoded.
use crate::{
    engine::{EngineError, Event, ExportResult, Reporter},
    inspection::{FileStamp, Snapshot},
    subtitle_track::SubtitleTrack,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub const TOTAL_TIMEOUT_SECS: u64 = 8100;
const MAX_DURATION_SECONDS: f64 = 7200.0;
const MAX_ASS_BYTES: usize = 2 * 1024 * 1024;
const FONT: &[u8] = include_bytes!("../assets/NOHCJK.otf");
pub(crate) const FILTER: &str = "subtitles=filename=captions.ass:fontsdir=fonts:wrap_unicode=1";
const UNSUPPORTED_WRAP: &str = "libass wasn't built with ASS_FEATURE_WRAP_UNICODE support";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPlacement {
    #[default]
    Bottom,
    Top,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptionStyle {
    pub size: CaptionSize,
    pub placement: CaptionPlacement,
    #[serde(default)]
    pub safe_area: crate::safe_area::SafeArea,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptionRequest {
    pub source: PathBuf,
    pub subtitles: PathBuf,
    pub output: PathBuf,
    pub ffmpeg: PathBuf,
    pub style: CaptionStyle,
    pub preview: bool,
}

impl CaptionRequest {
    /// Syntax only. An empty FFmpeg path retains the shared default resolver.
    pub fn validate(&self) -> Result<(), EngineError> {
        for (path, code) in [
            (&self.source, "error.caption_input"),
            (&self.subtitles, "error.caption_track"),
            (&self.output, "error.caption_config"),
        ] {
            if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err(error(
                    code,
                    "validate_captions",
                    Some(path),
                    "A nonempty path without NUL characters is required.",
                ));
            }
        }
        if self.ffmpeg.as_os_str().as_encoded_bytes().contains(&0)
            || !self
                .output
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("mp4"))
        {
            return Err(error(
                "error.caption_config",
                "validate_captions",
                Some(&self.output),
                "Caption output must be MP4 and the FFmpeg path must not contain NUL characters.",
            ));
        }
        Ok(())
    }

    /// Metadata stamps invalidate source/SRT/engine mutations during this job.
    /// These are not content hashes or locked input files.
    pub fn snapshot(&self) -> Result<Snapshot, EngineError> {
        self.validate()?;
        let source = stamp(&self.source, "error.caption_input")?;
        let subtitles = stamp(&self.subtitles, "error.caption_track")?;
        if subtitles.bytes > crate::subtitle_srt::MAX_INPUT_BYTES as u64 {
            return Err(error(
                "error.caption_limit",
                "snapshot_captions",
                Some(&self.subtitles),
                "SRT input exceeds one MiB.",
            ));
        }
        let ffmpeg = crate::inspection::resolve_ffmpeg(&self.ffmpeg)?;
        let ffmpeg = stamp(&ffmpeg, "error.ffmpeg")?;
        let snapshot = Snapshot(vec![source, subtitles, ffmpeg]);
        snapshot.verify()?;
        Ok(snapshot)
    }
}

fn error(code: &str, operation: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new(code, operation, path, detail)
}
fn stamp(path: &Path, code: &str) -> Result<FileStamp, EngineError> {
    FileStamp::read(path).map_err(|e| error(code, "snapshot_captions", Some(path), e))
}

/// Execute only within the supervisor-owned workspace. Never publish to output.
pub(crate) fn execute(
    request: &CaptionRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    let started = Instant::now();
    let snapshot = request.snapshot()?;
    let source = &snapshot.0[0].resolved;
    let subtitles = &snapshot.0[1].resolved;
    let ffmpeg = &snapshot.0[2].resolved;
    let workspace = workspace.canonicalize().map_err(|e| {
        error(
            "error.caption_config",
            "caption_workspace",
            Some(workspace),
            e,
        )
    })?;
    let mut report = Reporter::new(emit);
    report.progress(0, "caption.preparing");
    let media = crate::media::inspect_timed(ffmpeg, source).map_err(|e| {
        let mut failure = error(
            "error.caption_input",
            "inspect_caption_source",
            Some(source),
            &e,
        );
        if let Some(original) = e.downcast_ref::<EngineError>() {
            failure.technical = original.technical.clone();
        }
        failure
    })?;
    supported_video(&media.video)?;
    let duration_ms = duration_ms(media.duration)?;
    report.diagnostic(format!(
        "Source presentation timeline: video {:.6}..{:.6} s; first audio {:?}..{:?} s.",
        media.video_start, media.video_end, media.audio_start, media.audio_end,
    ));
    let canvas = Canvas::new(
        media.video.display_width,
        media.video.display_height,
        request.style,
    )?;
    let srt = read_srt(subtitles)?;
    let track = crate::subtitle_srt::parse(&srt, duration_ms)
        .map_err(|e| error("error.caption_track", "parse_captions", Some(subtitles), e))?;
    snapshot.verify()?;
    let quantized = stage(
        &track,
        &canvas,
        media.video_start,
        media.video_end,
        &workspace,
    )?;
    let staged = workspace.join("export.mp4");
    require_absent(&staged)?;
    report.warning("caption.reencode");
    let copy_audio = media.audio_codec.as_deref() == Some("aac");
    if media.audio_codec.is_some() && !copy_audio {
        report.warning("caption.audio_convert");
    }
    if media.audio_streams > 1 {
        report.warning("caption.multiple_audio");
    }
    if quantized {
        report.warning("caption.quantization");
    }
    if media.unknown_final_duration {
        report.warning("caption.unknown_duration");
    }
    report.progress(1, "caption.rendering");
    let command = render_command(
        ffmpeg,
        source,
        &workspace,
        &canvas,
        request.preview,
        copy_audio,
    );
    let timeout = Duration::from_secs(TOTAL_TIMEOUT_SECS)
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            error(
                "error.caption_backend",
                "render_captions",
                Some(source),
                "Caption preparation exceeded the operation deadline.",
            )
        })?;
    let (status, log) = crate::process::stream(command, Some(timeout), false, |line| {
        if let Some(value) = line
            .strip_prefix("out_time_us=")
            .and_then(|s| s.parse::<f64>().ok())
            && value.is_finite()
        {
            let percent =
                1 + (98.0 * (value / 1_000_000.0 / media.duration).clamp(0.0, 1.0)) as u32;
            report.progress(percent, "caption.rendering");
        }
        Ok(())
    })
    .map_err(|e| {
        let mut failure = error(
            "error.caption_backend",
            "render_captions",
            Some(source),
            "FFmpeg could not complete caption rendering.",
        );
        failure.technical = crate::engine::bounded(&e.to_string(), 8192).into();
        failure
    })?;
    if !status.success() || unsupported_renderer(&log) {
        let mut failure = error(
            "error.caption_backend",
            "render_captions",
            Some(source),
            if unsupported_renderer(&log) {
                "FFmpeg lacks required Unicode wrapping or bundled glyph support."
            } else {
                "FFmpeg could not render the captions."
            },
        );
        failure.technical = crate::engine::bounded(&log, 8192).into();
        return Err(failure);
    }
    snapshot.verify()?;
    let metadata = fs::symlink_metadata(&staged).map_err(|e| {
        error(
            "error.caption_result",
            "validate_caption_output",
            Some(&staged),
            e,
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(error(
            "error.caption_result",
            "validate_caption_output",
            Some(&staged),
            "Caption output must be a nonempty regular MP4 file.",
        ));
    }
    report.progress(99, "caption.rendering");
    Ok(ExportResult {
        output: request.output.clone(),
        duration: media.duration,
    })
}

fn duration_ms(seconds: f64) -> Result<u64, EngineError> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(error(
            "error.caption_input",
            "caption_duration",
            None,
            "Source playback duration is unavailable or empty.",
        ));
    }
    if seconds > MAX_DURATION_SECONDS {
        return Err(error(
            "error.caption_limit",
            "caption_duration",
            None,
            "Caption rendering supports sources up to two hours.",
        ));
    }
    Ok((seconds * 1000.0).ceil() as u64)
}

pub(crate) fn supported_video(video: &crate::media::MediaInfo) -> Result<(), EngineError> {
    let description = video.description.to_ascii_lowercase();
    if ["smpte2084", "arib-std-b67", "bt2020"]
        .iter()
        .any(|tag| description.contains(tag))
    {
        return Err(error(
            "error.caption_input",
            "caption_color",
            None,
            "HDR/PQ/HLG/BT.2020 sources require a supported tone-mapping workflow before caption rendering; this renderer produces eight-bit SDR.",
        ));
    }
    let angle = video.rotation.rem_euclid(90.0);
    if !angle.is_finite() || angle.min(90.0 - angle) > 0.01 {
        return Err(error(
            "error.caption_input",
            "caption_rotation",
            None,
            "Caption rendering supports only unrotated or quarter-turn display geometry.",
        ));
    }
    Ok(())
}

pub(crate) fn read_srt(path: &Path) -> Result<String, EngineError> {
    let input = File::open(path)
        .map_err(|e| error("error.caption_track", "read_captions", Some(path), e))?;
    let mut bytes = Vec::new();
    input
        .take(crate::subtitle_srt::MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error("error.caption_track", "read_captions", Some(path), e))?;
    if bytes.len() > crate::subtitle_srt::MAX_INPUT_BYTES {
        return Err(error(
            "error.caption_limit",
            "read_captions",
            Some(path),
            "SRT input exceeds one MiB.",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|e| error("error.caption_track", "read_captions", Some(path), e))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| error("error.caption_config", "stage_captions", Some(path), e))?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|e| error("error.caption_config", "stage_captions", Some(path), e))
}
fn require_absent(path: &Path) -> Result<(), EngineError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(
            "error.caption_result",
            "stage_captions",
            Some(path),
            e,
        )),
        Ok(_) => Err(error(
            "error.caption_result",
            "stage_captions",
            Some(path),
            "Caption workspace output already exists.",
        )),
    }
}

pub(crate) struct Canvas {
    width: u32,
    height: u32,
    font_size: f32,
    margin_left: u32,
    margin_right: u32,
    margin_vertical: u32,
    outline: u32,
    max_width: f32,
    max_height: f32,
    alignment: u8,
}
impl Canvas {
    pub(crate) fn new(width: f64, height: f64, style: CaptionStyle) -> Result<Self, EngineError> {
        let even = |value: f64| {
            if !value.is_finite() || !(1.0..=16384.0).contains(&value) {
                return Err(error(
                    "error.caption_limit",
                    "caption_geometry",
                    None,
                    "Displayed dimensions must be finite and between one and 16384 pixels.",
                ));
            }
            Ok(((value / 2.0).round().max(1.0) * 2.0) as u32)
        };
        let (width, height) = (even(width)?, even(height)?);
        let short = width.min(height) as f32;
        let factor = match style.size {
            CaptionSize::Small => 0.04,
            CaptionSize::Medium => 0.055,
            CaptionSize::Large => 0.07,
        };
        let font_size = (short * factor).round().max(8.0);
        let margin = (short * 0.05).ceil().max(4.0) as u32;
        let outline = (font_size / 16.0).ceil().max(1.0) as u32;
        // Libass requests real ascender/descender height, while layout measures
        // em-sized nominal advances: same numeric size overestimates this
        // bundled font's rendered width/height. Reserve additional raster/ink
        // overhang and outline instead of pretending the two scales are equal.
        let reserve = outline as f32 + (font_size * 0.10).ceil() + 2.0;
        let [left, top, right, bottom] = style
            .safe_area
            .insets(width, height)
            .map(|insets| insets.map(|v| v + reserve.ceil() as u32))
            .unwrap_or([margin; 4]);
        let max_width = width as f32 - (left + right) as f32 - 2.0 * reserve;
        let max_height = height as f32 - (top + bottom) as f32 - 2.0 * reserve;
        if max_width <= 0.0 || max_height <= 0.0 {
            return Err(error(
                "error.caption_layout",
                "caption_geometry",
                None,
                "The displayed canvas is too small for captions with safety margins.",
            ));
        }
        Ok(Self {
            width,
            height,
            font_size,
            margin_left: left,
            margin_right: right,
            margin_vertical: if style.placement == CaptionPlacement::Top {
                top
            } else {
                bottom
            },
            outline,
            max_width,
            max_height,
            alignment: if style.placement == CaptionPlacement::Top {
                8
            } else {
                2
            },
        })
    }
}

/// One pass over original text. Inserted escapes are never processed again.
/// U+2060 prevents literal backslashes becoming ASS N/n/h/brace escapes; its
/// zero-width native libass behavior is covered by retained rendering probes.
fn literal(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\u{2060}"),
            '{' => output.push_str("\\{"),
            '}' => output.push_str("\\}"),
            '\n' => output.push_str("\\N"),
            _ => output.push(character),
        }
    }
    output
}
fn ass_time(milliseconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        milliseconds / 3_600_000,
        milliseconds / 60_000 % 60,
        milliseconds / 1000 % 60,
        milliseconds / 10 % 100
    )
}
fn rounded(milliseconds: u64) -> u64 {
    // SubtitleTrack's duration validation bounds addition safely.
    (milliseconds + 5) / 10 * 10
}
fn intersects(start: u64, end: u64, video_start: f64, video_end: f64) -> bool {
    start as f64 / 1000.0 < video_end && end as f64 / 1000.0 > video_start
}
fn compose(
    track: &SubtitleTrack,
    canvas: &Canvas,
    video_start: f64,
    video_end: f64,
) -> Result<(String, bool), EngineError> {
    track
        .validate()
        .map_err(|e| error("error.caption_track", "compose_captions", None, e))?;
    if track.cues.is_empty() {
        return Err(error(
            "error.caption_track",
            "compose_captions",
            None,
            "The subtitle track contains no cues to render.",
        ));
    }
    if !video_start.is_finite() || !video_end.is_finite() || video_end <= video_start {
        return Err(error(
            "error.caption_input",
            "compose_captions",
            None,
            "Source video timeline is unavailable.",
        ));
    }
    let mut output = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {}\nPlayResY: {}\nLayoutResX: {}\nLayoutResY: {}\nScaledBorderAndShadow: yes\nWrapStyle: 2\nYCbCr Matrix: None\n[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\nStyle: Default,NOH CJK,{},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,{},0,{},{},{},{},1\n[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n",
        canvas.width,
        canvas.height,
        canvas.width,
        canvas.height,
        canvas.font_size,
        canvas.outline,
        canvas.alignment,
        canvas.margin_left,
        canvas.margin_right,
        canvas.margin_vertical
    );
    let (mut changed, mut previous_end) = (false, 0);
    for (index, cue) in track.cues.iter().enumerate() {
        let (start, end) = (rounded(cue.start_ms), rounded(cue.end_ms));
        if start >= end || start < previous_end {
            return Err(error(
                "error.caption_track",
                "quantize_captions",
                None,
                format!(
                    "Cue {} collapses or overlaps after rounding to centiseconds.",
                    index + 1
                ),
            ));
        }
        if !intersects(cue.start_ms, cue.end_ms, video_start, video_end)
            || !intersects(start, end, video_start, video_end)
        {
            return Err(error(
                "error.caption_track",
                "compose_captions",
                None,
                format!(
                    "Cue {} has no overlap with the actual video timeline.",
                    index + 1
                ),
            ));
        }
        changed |= start != cue.start_ms || end != cue.end_ms;
        previous_end = end;
        let lines = crate::caption_layout::layout_text(
            &cue.text,
            canvas.font_size,
            canvas.max_width,
            canvas.max_height,
        )
        .map_err(|e| {
            error(
                "error.caption_layout",
                "layout_captions",
                None,
                format!("Cue {}: {e}", index + 1),
            )
        })?;
        let text = lines
            .iter()
            .map(|line| literal(line))
            .collect::<Vec<_>>()
            .join("\\N");
        writeln!(
            output,
            "Dialogue: 0,{},{},Default,,0,0,0,,{}",
            ass_time(start),
            ass_time(end),
            text
        )
        .expect("Writing to String is infallible");
        if output.len() > MAX_ASS_BYTES {
            return Err(error(
                "error.caption_limit",
                "compose_captions",
                None,
                "Generated captions exceed the supported script size.",
            ));
        }
    }
    Ok((output, changed))
}

/// Stage the same qualified literal script and font for full videos and shorts.
/// The caller owns the private workspace and chooses the final display canvas.
pub(crate) fn stage(
    track: &SubtitleTrack,
    canvas: &Canvas,
    video_start: f64,
    video_end: f64,
    workspace: &Path,
) -> Result<bool, EngineError> {
    let (script, quantized) = compose(track, canvas, video_start, video_end)?;
    write_new(&workspace.join("captions.ass"), script.as_bytes())?;
    let fonts = workspace.join("fonts");
    fs::create_dir(&fonts).map_err(|e| {
        error(
            "error.caption_config",
            "stage_caption_font",
            Some(&fonts),
            e,
        )
    })?;
    write_new(&fonts.join("NOHCJK.otf"), FONT)?;
    Ok(quantized)
}

fn render_command(
    ffmpeg: &Path,
    source: &Path,
    workspace: &Path,
    canvas: &Canvas,
    preview: bool,
    copy_audio: bool,
) -> Command {
    let mut filter = format!("scale={}:{},setsar=1,{FILTER}", canvas.width, canvas.height);
    if preview {
        let ratio = (960.0 / f64::from(canvas.width))
            .min(540.0 / f64::from(canvas.height))
            .min(1.0);
        let width = ((f64::from(canvas.width) * ratio / 2.0).floor().max(1.0) * 2.0) as u32;
        let height = ((f64::from(canvas.height) * ratio / 2.0).floor().max(1.0) * 2.0) as u32;
        write!(filter, ",scale={width}:{height}").expect("Writing to String is infallible");
    }
    let mut cmd = crate::command(ffmpeg);
    cmd.current_dir(workspace)
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-n",
            "-xerror",
            "-err_detect",
            "explode",
            "-i",
        ])
        .arg(source)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-vf",
            &filter,
            "-fps_mode",
            "passthrough",
            "-enc_time_base:v",
            "filter",
            "-c:v",
            "libx264",
            // With irregular PTS, the qualified libx264/MP4 path can shorten
            // the container timeline when output pictures are reordered.
            // Input B-frames remain supported; this affects the new encode.
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            if preview { "24" } else { "16" },
            "-preset",
            "medium",
            "-c:a",
            if copy_audio { "copy" } else { "aac" },
        ]);
    if !copy_audio {
        cmd.args(["-b:a", "320k"]);
    }
    cmd.args([
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "export.mp4",
    ]);
    cmd
}
pub(crate) fn unsupported_renderer(log: &str) -> bool {
    log.contains(UNSUPPORTED_WRAP)
        || log.contains("failed to find any fallback with glyph")
        || log.contains("Glyph ") && log.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle_track::SubtitleCue;

    fn request() -> CaptionRequest {
        CaptionRequest {
            source: "source.mp4".into(),
            subtitles: "captions.srt".into(),
            output: "out.mp4".into(),
            ffmpeg: "".into(),
            style: CaptionStyle::default(),
            preview: false,
        }
    }
    fn track(start: u64, end: u64, text: &str) -> SubtitleTrack {
        SubtitleTrack {
            language: None,
            duration_ms: 1000,
            cues: vec![SubtitleCue {
                start_ms: start,
                end_ms: end,
                text: text.into(),
            }],
        }
    }

    #[test]
    fn request_validation_is_syntax_only_and_serialization_is_closed() {
        assert!(request().validate().is_ok());
        let mut req = request();
        req.output = "out.mkv".into();
        assert_eq!(req.validate().unwrap_err().code, "error.caption_config");
        req = request();
        req.subtitles = "".into();
        assert_eq!(req.validate().unwrap_err().code, "error.caption_track");
        let mut value = serde_json::to_value(request()).unwrap();
        assert_eq!(value["style"]["size"], "medium");
        assert_eq!(value["style"]["placement"], "bottom");
        value["extra"] = true.into();
        assert!(serde_json::from_value::<CaptionRequest>(value).is_err());
    }

    #[test]
    fn literal_escaping_cannot_emit_user_override_or_newline_sequences() {
        assert_eq!(
            literal("{\\pos(0,0)}\\N\\n\\h\nnext"),
            "\\{\\\u{2060}pos(0,0)\\}\\\u{2060}N\\\u{2060}n\\\u{2060}h\\Nnext"
        );
        assert_eq!(
            literal("comma, colon: apostrophe' 日本語"),
            "comma, colon: apostrophe' 日本語"
        );
    }

    #[test]
    fn quantization_is_explicit_and_collapsed_or_nonvideo_cues_fail() {
        let canvas = Canvas::new(640.0, 360.0, CaptionStyle::default()).unwrap();
        let (script, changed) = compose(&track(6, 994, "Text"), &canvas, 0.0, 1.0).unwrap();
        assert!(changed);
        assert!(script.contains("00:00:00.01,00:00:00.99"));
        assert!(
            !compose(&track(0, 1000, "Text"), &canvas, 0.0, 1.0)
                .unwrap()
                .1
        );
        assert_eq!(
            compose(&track(1, 4, "Text"), &canvas, 0.0, 1.0)
                .unwrap_err()
                .code,
            "error.caption_track"
        );
        assert!(compose(&track(0, 200, "Text"), &canvas, 0.2, 1.0).is_err());
        assert!(compose(&track(900, 1000, "Text"), &canvas, 0.0, 0.9).is_err());
        assert!(compose(&track(190, 210, "Text"), &canvas, 0.2, 1.0).is_ok());
        assert_eq!(ass_time(3_600_010), "01:00:00.01");
    }

    #[test]
    fn presets_wrap_literal_multilingual_text_and_reject_overflow_glyphs() {
        for (w, h) in [(640.0, 360.0), (360.0, 640.0), (1920.0, 1080.0)] {
            let mut previous = 0.0;
            for size in [CaptionSize::Small, CaptionSize::Medium, CaptionSize::Large] {
                let canvas = Canvas::new(
                    w,
                    h,
                    CaptionStyle {
                        size,
                        placement: CaptionPlacement::Top,
                        ..CaptionStyle::default()
                    },
                )
                .unwrap();
                assert!(canvas.font_size > previous);
                previous = canvas.font_size;
                for text in ["Été Grüße", "字幕 한국어 中文", &"W".repeat(90)] {
                    let lines = crate::caption_layout::layout_text(
                        text,
                        canvas.font_size,
                        canvas.max_width,
                        canvas.max_height,
                    )
                    .unwrap();
                    assert_eq!(lines.concat(), text);
                    assert!(lines.iter().all(|s| {
                        crate::caption_layout::measure_line(s, canvas.font_size).unwrap()
                            <= canvas.max_width
                    }));
                    assert!(
                        compose(&track(0, 1000, text), &canvas, 0.0, 1.0)
                            .unwrap()
                            .0
                            .contains("WrapStyle: 2")
                    );
                }
                assert_eq!(
                    compose(&track(0, 1000, "\u{10ffff}"), &canvas, 0.0, 1.0)
                        .unwrap_err()
                        .code,
                    "error.caption_layout"
                );
                assert!(
                    compose(
                        &track(0, 1000, &vec!["Line"; 100].join("\n")),
                        &canvas,
                        0.0,
                        1.0
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn command_keeps_source_outside_filter_and_preserves_timeline() {
        let source = Path::new("C:\\folder 'comma, 日本語\\source.mp4");
        let canvas = Canvas::new(1080.0, 1920.0, CaptionStyle::default()).unwrap();
        let cmd = render_command(
            Path::new("ffmpeg"),
            source,
            Path::new("workspace"),
            &canvas,
            true,
            true,
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args[args.iter().position(|s| s == "-i").unwrap() + 1],
            source.to_string_lossy()
        );
        let filter = &args[args.iter().position(|s| s == "-vf").unwrap() + 1];
        assert!(filter.ends_with(",scale=302:540"));
        assert!(!filter.contains("日本語"));
        assert!(!filter.contains("setpts"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-fps_mode", "passthrough"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-c:a", "copy"]));
        for forbidden in ["-t", "-shortest", "-copyts", "-r"] {
            assert!(!args.iter().any(|s| s == forbidden));
        }
        assert_eq!(args.last().unwrap(), "export.mp4");
    }

    #[test]
    fn duration_geometry_and_renderer_limits_fail_honestly() {
        assert_eq!(duration_ms(0.0001).unwrap(), 1);
        assert_eq!(duration_ms(7200.0).unwrap(), 7_200_000);
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, 7200.001] {
            assert!(duration_ms(invalid).is_err());
        }
        assert!(Canvas::new(f64::INFINITY, 360.0, CaptionStyle::default()).is_err());
        assert!(Canvas::new(2.0, 2.0, CaptionStyle::default()).is_err());
        assert!(unsupported_renderer(UNSUPPORTED_WRAP));
        assert!(unsupported_renderer(
            "Glyph 0xE000 not found, selecting one more font"
        ));
        assert!(!unsupported_renderer("Using font provider directwrite"));
        let mut video = crate::media::MediaInfo::default();
        assert!(supported_video(&video).is_ok());
        for rotation in [90.0, -90.0, 180.0, 270.0] {
            video.rotation = rotation;
            assert!(supported_video(&video).is_ok());
        }
        video.rotation = 45.0;
        assert_eq!(
            supported_video(&video).unwrap_err().code,
            "error.caption_input"
        );
        video.rotation = 0.0;
        for description in ["yuv420p10le(bt2020nc/bt2020/smpte2084)", "arib-std-b67"] {
            video.description = description.into();
            assert_eq!(
                supported_video(&video).unwrap_err().code,
                "error.caption_input"
            );
        }
        let canvas = Canvas::new(640.0, 360.0, CaptionStyle::default()).unwrap();
        let empty = SubtitleTrack {
            language: None,
            duration_ms: 1000,
            cues: vec![],
        };
        assert_eq!(
            compose(&empty, &canvas, 0.0, 1.0).unwrap_err().code,
            "error.caption_track"
        );
    }

    #[test]
    fn snapshots_invalidate_each_input_and_bound_srt_reads() {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request();
        req.source = temp.path().join("source");
        req.subtitles = temp.path().join("text.srt");
        req.ffmpeg = temp.path().join("ffmpeg.exe");
        for path in [&req.source, &req.subtitles, &req.ffmpeg] {
            fs::write(path, b"initial").unwrap();
        }
        for path in [&req.source, &req.subtitles, &req.ffmpeg] {
            let snap = req.snapshot().unwrap();
            fs::write(path, b"changed and longer").unwrap();
            assert!(snap.verify().is_err());
        }
        fs::write(
            &req.subtitles,
            vec![b'x'; crate::subtitle_srt::MAX_INPUT_BYTES + 1],
        )
        .unwrap();
        assert_eq!(req.snapshot().unwrap_err().code, "error.caption_limit");
        assert_eq!(
            read_srt(&req.subtitles).unwrap_err().code,
            "error.caption_limit"
        );
        let stage = temp.path().join("export.mp4");
        fs::write(&stage, b"retain").unwrap();
        assert!(require_absent(&stage).is_err());
        assert_eq!(fs::read(stage).unwrap(), b"retain");
    }
}
