//! The shared project clock rendered as one interactive timeline. No media work or IO.
use crate::{app_locale::Locale, app_style as style};
use eframe::egui::{self, Align2, Color32, FontId, Id, Key, Rect, Sense, Stroke, Vec2, pos2, vec2};
use noh::{
    i18n::Language,
    subtitle_track::SubtitleTrack,
    timeline::{Drag, DragPart, Range, Viewport},
    waveform::Waveform,
};

const MIN_RANGE: u64 = 100;
const MIN_VIEW: u64 = 2_000;
const MAX_PIECES: usize = 1024;
// Lane heights from the design system (`lane-*-h`); the waveform is 64, 48 compact.
const PICTURES_H: f32 = 28.0;
const LYRICS_H: f32 = 20.0;
const GAP: f32 = 4.0;

pub struct Visual {
    pub label: String,
    pub seconds: f64,
    /// Position in the original media strip, retained when unreadable items are skipped.
    pub source_index: usize,
}

pub struct Input<'a> {
    pub cursor_ms: Option<u64>,
    pub duration_ms: u64,
    pub range: Option<Range>,
    pub viewport: Option<Viewport>,
    pub waveform: Option<&'a Waveform>,
    pub track: Option<&'a SubtitleTrack>,
    pub visuals: &'a [Visual],
    pub waveform_loading: bool,
    pub waveform_error: Option<&'a str>,
    pub enabled: bool,
    pub fade_in_seconds: f64,
    pub fade_out_seconds: f64,
    /// "Pictures from the start": the ↺ mark at the range start.
    pub restart: bool,
}

/// Assign range and viewport every frame. Bump the short revision only when
/// commit_changed is true; intermediate pointer movement is an uncommitted edit.
pub struct Output {
    pub hover_ms: Option<u64>,
    pub seek_ms: Option<u64>,
    pub range: Option<Range>,
    pub viewport: Option<Viewport>,
    pub commit_changed: bool,
    pub retry: bool,
}

#[derive(Default)]
pub struct TimelineState {
    drag: Option<RangeDrag>,
    overview: Option<(Viewport, f32)>,
    duration_ms: u64,
    pub body_id: Option<Id>,
    wave: Option<WavePaint>,
    geometry: Option<Geometry>,
    /// The ↺ mark painted last frame, for tests.
    #[cfg(test)]
    pub restart_mark: Option<Rect>,
}

/// One immutable waveform mesh. Pointer/playback movement changes neither its
/// envelope nor its tessellation. Keep only the current view (bounded memory).
struct WavePaint {
    source: noh::inspection::FileStamp,
    decoder: noh::inspection::FileStamp,
    key: WavePaintKey,
    meshes: Vec<std::sync::Arc<egui::Mesh>>,
}
#[derive(PartialEq)]
struct WavePaintKey {
    rect: Rect,
    view: Viewport,
    range: Option<Range>,
    scale: f32,
    wave: Color32,
    selection: Color32,
}
impl TimelineState {
    /// Bottom of the last lane drawn, for layout tests.
    #[cfg(test)]
    pub fn lanes_bottom(&self) -> Option<f32> {
        self.geometry.map(|g| g.lanes().bottom())
    }
    /// The pictures lane drawn last frame, for tests.
    pub fn pictures_rect(&self) -> Option<Rect> {
        self.geometry.map(|g| g.pictures)
    }
    /// Zoom controls wait while a range drag is in progress.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

#[derive(Clone, Copy)]
struct RangeDrag {
    original: Option<Range>,
    part: DragPart,
    anchor_ms: u64,
    anchor_x: f32,
    moved: bool,
}

#[derive(Clone, Copy)]
struct Geometry {
    stage: Rect,
    ruler: Rect,
    pictures: Rect,
    wav: Rect,
    cues: Option<Rect>,
}
impl Geometry {
    /// Ruler, pictures, waveform and (with a track) the lyrics lane.
    fn height(compact: bool, cues: bool) -> f32 {
        let (ruler, wave) = if compact { (18.0, 48.0) } else { (20.0, 64.0) };
        ruler + PICTURES_H + GAP + wave + if cues { GAP + LYRICS_H } else { 0.0 }
    }
    fn new(rect: Rect, compact: bool, cues: bool) -> Self {
        let ruler = Rect::from_min_size(
            rect.min,
            vec2(rect.width(), if compact { 18.0 } else { 20.0 }),
        );
        let pictures = Rect::from_min_size(
            pos2(rect.left(), ruler.bottom()),
            vec2(rect.width(), PICTURES_H),
        );
        let wav = Rect::from_min_size(
            pos2(rect.left(), pictures.bottom() + 4.0),
            vec2(rect.width(), if compact { 48.0 } else { 64.0 }),
        );
        let cues = cues.then(|| {
            Rect::from_min_size(
                pos2(rect.left(), wav.bottom() + 4.0),
                vec2(rect.width(), LYRICS_H),
            )
        });
        Self {
            stage: rect,
            ruler,
            pictures,
            wav,
            cues,
        }
    }
    fn lanes(self) -> Rect {
        Rect::from_min_max(self.pictures.min, self.stage.max)
    }
}

#[derive(Clone, Copy)]
struct Palette {
    wav: Color32,
    wave: Color32,
    cue: Color32,
    selection: Color32,
    on_selection: Color32,
    tint: Color32,
    focus: Color32,
    segments: [Color32; 3],
    cue_in: Color32,
    playhead: Color32,
    hover: Color32,
}
impl Palette {
    /// Timeline colours from the design-system tokens (lanes on `surface`,
    /// the short range in the Lagon accent, pictures in the sequence colours).
    fn new(dark: bool) -> Self {
        let t = style::tokens_for(dark);
        Self {
            wav: t.surface,
            wave: t.wave,
            cue: t.cue,
            selection: t.accent,
            on_selection: t.on_accent,
            tint: t.accent_soft,
            focus: t.focus,
            segments: t.seq,
            cue_in: t.accent_text,
            playhead: t.text,
            hover: t.text_3,
        }
    }
}

fn snapped(ms: u64) -> u64 {
    ms.saturating_add(50) / 100 * 100
}
fn x_at(view: Viewport, rect: Rect, ms: u64) -> f32 {
    rect.left() + view.fraction(ms) as f32 * rect.width()
}
fn time_at(view: Viewport, rect: Rect, x: f32) -> u64 {
    view.time(f64::from((x - rect.left()) / rect.width().max(1.0)))
}
fn bounded_view(view: Viewport, duration: u64) -> Option<Viewport> {
    Range::place(
        view.0.start_ms,
        view.0.duration_ms().max(MIN_VIEW.min(duration)),
        duration,
    )
    .map(Viewport)
}
fn zoom(view: Viewport, factor: f64, anchor: f64, duration: u64) -> Viewport {
    let span = ((view.0.duration_ms() as f64 / factor).round() as u64)
        .clamp(MIN_VIEW.min(duration), duration);
    let start = view
        .time(anchor)
        .saturating_sub((anchor * span as f64).round() as u64);
    Viewport(Range::place(start, span, duration).unwrap())
}

fn current_view(view: Option<Viewport>, duration: u64) -> Option<Viewport> {
    view.and_then(|v| bounded_view(v, duration))
        .or_else(|| Viewport::full(duration))
}

/// Zoom buttons: ×2 / ×0.5 around the range when it is visible, otherwise
/// around the view centre. Only the view changes.
pub fn zoom_by(
    view: Option<Viewport>,
    factor: f64,
    range: Option<Range>,
    duration: u64,
) -> Option<Viewport> {
    let view = current_view(view, duration)?;
    let center = range
        .filter(|r| r.end_ms > view.0.start_ms && r.start_ms < view.0.end_ms)
        .map(|r| r.start_ms + r.duration_ms() / 2)
        .unwrap_or_else(|| view.time(0.5));
    Some(zoom(
        view,
        factor,
        view.fraction(center).clamp(0.0, 1.0),
        duration,
    ))
}

/// "Choose a short": 15 s, or 40 % of the visible span, or the whole song
/// when shorter, centred in the view.
pub fn place_short(view: Option<Viewport>, duration: u64) -> Option<Range> {
    let view = current_view(view, duration)?;
    let length = (view.0.duration_ms().saturating_mul(4) / 10)
        .min(15_000)
        .max(MIN_RANGE.min(duration))
        .min(duration);
    Range::place(view.time(0.5).saturating_sub(length / 2), length, duration)
}

/// The visible part of the song, for the range row's `0:00 – 0:30` label;
/// `None` while the whole song is shown.
pub fn visible_label(view: Option<Viewport>, duration: u64, language: Language) -> Option<String> {
    let view = current_view(view, duration)?;
    (view != Viewport::full(duration)?).then(|| {
        let hours = duration >= 3_600_000;
        format!(
            "{} – {}",
            time_label(view.0.start_ms, false, hours, language),
            time_label(view.0.end_ms, false, hours, language)
        )
    })
}

/// The short panel and offscreen chips share this exact viewport operation.
pub fn focus_range(range: Range, duration_ms: u64) -> Option<Viewport> {
    let margin = (range.duration_ms().saturating_mul(3) / 10).max(500);
    let span = range
        .duration_ms()
        .saturating_add(margin.saturating_mul(2))
        .max(MIN_VIEW.min(duration_ms));
    Range::place(range.start_ms.saturating_sub(margin), span, duration_ms).map(Viewport)
}

/// A, B, … Z, then AA, AB…: the sequence letter of the item at `index`,
/// shared by the media tiles and the timeline cells.
pub fn sequence_letter(index: usize) -> String {
    let mut n = index + 1;
    let mut letters = Vec::new();
    while n > 0 {
        n -= 1;
        letters.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap()
}

/// Project time as `m:ss,d` (`h:mm:ss,d` for songs of an hour or more).
pub fn clock(ms: u64, duration_ms: u64, language: Language) -> String {
    time_label(ms, true, duration_ms >= 3_600_000, language)
}
/// [`clock`] floored to the tenth: never later than the frame on screen.
pub fn clock_floor(ms: u64, duration_ms: u64, language: Language) -> String {
    time_label(ms / 100 * 100, true, duration_ms >= 3_600_000, language)
}

fn time_label(ms: u64, tenths: bool, hours: bool, language: Language) -> String {
    let ms = if tenths { snapped(ms) } else { ms };
    let seconds = ms / 1000;
    let base = if hours {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    };
    language.localize_decimal(if tenths {
        format!("{base}.{}", ms / 100 % 10)
    } else {
        base
    })
}

fn range_edit(drag: RangeDrag, pointer: u64, duration: u64) -> Option<Range> {
    if let Some(original) = drag.original {
        let result = Drag::new(original, drag.part, drag.anchor_ms).at(pointer, duration)?;
        match drag.part {
            DragPart::Body => {
                Range::place(snapped(result.start_ms), original.duration_ms(), duration)
            }
            DragPart::Start => Range::new(
                snapped(result.start_ms).min(original.end_ms.saturating_sub(MIN_RANGE)),
                original.end_ms,
                duration,
            ),
            DragPart::End => Range::new(
                original.start_ms,
                snapped(result.end_ms)
                    .max(original.start_ms.saturating_add(MIN_RANGE))
                    .min(duration),
                duration,
            ),
        }
    } else {
        let anchor = snapped(drag.anchor_ms).min(duration);
        let end = snapped(pointer).min(duration);
        Range::place(
            anchor.min(end),
            anchor.abs_diff(end).max(MIN_RANGE.min(duration)),
            duration,
        )
    }
}

fn keyboard_edit(
    range: Range,
    part: DragPart,
    key: Key,
    shift: bool,
    duration: u64,
) -> Option<Range> {
    let step = if shift { 1000 } else { 100 };
    let delta = match key {
        Key::ArrowLeft | Key::ArrowDown => -step,
        Key::ArrowRight | Key::ArrowUp => step,
        Key::PageUp => 10_000,
        Key::PageDown => -10_000,
        _ => 0,
    };
    let current = if part == DragPart::End {
        range.end_ms
    } else {
        range.start_ms
    };
    let target = match key {
        Key::Home => 0,
        Key::End => duration,
        _ => current.saturating_add_signed(delta),
    };
    match part {
        DragPart::Body => Range::place(target, range.duration_ms(), duration),
        DragPart::Start => Range::new(
            target.min(range.end_ms.saturating_sub(MIN_RANGE)),
            range.end_ms,
            duration,
        ),
        DragPart::End => Range::new(
            range.start_ms,
            target
                .max(range.start_ms.saturating_add(MIN_RANGE))
                .min(duration),
            duration,
        ),
    }
}

fn reveal(view: Viewport, range: Range, part: DragPart, duration: u64) -> Viewport {
    let edge = if part == DragPart::Start {
        range.start_ms
    } else {
        range.end_ms
    };
    if edge < view.0.start_ms {
        Viewport(Range::place(edge, view.0.duration_ms(), duration).unwrap())
    } else if edge > view.0.end_ms {
        Viewport(
            Range::place(
                edge.saturating_sub(view.0.duration_ms()),
                view.0.duration_ms(),
                duration,
            )
            .unwrap(),
        )
    } else if part == DragPart::Body && range.start_ms < view.0.start_ms {
        Viewport(Range::place(range.start_ms, view.0.duration_ms(), duration).unwrap())
    } else {
        view
    }
}

pub fn show(
    ui: &mut egui::Ui,
    locale: &Locale,
    state: &mut TimelineState,
    input: Input<'_>,
) -> Output {
    ui.push_id("project-timeline", |ui| {
        show_inner(ui, locale.language, state, input)
    })
    .inner
}

fn show_inner(
    ui: &mut egui::Ui,
    l: Language,
    state: &mut TimelineState,
    input: Input<'_>,
) -> Output {
    let mut output = Output {
        hover_ms: None,
        seek_ms: None,
        range: input.range,
        viewport: input.viewport,
        commit_changed: false,
        retry: false,
    };
    let duration = input.duration_ms;
    if state.duration_ms != duration {
        state.drag = None;
        state.overview = None;
        state.duration_ms = duration;
    }
    if duration == 0 {
        ui.label(l.text("timeline.empty"));
        output.viewport = None;
        return output;
    }
    let compact = ui.available_width() < style::COMPACT_BELOW;
    let palette = Palette::new(ui.visuals().dark_mode);
    let hours = duration >= 3_600_000;
    let label = |ms| time_label(ms, true, hours, l);
    let mut view = input
        .viewport
        .and_then(|v| bounded_view(v, duration))
        .unwrap_or_else(|| Viewport::full(duration).unwrap());
    let enabled = input.enabled && ui.is_enabled();
    let id = ui.make_persistent_id("clock");
    // The range row (app_range.rs) moves focus here after "Choose a short".
    state.body_id = Some(id.with("body"));
    let height = Geometry::height(compact, input.track.is_some());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width().max(1.0), height), Sense::hover());
    let geometry = Geometry::new(rect, compact, input.track.is_some());
    if enabled {
        let ruler = ui.interact(geometry.ruler, id.with("seek"), Sense::click_and_drag());
        if (ruler.clicked() || ruler.dragged())
            && let Some(position) = ruler.interact_pointer_pos()
        {
            output.seek_ms = Some(time_at(view, rect, position.x));
        }
    }
    {
        state.geometry = Some(geometry);
    }
    let names = input
        .visuals
        .iter()
        .take(12)
        .map(|v| v.label.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let sequence = input
        .visuals
        .iter()
        .filter_map(|v| (v.seconds.is_finite() && v.seconds > 0.0).then_some(v.seconds))
        .sum::<f64>();
    let passes = if sequence > 0.0 {
        (duration as f64 / 1000.0 / sequence).ceil() as u64
    } else {
        0
    };
    let mut summary = l.format(
        "timeline.summary",
        &[
            names,
            passes.to_string(),
            label(duration),
            input.track.map_or(0, |track| track.cues.len()).to_string(),
        ],
    );
    if let Some(range) = output.range {
        summary.push_str(&format!(
            ". {}",
            l.format(
                "timeline.short_summary",
                &[
                    label(range.start_ms),
                    label(range.end_ms),
                    l.decimal(range.duration_ms() as f64 / 1000.0, 1)
                ]
            )
        ));
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, summary.clone()));

    if ui.rect_contains_pointer(rect) && state.drag.is_none() {
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
        let wheel = ui.input_mut(|i| {
            let mut zoom_steps = 0.0_f32;
            let mut pan = 0.0_f32;
            i.events.retain(|event| {
                if let egui::Event::MouseWheel {
                    delta, modifiers, ..
                } = event
                {
                    if modifiers.ctrl || modifiers.command {
                        zoom_steps += delta.y;
                        return false;
                    }
                    if modifiers.shift || delta.x != 0.0 {
                        pan += if delta.x != 0.0 { delta.x } else { delta.y };
                        return false;
                    }
                }
                true
            });
            if zoom_steps != 0.0 || pan != 0.0 {
                i.smooth_scroll_delta = Vec2::ZERO;
            }
            (zoom_steps, pan)
        });
        if wheel.0 != 0.0 {
            view = zoom(
                view,
                if wheel.0 > 0.0 { 1.25 } else { 0.8 },
                f64::from((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
                duration,
            );
        }
        if wheel.1 != 0.0 {
            view = view
                .pan(
                    (-f64::from(wheel.1) / f64::from(rect.width()) * view.0.duration_ms() as f64)
                        .round() as i64,
                    duration,
                )
                .unwrap();
        }
    }

    let mut focus = None;
    if let Some(range) = output.range {
        let left = x_at(view, rect, range.start_ms);
        let right = x_at(view, rect, range.end_ms);
        // Hit width 24 (28 compact), centred on each edge.
        let half_hit = if compact { 14.0 } else { 12.0 };
        let midpoint = (left + right) * 0.5;
        let lanes = geometry.lanes();
        let start_hit = Rect::from_min_max(
            pos2(left - half_hit, lanes.top()),
            pos2((left + half_hit).min(midpoint - 0.5), lanes.bottom()),
        )
        .intersect(lanes);
        let end_hit = Rect::from_min_max(
            pos2((right - half_hit).max(midpoint + 0.5), lanes.top()),
            pos2(right + half_hit, lanes.bottom()),
        )
        .intersect(lanes);
        let body = Rect::from_min_max(
            pos2((left + half_hit).min(midpoint - 0.5), lanes.top()),
            pos2((right - half_hit).max(midpoint + 0.5), lanes.bottom()),
        )
        .intersect(lanes);
        // Stable declaration order is also the keyboard focus order.
        for (part, hit, key, name) in [
            (DragPart::Start, start_hit, "start", "timeline.range_start"),
            (DragPart::Body, body, "body", "timeline.range_body"),
            (DragPart::End, end_hit, "end", "timeline.range_end"),
        ] {
            if !hit.is_positive() {
                continue;
            }
            let interaction = ui.interact(
                hit,
                id.with(key),
                if enabled {
                    Sense::click_and_drag()
                } else {
                    Sense::hover()
                },
            );
            interaction.widget_info(|| {
                let mut info = egui::WidgetInfo::slider(
                    enabled,
                    if part == DragPart::End {
                        range.end_ms
                    } else {
                        range.start_ms
                    } as f64
                        / 1000.0,
                    l.text(name),
                );
                info.current_text_value = Some(if part == DragPart::Body {
                    format!("{} – {}", label(range.start_ms), label(range.end_ms))
                } else {
                    label(if part == DragPart::End {
                        range.end_ms
                    } else {
                        range.start_ms
                    })
                });
                info
            });
            ui.ctx().accesskit_node_builder(interaction.id, |node| {
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(duration as f64 / 1000.0);
                node.set_numeric_value_step(0.1);
                if enabled {
                    node.add_action(egui::accesskit::Action::Increment);
                    node.add_action(egui::accesskit::Action::Decrement);
                }
            });
            let has_focus = interaction.has_focus();
            if has_focus {
                focus = Some(part);
                // Arrow keys edit this slider; they must not also schedule
                // egui's spatial focus navigation to a neighboring handle.
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        interaction.id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            ..Default::default()
                        },
                    )
                });
            }
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if enabled
                && pressed
                && interaction.is_pointer_button_down_on()
                && let Some(pointer) = interaction.interact_pointer_pos()
            {
                interaction.request_focus();
                state.drag = Some(RangeDrag {
                    original: Some(range),
                    part,
                    anchor_ms: time_at(view, rect, pointer.x),
                    anchor_x: pointer.x,
                    moved: false,
                });
            }
            if enabled && interaction.hovered() {
                ui.ctx().set_cursor_icon(if part == DragPart::Body {
                    if state.drag.is_some() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    }
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
            }
            if enabled && state.drag.is_none() {
                let action = ui.input_mut(|i| {
                    let shift = i.modifiers.shift;
                    let mut key = None;
                    if has_focus {
                        for candidate in [
                            Key::ArrowLeft,
                            Key::ArrowDown,
                            Key::ArrowRight,
                            Key::ArrowUp,
                            Key::PageUp,
                            Key::PageDown,
                            Key::Home,
                            Key::End,
                        ] {
                            if i.consume_key(i.modifiers, candidate) {
                                key = Some(candidate);
                                break;
                            }
                        }
                    }
                    if i.num_accesskit_action_requests(
                        interaction.id,
                        egui::accesskit::Action::Increment,
                    ) > 0
                    {
                        key = Some(Key::ArrowRight);
                    }
                    if i.num_accesskit_action_requests(
                        interaction.id,
                        egui::accesskit::Action::Decrement,
                    ) > 0
                    {
                        key = Some(Key::ArrowLeft);
                    }
                    key.map(|key| (key, shift))
                });
                if let Some((key, shift)) = action
                    && let Some(next) = keyboard_edit(range, part, key, shift, duration)
                {
                    output.commit_changed |= Some(next) != output.range;
                    output.range = Some(next);
                    view = reveal(view, next, part, duration);
                }
            }
            if !enabled {
                interaction.on_hover_text(l.text("bar.locked"));
            }
        }
    } else if enabled {
        let creation = ui
            .interact(geometry.lanes(), id.with("draw"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        if ui.input(|i| i.pointer.primary_pressed()) && creation.is_pointer_button_down_on() {
            if let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) {
                state.drag = Some(RangeDrag {
                    original: None,
                    part: DragPart::Body,
                    anchor_ms: time_at(view, rect, pointer.x),
                    anchor_x: pointer.x,
                    moved: false,
                });
            }
        }
    }
    if let Some(mut drag) = state.drag {
        let (pointer, down, released, escape, gone) = ui.input_mut(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.events
                    .iter()
                    .any(|event| matches!(event, egui::Event::PointerGone)),
            )
        });
        if escape || gone || !enabled || (!down && !released) {
            output.range = drag.original;
            state.drag = None;
        } else {
            if let Some(pointer) = pointer {
                drag.moved |= (pointer.x - drag.anchor_x).abs() > 3.0;
                if drag.moved {
                    output.range = range_edit(drag, time_at(view, rect, pointer.x), duration);
                }
            }
            if released {
                output.commit_changed |= output.range != drag.original;
                if output.range.is_some() && drag.original.is_none() {
                    ui.memory_mut(|memory| memory.request_focus(id.with("body")));
                }
                state.drag = None;
            } else {
                state.drag = Some(drag);
            }
        }
    }

    paint_lanes(
        ui,
        state,
        geometry,
        view,
        &input,
        output.range,
        palette,
        compact,
        l,
    );
    if let Some(range) = output.range {
        paint_range(
            ui, id, geometry, view, range, palette, state.drag, focus, l, duration, enabled,
        );
        let mark = input
            .restart
            .then(|| restart_mark(ui, id, geometry, view, range, palette, l, enabled))
            .flatten();
        #[cfg(test)]
        {
            state.restart_mark = mark;
        }
        let _ = mark;
        if range.end_ms <= view.0.start_ms || range.start_ms >= view.0.end_ms {
            let before = range.end_ms <= view.0.start_ms;
            let chip = Rect::from_min_size(
                pos2(
                    if before {
                        rect.left() + 4.0
                    } else {
                        rect.right() - 92.0
                    },
                    geometry.wav.center().y - 14.0,
                ),
                vec2(88.0, 28.0),
            );
            if ui
                .put(
                    chip,
                    egui::Button::new(l.text(if before {
                        "timeline.short_before"
                    } else {
                        "timeline.short_after"
                    })),
                )
                .clicked()
            {
                view = focus_range(range, duration).unwrap();
            }
        }
    }
    if input.waveform.is_none() {
        let text = l.text(if input.waveform_loading {
            "timeline.wav_loading"
        } else {
            "timeline.wav_unavailable"
        });
        let painter = ui.painter_at(geometry.wav);
        painter.text(
            pos2(
                geometry.wav.center().x - if input.waveform_loading { 0.0 } else { 36.0 },
                geometry.wav.center().y,
            ),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(13.0),
            palette.wave,
        );
        if !input.waveform_loading {
            let retry = Rect::from_center_size(
                pos2(geometry.wav.right() - 36.0, geometry.wav.center().y),
                vec2(64.0, 28.0),
            );
            // A detached child: the button sits over the lane without growing
            // the timeline's allocated height.
            let mut over = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("waveform-retry")
                    .max_rect(retry)
                    .layout(egui::Layout::centered_and_justified(
                        egui::Direction::LeftToRight,
                    )),
            );
            let response = over.add(egui::Button::new(l.text("timeline.retry")));
            if response.clicked() {
                output.retry = true;
            }
            if let Some(error) = input.waveform_error {
                response.on_hover_text(error);
            }
        }
    }
    if view.0.duration_ms() < duration {
        ui.add_space(GAP);
        view = overview(ui, id, state, view, output.range, duration, palette);
    }
    // Use the final viewport, clipping and layer hit testing. A range edit must
    // not also scrub the preview, including its release frame.
    if enabled
        && ui.rect_contains_pointer(rect)
        && state.drag.is_none()
        && !ui.input(|i| i.pointer.primary_down() || i.pointer.primary_released())
        && let Some(position) = ui.ctx().pointer_hover_pos()
    {
        output.hover_ms = Some(time_at(view, rect, position.x));
    }
    // The playhead (2 px `text`, with a cap in the ruler) marks the preview
    // cursor; a 1 px `text-3` line follows the pointer when it differs.
    let lanes = geometry.lanes();
    let painter = ui.painter_at(rect.expand2(vec2(6.0, 0.0)));
    let visible = |ms: &u64| *ms >= view.0.start_ms && *ms <= view.0.end_ms;
    if let Some(ms) = output
        .hover_ms
        .filter(visible)
        .filter(|ms| Some(*ms) != input.cursor_ms)
    {
        let x = x_at(view, rect, ms);
        painter.line_segment(
            [pos2(x, lanes.top()), pos2(x, lanes.bottom())],
            Stroke::new(1.0, palette.hover),
        );
    }
    if let Some(ms) = input.cursor_ms.filter(visible) {
        let x = x_at(view, rect, ms);
        painter.line_segment(
            [
                pos2(x, geometry.ruler.bottom() - 6.0),
                pos2(x, lanes.bottom() + 2.0),
            ],
            Stroke::new(2.0, palette.playhead),
        );
        painter.rect_filled(playhead_cap(geometry, x), 2, palette.playhead);
    }
    output.viewport = Some(view);
    output
}

fn paint_lanes(
    ui: &egui::Ui,
    state: &mut TimelineState,
    geometry: Geometry,
    view: Viewport,
    input: &Input<'_>,
    range: Option<Range>,
    colors: Palette,
    compact: bool,
    l: Language,
) {
    // Lanes sit on `surface` over the window ground; the ruler has no fill.
    let painter = ui.painter_at(geometry.stage);
    let lane_radius = style::radius::S;
    painter.rect_filled(geometry.wav, lane_radius, colors.wav);
    paint_pictures(ui, geometry.pictures, view, input, colors);
    paint_ruler(ui, geometry.ruler, view, input.duration_ms, compact, l);
    if let Some(waveform) = input.waveform {
        let key = WavePaintKey {
            rect: geometry.wav,
            view,
            range,
            scale: ui.ctx().pixels_per_point(),
            wave: colors.wave,
            selection: colors.selection,
        };
        if state.wave.as_ref().is_none_or(|cache| {
            cache.key != key || cache.source != waveform.source || cache.decoder != waveform.decoder
        }) {
            let center = geometry.wav.center().y;
            let columns = (geometry.wav.width() * key.scale)
                .ceil()
                .clamp(1.0, 16_384.0) as usize;
            let dx = geometry.wav.width() / columns as f32;
            let start = view.0.start_ms as f64 / 1000.0;
            let dt = view.0.duration_ms() as f64 / 1000.0 / columns as f64;
            let mut shapes = Vec::with_capacity(columns);
            for column in 0..columns {
                let t0 = start + column as f64 * dt;
                let t1 = start + (column + 1) as f64 * dt;
                let peak = waveform.peak_at(t0, t1);
                let height = peak * (geometry.wav.height() * 0.5 - 4.0);
                let x = geometry.wav.left() + (column as f32 + 0.5) * dx;
                // Bars inside the short range take the accent: the product's signature.
                let color = if range.is_some_and(|r| {
                    (t0 * 1000.0) as u64 >= r.start_ms && (t1 * 1000.0) as u64 <= r.end_ms
                }) {
                    colors.selection
                } else {
                    colors.wave
                };
                shapes.push(egui::epaint::ClippedShape {
                    clip_rect: geometry.wav,
                    shape: egui::Shape::line_segment(
                        [
                            pos2(x, center - height.max(0.3)),
                            pos2(x, center + height.max(0.3)),
                        ],
                        Stroke::new(dx, color),
                    ),
                });
            }
            let meshes = ui
                .ctx()
                .tessellate(shapes, key.scale)
                .into_iter()
                .filter_map(|p| match p.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => Some(std::sync::Arc::new(mesh)),
                    _ => None,
                })
                .collect();
            state.wave = Some(WavePaint {
                source: waveform.source.clone(),
                decoder: waveform.decoder.clone(),
                key,
                meshes,
            });
        }
        for mesh in &state.wave.as_ref().unwrap().meshes {
            ui.painter_at(geometry.wav)
                .add(egui::Shape::Mesh(mesh.clone()));
        }
    } else {
        state.wave = None;
    }
    if let (Some(track), Some(lane)) = (input.track, geometry.cues) {
        let first = track
            .cues
            .partition_point(|cue| cue.end_ms <= view.0.start_ms);
        for cue in track.cues[first..]
            .iter()
            .take_while(|cue| cue.start_ms < view.0.end_ms)
        {
            let left = x_at(view, lane, cue.start_ms).max(lane.left());
            let right = x_at(view, lane, cue.end_ms)
                .min(lane.right())
                .max(left + 2.0)
                .min(lane.right());
            // 8 px cue bars, 6 px from the lane top; cues within the short
            // (half a second of tolerance) use `accent-text`.
            let near = range
                .is_some_and(|r| cue.start_ms + 500 >= r.start_ms && cue.end_ms <= r.end_ms + 500);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(left, lane.top() + 6.0),
                    pos2((right - 1.0).max(left + 2.0), lane.top() + 14.0),
                ),
                4,
                if near { colors.cue_in } else { colors.cue },
            );
        }
    }
}

fn paint_ruler(
    ui: &egui::Ui,
    rect: Rect,
    view: Viewport,
    duration: u64,
    compact: bool,
    l: Language,
) {
    const STEPS: [u64; 17] = [
        100, 200, 500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000,
        900_000, 1_800_000, 3_600_000, 7_200_000,
    ];
    let minimum = if compact { 64.0 } else { 78.0 };
    let step = STEPS
        .into_iter()
        .find(|step| {
            *step as f64 / view.0.duration_ms() as f64 * f64::from(rect.width()) >= minimum
        })
        .unwrap_or_else(|| {
            ((view.0.duration_ms() as f64 / f64::from(rect.width()) * minimum / 3_600_000.0).ceil()
                as u64)
                .max(1)
                * 3_600_000
        });
    let painter = ui.painter_at(rect);
    // Design: a 16 px `border` tick per label, 12 px `text-3` labels 4 px after it.
    let tokens = style::tokens(ui);
    let first = view.0.start_ms / step * step;
    for major in (first..=view.0.end_ms).step_by(step as usize).take(256) {
        for minor in 0..5 {
            let time = major.saturating_add(step * minor / 5);
            if time < view.0.start_ms || time > view.0.end_ms {
                continue;
            }
            let x = x_at(view, rect, time);
            painter.line_segment(
                [
                    pos2(
                        x + 0.5,
                        if minor == 0 {
                            rect.top()
                        } else {
                            rect.bottom() - 3.0
                        },
                    ),
                    pos2(
                        x + 0.5,
                        if minor == 0 {
                            rect.top() + 16.0
                        } else {
                            rect.bottom()
                        },
                    ),
                ],
                Stroke::new(1.0, tokens.border),
            );
            if minor == 0 {
                let galley = painter.layout_no_wrap(
                    time_label(time, step < 1000, duration >= 3_600_000, l),
                    FontId::proportional(style::text::SMALL),
                    tokens.text_3,
                );
                if x + galley.size().x + 4.0 <= rect.right() {
                    painter.galley(pos2(x + 4.0, rect.top()), galley, tokens.text_3);
                }
            }
        }
    }
}

struct Piece {
    index: usize,
    start: f64,
    end: f64,
    partial: bool,
}
/// Total seconds of one pass over the pictures, when every duration is valid.
pub fn sequence_seconds(visuals: &[Visual]) -> Option<f64> {
    let sequence: f64 = visuals.iter().map(|v| v.seconds).sum();
    (sequence.is_finite()
        && sequence > 0.0
        && visuals
            .iter()
            .all(|v| v.seconds.is_finite() && v.seconds > 0.0))
    .then_some(sequence)
}
/// The looping picture cells as `(visual index, start, end)` in seconds, from
/// the start of the pass containing `from`. The lane and the picture
/// navigation share this arithmetic. Ends when a cell would not advance.
pub fn cells(visuals: &[Visual], from: f64) -> impl Iterator<Item = (usize, f64, f64)> + '_ {
    let sequence = sequence_seconds(visuals);
    let mut offset = sequence.map_or(0.0, |s| (from / s).floor() * s);
    let mut index = 0;
    std::iter::from_fn(move || {
        sequence?;
        let start = offset;
        let next = offset + visuals[index].seconds;
        if next <= offset {
            return None;
        }
        let cell = (index, start, next);
        offset = next;
        index = (index + 1) % visuals.len();
        Some(cell)
    })
}
fn pieces(visuals: &[Visual], view: Viewport, duration: u64, width: f32) -> Option<Vec<Piece>> {
    let Some(sequence) = sequence_seconds(visuals) else {
        return Some(Vec::new());
    };
    let start = view.0.start_ms as f64 / 1000.0;
    let end = view.0.end_ms as f64 / 1000.0;
    if sequence / visuals.len() as f64 / (end - start) * f64::from(width) < 3.0 {
        return None;
    }
    let mut output = Vec::new();
    let mut reached = false;
    for (index, offset, next) in cells(visuals, start) {
        if offset >= end {
            reached = true;
            break;
        }
        if next > start {
            if output.len() == MAX_PIECES {
                return None;
            }
            output.push(Piece {
                index,
                start: offset.max(start),
                end: next.min(end),
                partial: next > duration as f64 / 1000.0,
            });
        }
    }
    // A cell that stopped advancing (precision exhausted) is not a lane.
    reached.then_some(output)
}

fn paint_pictures(ui: &egui::Ui, rect: Rect, view: Viewport, input: &Input<'_>, colors: Palette) {
    let painter = ui.painter_at(rect);
    let ms_x = |seconds: f64| {
        rect.left()
            + ((seconds * 1000.0 - view.0.start_ms as f64) / view.0.duration_ms() as f64) as f32
                * rect.width()
    };
    if let Some(pieces) = pieces(input.visuals, view, input.duration_ms, rect.width()) {
        for piece in pieces {
            let cell = Rect::from_min_max(
                pos2(ms_x(piece.start), rect.top()),
                pos2(ms_x(piece.end), rect.bottom()),
            );
            painter.rect_filled(
                cell.shrink2(vec2(0.5, 0.0)),
                0,
                colors.segments[input.visuals[piece.index].source_index % 3],
            );
            if cell.width() >= 14.0 {
                painter.with_clip_rect(cell).text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    &input.visuals[piece.index].label,
                    FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
            if piece.partial {
                for i in 0..((cell.width() + cell.height()) / 8.0).ceil() as usize {
                    let x = cell.left() - cell.height() + i as f32 * 8.0;
                    painter.with_clip_rect(cell).line_segment(
                        [pos2(x, cell.bottom()), pos2(x + cell.height(), cell.top())],
                        Stroke::new(1.0, ui.visuals().text_color().gamma_multiply(0.2)),
                    );
                }
            }
        }
    } else {
        painter.rect_filled(rect, 0, colors.segments[0]);
        let sequence: f64 = input.visuals.iter().map(|v| v.seconds).sum();
        let pass_width = (sequence * 1000.0 / view.0.duration_ms() as f64) as f32 * rect.width();
        let stride = (3.0 / pass_width.max(0.001)).ceil().max(1.0);
        let width = (pass_width * stride).max(3.0);
        let phase = ((view.0.start_ms as f64 / 1000.0 / sequence) as f32 / stride).fract() * width;
        for band in 0..((rect.width() + phase) / width).ceil().min(1024.0) as usize {
            if band % 2 == 1 {
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(rect.left() + band as f32 * width - phase, rect.top()),
                        pos2(
                            (rect.left() + (band + 1) as f32 * width - phase).min(rect.right()),
                            rect.bottom(),
                        ),
                    ),
                    0,
                    colors.segments[1],
                );
            }
        }
        let names = input
            .visuals
            .iter()
            .take(8)
            .map(|v| v.label.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!(
                "{names} × {}",
                (input.duration_ms as f64 / 1000.0 / sequence).ceil() as u64
            ),
            FontId::proportional(12.0),
            ui.visuals().text_color(),
        );
    }
    for (seconds, reverse) in [
        (input.fade_in_seconds, false),
        (input.fade_out_seconds, true),
    ] {
        if !seconds.is_finite() || seconds <= 0.0 {
            continue;
        }
        let seconds = seconds.min(input.duration_ms as f64 / 1000.0);
        let start = if reverse {
            input.duration_ms as f64 / 1000.0 - seconds
        } else {
            0.0
        };
        for band in 0..24 {
            let a = start + band as f64 / 24.0 * seconds;
            let b = start + (band + 1) as f64 / 24.0 * seconds;
            let fade = Rect::from_min_max(pos2(ms_x(a), rect.top()), pos2(ms_x(b), rect.bottom()))
                .intersect(rect);
            if fade.is_positive() {
                painter.rect_filled(
                    fade,
                    0,
                    Color32::BLACK.gamma_multiply(
                        (if reverse {
                            band as f32 / 24.0
                        } else {
                            1.0 - band as f32 / 24.0
                        }) * 0.65,
                    ),
                );
            }
        }
    }
}

/// "Extrait 5,0 s": the short's length in the interface language.
fn flag_text(range: Range, l: Language) -> String {
    l.format(
        "timeline.short_flag",
        &[l.decimal(range.duration_ms() as f64 / 1000.0, 1)],
    )
}
/// The flag is 20 px high and ends 6 px above the lanes, so the
/// playhead cap below never covers its text; it stays inside the stage width.
fn flag_rect(geometry: Geometry, left: f32, width: f32) -> Rect {
    let x = (left - 2.0)
        .max(geometry.ruler.left())
        .min((geometry.ruler.right() - width).max(geometry.ruler.left()));
    let bottom = geometry.ruler.bottom() - 6.0;
    Rect::from_min_max(pos2(x, bottom - 20.0), pos2(x + width, bottom))
}
/// Restart on: a 20 × 18 accent ↺ at the range start in the pictures lane,
/// over the lane (the lane itself never changes). Its tooltip and accessible
/// name say the short starts with picture A.
fn restart_mark(
    ui: &egui::Ui,
    id: Id,
    geometry: Geometry,
    view: Viewport,
    range: Range,
    colors: Palette,
    l: Language,
    enabled: bool,
) -> Option<Rect> {
    let left = x_at(view, geometry.stage, range.start_ms);
    if range.start_ms < view.0.start_ms || left > geometry.pictures.right() - 22.0 {
        return None;
    }
    let mark = Rect::from_min_size(
        pos2(left + 3.0, geometry.pictures.center().y - 9.0),
        vec2(20.0, 18.0),
    );
    let mut painter = ui.painter_at(geometry.stage);
    if !enabled {
        painter.multiply_opacity(0.45);
    }
    painter.rect_filled(mark, style::radius::S, colors.selection);
    crate::app_icons::paint(
        &painter,
        Rect::from_center_size(mark.center(), vec2(14.0, 14.0)),
        crate::app_icons::Icon::Restart,
        colors.on_selection,
    );
    let text = l.format("timeline.restart_mark", &[sequence_letter(0)]);
    let response = ui.interact(mark, id.with("restart-mark"), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
    response.on_hover_text(if enabled {
        text
    } else {
        l.text("bar.locked").into()
    });
    Some(mark)
}

/// The playhead's 10 × 6 cap, at the bottom of the ruler.
fn playhead_cap(geometry: Geometry, x: f32) -> Rect {
    Rect::from_center_size(pos2(x, geometry.ruler.bottom() - 3.0), vec2(10.0, 6.0))
}

fn paint_range(
    ui: &egui::Ui,
    id: Id,
    geometry: Geometry,
    view: Viewport,
    range: Range,
    colors: Palette,
    drag: Option<RangeDrag>,
    focus: Option<DragPart>,
    l: Language,
    duration: u64,
    enabled: bool,
) {
    if range.end_ms <= view.0.start_ms || range.start_ms >= view.0.end_ms {
        return;
    }
    // The flag above and the knobs beside the lanes may extend past the stage.
    let mut painter = ui.painter_at(geometry.stage.expand2(vec2(8.0, 10.0)));
    if !enabled {
        painter.multiply_opacity(0.45);
    }
    let left = x_at(view, geometry.stage, range.start_ms);
    let right = x_at(view, geometry.stage, range.end_ms);
    let lanes = geometry.lanes();
    // `accent-soft` body with a 2 px accent border (radius 6), 2 px past the lanes.
    let body = Rect::from_min_max(
        pos2(left.max(lanes.left()), lanes.top() - 2.0),
        pos2(right.min(lanes.right()), lanes.bottom() + 2.0),
    );
    painter.rect_filled(body, style::radius::M, colors.tint);
    painter.rect_stroke(
        body,
        style::radius::M,
        Stroke::new(2.0, colors.selection),
        egui::StrokeKind::Inside,
    );
    if focus == Some(DragPart::Body) {
        painter.rect_stroke(
            body.expand(2.0),
            style::radius::M,
            Stroke::new(2.0, colors.focus),
            egui::StrokeKind::Outside,
        );
    }
    // Handles: 10 × 32 knobs (28 compact) centred on the waveform lane.
    let knob_height = if geometry.wav.height() < 60.0 {
        28.0
    } else {
        32.0
    };
    for (x, part) in [(left, DragPart::Start), (right, DragPart::End)] {
        if x < lanes.left() || x > lanes.right() {
            continue;
        }
        let knob =
            Rect::from_center_size(pos2(x, geometry.wav.center().y), vec2(10.0, knob_height));
        // A 2 px lane-coloured outline keeps the knob visible over accent bars.
        painter.rect_filled(knob.expand(2.0), 7, colors.wav);
        painter.rect_filled(knob, 5, colors.selection);
        painter.line_segment(
            [
                pos2(x, knob.center().y - 6.0),
                pos2(x, knob.center().y + 6.0),
            ],
            Stroke::new(2.0, colors.on_selection),
        );
        if focus == Some(part) || drag.is_some_and(|d| d.part == part) {
            painter.rect_stroke(
                knob.expand(2.0),
                6,
                Stroke::new(2.0, colors.focus),
                egui::StrokeKind::Outside,
            );
        }
    }
    // Flag in the ruler lane at the range start, pinned inside the stage.
    let galley = painter.layout_no_wrap(
        flag_text(range, l),
        style::semibold(style::text::SMALL),
        colors.on_selection,
    );
    let flag_rect = flag_rect(geometry, left, galley.size().x + 16.0);
    painter.rect_filled(flag_rect, style::radius::S, colors.selection);
    painter.galley(
        pos2(
            flag_rect.left() + 8.0,
            flag_rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        colors.on_selection,
    );
    if !enabled {
        ui.interact(flag_rect, id.with("locked-flag-reason"), Sense::hover())
            .on_hover_text(l.text("bar.locked"));
    }
    if let Some(drag) = drag.filter(|drag| drag.moved) {
        let label = |ms| time_label(ms, true, duration >= 3_600_000, l);
        let text = match drag.part {
            DragPart::Start => label(range.start_ms),
            DragPart::End => label(range.end_ms),
            DragPart::Body => format!("{} – {}", label(range.start_ms), label(range.end_ms)),
        };
        let galley = painter.layout_no_wrap(
            text,
            FontId::proportional(style::text::SMALL),
            ui.visuals().text_color(),
        );
        let bubble = Rect::from_min_size(
            pos2(
                body.center().x - galley.size().x * 0.5 - 6.0,
                lanes.top() + 4.0,
            ),
            galley.size() + vec2(12.0, 4.0),
        );
        painter.rect_filled(bubble, style::radius::S, style::surface(ui));
        painter.rect_stroke(
            bubble,
            style::radius::S,
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            bubble.min + vec2(6.0, 2.0),
            galley,
            ui.visuals().text_color(),
        );
    }
}

fn overview(
    ui: &mut egui::Ui,
    id: Id,
    state: &mut TimelineState,
    mut view: Viewport,
    range: Option<Range>,
    duration: u64,
    colors: Palette,
) -> Viewport {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let response = ui.interact(rect, id.with("overview"), Sense::click_and_drag());
    let full = Viewport::full(duration).unwrap();
    let window = Rect::from_min_max(
        pos2(x_at(full, rect, view.0.start_ms), rect.top()),
        pos2(x_at(full, rect, view.0.end_ms), rect.bottom()),
    );
    if ui.input(|i| i.pointer.primary_pressed())
        && response.is_pointer_button_down_on()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        if !window.contains(pointer) {
            view = Viewport(
                Range::place(
                    time_at(full, rect, pointer.x).saturating_sub(view.0.duration_ms() / 2),
                    view.0.duration_ms(),
                    duration,
                )
                .unwrap(),
            );
        }
        state.overview = Some((view, pointer.x));
    }
    if let Some((original, anchor)) = state.overview {
        if let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) {
            view = original
                .pan(
                    (f64::from(pointer.x - anchor) / f64::from(rect.width()) * duration as f64)
                        .round() as i64,
                    duration,
                )
                .unwrap();
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            state.overview = None;
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, colors.wav);
    if let Some(range) = range {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x_at(full, rect, range.start_ms), rect.center().y - 2.0),
                pos2(
                    x_at(full, rect, range.end_ms).max(x_at(full, rect, range.start_ms) + 1.0),
                    rect.center().y + 2.0,
                ),
            ),
            0,
            colors.selection,
        );
    }
    let window = Rect::from_min_max(
        pos2(x_at(full, rect, view.0.start_ms), rect.top()),
        pos2(x_at(full, rect, view.0.end_ms), rect.bottom()),
    );
    painter.rect_filled(window, 2, colors.wave.gamma_multiply(0.2));
    painter.rect_stroke(
        window,
        2,
        Stroke::new(1.0, colors.wave),
        egui::StrokeKind::Inside,
    );
    response.on_hover_cursor(egui::CursorIcon::Grab);
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    /// Opt-in CPU frame cost, including egui tessellation, with a real envelope.
    /// Media extraction and font warmup are outside the measured samples.
    #[test]
    #[ignore = "opt-in timeline performance measurement; requires NOH_QA_WAV and NOH_FFMPEG"]
    fn timeline_hover_frame_cost() {
        let wav = std::env::var_os("NOH_QA_WAV").expect("NOH_QA_WAV");
        let ffmpeg = std::env::var_os("NOH_FFMPEG").expect("NOH_FFMPEG");
        let waveform = noh::waveform::extract(
            std::path::Path::new(&wav),
            std::path::Path::new(&ffmpeg),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let duration = (waveform.duration * 1000.0) as u64;
        for (width, scale) in [(980.0, 1.0), (1920.0, 2.0)] {
            let ctx = egui::Context::default();
            crate::app_locale::install_fonts(&ctx);
            ctx.set_pixels_per_point(scale);
            let locale = Locale::load();
            let mut state = TimelineState::default();
            let visuals = [Visual {
                label: "A".into(),
                seconds: 4.0,
                source_index: 0,
            }];
            let mut samples = Vec::new();
            for frame in 0..620 {
                let started = std::time::Instant::now();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0))),
                        events: vec![egui::Event::PointerMoved(pos2(
                            10.0 + (frame % 800) as f32,
                            80.0,
                        ))],
                        ..Default::default()
                    },
                    |ui| {
                        show(
                            ui,
                            &locale,
                            &mut state,
                            Input {
                                cursor_ms: Some((frame * 23) % duration),
                                duration_ms: duration,
                                range: Range::new(duration / 4, duration / 2, duration),
                                viewport: Viewport::full(duration),
                                waveform: Some(&waveform),
                                track: None,
                                visuals: &visuals,
                                waveform_loading: false,
                                waveform_error: None,
                                enabled: true,
                                fade_in_seconds: 1.0,
                                fade_out_seconds: 2.0,
                                restart: false,
                            },
                        );
                    },
                );
                std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
                if frame >= 20 {
                    samples.push(started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "TIMELINE width={width} scale={scale} frames={} p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
                samples.len(),
                samples[300],
                samples[570],
                samples[599]
            );
        }
    }

    struct Harness {
        ctx: egui::Context,
        locale: Locale,
        state: TimelineState,
        range: Option<Range>,
        view: Option<Viewport>,
        width: f32,
        enabled: bool,
        clip: Option<Rect>,
    }
    impl Harness {
        fn new(range: Option<Range>) -> Self {
            let mut locale = Locale::load();
            locale.language = Language::En;
            let mut result = Self {
                ctx: egui::Context::default(),
                locale,
                state: TimelineState::default(),
                range,
                view: Viewport::full(20_000),
                width: 900.0,
                enabled: true,
                clip: None,
            };
            // The range flag uses the SemiBold family: install the app's fonts.
            crate::app_locale::install_fonts(&result.ctx);
            result.frame(Vec::new());
            result.frame(Vec::new());
            result
        }
        fn frame(&mut self, mut events: Vec<egui::Event>) -> Output {
            let visuals = [
                Visual {
                    label: "A".into(),
                    seconds: 4.0,
                    source_index: 0,
                },
                Visual {
                    label: "B".into(),
                    seconds: 6.0,
                    source_index: 1,
                },
            ];
            let modifiers = events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { modifiers, .. }
                    | egui::Event::MouseWheel { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let mut result = None;
            let mut rendered = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, 420.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(clip) = self.clip {
                        ui.set_clip_rect(clip);
                    }
                    result = Some(show(
                        ui,
                        &self.locale,
                        &mut self.state,
                        Input {
                            cursor_ms: None,
                            duration_ms: 20_000,
                            range: self.range,
                            viewport: self.view,
                            waveform: None,
                            track: None,
                            visuals: &visuals,
                            waveform_loading: true,
                            waveform_error: None,
                            enabled: self.enabled,
                            fade_in_seconds: 1.0,
                            fade_out_seconds: 2.0,
                            restart: false,
                        },
                    ));
                },
            );
            rendered.textures_delta.clear();
            let output = result.unwrap();
            self.range = output.range;
            self.view = output.viewport;
            output
        }
        fn point(&self, ms: u64) -> Pos2 {
            let geometry = self.state.geometry.unwrap();
            pos2(
                x_at(self.view.unwrap(), geometry.stage, ms),
                geometry.wav.center().y,
            )
        }
        fn press(&mut self, point: Pos2) -> Output {
            self.frame(vec![egui::Event::PointerMoved(point)]);
            self.frame(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }])
        }
        fn release(&mut self, point: Pos2) -> Output {
            self.frame(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }])
        }
        fn key(&mut self, key: Key, shift: bool) -> Output {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    shift,
                    ..Default::default()
                },
            }])
        }
    }
    #[test]
    fn range_gestures_and_clipped_timeline_do_not_scrub_preview() {
        let mut h = Harness::new(Range::new(5000, 10_000, 20_000));
        let point = h.point(7000);
        assert!(
            h.frame(vec![egui::Event::PointerMoved(point)])
                .hover_ms
                .is_some()
        );
        assert!(h.press(point).hover_ms.is_none());
        let moved = point + vec2(80.0, 0.0);
        assert!(
            h.frame(vec![egui::Event::PointerMoved(moved)])
                .hover_ms
                .is_none()
        );
        assert!(h.release(moved).hover_ms.is_none());
        h.clip = Some(Rect::from_min_size(pos2(0.0, 300.0), vec2(900.0, 100.0)));
        assert!(
            h.frame(vec![egui::Event::PointerMoved(point)])
                .hover_ms
                .is_none()
        );
    }

    #[test]
    fn seeking_on_the_ruler_does_not_edit_the_short_range() {
        let range = Range::new(4_000, 8_000, 20_000);
        let mut harness = Harness::new(range);
        let ruler = harness.state.geometry.unwrap().ruler;
        let point = pos2(x_at(harness.view.unwrap(), ruler, 12_000), ruler.center().y);
        harness.press(point);
        let result = harness.release(point);
        assert!(result.seek_ms.is_some_and(|ms| ms.abs_diff(12_000) < 3));
        assert_eq!(result.range, range);
        assert!(!result.commit_changed);
    }

    #[test]
    fn pointer_drawing_requires_movement_and_commits_only_on_release() {
        let mut ui = Harness::new(None);
        let anchor = ui.point(5000);
        ui.press(anchor);
        let output = ui.frame(vec![egui::Event::PointerMoved(anchor + vec2(2.0, 0.0))]);
        assert!(output.range.is_none());
        assert!(!ui.release(anchor + vec2(2.0, 0.0)).commit_changed);
        ui.press(anchor);
        let end = ui.point(8000);
        let output = ui.frame(vec![egui::Event::PointerMoved(end)]);
        assert_eq!(output.range, Range::new(5000, 8000, 20_000));
        assert!(!output.commit_changed);
        assert!(ui.release(end).commit_changed);
        let output = ui.key(Key::ArrowRight, false);
        assert_eq!(output.range, Range::new(5100, 8100, 20_000));
        assert!(output.commit_changed);
    }

    #[test]
    fn actual_body_drag_preserves_grab_offset_clamps_and_escape_restores() {
        let original = Range::new(13_000, 18_000, 20_000);
        let mut ui = Harness::new(original);
        let anchor = ui.point(14_300);
        assert_eq!(ui.press(anchor).range, original);
        let limit = ui.point(20_000);
        assert_eq!(
            ui.frame(vec![egui::Event::PointerMoved(limit)]).range,
            Range::new(15_000, 20_000, 20_000)
        );
        assert_eq!(
            ui.frame(vec![egui::Event::PointerMoved(anchor)]).range,
            original
        );
        let moved = ui.point(13_300);
        assert_eq!(
            ui.frame(vec![egui::Event::PointerMoved(moved)]).range,
            Range::new(12_000, 17_000, 20_000)
        );
        let escaped = ui.key(Key::Escape, false);
        assert_eq!(escaped.range, original);
        assert!(!escaped.commit_changed);
        assert!(!ui.release(moved).commit_changed);
    }

    #[test]
    fn actual_handles_keep_the_other_boundary_and_cannot_cross() {
        let original = Range::new(5000, 10_000, 20_000);
        let mut ui = Harness::new(original);
        let start = ui.point(5000) - vec2(5.0, 0.0);
        assert_eq!(
            ui.press(start).range,
            original,
            "The handle hit area must not cause a jump"
        );
        let crossed = ui.point(12_000);
        let output = ui.frame(vec![egui::Event::PointerMoved(crossed)]);
        assert_eq!(output.range, Range::new(9900, 10_000, 20_000));
        assert!(ui.release(crossed).commit_changed);
        ui.range = original;
        ui.frame(Vec::new());
        let end = ui.point(10_000) + vec2(5.0, 0.0);
        assert_eq!(ui.press(end).range, original);
        let crossed = ui.point(1000);
        let output = ui.frame(vec![egui::Event::PointerMoved(crossed)]);
        assert_eq!(output.range, Range::new(5000, 5100, 20_000));
        assert!(ui.release(crossed).commit_changed);
    }

    #[test]
    fn keyboard_steps_home_end_and_locked_controls_use_real_focus() {
        let mut ui = Harness::new(Range::new(5000, 10_000, 20_000));
        let body = ui.point(7000);
        ui.press(body);
        ui.release(body);
        assert_eq!(
            ui.key(Key::ArrowLeft, false).range,
            Range::new(4900, 9900, 20_000)
        );
        assert_eq!(
            ui.key(Key::ArrowRight, true).range,
            Range::new(5900, 10_900, 20_000)
        );
        assert_eq!(
            ui.key(Key::PageUp, false).range,
            Range::new(15_000, 20_000, 20_000)
        );
        assert_eq!(ui.key(Key::Home, false).range, Range::new(0, 5000, 20_000));
        assert_eq!(
            ui.key(Key::End, false).range,
            Range::new(15_000, 20_000, 20_000)
        );
        ui.enabled = false;
        let output = ui.key(Key::Home, false);
        assert_eq!(output.range, Range::new(15_000, 20_000, 20_000));
        assert!(!output.commit_changed);
    }

    #[test]
    fn locked_range_dims_without_dimming_the_waveform_lane_or_disabling_pan() {
        let range = Range::new(5000, 10_000, 20_000).unwrap();
        let mut h = Harness::new(Some(range));
        h.enabled = false;
        let visuals = [Visual {
            label: "A".into(),
            seconds: 4.0,
            source_index: 0,
        }];
        let mut rendered = h.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 420.0))),
                ..Default::default()
            },
            |ui| {
                show(
                    ui,
                    &h.locale,
                    &mut h.state,
                    Input {
                        cursor_ms: None,
                        duration_ms: 20_000,
                        range: Some(range),
                        viewport: h.view,
                        waveform: None,
                        track: None,
                        visuals: &visuals,
                        waveform_loading: true,
                        waveform_error: None,
                        enabled: false,
                        fade_in_seconds: 1.0,
                        fade_out_seconds: 2.0,
                        restart: true,
                    },
                );
            },
        );
        rendered.textures_delta.clear();
        let geometry = h.state.geometry.unwrap();
        let fills: Vec<_> = rendered
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(shape) => Some((shape.rect, shape.fill)),
                _ => None,
            })
            .collect();
        let wav = fills
            .iter()
            .find(|(rect, _)| *rect == geometry.wav)
            .expect("waveform background painted");
        assert_eq!(
            wav.1,
            Palette::new(h.ctx.global_style().visuals.dark_mode).wav
        );
        let body = fills
            .iter()
            .find(|(rect, _)| {
                (rect.top() - (geometry.lanes().top() - 2.0)).abs() < 0.1
                    && (rect.bottom() - (geometry.lanes().bottom() + 2.0)).abs() < 0.1
            })
            .expect("short range painted");
        assert_eq!(
            body.1,
            Palette::new(h.ctx.global_style().visuals.dark_mode)
                .tint
                .gamma_multiply(0.45)
        );
        // Ctrl+wheel remains a viewport action while range editing is locked.
        let point = h.point(7000);
        h.frame(vec![egui::Event::PointerMoved(point)]);
        let before = h.view;
        h.frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            modifiers: egui::Modifiers::CTRL,
            phase: egui::TouchPhase::Move,
        }]);
        assert_ne!(h.view, before);
    }

    #[test]
    fn unavailable_waveform_retry_responds_to_a_real_click_while_range_is_locked() {
        let mut h = Harness::new(None);
        h.enabled = false;
        let visuals: [Visual; 0] = [];
        let mut frame = |events: Vec<egui::Event>| {
            let mut result = None;
            let mut rendered = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 420.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    result = Some(show(
                        ui,
                        &h.locale,
                        &mut h.state,
                        Input {
                            cursor_ms: None,
                            duration_ms: 20_000,
                            range: None,
                            viewport: h.view,
                            waveform: None,
                            track: None,
                            visuals: &visuals,
                            waveform_loading: false,
                            waveform_error: Some("Decode failed"),
                            enabled: false,
                            fade_in_seconds: 1.0,
                            fade_out_seconds: 2.0,
                            restart: false,
                        },
                    ));
                },
            );
            rendered.textures_delta.clear();
            let texts: Vec<_> = rendered
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            (result.unwrap(), texts, h.state.geometry.unwrap().wav)
        };
        let (_, texts, wav) = frame(vec![]);
        assert!(
            texts
                .iter()
                .any(|t| t == Language::En.text("timeline.wav_unavailable"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t == Language::En.text("timeline.retry"))
        );
        let point = pos2(wav.right() - 36.0, wav.center().y);
        frame(vec![egui::Event::PointerMoved(point)]);
        frame(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        let (out, _, _) = frame(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert!(out.retry);
    }

    #[test]
    fn locked_timeline_keeps_decoded_waveform_bars_at_full_opacity() {
        use std::{io::Write, sync::atomic::AtomicBool};
        let ffmpeg = match noh::inspection::resolve_ffmpeg(std::path::Path::new("")) {
            Ok(path) => path,
            Err(error) => {
                assert!(
                    std::env::var_os("NOH_MEDIA_TESTS").is_none(),
                    "media suite requires FFmpeg: {error}"
                );
                eprintln!("SKIPPED: decoded waveform opacity needs FFmpeg");
                return;
            }
        };
        let folder = tempfile::tempdir().unwrap();
        let wav = folder.path().join("constant.wav");
        let mut file = std::fs::File::create(&wav).unwrap();
        let samples = 8000_u32;
        let bytes = samples * 2;
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&8000_u32.to_le_bytes()).unwrap();
        file.write_all(&16000_u32.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        for _ in 0..samples {
            file.write_all(&16_000_i16.to_le_bytes()).unwrap();
        }
        drop(file);
        let waveform = noh::waveform::extract(&wav, &ffmpeg, &AtomicBool::new(false)).unwrap();
        let mut h = Harness::new(Some(Range::new(250, 750, 1000).unwrap()));
        let visuals: [Visual; 0] = [];
        let mut rendered = h.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 420.0))),
                ..Default::default()
            },
            |ui| {
                show(
                    ui,
                    &h.locale,
                    &mut h.state,
                    Input {
                        cursor_ms: None,
                        duration_ms: 1000,
                        range: Range::new(250, 750, 1000),
                        viewport: Viewport::full(1000),
                        waveform: Some(&waveform),
                        track: None,
                        visuals: &visuals,
                        waveform_loading: false,
                        waveform_error: None,
                        enabled: false,
                        fade_in_seconds: 1.0,
                        fade_out_seconds: 2.0,
                        restart: false,
                    },
                );
            },
        );
        rendered.textures_delta.clear();
        let wav_rect = h.state.geometry.unwrap().wav;
        let palette = Palette::new(h.ctx.global_style().visuals.dark_mode);
        let bars: Vec<_> = rendered
            .shapes
            .iter()
            .flat_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) => mesh
                    .vertices
                    .iter()
                    .filter(|v| wav_rect.contains(v.pos) && v.color != Color32::TRANSPARENT)
                    .map(|v| v.color)
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        assert!(bars.iter().filter(|&&c| c == palette.wave).count() > 100);
        assert!(bars.iter().filter(|&&c| c == palette.selection).count() > 100);
        assert!(bars.iter().all(|c| c.a() == 255));
    }

    #[test]
    fn ctrl_wheel_zooms_about_pointer_plain_wheel_keeps_view_and_no_edit_commit() {
        let mut ui = Harness::new(Range::new(5000, 10_000, 20_000));
        let point = ui.point(7000);
        ui.frame(vec![egui::Event::PointerMoved(point)]);
        let wheel = |modifiers| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            modifiers,
            phase: egui::TouchPhase::Move,
        };
        let output = ui.frame(vec![wheel(egui::Modifiers {
            ctrl: true,
            ..Default::default()
        })]);
        assert_eq!(output.viewport.unwrap().0.duration_ms(), 16_000);
        assert_eq!(
            output.hover_ms,
            Some(time_at(
                output.viewport.unwrap(),
                ui.state.geometry.unwrap().stage,
                point.x
            ))
        );
        assert!(!output.commit_changed);
        let before = ui.view;
        assert_eq!(ui.frame(vec![wheel(Default::default())]).viewport, before);
    }

    #[test]
    fn zoom_floor_short_focus_and_dense_sequences_are_bounded() {
        let full = Viewport::full(7_200_000).unwrap();
        assert_eq!(zoom(full, 1e9, 0.5, 7_200_000).0.duration_ms(), 2000);
        assert_eq!(
            zoom(Viewport::full(50).unwrap(), 100.0, 0.5, 50)
                .0
                .duration_ms(),
            50
        );
        assert_eq!(
            focus_range(Range::new(13_000, 18_000, 20_000).unwrap(), 20_000)
                .unwrap()
                .0,
            Range::new(11_500, 19_500, 20_000).unwrap()
        );
        assert!(
            pieces(
                &[Visual {
                    label: "A".into(),
                    seconds: 0.001,
                    source_index: 0,
                }],
                full,
                7_200_000,
                900.0
            )
            .is_none()
        );
        let visuals = [
            Visual {
                label: "A".into(),
                seconds: 4.0,
                source_index: 0,
            },
            Visual {
                label: "B".into(),
                seconds: 6.0,
                source_index: 1,
            },
        ];
        let pieces = pieces(
            &visuals,
            Viewport(Range::new(13_000, 18_000, 20_000).unwrap()),
            20_000,
            900.0,
        )
        .unwrap();
        assert_eq!(pieces.len(), 2);
        assert_eq!(
            (pieces[0].index, pieces[0].start, pieces[0].end),
            (0, 13.0, 14.0)
        );
        assert_eq!(
            (pieces[1].index, pieces[1].start, pieces[1].end),
            (1, 14.0, 18.0)
        );
    }

    #[test]
    fn short_flag_uses_the_interface_decimal_and_is_never_covered_by_the_playhead() {
        let range = Range::new(13_000, 18_000, 182_400).unwrap();
        for (language, text) in [
            (Language::De, "Kurzclip 5,0 s"),
            (Language::Fr, "Extrait 5,0 s"),
            (Language::Es, "Clip corto 5,0 s"),
            (Language::En, "Short 5.0 s"),
            (Language::Ja, "ショート 5.0 秒"),
        ] {
            assert_eq!(flag_text(range, language), text);
        }
        // The flag ends 6 px above the lanes, where the cap begins: a
        // playhead under the flag (e.g. at 0:15,2) must not hide the comma.
        for compact in [false, true] {
            let geometry = Geometry::new(
                Rect::from_min_size(pos2(24.0, 100.0), vec2(932.0, 200.0)),
                compact,
                true,
            );
            let flag = flag_rect(geometry, 420.0, 110.0);
            assert_eq!(flag.height(), 20.0);
            assert_eq!(flag.bottom(), geometry.pictures.top() - 6.0);
            for x in (0..=110).map(|dx| 420.0 + dx as f32) {
                let cap = playhead_cap(geometry, x);
                assert!(
                    flag.bottom() <= cap.top(),
                    "compact={compact} x={x}: {flag:?} {cap:?}"
                );
            }
        }
    }

    #[test]
    fn compact_and_wide_geometry_stays_inside_all_localized_themes() {
        let mut ui = Harness::new(Range::new(5000, 10_000, 20_000));
        for language in Language::ALL {
            ui.locale.language = language;
            for dark in [true, false] {
                ui.ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                for width in [320.0, 980.0] {
                    ui.width = width;
                    ui.frame(Vec::new());
                    let geometry = ui.state.geometry.unwrap();
                    assert!(geometry.stage.right() <= width + 0.5);
                    assert_eq!(
                        geometry.wav.height(),
                        if width < 560.0 { 48.0 } else { 64.0 }
                    );
                    assert!(geometry.wav.bottom() <= geometry.stage.bottom());
                }
            }
        }
    }
}
