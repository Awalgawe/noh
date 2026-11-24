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
    /// A caption effect plugin drawn by the GPU engine instead of libass. Without
    /// captions it has nothing to draw and is ignored.
    #[serde(default)]
    pub effect: Option<EffectChoice>,
    /// A motion scene drawn instead of the captions (it draws the lyrics
    /// itself, on the song's clock). Exclusive with `effect`; like it, without
    /// captions it has nothing to draw and is ignored.
    #[serde(default)]
    pub motion: Option<crate::scene_request::ProjectScene>,
}

/// The effect plugin chosen for the captions: its folder, its id (checked when the
/// folder is loaded) and the preset (level 0). The plugin is read once per export,
/// so editing its files during an export cannot mix two versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectChoice {
    pub id: String,
    #[serde(default)]
    pub preset: Option<String>,
    pub dir: std::path::PathBuf,
    /// Parameter values set from the app (Classique's size and placement), over
    /// the preset's.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

impl EffectChoice {
    /// A chosen effect plugin for a caption burn or a Short: an id, a folder, and
    /// a build that can draw it (without the engine it is refused, never drawn
    /// as Classique). `operation` names the request's validation step.
    pub(crate) fn check(&self, operation: &str) -> Result<(), EngineError> {
        if !cfg!(feature = "effects") {
            return Err(EngineError::new(
                "error.effects_unavailable",
                operation,
                None,
                "This build of NOH has no caption effects engine. Nothing was published.",
            ));
        }
        if self.id.trim().is_empty()
            || self.dir.as_os_str().is_empty()
            || self.dir.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err(EngineError::new(
                "error.effect",
                operation,
                Some(&self.dir),
                "A caption effect needs a plugin id and its folder.",
            ));
        }
        Ok(())
    }
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
            .chain(self.effect.iter().map(|e| &e.dir))
            .chain(self.motion.iter().map(|m| &m.scene))
        {
            if path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err(error(
                    "error.short_config",
                    Some(path),
                    "Project paths must be nonempty and contain no NUL characters.",
                ));
            }
        }
        if self.effect.as_ref().is_some_and(|e| e.id.trim().is_empty()) {
            return Err(error(
                "error.short_config",
                None,
                "A caption effect needs a plugin id.",
            ));
        }
        if let Some(motion) = &self.motion {
            if self.effect.is_some() {
                return Err(error(
                    "error.scene_with_effect",
                    Some(&motion.scene),
                    "A scene draws the lyrics itself: it cannot be combined with a caption effect.",
                ));
            }
            motion.validate()?;
        }
        Ok(())
    }

    /// Whether the captions are drawn live over a preview (by an effect or a
    /// scene) rather than burned into it by libass.
    pub fn drawn_live(&self) -> bool {
        self.active_effect().is_some() || self.active_motion().is_some()
    }

    /// The scene actually drawn: one is chosen and there are captions (its
    /// lyrics) to draw.
    pub fn active_motion(&self) -> Option<&crate::scene_request::ProjectScene> {
        self.motion.as_ref().filter(|_| self.captions.is_some())
    }

    /// The effect actually applied: one is chosen and there are captions to draw.
    pub fn active_effect(&self) -> Option<&EffectChoice> {
        self.effect.as_ref().filter(|_| self.captions.is_some())
    }

    /// The command line and the MCP choose no effect: their exported captions are
    /// Classique, built into NOH, drawn by the same engine as the app's (Task 43).
    /// A preview render keeps libass: the app draws its effect over it live, or
    /// shows libass there when its monitor could not draw.
    fn default_classic(&self) -> Option<EffectChoice> {
        if !cfg!(feature = "effects")
            || self.effect.is_some()
            || self.motion.is_some()
            || self.montage.preview
        {
            return None;
        }
        self.captions.as_ref().map(|c| EffectChoice {
            id: CLASSIC.into(),
            preset: None,
            dir: std::path::PathBuf::new(),
            values: classic_values(&c.style),
        })
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
            // Which effect or scene does not matter: previews and stills carry no
            // captions with one (the monitor draws it), only whether one is active.
            && (self.active_effect().is_some() || self.active_motion().is_some())
                == (other.active_effect().is_some() || other.active_motion().is_some())
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
    // Classique's libass fallback: its cues and style, staged only when needed.
    let mut classic_fallback: Option<(
        crate::subtitle_track::SubtitleTrack,
        crate::captions::CaptionStyle,
    )> = None;
    // With an effect, the export draws the captions with the GPU engine (E4) and a
    // preview render carries none: the preview draws the effect over it live.
    let default_classic = request.default_classic();
    let builtin = default_classic.is_some();
    let effect = request.active_effect().or(default_classic.as_ref());
    let motion = request.active_motion();
    let mut effect_track = None;
    // A scene sees the whole lyrics on the song's clock, with the count of
    // lines whose word times were read beside the subtitles.
    let mut scene_track = None;
    if let Some(captions) = request
        .captions
        .as_ref()
        .filter(|_| !((effect.is_some() || motion.is_some()) && montage.preview))
    {
        let path = &snapshot.0.last().unwrap().resolved;
        let text = crate::captions::read_srt(path)?;
        let mut track = crate::subtitle_srt::parse(&text, (duration * 1000.0).ceil() as u64)
            .map_err(|e| error("error.caption_track", Some(path), e))?;
        // The effect exports with the word times kept beside the subtitles
        // (libass ignores them); a cue changed outside NOH has none.
        let words = crate::subtitle_words::read_beside(path, &mut track);
        if motion.is_some() {
            scene_track = Some((track.clone(), words));
        }
        let track = if let Some(short) = &request.short {
            track
                .trim(short.start_ms, short.end_ms)
                .map_err(|e| error("error.caption_track", Some(path), e))?
        } else {
            track
        };
        if motion.is_some() {
            // The scene draws even where no line falls in a short's interval.
        } else if track.cues.is_empty() {
            report.warning("short.no_captions");
        } else if effect.is_some() {
            // Classique keeps what libass needs (the cues and the same style: size,
            // placement, safe area): staged only if no OpenGL context exists, so
            // libass's layout limits never stop an export the GPU draws.
            if let Some(e) = effect.filter(|e| e.id == CLASSIC) {
                classic_fallback = Some((track.clone(), fallback_style(captions.style, &e.values)));
            }
            effect_track = Some(track);
        } else {
            let canvas = crate::captions::Canvas::new(
                f64::from(canvas_w),
                f64::from(canvas_h),
                captions.style,
            )?;
            if crate::captions::stage(&track, &canvas, 0.0, span, &workspace)? {
                report.warning("caption.quantization");
            }
            if let Some(at) = crate::captions::first_too_wide(&track, &canvas) {
                report.warning(&crate::captions::word_too_wide_warning(at));
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
                &inspection.decision.color,
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
    // FFV1/NUT pieces carry no tags: state the export matrix first, so libass
    // blends in it, and again last (only `setparams` tags; never -colorspace).
    let color_tags = inspection.decision.color.tags.setparams();
    filter.extend(color_tags.clone());
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
    // The geometry and captions tail; the libass fallback is the same with captions.
    let tail = |captions: bool| -> Vec<String> {
        let mut tail = Vec::new();
        if let Some(geometry) = &geometry {
            tail.push(geometry.filter(captions, montage.preview));
        } else {
            if captions {
                tail.push(crate::captions::FILTER.into());
            }
            if montage.preview {
                tail.push("scale=w='max(2,trunc(min(640,min(iw,360*dar))/2)*2)':h='max(2,trunc(min(360,min(ih,640/dar))/2)*2)'".into());
            }
        }
        tail.extend(color_tags.clone());
        tail
    };
    let mut fallback_filter = filter.clone();
    fallback_filter.extend(tail(true));
    filter.extend(tail(has_captions));
    let audio = if montage.clip_audio {
        format!(
            "[0:a]aresample=48000:async=1:first_pts=0,apad,atrim=duration={span:.9}[clips];[1:a]asetpts=PTS-STARTPTS,aresample=48000,apad,atrim=duration={span:.9}[wav];[clips][wav]amix=inputs=2:duration=first:dropout_transition=0[a]"
        )
    } else {
        format!("[1:a]asetpts=PTS-STARTPTS,apad,atrim=duration={span:.9}[a]")
    };
    let filter_complex = format!("[0:v]{}[v];{audio}", filter.join(","));
    let fallback_complex = format!("[0:v]{}[v];{audio}", fallback_filter.join(","));
    let final_command = |encoder: crate::encoder::VideoEncoder, filter_complex: &str| {
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
            .arg(wav)
            .args([
                "-filter_complex",
                filter_complex,
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
            ])
            .args(encoder.args(target.rate_num, target.rate_den, montage.preview))
            // FFV1/NUT can carry zero duration on the final decoded frame. Passing
            // that duration through would shorten MP4's edit list and hide its
            // final picture. Use the known normalized frame period, clipped to
            // the selected endpoint.
            .args([
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
        cmd
    };
    let output = workspace.join("export.mp4");
    let selection = crate::encoder::select(ffmpeg, montage.preview);
    let pass = match (motion, scene_track, effect, effect_track) {
        (Some(m), Some((track, words)), _, _) => Some((
            Pass::Scene {
                scene: &m.scene,
                words,
                start,
                duration,
                safe_area: request
                    .captions
                    .as_ref()
                    .map(|c| c.style.safe_area)
                    .unwrap_or_default(),
            },
            track,
        )),
        (None, _, Some(choice), Some(track)) => Some((Pass::Effect { choice, builtin }, track)),
        _ => None,
    };
    let effect_job = match pass {
        Some((pass, track)) => Some(EffectJob {
            pass,
            track,
            video_filter: filter.join(","),
            clip_audio: montage.clip_audio,
            audio: audio.clone(),
            size: (canvas_w, canvas_h),
            color_tags: color_tags.clone(),
            bt709: inspection.decision.color.matrix != crate::color::Matrix::Bt601,
            // The effect's surface: the same safe area libass uses for these captions.
            safe: request
                .captions
                .as_ref()
                .and_then(|c| c.style.safe_area.insets(canvas_w, canvas_h)),
        }),
        _ => None,
    };
    if let Some(probe) = selection.probe {
        report.diagnostic(format!(
            "NVENC probe: {} in {} ms.",
            if selection.encoder == crate::encoder::VideoEncoder::Nvenc {
                "available"
            } else {
                "unavailable"
            },
            probe.as_millis()
        ));
    }
    let mut fallback_log = None;
    // libass draws the captions here (no effect), or below when OpenGL fails: an
    // FFmpeg older than 7.1 blends them in BT.601 whatever the export matrix.
    let libass_colours =
        || crate::color::libass_colour_warning(ffmpeg, inspection.decision.color.matrix);
    if has_captions && let Some(w) = libass_colours() {
        report.warning(w);
    }
    let mut fallback_said = false;
    let rendered = crate::encoder::render_with_fallback(
        selection.encoder,
        &output,
        |encoder| match &effect_job {
            Some(job) => match run_effect(
                job,
                ffmpeg,
                wav,
                &workspace,
                encoder,
                target,
                (start, span, fps),
                started,
                &mut report,
            ) {
                // No OpenGL context, nothing started: Classique's captions are drawn
                // by libass instead, from the same style, and the export says so.
                Err(e) if e.code == "error.effect_gl_unavailable" && classic_fallback.is_some() => {
                    let (track, style) = classic_fallback.as_ref().unwrap();
                    // libass's own limits apply here: without a GPU and without a
                    // libass layout, the export fails with libass's error.
                    let canvas = crate::captions::Canvas::new(
                        f64::from(canvas_w),
                        f64::from(canvas_h),
                        *style,
                    )?;
                    let quantized = crate::captions::stage(track, &canvas, 0.0, span, &workspace)?;
                    // Said once: an NVENC retry comes back through this branch.
                    if !fallback_said {
                        fallback_said = true;
                        report.warning("warning.effect_fallback");
                        if let Some(w) = libass_colours() {
                            report.warning(w);
                        }
                        if quantized {
                            report.warning("caption.quantization");
                        }
                        if let Some(at) = crate::captions::first_too_wide(track, &canvas) {
                            report.warning(&crate::captions::word_too_wide_warning(at));
                        }
                    }
                    run_command(
                        final_command(encoder, &fallback_complex),
                        started,
                        "render_project",
                        &mut report,
                        60,
                        96,
                        0.0,
                        span,
                        true,
                    )
                }
                other => other,
            },
            None => run_command(
                final_command(encoder, &filter_complex),
                started,
                "render_project",
                &mut report,
                60,
                96,
                0.0,
                span,
                has_captions,
            ),
        },
        |failure: &EngineError| fallback_log = Some(failure.technical.to_string()),
    );
    // Report the NVENC failure and the retry even when the retry fails too.
    if let Some(log) = fallback_log {
        report.warning("short.encoder_fallback");
        report.diagnostic(format!("NVENC failure, retried with libx264:\n{log}"));
    }
    let used = rendered?;
    report.diagnostic(format!("Video encoder: {}.", used.name()));
    report.progress(97, "short.verifying");
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

/// What draws the layer of a project export.
enum Pass<'a> {
    Effect {
        choice: &'a EffectChoice,
        /// The built-in Classique (`style::catalog::builtin_classic`), not a folder.
        builtin: bool,
    },
    /// A motion scene, validated at the export's frames before anything starts.
    Scene {
        scene: &'a Path,
        /// Lines whose word times were read beside the subtitles.
        words: usize,
        /// The output's first instant on the song's clock (a short's start).
        start: f64,
        /// The song's length.
        duration: f64,
        safe_area: crate::safe_area::SafeArea,
    },
}

/// The caption effect pass of a project export (see `fx::export`).
struct EffectJob<'a> {
    pass: Pass<'a>,
    /// Cues: on the output clock (trimmed for a short) for an effect, as libass
    /// would get them; whole, on the song's clock, for a scene.
    track: crate::subtitle_track::SubtitleTrack,
    /// The project's video filters without captions.
    video_filter: String,
    clip_audio: bool,
    /// The project's audio graph, inputs numbered as in the libass command.
    audio: String,
    size: (u32, u32),
    /// The final `setparams` of the export (FFmpeg #2 receives untagged frames).
    color_tags: Option<String>,
    /// The export matrix is BT.709 (or BT.2020, mixed as BT.709) rather than BT.601.
    bt709: bool,
    /// The safe area as insets from the canvas edges, None for the whole canvas.
    safe: Option<[u32; 4]>,
}

/// The effect that draws classic captions (it has a libass fallback).
pub const CLASSIC: &str = "noh.classic";

/// Classique's parameter values for a caption style: its size, its placement and,
/// once set, its line width (in %). The app sends them; the command line and the
/// MCP get them by default.
pub fn classic_values(
    style: &crate::captions::CaptionStyle,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    use crate::captions::{CaptionPlacement, CaptionSize};
    let mut values = std::collections::BTreeMap::new();
    let size = match style.size {
        CaptionSize::Small => "small",
        CaptionSize::Medium => "medium",
        CaptionSize::Large => "large",
    };
    let placement = match style.placement {
        CaptionPlacement::Top => "top",
        CaptionPlacement::Bottom => "bottom",
    };
    values.insert("size".into(), size.into());
    values.insert("placement".into(), placement.into());
    // Classique's range, as the libass fallback clamps it (Canvas::new).
    if let Some(width) = style.line_width {
        values.insert(
            "width".into(),
            (f64::from(width.clamp(4_000, 10_000)) / 100.0).into(),
        );
    }
    values
}

/// The style libass draws Classique's fallback with: Classique's own line width
/// when its value is set, so both break lines at the same places.
fn fallback_style(
    mut style: crate::captions::CaptionStyle,
    values: &std::collections::BTreeMap<String, serde_json::Value>,
) -> crate::captions::CaptionStyle {
    if let Some(width) = values
        .get("width")
        .and_then(serde_json::Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0)
    {
        style.line_width = Some((width * 100.0).round() as u32);
    }
    style
}

#[cfg(not(feature = "effects"))]
#[allow(clippy::too_many_arguments)]
fn run_effect(
    _: &EffectJob<'_>,
    _: &Path,
    _: &Path,
    _: &Path,
    _: crate::encoder::VideoEncoder,
    _: &crate::plan::Target,
    _: (f64, f64, f64),
    _: Instant,
    _: &mut Reporter<'_>,
) -> Result<(), EngineError> {
    Err(error(
        "error.effects_unavailable",
        None,
        "This build of NOH has no caption effects engine. Nothing was published.",
    ))
}

/// The layer a project export mixes into its frames: the effect's scene, or
/// the motion scene's overlay once it has passed its validation at the
/// export's frames (refused with the instant and the cap, nothing started).
#[cfg(feature = "effects")]
fn job_layer(
    job: &EffectJob<'_>,
    ffmpeg: &Path,
    wav: &Path,
    fps: f64,
    span: f64,
) -> Result<crate::fx::export::Layer, EngineError> {
    use crate::style;
    let (choice, builtin) = match &job.pass {
        Pass::Effect { choice, builtin } => (*choice, *builtin),
        Pass::Scene {
            scene,
            words,
            start,
            duration,
            safe_area,
        } => {
            return scene_layer(
                job, scene, *words, *start, *duration, *safe_area, ffmpeg, wav, fps, span,
            );
        }
    };
    let effect = if builtin {
        style::catalog::builtin_classic()
    } else {
        style::catalog::load_one(&choice.dir, &choice.id)
            .map_err(|e| error("error.effect", Some(&choice.dir), e))?
    };
    if let Some(preset) = &choice.preset
        && !effect.presets.iter().any(|(id, _)| id == preset)
    {
        return Err(error(
            "error.effect",
            Some(&choice.dir),
            format!("The effect has no preset \"{preset}\"."),
        ));
    }
    let (w, h) = job.size;
    let scene = style::plugin::PluginScene::new(
        effect,
        w,
        h,
        style::line::from_track(&job.track),
        choice.preset.as_deref(),
    )
    .with_values(&choice.values)
    .map_err(|e| error("error.effect", Some(&choice.dir), e))?
    .with_safe_insets(job.safe);
    // Classique's letters never leave the picture by design: when they do, say so.
    // Animated effects may (Typographic composition slides words in from outside).
    let scene = if choice.id == CLASSIC {
        scene.with_frame_check()
    } else {
        scene
    };
    Ok(Box::new(crate::style::plugin::StyleLayer::new(scene)))
}

#[cfg(all(feature = "effects", not(feature = "motion")))]
#[allow(clippy::too_many_arguments)]
fn scene_layer(
    _: &EffectJob<'_>,
    scene: &Path,
    _: usize,
    _: f64,
    _: f64,
    _: crate::safe_area::SafeArea,
    _: &Path,
    _: &Path,
    _: f64,
    _: f64,
) -> Result<crate::fx::export::Layer, EngineError> {
    Err(error(
        "error.scene_unavailable",
        Some(scene),
        "This build of NOH cannot draw scenes.",
    ))
}

/// The song's sound gives the energy and onsets; the frame is the export's;
/// the scene's clock is the song's, so a short's frames start at its start.
#[cfg(feature = "motion")]
#[allow(clippy::too_many_arguments)]
fn scene_layer(
    job: &EffectJob<'_>,
    scene: &Path,
    words: usize,
    start: f64,
    duration: f64,
    safe_area: crate::safe_area::SafeArea,
    ffmpeg: &Path,
    wav: &Path,
    fps: f64,
    span: f64,
) -> Result<crate::fx::export::Layer, EngineError> {
    use crate::fx::layer_shader as shader;
    use crate::motion::{layer, song, thread::ScriptThread, validate};
    let refused = |e: String| error("error.scene_refused", Some(scene), e);
    let size = job.size;
    crate::scene_request::safe_area_required(size.0, size.1, safe_area)?;
    let gathered = song::gather(
        ffmpeg, wav, true, size, size, duration, &job.track, words, safe_area,
    )
    .map_err(|e| error("error.scene_input", Some(wav), e))?;
    let src = layer::read_scene(scene).map_err(refused)?;
    let shaders = shader::read_beside(scene).map_err(refused)?;
    let mut inputs = gathered.inputs;
    inputs.shaders = shaders.iter().map(|(n, _)| n.clone()).collect();
    let thread = ScriptThread::start(src, inputs, gathered.audio, false).map_err(refused)?;
    let fps = validate::known_fps(fps).unwrap_or(validate::FALLBACK_FPS);
    thread
        .validate(validate::grid(start, start + span, fps))
        .map_err(refused)?;
    let overlay = layer::MotionLayer::new(thread, size, start, shaders).map_err(refused)?;
    Ok(Box::new(overlay))
}

/// E4: FFmpeg #1 (video, no captions) -> GPU caption layer and YUV mix -> FFmpeg #2
/// (sound, encoder). Same inputs, filters, encoder and timestamps as the libass
/// command, the captions excepted.
#[cfg(feature = "effects")]
#[allow(clippy::too_many_arguments)]
fn run_effect(
    job: &EffectJob<'_>,
    ffmpeg: &Path,
    wav: &Path,
    workspace: &Path,
    encoder: crate::encoder::VideoEncoder,
    target: &crate::plan::Target,
    (start, span, fps): (f64, f64, f64),
    started: Instant,
    report: &mut Reporter<'_>,
) -> Result<(), EngineError> {
    use crate::fx;
    let deadline = Duration::from_secs(TOTAL_TIMEOUT_SECS)
        .checked_sub(started.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| {
            error(
                "error.short_backend",
                None,
                "Project rendering deadline exceeded.",
            )
        })?;
    let layer = job_layer(job, ffmpeg, wav, fps, span)?;
    let (w, h) = job.size;
    let mut first = crate::command(ffmpeg);
    first.current_dir(workspace).args([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-xerror",
        "-f",
        "concat",
        "-safe",
        "1",
        "-i",
        "pieces.ffconcat",
        "-filter_complex",
        &format!("[0:v]{}[v]", job.video_filter),
        "-map",
        "[v]",
        "-t",
        &format!("{span:.9}"),
        "-fps_mode",
        "passthrough",
        "-enc_time_base:v",
        "filter",
    ]);
    // In FFmpeg #2 the Matroska pipe is input 0: the clips (if any) and the WAV move up one.
    let audio = if job.clip_audio {
        job.audio
            .replace("[1:a]", "[2:a]")
            .replace("[0:a]", "[1:a]")
    } else {
        job.audio.clone()
    };
    let (ffmpeg2, wav2, workspace2, clip_audio) = (
        ffmpeg.to_path_buf(),
        wav.to_path_buf(),
        workspace.to_path_buf(),
        job.clip_audio,
    );
    let (rate_num, rate_den) = (target.rate_num, target.rate_den);
    let color_tags = job.color_tags.clone();
    let second = move |time_base: &str, first: f64| {
        let mut cmd = crate::command(&ffmpeg2);
        cmd.current_dir(&workspace2).args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-n",
            "-xerror",
        ]);
        fx::export::pipe_input(&mut cmd, first);
        if clip_audio {
            cmd.args(["-f", "concat", "-safe", "1", "-i", "pieces.ffconcat"]);
        }
        cmd.args(["-ss", &format!("{start:.9}"), "-i"])
            .arg(&wav2)
            .args(["-filter_complex", &audio, "-map", "0:v", "-map", "[a]"]);
        // Tags only, on the mapped stream: frames keep their values.
        if let Some(tags) = &color_tags {
            cmd.args(["-filter:v", tags]);
        }
        cmd.args([
            "-t",
            &format!("{span:.9}"),
            "-fps_mode",
            "passthrough",
            "-enc_time_base:v",
            time_base,
        ])
        .args(encoder.args(rate_num, rate_den, false))
        .args([
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
        cmd
    };
    let job_safe = job.safe.is_some();
    let job = fx::export::Job {
        first,
        second: Box::new(second),
        workspace: workspace.to_path_buf(),
        width: w,
        height: h,
        source: (w, h),
        matrix: if job.bt709 {
            fx::gl::Matrix::Bt709
        } else {
            fx::gl::Matrix::Bt601
        },
        scene: layer,
        deadline,
        end: None,
    };
    let result = fx::export::run(job, |line| {
        if let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|s| s.parse::<f64>().ok())
            && us.is_finite()
            && span > 0.0
        {
            report.progress(
                60 + (36.0 * (us / 1_000_000.0 / span).clamp(0.0, 1.0)) as u32,
                "short.rendering",
            );
        }
    });
    match result {
        Ok((status, _, stats)) if status.success() => {
            // The effect drew letters outside the safe area: say when, change nothing.
            if let Some(at) = stats.overflow_at {
                let at = at.max(0.0).round() as u64;
                let code = if job_safe {
                    "warning.effect_overflow"
                } else {
                    "warning.effect_off_frame"
                };
                report.warning(&format!("{code}|{}:{:02}", at / 60, at % 60));
            }
            if let Some(at) = stats.too_wide_at {
                report.warning(&crate::captions::word_too_wide_warning(at));
            }
            if let Some(family) = &stats.font_missing {
                report.warning(&crate::caption_gpu::font_missing_warning(family));
            }
            if let Some((at, why)) = &stats.over_caps {
                report.warning(&crate::caption_gpu::heavy_warning(*at));
                report.diagnostic(format!("Caption effect over a cap: {why}."));
            }
            report.diagnostic(format!(
                "Caption effect: {} frames, engine set up in {:.0} ms on {}, time base {}, layer and mix p95 {:.2} ms per frame (p50/p95/max ms: layer {:.2}/{:.2}/{:.2}, mix submit {:.2}/{:.2}/{:.2}, readback wait {:.2}/{:.2}/{:.2}).",
                stats.frames, stats.setup_ms, stats.adapter, stats.time_base, stats.render_ms_p95,
                stats.layer_ms[0], stats.layer_ms[1], stats.layer_ms[2],
                stats.mix_ms[0], stats.mix_ms[1], stats.mix_ms[2],
                stats.fetch_ms[0], stats.fetch_ms[1], stats.fetch_ms[2]
            ));
            Ok(())
        }
        Ok((_, log, _)) => {
            let mut failure = EngineError::new(
                "error.short_backend",
                "render_project_effect",
                None,
                "FFmpeg could not encode this project. Inspect the technical log for the encoder error.",
            );
            failure.technical = crate::engine::bounded(&log, 8192).into();
            Err(failure)
        }
        Err(f) if f.gl_unavailable => Err(error(
            "error.effect_gl_unavailable",
            None,
            format!("This computer cannot draw caption effects ({}).", f.message),
        )),
        Err(f) => {
            let mut failure = EngineError::new(
                "error.effect_render",
                "render_project_effect",
                None,
                format!(
                    "The caption effect could not be rendered: {}. Nothing was published.",
                    f.message
                ),
            );
            failure.technical = crate::engine::bounded(&f.log, 8192).into();
            Err(failure)
        }
    }
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
    color: &crate::color::Plan,
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
        crate::images::filter(target, meta.image_orientation, color)
    } else {
        cmd.args([
            "-ss",
            &format!("{:.9}", (video_start + piece.offset).max(0.0)),
            "-i",
        ])
        .arg(item.path());
        format!(
            "[0:v]setpts=PTS-STARTPTS,scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={n}/{d},tpad=stop_mode=clone:stop_duration=1,{tail}[v]",
            w = target.width,
            h = target.height,
            n = target.rate_num,
            d = target.rate_den,
            // One matrix for the export; FFV1/NUT drops the tags, the final pass sets them.
            tail = color.video_tail(&crate::color::Input::video(meta))
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

    fn captioned(
        effect: Option<EffectChoice>,
        preview: bool,
        style: crate::captions::CaptionStyle,
    ) -> ProjectRequest {
        ProjectRequest {
            motion: None,
            montage: ExportRequest {
                items: vec![MediaItem::Video {
                    path: "a.mp4".into(),
                }],
                wav: "a.wav".into(),
                output: "out.mp4".into(),
                ffmpeg: "ffmpeg".into(),
                fade_in: 0.0,
                fade_out: 0.0,
                partial_fades: true,
                preview,
                clip_audio: false,
                force_encode: false,
            },
            short: None,
            captions: Some(ShortCaptions {
                subtitles: "a.srt".into(),
                style,
            }),
            effect,
        }
    }

    /// A scene draws the lyrics itself: never with an effect, never with the
    /// default Classique, and like an effect only with captions; a preview
    /// render with one carries no captions (the monitor draws it).
    #[test]
    fn a_scene_replaces_the_effect_and_needs_captions() {
        let scene = || crate::scene_request::ProjectScene {
            scene: std::env::temp_dir().join("a.noh-motion.rhai"),
        };
        let style = crate::captions::CaptionStyle::default();
        let mut with = captioned(None, false, style);
        with.motion = Some(scene());
        assert!(with.default_classic().is_none());
        assert!(with.active_motion().is_some());
        if cfg!(feature = "motion") {
            with.validate().unwrap();
        }
        let classic = EffectChoice {
            id: CLASSIC.into(),
            preset: None,
            dir: "c".into(),
            values: Default::default(),
        };
        let mut both = captioned(Some(classic), false, style);
        both.motion = Some(scene());
        assert_eq!(both.validate().unwrap_err().code, "error.scene_with_effect");
        let mut bare = captioned(None, false, style);
        bare.motion = Some(scene());
        bare.captions = None;
        assert!(bare.active_motion().is_none());
        let mut relative = captioned(None, false, style);
        relative.motion = Some(crate::scene_request::ProjectScene {
            scene: "a.noh-motion.rhai".into(),
        });
        assert_eq!(relative.validate().unwrap_err().code, "error.scene_request");
        // Previews with a scene or an effect render the same (no captions).
        let mut preview_scene = captioned(None, true, style);
        preview_scene.motion = Some(scene());
        let karaoke = EffectChoice {
            id: "test.wipe".into(),
            preset: None,
            dir: "k".into(),
            values: Default::default(),
        };
        assert!(preview_scene.same_render_settings(&captioned(Some(karaoke), true, style)));
        assert!(!preview_scene.same_render_settings(&captioned(None, true, style)));
    }

    /// Task 43: the command line and the MCP choose no effect; their exported
    /// captions are the built-in Classique with the caption style's values.
    #[cfg(feature = "effects")]
    #[test]
    fn captions_without_an_effect_export_with_classique() {
        use crate::captions::{CaptionPlacement, CaptionSize, CaptionStyle};
        let style = CaptionStyle {
            size: CaptionSize::Large,
            placement: CaptionPlacement::Top,
            line_width: Some(5500),
            ..CaptionStyle::default()
        };
        let classic = captioned(None, false, style).default_classic().unwrap();
        assert_eq!(
            (classic.id.as_str(), classic.preset.as_deref()),
            (CLASSIC, None)
        );
        assert_eq!(
            serde_json::to_value(&classic.values).unwrap(),
            serde_json::json!({"size": "large", "placement": "top", "width": 55.0})
        );
        // The historical width is Classique's default: no value is sent.
        assert!(
            !captioned(None, false, CaptionStyle::default())
                .default_classic()
                .unwrap()
                .values
                .contains_key("width")
        );
        // A preview render keeps libass; a chosen effect is kept; no captions, no effect.
        assert!(captioned(None, true, style).default_classic().is_none());
        let karaoke = EffectChoice {
            id: "test.wipe".into(),
            preset: None,
            dir: "k".into(),
            values: Default::default(),
        };
        assert!(
            captioned(Some(karaoke), false, style)
                .default_classic()
                .is_none()
        );
        let mut bare = captioned(None, false, style);
        bare.captions = None;
        assert!(bare.default_classic().is_none());
    }

    /// Classique's libass fallback wraps at Classique's own width value.
    #[test]
    fn the_classic_fallback_takes_the_effect_line_width() {
        use crate::captions::CaptionStyle;
        let mut values = std::collections::BTreeMap::new();
        assert_eq!(
            fallback_style(CaptionStyle::default(), &values).line_width,
            None
        );
        values.insert("width".to_string(), serde_json::json!(47.0));
        assert_eq!(
            fallback_style(CaptionStyle::default(), &values).line_width,
            Some(4700)
        );
        let set = CaptionStyle {
            line_width: Some(8000),
            ..CaptionStyle::default()
        };
        assert_eq!(
            fallback_style(set, &values).line_width,
            Some(4700),
            "the effect's value wins"
        );
        assert_eq!(
            classic_values(&set).get("width"),
            Some(&serde_json::json!(80.0))
        );
        // Out of range (an old or edited settings file): clamped as the fallback does.
        let wide = CaptionStyle {
            line_width: Some(12_000),
            ..CaptionStyle::default()
        };
        assert_eq!(
            classic_values(&wide).get("width"),
            Some(&serde_json::json!(100.0))
        );
    }
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
