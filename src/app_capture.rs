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
    /// The simulated primary button is held on the ruler.
    pressed: bool,
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
        // NOH_CAPTURE_SCRUB_TARGET=lanes hovers the pictures lane without pressing
        // (hover must not scrub); the default presses and drags the time ruler.
        let lanes = std::env::var("NOH_CAPTURE_SCRUB_TARGET").as_deref() == Ok("lanes");
        // The sweep's own clock and button: the first 12.5 s, or the handoff drag.
        let mut sweep = Some((t, t < 12.5 && !lanes));
        if std::env::var("NOH_CAPTURE_SCRUB_HANDOFF").as_deref() == Ok("1") && t >= 13.0 {
            // Play, pause, then drag the ruler while paused (the scrubber must take
            // the picture), release, play again from the cursor, pause.
            let steps = [
                (13.0, "play"),
                (15.0, "pause"),
                (16.0, "drag"),
                (18.5, "release"),
                (19.5, "play"),
                (21.5, "pause"),
                (22.5, "settled"),
            ];
            if let Some(&(at, action)) = steps.get(self.scrub_qa.handoff_step)
                && t >= at
            {
                let scrub = self.mini_preview.scrubber.as_ref().map(|s| {
                    let snap = s.snapshot();
                    serde_json::json!({"time_ms":snap.time_ms,"frame":snap.frame,"error":snap.error})
                });
                self.scrub_qa.handoff.push(serde_json::json!({"seconds":t,"before":action,"state":self.mini_capture_report(),"scrub":scrub,"scrub_active":self.mini_preview.scrub_active,"player_open":self.mini_preview.player_stamp.is_some()}));
                if matches!(action, "play" | "pause") {
                    self.mini_play(ctx);
                }
                self.scrub_qa.handoff_step += 1;
            }
            let dragging = self.scrub_qa.handoff_step >= 3 && self.scrub_qa.handoff_step < 4;
            sweep = dragging.then(|| ((t - 16.0) * 4.0 / 2.5, true));
            if sweep.is_none() {
                if self.scrub_qa.pressed {
                    self.scrub_qa.pressed = false;
                    if let Some(rect) = self.project_ui.timeline.ruler_rect() {
                        input.events.push(egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
                input.events.push(egui::Event::PointerGone);
                ctx.request_repaint_after(Duration::from_millis(16));
                return;
            }
        }
        if snapshot.frame != self.scrub_qa.last_frame && self.scrub_qa.samples.len() < 2000 {
            self.scrub_qa.last_frame = snapshot.frame;
            self.scrub_qa.samples.push(serde_json::json!({"seconds":t,"time_ms":snapshot.time_ms,"frame":snapshot.frame,"request_to_ready_ms":snapshot.latency_ms,"error":snapshot.error,"decoded_frame":snapshot.decoded_frame,"source_seconds":snapshot.source_seconds,"cursor_ms":self.mini_preview.cursor_ms}));
        }
        let rect = if lanes {
            self.project_ui.timeline.pictures_rect()
        } else {
            self.project_ui.timeline.ruler_rect()
        };
        let (Some(rect), Some((t, hold))) = (rect, sweep) else {
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
            let at = egui::pos2(x, rect.center().y);
            input.events.push(egui::Event::PointerMoved(at));
            // Press on the ruler, drag across it, release after the sweep.
            if hold != self.scrub_qa.pressed {
                input.events.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: hold,
                    modifiers: egui::Modifiers::default(),
                });
                self.scrub_qa.pressed = hold;
            }
            self.scrub_qa.last_input = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}
/// Clicks on the time ruler while the live preview plays, and what each one cost
/// (`NOH_CAPTURE_SEEK_PLAYING=1` with the `live-preview` or `live-short` action).
#[derive(Default)]
pub struct SeekQa {
    started: Option<Instant>,
    /// Since when the preview has been playing steadily.
    steady: Option<Instant>,
    next: usize,
    click: Option<SeekClick>,
    results: Vec<serde_json::Value>,
    done: bool,
}
struct SeekClick {
    target_ms: u64,
    at: egui::Pos2,
    /// The primary button went down: the origin of every time.
    pressed: Instant,
    /// The primary button went up.
    released: Option<Instant>,
    /// The pointer left the ruler.
    gone: bool,
    seek_before: Option<u64>,
    position_before: f64,
    luma_before: f64,
    last_sample: f64,
    first_frame_ms: Option<f64>,
    /// What showed that first picture: the player, or the paused preview.
    shown_by: &'static str,
    /// The first picture of the new position, as a grid of mean colours.
    picture: Vec<u8>,
    /// The clock of that picture: the player's own, or for the paused preview
    /// the time it asked of the source's copy.
    picture_seconds: Option<f64>,
    resumed_ms: Option<f64>,
    black_ms: f64,
    no_picture_ms: f64,
    spinner_ms: f64,
    loading_ms: f64,
    samples: Vec<serde_json::Value>,
}
/// Steady playback before each click, so one seek does not run into the next.
const SEEK_DWELL: Duration = Duration::from_millis(1500);
/// A click that has not resumed playback by then is recorded as it stands.
const SEEK_LIMIT: Duration = Duration::from_secs(20);
/// Below this mean brightness a played frame counts as black.
const SEEK_BLACK: f64 = 4.0;
/// The live capture must be played by libmpv, with a sound output: it waits
/// for the light copies before Play and fails otherwise
/// (`NOH_CAPTURE_REQUIRE_LIVE=1`).
pub fn require_live() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_REQUIRE_LIVE").as_deref() == Ok("1")
}
/// A capture of the live preview that sends its own pointer events, or none.
pub fn drives_pointer() -> bool {
    seek_playing_enabled() || drag_playing_enabled() || transport_enabled() || lane_tip_enabled()
}
/// The capture of the lyrics lane's tooltip rests its own pointer on the lane.
pub fn lane_tip_enabled() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_STATE").as_deref() == Ok(LANE_TIP)
}
const LANE_TIP: &str = "lyrics-lane-tip";
pub fn seek_playing_enabled() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_SEEK_PLAYING").as_deref() == Ok("1")
}
/// How long each click holds the button (`NOH_CAPTURE_SEEK_HOLD_MS`): none by
/// default, the release then comes on the next frame; a hand takes about 100 ms.
fn seek_hold() -> Duration {
    Duration::from_millis(
        std::env::var("NOH_CAPTURE_SEEK_HOLD_MS")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0),
    )
}
/// Project times to click, in order (`NOH_CAPTURE_SEEK_TARGETS_MS`, comma separated).
fn seek_targets() -> Vec<u64> {
    std::env::var("NOH_CAPTURE_SEEK_TARGETS_MS")
        .ok()
        .map(|text| {
            text.split(',')
                .map(|item| {
                    item.trim()
                        .parse()
                        .expect("NOH_CAPTURE_SEEK_TARGETS_MS: milliseconds")
                })
                .collect()
        })
        .unwrap_or_else(|| vec![5000, 2000, 9500, 15000, 35000, 39500, 1000])
}
impl NohApp {
    /// One real click on the time ruler per target: press, release on a later
    /// frame, then the pointer leaves. Nothing else differs from a user's click.
    pub(super) fn seek_capture_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if !seek_playing_enabled() || self.seek_qa.done {
            return;
        }
        // A click in flight goes on whatever became of the player: the press
        // itself may have stopped it.
        if let Some(click) = &mut self.seek_qa.click {
            ctx.request_repaint_after(Duration::from_millis(8));
            if click.released.is_none() {
                if click.pressed.elapsed() >= seek_hold() {
                    input.events.push(egui::Event::PointerButton {
                        pos: click.at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    });
                    click.released = Some(Instant::now());
                }
            } else if !click.gone {
                input.events.push(egui::Event::PointerGone);
                click.gone = true;
            }
            return;
        }
        let Some(state) = self
            .mini_preview
            .player
            .as_ref()
            .filter(|_| self.mini_preview.player_stamp.is_some())
            .map(noh::playback::Player::snapshot)
        else {
            return;
        };
        self.seek_qa.started.get_or_insert_with(Instant::now);
        // Sample finer than the 33 ms the preview asks for while it loads.
        ctx.request_repaint_after(Duration::from_millis(8));
        let steady =
            state.phase == noh::playback::Phase::Ready && state.playing && state.frame.is_some();
        if !steady {
            self.seek_qa.steady = None;
            return;
        }
        if self
            .seek_qa
            .steady
            .get_or_insert_with(Instant::now)
            .elapsed()
            < SEEK_DWELL
        {
            return;
        }
        let Some(&target_ms) = seek_targets().get(self.seek_qa.next) else {
            self.seek_qa.done = true;
            return;
        };
        let Some(rect) = self.project_ui.timeline.ruler_rect() else {
            return;
        };
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
                * ((target_ms as f64 - view.start_ms as f64) / (view.end_ms - view.start_ms) as f64)
                    as f32;
        let at = egui::pos2(x, rect.center().y);
        input.events.push(egui::Event::PointerMoved(at));
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        });
        self.seek_qa.next += 1;
        self.seek_qa.click = Some(SeekClick {
            target_ms,
            at,
            pressed: Instant::now(),
            released: None,
            gone: false,
            seek_before: state.seek.map(|s| s.seek_id),
            position_before: state.position,
            luma_before: self.mini_preview.player_luma,
            last_sample: 0.0,
            first_frame_ms: None,
            shown_by: "",
            picture: Vec::new(),
            picture_seconds: None,
            resumed_ms: None,
            black_ms: 0.0,
            no_picture_ms: 0.0,
            spinner_ms: 0.0,
            loading_ms: 0.0,
            samples: Vec::new(),
        });
    }
    /// After the frame is painted: what the monitor showed since the press.
    fn seek_capture_sample(&mut self) {
        let (start, _) = self.mini_bounds();
        let Some(click) = &mut self.seek_qa.click else {
            return;
        };
        let Some(player) = &self.mini_preview.player else {
            return;
        };
        let state = player.snapshot();
        let open = self.mini_preview.player_stamp.is_some();
        let live = player.is_live();
        let t = click.pressed.elapsed().as_secs_f64() * 1000.0;
        let span = t - click.last_sample;
        click.last_sample = t;
        let target = click.target_ms.saturating_sub(start) as f64 / 1000.0;
        let (picture, spinner) = self.mini_preview.painted;
        // The paused preview's picture is one too.
        let picture = picture || self.mini_preview.painted_scrub;
        let luma = self.mini_preview.player_luma;
        let loading = open && state.phase == noh::playback::Phase::Loading;
        // The seek this click asked for, not one still running from before.
        let seek = state
            .seek
            .filter(|s| open && Some(s.seek_id) != click.seek_before);
        // The picture of the new position is on the monitor: the player's after
        // its seek, or the paused preview's of that very time while the press
        // holds the player stopped.
        let by_player = seek.is_some_and(|s| s.frame_ms.is_some())
            && self
                .mini_preview
                .player_frame
                .zip(state.frame.as_ref())
                .is_some_and(|(shown, frame)| shown == (state.generation, frame.id));
        let by_paused = !open
            && self.mini_preview.painted_scrub
            && self
                .mini_preview
                .scrubber
                .as_ref()
                .map(|s| s.snapshot())
                .is_some_and(|s| s.error.is_none() && s.time_ms == click.target_ms);
        // Or the first picture of a player opened again at the release, when
        // the release came before the paused preview had answered.
        let by_opened = open
            && state.seek.is_none()
            && click.released.is_some()
            && self
                .mini_preview
                .player_frame
                .zip(state.frame.as_ref())
                .is_some_and(|(shown, frame)| {
                    shown == (state.generation, frame.id) && (frame.position - target).abs() < 0.5
                });
        // Or the still of that very time, where there is no libmpv.
        let by_still = !open && self.mini_preview.still_shown() == Some(click.target_ms);
        let landed = by_player || by_paused || by_opened || by_still;
        if !picture {
            click.no_picture_ms += span;
        } else if luma < SEEK_BLACK && click.luma_before >= SEEK_BLACK {
            click.black_ms += span;
        }
        if spinner {
            click.spinner_ms += span;
        }
        if loading {
            click.loading_ms += span;
        }
        if landed && click.first_frame_ms.is_none() {
            click.first_frame_ms = Some(t);
            click.shown_by = if by_paused {
                "paused preview"
            } else if by_still {
                "still"
            } else {
                "player"
            };
            click.picture_seconds = if by_paused {
                self.mini_preview
                    .scrubber
                    .as_ref()
                    .map(|s| s.snapshot().source_seconds)
            } else {
                state.frame.as_ref().map(|f| f.position)
            };
            // What libmpv draws is measured where it is drawn.
            click.picture = match &self.mini_preview.scrubber {
                Some(scrubber) if live || by_paused => scrubber.grid(),
                // A still is not measured as a grid of mean colours.
                _ if by_still => Vec::new(),
                _ => self.mini_preview.player_grid.clone(),
            };
        }
        if click.first_frame_ms.is_some()
            && open
            && click.resumed_ms.is_none()
            && state.playing
            && !loading
            && state.position >= target + 0.2
        {
            click.resumed_ms = Some(t);
        }
        if click.samples.len() < 4000 {
            click.samples.push(serde_json::json!([
                (t * 10.0).round() / 10.0,
                format!("{:?}", state.phase),
                state.playing,
                (state.position * 1000.0).round() / 1000.0,
                state.frame.as_ref().map(|f| f.id),
                state
                    .frame
                    .as_ref()
                    .map(|f| (f.position * 1000.0).round() / 1000.0),
                picture,
                spinner,
                luma.round(),
            ]));
        }
        let failed = state.phase == noh::playback::Phase::Error;
        if click.resumed_ms.is_none() && !failed && click.pressed.elapsed() < SEEK_LIMIT {
            return;
        }
        let click = self.seek_qa.click.take().unwrap();
        self.seek_qa.steady = None;
        self.seek_qa.results.push(serde_json::json!({
            "target_ms": click.target_ms,
            "from_ms": (click.position_before * 1000.0).round() + start as f64,
            "released_ms": click.released.map(|at| at.duration_since(click.pressed).as_secs_f64() * 1000.0),
            "first_frame_ms": click.first_frame_ms,
            "shown_by": click.shown_by,
            "resumed_ms": click.resumed_ms,
            "no_picture_ms": click.no_picture_ms,
            "black_ms": click.black_ms,
            "spinner_ms": click.spinner_ms,
            "loading_ms": click.loading_ms,
            "luma_before": click.luma_before,
            "picture": click.picture,
            "picture_seconds": click.picture_seconds,
            "worker": seek.map(|s| serde_json::json!({
                "target": s.target,
                "started_ms": s.started_ms,
                "audio_ms": s.audio_ms,
                "command_ms": s.command_ms,
                "spawned_ms": s.spawned_ms,
                "frame_ms": s.frame_ms,
            })),
            "error": state.error,
            "columns": ["ms", "phase", "playing", "position", "frame", "frame_position", "picture", "spinner", "luma"],
            "samples": click.samples,
        }));
    }
    fn seek_capture_terminal(&self) -> Option<&'static str> {
        let failed = self
            .mini_preview
            .player
            .as_ref()
            .is_some_and(|p| p.snapshot().phase == noh::playback::Phase::Error);
        if failed
            || self
                .seek_qa
                .started
                .is_some_and(|at| at.elapsed() > Duration::from_secs(240))
        {
            return Some("failed");
        }
        (self.seek_qa.done && self.seek_qa.click.is_none()).then_some("finished")
    }
}
/// A walk through the transport while the live preview plays, and where the
/// player stood after each step (`NOH_CAPTURE_TRANSPORT=1` with the
/// `live-preview` action). Each step calls what its button calls.
#[derive(Default)]
pub struct TransportQa {
    started: Option<Instant>,
    /// Since when the preview has been playing steadily, before the first step.
    steady: Option<Instant>,
    /// The step in progress, when it was made, and whether its sample is taken.
    step: Option<(usize, Instant, bool)>,
    samples: Vec<serde_json::Value>,
    done: bool,
}
const TRANSPORT_STEPS: &[&str] = &[
    "pause",
    "frame forward",
    "frame forward",
    "frame forward",
    "frame back",
    "frame back",
    "frame back",
    "play",
    "volume 0.3",
    "volume 0",
    "volume 0.7",
    "resize",
    "seek to the end",
    "play after the end",
    "edit",
    "play after the edit",
    "short view",
    "video view",
];
/// What a step waits for before its sample is taken.
const TRANSPORT_SETTLE: Duration = Duration::from_millis(700);
const TRANSPORT_LIMIT: Duration = Duration::from_secs(20);
pub fn transport_enabled() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_TRANSPORT").as_deref() == Ok("1")
}
impl NohApp {
    fn transport_capture_step(&mut self, ctx: &egui::Context) {
        if !transport_enabled() || self.transport_qa.done || !self.capture_project_started {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let state = self
            .mini_preview
            .player
            .as_ref()
            .map(noh::playback::Player::snapshot)
            .unwrap_or_default();
        let open = self.mini_preview.player_stamp.is_some();
        let playing_now = open
            && state.phase == noh::playback::Phase::Ready
            && state.playing
            && state.frame.is_some();
        let next = match self.transport_qa.step {
            None => {
                // The first step waits for steady playback.
                if !playing_now {
                    self.transport_qa.steady = None;
                    return;
                }
                if self
                    .transport_qa
                    .steady
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    < SEEK_DWELL
                {
                    return;
                }
                self.transport_qa.started = Some(Instant::now());
                0
            }
            Some((step, _, true)) => step + 1,
            Some((step, at, false)) => {
                let name = TRANSPORT_STEPS[step];
                let waited = at.elapsed();
                let settled = match name {
                    "seek to the end" => state.phase == noh::playback::Phase::Ended,
                    "play after the end" | "play after the edit" | "short view" | "video view" => {
                        playing_now && waited >= TRANSPORT_SETTLE
                    }
                    _ => waited >= TRANSPORT_SETTLE,
                };
                if !settled && waited < TRANSPORT_LIMIT {
                    return;
                }
                let (start, _) = self.mini_bounds();
                self.transport_qa.samples.push(serde_json::json!({
                    "step": name,
                    "ms": self.transport_qa.started.map(|s| s.elapsed().as_millis() as u64),
                    "settled": settled,
                    "engine": self.mini_preview.engines.last(),
                    "plays": self.mini_preview.engines.len(),
                    "live": self.mini_live(),
                    "open": open,
                    "phase": format!("{:?}", state.phase),
                    "playing": state.playing,
                    "position_ms": (state.position * 1000.0).round() as u64 + start,
                    "picture_ms": state.frame.as_ref().map(|f| (f.position * 1000.0).round() as u64 + start),
                    "pictures": state.frame.as_ref().map(|f| f.id),
                    "cursor_ms": self.mini_preview.cursor_ms,
                    "volume": state.volume,
                    "sound": state.audio_available,
                    "error": state.error.clone().or(self.mini_preview.error().cloned()),
                }));
                self.transport_qa.step = Some((step, at, true));
                step + 1
            }
        };
        let Some(&name) = TRANSPORT_STEPS.get(next) else {
            self.transport_qa.done = true;
            return;
        };
        // An edit clears the analysis; Play is only there again once it is back.
        if name == "play after the edit" && !(self.ready() && self.diagnosis_current()) {
            return;
        }
        let (start, end) = self.mini_bounds();
        let frame = 42;
        match name {
            "pause" | "play" | "play after the end" | "play after the edit" => self.mini_play(ctx),
            "frame forward" => self.navigate(self.mini_preview.cursor_ms + frame, true),
            "frame back" => self.navigate(self.mini_preview.cursor_ms.saturating_sub(frame), true),
            "volume 0.3" => self.mini_set_volume(0.3),
            "volume 0" => self.mini_set_volume(0.0),
            "volume 0.7" => self.mini_set_volume(0.7),
            "resize" => {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1100.0, 780.0)))
            }
            "seek to the end" => self.navigate(end.saturating_sub(1500).max(start), false),
            "edit" => {
                // What a change of fade does: the montage is no longer the one
                // being played.
                self.fade_in = if self.fade_in > 0.0 { 0.0 } else { 1.0 };
                self.in_enabled = self.fade_in > 0.0;
                self.invalidated();
            }
            "short view" if self.project.range.is_some() => self.play_live_scope(ctx, true),
            "video view" if self.project.range.is_some() => self.play_live_scope(ctx, false),
            // Without an excerpt there is no short to look at.
            _ => {}
        }
        self.transport_qa.step = Some((next, Instant::now(), false));
    }
    fn transport_capture_terminal(&self) -> Option<&'static str> {
        if self
            .transport_qa
            .started
            .is_some_and(|at| at.elapsed() > Duration::from_secs(180))
        {
            return Some("failed");
        }
        self.transport_qa.done.then_some("finished")
    }
}
/// A drag on the time ruler while the live preview plays, and what the monitor
/// showed at each frame of it (`NOH_CAPTURE_DRAG_PLAYING=1` with the
/// `live-preview` or `live-short` action): slow inside a clip, held, fast across
/// clips, held, released.
#[derive(Default)]
pub struct DragQa {
    started: Option<Instant>,
    /// Since when the preview has been playing steadily.
    steady: Option<Instant>,
    pressed: Option<Instant>,
    released: Option<Instant>,
    gone: bool,
    /// The time under the pointer at the last frame of the drag.
    target_ms: u64,
    samples: Vec<serde_json::Value>,
    done: bool,
}
/// The drag: seconds since the press, and the project time under the pointer.
/// Between two points the pointer moves at a constant speed.
const DRAG_PATH: [(f64, u64); 5] = [
    (0.0, 2_000),
    (2.0, 6_000),
    (2.6, 6_000),
    (3.6, 96_000),
    (4.4, 96_000),
];
pub fn drag_playing_enabled() -> bool {
    std::env::var_os("NOH_CAPTURE_UI").is_some()
        && std::env::var("NOH_CAPTURE_DRAG_PLAYING").as_deref() == Ok("1")
}
fn drag_target(seconds: f64) -> u64 {
    let mut before = DRAG_PATH[0];
    for point in DRAG_PATH {
        if seconds < point.0 {
            let span = point.0 - before.0;
            let part = if span > 0.0 {
                (seconds - before.0) / span
            } else {
                1.0
            };
            return (before.1 as f64 + (point.1 as f64 - before.1 as f64) * part).round() as u64;
        }
        before = point;
    }
    before.1
}
impl NohApp {
    /// The pointer of the drag: pressed once the preview plays steadily, moved
    /// at every frame along `DRAG_PATH`, released at its end.
    pub(super) fn drag_capture_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if !drag_playing_enabled() || self.drag_qa.done {
            return;
        }
        let Some(rect) = self.project_ui.timeline.ruler_rect() else {
            return;
        };
        self.drag_qa.started.get_or_insert_with(Instant::now);
        ctx.request_repaint_after(Duration::from_millis(8));
        let view = self
            .project
            .viewport
            .map(|v| v.0)
            .unwrap_or(noh::timeline::Range {
                start_ms: 0,
                end_ms: self.project.duration_ms,
            });
        let at = |ms: u64| {
            let x = rect.left()
                + rect.width()
                    * ((ms as f64 - view.start_ms as f64) / (view.end_ms - view.start_ms) as f64)
                        as f32;
            egui::pos2(x, rect.center().y)
        };
        if self.drag_qa.released.is_some() {
            if !self.drag_qa.gone {
                input.events.push(egui::Event::PointerGone);
                self.drag_qa.gone = true;
            }
            return;
        }
        let Some(pressed) = self.drag_qa.pressed else {
            let steady = self.mini_playing()
                && self
                    .mini_preview
                    .player
                    .as_ref()
                    .is_some_and(|p| p.snapshot().frame.is_some());
            if !steady {
                self.drag_qa.steady = None;
                return;
            }
            if self
                .drag_qa
                .steady
                .get_or_insert_with(Instant::now)
                .elapsed()
                < SEEK_DWELL
            {
                return;
            }
            let target = drag_target(0.0);
            input.events.push(egui::Event::PointerMoved(at(target)));
            input.events.push(egui::Event::PointerButton {
                pos: at(target),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
            self.drag_qa.pressed = Some(Instant::now());
            self.drag_qa.target_ms = target;
            return;
        };
        let seconds = pressed.elapsed().as_secs_f64();
        let target = drag_target(seconds);
        self.drag_qa.target_ms = target;
        input.events.push(egui::Event::PointerMoved(at(target)));
        if seconds >= DRAG_PATH[DRAG_PATH.len() - 1].0 {
            input.events.push(egui::Event::PointerButton {
                pos: at(target),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            });
            self.drag_qa.released = Some(Instant::now());
        }
    }
    /// After the frame is painted: where the pointer is and what the monitor shows.
    fn drag_capture_sample(&mut self) {
        let Some(pressed) = self.drag_qa.pressed else {
            return;
        };
        if self.drag_qa.done {
            return;
        }
        let (start, _) = self.mini_bounds();
        let player = self
            .mini_preview
            .player
            .as_ref()
            .filter(|_| self.mini_preview.player_stamp.is_some())
            .map(noh::playback::Player::snapshot);
        let scrub = self.mini_preview.scrubber.as_ref().map(|s| s.snapshot());
        let (picture, spinner) = self.mini_preview.painted;
        if self.drag_qa.samples.len() < 6000 {
            self.drag_qa.samples.push(serde_json::json!([
                (pressed.elapsed().as_secs_f64() * 10_000.0).round() / 10.0,
                self.drag_qa.released.is_none(),
                self.drag_qa.target_ms,
                self.mini_preview.cursor_ms,
                player.is_some(),
                player.as_ref().map(|p| p.playing),
                player.as_ref().and_then(|p| p.frame.as_ref().map(|f| f.id)),
                player.as_ref().and_then(|p| p
                    .frame
                    .as_ref()
                    .map(|f| (f.position * 1000.0).round() as u64 + start)),
                scrub.as_ref().map(|s| s.time_ms),
                scrub.as_ref().map(|s| s.frame),
                scrub.as_ref().and_then(|s| s.error.clone()),
                picture,
                self.mini_preview.painted_scrub,
                spinner,
            ]));
        }
        let Some(released) = self.drag_qa.released else {
            return;
        };
        // The drag is over once the preview plays again past where it ended.
        let resumed = player.as_ref().is_some_and(|p| {
            p.playing
                && p.phase == noh::playback::Phase::Ready
                && p.position * 1000.0 + start as f64 >= self.drag_qa.target_ms as f64 + 300.0
        });
        if resumed || released.elapsed() > SEEK_LIMIT {
            self.drag_qa.done = true;
        }
    }
    fn drag_capture_terminal(&self) -> Option<&'static str> {
        if self
            .drag_qa
            .started
            .is_some_and(|at| at.elapsed() > Duration::from_secs(120))
        {
            return Some("failed");
        }
        self.drag_qa.done.then_some("finished")
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
    /// `effect=<id>`: that caption effect applied (its first preset).
    pub effect: Option<String>,
    /// `style=settings`: the style panel shows the applied effect's settings.
    pub style_settings: bool,
    /// `more=1`: the settings show their second level.
    pub more: bool,
    /// `font=<value>`: the applied effect's font setting (`system:<Family>`…).
    pub font: Option<String>,
    /// `font_open=1`: the font picker's list open.
    pub font_open: bool,
    /// `colour_open=1`: the first colour setting's picker unfolded.
    pub colour_open: bool,
    /// `motion=found|checking|use|refused|details`: the scene beside the song
    /// listed, or checked (with a click) until that state.
    pub motion: Option<String>,
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
            ("effect", _) if !item.is_empty() => tweaks.effect = Some(item.into()),
            ("style", "settings") => tweaks.style_settings = true,
            ("more", "1") => tweaks.more = true,
            ("font", _) if !item.is_empty() => tweaks.font = Some(item.into()),
            ("font_open", "1") => tweaks.font_open = true,
            ("colour_open", "1") => tweaks.colour_open = true,
            ("motion", "found" | "checking" | "use" | "refused" | "details") => {
                tweaks.motion = Some(item.into())
            }
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
    "lyrics-lane-tip",
    "promote-menu",
    "lyrics-screen",
    "lyrics-screen-cut",
    "lyrics-screen-word",
    "lyrics-screen-syllables",
    "lyrics-screen-game",
    "lyrics-screen-game-hold",
    "lyrics-screen-game-missed",
    "lyrics-screen-summary",
    "lyrics-screen-read-only",
    "lyrics-screen-voices",
    "lyrics-screen-voices-menu",
    "lyrics-screen-voices-remove",
    "lyrics-screen-backing",
    "lyrics-screen-backing-game",
    "lyrics-first-edit",
    "lyrics-unsaved",
    "lyrics-srt-stale",
    "lyrics-save-failed",
    "lyrics-differ",
    "lyrics-differ-later",
    "short",
    "exporting",
    "done",
    "stale",
    "exists",
    "details",
    "options",
    "update-ready",
    "update-waiting",
    "update-busy",
    "analyzing",
    "cancelling",
];
const LYRICS_STATES: &[&str] = &[
    "unreadable",
    "lyrics-track-menu",
    "lyrics-lane-tip",
    "lyrics-screen",
    "lyrics-screen-cut",
    "lyrics-screen-word",
    "lyrics-screen-syllables",
    "lyrics-screen-game",
    "lyrics-screen-game-hold",
    "lyrics-screen-game-missed",
    "lyrics-screen-summary",
    "lyrics-screen-read-only",
    "lyrics-screen-voices",
    "lyrics-screen-voices-menu",
    "lyrics-screen-voices-remove",
    "lyrics-screen-backing",
    "lyrics-screen-backing-game",
    "lyrics-first-edit",
    "lyrics-unsaved",
    "lyrics-srt-stale",
    "lyrics-save-failed",
    "lyrics-differ",
    "lyrics-differ-later",
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
                // A downloaded update: the header badge, the wait for MCP
                // servers, a refusal naming what runs.
                if state.starts_with("update-") {
                    self.stage_autoupdate(state);
                }
                // Writing as you edit (lyrics plan 3): the first-edit question
                // after a real edit, and the marks a failed write leaves.
                if state == "lyrics-first-edit" {
                    self.edit_track(|t| {
                        t.cues.first_mut().map(|c| c.text.push_str(" !")).is_some()
                    });
                }
                if state == "lyrics-unsaved" || state == "lyrics-save-failed" {
                    self.lyrics_save.mark = Some(super::app_lyrics_save::Mark::Unsaved);
                }
                if state == "lyrics-srt-stale" {
                    self.lyrics_save.mark = Some(super::app_lyrics_save::Mark::SrtStale);
                }
                if matches!(state, "lyrics-differ" | "lyrics-differ-later")
                    && let (Some(path), Some(track)) =
                        (self.project.track.path.clone(), self.lyrics_track())
                {
                    // The source-of-truth question, staged over the real lyrics:
                    // 3 lines differ, 2 lose word times set by hand.
                    self.lyrics_pair.question = Some(super::app_lyrics_pair::Question {
                        path,
                        kind: super::app_lyrics_pair::Kind::Differ {
                            srt: track,
                            lines: 3,
                            lose: 2,
                            hand: true,
                            voices_lost: 0,
                            srt_time: Some("14:32".into()),
                            lyrics_time: Some("14:05".into()),
                        },
                        at_load: true,
                        open: state == "lyrics-differ",
                        quitting: false,
                    });
                }
                if state == "lyrics-save-failed" {
                    self.lyrics_save.failed = Some("Access is denied. (os error 5)".to_owned());
                }
                // The lyrics screen on a line, as it opens; with the text
                // cursor between two words; a word chosen in the track; a
                // word's split at the Syllables level. On the eleventh line
                // when the lyrics have one, as the mockup shows it.
                if let Some(scene) = state.strip_prefix("lyrics-screen")
                    && let Some(i) = self
                        .lyrics_track()
                        .and_then(|t| t.cues.len().checked_sub(1))
                        .map(|last| last.min(10))
                {
                    self.stage_lyrics_screen(i, scene);
                    if ctx.viewport_rect().width() < app_style::COMPACT_BELOW {
                        self.stage_lyrics_view_compact();
                    }
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
                    // A lyrics question: after its fade-in too.
                    && (!matches!(
                        state,
                        "lyrics-first-edit" | "lyrics-save-failed" | "lyrics-differ"
                    )
                        || settled(ctx, 500))
                    // The lyrics screen: once its monitor shows the picture
                    // of the staged time, not the wait for it.
                    && (!state.starts_with("lyrics-screen") || !self.mini_preview.painted.1)
                    // The lane's tooltip: once it has come, the pointer at rest.
                    && (state != LANE_TIP || settled(ctx, 1200))
            }
        }
    }
}
/// What a capture staged has finished coming in: a modal's fade-in (half a
/// second), a tooltip's delay. Counted from the capture's first wait for it.
fn settled(ctx: &egui::Context, ms: u64) -> bool {
    let id = egui::Id::new("capture-settled-since");
    let since = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, Instant::now));
    since.elapsed() > Duration::from_millis(ms)
}
impl NohApp {
    /// The pointer of the `lyrics-lane-tip` capture: brought once onto the
    /// lyrics lane, over the first line the timeline shows whole, and left
    /// there as a hand would leave it. Nothing is pressed.
    pub(super) fn lane_tip_capture_input(&mut self, input: &mut egui::RawInput) {
        if !lane_tip_enabled() || self.capture_overlay_step < 2 {
            return;
        }
        let (Some(lane), Some(track)) = (
            self.project_ui.timeline.lyrics_lane_rect(),
            self.lyrics_track(),
        ) else {
            return;
        };
        let view = self
            .project
            .viewport
            .map(|v| v.0)
            .unwrap_or(noh::timeline::Range {
                start_ms: 0,
                end_ms: self.project.duration_ms,
            });
        let Some(line) = track
            .cues
            .iter()
            .find(|c| c.start_ms >= view.start_ms && c.end_ms <= view.end_ms)
        else {
            return;
        };
        let middle = (line.start_ms + line.end_ms) as f64 / 2.0;
        let x = lane.left()
            + lane.width()
                * ((middle - view.start_ms as f64) / (view.end_ms - view.start_ms).max(1) as f64)
                    as f32;
        // Placed once: a tooltip waits for a pointer that has stopped moving,
        // and every such event counts as a move.
        static PLACED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !PLACED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(x, lane.center().y)));
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
        self.capture_effect_switch();
        self.seek_capture_sample();
        self.drag_capture_sample();
        self.transport_capture_step(ctx);
        if let Some(action) = project_action.as_deref()
            && !self.capture_project_started
            && self.ready()
            && self.diagnosis_current()
            && !self.project.track.loading()
            && !self.project.waveform_loading
            // A capture that requires libmpv plays once the copies are there; with no
            // libmpv preview none will be made, and it fails at once instead.
            && (!require_live()
                || self.mini_preview.scrubber.is_none()
                || self.mini_copies_ready())
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
        // `motion=` clicks « Vérifier et employer » once the scene is listed.
        if self.capture_overlay_step >= 1 {
            self.motion_capture_step(ctx);
        }
        // `effect=` applies a caption effect once, as a click on its card would;
        // `style=settings` then shows its settings (compact: in the sheet).
        if self.capture_overlay_step >= 1
            && let Ok(tweaks) = parse_tweaks(&tweaks_text())
        {
            if let Some(id) = tweaks.effect.as_deref()
                && self.project.effect.as_ref().is_none_or(|c| c.id != id)
            {
                let lang = self.locale.language.code().to_string();
                let options = self.effects.options(&lang);
                let card = app_style_panel::cards(&options, None, None)
                    .into_iter()
                    .find(|c| c.id == id)
                    .unwrap_or_else(|| panic!("NOH_CAPTURE_TWEAKS: no effect {id:?}"));
                self.choose_effect(Some(card));
            }
            if tweaks.style_settings && !self.style_panel.settings {
                self.style_panel.settings = true;
                self.style_panel.sheet = true;
                self.style_panel.more = tweaks.more;
                self.style_panel.font_open = tweaks.font_open;
                self.style_panel.colour_open = tweaks.colour_open;
                if tweaks.colour_open {
                    self.recent_colors = app_color::capture_recent();
                }
                self.style_panel.font_reveal = tweaks.font_open || tweaks.font.is_some();
                if let (Some(font), Some(choice)) = (&tweaks.font, self.project.effect.as_mut()) {
                    choice.values.insert("font".into(), serde_json::json!(font));
                }
            }
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
        // « Promote NOH » ticked, its corner menu open with the colour picker
        // shown (the setting is not saved).
        if self.capture_overlay_step >= 1 && self.capture_overlay.as_deref() == Some("promote-menu")
        {
            self.promote = true;
            let owner = egui::Id::new(app_promote::PICKER);
            if !app_color::is_open(ctx, owner) {
                self.recent_colors = app_color::capture_recent();
                let c = self.promote_color;
                app_color::open(ctx, owner, egui::Color32::from_rgb(c.0, c.1, c.2));
            }
            if !egui::Popup::is_id_open(ctx, egui::Id::new(app_promote::MENU)) {
                egui::Popup::open_id(ctx, egui::Id::new(app_promote::MENU));
            }
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
            if drag_playing_enabled() {
                self.drag_capture_terminal()
            } else if transport_enabled() {
                self.transport_capture_terminal()
            } else if seek_playing_enabled() {
                self.seek_capture_terminal()
            } else {
                self.mini_capture_terminal()
            }
        } else if project_action.is_some() {
            // A preview render can also be played back to a set time (measurement).
            let play_render = std::env::var_os("NOH_CAPTURE_PLAYBACK_UNTIL_MS").is_some()
                && matches!(project_action.as_deref(), Some("preview" | "short-preview"));
            if play_render
                && self.capture_project_started
                && self.job.is_none()
                && !self.project.busy()
            {
                if self.mini_preview.capture_started.is_none()
                    && self.mini_preview.player_stamp.is_some()
                {
                    self.mini_preview.capture_started = Some(Instant::now());
                }
                self.mini_capture_terminal()
            } else if self.capture_project_started
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
            let limit = if self.capture_motion().is_some() {
                // A scene is checked in the worker, then prepared by the window.
                240
            } else if self.capture_overlay.is_some() {
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
        if let Some(motion) = self.capture_motion() {
            capture_ready &= self.motion_capture_ready(&motion);
        }
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
        // A real operation has no deadline of its own here, so a capture that never
        // ends leaves no report. Once a second, record where it stands: the harness
        // reads this file whether the capture ends or not. It decides nothing.
        if actual
            && self
                .capture_state_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.capture_state_at = Some(Instant::now());
            let state = serde_json::json!({
                "elapsed_ms": self.opened.elapsed().as_millis() as u64,
                "focused": ctx.input(|i| i.focused),
                "geometry_ready": geometry_ready,
                "ready": self.ready(),
                "diagnosis_current": self.diagnosis_current(),
                "track_loading": self.project.track.loading(),
                "waveform_loading": self.project.waveform_loading,
                "project_started": self.capture_project_started,
                "project_terminal": terminal,
                "capture_ready": capture_ready,
                "capture_requested": self.capture_requested,
                "mini_preview": self.mini_capture_report(),
            });
            let target = std::path::PathBuf::from(&path).with_extension("state.json");
            let partial = target.with_extension("json.partial");
            if std::fs::write(&partial, state.to_string()).is_ok() {
                let _ = std::fs::rename(&partial, &target);
            }
        }
        if !self.capture_requested
            && self.opened.elapsed() > Duration::from_secs(2)
            && (capture_ready || timed_out)
        {
            let report = serde_json::json!({"requested_tweaks":tweaks_text(),"requested_state":std::env::var("NOH_CAPTURE_STATE").unwrap_or_else(|_|if populated {"project"} else {"empty"}.into()),"captured_state":self.capture_state.clone().or_else(||self.capture_overlay.clone()).unwrap_or_else(||if populated {"project"} else {"empty"}.into()),"capture_ready":capture_ready,"timed_out":timed_out,"viewport":{"width":viewport.width(),"height":viewport.height()},"pixels_per_point":ctx.pixels_per_point(),"fixture":fixture,"section":std::env::var("NOH_CAPTURE_SECTION").ok(),"revision":self.revision,"ready":self.ready(),"diagnosis":self.diagnosis,"diagnosis_error":self.diagnosis_error.as_ref().map(|e|&e.key),"log":self.log});
            let mut report = report;
            report["resource_issues"] = serde_json::json!(self.resources.issues.len());
            report["project_action"] = serde_json::json!(project_action);
            report["focused"] = serde_json::json!(ctx.input(|i| i.focused));
            report["mini_preview"] = self.mini_capture_report();
            if scrub_qa {
                let snapshot = self.mini_preview.scrubber.as_ref().map(|s| s.snapshot());
                report["scrub"] = serde_json::json!({"samples":self.scrub_qa.samples,"error":snapshot.and_then(|s|s.error),"runtime":self.mini_preview.scrubber.is_some()});
                report["scrub"]["handoff"] = serde_json::json!(self.scrub_qa.handoff);
            }
            if seek_playing_enabled() {
                report["seek_playing"] = serde_json::json!({"targets_ms":seek_targets(),"dwell_ms":SEEK_DWELL.as_millis() as u64,"black_below":SEEK_BLACK,"results":self.seek_qa.results});
            }
            if transport_enabled() {
                report["transport"] = serde_json::json!(self.transport_qa.samples);
            }
            if drag_playing_enabled() {
                report["drag_playing"] = serde_json::json!({"path":DRAG_PATH,"columns":["ms","down","target_ms","cursor_ms","player_open","playing","player_frame","player_ms","paused_ms","paused_frame","paused_error","picture","paused_picture","spinner"],"samples":self.drag_qa.samples});
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
                        // A staged edit (the first-edit question) is not written
                        // to the fixture and must not ask before closing.
                        self.project.edit = None;
                        // A staged job held the monitor's picture back: the
                        // closing window must not start its decoder, which
                        // would outlive it.
                        self.mini_keep_still();
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
                voices: Vec::new(),
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
