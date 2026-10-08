//! Compose a project interval from original inputs, keeping WAV and visual clocks separate.
//!
//! Selected visual pieces are normalized once into private lossless intermediates.
//! Only the final framing/caption/fade pass uses lossy video encoding. Repeated
//! pieces reuse their intermediate; no full-project video is required for a short.
use crate::{
    engine::{EngineError, Event, ExportRequest, ExportResult, MediaItem, Reporter},
    inspection::{FileStamp, Snapshot},
    shorts::{Framing, ShortCaptions},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub const TOTAL_TIMEOUT_SECS: u64 = 8100;
const MAX_PIECES: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectShort {
    /// Inclusive start on the WAV clock, in milliseconds.
    pub start_ms: u64,
    /// Exclusive end on the WAV clock, in milliseconds.
    pub end_ms: u64,
    #[serde(default)]
    pub restart_loops: bool,
    #[serde(default)]
    pub framing: Framing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRequest {
    pub montage: ExportRequest,
    #[serde(default)]
    pub short: Option<ProjectShort>,
    /// Reviewed SRT on the full WAV clock. Presence explicitly requests burning.
    #[serde(default)]
    pub captions: Option<ShortCaptions>,
}

impl ProjectRequest {
    pub fn validate(&self) -> Result<(), EngineError> {
        self.montage.validate()?;
        if self.short.is_some() || self.captions.is_some() {
            let output = &self.montage.output;
            if !output
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
            {
                return Err(error(
                    "error.short_config",
                    Some(output),
                    "Project captions and selected intervals require MP4 output.",
                ));
            }
        }
        if let Some(short) = &self.short
            && (short.start_ms >= short.end_ms || short.end_ms > crate::shorts::MAX_SOURCE_MS)
        {
            return Err(error(
                "error.short_range",
                None,
                "Select a nonempty WAV interval within the first two hours.",
            ));
        }
        for path in self
            .montage
            .items
            .iter()
            .map(MediaItem::path)
            .chain([&self.montage.wav, &self.montage.output])
            .chain(self.captions.iter().map(|c| &c.subtitles))
        {
            if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err(error(
                    "error.short_config",
                    Some(path),
                    "Project paths must be nonempty and contain no NUL characters.",
                ));
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot, EngineError> {
        self.validate()?;
        let mut snapshot = Snapshot::project(&self.montage)?;
        if let Some(captions) = &self.captions {
            let stamp = FileStamp::read(&captions.subtitles)?;
            if stamp.bytes > crate::subtitle_srt::MAX_INPUT_BYTES as u64 {
                return Err(error(
                    "error.caption_limit",
                    Some(&captions.subtitles),
                    "SRT input exceeds one MiB.",
                ));
            }
            snapshot.0.push(stamp);
        }
        snapshot.verify()?;
        Ok(snapshot)
    }

    pub fn same_render_settings(&self, other: &Self) -> bool {
        self.montage.same_render_settings(&other.montage)
            && self.short == other.short
            && self.captions == other.captions
    }
}

fn error(code: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new(code, "project_export", path, detail)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub item: usize,
    pub offset: f64,
    pub duration: f64,
}

/// Planning is independent of decoding and never changes the soundtrack range.
pub fn pieces(
    durations: &[f64],
    start: f64,
    span: f64,
    restart: bool,
) -> Result<Vec<Piece>, EngineError> {
    pieces_bounded(durations, start, span, restart, MAX_PIECES, false)
}

/// A bounded prefix for the desktop's live selection strip. The final visible
/// piece retains its actual duration; the caller can mark omitted repetitions.
pub fn preview_pieces(
    durations: &[f64],
    start: f64,
    span: f64,
    restart: bool,
    limit: usize,
) -> Result<Vec<Piece>, EngineError> {
    pieces_bounded(durations, start, span, restart, limit.clamp(1, 256), true)
}

fn pieces_bounded(
    durations: &[f64],
    start: f64,
    span: f64,
    restart: bool,
    limit: usize,
    truncate: bool,
) -> Result<Vec<Piece>, EngineError> {
    let loop_duration: f64 = durations.iter().sum();
    if durations.is_empty()
        || durations.iter().any(|d| !d.is_finite() || *d <= 0.0)
        || !loop_duration.is_finite()
        || !start.is_finite()
        || !span.is_finite()
        || span <= 0.0
        || start < 0.0
    {
        return Err(error(
            "error.short_range",
            None,
            "Visual sequence and selection require finite positive durations.",
        ));
    }
    let mut offset = if restart { 0.0 } else { start % loop_duration };
    // Frame-grid durations are often repeating binary fractions. At a seam,
    // modulo/subtraction can leave an offset just below the old end.
    // Treat only that rounding residue as the next occurrence; otherwise a
    // one-piece preview can repeatedly return a zero-duration FFmpeg command.
    let shortest = durations.iter().copied().fold(f64::INFINITY, f64::min);
    let seam_tolerance = (8.0 * f64::EPSILON * start.max(loop_duration).max(1.0))
        .max(1e-7)
        .min(shortest * 1e-6);
    let mut item = 0;
    while offset >= durations[item] || durations[item] - offset <= seam_tolerance {
        offset = (offset - durations[item]).max(0.0);
        item += 1;
        if item == durations.len() {
            item = 0;
        }
    }
    let mut remaining = span;
    let mut result = Vec::new();
    while remaining > 0.000_000_1 {
        if result.len() >= limit {
            if truncate {
                break;
            }
            return Err(error(
                "error.short_limit",
                None,
                "The selection exceeds 200000 visual pieces. Use longer media occurrences or a shorter selection.",
            ));
        }
        let duration = (durations[item] - offset).min(remaining);
        result.push(Piece {
            item,
            offset,
            duration,
        });
        remaining -= duration;
        offset = 0.0;
        item = (item + 1) % durations.len();
    }
    Ok(result)
}

pub(crate) fn execute(
    request: &ProjectRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    request.validate()?;
    // Retain packet copying and all existing full-montage behavior when no
    // composition is requested.
    if request.short.is_none() && request.captions.is_none() {
        return crate::engine::execute(request.montage.clone(), workspace, emit);
    }
    let started = Instant::now();
    let snapshot = request.snapshot()?;
    let montage = &request.montage;
    let mut items = montage.items.clone();
    for (item, stamp) in items.iter_mut().zip(&snapshot.0) {
        *item.path_mut() = stamp.resolved.clone();
    }
    let wav = &snapshot.0[items.len()].resolved;
    let ffmpeg = &snapshot.0[items.len() + 1].resolved;
    let workspace = workspace
        .canonicalize()
        .map_err(|e| error("error.short_config", Some(workspace), e))?;
    let duration = crate::wav_duration(wav).map_err(|e| error("error.wav", Some(wav), e))?;
    if !duration.is_finite()
        || duration <= 0.0
        || duration * 1000.0 > crate::shorts::MAX_SOURCE_MS as f64
    {
        return Err(error(
            "error.short_limit",
            Some(wav),
            "Captioned projects and project shorts require a positive WAV duration of at most two hours.",
        ));
    }
    if montage.fade_in + montage.fade_out > duration {
        return Err(error(
            "error.fades",
            Some(wav),
            "Combined fades exceed the WAV duration.",
        ));
    }
    let (start, end, restart) = request.short.as_ref().map_or((0.0, duration, false), |s| {
        (
            s.start_ms as f64 / 1000.0,
            s.end_ms as f64 / 1000.0,
            s.restart_loops,
        )
    });
    if end > duration + 0.000_000_1 {
        return Err(error(
            "error.short_range",
            Some(wav),
            "The selected end exceeds the WAV duration.",
        ));
    }
    let span = end - start;
    let mut report = Reporter::new(emit);
    report.progress(0, "short.preparing");
    // Plan against the full-resolution montage grid even for previews: changing
    // preview quality must never change image duration or short visual phase.
    let inspection = crate::sequence::inspect(ffmpeg, &items, true, false, false, &mut report)
        .map_err(|e| EngineError::wrap("inspect_project", None, e.as_ref()))?;
    let target = &inspection.decision.public.target;
    let mut plan = inspection.decision.public.clone();
    plan.stage = "project_interval".into();
    plan.segments = vec![crate::plan::Segment {
        start: 0.0,
        end: span,
        treatment: crate::plan::Treatment::Convert,
    }];
    report.plan(plan);
    let fps = target.rate_num as f64 / target.rate_den as f64;
    if !fps.is_finite()
        || fps <= 0.0
        || fps > 240.0
        || target.width > 16384
        || target.height > 16384
        || u64::from(target.width) * u64::from(target.height) > 64_000_000
    {
        return Err(error(
            "error.short_limit",
            None,
            "Project composition requires at most 240 fps, 16384 pixels per dimension and 64 million pixels per frame.",
        ));
    }
    let durations: Vec<_> = inspection
        .info
        .iter()
        .map(|m| (m.seconds * fps).round().max(1.0) / fps)
        .collect();
    let selection = pieces(&durations, start, span, restart)?;
    let geometry = request
        .short
        .as_ref()
        .map(|s| {
            crate::shorts::Geometry::new(
                f64::from(target.width),
                f64::from(target.height),
                s.framing,
            )
        })
        .transpose()?;
    let (canvas_w, canvas_h) = if request.short.is_some() {
        (1080, 1920)
    } else {
        (target.width, target.height)
    };
    let mut has_captions = false;
    if let Some(captions) = &request.captions {
        let path = &snapshot.0.last().unwrap().resolved;
        let text = crate::captions::read_srt(path)?;
        let track = crate::subtitle_srt::parse(&text, (duration * 1000.0).ceil() as u64)
            .map_err(|e| error("error.caption_track", Some(path), e))?;
        let track = if let Some(short) = &request.short {
            track
                .trim(short.start_ms, short.end_ms)
                .map_err(|e| error("error.caption_track", Some(path), e))?
        } else {
            track
        };
        if track.cues.is_empty() {
            report.warning("short.no_captions");
        } else {
            let canvas = crate::captions::Canvas::new(
                f64::from(canvas_w),
                f64::from(canvas_h),
                captions.style,
            )?;
            if crate::captions::stage(&track, &canvas, 0.0, span, &workspace)? {
                report.warning("caption.quantization");
            }
            has_captions = true;
        }
    }
    snapshot.verify()?;
    report.warning(if request.short.is_some() {
        "short.reencode"
    } else {
        "caption.reencode"
    });
    if request
        .short
        .as_ref()
        .is_some_and(|s| s.framing == Framing::Crop)
    {
        report.warning("short.crop");
    }
    report.diagnostic(format!("Direct project composition: WAV {start:.3}..{end:.3} s, visual restart {restart}; fades retain their full-project WAV clock. Private pieces use lossless FFV1, followed by one final H.264 encode."));
    let mut cached: HashMap<(usize, u64, u64), String> = HashMap::new();
    let mut origins = HashMap::new();
    let mut concat = String::from("ffconcat version 1.0\n");
    let mut done = 0.0;
    for piece in &selection {
        let key = (
            piece.item,
            (piece.offset * 1_000_000_000.0).round() as u64,
            (piece.duration * 1_000_000_000.0).round() as u64,
        );
        let name = if let Some(name) = cached.get(&key) {
            name.clone()
        } else {
            let name = format!("piece-{}.nut", cached.len());
            let item = &items[piece.item];
            let meta = &inspection.info[piece.item];
            let video_start = if item.is_image() {
                0.0
            } else if let Some(value) = origins.get(&piece.item) {
                *value
            } else {
                let timed = crate::media::inspect_timed(ffmpeg, item.path()).map_err(|e| {
                    EngineError::wrap("inspect_project_timing", Some(item.path()), e.as_ref())
                })?;
                crate::captions::supported_video(&timed.video)?;
                origins.insert(piece.item, timed.video_start);
                timed.video_start
            };
            let cmd = piece_command(
                ffmpeg,
                item,
                meta,
                target,
                piece,
                video_start,
                montage.clip_audio,
                &workspace,
                &name,
            )?;
            run_command(
                cmd,
                started,
                "prepare_project_piece",
                &mut report,
                1,
                59,
                done / span,
                0.0,
                false,
            )?;
            cached.insert(key, name.clone());
            name
        };
        concat.push_str(&format!("file '{name}'\nduration {:.9}\n", piece.duration));
        done += piece.duration;
        report.progress(1 + (58.0 * done / span).floor() as u32, "short.preparing");
    }
    fs::write(workspace.join("pieces.ffconcat"), concat)
        .map_err(|e| error("error.short_backend", Some(&workspace), e))?;
    snapshot.verify()?;
    let mut filter = vec!["setpts=PTS-STARTPTS".to_string()];
    // Apply fades before captions so text is never cropped or burned twice.
    // Offset PTS temporarily to the WAV/project clock, preserving partial fades
    // already in progress and avoiding new fades at interior short boundaries.
    if montage.fade_in > 0.0 || montage.fade_out > 0.0 {
        filter.push(format!("setpts=PTS+{start:.9}/TB"));
        if montage.fade_in > 0.0 {
            filter.push(format!("fade=t=in:st=0:d={:.9}", montage.fade_in));
        }
        if montage.fade_out > 0.0 {
            filter.push(format!(
                "fade=t=out:st={:.9}:d={:.9}",
                duration - montage.fade_out,
                montage.fade_out
            ));
        }
        filter.push(format!("setpts=PTS-{start:.9}/TB"));
    }
    if let Some(geometry) = geometry {
        filter.push(geometry.filter(has_captions, montage.preview));
    } else {
        if has_captions {
            filter.push(crate::captions::FILTER.into());
        }
        if montage.preview {
            filter.push("scale=w='max(2,trunc(min(640,min(iw,360*dar))/2)*2)':h='max(2,trunc(min(360,min(ih,640/dar))/2)*2)'".into());
        }
    }
    let mut cmd = crate::command(ffmpeg);
    cmd.current_dir(&workspace)
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-n",
            "-xerror",
            "-f",
            "concat",
            "-safe",
            "1",
            "-i",
            "pieces.ffconcat",
            "-ss",
            &format!("{start:.9}"),
            "-i",
        ])
        .arg(wav);
    let audio = if montage.clip_audio {
        format!(
            "[0:a]aresample=48000:async=1:first_pts=0,apad,atrim=duration={span:.9}[clips];[1:a]asetpts=PTS-STARTPTS,aresample=48000,apad,atrim=duration={span:.9}[wav];[clips][wav]amix=inputs=2:duration=first:dropout_transition=0[a]"
        )
    } else {
        format!("[1:a]asetpts=PTS-STARTPTS,apad,atrim=duration={span:.9}[a]")
    };
    // FFV1/NUT can carry zero duration on the final decoded frame. Passing that
    // duration through would shorten MP4's edit list and hide its final picture.
    // Use the known normalized frame period, clipped to the selected endpoint.
    cmd.args([
        "-filter_complex",
        &format!("[0:v]{}[v];{audio}", filter.join(",")),
        "-map",
        "[v]",
        "-map",
        "[a]",
        "-t",
        &format!("{span:.9}"),
        "-fps_mode",
        "passthrough",
        "-enc_time_base:v",
        "filter",
        "-c:v",
        "libx264",
        // Concatenated FFV1/NUT pieces do not provide a nominal picture rate.
        // Keep their precise timestamps, but prevent x264 from treating the
        // time-base frequency as fps (level 6.2 and hardware decode artifacts).
        "-x264-params",
        &format!("fps={}/{}", target.rate_num, target.rate_den),
        "-bf",
        "0",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        if montage.preview { "24" } else { "16" },
        "-preset",
        if montage.preview {
            "ultrafast"
        } else {
            "medium"
        },
        "-c:a",
        "aac",
        "-b:a",
        "320k",
        "-bsf:v",
        &format!(
            "setts=duration='min({:.12}/TB,{span:.9}/TB-(PTS-STARTPTS))'",
            1.0 / fps
        ),
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "export.mp4",
    ]);
    run_command(
        cmd,
        started,
        "render_project",
        &mut report,
        60,
        96,
        0.0,
        span,
        has_captions,
    )?;
    report.progress(97, "short.verifying");
    let output = workspace.join("export.mp4");
    let measured = crate::media::inspect_timed_raw(ffmpeg, &output)
        .map_err(|e| EngineError::wrap("verify_project", Some(&output), e.as_ref()))?;
    if measured.video.frames == 0
        || measured.video.codec != "h264"
        || !measured.video.square_pixels
        || measured.unknown_final_duration
        || measured.video_start.abs() > 0.001
        || measured.video_end > span + 0.001
        || measured.video_end < span - 1.0 / fps - 0.001
        || !measured.duration.is_finite()
        || measured.duration <= 0.0
    {
        return Err(error(
            "error.short_result",
            Some(&output),
            "Composed video has invalid or out-of-range presentation bounds. Nothing was published.",
        ));
    }
    if (measured.video_end - span).abs() > 0.001 {
        report.warning(&format!(
            "short.boundaries|{:.3}|{:.3}|{span:.3}",
            measured.video_start, measured.video_end
        ));
    }
    if let Some(audio_end) = measured.audio_end
        && audio_end > span + 0.001
    {
        report.warning(&format!(
            "short.audio_padding|{:.0}|{span:.3}",
            (audio_end - span) * 1000.0
        ));
    }
    snapshot.verify()?;
    report.progress(99, "short.verifying");
    Ok(ExportResult {
        output: montage.output.clone(),
        duration: measured.duration,
    })
}

#[allow(clippy::too_many_arguments)]
fn piece_command(
    ffmpeg: &Path,
    item: &MediaItem,
    meta: &crate::media::MediaInfo,
    target: &crate::plan::Target,
    piece: &Piece,
    video_start: f64,
    audio: bool,
    workspace: &Path,
    name: &str,
) -> Result<Command, EngineError> {
    let mut cmd = crate::command(ffmpeg);
    cmd.current_dir(workspace).args([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-n",
        "-xerror",
        "-err_detect",
        "explode",
    ]);
    let filter = if item.is_image() {
        cmd.args(
            crate::images::input_args(item.path(), target.rate_num, target.rate_den).map_err(
                |e| EngineError::wrap("prepare_project_image", Some(item.path()), e.as_ref()),
            )?,
        );
        crate::images::filter(target, meta.image_orientation)
    } else {
        cmd.args([
            "-ss",
            &format!("{:.9}", (video_start + piece.offset).max(0.0)),
            "-i",
        ])
        .arg(item.path());
        format!(
            "[0:v]setpts=PTS-STARTPTS,scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={n}/{d},tpad=stop_mode=clone:stop_duration=1,format=yuv420p[v]",
            w = target.width,
            h = target.height,
            n = target.rate_num,
            d = target.rate_den
        )
    };
    if audio && (!meta.audio || item.is_image()) {
        cmd.args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
    }
    cmd.args(["-filter_complex", &filter, "-map", "[v]"]);
    if audio {
        if meta.audio && !item.is_image() {
            cmd.args([
                "-map",
                "0:a:0",
                "-af",
                "aresample=48000:async=1:first_pts=0,apad",
            ]);
        } else {
            cmd.args(["-map", "1:a:0"]);
        }
        cmd.args(["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"]);
    } else {
        cmd.arg("-an");
    }
    cmd.args([
        "-t",
        &format!("{:.9}", piece.duration),
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-g",
        "1",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "nut",
        name,
    ]);
    Ok(cmd)
}

#[allow(clippy::too_many_arguments)]
fn run_command(
    command: Command,
    started: Instant,
    operation: &str,
    report: &mut Reporter<'_>,
    low: u32,
    high: u32,
    fraction: f64,
    span: f64,
    captions: bool,
) -> Result<(), EngineError> {
    let timeout = Duration::from_secs(TOTAL_TIMEOUT_SECS)
        .checked_sub(started.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| {
            error(
                "error.short_backend",
                None,
                "Project rendering deadline exceeded.",
            )
        })?;
    let (status, log) = crate::process::stream(command, Some(timeout), false, |line| {
        if let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|s| s.parse::<f64>().ok())
            && us.is_finite()
            && span > 0.0
        {
            report.progress(
                low + ((high - low) as f64 * (us / 1_000_000.0 / span).clamp(0.0, 1.0)) as u32,
                "short.rendering",
            );
        }
        Ok(())
    })
    .map_err(|e| EngineError::wrap(operation, None, e.as_ref()))?;
    if !status.success() || captions && crate::captions::unsupported_renderer(&log) {
        let mut failure = EngineError::new(
            "error.short_backend",
            operation,
            None,
            "FFmpeg could not compose this project. Inspect the technical log for the decoder or renderer error.",
        );
        failure.technical = crate::engine::bounded(&log, 8192).into();
        return Err(failure);
    }
    if span == 0.0 {
        report.progress(
            low + ((high - low) as f64 * fraction.clamp(0.0, 1.0)) as u32,
            "short.preparing",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_frame_grid_seams_always_advance() {
        for fps in [24.0, 24000.0 / 1001.0, 30000.0 / 1001.0] {
            for durations in [vec![241.0 / fps], vec![241.0 / fps, 73.0 / fps]] {
                let mut clock = 0.0;
                for occurrence in 0..10000 {
                    let piece = preview_pieces(&durations, clock, 30.0, false, 1).unwrap()[0];
                    assert_eq!(piece.item, occurrence % durations.len());
                    assert!(piece.offset < 1e-7, "{clock}: {piece:?}");
                    assert!(piece.duration > 1.0 / fps, "{clock}: {piece:?}");
                    let next = clock + piece.duration;
                    assert!(next > clock);
                    clock = next;
                }
                // A real position before the seam must retain the old frame.
                let before =
                    preview_pieces(&durations, durations[0] - 0.001, 1.0, false, 1).unwrap()[0];
                assert_eq!(before.item, 0);
                assert!((before.duration - 0.001).abs() < 1e-9);
            }
        }
    }
    #[test]
    fn phase_changes_visual_sequence_only() {
        assert_eq!(
            pieces(&[4.0, 6.0], 13.0, 5.0, false).unwrap(),
            [
                Piece {
                    item: 0,
                    offset: 3.0,
                    duration: 1.0
                },
                Piece {
                    item: 1,
                    offset: 0.0,
                    duration: 4.0
                }
            ]
        );
        assert_eq!(
            pieces(&[4.0, 6.0], 13.0, 5.0, true).unwrap(),
            [
                Piece {
                    item: 0,
                    offset: 0.0,
                    duration: 4.0
                },
                Piece {
                    item: 1,
                    offset: 0.0,
                    duration: 1.0
                }
            ]
        );
        let result = pieces(&[4.0, 6.0], 9.0, 23.0, false).unwrap();
        assert_eq!(result.iter().map(|p| p.duration).sum::<f64>(), 23.0);
        assert_eq!(result.first().unwrap().item, 1);
        assert_eq!(result.last().unwrap().item, 0);
    }
}
