//! A NOH window the agent drives (`noh-app --driven`, opened by `noh-mcp`):
//! the agent acts, the person watches and moves around (agent-follow note
//! §4). The channel is the window's standard input and output
//! (`noh::driven`); it is served from `advance`, so a minimized window
//! answers too.
//!
//! While driven, the person's editing gestures are taken out before the
//! interface sees them: an allow-list, so a gesture added to NOH later is
//! blocked until it is listed. What passes: the window itself (the system's),
//! navigation (transport, view, volume, zoom, the ruler for the playhead),
//! designation (a click on a clip or a lyrics line, never a drag), the wheel,
//! and the badge with « Arrêter le pilotage ». No key reaches the content.
//! Elsewhere a press only shows « Demandez à l'agent ».
use super::{NohApp, app_icons, app_style as style};
use eframe::egui::{self, Align2, Pos2, Rect, vec2};
use noh::driven::{self, Inbound, Outbox, Refusal, ToApp, ToServer};
use std::{
    io::{Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long the « Demandez à l'agent » bubble stays.
const BUBBLE: Duration = Duration::from_millis(2500);
/// How long the badge says the piloting ended before it goes.
const FAREWELL: Duration = Duration::from_secs(4);

/// What the person may reach in a driven window, by the rectangle drawn last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Zone {
    /// Clicks and drags pass: volume, zoom, overview.
    Navigate,
    /// The transport group and the view toggle: they pass, and the person
    /// then navigates (the playhead is theirs for a while).
    Transport,
    /// The ruler: a click or a drag moves the playhead.
    Seek,
    /// A clip or a lyrics line: a press and its release become one click.
    Designate,
    /// The badge: always passes, a question open or not.
    Badge,
}

/// The pointer button held down, and what its press was.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    Pass,
    Designate {
        pos: Pos2,
        modifiers: egui::Modifiers,
    },
    Blocked,
}

/// The input filter (`raw_input_hook`), built from the previous drawn frame.
#[derive(Default)]
pub(super) struct Gate {
    drawing: Vec<(Rect, Zone)>,
    drawing_denied: Vec<Rect>,
    zones: Vec<(Rect, Zone)>,
    denied: Vec<Rect>,
    held: Option<Held>,
    /// A question of the application is open: it is the agent's.
    question_open: bool,
    /// A press on the playhead's controls passed since this was last taken.
    pub navigated: bool,
}

impl Gate {
    fn begin(&mut self) {
        self.drawing.clear();
        self.drawing_denied.clear();
    }
    fn finish(&mut self) {
        std::mem::swap(&mut self.zones, &mut self.drawing);
        std::mem::swap(&mut self.denied, &mut self.drawing_denied);
    }
    /// The zone under `pos`: a denied rectangle wins, then the latest drawn.
    fn zone_at(&self, pos: Pos2) -> Option<Zone> {
        if self.denied.iter().any(|r| r.contains(pos)) {
            return None;
        }
        self.zones
            .iter()
            .rev()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, zone)| *zone)
    }

    /// Keep only what the person may do. Returns where a blocked press was.
    pub(super) fn filter(&mut self, input: &mut egui::RawInput) -> Option<Pos2> {
        use egui::Event;
        input.dropped_files.clear();
        input.hovered_files.clear();
        let mut blocked = None;
        let mut kept = Vec::with_capacity(input.events.len());
        for event in std::mem::take(&mut input.events) {
            match event {
                Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers,
                } => {
                    if self.held.is_some() {
                        // A second button while one is held.
                        continue;
                    }
                    let zone = self.zone_at(pos);
                    let primary = button == egui::PointerButton::Primary;
                    self.held = Some(match zone {
                        Some(Zone::Badge) if primary => Held::Pass,
                        _ if self.question_open || !primary => Held::Blocked,
                        Some(Zone::Transport | Zone::Seek) => {
                            self.navigated = true;
                            Held::Pass
                        }
                        Some(Zone::Navigate) => Held::Pass,
                        Some(Zone::Designate) => Held::Designate { pos, modifiers },
                        _ => Held::Blocked,
                    });
                    match self.held {
                        Some(Held::Pass) => kept.push(event),
                        Some(Held::Blocked) => blocked = Some(pos),
                        _ => {}
                    }
                }
                Event::PointerButton {
                    button,
                    pressed: false,
                    ..
                } => match self.held.take() {
                    Some(Held::Pass) => kept.push(event),
                    Some(Held::Designate { pos, modifiers }) => {
                        // One click where it was pressed, whatever happened
                        // in between: never a drag, never a long press.
                        kept.push(Event::PointerMoved(pos));
                        for pressed in [true, false] {
                            kept.push(Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                modifiers,
                            });
                        }
                    }
                    Some(Held::Blocked) | None => {}
                },
                Event::PointerMoved(_) | Event::MouseMoved(_) => {
                    if matches!(self.held, None | Some(Held::Pass)) {
                        kept.push(event);
                    }
                }
                Event::PointerGone => {
                    if matches!(self.held, Some(Held::Designate { .. } | Held::Blocked)) {
                        self.held = None;
                    }
                    kept.push(event);
                }
                // The wheel scrolls the page and zooms or pans the timeline;
                // nothing it reaches edits.
                Event::MouseWheel { .. }
                | Event::Zoom(_)
                | Event::WindowFocused(_)
                | Event::Screenshot { .. } => kept.push(event),
                // Keys, text, clipboard, IME, touch, accessibility actions,
                // and whatever egui adds later.
                _ => {}
            }
        }
        input.events = kept;
        blocked
    }
}

/// Where the piloting stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Driving,
    /// The person stopped the piloting.
    Released(Instant),
    /// The agent's session ended (`cut`: the link broke without a word).
    Ended {
        at: Instant,
        cut: bool,
    },
}

/// What the person designated, for the agent to know what « this one » is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    Clip(u64),
    Line(usize),
}

enum Incoming {
    Message(Inbound),
    Disconnected,
}

pub(super) struct Driven {
    pub title: String,
    pub phase: Phase,
    pub gate: Gate,
    pub selection: Option<Selection>,
    bubble: Option<(Pos2, Instant)>,
    inbox: mpsc::Receiver<Incoming>,
    outbox: Outbox,
    ready_sent: bool,
    /// The agent asked to close: the next close request is its own.
    closing_by_agent: bool,
    pub ctx: egui::Context,
    /// The revision of the last `state` sent.
    pub revision: u64,
    pub last_core: String,
    pub last_position: String,
    pub last_sent: Option<Instant>,
    /// The person's last navigation gesture, and who moved the playhead last.
    pub nav_at: Option<Instant>,
    pub controller: super::app_driven_act::Controller,
    /// Playhead gestures waiting while the person navigates.
    pub pending: Vec<super::app_driven_act::Pending>,
    /// The monitor painted last, and whether its picture is the playhead's.
    pub monitor: Option<(Rect, bool)>,
    pub look: Option<super::app_driven_act::LookJob>,
    /// When the agent last changed something (the badge marks it).
    pub acted_at: Option<Instant>,
}

impl Driven {
    /// The channel over `input` and `output`; `ctx` is woken for each line.
    pub(super) fn new(
        title: &str,
        input: impl Read + Send + 'static,
        output: impl Write + Send + 'static,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, inbox) = mpsc::channel();
        let (line_tx, end_tx) = (tx.clone(), tx);
        let (line_ctx, end_ctx) = (ctx.clone(), ctx.clone());
        driven::spawn_reader(
            "driven-reader",
            input,
            driven::MAX_TO_APP,
            move |line| {
                if let Some(message) = driven::parse_to_app(&line) {
                    let _ = line_tx.send(Incoming::Message(message));
                    line_ctx.request_repaint();
                }
            },
            move || {
                let _ = end_tx.send(Incoming::Disconnected);
                end_ctx.request_repaint();
            },
        );
        Self {
            title: driven::clean_title(title),
            phase: Phase::Driving,
            gate: Gate::default(),
            selection: None,
            bubble: None,
            inbox,
            outbox: Outbox::spawn("driven-writer", output, 64),
            ready_sent: false,
            closing_by_agent: false,
            ctx: ctx.clone(),
            revision: 0,
            last_core: String::new(),
            last_position: String::new(),
            last_sent: None,
            nav_at: None,
            controller: Default::default(),
            pending: Vec::new(),
            monitor: None,
            look: None,
            acted_at: None,
        }
    }

    /// The person's gestures are filtered.
    pub(super) fn locked(&self) -> bool {
        self.phase == Phase::Driving
    }

    fn send(&self, message: &ToServer) {
        // The server may be gone: the window carries on regardless.
        let _ = self.outbox.send(message);
    }

    pub(super) fn outbox_send(&self, message: &ToServer) -> Result<(), noh::driven::Unsent> {
        self.outbox.send(message)
    }
}

/// `--driven [--title T]` as the first arguments: the title, else nothing.
pub(super) fn arguments(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<String> {
    args.next();
    if args.next()? != driven::ARGUMENT {
        return None;
    }
    let mut title = String::new();
    if args.next().is_some_and(|a| a == driven::TITLE_ARGUMENT)
        && let Some(value) = args.next()
    {
        title = value.to_string_lossy().into_owned();
    }
    Some(title)
}

impl NohApp {
    /// Each frame, drawn or not: the agent's messages, and the person's close.
    pub(super) fn driven_frame(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.ready_sent {
            d.ready_sent = true;
            d.send(&ToServer::Ready {
                pid: std::process::id(),
                version: noh::build_info::VERSION.into(),
            });
        }
        // The person closes the window: the piloting stops first, then the
        // ordinary close follows, its questions theirs.
        if d.locked() && !d.closing_by_agent && ctx.input(|i| i.viewport().close_requested()) {
            self.release_piloting(ctx);
        }
        let Some(d) = &mut self.driven else {
            return;
        };
        let incoming: Vec<Incoming> = d.inbox.try_iter().collect();
        // A stalled look ends before a new one is asked for.
        self.driven_look_expire();
        for message in incoming {
            match message {
                Incoming::Message(Inbound::Message(ToApp::Close { id })) => {
                    let answer = self.agent_close(ctx);
                    if let Some(d) = &self.driven {
                        let (result, error) = match answer {
                            Ok(result) => (Some(result), None),
                            Err(refusal) => (None, Some(refusal)),
                        };
                        d.send(&ToServer::Done { id, result, error });
                    }
                }
                Incoming::Message(Inbound::Message(ToApp::SessionEnd)) => {
                    self.end_piloting(ctx, false);
                }
                Incoming::Message(Inbound::Unsupported { id: Some(id) }) => {
                    if let Some(d) = &self.driven {
                        d.send(&ToServer::Done {
                            id,
                            result: None,
                            error: Some(Refusal::new(
                                "app.unsupported",
                                "This NOH window does not serve that request.",
                            )),
                        });
                    }
                }
                Incoming::Message(Inbound::Message(ToApp::Act { id, action })) => {
                    self.driven_act(ctx, id, action);
                }
                Incoming::Message(Inbound::Message(ToApp::Look {
                    id,
                    scope,
                    at_ms,
                    output,
                })) => self.driven_look(ctx, id, scope, at_ms, output),
                Incoming::Message(Inbound::Unsupported { id: None }) => {}
                Incoming::Disconnected => self.end_piloting(ctx, true),
            }
        }
        // The agent's playhead gestures waiting for the person; then what the
        // window shows, if it changed.
        self.driven_pending(ctx);
        self.driven_state(false);
        let Some(d) = &mut self.driven else {
            return;
        };
        let now = Instant::now();
        if let Some((_, at)) = d.bubble {
            match (at + BUBBLE).checked_duration_since(now) {
                Some(left) => ctx.request_repaint_after(left),
                None => d.bubble = None,
            }
        }
        if let Phase::Released(at) | Phase::Ended { at, .. } = d.phase
            && let Some(left) = (at + FAREWELL).checked_duration_since(now)
        {
            ctx.request_repaint_after(left);
        }
    }

    /// « Arrêter le pilotage », or a close by the person: the agent can do
    /// nothing more here, and the window is theirs.
    pub(super) fn release_piloting(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.locked() {
            return;
        }
        d.phase = Phase::Released(Instant::now());
        d.gate.held = None;
        d.send(&ToServer::Released);
        self.driven_drop_requests();
        self.driven_title(ctx);
    }

    /// The agent's requests still waiting are refused: the window is no
    /// longer the agent's.
    fn driven_drop_requests(&mut self) {
        let Some(d) = &mut self.driven else {
            return;
        };
        let ids: Vec<u64> = d
            .pending
            .drain(..)
            .map(|p| p.id)
            .chain(d.look.take().map(|j| j.id))
            .collect();
        for id in ids {
            d.send(&ToServer::Done {
                id,
                result: None,
                error: Some(Refusal::new(
                    "app.released_by_user",
                    "The person stopped the piloting: this window is theirs now.",
                )),
            });
        }
        self.driven_state(true);
    }

    /// The agent's session ended, said or not: the window is an ordinary NOH.
    fn end_piloting(&mut self, ctx: &egui::Context, cut: bool) {
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.locked() {
            return;
        }
        d.phase = Phase::Ended {
            at: Instant::now(),
            cut,
        };
        d.gate.held = None;
        d.pending.clear();
        d.look = None;
        self.driven_title(ctx);
    }

    /// The window title says the window is driven, until it is not.
    pub(super) fn driven_title(&self, ctx: &egui::Context) {
        let l = self.locale.language;
        let title = match &self.driven {
            Some(d) if d.locked() => {
                format!("{} — {}", l.text("title"), l.text("driven.window_title"))
            }
            _ => l.text("title").to_owned(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// The question of the application open now, by kind. Each is the agent's
    /// to answer while it drives (note §4.1).
    pub(super) fn open_question(&self) -> Option<&'static str> {
        if self.lyrics_pair.question.is_some() {
            // Shown or set aside, the difference waits for its answer.
            return Some("lyrics_truth");
        }
        if let Some(kind) = self.lyrics_question_kind() {
            return Some(kind);
        }
        self.close_question.then_some("close_while_working")
    }

    /// `noh_app_close` (note §6): pending lyrics are written first; a
    /// question, a running job or a save that needs a question closes nothing.
    fn agent_close(&mut self, ctx: &egui::Context) -> Result<serde_json::Value, Refusal> {
        if !self.driven.as_ref().is_some_and(Driven::locked) {
            return Err(Refusal::new(
                "app.released_by_user",
                "The person stopped the piloting: this window is theirs now.",
            ));
        }
        let question = |kind: &str| {
            let mut refusal = Refusal::new(
                "app.question_open",
                "A question is open in the window: nothing was closed. Answer it, then close again.",
            );
            refusal.details = Some(serde_json::json!({ "question": kind }));
            refusal
        };
        if let Some(kind) = self.open_question() {
            return Err(question(kind));
        }
        if self.busy() {
            return Err(Refusal::new(
                "app.job_running",
                "A job is running in the window: nothing was closed. Cancel it or wait for its end.",
            ));
        }
        if self.has_unsaved_lyrics() && !self.flush_lyrics() {
            // Not allowed to write yet: the save asks its question.
            self.save_lyrics();
            return Err(question(self.open_question().unwrap_or("first_edit")));
        }
        if let Some(kind) = self.open_question() {
            // The write failed and says so.
            return Err(question(kind));
        }
        if let Some(d) = &mut self.driven {
            d.closing_by_agent = true;
        }
        self.close_now = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        Ok(serde_json::json!({ "state": "closed" }))
    }

    /// The window is closing for good: a close by the person is reported, and
    /// the last lines are given a moment to leave.
    pub(super) fn driven_exit(&mut self) {
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.closing_by_agent {
            d.send(&ToServer::Closed);
        }
        d.outbox.close(Duration::from_millis(300));
    }

    /// A rectangle the person may reach in a driven window this frame.
    pub(super) fn driven_allow(&mut self, rect: Rect, zone: Zone) {
        if let Some(d) = &mut self.driven
            && d.locked()
            && rect.is_positive()
        {
            d.gate.drawing.push((rect, zone));
        }
    }

    /// A rectangle inside an allowed one that the person may not reach.
    pub(super) fn driven_deny(&mut self, rect: Rect) {
        if let Some(d) = &mut self.driven
            && d.locked()
        {
            d.gate.drawing_denied.push(rect);
        }
    }

    pub(super) fn driven_locked(&self) -> bool {
        self.driven.as_ref().is_some_and(Driven::locked)
    }

    /// The person designated something.
    pub(super) fn driven_select(&mut self, selection: Selection) {
        if let Some(d) = &mut self.driven
            && d.locked()
        {
            d.selection = Some(selection);
        }
    }

    /// `raw_input_hook`, while driven.
    pub(super) fn driven_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.driven_screenshot(ctx, input);
        let question_open = self.open_question().is_some();
        let Some(d) = &mut self.driven else {
            return;
        };
        if !d.locked() {
            return;
        }
        d.gate.question_open = question_open;
        if let Some(pos) = d.gate.filter(input) {
            d.bubble = Some((pos, Instant::now()));
        }
        if std::mem::take(&mut d.gate.navigated) {
            d.nav_at = Some(Instant::now());
            d.controller = super::app_driven_act::Controller::Person;
        }
    }

    /// At the start of a drawn frame.
    pub(super) fn driven_begin(&mut self) {
        if let Some(d) = &mut self.driven {
            d.gate.begin();
        }
    }

    /// At the end of a drawn frame: the badge and the bubble, over
    /// everything (a question included), then this frame's zones for the
    /// next input.
    pub(super) fn driven_overlay(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        let Some(d) = &self.driven else {
            return;
        };
        let now = Instant::now();
        let (text, stop) = match d.phase {
            Phase::Driving => (
                l.format("driven.badge", std::slice::from_ref(&d.title)),
                true,
            ),
            Phase::Released(at) if now < at + FAREWELL => (l.text("driven.released").into(), false),
            Phase::Ended { at, cut } if now < at + FAREWELL => (
                l.text(if cut { "driven.cut" } else { "driven.ended" })
                    .into(),
                false,
            ),
            _ => (String::new(), false),
        };
        let bubble = d.bubble;
        // A brief mark on the badge at each change the agent makes.
        let marked = d
            .acted_at
            .is_some_and(|at| now < at + Duration::from_millis(600));
        if marked {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut stop_clicked = false;
        let mut badge_rect = None;
        if !text.is_empty() {
            // Above `egui::Modal` (Foreground): « Arrêter le pilotage » stays
            // reachable while a question is open.
            let area = egui::Area::new(egui::Id::new("driven-badge"))
                .order(egui::Order::Tooltip)
                .anchor(Align2::CENTER_TOP, vec2(0.0, 8.0))
                .interactable(true)
                .show(ctx, |ui| {
                    let t = style::tokens(ui);
                    egui::Frame::new()
                        .fill(t.surface_2)
                        .stroke(egui::Stroke::new(if marked { 3.0 } else { 1.5 }, t.focus))
                        .corner_radius(style::radius::M)
                        .inner_margin(egui::Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                app_icons::paint(
                                    ui.painter(),
                                    Rect::from_center_size(
                                        ui.cursor().min + vec2(8.0, 10.0),
                                        vec2(14.0, 14.0),
                                    ),
                                    app_icons::Icon::Play,
                                    t.focus,
                                );
                                ui.add_space(18.0);
                                ui.label(egui::RichText::new(&text).color(t.text));
                                if stop {
                                    stop_clicked =
                                        style::button(ui, l.text("driven.stop"), true, false, 0.0)
                                            .on_hover_text(l.text("driven.stop_hint"))
                                            .clicked();
                                }
                            });
                        });
                });
            badge_rect = Some(area.response.rect);
        }
        if let Some((pos, at)) = bubble
            && now < at + BUBBLE
        {
            egui::Area::new(egui::Id::new("driven-ask"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + vec2(12.0, 12.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(l.text("driven.ask"));
                    });
                });
        }
        if let Some(rect) = badge_rect {
            self.driven_allow(rect, Zone::Badge);
        }
        if stop_clicked {
            self.release_piloting(ctx);
        }
        if let Some(d) = &mut self.driven {
            d.gate.finish();
        }
        self.driven_look_frame(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, PointerButton};

    fn press(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }
    fn gate() -> Gate {
        let mut gate = Gate::default();
        gate.begin();
        gate.drawing.push((
            Rect::from_min_size(Pos2::ZERO, vec2(100.0, 20.0)),
            Zone::Seek,
        ));
        gate.drawing.push((
            Rect::from_min_size(Pos2::new(0.0, 40.0), vec2(100.0, 40.0)),
            Zone::Designate,
        ));
        gate.drawing.push((
            Rect::from_min_size(Pos2::new(200.0, 0.0), vec2(80.0, 20.0)),
            Zone::Badge,
        ));
        gate.drawing_denied
            .push(Rect::from_min_size(Pos2::new(80.0, 40.0), vec2(20.0, 20.0)));
        gate.finish();
        gate
    }
    fn filtered(gate: &mut Gate, events: Vec<Event>) -> (Vec<Event>, Option<Pos2>) {
        let mut input = egui::RawInput {
            events,
            ..Default::default()
        };
        let blocked = gate.filter(&mut input);
        (input.events, blocked)
    }

    #[test]
    fn keys_text_and_files_never_pass() {
        let mut g = gate();
        let mut input = egui::RawInput {
            events: vec![
                Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                },
                Event::Text(" ".into()),
                Event::Paste("x".into()),
                Event::Copy,
            ],
            ..Default::default()
        };
        g.filter(&mut input);
        assert!(input.events.is_empty());
    }

    #[test]
    fn the_ruler_takes_a_drag_and_a_designation_becomes_one_click() {
        let mut g = gate();
        let at = Pos2::new(10.0, 10.0);
        let (events, blocked) = filtered(
            &mut g,
            vec![
                press(at, true),
                Event::PointerMoved(Pos2::new(60.0, 12.0)),
                press(Pos2::new(60.0, 12.0), false),
            ],
        );
        assert_eq!(events.len(), 3);
        assert!(blocked.is_none());
        // On a clip: the drag's moves vanish, the release lands where the press was.
        let on = Pos2::new(20.0, 50.0);
        let (events, _) = filtered(
            &mut g,
            vec![press(on, true), Event::PointerMoved(Pos2::new(90.0, 70.0))],
        );
        assert!(events.is_empty());
        let (events, _) = filtered(&mut g, vec![press(Pos2::new(90.0, 70.0), false)]);
        assert_eq!(
            events,
            vec![Event::PointerMoved(on), press(on, true), press(on, false)]
        );
    }

    #[test]
    fn elsewhere_a_press_is_blocked_and_shows_where() {
        let mut g = gate();
        // Outside every zone, and on the clip's ✕ (denied inside its zone).
        for at in [Pos2::new(150.0, 150.0), Pos2::new(90.0, 45.0)] {
            let (events, blocked) = filtered(
                &mut g,
                vec![
                    press(at, true),
                    Event::PointerMoved(at + vec2(30.0, 0.0)),
                    press(at, false),
                ],
            );
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(blocked, Some(at));
        }
        // The secondary button never passes, even on the ruler.
        let (events, _) = filtered(
            &mut g,
            vec![Event::PointerButton {
                pos: Pos2::new(10.0, 10.0),
                button: PointerButton::Secondary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(events.is_empty());
    }

    #[test]
    fn a_question_open_leaves_only_the_badge() {
        let mut g = gate();
        g.question_open = true;
        let (events, blocked) = filtered(&mut g, vec![press(Pos2::new(10.0, 10.0), true)]);
        assert!(events.is_empty() && blocked.is_some());
        filtered(&mut g, vec![press(Pos2::new(10.0, 10.0), false)]);
        let badge = Pos2::new(220.0, 10.0);
        let (events, _) = filtered(&mut g, vec![press(badge, true), press(badge, false)]);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn the_wheel_and_hovering_pass() {
        let mut g = gate();
        let (events, _) = filtered(
            &mut g,
            vec![
                Event::PointerMoved(Pos2::new(150.0, 150.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -10.0),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
        );
        assert_eq!(events.len(), 2);
    }

    /// A whole driven window, headless: two pictures, a song, lyrics, a range.
    struct Window {
        app: NohApp,
        ctx: egui::Context,
        to_app: Option<std::io::PipeWriter>,
        from_app: mpsc::Receiver<ToServer>,
        /// The `state` lines, apart.
        states: mpsc::Receiver<ToServer>,
        _folder: tempfile::TempDir,
    }

    fn window(driven: bool) -> Window {
        let folder = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        super::super::app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        let mut app = NohApp::default();
        app.locale.language = noh::i18n::Language::En;
        app.wav = Some("song.wav".into());
        app.wav_seconds = Some(20.0);
        let mut info = noh::media::MediaInfo::default();
        info.seconds = 5.0;
        for id in [1, 2] {
            app.clips.push(super::super::Clip {
                id,
                request_id: id,
                item: std::path::PathBuf::from(format!("picture-{id}.png")).into(),
                info: Some(info.clone()),
                error: None,
            });
        }
        app.next_id = 2;
        let srt = "1\n00:00:01,000 --> 00:00:09,000\nFirst test line\n\n2\n00:00:11,000 --> 00:00:19,000\nSecond test line\n\n";
        let path = folder.path().join("lines.srt");
        std::fs::write(&path, srt).unwrap();
        let track = noh::subtitle_srt::parse(srt, 20_000).unwrap();
        app.project.track.path = Some(path.clone());
        app.project.track.loaded = Some(super::super::app_track::Loaded {
            track: std::sync::Arc::new(track),
            snapshot: noh::inspection::Snapshot(vec![
                noh::inspection::FileStamp::read(&path).unwrap(),
            ]),
            model: None,
            lyrics_unwritten: None,
            srt_stamp: noh::inspection::FileStamp::read(&path).ok(),
            lyrics_stamp: None,
            pairing: Default::default(),
        });
        app.project.track.revision = 1;
        app.project.range = noh::timeline::Range::new(5_000, 10_000, 20_000);
        // As the layout tests: no media is read, the duration is the song's.
        app.capture_state = Some("ready".into());
        app.project.duration_ms = 20_000;
        app.resources_checked();
        let (to_app, from_app) = if driven {
            let (app_reads, server_writes) = std::io::pipe().unwrap();
            let (server_reads, app_writes) = std::io::pipe().unwrap();
            app.driven = Some(Driven::new("Test window", app_reads, app_writes, &ctx));
            let (tx, rx) = mpsc::channel();
            let (state_tx, state_rx) = mpsc::channel();
            driven::spawn_reader(
                "test-server",
                server_reads,
                driven::MAX_TO_SERVER,
                move |line| match driven::parse_to_server(&line) {
                    Some(state @ ToServer::State { .. }) => {
                        let _ = state_tx.send(state);
                    }
                    Some(message) => {
                        let _ = tx.send(message);
                    }
                    None => {}
                },
                || {},
            );
            (Some(server_writes), (rx, state_rx))
        } else {
            (None, (mpsc::channel().1, mpsc::channel().1))
        };
        let (from_app, states) = from_app;
        let mut w = Window {
            app,
            ctx,
            to_app,
            from_app,
            states,
            _folder: folder,
        };
        for _ in 0..3 {
            w.frame(vec![]);
        }
        if driven {
            assert!(matches!(w.next(), Some(ToServer::Ready { .. })));
        }
        w
    }

    impl Window {
        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, 850.0))),
                events,
                ..Default::default()
            };
            eframe::App::raw_input_hook(&mut self.app, &self.ctx, &mut input);
            self.app.strip.tiles.clear();
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(input, |ui| {
                app.lyrics_edit_frame(ui.ctx());
                app.draw(ui);
            });
            output.textures_delta.clear();
            output
        }
        fn key(&mut self, key: Key, modifiers: Modifiers) {
            for pressed in [true, false] {
                self.frame(vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                }]);
            }
        }
        fn click(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at), press(at, true)]);
            self.frame(vec![press(at, false)]);
            self.frame(vec![]);
        }
        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.frame(vec![Event::PointerMoved(from), press(from, true)]);
            for step in 1..=8 {
                let at = from + (to - from) * (step as f32 / 8.0);
                self.frame(vec![Event::PointerMoved(at)]);
            }
            self.frame(vec![press(to, false)]);
            self.frame(vec![]);
        }
        fn send(&mut self, message: &ToApp) {
            let pipe = self.to_app.as_mut().unwrap();
            pipe.write_all(&driven::line(message)).unwrap();
            pipe.flush().unwrap();
        }
        /// The next message from the window, frames running meanwhile.
        fn next(&mut self) -> Option<ToServer> {
            self.next_within(Duration::from_secs(5))
        }
        fn next_within(&mut self, limit: Duration) -> Option<ToServer> {
            let deadline = Instant::now() + limit;
            loop {
                self.frame(vec![]);
                if let Ok(message) = self.from_app.recv_timeout(Duration::from_millis(20)) {
                    return Some(message);
                }
                if Instant::now() > deadline {
                    return None;
                }
            }
        }
        fn ruler(&mut self) -> Rect {
            self.app.project_ui.timeline.driven_zones().0.unwrap()
        }
        fn lane(&mut self) -> Rect {
            self.app.project_ui.timeline.driven_zones().1.unwrap()
        }
        fn text(&self) -> String {
            self.app.lyrics_track().unwrap().cues[0].text.clone()
        }
        /// The point at `ms` on the ruler's time axis, `dy` below the ruler.
        fn at(&mut self, ms: u64, dy: f32) -> Pos2 {
            let ruler = self.ruler();
            Pos2::new(
                ruler.left() + ruler.width() * ms as f32 / 20_000.0,
                ruler.bottom() + dy,
            )
        }
        fn painted(output: &egui::FullOutput, text: &str) -> Option<Rect> {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(t) if t.galley.text() == text => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                _ => None,
            })
        }
    }

    #[test]
    fn keys_change_nothing_with_a_question_open_nor_with_the_lane_focused() {
        let mut w = window(true);
        // An edit by the agent's hand asks the first-edit question.
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Edited by the agent"));
        w.app.lyrics_edited();
        assert_eq!(w.app.open_question(), Some("first_edit"));
        let range = w.app.project.range;
        let cursor = w.app.mini_preview.cursor_ms;
        let keys = [
            (Key::Escape, Modifiers::NONE),
            (Key::Space, Modifiers::NONE),
            (Key::Delete, Modifiers::NONE),
            (Key::Z, Modifiers::COMMAND),
            (Key::S, Modifiers::COMMAND),
            (Key::ArrowLeft, Modifiers::NONE),
        ];
        for (key, modifiers) in keys {
            w.key(key, modifiers);
            assert_eq!(w.app.open_question(), Some("first_edit"), "{key:?}");
            assert_eq!(w.text(), "Edited by the agent", "{key:?}");
        }
        // The agent answers: written. Then the lyrics lane is focused, as a
        // click on it leaves it.
        let source = w.app.project.track.path.clone().unwrap();
        w.app.consent_first_edit(source);
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Edited again"));
        assert_eq!(w.app.open_question(), None);
        let lane = w.lane();
        w.click(lane.center());
        for (key, modifiers) in keys {
            w.key(key, modifiers);
            assert_eq!(w.text(), "Edited again", "{key:?}");
        }
        assert_eq!(w.app.project.range, range);
        assert_eq!(w.app.mini_preview.cursor_ms, cursor);
        assert!(!w.app.lyrics_screen.open);
        assert_eq!(w.app.clips.len(), 2);
        // The same Ctrl+Z in an ordinary window undoes: the keys do reach
        // these frames, and the driven window drops them.
        let mut ordinary = window(false);
        let source = ordinary.app.project.track.path.clone().unwrap();
        ordinary.app.trust_lyrics(source);
        ordinary
            .app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Edited in an ordinary window"));
        ordinary.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(ordinary.text(), "First test line");
    }

    #[test]
    fn drags_change_nothing_and_the_ruler_moves_the_playhead() {
        let mut w = window(true);
        let order: Vec<u64> = w.app.clips.iter().map(|c| c.id).collect();
        let range = w.app.project.range;
        let (_, first) = w.app.strip.tiles[0];
        let (_, second) = w.app.strip.tiles[1];
        // A tile onto the next one.
        w.drag(first.center(), second.center() + vec2(20.0, 0.0));
        assert_eq!(w.app.clips.iter().map(|c| c.id).collect::<Vec<_>>(), order);
        assert!(w.app.dragging.is_none());
        // The range's start edge, its body, and a new range on the waveform.
        let edge = w.at(5_000, 20.0);
        w.drag(edge, edge + vec2(-120.0, 0.0));
        let body = w.at(7_500, 20.0);
        w.drag(body, body + vec2(150.0, 0.0));
        let wave = w.at(15_000, 70.0);
        w.drag(wave, wave + vec2(80.0, 0.0));
        assert_eq!(w.app.project.range, range);
        // Along the lyrics lane: no line changes, no screen opens.
        let lane = w.lane();
        w.drag(
            lane.left_center() + vec2(60.0, 0.0),
            lane.left_center() + vec2(200.0, 0.0),
        );
        assert_eq!(w.text(), "First test line");
        assert!(!w.app.lyrics_screen.open);
        assert_eq!(w.app.project.range, range);
        // A click on the ruler moves the playhead there.
        let ruler = w.ruler();
        let target = Pos2::new(w.at(12_000, 0.0).x, ruler.center().y);
        w.click(target);
        let cursor = w.app.mini_preview.cursor_ms;
        assert!((11_500..12_500).contains(&cursor), "{cursor}");
        // The same edge drag in an ordinary window moves the range.
        let mut ordinary = window(false);
        let edge = ordinary.at(5_000, 20.0);
        ordinary.drag(edge, edge + vec2(-120.0, 0.0));
        assert_ne!(ordinary.app.project.range, range);
    }

    #[test]
    fn a_click_designates_and_elsewhere_the_bubble_answers() {
        let mut w = window(true);
        let (id, thumb) = w.app.strip.tiles[1];
        w.click(thumb.center());
        assert_eq!(
            w.app.driven.as_ref().unwrap().selection,
            Some(Selection::Clip(id))
        );
        let lane = w.lane();
        let on_second_line = Pos2::new(w.at(15_000, 0.0).x, lane.center().y);
        w.click(on_second_line);
        assert_eq!(
            w.app.driven.as_ref().unwrap().selection,
            Some(Selection::Line(1))
        );
        assert!(!w.app.lyrics_screen.open);
        // The clip's ✕ is never there, hovering included.
        w.frame(vec![Event::PointerMoved(thumb.center())]);
        let remove = Rect::from_min_size(
            Pos2::new(thumb.right() - 24.0, thumb.top() + 4.0),
            vec2(20.0, 20.0),
        );
        let ask = "Ask the agent: it is driving this window.";
        let output = w.frame(vec![]);
        assert!(Window::painted(&output, ask).is_none());
        // A press on it is blocked, and says why.
        w.click(remove.center());
        assert_eq!(w.app.clips.len(), 2);
        let output = w.frame(vec![]);
        assert!(Window::painted(&output, ask).is_some());
    }

    #[test]
    fn the_badge_releases_even_over_a_question_and_the_window_is_theirs() {
        let mut w = window(true);
        let output = w.frame(vec![]);
        assert!(Window::painted(&output, "The agent is driving · Test window").is_some());
        // A Modal question over everything: the first-edit question.
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Edited by the agent"));
        w.app.lyrics_edited();
        assert_eq!(w.app.open_question(), Some("first_edit"));
        let output = w.frame(vec![]);
        let stop = Window::painted(&output, "Stop driving").unwrap();
        w.click(stop.center());
        assert_eq!(w.next(), Some(ToServer::Released));
        assert!(!w.app.driven_locked());
        // The question is theirs now: Escape answers it, as in any NOH.
        w.key(Key::Escape, Modifiers::NONE);
        assert_eq!(w.app.open_question(), None);
        // The agent can no longer close it.
        w.send(&ToApp::Close { id: 5 });
        match w.next() {
            Some(ToServer::Done {
                id: 5,
                error: Some(e),
                ..
            }) => assert_eq!(e.code, "app.released_by_user"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_agent_close_waits_for_questions_and_jobs_and_writes_first() {
        let mut w = window(true);
        let close = |w: &mut Window, id| {
            w.send(&ToApp::Close { id });
            match w.next() {
                Some(ToServer::Done {
                    id: got,
                    result,
                    error,
                }) if got == id => (result, error),
                other => panic!("{other:?}"),
            }
        };
        // A job running: nothing closes.
        w.app.cancelling = true;
        let (_, error) = close(&mut w, 1);
        assert_eq!(error.unwrap().code, "app.job_running");
        w.app.cancelling = false;
        // Unsaved lyrics NOH may not write yet: the save asks, nothing closes.
        w.app.project.edit = None;
        w.app.lyrics_edit = Default::default();
        let source = w.app.project.track.path.clone().unwrap();
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed by the agent"));
        w.app.set_first_edit_aside();
        assert_eq!(w.app.open_question(), None);
        let (_, error) = close(&mut w, 2);
        let error = error.unwrap();
        assert_eq!(error.code, "app.question_open");
        assert_eq!(error.details.unwrap()["question"], "first_edit");
        assert!(!w.app.close_now);
        // Allowed to write: written, then closed.
        w.app.consent_first_edit(source.clone());
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed once more"));
        assert!(w.app.has_unsaved_lyrics());
        let (result, error) = close(&mut w, 3);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(result.unwrap()["state"], "closed");
        assert!(w.app.close_now);
        assert!(
            std::fs::read_to_string(&source)
                .unwrap()
                .contains("Changed once more")
        );
        // The close that follows is the agent's: no `released`, no `closed`.
        w.app.driven_exit();
        assert!(w.from_app.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn the_session_end_and_a_broken_link_hand_the_window_over() {
        let mut w = window(true);
        w.send(&ToApp::SessionEnd);
        for _ in 0..100 {
            w.frame(vec![]);
            if !w.app.driven_locked() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            w.app.driven.as_ref().unwrap().phase,
            Phase::Ended { cut: false, .. }
        ));
        let output = w.frame(vec![]);
        assert!(Window::painted(&output, "The agent disconnected").is_some());
        // Its keys work again: Ctrl+Z undoes, as in any NOH.
        let source = w.app.project.track.path.clone().unwrap();
        w.app.trust_lyrics(source);
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed"));
        w.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(w.text(), "First test line");

        let mut w = window(true);
        w.to_app = None;
        for _ in 0..100 {
            w.frame(vec![]);
            if !w.app.driven_locked() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            w.app.driven.as_ref().unwrap().phase,
            Phase::Ended { cut: true, .. }
        ));
    }

    #[test]
    fn a_close_by_the_person_stops_the_piloting_first() {
        let mut w = window(true);
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, 850.0))),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        let app = &mut w.app;
        w.ctx
            .run_ui(input, |ui| {
                app.draw(ui);
            })
            .textures_delta
            .clear();
        assert_eq!(
            w.from_app.recv_timeout(Duration::from_secs(2)).ok(),
            Some(ToServer::Released)
        );
        assert!(!w.app.driven_locked());
        w.app.driven_exit();
        assert_eq!(
            w.from_app.recv_timeout(Duration::from_secs(2)).ok(),
            Some(ToServer::Closed)
        );
    }

    impl Window {
        /// The latest `state` the window sent, frames running meanwhile.
        fn latest_state(&mut self) -> serde_json::Value {
            // Position-only changes leave at most every 250 ms: frames for longer.
            let until = Instant::now() + Duration::from_millis(400);
            let mut last = None;
            while Instant::now() < until {
                self.frame(vec![]);
                while let Ok(ToServer::State { state, .. }) = self.states.try_recv() {
                    last = Some(state);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            last.expect("a state")
        }
        fn act(&mut self, id: u64, action: driven::Action) -> ToServer {
            self.send(&ToApp::Act { id, action });
            self.next().expect("an answer")
        }
    }

    fn refusal_code(answer: &ToServer) -> Option<&str> {
        match answer {
            ToServer::Done { error: Some(e), .. } => Some(e.code.as_str()),
            _ => None,
        }
    }

    #[test]
    fn gestures_apply_and_the_state_follows() {
        use driven::{Action, LineEdit};
        let mut w = window(true);
        let state = w.latest_state();
        assert_eq!(state["lyrics"]["count"], 2);
        assert_eq!(state["clips"].as_array().unwrap().len(), 2);
        assert_eq!(state["piloting"], "driving");
        // A range, a lyrics line, a picture's duration.
        let answer = w.act(
            1,
            Action::SetRange {
                start_ms: Some(2_000),
                end_ms: Some(8_000),
            },
        );
        assert!(
            matches!(
                answer,
                ToServer::Done {
                    id: 1,
                    error: None,
                    ..
                }
            ),
            "{answer:?}"
        );
        assert_eq!(
            w.app.project.range.map(|r| (r.start_ms, r.end_ms)),
            Some((2_000, 8_000))
        );
        let source = w.app.project.track.path.clone().unwrap();
        w.app.trust_lyrics(source);
        let revision = w.app.project.track.revision;
        let answer = w.act(
            2,
            Action::EditLine {
                edit: LineEdit::SetText {
                    index: 1,
                    text: "Second line, changed".into(),
                },
                based_on: Some(revision),
            },
        );
        assert!(
            matches!(
                answer,
                ToServer::Done {
                    id: 2,
                    error: None,
                    ..
                }
            ),
            "{answer:?}"
        );
        // A stale revision is refused, and nothing changes.
        let answer = w.act(
            3,
            Action::EditLine {
                edit: LineEdit::Delete { index: 0 },
                based_on: Some(revision + 7),
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.state_changed"));
        assert_eq!(w.app.lyrics_track().unwrap().cues.len(), 2);
        let answer = w.act(4, Action::Undo {});
        assert!(
            matches!(answer, ToServer::Done { error: None, .. }),
            "{answer:?}"
        );
        assert_eq!(
            w.app.lyrics_track().unwrap().cues[1].text,
            "Second test line"
        );
        let answer = w.act(
            5,
            Action::SetPhotoDuration {
                clip_id: 99,
                seconds: 3.0,
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.refused"));
        // The answer names the revision of a state that shows the change.
        let answer = w.act(
            6,
            Action::SetRange {
                start_ms: Some(1_000),
                end_ms: Some(6_000),
            },
        );
        let ToServer::Done {
            result: Some(result),
            ..
        } = answer
        else {
            panic!("{answer:?}")
        };
        let revision = result["revision"].as_u64().unwrap();
        let mut seen = None;
        while let Ok(ToServer::State { revision: r, state }) =
            w.states.recv_timeout(Duration::from_millis(200))
        {
            if r == revision {
                seen = Some(state);
            }
        }
        assert_eq!(seen.unwrap()["range"]["start_ms"], 1_000);
    }

    #[test]
    fn the_agent_waits_while_the_person_navigates_then_gives_way() {
        use driven::Action;
        let mut w = window(true);
        // The agent seeks: applied at once.
        let answer = w.act(1, Action::Seek { ms: 4_000 });
        assert!(
            matches!(answer, ToServer::Done { error: None, .. }),
            "{answer:?}"
        );
        assert_eq!(w.app.mini_preview.cursor_ms, 4_000);
        // The person clicks on the ruler: the playhead is theirs.
        let ruler = w.ruler();
        let at = Pos2::new(w.at(15_000, 0.0).x, ruler.center().y);
        w.click(at);
        let theirs = w.app.mini_preview.cursor_ms;
        assert!((14_500..15_500).contains(&theirs), "{theirs}");
        let position = w.latest_state()["user_position"].clone();
        assert_eq!(position["navigating"], true);
        assert_eq!(position["line"]["index"], 1);
        // The agent's seek waits, and is applied once the 3 s are over.
        let start = Instant::now();
        let answer = w.act(2, Action::Seek { ms: 2_000 });
        assert!(
            matches!(answer, ToServer::Done { error: None, .. }),
            "{answer:?}"
        );
        assert!(
            start.elapsed() >= Duration::from_millis(2_000),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(w.app.mini_preview.cursor_ms, 2_000);
        // A gesture of the person within the wait restarts it: refused.
        w.click(at);
        w.send(&ToApp::Act {
            id: 3,
            action: Action::Seek { ms: 1_000 },
        });
        // Queued while the person navigates...
        for _ in 0..5 {
            w.frame(vec![]);
        }
        std::thread::sleep(Duration::from_millis(1_300));
        // ...who navigates again before the wait is over.
        w.click(at);
        let answer = w.next().expect("an answer");
        assert_eq!(
            refusal_code(&answer),
            Some("app.user_navigating"),
            "{answer:?}"
        );
        let ToServer::Done { error: Some(e), .. } = answer else {
            unreachable!()
        };
        let details = e.details.unwrap();
        assert!(details["retry_after_ms"].as_u64().is_some(), "{details}");
        assert_eq!(details["user_position"]["navigating"], true);
        assert!((14_500..15_500).contains(&w.app.mini_preview.cursor_ms));
    }

    #[test]
    fn a_look_writes_a_png_and_never_over_a_file() {
        let mut w = window(true);
        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("look.png");
        w.send(&ToApp::Look {
            id: 7,
            scope: driven::LookScope::Monitor,
            at_ms: Some(3_000),
            output: output.clone(),
        });
        // Headless: no screenshot comes back by itself; answer the window's
        // request as eframe does.
        let mut answered = false;
        for _ in 0..200 {
            let mut events = vec![];
            if let Some(job) = w.app.driven.as_ref().and_then(|d| d.look.as_ref())
                && job.look.requested
            {
                events.push(Event::Screenshot {
                    viewport_id: egui::ViewportId::ROOT,
                    user_data: egui::UserData::new(job.id),
                    image: std::sync::Arc::new(egui::ColorImage::filled(
                        [980, 850],
                        egui::Color32::from_rgb(10, 20, 30),
                    )),
                });
                answered = true;
            }
            w.frame(events);
            if answered {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(answered, "the look asked for a screenshot");
        let answer = w.next().expect("an answer");
        let ToServer::Done {
            id: 7,
            result: Some(result),
            error: None,
        } = answer
        else {
            panic!("{answer:?}")
        };
        assert_eq!(w.app.mini_preview.cursor_ms, 3_000);
        let monitor = w.app.driven.as_ref().unwrap().monitor.unwrap().0;
        assert_eq!(result["width"], monitor.width().round() as u64);
        let decoded = image::open(&output).unwrap();
        assert_eq!(decoded.width() as u64, result["width"].as_u64().unwrap());
        // The same path again: refused, the file untouched.
        let before = std::fs::read(&output).unwrap();
        w.send(&ToApp::Look {
            id: 8,
            scope: driven::LookScope::Window,
            at_ms: None,
            output: output.clone(),
        });
        let answer = w.next().expect("an answer");
        assert_eq!(refusal_code(&answer), Some("app.output_invalid"));
        assert_eq!(std::fs::read(&output).unwrap(), before);
    }

    #[test]
    fn a_job_a_question_and_a_lost_picture_are_answered() {
        use driven::Action;
        let mut w = window(true);
        // A job running: what the interface locks is refused.
        w.app.cancelling = true;
        let answer = w.act(
            1,
            Action::SetRange {
                start_ms: Some(1_000),
                end_ms: Some(4_000),
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.job_running"));
        w.app.cancelling = false;
        // A question open: no files are added under it.
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed by the agent"));
        let folder = tempfile::tempdir().unwrap();
        let other = folder.path().join("other.srt");
        std::fs::write(&other, "1\n00:00:01,000 --> 00:00:02,000\nOther\n\n").unwrap();
        let answer = w.act(2, Action::Add { paths: vec![other] });
        assert_eq!(refusal_code(&answer), Some("app.question_open"));
        // A look whose picture never comes fails, and the next one is served.
        let output = folder.path().join("never.png");
        w.send(&ToApp::Look {
            id: 3,
            scope: driven::LookScope::Window,
            at_ms: None,
            output,
        });
        let start = Instant::now();
        let answer = w.next_within(Duration::from_secs(8)).expect("an answer");
        assert_eq!(refusal_code(&answer), Some("app.look_failed"), "{answer:?}");
        assert!(start.elapsed() >= Duration::from_secs(4));
        assert!(w.app.driven.as_ref().unwrap().look.is_none());
    }

    fn ok(answer: &ToServer) -> bool {
        matches!(answer, ToServer::Done { error: None, .. })
    }

    #[test]
    fn the_montage_gestures_change_what_the_clicks_change() {
        use driven::Action;
        let mut w = window(true);
        // Move the second clip first, then remove the first.
        assert!(ok(&w.act(
            1,
            Action::MoveClip {
                clip_id: 2,
                to_index: 0
            }
        )));
        assert_eq!(w.app.clips.iter().map(|c| c.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(
            refusal_code(&w.act(
                2,
                Action::MoveClip {
                    clip_id: 2,
                    to_index: 5
                }
            )),
            Some("app.refused")
        );
        assert!(ok(&w.act(3, Action::RemoveClip { clip_id: 1 })));
        assert_eq!(w.app.clips.iter().map(|c| c.id).collect::<Vec<_>>(), [2]);
        assert_eq!(
            refusal_code(&w.act(4, Action::RemoveClip { clip_id: 1 })),
            Some("app.refused")
        );
        // Fades and the clips' sound; a fade that cannot be is refused whole.
        let answer = w.act(
            5,
            Action::SetFades {
                fade_in_seconds: Some(1.5),
                fade_out_seconds: Some(0.0),
                clip_audio: Some(true),
            },
        );
        assert!(ok(&answer), "{answer:?}");
        assert!(w.app.in_enabled && w.app.fade_in == 1.5 && !w.app.out_enabled && w.app.clip_audio);
        assert_eq!(
            refusal_code(&w.act(
                6,
                Action::SetFades {
                    fade_in_seconds: Some(-1.0),
                    fade_out_seconds: None,
                    clip_audio: None,
                }
            )),
            Some("app.refused")
        );
        assert_eq!(w.app.fade_in, 1.5);
        assert!(ok(&w.act(7, Action::SetConvertAll { convert_all: true })));
        assert!(!w.app.partial);
        // The short's framing and loops.
        let short_revision = w.app.project.short_revision;
        let answer = w.act(
            8,
            Action::SetFraming {
                framing: Some(noh::shorts::Framing::Crop),
                restart_loops: Some(true),
            },
        );
        assert!(ok(&answer), "{answer:?}");
        assert_eq!(w.app.project.framing, noh::shorts::Framing::Crop);
        assert!(w.app.project.restart_loops);
        assert_ne!(
            w.app.project.short_revision, short_revision,
            "the short is a new one"
        );
        // The destination: an absolute .mp4 in a folder that exists.
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("final.mp4");
        let answer = w.act(
            9,
            Action::SetDestination {
                path: Some(path.clone()),
                short_name: Some("final-short.mp4".into()),
            },
        );
        assert!(ok(&answer), "{answer:?}");
        assert_eq!(w.app.output.as_deref(), Some(path.as_path()));
        assert_eq!(w.app.project.short_name, "final-short.mp4");
        assert_eq!(
            refusal_code(&w.act(
                10,
                Action::SetDestination {
                    path: Some("relative.mp4".into()),
                    short_name: None,
                }
            )),
            Some("app.refused")
        );
        // Nothing to cancel; a job running locks the montage gestures.
        assert_eq!(
            refusal_code(&w.act(11, Action::Cancel {})),
            Some("app.refused")
        );
        w.app.cancelling = true;
        assert_eq!(
            refusal_code(&w.act(12, Action::RemoveClip { clip_id: 2 })),
            Some("app.job_running")
        );
    }

    #[test]
    fn effect_settings_and_export_follow_the_style_panel_and_the_bar() {
        use driven::Action;
        let mut w = window(true);
        let answer = w.act(
            1,
            Action::SetEffectSettings {
                preset: None,
                values: Default::default(),
                size: Some(noh::captions::CaptionSize::Large),
                placement: Some(noh::captions::CaptionPlacement::Top),
                show_on_video: Some(true),
            },
        );
        assert!(ok(&answer), "{answer:?}");
        assert!(w.app.project.apply_subtitles);
        assert_eq!(
            w.app.project.caption_style.size,
            noh::captions::CaptionSize::Large
        );
        assert_eq!(
            w.app.project.caption_style.placement,
            noh::captions::CaptionPlacement::Top
        );
        // A preset of no effect, or a setting the effect does not have.
        w.app.project.effect = None;
        let answer = w.act(
            2,
            Action::SetEffectSettings {
                preset: Some("glow".into()),
                values: Default::default(),
                size: None,
                placement: None,
                show_on_video: None,
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.refused"));
        // The export is refused with the bar's own reason when it cannot start.
        let answer = w.act(3, Action::Export { short: false });
        assert!(
            matches!(
                refusal_code(&answer),
                Some("app.export_unavailable" | "app.not_started")
            ),
            "{answer:?}"
        );
        assert!(!w.app.busy());
    }

    #[test]
    fn the_lyrics_questions_take_the_answers_of_their_buttons() {
        use driven::{Action, Answer};
        let mut w = window(true);
        let source = w.app.project.track.path.clone().unwrap();
        let original = std::fs::read_to_string(&source).unwrap();
        // First edit: a copy, the original kept.
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed by the agent"));
        assert_eq!(w.app.open_question(), Some("first_edit"));
        let folder = tempfile::tempdir().unwrap();
        let copy = folder.path().join("copy.srt");
        let answer = w.act(
            1,
            Action::Answer {
                answer: Answer::WorkOnCopy,
                user_confirmed: false,
                path: Some("relative.srt".into()),
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.output_invalid"));
        let answer = w.act(
            2,
            Action::Answer {
                answer: Answer::WorkOnCopy,
                user_confirmed: false,
                path: Some(copy.clone()),
            },
        );
        assert!(ok(&answer), "{answer:?}");
        assert!(
            std::fs::read_to_string(&copy)
                .unwrap()
                .contains("Changed by the agent")
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert_eq!(w.app.project.track.path.as_deref(), Some(copy.as_path()));
        // A failed save: dismissed.
        w.app.lyrics_save.failed = Some("disk full".into());
        assert_eq!(w.app.open_question(), Some("save_failed"));
        assert_eq!(
            refusal_code(&w.act(
                3,
                Action::Answer {
                    answer: Answer::KeepSrt,
                    user_confirmed: true,
                    path: None,
                }
            )),
            Some("app.no_such_question")
        );
        assert!(ok(&w.act(
            4,
            Action::Answer {
                answer: Answer::Dismiss,
                user_confirmed: false,
                path: None,
            }
        )));
        assert_eq!(w.app.open_question(), None);
        // Dropping unsaved changes needs the person's word.
        w.app.lyrics_save.pending = Some(super::super::app_lyrics::PendingLyrics::Remove);
        let answer = w.act(
            5,
            Action::Answer {
                answer: Answer::Discard,
                user_confirmed: false,
                path: None,
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.confirmation_required"));
        assert!(ok(&w.act(
            6,
            Action::Answer {
                answer: Answer::Cancel,
                user_confirmed: false,
                path: None,
            }
        )));
        assert_eq!(w.app.open_question(), None);
    }

    #[test]
    fn a_destructive_answer_needs_the_person_s_confirmation() {
        use driven::{Action, Answer};
        let mut w = window(true);
        w.app
            .edit_track(|t| noh::subtitle_edit::set_text(t, 0, "Changed by the agent"));
        assert_eq!(w.app.open_question(), Some("first_edit"));
        let answer = w.act(
            1,
            Action::Answer {
                answer: Answer::ModifySrt,
                user_confirmed: false,
                path: None,
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.confirmation_required"));
        assert_eq!(w.app.open_question(), Some("first_edit"));
        let answer = w.act(
            2,
            Action::Answer {
                answer: Answer::KeepSrt,
                user_confirmed: true,
                path: None,
            },
        );
        assert_eq!(refusal_code(&answer), Some("app.no_such_question"));
        let answer = w.act(
            3,
            Action::Answer {
                answer: Answer::ModifySrt,
                user_confirmed: true,
                path: None,
            },
        );
        assert!(
            matches!(answer, ToServer::Done { error: None, .. }),
            "{answer:?}"
        );
        assert_eq!(w.app.open_question(), None);
        let source = w.app.project.track.path.clone().unwrap();
        assert!(
            std::fs::read_to_string(source)
                .unwrap()
                .contains("Changed by the agent")
        );
    }

    #[test]
    fn arguments_name_a_driven_window_only_first() {
        let args = |list: &[&str]| arguments(list.iter().map(std::ffi::OsString::from));
        assert_eq!(
            args(&["noh-app", "--driven", "--title", "Clip"]),
            Some("Clip".into())
        );
        assert_eq!(args(&["noh-app", "--driven"]), Some(String::new()));
        assert_eq!(args(&["noh-app", "song.wav", "--driven"]), None);
        assert_eq!(args(&["noh-app"]), None);
    }
}
