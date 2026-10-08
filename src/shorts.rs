//! Explicit selected-interval vertical video requests, independent of montage.
use crate::{
    captions::CaptionStyle,
    engine::{EngineError, Event, ExportResult, Reporter},
    inspection::{FileStamp, Snapshot},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub const MAX_SOURCE_MS: u64 = 7_200_000;
pub const OUTPUT_WIDTH: u32 = 1080;
pub const OUTPUT_HEIGHT: u32 = 1920;
pub const PREVIEW_WIDTH: u32 = 360;
pub const PREVIEW_HEIGHT: u32 = 640;
pub const TOTAL_TIMEOUT_SECS: u64 = 8100;

// Bound the intermediate picture before a centered crop, including unusual SAR.
const MAX_DIMENSION: f64 = 16384.0;
const MAX_SCALED_PIXELS: u64 = 64_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Preserve the complete picture, centered on a black vertical canvas.
    #[default]
    Pad,
    /// Fill the vertical canvas, removing equal amounts from opposite sides.
    Crop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortCaptions {
    pub subtitles: PathBuf,
    pub style: CaptionStyle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortRequest {
    pub source: PathBuf,
    pub output: PathBuf,
    pub ffmpeg: PathBuf,
    /// Half-open interval in the source's common playback timeline.
    pub start_ms: u64,
    pub end_ms: u64,
    pub framing: Framing,
    pub captions: Option<ShortCaptions>,
    pub preview: bool,
}

fn error(code: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new(code, "validate_short", path, detail)
}

fn path_valid(path: &Path) -> bool {
    !path.as_os_str().is_empty() && !path.as_os_str().as_encoded_bytes().contains(&0)
}

impl ShortRequest {
    /// Syntax and bounded interval only. Media boundaries need worker inspection.
    pub fn validate(&self) -> Result<(), EngineError> {
        if !path_valid(&self.source) {
            return Err(error(
                "error.short_input",
                Some(&self.source),
                "A nonempty source path without NUL characters is required.",
            ));
        }
        if !path_valid(&self.output)
            || !self
                .output
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("mp4"))
            || self.ffmpeg.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err(error(
                "error.short_config",
                Some(&self.output),
                "Short output must be MP4 and paths must not contain NUL characters.",
            ));
        }
        if self.start_ms >= self.end_ms || self.end_ms > MAX_SOURCE_MS {
            return Err(error(
                "error.short_range",
                Some(&self.source),
                "Select a nonempty interval within the first two hours of the source.",
            ));
        }
        if let Some(captions) = &self.captions
            && !path_valid(&captions.subtitles)
        {
            return Err(error(
                "error.caption_track",
                Some(&captions.subtitles),
                "A nonempty reviewed SRT path without NUL characters is required.",
            ));
        }
        Ok(())
    }

    /// Stable order: source, optional SRT, resolved FFmpeg. Metadata stamps are
    /// invalidation evidence, not content hashes or locked copies of inputs.
    pub fn snapshot(&self) -> Result<Snapshot, EngineError> {
        self.validate()?;
        let stamp =
            |path: &Path, code: &str| FileStamp::read(path).map_err(|e| error(code, Some(path), e));
        let mut inputs = vec![stamp(&self.source, "error.short_input")?];
        if let Some(captions) = &self.captions {
            let subtitles = stamp(&captions.subtitles, "error.caption_track")?;
            if subtitles.bytes > crate::subtitle_srt::MAX_INPUT_BYTES as u64 {
                return Err(error(
                    "error.caption_limit",
                    Some(&captions.subtitles),
                    "SRT input exceeds one MiB.",
                ));
            }
            inputs.push(subtitles);
        }
        let ffmpeg = crate::inspection::resolve_ffmpeg(&self.ffmpeg)?;
        inputs.push(stamp(&ffmpeg, "error.ffmpeg")?);
        let snapshot = Snapshot(inputs);
        snapshot.verify()?;
        Ok(snapshot)
    }
}

/// Execute inside the controller's private directory; publication is its duty.
pub(crate) fn execute(
    request: &ShortRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    let started = Instant::now();
    let snapshot = request.snapshot()?;
    let source = &snapshot.0[0].resolved;
    let ffmpeg = &snapshot
        .0
        .last()
        .expect("Snapshot includes FFmpeg")
        .resolved;
    let workspace = workspace.canonicalize().map_err(|e| {
        EngineError::new("error.short_config", "short_workspace", Some(workspace), e)
    })?;
    let mut report = Reporter::new(emit);
    report.progress(0, "short.preparing");
    let media = crate::media::inspect_timed(ffmpeg, source)
        .map_err(|e| media_error("error.short_input", "inspect_short_source", source, &*e))?;
    crate::captions::supported_video(&media.video).map_err(|mut e| {
        e.code = "error.short_input".into();
        e.operation = "short_source".into();
        e.path = Some(source.clone());
        e.detail = e
            .detail
            .replace("caption rendering", "vertical rendering")
            .replace("Caption rendering", "Vertical rendering")
            .into();
        e
    })?;
    let source_ms = source_duration(media.duration)?;
    let start = request.start_ms as f64 / 1000.0;
    let end = request.end_ms as f64 / 1000.0;
    let span = end - start;
    if request.end_ms > source_ms
        || end > media.video_end + 0.001
        || start >= media.video_end
        || end <= media.video_start
    {
        return Err(error(
            "error.short_range",
            Some(source),
            "Select an interval within the known video timeline; audio-only tails cannot form a vertical clip.",
        ));
    }
    let geometry = Geometry::new(
        media.video.display_width,
        media.video.display_height,
        request.framing,
    )?;
    let mut has_captions = false;
    if let Some(captions) = &request.captions {
        let path = &snapshot.0[1].resolved;
        let text = crate::captions::read_srt(path)?;
        let track = crate::subtitle_srt::parse(&text, source_ms)
            .map_err(|e| error("error.caption_track", Some(path), e))?;
        let track = track
            .trim(request.start_ms, request.end_ms)
            .map_err(|e| error("error.caption_track", Some(path), e))?;
        if track.cues.is_empty() {
            report.warning("short.no_captions");
        } else {
            let canvas = crate::captions::Canvas::new(
                f64::from(OUTPUT_WIDTH),
                f64::from(OUTPUT_HEIGHT),
                captions.style,
            )?;
            if crate::captions::stage(&track, &canvas, 0.0, span, &workspace)? {
                report.warning("caption.quantization");
            }
            has_captions = true;
        }
    }
    snapshot.verify()?;
    let staged = workspace.join("export.mp4");
    match fs::symlink_metadata(&staged) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error("error.short_result", Some(&staged), e)),
        Ok(_) => {
            return Err(error(
                "error.short_result",
                Some(&staged),
                "Short workspace output already exists.",
            ));
        }
    }
    report.warning("short.reencode");
    if request.framing == Framing::Crop {
        report.warning("short.crop");
    }
    if media.audio_streams > 1 {
        report.warning("caption.multiple_audio");
    }
    if media.unknown_final_duration {
        report.warning("caption.unknown_duration");
    }
    report.diagnostic(format!(
        "Source playback timeline: video {:.6}..{:.6} s, audio {:?}..{:?} s; selection {start:.3}..{end:.3} s.",
        media.video_start, media.video_end, media.audio_start, media.audio_end,
    ));
    report.progress(1, "short.rendering");
    let command = render_command(request, ffmpeg, source, &workspace, &geometry, has_captions);
    let timeout = Duration::from_secs(TOTAL_TIMEOUT_SECS)
        .checked_sub(started.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| {
            error(
                "error.short_backend",
                Some(source),
                "Short preparation exceeded its deadline.",
            )
        })?;
    let (status, log) = crate::process::stream(command, Some(timeout), false, |line| {
        if let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|s| s.parse::<f64>().ok())
            && us.is_finite()
        {
            report.progress(
                1 + (95.0 * (us / 1_000_000.0 / span).clamp(0.0, 1.0)) as u32,
                "short.rendering",
            );
        }
        Ok(())
    })
    .map_err(|e| media_error("error.short_backend", "render_short", source, &*e))?;
    if !status.success() || has_captions && crate::captions::unsupported_renderer(&log) {
        let mut failure = EngineError::new(
            "error.short_backend",
            "render_short",
            Some(source),
            "FFmpeg could not render the selected interval. Inspect the technical log for decoder or caption renderer details.",
        );
        failure.technical = crate::engine::bounded(&log, 8192).into();
        return Err(failure);
    }
    report.progress(97, "short.verifying");
    let metadata =
        fs::symlink_metadata(&staged).map_err(|e| error("error.short_result", Some(&staged), e))?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(error(
            "error.short_result",
            Some(&staged),
            "Short output must be a nonempty regular MP4 file.",
        ));
    }
    let output = crate::media::inspect_timed_raw(ffmpeg, &staged)
        .map_err(|e| media_error("error.short_result", "inspect_short_output", &staged, &*e))?;
    verify_output(&output, request.preview, span)?;
    snapshot.verify()?;
    if output.video_start.abs() > 0.001 || (output.video_end - span).abs() > 0.001 {
        report.warning(&format!(
            "short.boundaries|{:.3}|{:.3}|{span:.3}",
            output.video_start, output.video_end
        ));
    }
    if let Some(audio_end) = output.audio_end
        && audio_end > span + 0.001
    {
        report.warning(&format!(
            "short.audio_padding|{:.0}|{span:.3}",
            (audio_end - span) * 1000.0
        ));
    }
    report.diagnostic(format!(
        "Rendered container timeline: video {:.6}..{:.6} s, audio {:?}..{:?} s; {} video packets.",
        output.video_start,
        output.video_end,
        output.audio_start,
        output.audio_end,
        output.video.frames,
    ));
    report.progress(99, "short.verifying");
    Ok(ExportResult {
        output: request.output.clone(),
        duration: output.duration,
    })
}

fn source_duration(seconds: f64) -> Result<u64, EngineError> {
    if !seconds.is_finite() || seconds <= 0.0 || seconds * 1000.0 > MAX_SOURCE_MS as f64 {
        return Err(error(
            "error.short_limit",
            None,
            "Vertical clips require a known positive source duration of at most two hours.",
        ));
    }
    Ok((seconds * 1000.0).ceil() as u64)
}

fn media_error(
    code: &str,
    operation: &str,
    path: &Path,
    source: &(dyn std::error::Error + 'static),
) -> EngineError {
    let mut error = EngineError::new(code, operation, Some(path), source);
    if let Some(original) = source.downcast_ref::<EngineError>() {
        error.technical = original.technical.clone();
    }
    error
}

fn verify_output(
    output: &crate::media::TimedMediaInfo,
    preview: bool,
    span: f64,
) -> Result<(), EngineError> {
    let (width, height) = if preview {
        (PREVIEW_WIDTH, PREVIEW_HEIGHT)
    } else {
        (OUTPUT_WIDTH, OUTPUT_HEIGHT)
    };
    if output.video.width != width
        || output.video.height != height
        || !output.video.square_pixels
        || output.video.rotation.abs() > 0.01
        || output.video.codec != "h264"
        || output.video.frames == 0
        || output.unknown_final_duration
        || output.video_start < -0.001
        || output.video_end <= output.video_start
        || output.video_end > span + 0.001
        || output.duration <= 0.0
        || !output.duration.is_finite()
    {
        return Err(error(
            "error.short_result",
            None,
            "The rendered video has no selected frames, unsupported geometry, or unverifiable/out-of-range presentation bounds. Nothing was published.",
        ));
    }
    Ok(())
}

pub(crate) struct Geometry {
    width: u32,
    height: u32,
    framing: Framing,
}
impl Geometry {
    pub(crate) fn new(width: f64, height: f64, framing: Framing) -> Result<Self, EngineError> {
        if !width.is_finite()
            || !height.is_finite()
            || !(1.0..=MAX_DIMENSION).contains(&width)
            || !(1.0..=MAX_DIMENSION).contains(&height)
        {
            return Err(error(
                "error.short_limit",
                None,
                "Displayed source dimensions must be between one and 16384 pixels.",
            ));
        }
        let (x, y) = (
            f64::from(OUTPUT_WIDTH) / width,
            f64::from(OUTPUT_HEIGHT) / height,
        );
        let ratio = match framing {
            Framing::Pad => x.min(y),
            Framing::Crop => x.max(y),
        };
        let even = |n: f64| match framing {
            Framing::Pad => (n / 2.0).floor().max(1.0) * 2.0,
            Framing::Crop => (n / 2.0).ceil() * 2.0,
        };
        let (width, height) = (even(width * ratio), even(height * ratio));
        if width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || width * height > MAX_SCALED_PIXELS as f64
        {
            return Err(error(
                "error.short_limit",
                None,
                "Centered cropping would require an oversized intermediate picture. Choose padding or a less extreme aspect ratio.",
            ));
        }
        Ok(Self {
            width: width as u32,
            height: height as u32,
            framing,
        })
    }
    pub(crate) fn filter(&self, captions: bool, preview: bool) -> String {
        let mut filter = format!("scale={}:{},setsar=1,", self.width, self.height);
        filter.push_str(match self.framing {
            Framing::Pad => "pad=1080:1920:(ow-iw)/2:(oh-ih)/2:color=black",
            Framing::Crop => "crop=1080:1920:(iw-ow)/2:(ih-oh)/2",
        });
        if captions {
            filter.push(',');
            filter.push_str(crate::captions::FILTER);
        }
        if preview {
            filter.push_str(",scale=360:640");
        }
        filter
    }
    /// The same centered geometry evaluated at the monitor's resolution.
    #[cfg(feature = "gui")]
    pub(crate) fn monitor_filter(&self, width: u32, height: u32) -> String {
        let scale = f64::from(height) / 1920.0;
        let even = |v: u32| (f64::from(v) * scale / 2.0).round().max(1.0) as u32 * 2;
        let (w, h) = (even(self.width), even(self.height));
        match self.framing {
            Framing::Pad => format!(
                "scale={}:{},setsar=1,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black",
                w.min(width),
                h.min(height)
            ),
            Framing::Crop => format!(
                "scale={}:{},setsar=1,crop={width}:{height}:(iw-ow)/2:(ih-oh)/2",
                w.max(width),
                h.max(height)
            ),
        }
    }
}

fn render_command(
    request: &ShortRequest,
    ffmpeg: &Path,
    source: &Path,
    workspace: &Path,
    geometry: &Geometry,
    captions: bool,
) -> Command {
    let start = format!("{:.3}", request.start_ms as f64 / 1000.0);
    let span = format!("{:.3}", (request.end_ms - request.start_ms) as f64 / 1000.0);
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
            "-ss",
            &start,
            "-i",
        ])
        .arg(source)
        .args([
            "-t",
            &span,
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-vf",
            &geometry.filter(captions, request.preview),
            "-fps_mode",
            "passthrough",
            "-enc_time_base:v",
            "filter",
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            if request.preview { "24" } else { "16" },
            "-preset",
            "medium",
            "-c:a",
            "aac",
            "-b:a",
            "320k",
            "-bsf:v",
        ])
        // Qualify the downstream MP4 duration, not just the BSF expression:
        // using PTS alone subtracts the initial cut residual twice in this path.
        .arg(format!(
            "setts=duration='min(DURATION,{span}/TB-(PTS-STARTPTS))'"
        ))
        .args([
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
            "export.mp4",
        ]);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ShortRequest {
        ShortRequest {
            source: "source.mp4".into(),
            output: "short.mp4".into(),
            ffmpeg: "".into(),
            start_ms: 135,
            end_ms: 1775,
            framing: Framing::Pad,
            captions: None,
            preview: false,
        }
    }

    #[test]
    fn framing_bounds_intermediate_memory_and_preserves_fit_or_fill() {
        for (width, height) in [(1920.0, 1080.0), (1080.0, 1920.0), (720.0, 576.0)] {
            let pad = Geometry::new(width, height, Framing::Pad).unwrap();
            assert!(pad.width <= OUTPUT_WIDTH && pad.height <= OUTPUT_HEIGHT);
            let crop = Geometry::new(width, height, Framing::Crop).unwrap();
            assert!(crop.width >= OUTPUT_WIDTH && crop.height >= OUTPUT_HEIGHT);
            let filter = crop.filter(true, true);
            assert!(filter.find("crop=").unwrap() < filter.find("subtitles=").unwrap());
            assert!(filter.ends_with(",scale=360:640"));
        }
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, 16385.0] {
            assert!(Geometry::new(value, 1080.0, Framing::Pad).is_err());
        }
        assert!(Geometry::new(16384.0, 1.0, Framing::Pad).is_ok());
        assert!(Geometry::new(16384.0, 1.0, Framing::Crop).is_err());
        for seconds in [0.0, -1.0, f64::NAN, f64::INFINITY, 7200.001] {
            assert!(source_duration(seconds).is_err());
        }
    }

    #[test]
    fn short_request_requires_a_bounded_nonempty_interval_and_mp4_output() {
        let mut request = request();
        assert!(request.validate().is_ok());
        request.output = "東京.MP4".into();
        assert!(request.validate().is_ok());
        for (start, end) in [(0, 0), (2, 1), (0, MAX_SOURCE_MS + 1), (u64::MAX, u64::MAX)] {
            request.start_ms = start;
            request.end_ms = end;
            assert_eq!(request.validate().unwrap_err().code, "error.short_range");
        }
        request.start_ms = 0;
        request.end_ms = MAX_SOURCE_MS;
        assert!(request.validate().is_ok());
        request.output = "short.mkv".into();
        assert_eq!(request.validate().unwrap_err().code, "error.short_config");
        request.output = "short.mp4".into();
        request.captions = Some(ShortCaptions {
            subtitles: "".into(),
            style: CaptionStyle::default(),
        });
        assert_eq!(request.validate().unwrap_err().code, "error.caption_track");
        request.source = "".into();
        assert_eq!(request.validate().unwrap_err().code, "error.short_input");
    }

    #[test]
    fn short_wire_rejects_unknown_fields_and_invalid_framing_or_caption_style() {
        let original = request();
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(
            serde_json::from_value::<ShortRequest>(json.clone()).unwrap(),
            original
        );
        let mut unknown = json.clone();
        unknown["fps"] = 30.into();
        assert!(serde_json::from_value::<ShortRequest>(unknown).is_err());
        let mut invalid = json.clone();
        invalid["framing"] = "automatic".into();
        assert!(serde_json::from_value::<ShortRequest>(invalid).is_err());
        let mut invalid = json;
        invalid["captions"] = serde_json::json!({"subtitles":"reviewed.srt", "style":{"size":"infinite", "placement":"bottom"}});
        assert!(serde_json::from_value::<ShortRequest>(invalid).is_err());
    }

    #[test]
    fn short_snapshot_covers_optional_subtitles_and_rejects_mutated_inputs() {
        let folder = tempfile::tempdir().unwrap();
        let mut request = request();
        request.source = folder.path().join("source.mp4");
        request.ffmpeg = folder.path().join("ffmpeg.exe");
        std::fs::write(&request.source, b"source metadata fixture").unwrap();
        std::fs::write(&request.ffmpeg, b"metadata fixture, never executed").unwrap();
        assert_eq!(request.snapshot().unwrap().0.len(), 2);
        let subtitles = folder.path().join("reviewed.srt");
        std::fs::write(&subtitles, b"track metadata fixture").unwrap();
        request.captions = Some(ShortCaptions {
            subtitles: subtitles.clone(),
            style: CaptionStyle::default(),
        });
        let snapshot = request.snapshot().unwrap();
        assert_eq!(snapshot.0.len(), 3);
        std::fs::write(&subtitles, b"changed").unwrap();
        assert!(snapshot.verify().is_err());
        std::fs::write(
            &subtitles,
            vec![b'x'; crate::subtitle_srt::MAX_INPUT_BYTES + 1],
        )
        .unwrap();
        assert_eq!(request.snapshot().unwrap_err().code, "error.caption_limit");
    }
}
