//! Embedded preview presentation. Decode, source verification and audio stay off the UI thread.
use super::*;
use app_style::{self as style, space};
use egui::{Align, Layout};
use noh::{
    playback::{Phase, Player},
    preview::{FrameRequest, FrameWorker},
    project::{ProjectRequest, ProjectShort},
    shorts::ShortCaptions,
};
use std::collections::HashMap;
/// Marks a monitor-size still in the decoded-frame cache (clip ids are small).
const STILL_KEY: u64 = 1 << 63;

/// How the transport is laid out (Monitor README): one stage-wide row when it
/// fits, else two rows; three rows in a compact window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportMode {
    One,
    Two,
    Compact,
}
/// Where the transport's groups landed (for layout tests).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct TransportLayout {
    pub mode: TransportMode,
    pub view: Option<egui::Rect>,
    /// The navigation group, Play at its centre.
    pub group: egui::Rect,
    pub play: egui::Rect,
    pub time: egui::Rect,
    pub right: egui::Rect,
}
/// Transport widths in the current language, known before anything is drawn.
struct TransportMetrics {
    view: f32,
    time: f32,
    skip: f32,
    picture: f32,
    badge: f32,
    gap: f32,
    group: f32,
    right: f32,
}
/// Navigation buttons are 28 px, 2 apart, around the 36 px Play.
const NAV_BUTTON: f32 = 28.0;
const NAV_GAP: f32 = 2.0;
const NAV_BADGE_H: f32 = 18.0;
pub struct State {
    pub reveal: bool,
    pub cursor_ms: u64,
    pub short: bool,
    stamp: Option<app_project::Stamp>,
    worker: Option<FrameWorker>,
    thumbnails: Option<FrameWorker>,
    textures: HashMap<u64, egui::TextureHandle>,
    order: VecDeque<u64>,
    thumbnail_pending: HashMap<u64, u64>,
    thumbnail_failures: VecDeque<u64>,
    still: Option<egui::TextureHandle>,
    pub source: Option<egui::TextureHandle>,
    requested: Option<(u64, u64)>,
    still_pending: bool,
    changed_at: Instant,
    hover_candidate: Option<(u64, Instant)>,
    error: Option<String>,
    player: Option<Player>,
    player_stamp: Option<app_project::Stamp>,
    player_texture: Option<egui::TextureHandle>,
    player_frame: Option<(u64, u64)>,
    volume: f32,
    capture_started: Option<Instant>,
    capture_first_frame_ms: Option<f64>,
    pub scrubber: Option<noh::scrub::Scrubber>,
    scrub_requested: Option<u64>,
    scrub_active: bool,
    scrub_failed: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            reveal: false,
            cursor_ms: 0,
            short: false,
            stamp: None,
            worker: None,
            thumbnails: None,
            textures: HashMap::new(),
            order: VecDeque::new(),
            thumbnail_pending: HashMap::new(),
            thumbnail_failures: VecDeque::new(),
            still: None,
            source: None,
            requested: None,
            still_pending: false,
            changed_at: Instant::now(),
            hover_candidate: None,
            error: None,
            player: None,
            player_stamp: None,
            player_texture: None,
            player_frame: None,
            volume: 0.7,
            capture_started: None,
            capture_first_frame_ms: None,
            scrubber: None,
            scrub_requested: None,
            scrub_active: false,
            scrub_failed: false,
        }
    }
}
impl State {
    fn receive_still(&mut self, reply: noh::preview::FrameReply, ctx: &egui::Context) {
        if !self
            .requested
            .is_some_and(|(id, time)| id == reply.revision && time == self.cursor_ms)
        {
            return;
        }
        self.still_pending = false;
        match reply.result {
            Ok(frame) => {
                self.still = Some(ctx.load_texture(
                    "mini-still",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                ));
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    /// The monitor shows a decoded frame (a cursor still or live playback).
    pub fn has_frame(&self) -> bool {
        self.still.is_some() || self.player_texture.is_some() || self.scrub_active
    }
    pub fn has_still(&self, id: u64) -> bool {
        self.textures.contains_key(&(id | STILL_KEY))
    }
    pub fn invalidate(&mut self) {
        if let Some(scrubber) = &self.scrubber {
            scrubber.invalidate();
        }
        self.scrub_requested = None;
        self.scrub_active = false;
        self.scrub_failed = false;
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
        if let Some(player) = &self.player {
            player.stop();
        }
        self.player_stamp = None;
        self.player_texture = None;
        self.player_frame = None;
        self.still = None;
        self.source = None;
        self.requested = None;
        self.still_pending = false;
        self.error = None;
        self.changed_at = Instant::now();
        self.hover_candidate = None;
    }
    pub fn thumbnail(
        &mut self,
        id: u64,
        item: &noh::input::MediaItem,
        ffmpeg: &Path,
        ctx: &egui::Context,
        visible: bool,
    ) -> Option<egui::TextureHandle> {
        let request = FrameRequest::Thumbnail {
            item: item.clone(),
            ffmpeg: ffmpeg.into(),
        };
        self.decoded(id, request, ctx, visible)
    }
    /// The first frame of `item` at monitor size (1280 × 720), for the
    /// monitor while there is no montage to decode. Until it arrives, callers
    /// show the tile's thumbnail.
    pub fn first_still(
        &mut self,
        id: u64,
        item: &noh::input::MediaItem,
        ffmpeg: &Path,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let request = FrameRequest::Still {
            item: item.clone(),
            ffmpeg: ffmpeg.into(),
        };
        self.decoded(id | STILL_KEY, request, ctx, true)
    }
    /// Cached decoded frames: thumbnails under their clip id, stills under
    /// the id with `STILL_KEY` set.
    fn decoded(
        &mut self,
        key: u64,
        request: FrameRequest,
        ctx: &egui::Context,
        visible: bool,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.textures.get(&key) {
            self.order.retain(|k| *k != key);
            self.order.push_back(key);
            return Some(texture.clone());
        }
        if visible
            && !self.thumbnail_pending.values().any(|k| *k == key)
            && !self.thumbnail_failures.contains(&key)
            && self.thumbnail_pending.len() < 16
        {
            let worker = self.thumbnails.get_or_insert_with(|| {
                let ctx = ctx.clone();
                FrameWorker::new(move || ctx.request_repaint())
            });
            if let Some(revision) = worker.enqueue(request) {
                self.thumbnail_pending.insert(revision, key);
            }
        }
        None
    }
    fn receive_thumbnails(&mut self, ctx: &egui::Context) {
        while let Some(reply) = self.thumbnails.as_ref().and_then(FrameWorker::try_recv) {
            let Some(id) = self.thumbnail_pending.remove(&reply.revision) else {
                continue;
            };
            match reply.result {
                Ok(frame) => {
                    let texture = ctx.load_texture(
                        format!("source-{id}"),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(id, texture);
                    self.order.retain(|key| *key != id);
                    self.order.push_back(id);
                    while self.order.len() > 32 {
                        if let Some(old) = self.order.pop_front() {
                            self.textures.remove(&old);
                        }
                    }
                }
                Err(_) => {
                    self.thumbnail_failures.push_back(id);
                    if self.thumbnail_failures.len() > 128 {
                        self.thumbnail_failures.pop_front();
                    }
                }
            }
        }
    }
}

impl NohApp {
    fn mini_project_request(&self) -> Option<ProjectRequest> {
        let mut montage = self.diagnostic_request()?;
        montage.preview = true;
        let short = if self.mini_preview.short {
            let range = self.project.range?;
            Some(ProjectShort {
                start_ms: range.start_ms,
                end_ms: range.end_ms,
                restart_loops: self.project.restart_loops,
                framing: self.project.framing,
            })
        } else {
            None
        };
        let captions = if self.project.apply_subtitles {
            if self.project.track.loaded.is_none() || self.project.track.validating() {
                return None;
            }
            Some(ShortCaptions {
                subtitles: self.project.track.path.clone()?,
                style: self
                    .project
                    .effective_caption_style(self.mini_preview.short),
            })
        } else {
            None
        };
        Some(ProjectRequest {
            montage,
            short,
            captions,
        })
    }
    /// No still or scrub decode while project work, an export (any kind,
    /// preparation included) or a cancellation runs.
    fn preview_blocked(&self) -> bool {
        self.project.busy()
            || self.job.is_some()
            || self.cancelling
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
    }
    pub(super) fn mini_bounds(&self) -> (u64, u64) {
        if self.mini_preview.short
            && let Some(range) = self.project.range
        {
            (range.start_ms, range.end_ms)
        } else {
            (0, self.project.duration_ms)
        }
    }
    pub(super) fn mini_seek(&mut self, ms: u64, seek_player: bool) {
        let (start, end) = self.mini_bounds();
        let ms = ms.clamp(start, end.saturating_sub(1).max(start));
        let live = self.mini_preview.player_stamp.is_some();
        if live && !seek_player {
            if self
                .mini_preview
                .player
                .as_ref()
                .is_some_and(|p| p.snapshot().playing)
            {
                return;
            }
            if let Some(player) = &self.mini_preview.player {
                player.stop();
            }
            self.mini_preview.player_stamp = None;
            self.mini_preview.player_texture = None;
        }
        self.mini_preview.source = None;
        if ms != self.mini_preview.cursor_ms {
            // Cancel immediately, rather than accepting an obsolete image while
            // the next position is still waiting for its debounce deadline.
            if self.mini_preview.still_pending {
                if let Some(worker) = &self.mini_preview.worker {
                    worker.cancel();
                }
                self.mini_preview.still_pending = false;
                self.mini_preview.requested = None;
            }
            self.mini_preview.cursor_ms = ms;
            self.mini_preview.error = None;
            self.mini_preview.changed_at = Instant::now();
            if self.mini_preview.player_stamp.is_some()
                && let Some(player) = &self.mini_preview.player
            {
                player.seek((ms - start) as f64 / 1000.0);
            }
        }
    }
    pub(super) fn mini_hover(&mut self, ms: Option<u64>, ctx: &egui::Context) {
        let Some(ms) = ms else {
            self.mini_preview.hover_candidate = None;
            return;
        };
        if ms != self.mini_preview.cursor_ms
            && self
                .mini_preview
                .scrubber
                .as_ref()
                .is_some_and(|s| s.snapshot().fallback)
        {
            self.mini_preview.scrub_failed = false;
        }
        if self.mini_preview.scrubber.is_some() && !self.mini_preview.scrub_failed {
            if self
                .mini_preview
                .player
                .as_ref()
                .is_some_and(|p| self.mini_preview.player_stamp.is_some() && p.snapshot().playing)
            {
                return;
            }
            // Silent scrubbing uses its own persistent decoder. Pause/play audio
            // is only repositioned when the user resumes playback.
            self.mini_seek(ms, false);
            ctx.request_repaint();
            return;
        }
        if self.mini_preview.player_stamp.is_none() {
            self.mini_seek(ms, false);
            return;
        }
        if self
            .mini_preview
            .player
            .as_ref()
            .is_some_and(|p| p.snapshot().playing)
        {
            self.mini_preview.hover_candidate = None;
            return;
        }
        let (start, end) = self.mini_bounds();
        let ms = ms.clamp(start, end.saturating_sub(1).max(start));
        if ms == self.mini_preview.cursor_ms {
            self.mini_preview.hover_candidate = None;
            return;
        }
        let candidate = self
            .mini_preview
            .hover_candidate
            .get_or_insert((ms, Instant::now()));
        if candidate.0 != ms {
            *candidate = (ms, Instant::now());
        }
        let remaining = Duration::from_millis(120).saturating_sub(candidate.1.elapsed());
        if remaining.is_zero() {
            // Keep the paused player and its audio/session alive. Only a settled
            // hover seeks it; range drags and pointer jitter do not reopen it.
            self.mini_seek(ms, true);
        } else {
            ctx.request_repaint_after(remaining);
        }
    }
    pub(super) fn poll_mini_preview(&mut self, ctx: &egui::Context) {
        if self.capture_state.is_some() {
            return;
        }
        self.mini_preview.receive_thumbnails(ctx);
        if self.project.range.is_none() {
            self.mini_preview.short = false;
        }
        let stamp = self.project.stamp(self.revision, self.mini_preview.short);
        if self.mini_preview.stamp.as_ref() != Some(&stamp) {
            if self
                .mini_preview
                .stamp
                .as_ref()
                .is_some_and(|old| old.revision != stamp.revision)
            {
                // Metadata request IDs change when a source or FFmpeg changes.
                // Fade/range/order edits keep source textures instead of decoding them again.
                let retained: std::collections::HashSet<_> =
                    self.clips.iter().map(|clip| clip.request_id).collect();
                self.mini_preview
                    .textures
                    .retain(|id, _| retained.contains(&(id & !STILL_KEY)));
                self.mini_preview
                    .order
                    .retain(|id| retained.contains(&(id & !STILL_KEY)));
                self.mini_preview
                    .thumbnail_failures
                    .retain(|id| retained.contains(&(id & !STILL_KEY)));
                if self
                    .mini_preview
                    .thumbnail_pending
                    .values()
                    .any(|id| !retained.contains(&(id & !STILL_KEY)))
                {
                    self.mini_preview.thumbnail_pending.clear();
                    if let Some(worker) = &self.mini_preview.thumbnails {
                        worker.cancel();
                    }
                }
            }
            self.mini_preview.invalidate();
            self.mini_preview.stamp = Some(stamp);
            self.mini_seek(self.mini_preview.cursor_ms, false);
        }
        if let Some(player) = &self.mini_preview.player {
            let state = player.snapshot();
            if self.mini_preview.player_stamp.is_some() {
                let (start, end) = self.mini_bounds();
                self.mini_preview.cursor_ms =
                    (start + (state.position * 1000.0).round() as u64).min(end.saturating_sub(1));
                if let Some(frame) = &state.frame {
                    if self.mini_preview.capture_first_frame_ms.is_none() {
                        self.mini_preview.capture_first_frame_ms = self
                            .mini_preview
                            .capture_started
                            .map(|at| at.elapsed().as_secs_f64() * 1000.0);
                    }
                    let key = (state.generation, frame.id);
                    if self.mini_preview.player_frame != Some(key) {
                        let image =
                            egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
                        if let Some(texture) = &mut self.mini_preview.player_texture {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            self.mini_preview.player_texture = Some(ctx.load_texture(
                                "mini-player",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.mini_preview.player_frame = Some(key);
                    }
                }
                if state.phase == Phase::Error {
                    self.mini_preview.error = state.error;
                }
                if state.playing || state.phase == Phase::Loading {
                    ctx.request_repaint_after(Duration::from_millis(33));
                }
                return;
            }
        }
        if self.mini_preview.scrubber.is_some()
            && !self.mini_preview.scrub_failed
            && self.mini_preview.source.is_none()
            && !self.preview_blocked()
        {
            let ms = self.mini_preview.cursor_ms;
            if self.mini_preview.scrub_requested != Some(ms)
                && let Some(project) = self.mini_project_request()
            {
                self.mini_preview
                    .scrubber
                    .as_ref()
                    .unwrap()
                    .request(project, ms);
                self.mini_preview.scrub_requested = Some(ms);
            }
            let scrubber = self.mini_preview.scrubber.as_ref().unwrap();
            let snapshot = scrubber.snapshot();
            if snapshot.epoch == scrubber.epoch() {
                self.mini_preview.scrub_failed =
                    snapshot.error.is_some() && (!snapshot.fallback || snapshot.time_ms == ms);
                self.mini_preview.scrub_active = snapshot.frame > 0 && snapshot.error.is_none();
            }
            if self.mini_preview.scrub_active {
                if self.mini_preview.still_pending
                    && let Some(worker) = &self.mini_preview.worker
                {
                    worker.cancel();
                }
                self.mini_preview.still_pending = false;
                self.mini_preview.requested = None;
                return;
            }
        }
        if let Some(reply) = self
            .mini_preview
            .worker
            .as_ref()
            .and_then(FrameWorker::try_recv)
        {
            self.mini_preview.receive_still(reply, ctx);
        }
        let ms = self.mini_preview.cursor_ms;
        if self
            .mini_preview
            .requested
            .is_none_or(|(_, time)| time != ms)
            && self.mini_preview.source.is_none()
            && !self.preview_blocked()
        {
            if self.mini_preview.changed_at.elapsed() < Duration::from_millis(120) {
                ctx.request_repaint_after(
                    Duration::from_millis(120)
                        .saturating_sub(self.mini_preview.changed_at.elapsed()),
                );
            } else if let Some(project) = self.mini_project_request() {
                let worker = self.mini_preview.worker.get_or_insert_with(|| {
                    let ctx = ctx.clone();
                    FrameWorker::new(move || ctx.request_repaint())
                });
                let id = worker.request(FrameRequest::Project {
                    project,
                    time_ms: ms,
                });
                self.mini_preview.requested = Some((id, ms));
                self.mini_preview.still_pending = true;
                self.mini_preview.error = None;
            }
        }
    }
    pub(super) fn open_embedded_preview(&mut self, path: &Path, short: bool, ctx: &egui::Context) {
        if self.mini_preview.short != short {
            self.mini_preview.invalidate();
        }
        self.mini_preview.short = short;
        let stamp = self.project.stamp(self.revision, short);
        self.mini_preview.stamp = Some(stamp.clone());
        let (start, end) = self.mini_bounds();
        let cursor = self
            .mini_preview
            .cursor_ms
            .clamp(start, end.saturating_sub(1).max(start));
        let player = self.mini_preview.player.get_or_insert_with(|| {
            let ctx = ctx.clone();
            Player::new(move || ctx.request_repaint_after(Duration::from_millis(33)))
        });
        player.set_volume(self.mini_preview.volume);
        player.open(
            path.into(),
            self.ffmpeg.clone().unwrap_or_default(),
            (cursor - start) as f64 / 1000.0,
            true,
        );
        self.mini_preview.player_stamp = Some(stamp);
        self.mini_preview.player_frame = None;
        self.mini_preview.player_texture = None;
        self.mini_preview.source = None;
        self.mini_preview.error = None;
    }
    pub(super) fn mini_play(&mut self, ctx: &egui::Context) {
        self.mini_preview.scrub_active = false;
        if self.mini_preview.player_stamp.is_some()
            && let Some(player) = &self.mini_preview.player
        {
            let state = player.snapshot();
            if state.phase != Phase::Error {
                if state.phase == Phase::Ended {
                    player.seek(0.0);
                }
                player.set_playing(!state.playing);
                return;
            }
        }
        self.mini_preview.source = None;
        let Some(project) = self.mini_project_request() else {
            return;
        };
        let (start, end) = self.mini_bounds();
        let cursor = self
            .mini_preview
            .cursor_ms
            .clamp(start, end.saturating_sub(1).max(start));
        let player = self.mini_preview.player.get_or_insert_with(|| {
            let ctx = ctx.clone();
            Player::new(move || ctx.request_repaint_after(Duration::from_millis(33)))
        });
        player.set_volume(self.mini_preview.volume);
        player.open_project(project, (cursor - start) as f64 / 1000.0, true);
        self.mini_preview.player_stamp =
            Some(self.project.stamp(self.revision, self.mini_preview.short));
        self.mini_preview.player_texture = None;
        self.mini_preview.player_frame = None;
        self.mini_preview.error = None;
    }
    /// Navigation moves the playhead, playing or paused; a frame step pauses
    /// first. The target becomes the cursor at once, so rapid clicks chain.
    fn navigate(&mut self, ms: u64, pause: bool) {
        if pause
            && self.mini_preview.player_stamp.is_some()
            && let Some(player) = &self.mini_preview.player
            && player.snapshot().playing
        {
            player.set_playing(false);
        }
        self.mini_seek(ms, true);
    }
    pub(super) fn play_live_scope(&mut self, ctx: &egui::Context, short: bool) {
        if self.mini_preview.short != short {
            self.mini_preview.invalidate();
            self.mini_preview.short = short;
        }
        self.mini_preview.stamp = Some(self.project.stamp(self.revision, short));
        self.mini_preview.reveal = true;
        if self.mini_preview.player_stamp.is_none()
            || !self
                .mini_preview
                .player
                .as_ref()
                .is_some_and(|p| p.snapshot().playing)
        {
            self.mini_play(ctx);
        }
    }
    /// Width / height of the full-video frame: the export canvas when known.
    fn canvas_aspect(&self) -> f32 {
        let from_plan = self
            .diagnosis
            .as_ref()
            .filter(|_| self.diagnosis_current())
            .and_then(|d| d.plan.as_ref())
            .map(|p| (p.target.width, p.target.height));
        let from_source = self
            .clips
            .iter()
            .filter_map(|c| c.info.as_ref())
            .find(|i| i.width > 0 && i.height > 0)
            .map(|i| (i.width, i.height));
        from_plan
            .or(from_source)
            .filter(|(w, h)| *w > 0 && *h > 0)
            .map_or(16.0 / 9.0, |(w, h)| w as f32 / h as f32)
            .clamp(0.25, 4.0)
    }
    /// Stage: the aspect-aware monitor and the transport under it.
    pub(super) fn stage_ui(&mut self, ui: &mut egui::Ui, locked: bool) -> Option<TransportLayout> {
        let l = self.locale.language;
        let ctx = ui.ctx().clone();
        let t = style::tokens(ui);
        let compact = ui.available_width() < style::COMPACT_BELOW;
        if self.project.range.is_none() && self.mini_preview.short {
            self.mini_preview.short = false;
            self.mini_preview.invalidate();
        }
        let width = ui.available_width();
        let has_transport = self.wav.is_some() && self.project.duration_ms > 0;
        let mode = if has_transport {
            self.transport_mode(ui, width)
        } else {
            TransportMode::One
        };
        // 320 at 980×850: the fixed zones above and below take 530 px, 40
        // more when the transport needs two rows. Width alone picks the mode.
        let fixed = if mode == TransportMode::Two {
            570.0
        } else {
            530.0
        };
        let mut height = if compact {
            200.0
        } else {
            (ctx.viewport_rect().height() - fixed).clamp(180.0, 360.0)
        };
        let aspect = self.canvas_aspect();
        height = height.min(width / aspect.max(16.0 / 9.0));
        let frame_width = if self.mini_preview.short {
            height * 9.0 / 16.0
        } else {
            height * aspect
        };
        let transport_width = (height * 16.0 / 9.0).max(frame_width).min(width);
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let monitor = egui::Rect::from_center_size(row.center(), egui::vec2(frame_width, height));
        if self.mini_preview.reveal {
            ui.scroll_to_rect(monitor, Some(egui::Align::Min));
            self.mini_preview.reveal = false;
        }
        ui.painter()
            .rect_filled(monitor, style::radius::L, t.monitor);
        // Without a song, or while a file is unreadable or still being read,
        // there is no montage to decode: show the first readable visual.
        let readable = self.clips.iter().find(|c| c.error.is_none());
        let first_visual = match (!self.ready(), readable) {
            (true, Some(clip)) => {
                let (id, item) = (clip.request_id, clip.item.clone());
                let ffmpeg = self.ffmpeg.clone().unwrap_or_default();
                // Sharp at monitor size; the tile's thumbnail until it arrives.
                self.mini_preview
                    .first_still(id, &item, &ffmpeg, &ctx)
                    .or_else(|| self.mini_preview.thumbnail(id, &item, &ffmpeg, &ctx, true))
            }
            _ => None,
        };
        let texture = self
            .mini_preview
            .source
            .as_ref()
            .or(self.mini_preview.player_texture.as_ref())
            .or(self.mini_preview.still.as_ref())
            .or(first_visual.as_ref());
        let snapshot = self
            .mini_preview
            .player
            .as_ref()
            .filter(|_| self.mini_preview.player_stamp.is_some())
            .map(Player::snapshot);
        let playing = snapshot.as_ref().is_some_and(|s| s.playing);
        let loading = self.project.verifying.is_some()
            || snapshot.as_ref().is_some_and(|s| s.phase == Phase::Loading);
        let still_loading = snapshot.is_none()
            && self.mini_preview.source.is_none()
            && (self.mini_preview.still_pending
                || self
                    .mini_preview
                    .requested
                    .is_some_and(|(_, ms)| ms != self.mini_preview.cursor_ms));
        if let Some(texture) = texture {
            paint_texture(ui, monitor, texture);
        }
        let scrub_visible = self.mini_preview.scrub_active
            && self.mini_preview.source.is_none()
            && self.mini_preview.player_stamp.is_none();
        if let Some(scrubber) = &self.mini_preview.scrubber {
            scrubber.paint(ui, monitor, scrub_visible, style::radius::L as f32);
        }
        if self.mini_preview.short && !self.project.hide_safe_area {
            paint_safe_area(ui, monitor, self.project.short_safe_area, l);
        }
        if let Some(error) = self.mini_preview.error.clone() {
            let mut inside = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("monitor-error")
                    .max_rect(monitor.shrink(16.0))
                    .layout(Layout::top_down(Align::Center)),
            );
            inside.add_space((monitor.height() - 72.0).max(0.0) / 2.0);
            inside
                .label(
                    egui::RichText::new(l.text("preview.unavailable")).color(egui::Color32::WHITE),
                )
                .on_hover_text(error);
            if style::button(&mut inside, l.text("preview.retry"), true, false, 0.0).clicked() {
                self.mini_preview.invalidate();
            }
        } else if texture.is_none() && !scrub_visible && (loading || still_loading || self.ready())
        {
            let spinner = egui::Rect::from_center_size(monitor.center(), egui::vec2(24.0, 24.0));
            // This overlay must not advance the layout cursor inside the monitor.
            ui.place(
                spinner,
                egui::Spinner::new().size(24.0).color(egui::Color32::WHITE),
            );
        }
        if self.mini_preview.source.is_some() {
            // A source picked in the list, not the composed montage.
            let galley = ui.painter().layout_no_wrap(
                l.text("preview.source").into(),
                egui::FontId::proportional(style::text::SMALL),
                egui::Color32::WHITE,
            );
            let chip = egui::Rect::from_min_size(
                monitor.left_bottom() + egui::vec2(8.0, -28.0),
                galley.size() + egui::vec2(12.0, 6.0),
            );
            ui.painter()
                .rect_filled(chip, style::radius::S, egui::Color32::from_black_alpha(160));
            ui.painter().galley(
                chip.min + egui::vec2(6.0, 3.0),
                galley,
                egui::Color32::WHITE,
            );
        }
        if !has_transport {
            return None;
        }
        // Transport 8 below the monitor: undo the body's zone spacing (20, or
        // 12 compact) that followed the monitor's allocation.
        ui.add_space(space::S - ui.spacing().item_spacing.y);
        Some(self.transport_ui(
            ui,
            &ctx,
            mode,
            transport_width,
            playing,
            loading || still_loading,
            locked,
            snapshot,
        ))
    }

    /// Widths of the transport's zones in the current language. They do not
    /// depend on the playhead, the view or the monitor, so the layout choice
    /// never oscillates.
    fn transport_metrics(&self, ui: &egui::Ui, compact: bool) -> TransportMetrics {
        let l = self.locale.language;
        let measure = |text: &str, font: egui::FontId| {
            ui.painter()
                .layout_no_wrap(text.into(), font, egui::Color32::WHITE)
                .size()
                .x
        };
        let view = if self.project.range.is_some() {
            style::segmented_width(ui, &[l.text("monitor.video"), l.text("project.short")])
        } else {
            0.0
        };
        let duration = self.project.duration_ms;
        let widest = app_timeline::clock(duration.max(599_900), duration, l);
        let time = measure(
            &format!("{widest} / {widest}"),
            egui::FontId::proportional(style::text::BODY),
        );
        let small = egui::FontId::proportional(style::text::SMALL);
        let label =
            measure(l.text("nav.minus_5"), small.clone()).max(measure(l.text("nav.plus_5"), small));
        let badge = self
            .project_visuals()
            .iter()
            .map(|v| measure(&v.label, style::semibold(style::text::SMALL)))
            .fold(0.0, f32::max)
            + 10.0;
        let badge = badge.max(20.0);
        let picture = 2.0 + 16.0 + 2.0 + badge + 2.0;
        let group = |gap: f32, skip: f32| {
            2.0 * picture + 4.0 * NAV_BUTTON + 2.0 * skip + style::CONTROL_H_LG + 8.0 * gap
        };
        let (mut gap, mut skip) = (NAV_GAP, label + 2.0 * if compact { space::XS } else { 6.0 });
        // The narrowest compact windows (CJK "−5 秒") tighten the group.
        if compact && group(gap, skip) > ui.available_width() {
            (gap, skip) = (0.0, label + 2.0);
        }
        let right = self.fades_button_width(ui) + space::S + style::CONTROL_H;
        TransportMetrics {
            view,
            time,
            skip,
            picture,
            badge,
            gap,
            group: group(gap, skip),
            right,
        }
    }
    /// One stage-wide row when `2 × max(left, right) + group + 2 × 12` fits
    /// the stage; two rows otherwise; three when compact.
    fn transport_mode(&self, ui: &egui::Ui, width: f32) -> TransportMode {
        if width < style::COMPACT_BELOW {
            return TransportMode::Compact;
        }
        let m = self.transport_metrics(ui, false);
        let left = m.view + if m.view > 0.0 { space::M } else { 0.0 } + m.time;
        if 2.0 * left.max(m.right) + m.group + 2.0 * space::M <= width {
            TransportMode::One
        } else {
            TransportMode::Two
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transport_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        mode: TransportMode,
        width: f32,
        playing: bool,
        loading: bool,
        locked: bool,
        snapshot: Option<noh::playback::Snapshot>,
    ) -> TransportLayout {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let m = self.transport_metrics(ui, mode == TransportMode::Compact);
        let full = ui.available_width();
        let h = style::CONTROL_H_LG;
        let gap = space::XS;
        let total_height = match mode {
            TransportMode::One => h,
            TransportMode::Two => 2.0 * h + gap,
            TransportMode::Compact => 2.0 * h + style::CONTROL_H + 2.0 * gap,
        };
        let (area, _) =
            ui.allocate_exact_size(egui::vec2(full, total_height), egui::Sense::hover());
        let row = |index: usize, height: f32| {
            egui::Rect::from_min_size(
                egui::pos2(area.left(), area.top() + index as f32 * (h + gap)),
                egui::vec2(full, height),
            )
        };
        let centred = |row: egui::Rect, width: f32| {
            egui::Rect::from_center_size(row.center(), egui::vec2(width, row.height()))
        };
        let with_view = |left: f32| left + if m.view > 0.0 { m.view + space::M } else { 0.0 };
        // Zones: [view] [time] | group | [Fades + volume].
        let (view_at, time_rect, group, right_rect) = match mode {
            TransportMode::One => {
                let row = row(0, h);
                (
                    row.left_top(),
                    egui::Rect::from_min_size(
                        egui::pos2(with_view(row.left()), row.top()),
                        egui::vec2(m.time, h),
                    ),
                    centred(row, m.group),
                    egui::Rect::from_min_max(egui::pos2(row.right() - m.right, row.top()), row.max),
                )
            }
            TransportMode::Two => {
                // Row 1 keeps its ends under the 16:9 box; row 2 centres the
                // group and the time together.
                let first = centred(row(0, h), width.max(m.view + space::M + m.right).min(full));
                let second = centred(row(1, h), m.group + space::M + m.time);
                (
                    first.left_top(),
                    egui::Rect::from_min_size(
                        egui::pos2(second.left() + m.group + space::M, second.top()),
                        egui::vec2(m.time, h),
                    ),
                    egui::Rect::from_min_size(second.min, egui::vec2(m.group, h)),
                    egui::Rect::from_min_max(
                        egui::pos2(first.right() - m.right, first.top()),
                        first.max,
                    ),
                )
            }
            TransportMode::Compact => {
                let first = row(0, h);
                let third = row(2, style::CONTROL_H);
                (
                    first.left_top(),
                    egui::Rect::from_min_size(
                        egui::pos2(with_view(first.left()), first.top()),
                        egui::vec2(m.time, h),
                    ),
                    centred(row(1, h), m.group),
                    egui::Rect::from_min_max(
                        egui::pos2(third.right() - m.right, third.top()),
                        third.max,
                    ),
                )
            }
        };
        let mut layout = TransportLayout {
            mode,
            view: None,
            group,
            play: egui::Rect::NOTHING,
            time: time_rect,
            right: egui::Rect::NOTHING,
        };
        // Video | Short, only while a range exists.
        if self.project.range.is_some() {
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("transport-view")
                    .max_rect(egui::Rect::from_min_size(view_at, egui::vec2(m.view, h)))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            let before = self.mini_preview.short;
            let chosen = style::segmented(
                &mut left,
                "preview-view",
                &[l.text("monitor.video"), l.text("project.short")],
                usize::from(before),
            );
            layout.view = Some(left.min_rect());
            if let Some(index) = chosen {
                self.mini_preview.short = index == 1;
            }
            if before != self.mini_preview.short {
                self.mini_preview.invalidate();
                self.mini_preview.stamp =
                    Some(self.project.stamp(self.revision, self.mini_preview.short));
                self.mini_seek(self.mini_preview.cursor_ms, false);
            }
        }
        layout.play = self.navigation_ui(
            ui,
            ctx,
            group,
            &m,
            playing,
            loading,
            locked,
            snapshot.is_some(),
        );
        // The time, fixed width so digits never shift; floored while paused so
        // it never reads later than the frame on screen.
        let (start, end) = self.mini_bounds();
        let duration = self.project.duration_ms;
        let offset = self.mini_preview.cursor_ms.saturating_sub(start);
        let current = if playing {
            app_timeline::clock(offset, duration, l)
        } else {
            app_timeline::clock_floor(offset, duration, l)
        };
        let total_text = app_timeline::clock(end.saturating_sub(start), duration, l);
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::proportional(style::text::BODY);
        job.append(
            &current,
            0.0,
            egui::TextFormat::simple(font.clone(), t.text),
        );
        job.append(
            &format!(" / {total_text}"),
            0.0,
            egui::TextFormat::simple(font, t.text_2),
        );
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let text_right = time_rect.left() + galley.size().x;
        ui.painter().galley(
            egui::pos2(
                time_rect.left(),
                time_rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            t.text,
        );
        let position = l.format("monitor.position", &[current.clone(), total_text.clone()]);
        ui.interact(
            time_rect,
            ui.id().with("transport-time"),
            egui::Sense::hover(),
        )
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &position));
        let spinner = egui::Rect::from_min_size(
            egui::pos2(text_right + space::S, time_rect.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        );
        // After the time, where it touches nothing (the monitor also spins).
        let limit = match mode {
            TransportMode::One => group.left() - space::XS,
            TransportMode::Two => area.right(),
            TransportMode::Compact => area.right(),
        };
        if loading && spinner.right() <= limit {
            // Keep the next row fixed while the preview starts or seeks.
            ui.place(spinner, egui::Spinner::new().size(16.0));
        }
        // Fades (values always readable), then the volume.
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("transport-right")
                .max_rect(right_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        right.spacing_mut().item_spacing.x = space::S;
        right.add_enabled_ui(!locked, |ui| self.fades_button(ui));
        self.volume_button(&mut right, snapshot.as_ref());
        layout.right = right.min_rect();
        layout
    }

    /// The navigation group, left to right: previous picture, start, −5 s,
    /// back one frame, Play, forward one frame, +5 s, end, next picture.
    /// Returns Play's rect.
    #[allow(clippy::too_many_arguments)]
    fn navigation_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        group: egui::Rect,
        m: &TransportMetrics,
        playing: bool,
        loading: bool,
        locked: bool,
        live: bool,
    ) -> egui::Rect {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let targets = self.navigation_targets();
        let rate = self.navigation_rate().rounded().to_string();
        let short = self.mini_preview.short;
        let mut x = group.left();
        let mut next_rect = |width: f32, height: f32| {
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, group.center().y - height / 2.0),
                egui::vec2(width, height),
            );
            x += width + m.gap;
            rect
        };
        let locked_text = l.text("bar.locked");
        let mut go = None;
        let picture = |ui: &mut egui::Ui,
                       rect: egui::Rect,
                       id: &str,
                       target: &Option<app_navigation::Picture>,
                       previous: bool| {
            let name = match target {
                Some(p) => l.format("nav.picture", &[p.label.clone()]),
                None => l.text("nav.no_picture").into(),
            };
            let clicked = nav_button(
                ui,
                rect,
                id,
                &name,
                locked.then_some(locked_text),
                target.is_some(),
                locked,
                |painter, rect, color| {
                    let chevron_x = if previous {
                        rect.left() + 2.0
                    } else {
                        rect.right() - 18.0
                    };
                    app_icons::paint(
                        painter,
                        egui::Rect::from_min_size(
                            egui::pos2(chevron_x, rect.center().y - 8.0),
                            egui::vec2(16.0, 16.0),
                        ),
                        if previous {
                            app_icons::Icon::ChevronLeft
                        } else {
                            app_icons::Icon::ChevronRight
                        },
                        color,
                    );
                    let badge_x = if previous {
                        chevron_x + 18.0
                    } else {
                        rect.left() + 2.0
                    };
                    let badge = egui::Rect::from_min_size(
                        egui::pos2(badge_x, rect.center().y - NAV_BADGE_H / 2.0),
                        egui::vec2(m.badge, NAV_BADGE_H),
                    );
                    let (text, fill, ink) = match target {
                        Some(p) => (p.label.as_str(), t.seq[p.source_index % 3], t.seq_ink),
                        None => ("–", t.surface_2, t.text_3),
                    };
                    painter.rect_filled(badge, style::radius::S, fill);
                    painter.text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        style::semibold(style::text::SMALL),
                        ink,
                    );
                },
            );
            clicked.then(|| target.as_ref().map(|p| p.ms)).flatten()
        };
        let icon = |ui: &mut egui::Ui,
                    rect: egui::Rect,
                    id: &str,
                    name: &str,
                    target: Option<u64>,
                    icon: app_icons::Icon| {
            nav_button(
                ui,
                rect,
                id,
                name,
                locked.then_some(locked_text),
                target.is_some(),
                locked,
                |painter, rect, color| {
                    app_icons::paint(
                        painter,
                        egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0)),
                        icon,
                        color,
                    );
                },
            )
            .then_some(target)
            .flatten()
        };
        let skip = |ui: &mut egui::Ui,
                    rect: egui::Rect,
                    id: &str,
                    label: &str,
                    name: &str,
                    target: Option<u64>| {
            nav_button(
                ui,
                rect,
                id,
                name,
                locked.then_some(locked_text),
                target.is_some(),
                locked,
                |painter, rect, color| {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(style::text::SMALL),
                        color,
                    );
                },
            )
            .then_some(target)
            .flatten()
        };
        let (start_name, end_name) = if short {
            ("nav.short_start", "nav.short_end")
        } else {
            ("nav.to_start", "nav.to_end")
        };
        let rect = next_rect(m.picture, NAV_BUTTON);
        go =
            go.or(picture(ui, rect, "nav-previous", &targets.previous, true).map(|ms| (ms, false)));
        let rect = next_rect(NAV_BUTTON, NAV_BUTTON);
        go = go.or(icon(
            ui,
            rect,
            "nav-start",
            l.text(start_name),
            targets.start,
            app_icons::Icon::ToStart,
        )
        .map(|ms| (ms, false)));
        let rect = next_rect(m.skip, NAV_BUTTON);
        go = go.or(skip(
            ui,
            rect,
            "nav-back-5",
            l.text("nav.minus_5"),
            l.text("nav.back_5"),
            targets.back_5,
        )
        .map(|ms| (ms, false)));
        let rect = next_rect(NAV_BUTTON, NAV_BUTTON);
        let back_frame = l.format("nav.frame_back", &[rate.clone()]);
        go = go.or(icon(
            ui,
            rect,
            "nav-back-frame",
            &back_frame,
            targets.back_frame,
            app_icons::Icon::FrameBack,
        )
        .map(|ms| (ms, true)));
        let play_rect = next_rect(style::CONTROL_H_LG, style::CONTROL_H_LG);
        // Play, round, in the centre of the group.
        let can_play = !locked && self.ready() && (live || !loading);
        let name = l.text(if playing {
            "preview.pause"
        } else {
            "preview.play"
        });
        let play_reason = if locked {
            l.text("bar.locked")
        } else if loading {
            l.text("preview.loading")
        } else {
            l.text("preview.unavailable")
        };
        let play = ui
            .interact(
                play_rect,
                ui.id().with("transport-play"),
                if can_play {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            )
            .on_hover_text(if can_play { name } else { play_reason });
        play.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, can_play, name));
        let mut play_painter = ui.painter().clone();
        if locked {
            play_painter.multiply_opacity(0.45);
        }
        play_painter.circle_filled(
            play_rect.center(),
            play_rect.height() / 2.0,
            if play.hovered() && can_play {
                t.surface_3
            } else {
                t.surface_2
            },
        );
        app_icons::paint(
            &play_painter,
            egui::Rect::from_center_size(play_rect.center(), egui::vec2(18.0, 18.0)),
            if playing {
                app_icons::Icon::Pause
            } else {
                app_icons::Icon::Play
            },
            if can_play { t.text } else { t.text_3 },
        );
        style::focus(ui, &play);
        let rect = next_rect(NAV_BUTTON, NAV_BUTTON);
        let forward_frame = l.format("nav.frame_forward", &[rate]);
        go = go.or(icon(
            ui,
            rect,
            "nav-forward-frame",
            &forward_frame,
            targets.forward_frame,
            app_icons::Icon::FrameForward,
        )
        .map(|ms| (ms, true)));
        let rect = next_rect(m.skip, NAV_BUTTON);
        go = go.or(skip(
            ui,
            rect,
            "nav-forward-5",
            l.text("nav.plus_5"),
            l.text("nav.forward_5"),
            targets.forward_5,
        )
        .map(|ms| (ms, false)));
        let rect = next_rect(NAV_BUTTON, NAV_BUTTON);
        go = go.or(icon(
            ui,
            rect,
            "nav-end",
            l.text(end_name),
            targets.end,
            app_icons::Icon::ToEnd,
        )
        .map(|ms| (ms, false)));
        let rect = next_rect(m.picture, NAV_BUTTON);
        go = go.or(picture(ui, rect, "nav-next", &targets.next, false).map(|ms| (ms, false)));
        if play.clicked() {
            self.mini_play(ctx);
        } else if let Some((ms, pause)) = go {
            self.navigate(ms, pause);
        }
        play_rect
    }

    fn fade_text(&self, enabled: bool, seconds: f64) -> String {
        let l = self.locale.language;
        if !enabled {
            return "–".into();
        }
        let precision = if (seconds - seconds.round()).abs() < 0.05 {
            0
        } else {
            1
        };
        format!(
            "{}{}",
            l.decimal(seconds, precision),
            l.text("seconds_suffix")
        )
    }
    pub(super) fn fades_label(&self) -> String {
        let l = self.locale.language;
        if !self.in_enabled && !self.out_enabled {
            return l.text("fades.none").into();
        }
        l.format(
            "fades.label",
            &[
                self.fade_text(self.in_enabled, self.fade_in),
                self.fade_text(self.out_enabled, self.fade_out),
            ],
        )
    }
    fn fades_button_width(&self, ui: &egui::Ui) -> f32 {
        let text = ui
            .painter()
            .layout_no_wrap(
                self.fades_label(),
                egui::FontId::proportional(style::text::BODY),
                style::tokens(ui).text,
            )
            .size()
            .x;
        24.0 + text
            + if self.clip_audio {
                space::S + 16.0
            } else {
                0.0
            }
    }
    /// Fades button: the values stay on the main screen; the popup
    /// edits both fades and the clip-sound mix, with the rules of before.
    fn fades_button(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let width = self.fades_button_width(ui);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, style::CONTROL_H), egui::Sense::click());
        let enabled = ui.is_enabled();
        let color = if enabled { t.text } else { t.text_3 };
        let name = l.format(
            "fades.name",
            &[
                self.fade_text(self.in_enabled, self.fade_in),
                self.fade_text(self.out_enabled, self.fade_out),
                l.text(if self.clip_audio {
                    "fades.mixed"
                } else {
                    "fades.not_mixed"
                })
                .into(),
            ],
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
        ui.painter().rect_filled(
            rect,
            style::radius::M,
            if response.hovered() && enabled {
                t.surface_3
            } else {
                t.surface_2
            },
        );
        if !self.fades_valid() {
            ui.painter().rect_stroke(
                rect,
                style::radius::M,
                egui::Stroke::new(2.0, t.error),
                egui::StrokeKind::Inside,
            );
        }
        let galley = ui.painter().layout_no_wrap(
            self.fades_label(),
            egui::FontId::proportional(style::text::BODY),
            color,
        );
        let text_pos = egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0);
        let text_right = text_pos.x + galley.size().x;
        ui.painter().galley(text_pos, galley, color);
        if self.clip_audio {
            app_icons::paint(
                ui.painter(),
                egui::Rect::from_min_size(
                    egui::pos2(text_right + space::S, rect.center().y - 8.0),
                    egui::vec2(16.0, 16.0),
                ),
                app_icons::Icon::ClipSound,
                t.accent_text,
            );
        }
        style::focus(ui, &response);
        let response = if enabled {
            response.on_hover_text(&name)
        } else {
            response.on_disabled_hover_text(l.text("bar.locked"))
        };
        let mut changed = false;
        egui::Popup::from_toggle_button_response(&response)
            .id(egui::Id::new("fades-popup"))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(300.0)
            .show(|ui| {
                ui.spacing_mut().item_spacing.y = space::S;
                ui.label(style::strong(l.text("ui.visual_fades"), style::text::BODY));
                let invalid = !self.fades_valid();
                for (id, key, enabled, value) in [
                    (
                        "fade-start",
                        "ui.at_start",
                        &mut self.in_enabled,
                        &mut self.fade_in,
                    ),
                    (
                        "fade-end",
                        "ui.at_end",
                        &mut self.out_enabled,
                        &mut self.fade_out,
                    ),
                ] {
                    ui.push_id(id, |ui| {
                        ui.horizontal(|ui| {
                            let (cell, _) = ui.allocate_exact_size(
                                egui::vec2(160.0, style::CONTROL_H),
                                egui::Sense::hover(),
                            );
                            let mut cell_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(cell)
                                    .layout(Layout::left_to_right(Align::Center)),
                            );
                            changed |=
                                style::checkbox(&mut cell_ui, enabled, l.text(key)).changed();
                            let field = ui
                                .add_enabled_ui(*enabled, |ui| {
                                    app_ui::seconds_control(ui, value, style::FADE_VALUE_W, l)
                                })
                                .inner;
                            style::focus(ui, &field);
                            if invalid {
                                ui.painter().rect_stroke(
                                    field.rect,
                                    style::radius::M,
                                    egui::Stroke::new(2.0, t.error),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            changed |= field.changed();
                        });
                    });
                }
                if invalid {
                    style::message(ui, style::Severity::Error, l.text("error.fades"), false);
                } else {
                    ui.label(
                        egui::RichText::new(l.text("ui.picture_only"))
                            .size(style::text::SMALL)
                            .color(t.text_2),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    changed |=
                        style::switch(ui, &mut self.clip_audio, l.text("ui.mix_audio")).changed();
                    ui.label(l.text("ui.mix_audio"));
                });
                ui.label(
                    egui::RichText::new(l.text(if self.clip_audio {
                        "mixed_audio"
                    } else {
                        "ui.audio_off"
                    }))
                    .size(style::text::SMALL)
                    .color(t.text_2),
                );
            });
        if changed {
            self.invalidated();
        }
    }
    fn volume_button(&mut self, ui: &mut egui::Ui, snapshot: Option<&noh::playback::Snapshot>) {
        let l = self.locale.language;
        let silent = snapshot.and_then(|s| s.warning.clone());
        let icon = if silent.is_some() {
            app_icons::Icon::Warning
        } else {
            app_icons::Icon::Volume
        };
        let response = style::icon_button(ui, icon, l.text("preview.volume"), style::CONTROL_H);
        let response = match &silent {
            Some(warning) => {
                response.on_hover_text(format!("{}\n{warning}", l.text("preview.silent")))
            }
            None => response,
        };
        egui::Popup::from_toggle_button_response(&response)
            .id(egui::Id::new("volume-popup"))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.spacing_mut().slider_width = 120.0;
                let slider = ui.add(
                    egui::Slider::new(&mut self.mini_preview.volume, 0.0..=1.0).show_value(false),
                );
                if slider.changed()
                    && let Some(player) = &self.mini_preview.player
                {
                    player.set_volume(self.mini_preview.volume);
                }
            });
    }
    pub(super) fn mini_capture_start(&mut self, short: bool, ctx: &egui::Context) {
        self.mini_preview.short = short;
        self.mini_preview.stamp = Some(self.project.stamp(self.revision, short));
        let start = self.mini_bounds().0;
        let offset = std::env::var("NOH_CAPTURE_PLAYBACK_START_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(500);
        self.mini_seek(start.saturating_add(offset), true);
        self.mini_preview.capture_started = Some(Instant::now());
        self.mini_preview.capture_first_frame_ms = None;
        self.mini_play(ctx);
    }
    pub(super) fn mini_capture_terminal(&self) -> Option<&'static str> {
        let state = self.mini_preview.player.as_ref()?.snapshot();
        if state.phase == Phase::Error {
            return Some("failed");
        }
        let until = std::env::var("NOH_CAPTURE_PLAYBACK_UNTIL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1000) as f64
            / 1000.0;
        (state.frame.as_ref().is_some_and(|f| f.position >= until) && state.position >= until)
            .then_some("finished")
    }
    pub(super) fn mini_capture_report(&self) -> serde_json::Value {
        let state = self.mini_preview.player.as_ref().map(Player::snapshot);
        serde_json::json!({"explicit_ffmpeg":self.ffmpeg.is_some(), "first_frame_ms":self.mini_preview.capture_first_frame_ms, "thumbnails":self.mini_preview.textures.len(), "still":self.mini_preview.still.is_some(), "cursor_ms":self.mini_preview.cursor_ms, "short":self.mini_preview.short, "error":self.mini_preview.error,
            "player":state.map(|s| serde_json::json!({"phase":format!("{:?}", s.phase), "playing":s.playing, "position":s.position, "audio_available":s.audio_available, "warning":s.warning, "error":s.error, "frame":s.frame.as_ref().map(|f|serde_json::json!({"width":f.width,"height":f.height,"position":f.position,"id":f.id}))})), "rendered_preview":self.preview_ready || self.project.short_preview_artifact.is_some()})
    }
}

/// A 28 px navigation button: transparent, `surface-2` on hover or focus,
/// `text` like Play (`text-2` is too close to `text-3` to tell them apart).
/// One that cannot move is `text-3` without fill; during an export the group
/// is locked at 45 % and `locked` replaces its tooltip. Returns a click.
#[allow(clippy::too_many_arguments)]
fn nav_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    name: &str,
    locked: Option<&str>,
    enabled: bool,
    is_locked: bool,
    paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> bool {
    let t = style::tokens(ui);
    let active = enabled && !is_locked;
    let response = ui.interact(
        rect,
        ui.id().with(id),
        if active {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, active, name));
    let mut painter = ui.painter().clone();
    if is_locked {
        painter.multiply_opacity(0.45);
    }
    if active && (response.hovered() || response.has_focus()) {
        painter.rect_filled(rect, style::radius::M, t.surface_2);
    }
    paint(&painter, rect, if active { t.text } else { t.text_3 });
    style::focus(ui, &response);
    let response = response.on_hover_text(locked.unwrap_or(name));
    active && response.clicked()
}

fn paint_safe_area(ui: &egui::Ui, rect: egui::Rect, area: noh::safe_area::SafeArea, l: Language) {
    let Some([left, top, right, bottom]) = area.reference_insets() else {
        return;
    };
    let safe = egui::Rect::from_min_max(
        rect.min
            + egui::vec2(
                rect.width() * left as f32 / 1080.0,
                rect.height() * top as f32 / 1920.0,
            ),
        rect.max
            - egui::vec2(
                rect.width() * right as f32 / 1080.0,
                rect.height() * bottom as f32 / 1920.0,
            ),
    );
    let painter = ui.painter().with_clip_rect(rect);
    let shade = egui::Color32::from_black_alpha(100);
    for masked in [
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), safe.top())),
        egui::Rect::from_min_max(egui::pos2(rect.left(), safe.bottom()), rect.max),
        egui::Rect::from_min_max(egui::pos2(rect.left(), safe.top()), safe.left_bottom()),
        egui::Rect::from_min_max(safe.right_top(), egui::pos2(rect.right(), safe.bottom())),
    ] {
        painter.rect_filled(masked, 0.0, shade);
    }
    painter.rect_stroke(
        safe,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(111, 221, 191)),
        egui::StrokeKind::Inside,
    );
    ui.interact(rect, ui.id().with("safe-area-guide"), egui::Sense::hover())
        .on_hover_text(l.text("preset.guide_help"));
}

pub(super) fn paint_texture(ui: &egui::Ui, rect: egui::Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let target = egui::Rect::from_center_size(rect.center(), size * scale);
    // The picture keeps the monitor's rounded corners (radius L).
    egui::Image::new(egui::load::SizedTexture::new(texture.id(), size))
        .corner_radius(style::radius::L)
        .paint_at(ui, target);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_loading_keeps_layout_stable(has_still: bool) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        for language in Language::ALL {
            for width in [318.0, 932.0] {
                for scale in [1.0, 2.0] {
                    ctx.set_pixels_per_point(scale);
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.wav = Some("song.wav".into());
                    app.project.duration_ms = 225_900;
                    app.project.range = noh::timeline::Range::new(69_800, 112_700, 225_900);
                    if has_still {
                        app.mini_preview.still = Some(ctx.load_texture(
                            "previous-preview",
                            egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    let mut settled = None;
                    // Warm up once, then alternate decoding and settled frames.
                    for (frame, loading) in
                        [false, false, true, false, true].into_iter().enumerate()
                    {
                        app.mini_preview.still_pending = loading;
                        let mut output = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 850.0),
                                )),
                                ..Default::default()
                            },
                            |ui| {
                                app_style::apply(&ctx);
                                ui.spacing_mut().item_spacing.y = if width < style::COMPACT_BELOW {
                                    12.0
                                } else {
                                    20.0
                                };
                                let transport = app.stage_ui(ui, false).expect("transport");
                                app.project_timeline(ui, false);
                                let geometry = (
                                    transport.view,
                                    transport.group,
                                    transport.time,
                                    transport.right,
                                    app.project_ui.timeline.pictures_rect(),
                                    app.project_ui.timeline.lanes_bottom(),
                                    ui.cursor().top(),
                                );
                                if frame == 1 {
                                    settled = Some(geometry);
                                } else if frame > 1 {
                                    assert_eq!(
                                        Some(geometry), settled,
                                        "{} width={width} scale={scale} still={has_still} loading={loading}",
                                        language.code(),
                                    );
                                }
                            },
                        );
                        output.textures_delta.clear();
                    }
                }
            }
        }
    }

    #[test]
    fn hover_loading_indicator_keeps_transport_and_timeline_in_place() {
        assert_loading_keeps_layout_stable(true);
    }

    #[test]
    fn empty_monitor_loading_indicator_keeps_transport_and_timeline_in_place() {
        assert_loading_keeps_layout_stable(false);
    }

    #[test]
    fn obsolete_still_cannot_replace_texture_or_surface_an_old_error() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.cursor_ms = 5000;
        state.requested = Some((4, 1000));
        let reply = |revision, result| noh::preview::FrameReply {
            revision,
            request: FrameRequest::Thumbnail {
                item: "unused.png".into(),
                ffmpeg: PathBuf::new(),
            },
            result,
        };
        let frame = || {
            Ok(std::sync::Arc::new(noh::preview::Frame {
                width: 2,
                height: 2,
                rgba: vec![255; 16],
            }))
        };
        state.receive_still(reply(4, frame()), &ctx);
        state.receive_still(
            reply(4, Err(noh::preview::PreviewError::SourceChanged)),
            &ctx,
        );
        assert!(state.still.is_none() && state.error.is_none());
        state.requested = Some((5, 5000));
        state.receive_still(reply(4, frame()), &ctx);
        assert!(state.still.is_none());
        state.receive_still(reply(5, frame()), &ctx);
        assert!(state.still.is_some());
    }

    #[test]
    fn returning_to_a_cancelled_still_position_can_request_it_again() {
        let mut app = NohApp::default();
        app.project.duration_ms = 20_000;
        app.mini_preview.cursor_ms = 1000;
        app.mini_preview.requested = Some((7, 1000));
        app.mini_preview.still_pending = true;
        app.mini_seek(5000, false);
        app.mini_seek(1000, false);
        assert_eq!(app.mini_preview.cursor_ms, 1000);
        assert!(app.mini_preview.requested.is_none());
        assert!(!app.mini_preview.still_pending);
    }

    #[test]
    fn paused_hover_debounces_seek_without_stopping_the_player() {
        let ctx = egui::Context::default();
        let mut app = NohApp::default();
        app.project.duration_ms = 20_000;
        app.mini_preview.player = Some(Player::new(|| {}));
        app.mini_preview.player_stamp = Some(app.project.stamp(0, false));
        let before = app
            .mini_preview
            .player
            .as_ref()
            .unwrap()
            .snapshot()
            .generation;
        app.mini_hover(Some(5000), &ctx);
        assert_eq!(app.mini_preview.cursor_ms, 0);
        app.mini_hover(Some(0), &ctx);
        assert!(app.mini_preview.hover_candidate.is_none());
        app.mini_preview.hover_candidate =
            Some((5000, Instant::now() - Duration::from_millis(150)));
        app.mini_hover(Some(5000), &ctx);
        assert_eq!(app.mini_preview.cursor_ms, 5000);
        assert!(app.mini_preview.player_stamp.is_some());
        assert_eq!(
            app.mini_preview
                .player
                .as_ref()
                .unwrap()
                .snapshot()
                .generation,
            before
        );
        app.mini_hover(None, &ctx);
        assert!(app.mini_preview.hover_candidate.is_none());
    }
    #[test]
    fn settings_edits_retain_thumbnails_but_removed_sources_are_released() {
        let ctx = egui::Context::default();
        let mut app = NohApp::default();
        app.clips.push(Clip {
            id: 1,
            request_id: 7,
            item: PathBuf::from("source.mp4").into(),
            info: None,
            error: None,
        });
        let texture = ctx.load_texture(
            "source",
            egui::ColorImage::filled([2, 2], egui::Color32::RED),
            egui::TextureOptions::LINEAR,
        );
        app.mini_preview.textures.insert(7, texture.clone());
        app.mini_preview
            .textures
            .insert(7 | STILL_KEY, texture.clone());
        app.mini_preview
            .textures
            .insert(9 | STILL_KEY, texture.clone());
        app.mini_preview.textures.insert(9, texture);
        app.mini_preview
            .order
            .extend([7, 7 | STILL_KEY, 9, 9 | STILL_KEY]);
        app.mini_preview.stamp = Some(app.project.stamp(0, false));
        app.invalidated();
        app.poll_mini_preview(&ctx);
        assert_eq!(app.mini_preview.textures.len(), 2);
        assert!(app.mini_preview.textures.contains_key(&7));
        assert!(app.mini_preview.has_still(7));
        assert_eq!(app.mini_preview.order.len(), 2);
        assert_eq!(app.mini_preview.order.front(), Some(&7));
    }
    /// Transport zones (view toggle, time, navigation group, Fades + volume)
    /// fit and never overlap, in every language and theme: one stage-wide row
    /// at the 980 layout with Play under the image centre, two rows on a
    /// narrower wide window, three rows when compact.
    #[test]
    fn stage_transport_fits_without_overlap_in_every_language() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        for language in Language::ALL {
            let mut tops = HashMap::new();
            for width in [318.0, 350.0, 388.0, 600.0, 700.0, 932.0] {
                for dark in [false, true] {
                    ctx.set_visuals(if dark {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.wav = Some("song.wav".into());
                    app.project.duration_ms = 182_400;
                    app.project.range = noh::timeline::Range::new(13_000, 18_000, 182_400);
                    // Widest labels: decimal fades and the clip-sound mark.
                    app.fade_in = 1.5;
                    app.fade_out = 2.5;
                    app.clip_audio = true;
                    app.mini_preview.error = Some("Decoder diagnostic".into());
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 850.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            app_style::apply(&ctx);
                            let measured = app.transport_metrics(ui, false).view;
                            let layout = app.stage_ui(ui, false).expect("transport");
                            let case = format!("{} {width} dark={dark}", language.code());
                            let view = layout.view.expect("Video | Short with a range");
                            assert!(
                                view.width() <= measured + 0.5,
                                "{case}: view {view:?} measured {measured}"
                            );
                            let zones = [view, layout.time, layout.group, layout.right];
                            for rect in zones {
                                assert!(
                                    rect.left() >= -0.5 && rect.right() <= width + 0.5,
                                    "{case}: {rect:?}"
                                );
                            }
                            for (i, a) in zones.iter().enumerate() {
                                for b in &zones[i + 1..] {
                                    assert!(
                                        !a.shrink(0.5).intersects(b.shrink(0.5)),
                                        "{case}: {a:?} {b:?}"
                                    );
                                }
                            }
                            assert!(ui.min_rect().right() <= width + 0.5, "{case}");
                            assert!(layout.group.contains_rect(layout.play), "{case}");
                            // Play sits at the centre of the group.
                            assert!(
                                (layout.play.center().x - layout.group.center().x).abs() < 0.5,
                                "{case}"
                            );
                            let same_row = |a: egui::Rect, b: egui::Rect| {
                                (a.center().y - b.center().y).abs() < 1.0
                            };
                            let expected = if width < style::COMPACT_BELOW {
                                TransportMode::Compact
                            } else if width < 932.0 {
                                TransportMode::Two
                            } else {
                                TransportMode::One
                            };
                            assert_eq!(layout.mode, expected, "{case}");
                            match layout.mode {
                                TransportMode::One => {
                                    // Play exactly under the centre of the image.
                                    assert!(
                                        (layout.play.center().x - width / 2.0).abs() < 0.5,
                                        "{case}"
                                    );
                                    for rect in [view, layout.time, layout.right] {
                                        assert!(same_row(rect, layout.group), "{case}");
                                    }
                                    assert!(view.right() <= layout.time.left(), "{case}");
                                    assert!(
                                        layout.time.right() + space::M <= layout.group.left() + 0.5,
                                        "{case}"
                                    );
                                    assert!(
                                        layout.group.right() + space::M
                                            <= layout.right.left() + 0.5,
                                        "{case}"
                                    );
                                }
                                TransportMode::Two => {
                                    assert!(same_row(view, layout.right), "{case}");
                                    assert!(same_row(layout.group, layout.time), "{case}");
                                    assert!(layout.group.top() >= view.bottom(), "{case}");
                                    // The group and the time are centred together.
                                    let pair = layout.group.union(layout.time);
                                    assert!((pair.center().x - width / 2.0).abs() < 0.5, "{case}");
                                }
                                TransportMode::Compact => {
                                    assert!(same_row(view, layout.time), "{case}");
                                    assert!(layout.group.top() >= view.bottom(), "{case}");
                                    assert!(layout.right.top() >= layout.group.bottom(), "{case}");
                                    assert!(
                                        (layout.group.center().x - width / 2.0).abs() < 0.5,
                                        "{case}"
                                    );
                                }
                            }
                            tops.insert(width as u32, view.top());
                        },
                    );
                    output.textures_delta.clear();
                }
            }
            // Width alone picks the mode; two rows cost the monitor 40 px.
            assert!(
                (tops[&700] + 40.0 - tops[&932]).abs() < 0.5,
                "{} {tops:?}",
                language.code()
            );
        }
    }
    /// The navigation group moves the playhead by exact frames and 5 s, stops
    /// at the bounds, reads floored while paused and is locked during export.
    #[test]
    fn navigation_buttons_move_the_playhead_and_stop_at_the_bounds() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.wav = Some("song.wav".into());
        app.project.duration_ms = 20_000;
        let mut time = 0.0;
        let mut frame = |app: &mut NohApp, events: Vec<egui::Event>, locked: bool| {
            time += 0.05;
            let mut centres = Vec::new();
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 850.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app_style::apply(ui.ctx());
                    ui.ctx().all_styles_mut(|style| {
                        style.interaction.tooltip_delay = 0.0;
                        style.interaction.show_tooltips_only_when_still = false;
                    });
                    let m = app.transport_metrics(ui, false);
                    let layout = app.stage_ui(ui, locked).expect("transport");
                    let mut x = layout.group.left();
                    for width in [
                        m.picture,
                        NAV_BUTTON,
                        m.skip,
                        NAV_BUTTON,
                        style::CONTROL_H_LG,
                        NAV_BUTTON,
                        m.skip,
                        NAV_BUTTON,
                        m.picture,
                    ] {
                        centres.push(egui::pos2(x + width / 2.0, layout.group.center().y));
                        x += width + m.gap;
                    }
                    assert!((centres[4].x - layout.play.center().x).abs() < 0.5);
                    assert!((x - m.gap - layout.group.right()).abs() < 0.5);
                },
            );
            rendered.textures_delta.clear();
            let texts: Vec<String> = rendered
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            (centres, texts)
        };
        let (centres, _) = frame(&mut app, vec![], false);
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut click = |app: &mut NohApp, button: usize, locked: bool| {
            let at = centres[button];
            frame(app, vec![egui::Event::PointerMoved(at)], locked);
            frame(app, vec![press(at, true)], locked);
            frame(app, vec![press(at, false)], locked);
            // egui hides a tooltip after a click until the pointer leaves.
            frame(
                app,
                vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))],
                locked,
            );
            frame(
                app,
                vec![egui::Event::PointerMoved(at + egui::vec2(1.0, 0.0))],
                locked,
            );
            frame(app, vec![], locked).1
        };
        let (start, back_5, back_frame, forward_frame, forward_5, end) = (1, 2, 3, 5, 6, 7);
        click(&mut app, start, false);
        click(&mut app, back_frame, false);
        assert_eq!(app.mini_preview.cursor_ms, 0, "disabled at the start");
        click(&mut app, forward_5, false);
        assert_eq!(app.mini_preview.cursor_ms, 5_000);
        // Rapid clicks start from the pending target, one 1/25 s frame each.
        click(&mut app, forward_frame, false);
        let texts = click(&mut app, forward_frame, false);
        assert_eq!(app.mini_preview.cursor_ms, 5_080);
        assert!(texts.iter().any(|t| t == "Forward 1/25 s"), "{texts:?}");
        click(&mut app, back_frame, false);
        assert_eq!(app.mini_preview.cursor_ms, 5_040);
        // Go to end = the start of the last frame; then forward is disabled.
        click(&mut app, end, false);
        assert_eq!(app.mini_preview.cursor_ms, 19_960);
        click(&mut app, forward_5, false);
        click(&mut app, forward_frame, false);
        assert_eq!(app.mini_preview.cursor_ms, 19_960);
        let texts = click(&mut app, back_5, false);
        assert_eq!(app.mini_preview.cursor_ms, 14_960);
        // The paused readout is floored: 14.96 s reads 0:14.9, not 0:15.0.
        assert!(texts.iter().any(|t| t == "0:14.9 / 0:20.0"), "{texts:?}");
        // Locked during an export: no move, and the tooltip says why.
        let texts = click(&mut app, start, true);
        assert_eq!(app.mini_preview.cursor_ms, 14_960);
        assert!(
            texts.iter().any(|t| t == Language::En.text("bar.locked")),
            "{texts:?}"
        );
        click(&mut app, start, false);
        assert_eq!(app.mini_preview.cursor_ms, 0);
    }
    /// The capture's `open=fades` tweak and a click share this popup id.
    #[test]
    fn fades_popup_opens_by_its_id_with_both_fades_and_the_mix() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::Fr;
        app.wav = Some("song.wav".into());
        app.project.duration_ms = 182_400;
        let texts = |app: &mut NohApp| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 850.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app_style::apply(ui.ctx());
                    app.stage_ui(ui, false);
                },
            );
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let closed = texts(&mut app);
        assert!(
            !closed
                .iter()
                .any(|t| t == Language::Fr.text("ui.visual_fades"))
        );
        egui::Popup::open_id(&ctx, egui::Id::new("fades-popup"));
        // egui sizes a new popup invisibly on its first frame.
        texts(&mut app);
        let open = texts(&mut app);
        for key in [
            "ui.visual_fades",
            "ui.at_start",
            "ui.at_end",
            "ui.mix_audio",
        ] {
            let text = Language::Fr.text(key);
            assert!(open.iter().any(|t| t == text), "{key} missing: {open:?}");
        }
    }

    #[test]
    fn fades_label_shows_values_without_opening_anything() {
        let mut app = NohApp::default();
        app.locale.language = Language::Fr;
        assert_eq!(app.fades_label(), "Fondus 1 s · 2 s");
        app.fade_out = 2.5;
        assert_eq!(app.fades_label(), "Fondus 1 s · 2,5 s");
        app.in_enabled = false;
        assert_eq!(app.fades_label(), "Fondus – · 2,5 s");
        app.out_enabled = false;
        assert_eq!(app.fades_label(), "Sans fondu");
        app.locale.language = Language::Ja;
        assert_eq!(app.fades_label(), "フェードなし");
    }
    #[test]
    fn cursor_uses_project_clock_and_half_open_short_bounds() {
        let mut app = NohApp::default();
        app.project.duration_ms = 20_000;
        app.project
            .set_range(noh::timeline::Range::new(13_000, 18_000, 20_000));
        app.mini_preview.short = true;
        app.mini_seek(0, false);
        assert_eq!(app.mini_preview.cursor_ms, 13_000);
        app.mini_seek(20_000, false);
        assert_eq!(app.mini_preview.cursor_ms, 17_999);
        app.mini_preview.short = false;
        app.mini_seek(4_500, false);
        assert_eq!(app.mini_preview.cursor_ms, 4_500);
        assert_eq!(app.project.range.unwrap().start_ms, 13_000);
    }

    #[test]
    fn locked_stage_keeps_monitor_texture_opaque_and_explains_play() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.wav = Some("song.wav".into());
        app.project.duration_ms = 20_000;
        let texture = ctx.load_texture(
            "locked-monitor",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        app.mini_preview.source = Some(texture.clone());
        let mut frame = |at: Option<egui::Pos2>, time: f64| {
            let mut layout = None;
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 850.0),
                    )),
                    time: Some(time),
                    events: at.into_iter().map(egui::Event::PointerMoved).collect(),
                    ..Default::default()
                },
                |ui| {
                    app_style::apply(ui.ctx());
                    ui.ctx().all_styles_mut(|style| {
                        style.interaction.tooltip_delay = 0.0;
                        style.interaction.show_tooltips_only_when_still = false;
                    });
                    layout = app.stage_ui(ui, true);
                },
            );
            rendered.textures_delta.clear();
            (layout.unwrap(), rendered.shapes)
        };
        let (layout, shapes) = frame(None, 0.0);
        let monitor = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(rect)
                if rect
                    .brush
                    .as_ref()
                    .is_some_and(|brush| brush.fill_texture_id == texture.id()) =>
            {
                Some(rect)
            }
            _ => None,
        });
        let monitor = monitor.expect("monitor texture painted");
        assert_eq!(monitor.fill.a(), 255);
        let point = layout.play.center();
        frame(Some(point), 0.1);
        let (_, shapes) = frame(None, 1.0);
        assert!(shapes.iter().any(|s| match &s.shape {
            egui::Shape::Text(text) => text.galley.text() == Language::En.text("bar.locked"),
            _ => false,
        }));
        let fades = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("Fades") => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("Fades label painted");
        frame(Some(egui::pos2(0.0, 0.0)), 1.1);
        frame(Some(fades), 1.2);
        let (_, shapes) = frame(None, 2.0);
        assert!(shapes.iter().any(|s| match &s.shape {
            egui::Shape::Text(text) => text.galley.text() == Language::En.text("bar.locked"),
            _ => false,
        }));
    }

    #[test]
    fn preview_error_offers_retry_and_a_click_clears_the_error() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.wav = Some("song.wav".into());
        app.project.duration_ms = 20_000;
        app.mini_preview.error = Some("Decode failed".into());
        let mut frame = |events: Vec<egui::Event>| {
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(980.0, 850.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app_style::apply(ui.ctx());
                    app.stage_ui(ui, false);
                },
            );
            rendered.textures_delta.clear();
            rendered.shapes
        };
        let shapes = frame(vec![]);
        let retry = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(text) if text.galley.text() == Language::En.text("preview.retry") => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            _ => None,
        });
        let retry = retry.expect("Retry button visible over preview error");
        frame(vec![egui::Event::PointerMoved(retry)]);
        frame(vec![egui::Event::PointerButton {
            pos: retry,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        frame(vec![egui::Event::PointerButton {
            pos: retry,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert!(app.mini_preview.error.is_none());
    }
}
