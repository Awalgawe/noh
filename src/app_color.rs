//! One colour component for the whole app (approved design, 2026-11-25): a
//! row of the caller's swatches, then « Autre couleur… », which unfolds an
//! inline picker under the row (saturation × brightness, hue, hex code and the
//! recent hand-made colours). The style panel's colour settings and the
//! « Promote NOH » corner menu both draw it; only one picker is open at a time.
use super::*;
use crate::app_style as style;
use egui::{Color32, Id, Key, Mesh, Modifiers, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

/// A swatch cell, its gap, and its chip's radius.
const CELL: f32 = 28.0;
const GAP: f32 = 6.0;
const CHIP: f32 = 11.0;
/// The saturation × brightness area's height and corner radius.
const AREA_H: f32 = 120.0;
const AREA_R: f32 = 8.0;
/// The picker's widest (the corner menu is narrower; a wide style panel is not).
const PICKER_MAX: f32 = 360.0;
/// The hue strip: its hit area's height and its track's height.
const STRIP_HIT: f32 = 20.0;
const STRIP_H: f32 = 12.0;
/// A knob's radius.
const KNOB: f32 = 7.0;
/// The hex field.
const HEX: Vec2 = vec2(112.0, 28.0);
/// A recent colour's cell and chip radius.
const RECENT_CELL: f32 = 22.0;
const RECENT_CHIP: f32 = 9.0;
/// The caller's label usually sits this far above the row: shown with it.
const LABEL_ABOVE: f32 = 24.0;
/// How many recent colours are kept.
pub const RECENT_MAX: usize = 4;
/// The one picker open in the app (egui memory).
const OPEN: &str = "noh-colour-picker";

/// What the person did with the component this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The colour to apply now (live while the picker moves).
    pub color: Option<Color32>,
    /// The colour came from a swatch of the row.
    pub from_row: bool,
    /// The colour is the one chosen (a click, a drag's end, a key, a full hex
    /// code, Escape): not a step of a drag. Callers that save save then.
    pub settled: bool,
    /// A hand-made colour to keep among the recent ones.
    pub remember: Option<Remember>,
}

/// A hand-made colour for the recent ones; it `replaces` the one the same
/// opening of the picker put there before, so one opening leaves one colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remember {
    pub color: Color32,
    pub replaces: Option<Color32>,
}

/// The open picker: whose it is, the colour before it opened (Escape brings
/// it back), the colour as hue, saturation and brightness (kept so the hue
/// survives grey and black), the hex text, and the recent colour this opening
/// added.
#[derive(Clone, Debug)]
struct Open {
    owner: Id,
    before: Color32,
    hsv: [f32; 3],
    shown: Color32,
    hex: String,
    entry: Option<Color32>,
    pass: u64,
    /// Just unfolded: scroll it into view (the style panel is short).
    reveal: bool,
}

fn outline() -> Color32 {
    Color32::from_rgba_unmultiplied(127, 127, 127, 140)
}

/// Open `owner`'s picker on `current` (a capture shows it this way).
pub fn open(ctx: &egui::Context, owner: Id, current: Color32) {
    let open = Open {
        owner,
        before: current,
        hsv: hsv_of(current),
        shown: current,
        hex: hex_text(current),
        entry: None,
        pass: ctx.cumulative_pass_nr(),
        reveal: true,
    };
    ctx.data_mut(|d| d.insert_temp(Id::new(OPEN), open));
}

/// `owner`'s picker is the open one.
pub fn is_open(ctx: &egui::Context, owner: Id) -> bool {
    ctx.data(|d| d.get_temp::<Open>(Id::new(OPEN)))
        .is_some_and(|o| o.owner == owner)
}

/// Recent colours a capture shows (never saved).
pub fn capture_recent() -> Vec<Color32> {
    vec![
        Color32::from_rgb(0xE0, 0x6C, 0x3C),
        Color32::from_rgb(0x7A, 0x5C, 0xFF),
    ]
}

fn close(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Open>(Id::new(OPEN)));
}

/// `owner`'s open picker, if it is the open one and was drawn last pass (a
/// picker whose menu or panel went away is folded).
fn opened(ctx: &egui::Context, owner: Id) -> Option<Open> {
    let open = ctx.data(|d| d.get_temp::<Open>(Id::new(OPEN)))?;
    let now = ctx.cumulative_pass_nr();
    (open.owner == owner && open.pass + 1 >= now).then_some(open)
}

/// The row, then the picker when it is unfolded. `swatches` are the caller's
/// colours with their names (the hex code names an unnamed one); `recent` the
/// app's recent hand-made colours, newest first.
pub fn colour_row(
    ui: &mut egui::Ui,
    owner: Id,
    swatches: &[(Color32, Option<&str>)],
    current: Color32,
    recent: &[Color32],
    l: noh::i18n::Language,
) -> Outcome {
    let mut out = Outcome::default();
    let mut state = opened(ui.ctx(), owner);
    let row_top = ui.cursor().top();
    let known = swatches.iter().any(|(c, _)| *c == current);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        ui.horizontal_wrapped(|ui| {
            for (c, name) in swatches {
                let name = name.map_or_else(|| hex(*c), str::to_owned);
                if swatch(ui, *c, *c == current, &name).clicked() {
                    out.color = Some(*c);
                    out.from_row = true;
                    out.settled = true;
                    state = None;
                }
            }
            let other = other_cell(
                ui,
                (!known).then_some(current),
                state.is_some(),
                l.text("style.other_color"),
            );
            if other.clicked() {
                state = match state {
                    Some(_) => None,
                    None => {
                        open(ui.ctx(), owner, current);
                        opened(ui.ctx(), owner)
                    }
                };
            }
        });
    });
    match state {
        Some(mut open) => {
            ui.add_space(8.0 - ui.spacing().item_spacing.y);
            // Never wider than PICKER_MAX: a wide panel keeps it left-aligned.
            let keep = ui
                .scope(|ui| {
                    ui.set_max_width(ui.available_width().min(PICKER_MAX));
                    picker(ui, &mut open, current, swatches, recent, l, &mut out)
                })
                .inner;
            // Just unfolded: bring it into view (the style panel may be shorter
            // than the picker): with the caller's label when all fits, else
            // from the row, else from the label down.
            if std::mem::take(&mut open.reveal) {
                let x = ui.min_rect().x_range();
                let bottom = ui.cursor().top();
                let labelled = Rect::from_x_y_ranges(x, row_top - LABEL_ABOVE..=bottom);
                let row = Rect::from_x_y_ranges(x, row_top..=bottom);
                let visible = ui.clip_rect().height();
                if labelled.height() <= visible {
                    ui.scroll_to_rect(labelled, None);
                } else if row.height() <= visible {
                    ui.scroll_to_rect(row, None);
                } else {
                    ui.scroll_to_rect(labelled, Some(egui::Align::TOP));
                }
            }
            if keep {
                open.pass = ui.ctx().cumulative_pass_nr();
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(OPEN), open));
            } else {
                close(ui.ctx());
            }
        }
        None => {
            if ui
                .ctx()
                .data(|d| d.get_temp::<Open>(Id::new(OPEN)))
                .is_some_and(|o| o.owner == owner)
            {
                close(ui.ctx());
            }
        }
    }
    out
}

/// A swatch: a circle chip with a neutral outline, ringed when current.
fn swatch(ui: &mut egui::Ui, color: Color32, current: bool, name: &str) -> egui::Response {
    let t = style::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, current, name)
    });
    let c = rect.center();
    let p = ui.painter();
    if response.hovered() && !current {
        p.circle_stroke(c, 14.0, Stroke::new(1.5, t.border_strong));
    }
    p.circle_filled(c, CHIP, color);
    p.circle_stroke(c, CHIP - 0.5, Stroke::new(1.0, outline()));
    if current {
        p.circle_stroke(c, 13.0, Stroke::new(2.0, t.text));
    }
    style::focus(ui, &response);
    response.on_hover_text(name)
}

/// « Autre couleur… »: a rainbow ring around the current colour when it is
/// none of the row's, else a « + »; it folds or unfolds the picker.
fn other_cell(
    ui: &mut egui::Ui,
    custom: Option<Color32>,
    unfolded: bool,
    name: &str,
) -> egui::Response {
    let t = style::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, custom.is_some(), name)
    });
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_expanded(unfolded));
    let c = rect.center();
    let p = ui.painter();
    if unfolded {
        p.circle_filled(c, 14.0, t.surface_2);
    } else if response.hovered() && custom.is_none() {
        p.circle_stroke(c, 14.0, Stroke::new(1.5, t.border_strong));
    }
    p.add(rainbow(c, CHIP, 7.5));
    p.circle_stroke(c, CHIP - 0.5, Stroke::new(1.0, outline()));
    match custom {
        Some(color) => {
            p.circle_filled(c, 7.0, color);
            p.circle_stroke(c, 13.0, Stroke::new(2.0, t.text));
        }
        None => {
            p.circle_filled(c, 7.0, t.surface_2);
            let ink = Stroke::new(1.5, t.text_2);
            p.line_segment([c - vec2(3.5, 0.0), c + vec2(3.5, 0.0)], ink);
            p.line_segment([c - vec2(0.0, 3.5), c + vec2(0.0, 3.5)], ink);
        }
    }
    style::focus(ui, &response);
    response.on_hover_text(name)
}

/// The unfolded picker. False when it folds.
fn picker(
    ui: &mut egui::Ui,
    open: &mut Open,
    current: Color32,
    swatches: &[(Color32, Option<&str>)],
    recent: &[Color32],
    l: noh::i18n::Language,
    out: &mut Outcome,
) -> bool {
    let t = style::tokens(ui);
    // The colour changed elsewhere (a swatch, an undo): follow it.
    if current != open.shown {
        open.hsv = hsv_of(current);
        open.shown = current;
        open.hex = hex_text(current);
    }
    let width = ui.available_width();
    let mut changed = false;
    let mut commit = false;
    let mut fold = false;

    // Saturation (left to right) × brightness (top to bottom).
    let (area, response) = ui.allocate_exact_size(vec2(width, AREA_H), Sense::click_and_drag());
    let [h, s, _] = open.hsv;
    let label = l.text("color.area");
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(s * 100.0), label));
    if let Some(pos) = response.interact_pointer_pos() {
        let s = ((pos.x - area.left()) / area.width()).clamp(0.0, 1.0);
        let v = 1.0 - ((pos.y - area.top()) / area.height()).clamp(0.0, 1.0);
        open.hsv = [h, s, v];
        changed = true;
    }
    commit |= response.drag_stopped() || response.clicked();
    if response.has_focus() {
        lock_arrows(ui, response.id);
        let (dx, dy) = arrows(ui);
        if dx != 0.0 || dy != 0.0 {
            open.hsv[1] = (open.hsv[1] + dx / 100.0).clamp(0.0, 1.0);
            open.hsv[2] = (open.hsv[2] - dy / 100.0).clamp(0.0, 1.0);
            changed = true;
            commit = true;
        }
        fold |= ui.input(|i| i.key_pressed(Key::Enter));
    }
    ui.painter().add(rounded_mesh(area, AREA_R, 8, 8, |p| {
        let s = (p.x - area.left()) / area.width();
        let v = 1.0 - (p.y - area.top()) / area.height();
        rgb_of([open.hsv[0], s, v])
    }));
    ui.painter().rect_stroke(
        area,
        AREA_R,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(127, 127, 127, 102)),
        egui::StrokeKind::Inside,
    );
    let at = pos2(
        area.left() + open.hsv[1] * area.width(),
        area.top() + (1.0 - open.hsv[2]) * area.height(),
    );
    knob(ui, at, rgb_of(open.hsv));
    style::focus(ui, &response);

    // Hue, red to red.
    ui.add_space(10.0 - ui.spacing().item_spacing.y);
    let (strip, response) = ui.allocate_exact_size(vec2(width, STRIP_HIT), Sense::click_and_drag());
    let label = l.text("color.hue");
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(open.hsv[0] * 360.0), label));
    if let Some(pos) = response.interact_pointer_pos() {
        open.hsv[0] = ((pos.x - strip.left()) / strip.width()).clamp(0.0, 1.0);
        changed = true;
    }
    commit |= response.drag_stopped() || response.clicked();
    if response.has_focus() {
        lock_arrows(ui, response.id);
        let (dx, dy) = arrows(ui);
        let step = dx - dy;
        if step != 0.0 {
            open.hsv[0] = (open.hsv[0] + step / 360.0).clamp(0.0, 1.0);
            changed = true;
            commit = true;
        }
        fold |= ui.input(|i| i.key_pressed(Key::Enter));
    }
    let track = Rect::from_center_size(strip.center(), vec2(width, STRIP_H));
    ui.painter()
        .add(rounded_mesh(track, STRIP_H / 2.0, 36, 1, |p| {
            rgb_of([(p.x - track.left()) / track.width(), 1.0, 1.0])
        }));
    let x = track.left() + open.hsv[0] * track.width();
    knob(
        ui,
        pos2(
            x.clamp(track.left() + KNOB, track.right() - KNOB),
            strip.center().y,
        ),
        rgb_of([open.hsv[0], 1.0, 1.0]),
    );
    style::focus(ui, &response);

    if changed {
        let c = rgb_of(open.hsv);
        open.shown = c;
        open.hex = hex_text(c);
        out.color = Some(c);
    }

    // The hex code, then the recent colours.
    ui.add_space(10.0 - ui.spacing().item_spacing.y);
    let label_w = ui
        .painter()
        .layout_no_wrap(
            l.text("color.recent").into(),
            egui::FontId::proportional(style::text::SMALL),
            t.text_2,
        )
        .size()
        .x;
    let recent_w = if recent.is_empty() {
        0.0
    } else {
        8.0 + label_w + 4.0 + recent.len() as f32 * RECENT_CELL
    };
    let one_line = HEX.x + recent_w <= width;
    let mut picked_recent = None;
    ui.horizontal(|ui| {
        let typed = hex_field(ui, &mut open.hex, l);
        if typed.changed()
            && let Some(c) = parse_hex(&open.hex)
        {
            open.hsv = hsv_of(c);
            open.shown = c;
            out.color = Some(c);
            commit = true;
        }
        if typed.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            fold = true;
        }
        if one_line && !recent.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                picked_recent = recent_chips(ui, recent, l);
            });
        }
    });
    if !one_line && !recent.is_empty() {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                picked_recent = recent_chips(ui, recent, l);
            });
        });
    }
    if let Some(c) = picked_recent {
        open.hsv = hsv_of(c);
        open.shown = c;
        open.hex = hex_text(c);
        out.color = Some(c);
        out.settled = true;
    }

    out.settled |= commit;
    // A drag's end may move nothing: the colour is still handed over, settled.
    if commit && out.color.is_none() {
        out.color = Some(open.shown);
    }
    if commit && !out.from_row {
        let c = open.shown;
        if !swatches.iter().any(|(s, _)| *s == c) {
            out.remember = Some(Remember {
                color: c,
                replaces: open.entry,
            });
            open.entry = Some(c);
        }
    }
    // Escape brings back the colour from before the picker opened and folds
    // it (the menu around it stays open).
    if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        if open.before != current {
            out.color = Some(open.before);
        }
        out.settled = true;
        out.remember = None;
        return false;
    }
    !fold
}

/// The hex field: a chip of the colour, « # », and six characters; a
/// malformed code is outlined in the error colour.
fn hex_field(ui: &mut egui::Ui, text: &mut String, l: noh::i18n::Language) -> egui::Response {
    let t = style::tokens(ui);
    let (rect, _) = ui.allocate_exact_size(HEX, Sense::hover());
    let valid = parse_hex(text);
    ui.painter().rect_filled(rect, style::radius::M, t.input);
    ui.painter().rect_stroke(
        rect,
        style::radius::M,
        Stroke::new(
            1.0,
            if valid.is_some() {
                t.border_strong
            } else {
                t.error
            },
        ),
        egui::StrokeKind::Inside,
    );
    let chip = Rect::from_center_size(
        pos2(rect.left() + 8.0 + 7.0, rect.center().y),
        Vec2::splat(14.0),
    );
    if let Some(c) = valid {
        ui.painter().rect_filled(chip, 3.0, c);
    }
    ui.painter().rect_stroke(
        chip,
        3.0,
        Stroke::new(1.0, outline()),
        egui::StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(13.0);
    let hash = ui
        .painter()
        .text(
            pos2(chip.right() + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "#",
            font.clone(),
            t.text_3,
        )
        .right();
    let inner = Rect::from_min_max(
        pos2(hash + 2.0, rect.top()),
        pos2(rect.right() - 6.0, rect.bottom()),
    );
    let edit = ui.put(
        inner,
        egui::TextEdit::singleline(text)
            .frame(egui::Frame::NONE)
            .char_limit(6)
            .font(font)
            .vertical_align(egui::Align::Center)
            .margin(Vec2::ZERO),
    );
    let name = l.text("color.hex");
    edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, name));
    if edit.changed() {
        let clean: String = text
            .chars()
            .filter(char::is_ascii_hexdigit)
            .map(|c| c.to_ascii_uppercase())
            .take(6)
            .collect();
        *text = clean;
    }
    edit
}

/// « Récentes » and the recent chips, newest first, in a right-to-left
/// layout. The colour clicked, if one was.
fn recent_chips(ui: &mut egui::Ui, recent: &[Color32], l: noh::i18n::Language) -> Option<Color32> {
    let t = style::tokens(ui);
    let mut picked = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    for &c in recent.iter().rev() {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(RECENT_CELL), Sense::click());
        let name = hex(c);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
        let p = ui.painter();
        if response.hovered() {
            p.circle_stroke(
                rect.center(),
                RECENT_CHIP + 1.5,
                Stroke::new(1.5, t.border_strong),
            );
        }
        p.circle_filled(rect.center(), RECENT_CHIP, c);
        p.circle_stroke(
            rect.center(),
            RECENT_CHIP - 0.5,
            Stroke::new(1.0, outline()),
        );
        style::focus(ui, &response);
        if response.on_hover_text(name).clicked() {
            picked = Some(c);
        }
    }
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(l.text("color.recent"))
            .size(style::text::SMALL)
            .color(t.text_2),
    );
    picked
}

/// A knob: the colour, a white ring, then a faint black ring.
fn knob(ui: &egui::Ui, at: Pos2, color: Color32) {
    // Over the edges of a scrolled panel too: a knob at the area's side is
    // half outside it.
    let p = ui
        .painter()
        .with_clip_rect(ui.clip_rect().expand(KNOB + 1.0));
    p.circle_filled(at, KNOB, color);
    p.circle_stroke(at, KNOB - 1.0, Stroke::new(2.0, Color32::WHITE));
    p.circle_stroke(
        at,
        KNOB + 0.5,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 115)),
    );
}

/// Arrow keys keep to the focused area instead of moving the focus.
fn lock_arrows(ui: &egui::Ui, id: Id) {
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        )
    });
}

/// The arrow keys pressed this frame, ±1 each (±10 with Shift): x right, y down.
fn arrows(ui: &egui::Ui) -> (f32, f32) {
    ui.input(|i| {
        let step = if i.modifiers.shift { 10.0 } else { 1.0 };
        let key = |k| if i.key_pressed(k) { step } else { 0.0 };
        (
            key(Key::ArrowRight) - key(Key::ArrowLeft),
            key(Key::ArrowDown) - key(Key::ArrowUp),
        )
    })
}

/// A rounded rectangle filled by `color` at each vertex: rows follow the
/// corner arcs, `cols` columns across, `mid` rows between the arcs.
fn rounded_mesh(
    rect: Rect,
    r: f32,
    cols: usize,
    mid: usize,
    color: impl Fn(Pos2) -> Color32,
) -> Mesh {
    let r = r.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let arc: Vec<f32> = (0..=4)
        .map(|k| r * (1.0 - (k as f32 * std::f32::consts::FRAC_PI_8).cos()))
        .collect();
    let mut ys: Vec<f32> = arc.iter().map(|d| rect.top() + d).collect();
    for k in 1..mid {
        ys.push(egui::lerp(
            rect.top() + r..=rect.bottom() - r,
            k as f32 / mid as f32,
        ));
    }
    ys.extend(arc.iter().rev().map(|d| rect.bottom() - d));
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    let mut mesh = Mesh::default();
    for &y in &ys {
        let dy = (y - rect.top()).min(rect.bottom() - y);
        let inset = if dy < r {
            r - (r * r - (r - dy) * (r - dy)).max(0.0).sqrt()
        } else {
            0.0
        };
        for j in 0..=cols {
            let x = egui::lerp(
                rect.left() + inset..=rect.right() - inset,
                j as f32 / cols as f32,
            );
            let p = pos2(x, y);
            mesh.colored_vertex(p, color(p));
        }
    }
    let w = cols as u32 + 1;
    for i in 0..ys.len() as u32 - 1 {
        for j in 0..cols as u32 {
            let a = i * w + j;
            mesh.add_triangle(a, a + 1, a + w);
            mesh.add_triangle(a + 1, a + w + 1, a + w);
        }
    }
    mesh
}

/// The « Autre couleur » ring: the hues around the circle, red at the top.
fn rainbow(c: Pos2, outer: f32, inner: f32) -> Mesh {
    let n = 24;
    let mut mesh = Mesh::default();
    for k in 0..=n {
        let a = k as f32 / n as f32;
        let angle = a * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let dir = vec2(angle.cos(), angle.sin());
        let col = rgb_of([a, 0.85, 0.9]);
        mesh.colored_vertex(c + dir * outer, col);
        mesh.colored_vertex(c + dir * inner, col);
    }
    for k in 0..n as u32 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    mesh
}

/// `#RRGGBB`.
pub fn hex(c: Color32) -> String {
    format!("#{}", hex_text(c))
}

fn hex_text(c: Color32) -> String {
    format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// Six hex digits, with or without `#`.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let s = text.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Hue, saturation and brightness (0 to 1) of an sRGB colour.
fn hsv_of(c: Color32) -> [f32; 3] {
    let [r, g, b] = [c.r(), c.g(), c.b()].map(|x| f32::from(x) / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, s, max]
}

/// The sRGB colour of a hue, saturation and brightness (0 to 1).
fn rgb_of([h, s, v]: [f32; 3]) -> Color32 {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let u = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(u(r), u(g), u(b))
}

/// The recent colours' file, beside the language file
/// (`<config>/noh/recent-colours`): up to four `#RRGGBB`, newest first.
pub fn path(language: &std::path::Path) -> std::path::PathBuf {
    language.with_file_name("recent-colours")
}

/// The recent colours, as last saved.
pub fn read(path: Option<&std::path::Path>) -> Vec<Color32> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(parse_hex)
        .take(RECENT_MAX)
        .collect()
}

fn save(path: &std::path::Path, recent: &[Color32]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let words: Vec<String> = recent.iter().map(|c| hex(*c)).collect();
    std::fs::write(path, words.join(" "))
}

/// The recent colours after `r`: newest first, no repeat, at most four.
fn remembered(recent: &[Color32], r: Remember) -> Vec<Color32> {
    std::iter::once(r.color)
        .chain(
            recent
                .iter()
                .copied()
                .filter(|c| *c != r.color && Some(*c) != r.replaces),
        )
        .take(RECENT_MAX)
        .collect()
}

impl NohApp {
    /// Keep a hand-made colour among the recent ones, for every colour
    /// component of the app: remembered across launches.
    pub(super) fn remember_color(&mut self, r: Remember) {
        self.recent_colors = remembered(&self.recent_colors, r);
        if let Some(language) = self.locale.path.as_deref()
            && let Err(e) = save(&path(language), &self.recent_colors)
        {
            eprintln!("recent colours: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips_every_byte_colour_it_is_given() {
        for c in [
            Color32::from_rgb(0, 0, 0),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(0x5F, 0xCA, 0xBE),
            Color32::from_rgb(0xE9, 0xC4, 0x6A),
            Color32::from_rgb(0xF0, 0xAC, 0x8B),
            Color32::from_rgb(12, 200, 255),
            Color32::from_rgb(255, 0, 128),
        ] {
            assert_eq!(rgb_of(hsv_of(c)), c);
        }
        assert_eq!(rgb_of([0.0, 1.0, 1.0]), Color32::from_rgb(255, 0, 0));
        assert_eq!(rgb_of([1.0 / 3.0, 1.0, 1.0]), Color32::from_rgb(0, 255, 0));
        assert_eq!(rgb_of([0.5, 0.0, 0.5]), Color32::from_rgb(128, 128, 128));
    }

    #[test]
    fn a_hex_code_needs_six_digits() {
        assert_eq!(
            parse_hex("#5FCABE"),
            Some(Color32::from_rgb(0x5F, 0xCA, 0xBE))
        );
        assert_eq!(
            parse_hex("5fcabe"),
            Some(Color32::from_rgb(0x5F, 0xCA, 0xBE))
        );
        assert_eq!(parse_hex("5FCAB"), None);
        assert_eq!(parse_hex("5FCABG"), None);
        assert_eq!(hex(Color32::from_rgb(1, 2, 255)), "#0102FF");
    }

    #[test]
    fn one_opening_of_the_picker_leaves_one_recent_colour() {
        let a = Color32::from_rgb(1, 1, 1);
        let b = Color32::from_rgb(2, 2, 2);
        let c = Color32::from_rgb(3, 3, 3);
        let d = Color32::from_rgb(4, 4, 4);
        let e = Color32::from_rgb(5, 5, 5);
        let recent = vec![a, b, c, d];
        // A new colour goes first and the oldest goes.
        let one = remembered(
            &recent,
            Remember {
                color: e,
                replaces: None,
            },
        );
        assert_eq!(one, vec![e, a, b, c]);
        // The same opening moves on: its colour is replaced, not added.
        let f = Color32::from_rgb(6, 6, 6);
        assert_eq!(
            remembered(
                &one,
                Remember {
                    color: f,
                    replaces: Some(e)
                }
            ),
            vec![f, a, b, c]
        );
        // A colour already there comes first, once.
        assert_eq!(
            remembered(
                &recent,
                Remember {
                    color: c,
                    replaces: None
                }
            ),
            vec![c, a, b, d]
        );
    }

    #[test]
    fn recent_colours_are_saved_and_read_back() {
        let folder = tempfile::tempdir().unwrap();
        let file = path(&folder.path().join("noh").join("language"));
        assert!(read(Some(&file)).is_empty());
        let colours = [
            Color32::from_rgb(0x5F, 0xCA, 0xBE),
            Color32::from_rgb(1, 2, 3),
        ];
        save(&file, &colours).unwrap();
        assert_eq!(read(Some(&file)), colours);
        std::fs::write(&file, "#000001 nope #000002 #000003 #000004 #000005").unwrap();
        assert_eq!(read(Some(&file)).len(), RECENT_MAX);
    }

    #[test]
    fn the_component_texts_exist_in_every_language() {
        for language in noh::i18n::Language::ALL {
            for key in [
                "color.area",
                "color.hue",
                "color.hex",
                "color.recent",
                "style.other_color",
            ] {
                assert!(
                    !language.text(key).contains("missing"),
                    "{} {key}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn the_rounded_area_mesh_stays_inside_its_rectangle() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(248.0, AREA_H));
        let mesh = rounded_mesh(rect, AREA_R, 8, 8, |_| Color32::WHITE);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| rect.expand(0.01).contains(v.pos))
        );
        assert!(mesh.is_valid());
        let strip = rounded_mesh(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(248.0, STRIP_H)),
            6.0,
            36,
            1,
            |_| Color32::RED,
        );
        assert!(strip.is_valid());
    }
}
