//! Opt-in presentation fixtures and real-operation capture QA.
use super::*;

#[derive(Default)]
pub struct ScrubQa {
    start: Option<Instant>,
    last_input: Option<Instant>,
    last_frame: u64,
    samples: Vec<serde_json::Value>,
    handoff_step: usize,
    handoff: Vec<serde_json::Value>,
}
impl NohApp {
    pub(super) fn scrub_capture_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if std::env::var_os("NOH_CAPTURE_UI").is_none()
            || std::env::var("NOH_CAPTURE_SCRUB").as_deref() != Ok("1")
        {
            return;
        }
        let Some(scrubber) = &self.mini_preview.scrubber else {
            return;
        };
        let snapshot = scrubber.snapshot();
        if snapshot.frame > 0 && snapshot.error.is_none() && self.scrub_qa.start.is_none() {
            self.scrub_qa.start = Some(Instant::now());
        }
        let Some(start) = self.scrub_qa.start else {
            return;
        };
        let t = start.elapsed().as_secs_f64();
        if std::env::var("NOH_CAPTURE_SCRUB_HANDOFF").as_deref() == Ok("1") && t >= 13.0 {
            let steps = [
                (13.0, "play"),
                (15.0, "pause"),
                (16.0, "seek"),
                (18.0, "play"),
                (20.0, "pause"),
                (21.0, "settled"),
            ];
            if let Some(&(at, action)) = steps.get(self.scrub_qa.handoff_step)
                && t >= at
            {
                self.scrub_qa.handoff.push(serde_json::json!({"seconds":t,"before":action,"state":self.mini_capture_report()}));
                match action {
                    "play" | "pause" => self.mini_play(ctx),
                    "seek" => self.mini_seek(
                        self.mini_bounds().0 + (self.mini_bounds().1 - self.mini_bounds().0) / 2,
                        false,
                    ),
                    _ => (),
                }
                self.scrub_qa.handoff_step += 1;
            }
            input.events.push(egui::Event::PointerGone);
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        if snapshot.frame != self.scrub_qa.last_frame && self.scrub_qa.samples.len() < 2000 {
            self.scrub_qa.last_frame = snapshot.frame;
            self.scrub_qa.samples.push(serde_json::json!({"seconds":t,"time_ms":snapshot.time_ms,"frame":snapshot.frame,"request_to_ready_ms":snapshot.latency_ms,"error":snapshot.error,"decoded_frame":snapshot.decoded_frame,"source_seconds":snapshot.source_seconds}));
        }
        let Some(rect) = self.project_ui.timeline.pictures_rect() else {
            return;
        };
        if self
            .scrub_qa
            .last_input
            .is_none_or(|at| at.elapsed() >= Duration::from_secs_f64(1.0 / 60.0))
        {
            let ratio = if t < 4.0 {
                0.1 + 0.8 * t / 4.0
            } else if t < 8.0 {
                0.9 - 0.8 * (t - 4.0) / 4.0
            } else if t < 12.0 {
                0.1 + 0.8 * (1.0 - ((t - 8.0) % 1.0 * 2.0 - 1.0).abs())
            } else {
                0.333
            };
            let (from, to) = self.mini_bounds();
            let ms = from as f64 + (to - from) as f64 * ratio;
            let view = self
                .project
                .viewport
                .map(|v| v.0)
                .unwrap_or(noh::timeline::Range {
                    start_ms: 0,
                    end_ms: self.project.duration_ms,
                });
            let x = rect.left()
                + rect.width()
                    * ((ms - view.start_ms as f64) / (view.end_ms - view.start_ms) as f64) as f32;
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(x, rect.center().y)));
            self.scrub_qa.last_input = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}
/// Requires an explicit capture output; ordinary launches never start a QA job.
pub fn transcribe_enabled() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_TRANSCRIBE").as_deref() == Ok("1")
}
pub fn burn_enabled() -> Option<bool> {
    std::env::var_os("NOH_CAPTURE_UI")?;
    match std::env::var("NOH_CAPTURE_BURN").as_deref() {
        Ok("preview") => Some(true),
        Ok("export") => Some(false),
        Err(_) => None,
        _ => panic!("NOH_CAPTURE_BURN must be preview or export"),
    }
}
pub fn short_enabled() -> Option<bool> {
    std::env::var_os("NOH_CAPTURE_UI")?;
    match std::env::var("NOH_CAPTURE_SHORT").as_deref() {
        Ok("preview") => Some(true),
        Ok("export") => Some(false),
        Err(_) => None,
        _ => panic!("NOH_CAPTURE_SHORT must be preview or export"),
    }
}
/// Reproducible variations of a capture state (`NOH_CAPTURE_TWEAKS`).
#[derive(Clone, Debug, Default, PartialEq)]
// Optional variations for the fixed layout capture campaign.
#[allow(dead_code)]
pub struct Tweaks {
    pub safe_area: Option<noh::safe_area::SafeArea>,
    pub short_view: bool,
    pub fill: bool,
    pub restart: bool,
    pub open: Option<String>,
    pub scroll: f32,
    pub short_export: bool,
    /// `start`, `end` (inside the last frame) or project milliseconds.
    pub playhead: Option<String>,
}
pub fn parse_tweaks(value: &str) -> Result<Tweaks, String> {
    let mut tweaks = Tweaks::default();
    let mut seen = Vec::new();
    for pair in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, item) = pair
            .split_once('=')
            .ok_or_else(|| format!("Tweak {pair:?} is not key=value"))?;
        if seen.contains(&key) {
            return Err(format!("Tweak {key:?} is repeated"));
        }
        seen.push(key);
        match (key, item) {
            ("preset", "none" | "youtube_shorts" | "tiktok" | "reels" | "universal") => {
                tweaks.safe_area = Some(serde_json::from_value(serde_json::json!(item)).unwrap());
            }
            ("view", "short" | "video") => tweaks.short_view = item == "short",
            ("framing", "fit" | "fill") => tweaks.fill = item == "fill",
            ("restart", "0" | "1") => tweaks.restart = item == "1",
            ("export", "video" | "short") => tweaks.short_export = item == "short",
            ("open", "popover" | "presets" | "fades" | "volume" | "lyrics" | "song") => {
                tweaks.open = Some(item.into())
            }
            ("playhead", "start" | "end") => tweaks.playhead = Some(item.into()),
            ("playhead", _) if item.parse::<u64>().is_ok() => tweaks.playhead = Some(item.into()),
            ("scroll", _) => {
                tweaks.scroll = item
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or_else(|| format!("Scroll {item:?} is not a nonnegative number"))?
            }
            _ => return Err(format!("Unknown tweak {pair:?}")),
        }
    }
    Ok(tweaks)
}
/// The raw tweak string, validated; empty outside capture mode.
pub fn tweaks_text() -> String {
    if std::env::var_os("NOH_CAPTURE_UI").is_none() {
        return String::new();
    }
    let text = std::env::var("NOH_CAPTURE_TWEAKS").unwrap_or_default();
    if let Err(error) = parse_tweaks(&text) {
        panic!("NOH_CAPTURE_TWEAKS: {error}");
    }
    text.trim().to_owned()
}
/// Capture states applied over a loaded project.
const OVERLAY_STATES: &[&str] = &[
    "empty",
    "partial",
    "song-only",
    "ready",
    "unreadable",
    "lyrics-menu",
    "lyrics-generating",
    "lyrics-track-menu",
    "short",
    "exporting",
    "done",
    "stale",
    "exists",
    "details",
    "options",
    "analyzing",
    "cancelling",
];
const LYRICS_STATES: &[&str] = &[
    "unreadable",
    "lyrics-track-menu",
    "short",
    "exporting",
    "done",
    "stale",
    "exists",
];
const RANGE_STATES: &[&str] = &["short", "exporting", "done", "stale", "exists"];
impl NohApp {
    /// Applies the overlay state over the real project in steps, and says when
    /// the capture may be taken:
    /// 0. once the media are measured: lyrics, range 0:13,0–0:18,0, view, cursor;
    /// 1. once the diagnosis, waveform, lyrics and a monitor frame are ready:
    ///    the staged export (no worker runs);
    /// 2. ready, after one more frame so everything is painted.
    pub(super) fn apply_capture_overlay(&mut self, ctx: &egui::Context) -> bool {
        let Some(state) = self.capture_overlay.clone() else {
            return false;
        };
        let state = state.as_str();
        let tweaks = parse_tweaks(&tweaks_text()).expect("validated tweaks");
        match self.capture_overlay_step {
            0 => {
                if !self.ready() || self.project.duration_ms == 0 {
                    return false;
                }
                // Partial states remove from the measured project what the
                // capture state lacks, the way the song chip or ✕ would.
                if matches!(state, "empty" | "partial") {
                    self.wav = None;
                    self.wav_seconds = None;
                }
                if matches!(state, "empty" | "song-only") {
                    self.clips.clear();
                }
                if state == "unreadable"
                    && let Some(folder) = self.wav.as_ref().and_then(|w| w.parent())
                {
                    // The fixture MOV cut before its index, read for real.
                    let item = app_media::item_for_path(folder.join("concert.mov"));
                    self.next_id += 1;
                    self.metadata.request_media(
                        self.next_id,
                        item.clone(),
                        self.ffmpeg.clone(),
                        ctx,
                    );
                    self.clips.push(Clip {
                        id: self.next_id,
                        request_id: self.next_id,
                        item,
                        info: None,
                        error: None,
                    });
                }
                if matches!(state, "empty" | "partial" | "song-only" | "unreadable") {
                    self.invalidated();
                }
                if state == "empty" {
                    self.capture_overlay_step = 2;
                    return false;
                }
                if LYRICS_STATES.contains(&state)
                    && let Some(wav) = self.wav.clone()
                {
                    self.project.attach(wav.with_extension("srt"), Some(&wav));
                    // Show lyrics over the video in these capture states.
                    self.project.apply_subtitles = true;
                }
                if RANGE_STATES.contains(&state) {
                    let duration = self.project.duration_ms;
                    self.project
                        .set_range(noh::timeline::Range::new(13_000, 18_000, duration));
                    self.project.restart_loops = tweaks.restart;
                    self.project.framing = if tweaks.fill {
                        noh::shorts::Framing::Crop
                    } else {
                        noh::shorts::Framing::Pad
                    };
                    if let Some(area) = tweaks.safe_area {
                        self.project.set_short_safe_area(area);
                    }
                    self.project.viewport =
                        noh::timeline::Range::new(0, 30_000.min(duration), duration)
                            .map(noh::timeline::Viewport);
                    self.mini_preview.short = tweaks.short_view;
                }
                // Fixed capture playheads: 0:15,2 with a short, 1:12,4 otherwise.
                let cursor = if RANGE_STATES.contains(&state) {
                    15_200
                } else {
                    72_400
                };
                let (start, end) = self.mini_bounds();
                let cursor = match tweaks.playhead.as_deref() {
                    Some("start") => start,
                    // mini_seek keeps it inside the last frame, whatever the rate.
                    Some("end") => end,
                    Some(ms) => ms.parse().unwrap_or(cursor),
                    None => cursor,
                };
                self.mini_seek(cursor, false);
                self.capture_overlay_step = 1;
                false
            }
            1 => {
                let lyrics = !LYRICS_STATES.contains(&state) || self.project.track.loaded.is_some();
                let settled = match state {
                    // No song: the monitor shows the first visual's thumbnail.
                    "partial" => self
                        .clips
                        .first()
                        .is_some_and(|c| self.mini_preview.has_still(c.request_id)),
                    // No visuals: nothing to diagnose or decode.
                    "song-only" => self.project.waveform.is_some(),
                    // The faulty file blocks the diagnosis; wait for its error.
                    "unreadable" => {
                        self.clips.iter().any(|c| c.error.is_some())
                            && self.project.waveform.is_some()
                            && lyrics
                            && self.mini_preview.has_still(self.clips[0].request_id)
                    }
                    _ => {
                        self.diagnosis_current()
                            && self.project.waveform.is_some()
                            && lyrics
                            && self.mini_preview.has_frame()
                    }
                };
                if !settled {
                    return false;
                }
                if state == "lyrics-generating"
                    && let Some(wav) = self.wav.clone()
                {
                    // Staged generation only: no transcriber runs in capture mode.
                    self.subtitles.source = wav.display().to_string();
                    self.subtitles.running = true;
                    self.project.generating = true;
                    self.phase = "subtitle.preparing".into();
                    self.started = Some(Instant::now());
                }
                if matches!(state, "exporting" | "cancelling") {
                    // Staged progress only: no worker starts in capture mode.
                    self.project.running = Some(app_project::Running {
                        preview: false,
                        short: tweaks.short_export,
                        stamp: self.project.stamp(self.revision, tweaks.short_export),
                        preview_file: None,
                    });
                    self.fraction = 0.42;
                    self.phase = "progress.assemble".into();
                    self.started = Some(Instant::now());
                    self.cancelling = state == "cancelling";
                }
                if state == "analyzing" {
                    // Keep the real measured project, stage only the checking state.
                    self.inspecting = true;
                    self.analysis_phase = Some(Message::new(
                        "progress.analyze_clip",
                        &["2".into(), "3".into()],
                    ));
                    self.diagnosis_revision = None;
                }
                if matches!(state, "done" | "stale") {
                    // A staged export of the video, current or from before an
                    // edit; no file is written.
                    self.export_artifact = Some(app_state::Artifact {
                        path: self.output_folder.join(&self.output_name),
                        revision: if state == "done" {
                            self.revision
                        } else {
                            self.revision.wrapping_sub(1)
                        },
                        bytes: None,
                    });
                    self.last_export_short = false;
                }
                if state == "exists" {
                    // A name the person chose that a real file of the fixture
                    // folder already has: the sheet with the conflict.
                    self.output_auto = false;
                    self.output_name = "vagues.mp4".into();
                    self.destination_edited();
                    self.open_sheet(false, None);
                }
                if state == "options" {
                    self.settings_open = true;
                }
                if state == "details" {
                    self.open_sheet(false, None);
                    self.sheet.details = true;
                }
                self.capture_overlay_step = 2;
                false
            }
            // The sheet shows its own check of the name before the capture.
            // Options: after its fade-in, which a capture would catch half done.
            _ => {
                (!self.sheet.open || self.sheet_checked())
                    && (!self.settings_open || self.options_shown_for(ctx) > 0.4)
            }
        }
    }
}
pub fn number(key: &str, default: f32, min: f32, max: f32) -> f32 {
    if std::env::var_os("NOH_CAPTURE_UI").is_none() {
        return default;
    }
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(min, max))
        .unwrap_or(default)
}
impl NohApp {
    pub fn install_capture_fixture(&mut self) {
        if std::env::var_os("NOH_CAPTURE_UI").is_none() {
            return;
        }
        let transcribe = transcribe_enabled();
        let burn = burn_enabled();
        let short = short_enabled();
        if short.is_some() || std::env::var("NOH_CAPTURE_SHORTS").as_deref() == Ok("1") {
            assert!(
                !transcribe && burn.is_none(),
                "Capture one real operation at a time"
            );
            self.shorts.open = true;
            self.subtitles.open = false;
            self.captions.open = false;
            if short.is_some() {
                assert!(
                    std::env::var_os("NOH_CAPTURE_SHORT_PROJECT").is_some(),
                    "Real short capture requires NOH_CAPTURE_SHORT_PROJECT"
                );
                assert!(
                    std::env::var_os("NOH_CAPTURE_STATE").is_none(),
                    "Real short capture cannot use synthetic state"
                );
            }
            if let Some(path) = std::env::var_os("NOH_CAPTURE_SHORT_PROJECT") {
                let request: noh::shorts::ShortRequest = serde_json::from_slice(
                    &std::fs::read(path).expect("Capture short project file"),
                )
                .expect("Capture short request");
                self.shorts.source = request.source.display().to_string();
                self.shorts.output = request.output.display().to_string();
                self.shorts.start_ms = request.start_ms;
                self.shorts.end_ms = request.end_ms;
                self.shorts.framing = request.framing;
                self.shorts.captions_enabled = request.captions.is_some();
                if let Some(captions) = request.captions {
                    self.shorts.subtitles = captions.subtitles.display().to_string();
                    self.shorts.size = captions.style.size;
                    self.shorts.placement = captions.style.placement;
                    self.shorts.safe_area = captions.style.safe_area;
                }
                self.ffmpeg = Some(request.ffmpeg);
            }
            if let Ok(state) = std::env::var("NOH_CAPTURE_STATE") {
                assert!(
                    [
                        "empty",
                        "ready",
                        "rendering",
                        "preview",
                        "preview-stale",
                        "cancelling",
                        "failed",
                        "succeeded",
                        "succeeded-stale"
                    ]
                    .contains(&state.as_str()),
                    "Unknown short capture state"
                );
                self.capture_state = Some(state.clone());
                self.shorts.revision = 1;
                self.shorts.operation_revision = 1;
                if state != "empty" {
                    self.shorts.source = "C:/noh-qa/Source video.mp4".into();
                    self.shorts.subtitles = "C:/noh-qa/Reviewed captions.srt".into();
                    self.shorts.captions_enabled = true;
                    self.shorts.start_ms = 135;
                    self.shorts.end_ms = 1775;
                    self.shorts.output = "C:/noh-qa/output/Vertical clip.mp4".into();
                    self.shorts.destination_result = Some(app_destination::ResultView {
                        path: self.shorts.output.clone().into(),
                        issue: None,
                        suggestion: None,
                        bytes: None,
                    });
                }
                match state.as_str() {
                    "rendering" | "cancelling" => {
                        self.shorts.running = Some(false);
                        self.cancelling = state == "cancelling";
                        self.fraction = 0.42;
                        self.phase = "short.rendering".into();
                        self.started = Some(Instant::now());
                    }
                    "preview" | "preview-stale" => {
                        self.shorts.preview_artifact = Some(app_shorts::PreviewArtifact {
                            path: "C:/noh-qa/preview.mp4".into(),
                            revision: if state == "preview" { 1 } else { 0 },
                            snapshot: noh::inspection::Snapshot(vec![]),
                        })
                    }
                    "succeeded" | "succeeded-stale" => {
                        self.shorts.export_artifact = Some(app_state::Artifact {
                            path: self.shorts.output.clone().into(),
                            revision: if state == "succeeded" { 1 } else { 0 },
                            bytes: Some(48_300_000),
                        })
                    }
                    "failed" => {
                        self.shorts.failure = Some("error.short_range".into());
                        self.shorts.failure_revision = Some(1);
                        self.shorts.failure_detail = Some("The selected interval ends beyond the source video. Choose an earlier end time.".into());
                    }
                    _ => {}
                }
                if std::env::var("NOH_CAPTURE_SHORT_CAPTIONS").as_deref() == Ok("0") {
                    self.shorts.captions_enabled = false;
                }
                if let Ok(framing) = std::env::var("NOH_CAPTURE_SHORT_FRAMING") {
                    self.shorts.framing = match framing.as_str() {
                        "pad" => noh::shorts::Framing::Pad,
                        "crop" => noh::shorts::Framing::Crop,
                        _ => panic!("NOH_CAPTURE_SHORT_FRAMING must be pad or crop"),
                    };
                }
            }
            return;
        }
        if burn.is_some() || std::env::var("NOH_CAPTURE_CAPTIONS").as_deref() == Ok("1") {
            assert!(!transcribe, "Capture one real operation at a time");
            self.captions.open = true;
            self.subtitles.open = false;
            if burn.is_some() {
                assert!(
                    std::env::var_os("NOH_CAPTURE_CAPTION_PROJECT").is_some(),
                    "Real caption capture requires NOH_CAPTURE_CAPTION_PROJECT"
                );
                assert!(
                    std::env::var_os("NOH_CAPTURE_STATE").is_none(),
                    "Real caption capture cannot use synthetic state"
                );
            }
            if let Some(path) = std::env::var_os("NOH_CAPTURE_CAPTION_PROJECT") {
                let request: noh::captions::CaptionRequest = serde_json::from_slice(
                    &std::fs::read(path).expect("Capture caption project file"),
                )
                .expect("Capture caption request");
                self.captions.source = request.source.display().to_string();
                self.captions.subtitles = request.subtitles.display().to_string();
                self.captions.output = request.output.display().to_string();
                self.captions.size = request.style.size;
                self.captions.placement = request.style.placement;
                self.ffmpeg = Some(request.ffmpeg);
            }
            if let Ok(state) = std::env::var("NOH_CAPTURE_STATE") {
                assert!(
                    [
                        "empty",
                        "ready",
                        "rendering",
                        "preview",
                        "preview-stale",
                        "cancelling",
                        "failed",
                        "succeeded",
                        "succeeded-stale"
                    ]
                    .contains(&state.as_str()),
                    "Unknown caption capture state"
                );
                self.capture_state = Some(state.clone());
                self.captions.revision = 1;
                self.captions.operation_revision = 1;
                if state != "empty" {
                    self.captions.source = "C:/noh-qa/Source video.mp4".into();
                    self.captions.subtitles = "C:/noh-qa/Reviewed captions.srt".into();
                    self.captions.output = "C:/noh-qa/output/Captioned video.mp4".into();
                    self.captions.destination_result = Some(app_destination::ResultView {
                        path: self.captions.output.clone().into(),
                        issue: None,
                        suggestion: None,
                        bytes: None,
                    });
                }
                match state.as_str() {
                    "rendering" | "cancelling" => {
                        self.captions.running = Some(false);
                        self.cancelling = state == "cancelling";
                        self.fraction = 0.42;
                        self.phase = "caption.rendering".into();
                        self.started = Some(Instant::now());
                    }
                    "preview" | "preview-stale" => {
                        self.captions.preview_artifact = Some(app_captions::PreviewArtifact {
                            path: "C:/noh-qa/preview.mp4".into(),
                            revision: if state == "preview" { 1 } else { 0 },
                            snapshot: noh::inspection::Snapshot(vec![]),
                        })
                    }
                    "succeeded" | "succeeded-stale" => {
                        self.captions.export_artifact = Some(app_state::Artifact {
                            path: self.captions.output.clone().into(),
                            revision: if state == "succeeded" { 1 } else { 0 },
                            bytes: Some(48_300_000),
                        })
                    }
                    "failed" => {
                        self.captions.failure = Some("error.caption_layout".into());
                        self.captions.failure_revision = Some(1);
                        self.captions.failure_detail = Some("SRT block 2: unsupported character U+1F600. Choose text supported by the bundled caption font.".into());
                    }
                    _ => {}
                }
            }
            return;
        }
        if transcribe {
            assert!(
                std::env::var_os("NOH_CAPTURE_SUBTITLE_PROJECT").is_some(),
                "Real subtitle capture requires NOH_CAPTURE_SUBTITLE_PROJECT"
            );
            assert!(
                std::env::var_os("NOH_CAPTURE_STATE").is_none(),
                "Real subtitle capture cannot use a synthetic capture state"
            );
        }
        if transcribe || std::env::var("NOH_CAPTURE_SUBTITLES").as_deref() == Ok("1") {
            self.subtitles.open = true;
            if let Some(path) = std::env::var_os("NOH_CAPTURE_SUBTITLE_PROJECT") {
                let request: noh::subtitles::SubtitleRequest = serde_json::from_slice(
                    &std::fs::read(path).expect("Capture subtitle project file"),
                )
                .expect("Capture subtitle request");
                self.subtitles.source = request.source.display().to_string();
                self.subtitles.output = request.output.display().to_string();
                self.subtitles.transcriber = request.transcriber.display().to_string();
                self.subtitles.model = request.model.display().to_string();
                self.subtitles.vad_model = request.vad_model.display().to_string();
                self.subtitles.language = request.language;
                self.ffmpeg = Some(request.ffmpeg);
            }
            if let Ok(state) = std::env::var("NOH_CAPTURE_STATE") {
                assert!(
                    [
                        "empty",
                        "ready",
                        "analyzing",
                        "cancelling",
                        "failed",
                        "succeeded",
                        "succeeded-stale"
                    ]
                    .contains(&state.as_str()),
                    "Unknown subtitle capture state"
                );
                self.capture_state = Some(state.clone());
                self.subtitles.revision = 1;
                if state != "empty" {
                    self.subtitles.source = "C:/noh-qa/Speech source.wav".into();
                    self.subtitles.output = "C:/noh-qa/output/Speech subtitles.srt".into();
                    self.subtitles.transcriber = "C:/noh-qa/whisper/whisper-cli.exe".into();
                    self.subtitles.model = "C:/noh-qa/whisper/ggml-tiny.bin".into();
                    self.subtitles.vad_model = "C:/noh-qa/whisper/ggml-silero.bin".into();
                }
                match state.as_str() {
                    "analyzing" | "cancelling" => {
                        self.subtitles.running = true;
                        self.cancelling = state == "cancelling";
                        self.phase = "subtitle.transcribing".into();
                        self.started = Some(Instant::now());
                    }
                    "failed" => {
                        self.subtitles.failure = Some("error.subtitle_backend".into());
                        self.subtitles.failure_revision = Some(1);
                    }
                    "succeeded" | "succeeded-stale" => {
                        self.subtitles.artifact = Some(app_subtitles::Artifact {
                            path: self.subtitles.output.clone().into(),
                            revision: if state == "succeeded" { 1 } else { 0 },
                            cues: 42,
                        })
                    }
                    _ => {}
                }
            }
            return;
        }
        let Ok(state) = std::env::var("NOH_CAPTURE_STATE") else {
            return;
        };
        if matches!(state.as_str(), "resources" | "resources-options") {
            self.settings_open = state == "resources-options";
            self.resources.issues = [
                (noh::resources::Component::Ffmpeg, "bin/ffmpeg.exe"),
                (
                    noh::resources::Component::Speech,
                    "bin/speech/whisper-cli.exe",
                ),
                (
                    noh::resources::Component::Model,
                    "bin/speech/ggml-small.bin",
                ),
                (
                    noh::resources::Component::Vad,
                    "bin/speech/ggml-silero-v6.2.0.bin",
                ),
            ]
            .into_iter()
            .map(|(component, path)| noh::resources::Issue {
                component,
                path: Some(PathBuf::from(path)),
                detail: "Synthetic unavailable resource for presentation QA.".into(),
            })
            .collect();
            self.capture_state = Some(state);
            return;
        }
        if state == "components" {
            // Design-system specimen gallery, no project.
            self.capture_state = Some(state);
            return;
        }
        if std::env::var_os("NOH_CAPTURE_PROJECT").is_some() {
            // Layout matrix: the real project is loaded, decoded and measured as
            // usual; the state is applied over it once it is ready.
            assert!(
                OVERLAY_STATES.contains(&state.as_str()),
                "Unknown capture state over a project"
            );
            self.capture_overlay = Some(state);
            return;
        }
        assert!(
            [
                "empty",
                "ready",
                "mixed",
                "analyzing",
                "preview",
                "preview-stale",
                "cancelling",
                "failed",
                "succeeded",
                "succeeded-stale"
            ]
            .contains(&state.as_str()),
            "Unknown capture state"
        );
        self.capture_state = Some(state.clone());
        self.revision = 1;
        self.output_folder = PathBuf::from("C:/noh-qa/output");
        self.output_name = "montage.mp4".into();
        self.output = Some(self.output_folder.join(&self.output_name));
        self.destination_result = Some(app_destination::ResultView {
            path: self.output.clone().unwrap(),
            issue: None,
            suggestion: None,
            bytes: None,
        });
        if state == "empty" {
            return;
        }
        self.wav = Some(PathBuf::from("Soundtrack.wav"));
        self.wav_seconds = Some(182.4);
        self.wav_id = 100;
        for (index, name) in [
            "Morning light.mp4",
            "A very long video name — 動画 테스트 视频.mp4",
            "Evening.mp4",
        ]
        .iter()
        .enumerate()
        {
            let mut info = noh::media::MediaInfo::default();
            info.seconds = if index == 1 { 14.0 } else { 10.0 };
            info.width = 1920;
            info.height = 1080;
            info.fps = 25.0;
            // Use the existing worker representation to populate private source
            // metadata without adding a fixture constructor to the media API.
            let mut source = serde_json::to_value(info).unwrap();
            source["codec"] = if state == "mixed" && index == 1 {
                "vp9"
            } else {
                "h264"
            }
            .into();
            let info = serde_json::from_value(source).unwrap();
            self.clips.push(Clip {
                id: (index + 1) as u64,
                request_id: (index + 1) as u64,
                item: (*name).into(),
                info: Some(info),
                error: None,
            });
        }
        self.next_id = 100;
        let request = self.diagnostic_request().unwrap();
        let plan = noh::plan::RenderPlan {
            exact_inspection: true,
            stage: "preparation".into(),
            segments: vec![],
            target: noh::plan::Target {
                width: 1920,
                height: 1080,
                rate_num: 25,
                rate_den: 1,
                codec: "h264".into(),
                pixel_format: "yuv420p".into(),
            },
            clips: self
                .clips
                .iter()
                .enumerate()
                .map(|(i, c)| noh::plan::ClipPlan {
                    path: c.item.path().clone(),
                    treatment: if state == "mixed" && i == 1 {
                        noh::plan::Treatment::Convert
                    } else {
                        noh::plan::Treatment::Copy
                    },
                    reasons: if state == "mixed" && i == 1 {
                        vec!["warning.partial_codec".into(), "dimensions".into()]
                    } else {
                        vec![]
                    },
                    timestamp_normalization: true,
                    source_seconds: match &c.item {
                        noh::input::MediaItem::Image { duration, .. } => *duration,
                        noh::input::MediaItem::Video { .. } => c.info.as_ref().unwrap().seconds,
                    },
                })
                .collect(),
        };
        self.diagnosis = Some(Box::new(noh::inspection::Diagnosis {
            version: 1,
            level: noh::inspection::Level::Exact,
            request,
            snapshot: noh::inspection::Snapshot(vec![]),
            duration: 182.4,
            container: "mp4".into(),
            audio: "aac_320".into(),
            quick_media: vec![],
            notes: vec!["diagnosis.partial".into(), "diagnosis.color_limit".into()],
            plan: Some(plan),
        }));
        self.diagnosis_revision = Some(self.revision);
        match state.as_str() {
            "analyzing" => {
                self.diagnosis = None;
                self.diagnosis_revision = None;
                self.inspecting = true;
                self.analysis_phase = Some("progress.analyze_clip|2|3".into());
                self.analysis_fraction = 0.46;
            }
            "preview" | "preview-stale" => {
                self.preview_artifact = Some(app_state::Artifact {
                    path: "preview.mp4".into(),
                    revision: if state == "preview-stale" { 0 } else { 1 },
                    bytes: Some(1_400_000),
                });
                self.preview_ready = state == "preview";
            }
            "cancelling" => {
                self.cancelling = true;
                self.fraction = 0.46;
                self.phase = "progress.assemble".into();
                self.started = Some(Instant::now());
            }
            "failed" => {
                self.error = Some("error.video".into());
                self.failure_revision = Some(1);
                self.failure_path = Some(self.clips[1].item.path().clone());
                self.clips[1].error = Some("error.video".into());
            }
            "succeeded" | "succeeded-stale" => {
                self.export_artifact = Some(app_state::Artifact {
                    path: self.output.clone().unwrap(),
                    revision: if state == "succeeded-stale" { 0 } else { 1 },
                    bytes: Some(48_300_000),
                });
                self.completed = self.output.clone();
                self.destination_result = Some(app_destination::ResultView {
                    path: self.output.clone().unwrap(),
                    issue: Some(app_destination::Issue::Exists),
                    suggestion: Some("montage-2.mp4".into()),
                    bytes: Some(48_300_000),
                });
            }
            // Show processing details in the location sheet.
            "mixed" => {
                self.open_sheet(false, None);
                self.sheet.details = true;
            }
            _ => {}
        }
    }
    /// A terminal event alone is insufficient: receive() must first reap the job.
    fn subtitle_capture_terminal(&self) -> Option<&'static str> {
        if self.job.is_some() || self.subtitles.running || self.cancelling {
            return None;
        }
        if self.subtitles.failure_revision == Some(self.subtitles.revision)
            && self.subtitles.failure.is_some()
        {
            return Some("failed");
        }
        if self
            .subtitles
            .outcome
            .as_ref()
            .is_some_and(|message| message.key == "ui.subtitle_cancelled")
        {
            return Some("cancelled");
        }
        self.subtitles
            .artifact
            .as_ref()
            .filter(|artifact| artifact.revision == self.subtitles.revision)
            .map(|_| "succeeded")
    }
    fn caption_capture_terminal(&self, preview: bool) -> Option<&'static str> {
        if self.job.is_some() || self.captions.running.is_some() || self.cancelling {
            return None;
        }
        if self.captions.failure_revision == Some(self.captions.revision)
            && self.captions.failure.is_some()
        {
            return Some("failed");
        }
        if self
            .captions
            .outcome
            .as_ref()
            .is_some_and(|m| m.key == "ui.caption_cancelled")
        {
            return Some("cancelled");
        }
        if preview && self.captions.preview_current() {
            return Some("preview");
        }
        if !preview
            && self
                .captions
                .export_artifact
                .as_ref()
                .is_some_and(|a| a.revision == self.captions.revision)
        {
            return Some("succeeded");
        }
        None
    }
    fn short_capture_terminal(&self, preview: bool) -> Option<&'static str> {
        if self.job.is_some() || self.shorts.running.is_some() || self.cancelling {
            return None;
        }
        if self.shorts.failure_revision == Some(self.shorts.revision)
            && self.shorts.failure.is_some()
        {
            return Some("failed");
        }
        if self
            .shorts
            .outcome
            .as_ref()
            .is_some_and(|m| m.key == "ui.short_cancelled")
        {
            return Some("cancelled");
        }
        if preview && self.shorts.preview_current() {
            return Some("preview");
        }
        if !preview
            && self
                .shorts
                .export_artifact
                .as_ref()
                .is_some_and(|a| a.revision == self.shorts.revision)
        {
            return Some("succeeded");
        }
        None
    }
    pub fn capture_frame(&mut self, ctx: &egui::Context) {
        let Some(path) = std::env::var_os("NOH_CAPTURE_UI") else {
            return;
        };
        let project_action = std::env::var("NOH_CAPTURE_PROJECT_ACTION").ok();
        if let Some(action) = project_action.as_deref()
            && !self.capture_project_started
            && self.ready()
            && self.diagnosis_current()
            && !self.project.track.loading()
            && !self.project.waveform_loading
        {
            self.capture_project_started = true;
            match action {
                "preview" => self.start_project(ctx, true, false),
                "export" => self.start_project(ctx, false, false),
                "short-preview" => self.start_project(ctx, true, true),
                "short-export" => self.start_project(ctx, false, true),
                "generate" => self.project_generate(ctx),
                "live-preview" => self.mini_capture_start(false, ctx),
                "live-short" => self.mini_capture_start(true, ctx),
                _ => panic!("Unknown capture project action"),
            }
        }
        let populated = std::env::var_os("NOH_CAPTURE_PROJECT").is_some()
            || std::env::var_os("NOH_CAPTURE_SUBTITLE_PROJECT").is_some()
            || std::env::var_os("NOH_CAPTURE_CAPTION_PROJECT").is_some()
            || std::env::var_os("NOH_CAPTURE_SHORT_PROJECT").is_some();
        let overlay_ready = self.apply_capture_overlay(ctx);
        // `open=` tweaks open a stage popup once the stage is ready, the way a
        // click on its button would (the Fades popup).
        if self.capture_overlay_step >= 1
            && let Ok(tweaks) = parse_tweaks(&tweaks_text())
            && let Some(popup) = match tweaks.open.as_deref() {
                Some("fades") => Some("fades-popup"),
                Some("volume") => Some("volume-popup"),
                Some("popover") => Some(app_range::TIMES),
                Some("presets") => Some(app_range::TIMES),
                _ => None,
            }
            && !egui::Popup::is_id_open(ctx, egui::Id::new(popup))
        {
            egui::Popup::open_id(ctx, egui::Id::new(popup));
        }
        // Capture the lyrics chip's menu open.
        if self.capture_overlay_step >= 1
            && matches!(
                self.capture_overlay.as_deref(),
                Some("lyrics-menu" | "lyrics-track-menu")
            )
            && !egui::Popup::is_id_open(ctx, egui::Id::new(app_lyrics::MENU))
        {
            egui::Popup::open_id(ctx, egui::Id::new(app_lyrics::MENU));
        }
        let fixture = self.capture_state.is_some() || self.capture_overlay.is_some();
        let transcribe = transcribe_enabled();
        let burn = burn_enabled();
        let short = short_enabled();
        let actual = transcribe || burn.is_some() || short.is_some() || project_action.is_some();
        let terminal = if project_action
            .as_deref()
            .is_some_and(|a| a.starts_with("live-"))
        {
            self.mini_capture_terminal()
        } else if project_action.is_some() {
            if self.capture_project_started
                && self.job.is_none()
                && !self.project.busy()
                && !self.cancelling
            {
                Some(
                    if self.error.is_some() || self.subtitles.failure.is_some() {
                        "failed"
                    } else {
                        "finished"
                    },
                )
            } else {
                None
            }
        } else if let Some(preview) = short {
            self.short_capture_terminal(preview)
        } else if transcribe {
            self.subtitle_capture_terminal()
        } else {
            burn.and_then(|preview| self.caption_capture_terminal(preview))
        };
        if terminal.is_some() && self.capture_terminal_at.is_none() {
            self.capture_terminal_at = Some(Instant::now());
        }
        // Real operations retain their engine/controller deadlines. Only geometry
        // settling after a reaped terminal gets the existing capture timeout.
        let timed_out = if actual {
            self.capture_terminal_at
                .is_some_and(|at| at.elapsed() > Duration::from_secs(15))
        } else {
            // Real media over the overlay need decoding, waveform and diagnosis.
            let limit = if self.capture_overlay.is_some() {
                25
            } else {
                15
            };
            self.opened.elapsed() > Duration::from_secs(limit)
        };
        let viewport = ctx.viewport_rect();
        let width = number("NOH_CAPTURE_WIDTH", 980.0, 420.0, 1920.0);
        let height = number("NOH_CAPTURE_HEIGHT", 850.0, 540.0, 2160.0);
        let scale = number("NOH_CAPTURE_SCALE", 1.0, 1.0, 2.0);
        // Cocoa rounds window sizes to physical pixels. At 1.5x, a one-pixel
        // rounding difference exceeds half a logical point.
        let geometry_ready = (viewport.width() - width).abs() * scale <= 1.0
            && (viewport.height() - height).abs() * scale <= 1.0
            && (ctx.pixels_per_point() - scale).abs() < 0.01;
        if !geometry_ready {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
        }
        let mut capture_ready = geometry_ready
            && if self.capture_overlay.is_some() {
                overlay_ready
            } else if actual {
                terminal.is_some()
                    && !self.project.track.validating()
                    && !self.project.waveform_loading
            } else {
                fixture
                    || !populated
                    || (self.subtitles.open && self.subtitles.issue().is_none())
                    || (self.shorts.open
                        && self
                            .shorts
                            .request(self.ffmpeg.as_ref(), false)
                            .validate()
                            .is_ok())
                    || (self.captions.open
                        && self
                            .captions
                            .request(self.ffmpeg.as_ref(), false)
                            .validate()
                            .is_ok())
                    || (self.ready()
                        && self.diagnosis_current()
                        && !self.project.waveform_loading
                        && !self.project.track.loading())
                    || self.diagnosis_error.is_some()
            };
        if let Some(percent) = std::env::var("NOH_CAPTURE_TRANSCRIPTION_PROGRESS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|p| (1..100).contains(p))
        {
            // Capture a genuine backend update while the job is still running.
            capture_ready =
                geometry_ready && self.subtitles.running && self.fraction >= percent as f32 / 100.0;
        }
        let scrub_qa = std::env::var("NOH_CAPTURE_SCRUB").as_deref() == Ok("1");
        if scrub_qa {
            let failed = self.mini_preview.scrubber.as_ref().is_none_or(|s| {
                let snapshot = s.snapshot();
                snapshot.error.is_some() && !snapshot.fallback
            });
            let duration = if std::env::var("NOH_CAPTURE_SCRUB_HANDOFF").as_deref() == Ok("1") {
                22
            } else {
                13
            };
            capture_ready &= failed
                || self
                    .scrub_qa
                    .start
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(duration));
        }
        let timed_out = if scrub_qa {
            self.opened.elapsed() > Duration::from_secs(75)
        } else {
            timed_out
        };
        if !self.capture_requested
            && self.opened.elapsed() > Duration::from_secs(2)
            && (capture_ready || timed_out)
        {
            let report = serde_json::json!({"requested_tweaks":tweaks_text(),"requested_state":std::env::var("NOH_CAPTURE_STATE").unwrap_or_else(|_|if populated {"project"} else {"empty"}.into()),"captured_state":self.capture_state.clone().or_else(||self.capture_overlay.clone()).unwrap_or_else(||if populated {"project"} else {"empty"}.into()),"capture_ready":capture_ready,"timed_out":timed_out,"viewport":{"width":viewport.width(),"height":viewport.height()},"pixels_per_point":ctx.pixels_per_point(),"fixture":fixture,"section":std::env::var("NOH_CAPTURE_SECTION").ok(),"revision":self.revision,"ready":self.ready(),"diagnosis":self.diagnosis,"diagnosis_error":self.diagnosis_error.as_ref().map(|e|&e.key),"log":self.log});
            let mut report = report;
            report["resource_issues"] = serde_json::json!(self.resources.issues.len());
            report["project_action"] = serde_json::json!(project_action);
            report["mini_preview"] = self.mini_capture_report();
            if scrub_qa {
                let snapshot = self.mini_preview.scrubber.as_ref().map(|s| s.snapshot());
                report["scrub"] = serde_json::json!({"samples":self.scrub_qa.samples,"error":snapshot.and_then(|s|s.error),"runtime":self.mini_preview.scrubber.is_some()});
                report["scrub"]["handoff"] = serde_json::json!(self.scrub_qa.handoff);
            }
            report["project_terminal"] = serde_json::json!(terminal);
            report["project_range"] = serde_json::json!(self.project.range);
            report["project_restart"] = serde_json::json!(self.project.restart_loops);
            report["project_waveform"] = serde_json::json!(self.project.waveform.is_some());
            report["project_waveform_error"] = serde_json::json!(self.project.waveform_error);
            report["project_track_cues"] = serde_json::json!(
                self.project
                    .track
                    .loaded
                    .as_ref()
                    .map(|t| t.track.cues.len())
            );
            report["project_track_error"] =
                serde_json::json!(self.project.track.error.as_ref().map(|e| &e.detail));
            report["speech_setup"] = serde_json::json!({
                "configured": self.subtitles.configured(),
                "runtime": self.subtitles.transcriber,
                "model": self.subtitles.model,
                "vad": self.subtitles.vad_model,
            });
            report["operation_progress"] = serde_json::json!({
                "percent": (self.fraction * 100.0).round() as u32,
                "phase": self.phase.key,
            });
            report["project_short_preview_current"] =
                serde_json::json!(self.project.short_preview_current(self.revision));
            report["project_short_export_current"] =
                serde_json::json!(self.project.short_export_current(self.revision));
            report["project_short_export"] =
                serde_json::json!(self.project.short_export_artifact.as_ref().map(|a| &a.path));
            report["project_failure"] = serde_json::json!(self.error.as_ref().map(|e| &e.key));
            if transcribe {
                report["requested_state"] = serde_json::json!("transcribe");
                report["captured_state"] = serde_json::json!(terminal.unwrap_or("transcribing"));
            }
            if let Some(preview) = burn {
                report["requested_state"] = serde_json::json!(if preview {
                    "burn-preview"
                } else {
                    "burn-export"
                });
                report["captured_state"] = serde_json::json!(terminal.unwrap_or("rendering"));
            }
            report["caption_workspace"] = serde_json::json!(self.captions.open);
            report["caption_revision"] = serde_json::json!(self.captions.revision);
            report["caption_operation"] =
                serde_json::json!(burn.or(self.captions.running).map(|p| if p {
                    "preview"
                } else {
                    "export"
                }));
            report["caption_terminal"] = serde_json::json!(burn.is_some() && terminal.is_some());
            report["caption_preview_current"] = serde_json::json!(self.captions.preview_current());
            report["caption_export_current"] = serde_json::json!(
                self.captions
                    .export_artifact
                    .as_ref()
                    .is_some_and(|a| a.revision == self.captions.revision)
            );
            report["caption_failure"] = serde_json::json!(
                self.captions
                    .failure
                    .as_ref()
                    .filter(|_| self.captions.failure_revision == Some(self.captions.revision))
                    .map(|m| &m.key)
            );
            report["caption_failure_detail"] = serde_json::json!(self.captions.failure_detail);
            report["caption_preview"] = serde_json::json!(
                self.captions
                    .preview_artifact
                    .as_ref()
                    .map(|a| serde_json::json!({"path":a.path,"revision":a.revision}))
            );
            report["caption_export"] = serde_json::json!(
                self.captions
                    .export_artifact
                    .as_ref()
                    .map(|a| serde_json::json!({"path":a.path,"revision":a.revision}))
            );
            if let Some(preview) = short {
                report["requested_state"] = serde_json::json!(if preview {
                    "short-preview"
                } else {
                    "short-export"
                });
                report["captured_state"] = serde_json::json!(terminal.unwrap_or("rendering"));
            }
            report["short_workspace"] = serde_json::json!(self.shorts.open);
            report["short_revision"] = serde_json::json!(self.shorts.revision);
            report["short_operation"] =
                serde_json::json!(short.or(self.shorts.running).map(|p| if p {
                    "preview"
                } else {
                    "export"
                }));
            report["short_terminal"] = serde_json::json!(short.is_some() && terminal.is_some());
            report["short_preview_current"] = serde_json::json!(self.shorts.preview_current());
            report["short_export_current"] = serde_json::json!(
                self.shorts
                    .export_artifact
                    .as_ref()
                    .is_some_and(|a| a.revision == self.shorts.revision)
            );
            report["short_failure"] = serde_json::json!(
                self.shorts
                    .failure
                    .as_ref()
                    .filter(|_| self.shorts.failure_revision == Some(self.shorts.revision))
                    .map(|m| &m.key)
            );
            report["short_failure_detail"] = serde_json::json!(self.shorts.failure_detail);
            report["short_request"] = serde_json::json!(
                self.shorts
                    .request(self.ffmpeg.as_ref(), short.unwrap_or(false))
            );
            report["short_warnings"] = serde_json::json!(
                self.shorts
                    .warnings
                    .iter()
                    .map(|m| serde_json::json!({"key":m.key,"args":m.args}))
                    .collect::<Vec<_>>()
            );
            report["short_preview"] = serde_json::json!(
                self.shorts
                    .preview_artifact
                    .as_ref()
                    .map(|a| serde_json::json!({"path":a.path,"revision":a.revision}))
            );
            report["short_export"] = serde_json::json!(
                self.shorts
                    .export_artifact
                    .as_ref()
                    .map(|a| serde_json::json!({"path":a.path,"revision":a.revision}))
            );
            report["subtitle_transcribe"] = serde_json::json!(transcribe);
            report["subtitle_terminal"] = serde_json::json!(transcribe && terminal.is_some());
            report["subtitle_workspace"] = serde_json::json!(self.subtitles.open);
            report["subtitle_revision"] = serde_json::json!(self.subtitles.revision);
            report["subtitle_ready"] = serde_json::json!(self.subtitles.issue().is_none());
            report["subtitle_current"] = serde_json::json!(
                self.subtitles
                    .artifact
                    .as_ref()
                    .is_some_and(|a| a.revision == self.subtitles.revision)
            );
            report["subtitle_artifact"] = serde_json::json!(self.subtitles.artifact.as_ref().map(
                |artifact| serde_json::json!({
                    "path": artifact.path,
                    "revision": artifact.revision,
                    "cues": artifact.cues
                })
            ));
            report["subtitle_failure"] =
                serde_json::json!(
                self.subtitles.failure.as_ref()
                    .filter(|_| self.subtitles.failure_revision == Some(self.subtitles.revision))
                    .map(|failure| &failure.key)
            );
            report["subtitle_outcome"] =
                serde_json::json!(self.subtitles.outcome.as_ref().map(|outcome| &outcome.key));
            #[cfg(feature = "updates")]
            {
                report["update_layout_fixture"] = serde_json::json!(self.updates.capture_fixture);
                report["update_safe_close"] = serde_json::json!(self.updates.confirm_install);
                report["update_qualification_step"] =
                    serde_json::json!(self.updates.qualification_step);
                report["update_qualification_automated_consent"] =
                    serde_json::json!(self.updates.qualification_step >= 2);
                report["update_state"] = serde_json::json!(match self.updates.view.state {
                    noh::update::State::Idle => "idle",
                    noh::update::State::Checking => "checking",
                    noh::update::State::Available { .. } => "available",
                    noh::update::State::Downloading { .. } => "downloading",
                    noh::update::State::Verifying => "verifying",
                    noh::update::State::Ready { .. } => "ready",
                    noh::update::State::Applying => "applying",
                    noh::update::State::Failed { .. } => "failed",
                });
            }
            let _ = std::fs::write(
                PathBuf::from(&path).with_extension("json"),
                serde_json::to_vec_pretty(&report).unwrap(),
            );
            self.capture_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        for event in ctx.input(|i| i.raw.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                let mut ppm =
                    format!("P6\n{} {}\n255\n", image.size[0], image.size[1]).into_bytes();
                for p in &image.pixels {
                    ppm.extend_from_slice(&[p.r(), p.g(), p.b()]);
                }
                let _ = std::fs::write(&path, ppm);
                // The synthetic cancelling scene has no job to stop. Its saved
                // frame must not open the ordinary close-confirmation dialog.
                if fixture {
                    self.cancelling = false;
                    self.subtitles.running = false;
                    self.captions.reaped();
                    self.shorts.reaped();
                }
                if std::env::var("NOH_CAPTURE_KEEP_OPEN").as_deref() != Ok("1")
                    && !(std::env::var_os("NOH_CAPTURE_TRANSCRIPTION_PROGRESS").is_some()
                        && self.subtitles.running)
                {
                    if self.capture_overlay.is_some() {
                        // Staged work has no worker: it must not ask to stop.
                        self.project.running = None;
                        self.project.generating = false;
                        self.subtitles.running = false;
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // A progress screenshot is taken mid-job. Let the real operation finish
        // before closing, so capture automation never opens the busy-close dialog.
        if self.capture_requested
            && std::env::var_os("NOH_CAPTURE_TRANSCRIPTION_PROGRESS").is_some()
            && std::env::var("NOH_CAPTURE_KEEP_OPEN").as_deref() != Ok("1")
            && self
                .capture_terminal_at
                .is_some_and(|at| at.elapsed() > Duration::from_millis(500))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_tweaks_cover_state_variants_and_reject_unknown_keys() {
        assert_eq!(parse_tweaks(""), Ok(Tweaks::default()));
        let tweaks =
            parse_tweaks("restart=1,view=short,framing=fill,open=popover,scroll=320").unwrap();
        assert!(tweaks.restart && tweaks.short_view && tweaks.fill);
        assert_eq!(tweaks.open.as_deref(), Some("popover"));
        assert_eq!(tweaks.scroll, 320.0);
        assert!(parse_tweaks("export=short").unwrap().short_export);
        for invalid in [
            "view",
            "view=left",
            "zoom=2",
            "scroll=-1",
            "restart=1,restart=0",
        ] {
            assert!(parse_tweaks(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn real_short_capture_waits_for_reaping_and_accepts_terminal_failure() {
        let mut app = NohApp {
            cancelling: true,
            ..Default::default()
        };
        app.shorts.running = Some(false);
        assert!(
            app.receive_job_event(Event::Done(Ok(noh::engine::ExportResult {
                output: "short.mp4".into(),
                duration: 1.64
            })))
        );
        assert!(app.shorts.export_artifact.is_some());
        assert_eq!(app.short_capture_terminal(false), None);
        app.shorts.reaped();
        app.cancelling = false;
        assert_eq!(app.short_capture_terminal(false), Some("succeeded"));
        app.shorts.edited();
        assert_eq!(app.short_capture_terminal(false), None);
        app.shorts.fail(
            &noh::engine::EngineError::new(
                "error.short_range",
                "inspect",
                None,
                "Selection exceeds the source duration",
            ),
            app.shorts.revision,
        );
        assert_eq!(app.short_capture_terminal(false), Some("failed"));
    }

    #[test]
    fn real_subtitle_capture_waits_for_reaping_and_accepts_terminal_errors() {
        let mut app = NohApp::default();
        app.subtitles.running = true;
        app.cancelling = true;
        let result = noh::subtitles::SubtitleResult {
            output: "speech.srt".into(),
            track: noh::subtitle_track::SubtitleTrack {
                language: None,
                duration_ms: 1000,
                cues: vec![],
            },
        };
        assert!(app.receive_job_event(Event::Subtitled(Ok(result))));
        assert!(app.subtitles.artifact.is_some());
        assert_eq!(app.subtitle_capture_terminal(), None);
        // receive() clears these only after taking/dropping the shared Job.
        app.subtitles.running = false;
        app.cancelling = false;
        assert_eq!(app.subtitle_capture_terminal(), Some("succeeded"));

        app.subtitles.edited();
        assert_eq!(app.subtitle_capture_terminal(), None);
        app.subtitles.running = true;
        app.subtitles.operation_revision = app.subtitles.revision;
        assert!(
            app.receive_job_event(Event::Subtitled(Err(noh::engine::EngineError::new(
                "error.timeout",
                "transcribe",
                None,
                "Timeout"
            ))))
        );
        assert_eq!(app.subtitle_capture_terminal(), None);
        app.subtitles.running = false;
        assert_eq!(app.subtitle_capture_terminal(), Some("failed"));
        app.subtitles.failure_revision = Some(app.subtitles.revision - 1);
        assert_eq!(app.subtitle_capture_terminal(), None);
    }
}

/// Capture-only gallery of the design-system components, laid out like the
/// design system's own specimen pages so both can be compared side by side.
/// The sample texts are the specimens' French wording, not catalog strings.
impl NohApp {
    pub(super) fn components_gallery(
        &mut self,
        ui: &mut egui::Ui,
    ) -> egui::scroll_area::ScrollAreaOutput<()> {
        use app_icons::Icon;
        use app_style::{self as style, Kind, space, text};
        let t = style::tokens(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(t.bg).inner_margin(16.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("components")
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                        let section = |ui: &mut egui::Ui, title: &str| {
                            ui.add_space(space::XS);
                            ui.label(egui::RichText::new(title).size(text::SMALL).color(t.text_3));
                        };
                        section(ui, "Type");
                        ui.label(style::strong(
                            "Déposez vos images, vidéos et votre chanson",
                            text::TITLE,
                        ));
                        ui.label(style::strong("Exporter la vidéo", text::HEADING));
                        ui.horizontal(|ui| {
                            ui.label("Images depuis le début");
                            ui.label(style::strong("Prêt à exporter", text::BODY));
                            ui.label(
                                egui::RichText::new("Durée : 3:02")
                                    .size(text::SMALL)
                                    .color(t.text_2),
                            );
                            ui.label(style::strong("Extrait 5,0 s", text::SMALL));
                        });
                        ui.horizontal(|ui| {
                            ui.label("動画を書き出す · 歌詞を確認");
                            ui.label("0:13,0 – 0:18,0 · 1:12,4 / 3:02,4");
                        });
                        section(ui, "Button");
                        ui.horizontal(|ui| {
                            style::button_kind(
                                ui,
                                "Exporter la vidéo",
                                true,
                                Kind::Primary,
                                0.0,
                                style::CONTROL_H_LG,
                            );
                            style::button_kind(
                                ui,
                                "Exporter l’extrait",
                                true,
                                Kind::Secondary,
                                0.0,
                                style::CONTROL_H_LG,
                            );
                            style::button_kind(
                                ui,
                                "Annuler",
                                true,
                                Kind::Quiet,
                                0.0,
                                style::CONTROL_H_LG,
                            );
                            let focused = style::button_kind(
                                ui,
                                "Exporter",
                                true,
                                Kind::Primary,
                                0.0,
                                style::CONTROL_H_LG,
                            );
                            focused.request_focus();
                            style::icon_button(ui, Icon::Plus, "Zoom avant", style::CONTROL_H);
                            style::icon_button(
                                ui,
                                Icon::Close,
                                "Supprimer l’extrait",
                                style::ROW_ICON,
                            );
                        });
                        ui.horizontal(|ui| {
                            style::button_kind(
                                ui,
                                "Exporter la vidéo",
                                false,
                                Kind::Primary,
                                0.0,
                                style::CONTROL_H_LG,
                            );
                            ui.label(
                                egui::RichText::new("Ajoutez un fichier .wav pour exporter.")
                                    .size(text::SMALL)
                                    .color(t.text_2),
                            );
                        });
                        section(ui, "Checkbox and switch");
                        ui.horizontal(|ui| {
                            style::checkbox(ui, &mut false, "Images depuis le début");
                            style::checkbox(ui, &mut true, "Images depuis le début");
                            ui.label("Afficher sur la vidéo");
                            style::switch(ui, &mut true, "Afficher sur la vidéo");
                            ui.label("Afficher sur la vidéo");
                            style::switch(ui, &mut false, "Afficher sur la vidéo");
                        });
                        section(ui, "Segmented control");
                        ui.horizontal(|ui| {
                            style::segmented(ui, "view", &["Vidéo", "Extrait"], 0);
                            style::segmented(ui, "framing", &["Image entière", "Plein cadre"], 0);
                            style::segmented(ui, "mode", &["Automatique", "Tout convertir"], 0);
                        });
                        section(ui, "Tag");
                        ui.horizontal(|ui| {
                            style::tag(ui, "anciens réglages", true);
                            style::tag(ui, "39 lignes", false);
                        });
                        section(ui, "Icons");
                        ui.horizontal_wrapped(|ui| {
                            for icon in Icon::ALL {
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(18.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                app_icons::paint(ui.painter(), rect, icon, t.text_2);
                                response.on_hover_text(format!("{icon:?}"));
                            }
                        });
                        section(ui, "Colour");
                        ui.horizontal_wrapped(|ui| {
                            for color in [
                                t.bg,
                                t.surface,
                                t.surface_2,
                                t.surface_3,
                                t.border_strong,
                                t.text,
                                t.text_2,
                                t.text_3,
                                t.accent,
                                t.accent_text,
                                t.seq[0],
                                t.seq[1],
                                t.seq[2],
                                t.info,
                                t.warn,
                                t.error,
                                t.ok,
                            ] {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(40.0, 24.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, style::radius::S, color);
                                ui.painter().rect_stroke(
                                    rect,
                                    style::radius::S,
                                    egui::Stroke::new(1.0, t.border),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        });
                    })
            })
            .inner
    }
}
