//! Shared desktop project additions, independent of their eventual presentation.
use super::{
    app_destination,
    app_job::PreviewFile,
    app_project_ops::{Prepared, Verified},
    app_track,
};
use noh::{
    captions::CaptionStyle,
    engine::EngineError,
    inspection::{FileStamp, Snapshot},
    shorts::Framing,
    timeline::{Range, Viewport},
    waveform::{Waveform, WaveformWorker},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub revision: u64,
    pub short_revision: Option<u64>,
    pub captions: Option<(u64, CaptionStyle)>,
}
pub struct ProjectArtifact {
    pub path: PathBuf,
    pub stamp: Stamp,
}
pub struct Running {
    pub preview: bool,
    pub short: bool,
    pub stamp: Stamp,
    pub preview_file: Option<PreviewFile>,
}
pub(super) struct Generation {
    pub wav_id: u64,
    pub wav: PathBuf,
    pub track_generation: u64,
}

#[derive(Default)]
pub struct State {
    pub track: app_track::Attachment,
    pub apply_subtitles: bool,
    pub caption_style: CaptionStyle,
    pub range: Option<Range>,
    /// A replacement song shortened the selection; shown until the next edit.
    pub range_moved: bool,
    pub viewport: Option<Viewport>,
    pub restart_loops: bool,
    pub framing: Framing,
    pub short_safe_area: noh::safe_area::SafeArea,
    pub hide_safe_area: bool,
    /// Changes confined to the short do not invalidate the full montage.
    pub short_revision: u64,
    pub waveform: Option<Arc<Waveform>>,
    pub waveform_loading: bool,
    pub waveform_error: Option<String>,
    pub duration_ms: u64,
    pub running: Option<Running>,
    pub generating: bool,
    pub failure: Option<noh::i18n::Message>,
    pub outcome: Option<noh::i18n::Message>,
    pub short_name: String,
    /// The person chose the short's file name: a taken one is never replaced.
    pub short_named: bool,
    pub short_destination_result: Option<app_destination::ResultView>,
    pub short_preview: Option<PreviewFile>,
    pub short_preview_artifact: Option<ProjectArtifact>,
    pub short_export_artifact: Option<ProjectArtifact>,
    pub full_preview_artifact: Option<ProjectArtifact>,
    pub full_export_artifact: Option<ProjectArtifact>,
    pub(super) generation: Option<Generation>,
    pub(super) work: Option<Receiver<Result<Prepared, EngineError>>>,
    pub(super) work_cancel: Option<Arc<AtomicBool>>,
    pub(super) verifying: Option<Receiver<Verified>>,
    pub(super) short_destination: Option<app_destination::Destination>,
    pub(super) seen_track_revision: u64,
    pub(super) input_changed: bool,
    input: Option<(u64, Option<PathBuf>, Option<PathBuf>)>,
    waveform_worker: Option<WaveformWorker>,
    track_pending_duration: bool,
    watcher: Option<Receiver<(u64, Result<Snapshot, EngineError>)>>,
    watched: Option<Result<Snapshot, EngineError>>,
    watched_at: Option<Instant>,
}

impl State {
    pub fn effective_caption_style(&self, short: bool) -> CaptionStyle {
        CaptionStyle {
            safe_area: if short {
                self.short_safe_area
            } else {
                self.caption_style.safe_area
            },
            ..self.caption_style
        }
    }

    pub fn set_short_safe_area(&mut self, area: noh::safe_area::SafeArea) {
        if self.short_safe_area != area {
            self.short_safe_area = area;
            self.short_revision = self.short_revision.wrapping_add(1);
            self.failure = None;
            self.outcome = None;
        }
    }
    /// IDs are supplied by the existing metadata controller. No stamps are read
    /// on the render thread. Visual-order/range/output-name edits leave WAV data.
    pub fn sync_inputs(
        &mut self,
        id: u64,
        wav: Option<&PathBuf>,
        ffmpeg: Option<&PathBuf>,
        seconds: Option<f64>,
        ctx: &eframe::egui::Context,
    ) {
        let input = (id, wav.cloned(), ffmpeg.cloned());
        if self.input.as_ref() != Some(&input) {
            self.range_moved = false;
            self.input = Some(input);
            self.watched = None;
            self.watched_at = None;
            self.track.invalidate();
            self.track_pending_duration = self.track.path.is_some();
            self.waveform = None;
            self.waveform_error = None;
            self.waveform_loading = wav.is_some();
            self.duration_ms = 0;
            self.viewport = None;
            if let Some(path) = wav {
                let worker = self.waveform_worker.get_or_insert_with(|| {
                    let ctx = ctx.clone();
                    WaveformWorker::new(move || ctx.request_repaint())
                });
                worker.request(path.clone(), ffmpeg.cloned().unwrap_or_default());
            } else {
                if let Some(worker) = &self.waveform_worker {
                    worker.cancel();
                }
                self.range = None;
                self.viewport = None;
            }
            self.short_revision = self.short_revision.wrapping_add(1);
        }
        if let Some(seconds) = seconds.filter(|s| s.is_finite() && *s > 0.0) {
            let ms = (seconds * 1_000.0).round() as u64;
            if ms != self.duration_ms {
                self.duration_ms = ms;
                let range = self.range.and_then(|r| r.clamped(ms));
                self.range_moved |= self.range.is_some() && range != self.range;
                self.range = range;
                self.viewport = self
                    .viewport
                    .and_then(|v| v.0.clamped(ms).map(Viewport))
                    .or_else(|| Viewport::full(ms));
            }
            if self.track_pending_duration
                && let (Some(path), Some(wav)) = (self.track.path.clone(), wav)
            {
                self.track.request(path, wav.clone(), ms);
                self.track_pending_duration = false;
            }
        }
        if let Some(reply) = self
            .waveform_worker
            .as_ref()
            .and_then(WaveformWorker::try_recv)
        {
            self.waveform_loading = false;
            match reply.result {
                Ok(waveform) => self.waveform = Some(waveform),
                Err(error) => self.waveform_error = Some(error.to_string()),
            }
        }
        self.track.poll(ctx);
    }

    pub fn attach(&mut self, path: PathBuf, wav: Option<&PathBuf>) {
        if let Some(wav) = wav.filter(|_| self.duration_ms > 0) {
            self.track.request(path, wav.clone(), self.duration_ms);
        } else {
            self.track.invalidate();
            self.track.path = Some(path);
            self.track_pending_duration = true;
        }
    }

    pub fn set_range(&mut self, range: Option<Range>) -> bool {
        let range = range.and_then(|r| r.clamped(self.duration_ms));
        if self.range == range {
            return false;
        }
        self.range = range;
        self.range_moved = false;
        self.failure = None;
        self.outcome = None;
        if self.range.is_none() {
            self.short_preview_artifact = None;
            self.short_preview = None;
        }
        self.short_revision = self.short_revision.wrapping_add(1);
        true
    }

    pub fn stamp(&self, revision: u64, short: bool) -> Stamp {
        Stamp {
            revision,
            short_revision: short.then_some(self.short_revision),
            captions: self
                .apply_subtitles
                .then_some((self.track.revision, self.effective_caption_style(short))),
        }
    }
    pub fn busy(&self) -> bool {
        self.running.is_some() || self.generating || self.work.is_some()
    }
    pub fn short_output(&self, main_name: &str, folder: &Path) -> PathBuf {
        let name = if self.short_name.is_empty() {
            format!(
                "{}-short.mp4",
                Path::new(main_name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        } else {
            self.short_name.clone()
        };
        folder.join(name)
    }
    pub fn short_name_issue(&self) -> Option<app_destination::Issue> {
        if self.short_name.is_empty() {
            None
        } else {
            app_destination::syntax(&self.short_name)
        }
    }
    pub fn short_preview_current(&self, revision: u64) -> bool {
        self.short_preview_artifact
            .as_ref()
            .is_some_and(|a| a.stamp == self.stamp(revision, true))
    }
    pub fn short_export_current(&self, revision: u64) -> bool {
        self.short_export_artifact
            .as_ref()
            .is_some_and(|a| a.stamp == self.stamp(revision, true))
    }
    pub fn retry_waveform(&mut self, ctx: &eframe::egui::Context) {
        if let Some((_, Some(wav), ffmpeg)) = &self.input {
            let worker = self.waveform_worker.get_or_insert_with(|| {
                let ctx = ctx.clone();
                WaveformWorker::new(move || ctx.request_repaint())
            });
            worker.request(wav.clone(), ffmpeg.clone().unwrap_or_default());
            self.waveform_loading = true;
            self.waveform_error = None;
        }
    }
    /// Check only the WAV/decoder identities here; ordinary media metadata is
    /// owned by the existing metadata controller. No file reads occur in paint.
    pub(super) fn poll_input_identity(&mut self, ctx: &eframe::egui::Context) {
        if let Some(work) = &self.watcher {
            match work.try_recv() {
                Ok((id, result)) => {
                    self.watcher = None;
                    if self.input.as_ref().is_some_and(|input| input.0 == id) {
                        if self.watched.as_ref().is_some_and(|old| old != &result) {
                            self.input_changed = true;
                            self.waveform = None;
                            self.track.invalidate();
                            self.track_pending_duration = self.track.path.is_some();
                        }
                        self.watched = Some(result);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.watcher = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.watcher.is_none()
            && self
                .watched_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
            && let Some((id, Some(wav), ffmpeg)) = self.input.clone()
        {
            let (tx, rx) = mpsc::sync_channel(1);
            self.watcher = Some(rx);
            self.watched_at = Some(Instant::now());
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = (|| {
                    let decoder = noh::inspection::resolve_ffmpeg(&ffmpeg.unwrap_or_default())?;
                    Ok(Snapshot(vec![
                        FileStamp::read(&wav)?,
                        FileStamp::read(&decoder)?,
                    ]))
                })();
                let _ = tx.send((id, result));
                ctx.request_repaint();
            });
        }
        if self.input.as_ref().is_some_and(|input| input.1.is_some()) {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_preset_changes_short_only_and_guide_visibility_never_stales_export() {
        let mut state = State {
            apply_subtitles: true,
            ..Default::default()
        };
        let full = state.stamp(7, false);
        let short = state.stamp(7, true);
        state.set_short_safe_area(noh::safe_area::SafeArea::YoutubeShorts);
        assert_eq!(full, state.stamp(7, false));
        assert_ne!(short, state.stamp(7, true));
        assert_eq!(
            state.effective_caption_style(true).safe_area,
            noh::safe_area::SafeArea::YoutubeShorts
        );
        assert_eq!(
            state.effective_caption_style(false).safe_area,
            noh::safe_area::SafeArea::None
        );
        let short = state.stamp(7, true);
        state.hide_safe_area = true;
        state.set_short_safe_area(noh::safe_area::SafeArea::YoutubeShorts);
        assert_eq!(short, state.stamp(7, true));
    }

    #[test]
    fn replacement_song_clamps_range_and_invalidates_lyrics_once() {
        let ctx = eframe::egui::Context::default();
        let wav = PathBuf::from("replacement.wav");
        let mut state = State {
            // Simulate the new identity already observed, before metadata arrives.
            input: Some((2, Some(wav.clone()), None)),
            range: Range::new(13_000, 18_000, 20_000),
            ..Default::default()
        };
        state.sync_inputs(2, Some(&wav), None, Some(10.0), &ctx);
        assert_eq!(state.range, Range::new(5_000, 10_000, 10_000));
        assert!(state.range_moved);
        state.sync_inputs(2, Some(&wav), None, Some(10.0), &ctx);
        assert!(state.range_moved);
        state.set_range(Range::new(0, 5_000, 10_000));
        assert!(!state.range_moved);
        state.sync_inputs(2, Some(&wav), None, Some(10.0), &ctx);
        assert!(!state.range_moved, "polling must not repeat the notice");

        state.track.path = Some("reviewed.srt".into());
        let revision = state.track.revision;
        // A new song identity invalidates the attachment before its duration arrives.
        state.sync_inputs(3, Some(&wav), None, None, &ctx);
        assert!(state.track.loaded.is_none());
        assert!(state.track_pending_duration);
        assert!(state.track.revision > revision);
        assert!(!state.range_moved);
        let revision = state.track.revision;
        state.sync_inputs(3, Some(&wav), None, None, &ctx);
        assert_eq!(state.track.revision, revision);
    }

    #[test]
    fn selection_edits_leave_track_and_full_timeline_intact() {
        let mut state = State {
            duration_ms: 20_000,
            viewport: Viewport::full(20_000),
            ..Default::default()
        };
        state.track.path = Some("reviewed.srt".into());
        let track_revision = state.track.revision;
        assert!(state.set_range(Range::new(13_000, 18_000, 20_000)));
        assert!(!state.set_range(Range::new(13_000, 18_000, 20_000)));
        assert_eq!(state.short_revision, 1);
        assert_eq!(state.track.revision, track_revision);
        assert_eq!(state.viewport, Viewport::full(20_000));
        assert!(state.set_range(None));
        assert!(state.track.path.is_some());
    }
}
