//! What the agent does in a window it drives (agent-follow note §3): its
//! gestures, applied between two frames by the functions the matching clicks
//! call; what it sees (`state`, with where the person is); and `look`, a PNG
//! of the window. One playhead is shared: while the person navigates (and
//! for 3 s after their last navigation gesture, or as long as they play) the
//! agent's playhead gestures wait, then are refused (note §4.2).
use super::{NohApp, app_driven::Driven};
use eframe::egui::{self, Rect};
use noh::driven::{Action, Answer, LineEdit, LookScope, Refusal, ToServer};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// The person navigates for this long after their last navigation gesture.
pub(super) const NAVIGATION: Duration = Duration::from_secs(3);
/// Position-only changes go out at most this often (4 per second).
const POSITION_EVERY: Duration = Duration::from_millis(250);
/// How long a paused look waits for the picture at the playhead.
const SETTLE: Duration = Duration::from_secs(2);
/// A look whose screenshot has not come by then fails (the window was
/// minimized after the request, or no picture came).
const LOOK_LIMIT: Duration = Duration::from_secs(5);
/// A `state` line stays well under the server's line limit.
const STATE_BYTES: usize = 60 * 1024;
/// Lyrics lines listed in `state`, and the characters kept of each.
const LINES: usize = 400;
const LINE_TEXT: usize = 160;
/// The gestures the history menu keeps.
const HISTORY: usize = 20;

/// Who moved the playhead last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Controller {
    #[default]
    Agent,
    Person,
}

/// A playhead gesture of the agent, waiting while the person navigates.
pub(super) struct Pending {
    pub id: u64,
    pub gesture: Playhead,
    /// Refused if the person still navigates then.
    pub until: Instant,
}

pub(super) enum Playhead {
    Seek(u64),
    Play,
    Pause,
    Look(Look),
    /// A scene look of the agent: the playhead there, its marker drawn.
    SceneLook(super::app_driven_scene::SceneMarker),
}

/// A `look` under way.
pub(super) struct Look {
    pub scope: LookScope,
    pub output: PathBuf,
    pub at_ms: Option<u64>,
    /// Waiting for the picture at the paused playhead until then, else none.
    pub settle_until: Option<Instant>,
    /// Painted frames since the request, so the screenshot shows the change.
    pub frames: u8,
    /// The screenshot was asked for.
    pub requested: bool,
    /// When the look started waiting for its picture.
    pub started: Instant,
}

pub(super) struct LookJob {
    pub id: u64,
    pub look: Look,
}

fn refusal(code: &str, message: &str) -> Refusal {
    Refusal::new(code, message)
}

impl NohApp {
    /// The person navigates now: within 3 s of their last navigation gesture,
    /// or while they play.
    pub(super) fn person_navigating(&self) -> Option<Duration> {
        let d = self.driven.as_ref()?;
        if d.controller == Controller::Person && self.mini_playing() {
            return Some(Duration::MAX);
        }
        let at = d.nav_at?;
        (at + NAVIGATION).checked_duration_since(Instant::now())
    }

    /// An `act` of the agent.
    pub(super) fn driven_act(&mut self, ctx: &egui::Context, id: u64, action: Action) {
        if !self.driven_locked() {
            return self.driven_answer(
                id,
                Err(refusal(
                    "app.released_by_user",
                    "The person stopped the piloting: this window is theirs now.",
                )),
            );
        }
        let playhead = match &action {
            Action::Seek { ms } => Some(Playhead::Seek(*ms)),
            Action::Play {} => Some(Playhead::Play),
            Action::Pause {} => Some(Playhead::Pause),
            Action::ShowSceneLook {
                at_ms,
                safe_area,
                grid,
            } => Some(Playhead::SceneLook(super::app_driven_scene::SceneMarker {
                at_ms: *at_ms,
                safe_area: *safe_area,
                grid: *grid,
            })),
            _ => None,
        };
        if let Some(gesture) = playhead {
            // Recorded when it applies (it may wait for the person, or be refused).
            return self.driven_queue(ctx, id, gesture);
        }
        let what = self.driven_history_text(&action);
        let outcome = self.driven_apply(ctx, action);
        if outcome.is_ok() {
            self.driven_record(what);
        }
        self.driven_answer(id, outcome);
    }

    /// One line of the history menu for a gesture, written before it applies
    /// (a removed clip's name is gone after).
    fn driven_history_text(&mut self, action: &Action) -> String {
        let l = self.locale.language;
        let file = |path: &std::path::Path| {
            path.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let clip = |id: u64| {
            self.clips
                .iter()
                .find(|c| c.id == id)
                .map(|c| file(c.item.path()))
                .unwrap_or_default()
        };
        let time = |ms: u64| noh::timeline::format_time(ms);
        match action {
            Action::Add { paths } => l.format("driven.h.add", &[paths.len().to_string()]),
            Action::SetPhotoDuration { clip_id, seconds } => l.format(
                "driven.h.photo_duration",
                &[clip(*clip_id), format!("{seconds:.1}")],
            ),
            Action::EditLine { edit, .. } => match edit {
                LineEdit::SetText { index, .. } => {
                    l.format("driven.h.line_text", &[(index + 1).to_string()])
                }
                LineEdit::SetStart { index, .. }
                | LineEdit::SetEnd { index, .. }
                | LineEdit::Shift { index, .. } => {
                    l.format("driven.h.line_time", &[(index + 1).to_string()])
                }
                LineEdit::Add { ms, .. } => l.format("driven.h.line_add", &[time(*ms)]),
                LineEdit::Delete { index } => {
                    l.format("driven.h.line_delete", &[(index + 1).to_string()])
                }
                LineEdit::SetVoices { index, .. } => {
                    l.format("driven.h.line_voices", &[(index + 1).to_string()])
                }
                LineEdit::NameVoices { names } => {
                    l.format("driven.h.voices", &[names.len().to_string()])
                }
            },
            Action::Undo {} => l.text("driven.h.undo").into(),
            Action::Redo {} => l.text("driven.h.redo").into(),
            Action::ChooseEffect { id: None, .. } => l.text("driven.h.effect_none").into(),
            Action::ChooseEffect {
                id: Some(id),
                preset,
            } => {
                let lang = l.code().to_string();
                let name = self
                    .effects
                    .options(&lang)
                    .into_iter()
                    .find(|(_, c)| &c.id == id && preset.as_ref().is_none_or(|p| &c.preset == p))
                    .map(|(name, _)| name)
                    .unwrap_or_else(|| id.clone());
                l.format("driven.h.effect", &[name])
            }
            Action::SetSafeArea { .. } => l.text("driven.h.safe_area").into(),
            Action::SetRange {
                start_ms: Some(start),
                end_ms: Some(end),
            } => l.format("driven.h.range", &[time(*start), time(*end)]),
            Action::SetRange { .. } => l.text("driven.h.range_none").into(),
            Action::Seek { ms } => l.format("driven.h.seek", &[time(*ms)]),
            Action::SetScene { path: Some(path) } => l.format("driven.h.scene", &[file(path)]),
            Action::SetScene { path: None } => l.text("driven.h.scene_none").into(),
            Action::ShowSceneLook { at_ms, .. } => l.format("driven.h.scene_look", &[time(*at_ms)]),
            Action::Play {} => l.text("driven.h.play").into(),
            Action::Pause {} => l.text("driven.h.pause").into(),
            Action::Answer { .. } => l.text("driven.h.answer").into(),
            Action::RemoveClip { clip_id } => l.format("driven.h.remove", &[clip(*clip_id)]),
            Action::MoveClip { clip_id, to_index } => l.format(
                "driven.h.move",
                &[clip(*clip_id), (to_index + 1).to_string()],
            ),
            Action::SetFades { .. } => l.text("driven.h.fades").into(),
            Action::SetConvertAll { convert_all: true } => l.text("driven.h.convert_on").into(),
            Action::SetConvertAll { convert_all: false } => l.text("driven.h.convert_off").into(),
            Action::SetEffectSettings { .. } => l.text("driven.h.effect_settings").into(),
            Action::SetFraming { .. } => l.text("driven.h.framing").into(),
            Action::SetDestination {
                path: Some(path), ..
            } => l.format("driven.h.destination", &[file(path)]),
            Action::SetDestination {
                short_name: Some(name),
                ..
            } => l.format("driven.h.destination", std::slice::from_ref(name)),
            Action::SetDestination { .. } => l.text("driven.h.destination_none").into(),
            Action::Export { short: false } => l.text("driven.h.export").into(),
            Action::Export { short: true } => l.text("driven.h.export_short").into(),
            Action::Cancel {} => l.text("driven.h.cancel").into(),
        }
    }

    /// Keep a gesture in the history menu (the last twenty).
    fn driven_record(&mut self, what: String) {
        if let Some(d) = &mut self.driven {
            let at = chrono::Local::now().format("%H:%M:%S").to_string();
            d.history.push_back((at, what));
            while d.history.len() > HISTORY {
                d.history.pop_front();
            }
        }
    }

    /// A `look` of the agent.
    pub(super) fn driven_look(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        scope: LookScope,
        at_ms: Option<u64>,
        output: PathBuf,
    ) {
        let check = if !self.driven_locked() {
            Err(refusal(
                "app.released_by_user",
                "The person stopped the piloting: this window is theirs now.",
            ))
        } else if ctx.input(|i| i.viewport().minimized) == Some(true) {
            Err(refusal(
                "app.hidden",
                "The window is minimized: nothing is painted. The state stays readable; do not restore a window the person minimized.",
            ))
        } else if !output.is_absolute() || output.exists() {
            Err(refusal(
                "app.output_invalid",
                "output must be a free absolute path.",
            ))
        } else if self.driven.as_ref().is_some_and(|d| d.look.is_some()) {
            Err(refusal("app.busy", "Another look is under way."))
        } else {
            Ok(())
        };
        if let Err(refusal) = check {
            return self.driven_answer(id, Err(refusal));
        }
        // An untimed look also needs the paused picture if a separate seek
        // just moved the playhead. It must not move the playhead itself.
        let settle_until = (at_ms.is_none() && !self.clips.is_empty() && !self.mini_playing())
            .then(|| Instant::now() + SETTLE);
        let look = Look {
            scope,
            output,
            at_ms,
            settle_until,
            frames: 0,
            requested: false,
            started: Instant::now(),
        };
        if at_ms.is_some() {
            self.driven_queue(ctx, id, Playhead::Look(look));
        } else if let Some(d) = &mut self.driven {
            d.look = Some(LookJob { id, look });
            ctx.request_repaint();
            ctx.request_repaint_after(LOOK_LIMIT);
        }
    }

    fn driven_queue(&mut self, ctx: &egui::Context, id: u64, gesture: Playhead) {
        if let Some(d) = &mut self.driven {
            d.pending.push(Pending {
                id,
                gesture,
                until: Instant::now() + NAVIGATION,
            });
        }
        // Applied at once when the person does not navigate.
        self.driven_pending(ctx);
    }

    /// Each frame, before the agent's new messages: a look that waited too
    /// long for its picture fails (a repaint is asked for at its limit, so an
    /// idle or minimized window gets there too).
    pub(super) fn driven_look_expire(&mut self) {
        if let Some(d) = &self.driven
            && let Some(job) = &d.look
            && let Some(left) = LOOK_LIMIT.checked_sub(job.look.started.elapsed())
        {
            // Each frame re-arms it: a frame drawn meanwhile consumes the request.
            d.ctx.request_repaint_after(left);
        }
        let stale = self
            .driven
            .as_ref()
            .and_then(|d| d.look.as_ref())
            .is_some_and(|job| job.look.started.elapsed() >= LOOK_LIMIT);
        if stale && let Some(job) = self.driven.as_mut().and_then(|d| d.look.take()) {
            self.driven_answer(
                job.id,
                Err(refusal(
                    "app.look_failed",
                    "No picture came in time (the window may have been minimized). Try again.",
                )),
            );
        }
    }

    /// Each frame: the playhead gestures waiting for the person are applied
    /// or refused.
    pub(super) fn driven_pending(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.driven else {
            return;
        };
        if d.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut d.pending);
        let navigating = self.person_navigating();
        let now = Instant::now();
        for p in pending {
            match navigating {
                None => {
                    let outcome = self.driven_playhead(ctx, p.id, p.gesture);
                    if let Some(outcome) = outcome {
                        self.driven_answer(p.id, outcome);
                    }
                }
                Some(left) if now >= p.until => {
                    let mut refusal = refusal(
                        "app.user_navigating",
                        "The person is moving through the video: the playhead is theirs. Try again later, or look where they are (look without at_ms).",
                    );
                    refusal.details = Some(serde_json::json!({
                        "user_position": self.user_position(),
                        "retry_after_ms": (left != Duration::MAX).then_some(left.as_millis() as u64),
                    }));
                    self.driven_answer(p.id, Err(refusal));
                }
                Some(_) => {
                    if let Some(d) = &mut self.driven {
                        d.pending.push(p);
                    }
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
        }
    }

    /// Apply a playhead gesture; `None` when its answer comes later (a look).
    fn driven_playhead(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        gesture: Playhead,
    ) -> Option<Result<serde_json::Value, Refusal>> {
        if let Some(d) = &mut self.driven {
            d.controller = Controller::Agent;
        }
        match gesture {
            Playhead::Seek(ms) => {
                self.navigate(ms, true);
                self.driven_mark();
                let what = self.driven_history_text(&Action::Seek { ms });
                self.driven_record(what);
                Some(Ok(serde_json::json!({ "ms": self.mini_preview.cursor_ms })))
            }
            Playhead::Play => {
                if !self.mini_playing() {
                    self.mini_play(ctx);
                }
                self.driven_mark();
                let what = self.driven_history_text(&Action::Play {});
                self.driven_record(what);
                Some(Ok(serde_json::Value::Null))
            }
            Playhead::Pause => {
                self.mini_pause();
                self.driven_mark();
                let what = self.driven_history_text(&Action::Pause {});
                self.driven_record(what);
                Some(Ok(serde_json::Value::Null))
            }
            Playhead::SceneLook(mut marker) => {
                self.navigate(marker.at_ms, true);
                // Where the playhead landed (the end of the song at most).
                marker.at_ms = self.mini_preview.cursor_ms;
                if let Some(d) = &mut self.driven {
                    d.marker = Some(marker);
                }
                self.driven_mark();
                Some(Ok(serde_json::json!({ "ms": marker.at_ms })))
            }
            Playhead::Look(mut look) => {
                if let Some(ms) = look.at_ms {
                    self.navigate(ms, true);
                    self.driven_mark();
                    look.settle_until = Some(Instant::now() + SETTLE);
                }
                look.started = Instant::now();
                if let Some(d) = &mut self.driven {
                    d.look = Some(LookJob { id, look });
                }
                ctx.request_repaint();
                ctx.request_repaint_after(LOOK_LIMIT);
                None
            }
        }
    }

    /// A drawn frame: the look under way asks for its screenshot once the
    /// screen shows what was asked (the picture at T, a frame after).
    pub(super) fn driven_look_frame(&mut self, ctx: &egui::Context) {
        let settled = self.driven.as_ref().and_then(|d| d.monitor).map(|(_, s)| s);
        let Some(d) = &mut self.driven else {
            return;
        };
        let Some(job) = &mut d.look else {
            return;
        };
        if job.look.requested {
            return;
        }
        job.look.frames = job.look.frames.saturating_add(1);
        let waiting = job
            .look
            .settle_until
            .is_some_and(|until| Instant::now() < until && settled != Some(true));
        if job.look.frames < 2 || waiting {
            ctx.request_repaint();
            return;
        }
        job.look.requested = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            job.id,
        )));
    }

    /// `raw_input_hook`: a screenshot asked for by a look.
    pub(super) fn driven_screenshot(&mut self, ctx: &egui::Context, input: &egui::RawInput) {
        let Some(d) = &self.driven else {
            return;
        };
        let Some(job) = &d.look else {
            return;
        };
        let id = job.id;
        let image = input.events.iter().find_map(|event| match event {
            egui::Event::Screenshot {
                user_data, image, ..
            } if user_data
                .data
                .as_ref()
                .and_then(|data| data.downcast_ref::<u64>())
                == Some(&id) =>
            {
                Some(image.clone())
            }
            _ => None,
        });
        let Some(image) = image else {
            return;
        };
        let job = self
            .driven
            .as_mut()
            .and_then(|d| d.look.take())
            .expect("a look");
        let crop = match job.look.scope {
            LookScope::Window => None,
            LookScope::Monitor => self.driven.as_ref().and_then(|d| d.monitor).map(|(r, _)| r),
        };
        let scale = ctx.pixels_per_point();
        let outcome = write_png(&image, crop, scale, &job.look.output).map(|(w, h)| {
            serde_json::json!({
                "width": w,
                "height": h,
                "at_ms": self.mini_preview.cursor_ms,
            })
        });
        self.driven_answer(job.id, outcome);
    }

    /// The agent's mark: the window shows that the agent just acted.
    pub(super) fn driven_mark(&mut self) {
        if let Some(d) = &mut self.driven {
            d.acted_at = Some(Instant::now());
        }
    }

    /// Answer `id`, with the state revision the gesture led to.
    fn driven_answer(&mut self, id: u64, outcome: Result<serde_json::Value, Refusal>) {
        let revision = self.driven_state(true);
        let Some(d) = &self.driven else {
            return;
        };
        let (result, error) = match outcome {
            Ok(mut value) => {
                if !value.is_object() {
                    value = serde_json::json!({});
                }
                value["revision"] = revision.into();
                (Some(value), None)
            }
            Err(refusal) => (None, Some(refusal)),
        };
        let _ = d.send_message(&ToServer::Done { id, result, error });
    }

    /// The gestures that are not about the playhead.
    fn driven_apply(
        &mut self,
        ctx: &egui::Context,
        action: Action,
    ) -> Result<serde_json::Value, Refusal> {
        let refused = |message: &str| Err(refusal("app.refused", message));
        // What the interface locks while a job runs is refused too.
        let locked_by_job = matches!(
            action,
            Action::Add { .. }
                | Action::SetPhotoDuration { .. }
                | Action::ChooseEffect { .. }
                | Action::SetSafeArea { .. }
                | Action::SetRange { .. }
                | Action::RemoveClip { .. }
                | Action::MoveClip { .. }
                | Action::SetFades { .. }
                | Action::SetConvertAll { .. }
                | Action::SetEffectSettings { .. }
                | Action::SetFraming { .. }
                | Action::SetDestination { .. }
                | Action::Export { .. }
        );
        if locked_by_job && self.busy() {
            return Err(refusal(
                "app.job_running",
                "A job is running: this cannot change until it ends or is cancelled.",
            ));
        }
        match action {
            Action::Add { paths } => {
                if paths.is_empty() || paths.iter().any(|p| !p.is_absolute() || !p.exists()) {
                    return refused("Every path must be absolute and exist.");
                }
                if let Some(kind) = self.open_question() {
                    // A drop could replace the lyrics the question is about.
                    let mut refusal = refusal(
                        "app.question_open",
                        "A question is open in the window: answer it before adding files.",
                    );
                    refusal.details = Some(serde_json::json!({ "question": kind }));
                    return Err(refusal);
                }
                self.drop_files(paths, ctx);
                self.driven_mark();
                Ok(serde_json::json!({ "notice": self.notice.as_ref().map(|m| m.key.clone()) }))
            }
            Action::SetPhotoDuration { clip_id, seconds } => {
                if self.set_image_duration(clip_id, seconds) {
                    self.driven_mark();
                    Ok(serde_json::Value::Null)
                } else {
                    refused("No picture with that clip id, or a duration it cannot take.")
                }
            }
            Action::EditLine { edit, based_on } => {
                if let Some(revision) = based_on
                    && revision != self.project.track.revision
                {
                    let mut refusal = refusal(
                        "app.state_changed",
                        "The lyrics were reloaded since that revision: read the state again.",
                    );
                    refusal.details = Some(serde_json::json!({
                        "lyrics_revision": self.project.track.revision,
                    }));
                    return Err(refusal);
                }
                let label = String::new();
                let mut added = None;
                let changed = self.edit_track_as(label, |t| {
                    use noh::subtitle_edit as e;
                    let n = t.cues.len();
                    match &edit {
                        LineEdit::SetText { index, text } => {
                            *index < n && e::set_text(t, *index, text)
                        }
                        LineEdit::SetStart { index, ms } => {
                            *index < n && {
                                e::set_edge(t, *index, e::Edge::Start, *ms);
                                true
                            }
                        }
                        LineEdit::SetEnd { index, ms } => {
                            *index < n && {
                                e::set_edge(t, *index, e::Edge::End, *ms);
                                true
                            }
                        }
                        LineEdit::Shift { index, delta_ms } => {
                            *index < n && e::move_cue(t, *index, *delta_ms) != 0
                        }
                        LineEdit::Add { ms, text } => {
                            added = e::add_cue(t, *ms, text);
                            added.is_some()
                        }
                        LineEdit::Delete { index } => *index < n && e::delete(t, *index).is_some(),
                        LineEdit::SetVoices { index, voices } => {
                            *index < n && e::set_voices(t, *index, *index, voices)
                        }
                        LineEdit::NameVoices { names } => e::declare_voices(
                            t,
                            names
                                .iter()
                                .map(|name| noh::subtitle_track::Voice {
                                    name: name.clone().filter(|n| !n.trim().is_empty()),
                                })
                                .collect(),
                        ),
                    }
                });
                if changed {
                    self.driven_mark();
                    Ok(serde_json::json!({ "added": added, "question": self.open_question() }))
                } else {
                    refused(
                        "Nothing changed: no lyrics, no such line, no room there, a voice not declared (name_voices first), the lyrics locked by a job, or a lyrics question waiting for its answer.",
                    )
                }
            }
            Action::Undo {} | Action::Redo {} => {
                if self.lyrics_frozen() || self.lyrics_question_open() {
                    return refused("The lyrics cannot change now (a job, or a question open).");
                }
                let done = if matches!(action, Action::Undo {}) {
                    self.undo()
                } else {
                    self.redo()
                };
                if done {
                    self.driven_mark();
                    Ok(serde_json::Value::Null)
                } else {
                    refused("Nothing to undo or redo.")
                }
            }
            Action::ChooseEffect { id, preset } => {
                let Some(id) = id else {
                    self.choose_effect(None);
                    self.driven_mark();
                    return Ok(serde_json::Value::Null);
                };
                let lang = self.locale.language.code().to_string();
                let options = self.effects.options(&lang);
                let choice = options
                    .iter()
                    .map(|(_, c)| c)
                    .find(|c| c.id == id && preset.as_ref().is_none_or(|p| &c.preset == p))
                    .cloned();
                match choice {
                    Some(choice) => {
                        self.choose_effect(Some(choice));
                        self.driven_mark();
                        Ok(serde_json::Value::Null)
                    }
                    None => refused("No such effect or preset (see noh_list_effects)."),
                }
            }
            Action::SetSafeArea { area } => {
                self.project.set_short_safe_area(area);
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::SetRange { start_ms, end_ms } => {
                let range = match (start_ms, end_ms) {
                    (None, None) => None,
                    (Some(start), Some(end)) => {
                        match noh::timeline::Range::new(start, end, self.project.duration_ms) {
                            Some(range) => Some(range),
                            None => return refused("That range does not fit the song."),
                        }
                    }
                    _ => return refused("Give both start_ms and end_ms, or neither."),
                };
                if self.project.set_range(range) && self.mini_preview.short {
                    self.mini_preview.invalidate();
                }
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::Answer { answer, path } => self.driven_answer_question(ctx, answer, path),
            Action::RemoveClip { clip_id } => {
                if self.remove_clip(clip_id) {
                    self.driven_mark();
                    Ok(serde_json::Value::Null)
                } else {
                    refused("No clip with that id.")
                }
            }
            Action::MoveClip { clip_id, to_index } => {
                let Some(from) = self.clips.iter().position(|c| c.id == clip_id) else {
                    return refused("No clip with that id.");
                };
                if to_index >= self.clips.len() {
                    return refused("to_index is past the last clip.");
                }
                self.move_clip(from, to_index);
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::SetFades {
                fade_in_seconds,
                fade_out_seconds,
                clip_audio,
            } => {
                let bad = |v: Option<f64>| v.is_some_and(|s| !s.is_finite() || s < 0.0);
                if bad(fade_in_seconds) || bad(fade_out_seconds) {
                    return refused("A fade is a number of seconds, 0 or more.");
                }
                let before = (
                    self.in_enabled,
                    self.fade_in,
                    self.out_enabled,
                    self.fade_out,
                    self.clip_audio,
                );
                if let Some(s) = fade_in_seconds {
                    self.in_enabled = s > 0.0;
                    if s > 0.0 {
                        self.fade_in = s;
                    }
                }
                if let Some(s) = fade_out_seconds {
                    self.out_enabled = s > 0.0;
                    if s > 0.0 {
                        self.fade_out = s;
                    }
                }
                if let Some(on) = clip_audio {
                    self.clip_audio = on;
                }
                if !self.fades_valid() {
                    (
                        self.in_enabled,
                        self.fade_in,
                        self.out_enabled,
                        self.fade_out,
                        self.clip_audio,
                    ) = before;
                    return refused(
                        "These fades do not fit the montage (together longer than it).",
                    );
                }
                self.invalidated();
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::SetConvertAll { convert_all } => {
                if self.partial == convert_all {
                    self.partial = !convert_all;
                    self.invalidated();
                }
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::SetEffectSettings {
                preset,
                values,
                size,
                placement,
                show_on_video,
            } => self.driven_effect_settings(preset, values, size, placement, show_on_video),
            Action::SetFraming {
                framing,
                restart_loops,
            } => {
                let before = (self.project.restart_loops, self.project.framing);
                if let Some(framing) = framing {
                    self.project.framing = framing;
                }
                if let Some(on) = restart_loops {
                    self.project.restart_loops = on;
                }
                // As the range row does: only the short changes, one revision.
                if before != (self.project.restart_loops, self.project.framing) {
                    self.project.short_revision = self.project.short_revision.wrapping_add(1);
                    self.project.failure = None;
                    self.project.outcome = None;
                }
                self.driven_mark();
                Ok(serde_json::Value::Null)
            }
            Action::SetDestination { path, short_name } => {
                if let Some(path) = path {
                    let fine = path.is_absolute()
                        && path
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
                        && path.parent().is_some_and(|p| p.is_dir());
                    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
                        return refused("The destination needs a folder and a file name.");
                    };
                    if !fine {
                        return refused(
                            "The destination must be an absolute .mp4 path in an existing folder.",
                        );
                    }
                    self.output_folder = folder.to_path_buf();
                    self.output_name = name.to_string_lossy().into_owned();
                    self.output_base = self.output_name.clone();
                    self.destination_edited();
                }
                if let Some(name) = short_name {
                    if super::app_destination::syntax(&name).is_some() {
                        return refused("That short name cannot be a file name.");
                    }
                    self.project.short_name = name;
                    self.project.short_named = true;
                }
                self.driven_mark();
                Ok(serde_json::json!({ "output": self.output }))
            }
            Action::Export { short } => {
                let status = self.status();
                let available = if short {
                    status.montage.as_ref().and_then(|m| m.short.clone())
                } else {
                    Some(status.export.clone())
                };
                let l = self.locale.language;
                match available {
                    Some(a) if a.enabled => {}
                    Some(a) => {
                        let mut refusal = refusal("app.export_unavailable", &a.reason.render(l));
                        refusal.details = Some(serde_json::json!({ "reason": a.reason.key }));
                        return Err(refusal);
                    }
                    None => return refused("No short: set a range first."),
                }
                let conflict = status
                    .montage
                    .as_ref()
                    .is_some_and(|m| if short { m.short_conflict } else { m.conflict });
                if conflict {
                    return Err(refusal(
                        "app.output_exists",
                        "The destination is taken; nothing is ever overwritten. Choose another (set_destination).",
                    ));
                }
                self.start_project(ctx, false, short);
                if self.busy() {
                    if let Some(d) = &mut self.driven {
                        d.agent_job = true;
                        d.agent_cancelled = false;
                    }
                    self.driven_mark();
                    Ok(serde_json::json!({ "started": true }))
                } else {
                    let why = self
                        .notice
                        .as_ref()
                        .or(self.error.as_ref())
                        .map(|m| m.render(l))
                        .unwrap_or_else(|| self.status().headline.render(l));
                    Err(refusal("app.not_started", &why))
                }
            }
            Action::Cancel {} => {
                if let Some(d) = &mut self.driven {
                    d.agent_cancelled = true;
                }
                if self.cancel_work() {
                    self.driven_mark();
                    Ok(serde_json::Value::Null)
                } else {
                    refused("No job is running.")
                }
            }
            Action::SetScene { path } => self.driven_set_scene(path),
            Action::Seek { .. }
            | Action::Play {}
            | Action::Pause {}
            | Action::ShowSceneLook { .. } => {
                unreachable!("playhead gestures are queued")
            }
        }
    }

    fn driven_answer_question(
        &mut self,
        ctx: &egui::Context,
        answer: Answer,
        path: Option<PathBuf>,
    ) -> Result<serde_json::Value, Refusal> {
        let question = self.open_question();
        let wrong = |q: Option<&str>, why: &str| {
            let mut refusal = refusal("app.no_such_question", why);
            refusal.details = Some(serde_json::json!({ "question": q }));
            Err(refusal)
        };
        if answer == Answer::WorkOnCopy {
            let fine = path.as_ref().is_some_and(|p| {
                p.is_absolute()
                    && !p.exists()
                    && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt"))
            });
            if !fine {
                return Err(refusal(
                    "app.output_invalid",
                    "work_on_copy needs a free absolute path ending in .srt.",
                ));
            }
        }
        match (question, answer) {
            (Some("lyrics_truth"), Answer::KeepLyrics) => self.answer_lyrics(),
            (Some("lyrics_truth"), Answer::KeepSrt) => self.answer_srt(),
            (Some("lyrics_truth"), Answer::Later) => self.close_lyrics_question(),
            (Some("lyrics_truth"), _) => {
                return wrong(
                    question,
                    "lyrics_truth takes keep_lyrics, keep_srt or later.",
                );
            }
            (Some("close_while_working"), Answer::Cancel) => self.close_question = false,
            (Some("close_while_working"), Answer::Discard) => self.stop_and_quit(ctx),
            (Some("close_while_working"), _) => {
                return wrong(question, "close_while_working takes cancel or discard.");
            }
            (Some(_), _) => {
                if let Err(why) = self.answer_lyrics_question(answer, path, ctx) {
                    return wrong(question, why);
                }
            }
            (None, _) => return wrong(None, "No question is open."),
        }
        self.driven_mark();
        Ok(serde_json::json!({ "question": self.open_question() }))
    }

    /// The applied effect's preset and settings, Classique's size and
    /// placement, « afficher sur la vidéo »: as the style panel does them.
    fn driven_effect_settings(
        &mut self,
        preset: Option<String>,
        values: std::collections::BTreeMap<String, serde_json::Value>,
        size: Option<noh::captions::CaptionSize>,
        placement: Option<noh::captions::CaptionPlacement>,
        show_on_video: Option<bool>,
    ) -> Result<serde_json::Value, Refusal> {
        let refused = |message: String| Err(refusal("app.refused", &message));
        if show_on_video == Some(true) && !self.has_lyrics() {
            return refused("Captions can be shown once there are lyrics.".into());
        }
        let before = (self.project.apply_subtitles, self.project.caption_style);
        if preset.is_some() || !values.is_empty() {
            let Some(choice) = self.project.effect.clone() else {
                return refused("No effect is chosen (choose_effect).".into());
            };
            let Some(effect) = self.effect_of(&choice) else {
                return refused(format!("The effect {} is not available.", choice.id));
            };
            let mut next = choice.clone();
            if let Some(preset) = preset {
                if !effect.presets.iter().any(|(id, _)| *id == preset) {
                    let ids: Vec<&str> = effect.presets.iter().map(|(id, _)| id.as_str()).collect();
                    return refused(format!("No preset {preset}; presets: {}.", ids.join(", ")));
                }
                next.preset = preset;
                next.values.clear();
            }
            for (key, value) in values {
                let Some(p) = effect.params.iter().find(|p| p.id == key) else {
                    let ids: Vec<&str> = effect.params.iter().map(|p| p.id.as_str()).collect();
                    return refused(format!("No setting {key}; settings: {}.", ids.join(", ")));
                };
                if !p.accepts(&value) {
                    return refused(format!("{value} is not a value {key} takes."));
                }
                next.values.insert(key, value);
            }
            if next != choice {
                self.choose_effect(Some(next));
            }
        }
        if let Some(size) = size {
            self.project.caption_style.size = size;
        }
        if let Some(placement) = placement {
            self.project.caption_style.placement = placement;
        }
        if let Some(on) = show_on_video {
            self.project.apply_subtitles = on;
            if !on {
                self.end_try();
            }
        }
        self.after_style_change(before);
        self.driven_mark();
        Ok(serde_json::Value::Null)
    }

    /// Where the person is (note §4.2).
    pub(super) fn user_position(&self) -> serde_json::Value {
        let ms = self.mini_preview.cursor_ms;
        let visuals = self.project_visuals();
        let total: f64 = visuals.iter().map(|v| v.seconds.max(0.0)).sum();
        let clip = (total > 0.0)
            .then(|| {
                let mut at = (ms as f64 / 1000.0) % total;
                visuals.iter().find(|v| {
                    if at < v.seconds {
                        true
                    } else {
                        at -= v.seconds;
                        false
                    }
                })
            })
            .flatten()
            .and_then(|v| {
                self.clips.get(v.source_index).map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "letter": v.label,
                        "file": c.item.path().file_name().map(|f| f.to_string_lossy().into_owned()),
                    })
                })
            });
        let line = self.lyrics_track().and_then(|t| {
            t.cues
                .iter()
                .position(|c| c.start_ms <= ms && ms < c.end_ms)
                .map(|i| serde_json::json!({ "index": i, "text": t.cues[i].text }))
        });
        let d = self.driven.as_ref();
        let selection = d.and_then(|d| d.selection).map(|s| match s {
            super::app_driven::Selection::Clip(id) => serde_json::json!({ "clip": id }),
            super::app_driven::Selection::Line(i) => serde_json::json!({ "line": i }),
        });
        let navigating = self.person_navigating().is_some();
        serde_json::json!({
            "ms": ms,
            "playing": self.mini_playing(),
            "view": if self.mini_preview.short { "short" } else { "montage" },
            "clip": clip,
            "line": line,
            "selection": selection,
            "navigating": navigating,
            "navigating_for_ms": d
                .and_then(|d| d.nav_at)
                .filter(|_| navigating)
                .map(|at| at.elapsed().as_millis() as u64),
        })
    }

    /// The state the agent reads, without the position (sent at each change).
    fn driven_core(&self, ctx: &egui::Context, lines: usize) -> serde_json::Value {
        let l = self.locale.language;
        let clips: Vec<_> = self
            .project_visuals()
            .iter()
            .filter_map(|v| self.clips.get(v.source_index).map(|c| (v.label.clone(), c)))
            .map(|(letter, c)| {
                serde_json::json!({
                    "id": c.id,
                    "letter": letter,
                    "path": c.item.path(),
                    "seconds": c.info.as_ref().map(|i| i.seconds),
                    "error": c.error.as_ref().map(|e| e.render(l)),
                })
            })
            .collect();
        let unreadable: Vec<_> = self
            .clips
            .iter()
            .filter(|c| c.error.is_some() || c.info.is_none())
            .map(|c| serde_json::json!({ "id": c.id, "path": c.item.path(), "error": c.error.as_ref().map(|e| e.render(l)) }))
            .collect();
        let track = self.lyrics_track();
        let shown = track.as_ref().map_or(0, |t| t.cues.len());
        let lyrics = self.project.track.path.as_ref().map(|path| {
            serde_json::json!({
                "path": path,
                "revision": self.project.track.revision,
                "count": shown,
                "unsaved": self.has_unsaved_lyrics(),
                "locked": self.lyrics_locked(),
                "lines": track.as_ref().map(|t| t.cues.iter().take(lines).enumerate().map(|(i, c)| {
                    serde_json::json!({
                        "index": i,
                        "start_ms": c.start_ms,
                        "end_ms": c.end_ms,
                        "text": c.text.chars().take(LINE_TEXT).collect::<String>(),
                        "voices": c.voices,
                    })
                }).collect::<Vec<_>>()),
                "voices": track.as_ref().map(|t| t.voices.iter().map(|v| v.name.clone()).collect::<Vec<_>>()),
                "lines_truncated": shown > lines,
            })
        });
        let status = self.status();
        let effect = self
            .project
            .effect
            .as_ref()
            .map(|c| serde_json::json!({ "id": c.id, "preset": c.preset }));
        serde_json::json!({
            "clips": clips,
            "unreadable": unreadable,
            "song": self.wav.as_ref().map(|w| serde_json::json!({
                "path": w,
                "seconds": self.wav_seconds,
            })),
            "duration_ms": self.project.duration_ms,
            "lyrics": lyrics,
            "effect": effect,
            "scene": self.driven_scene_state(),
            "captions_on_video": self.project.apply_subtitles,
            "safe_area": self.project.short_safe_area,
            "range": self.project.range.map(|r| serde_json::json!({ "start_ms": r.start_ms, "end_ms": r.end_ms })),
            "output": self.output_folder.join(&self.output_name),
            "export": {
                "ready": status.export.enabled,
                "reason": status.export.reason.render(l),
            },
            "status": status.headline.render(l),
            "job": self.busy(),
            "question": self.open_question(),
            "notice": self.notice.as_ref().map(|m| m.render(l)),
            "minimized": ctx.input(|i| i.viewport().minimized) == Some(true),
        })
    }

    /// Send the state if it changed (`force`: every time, for a `done`).
    /// Position-only changes go out 4 times a second at most. Returns the
    /// revision now.
    pub(super) fn driven_state(&mut self, force: bool) -> u64 {
        let Some(ctx) = self.driven.as_ref().map(|d| d.ctx.clone()) else {
            return 0;
        };
        let mut lines = LINES;
        let (core, core_text) = loop {
            let core = self.driven_core(&ctx, lines);
            let text = core.to_string();
            if text.len() <= STATE_BYTES || lines == 0 {
                break (core, text);
            }
            lines /= 2;
        };
        let position = self.user_position();
        let position_text = position.to_string();
        let Some(d) = &mut self.driven else {
            return 0;
        };
        let core_changed = core_text != d.last_core;
        let position_due = position_text != d.last_position
            && d.last_sent.is_none_or(|at| at.elapsed() >= POSITION_EVERY);
        if !(force || core_changed || position_due) {
            if position_text != d.last_position {
                d.ctx.request_repaint_after(POSITION_EVERY);
            }
            return d.revision;
        }
        d.revision += 1;
        d.last_core = core_text;
        d.last_position = position_text;
        d.last_sent = Some(Instant::now());
        let mut state = core;
        state["user_position"] = position;
        state["piloting"] = if d.locked() { "driving" } else { "released" }.into();
        let _ = d.send_message(&ToServer::State {
            revision: d.revision,
            state,
        });
        d.revision
    }

    /// A driven window waits for its preparation turn: its light copies and
    /// its waveform wait too (note §5). Not once the person took it back or
    /// the agent left: it is then an ordinary NOH. And not once nothing is
    /// left to prepare: the turn is given back then, and a window held after
    /// it showed no paused picture until something was played.
    pub(super) fn driven_holds_preparation(&self) -> bool {
        self.driven.as_ref().is_some_and(|d| d.locked() && !d.turn) && self.preparation_pending()
    }

    /// Copies being made or waiting, or a waveform to decode.
    fn preparation_pending(&self) -> bool {
        let copies = self.clips.iter().any(|c| {
            !c.item.is_image()
                && c.info.is_some()
                && c.error.is_none()
                && noh::copies::state(c.item.path()) == noh::copies::State::Preparing
        });
        // The project's check, wanted or running, is part of the preparation.
        // A check is pending when one would start for the project as it is
        // now: not the one already asked for, which checked, failed or was
        // cancelled (Preflight::update starts nothing for it again).
        let check = self.inspecting
            || (!self.busy() && !self.diagnosis_current() && {
                let request = self.diagnostic_request();
                request.is_some() && !self.preflight.as_ref().is_some_and(|p| p.asked(&request))
            });
        copies || check || self.project.waveform_loading || self.project.waveform_wanted()
    }

    /// Each frame: ask for the turn when there is something to prepare; give
    /// it back when there is nothing left.
    pub(super) fn driven_turn_frame(&mut self) {
        let pending = self.preparation_pending();
        let urgent = self.mini_playing();
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.locked() {
            return;
        }
        if d.turn && !pending {
            d.turn = false;
            let _ = d.send_message(&ToServer::Prepared);
        } else if !d.turn && pending && d.asked_turn != Some(urgent) {
            // Asked again only to say the person now plays it.
            if d.asked_turn.is_none() || urgent {
                d.asked_turn = Some(urgent);
                let _ = d.send_message(&ToServer::NeedsPrepare { urgent });
            }
        }
    }

    /// Each frame: the end of a heavy job the agent started here gives the
    /// server's slot back, even after the person took the window back.
    pub(super) fn driven_job_frame(&mut self) {
        let busy = self.busy();
        let outcome = if self.project.failure.is_some() || self.error.is_some() {
            "failed"
        } else if self.driven.as_ref().is_some_and(|d| d.agent_cancelled) {
            "cancelled"
        } else {
            "succeeded"
        };
        let Some(d) = &mut self.driven else {
            return;
        };
        if d.agent_job && !busy {
            d.agent_job = false;
            d.agent_cancelled = false;
            let _ = d.send_message(&ToServer::JobDone {
                outcome: outcome.into(),
            });
        }
    }

    /// The monitor as painted this frame, and whether its picture is the one
    /// at the playhead (for `look`).
    pub(super) fn driven_note_monitor(&mut self, rect: Rect, settled: bool) {
        if let Some(d) = &mut self.driven {
            d.monitor = Some((rect, settled));
        }
    }
}

impl Driven {
    pub(super) fn send_message(&self, message: &ToServer) -> Result<(), noh::driven::Unsent> {
        self.outbox_send(message)
    }
}

/// Write `image` (cropped to `crop`, in points, when given) as a PNG at
/// `output`, never over an existing file.
fn write_png(
    image: &egui::ColorImage,
    crop: Option<Rect>,
    scale: f32,
    output: &std::path::Path,
) -> Result<(u32, u32), Refusal> {
    let [width, height] = image.size;
    let (x0, y0, x1, y1) = match crop {
        Some(r) => (
            ((r.left() * scale).round().max(0.0) as usize).min(width),
            ((r.top() * scale).round().max(0.0) as usize).min(height),
            ((r.right() * scale).round().max(0.0) as usize).min(width),
            ((r.bottom() * scale).round().max(0.0) as usize).min(height),
        ),
        None => (0, 0, width, height),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err(Refusal::new("app.look_failed", "Nothing to capture there."));
    }
    let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        for p in &image.pixels[y * width + x0..y * width + x1] {
            rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
        }
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let failed = |e: String| {
        let mut refusal = Refusal::new("app.look_failed", "The picture could not be written.");
        refusal.details = Some(serde_json::json!({ "technical": e }));
        refusal
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| failed(e.to_string()))?;
    let written = (|| {
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(output);
        return Err(failed(e));
    }
    Ok((w, h))
}
