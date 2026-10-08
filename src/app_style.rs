//! Presentation tokens and small native components.
//! Colours, type and radii follow the NOH design system (`assets/ui-tokens.json`).
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, RichText, Shadow, Stroke, Ui, Vec2,
};
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
}
pub const CONTROL_H: f32 = 32.0;
pub const CONTROL_H_LG: f32 = 36.0;
pub const ROW_ICON: f32 = 28.0;
pub const HEADER_H: f32 = 48.0;
pub const COMPACT_BELOW: f32 = 560.0;
pub const FADE_VALUE_W: f32 = 80.0;
pub const PROGRESS_H: f32 = 6.0;
pub const PROGRESS_MAX_W: f32 = 420.0;
pub mod radius {
    pub const S: u8 = 4;
    pub const M: u8 = 6;
    pub const L: u8 = 8;
    pub const XL: u8 = 12;
}
pub mod text {
    pub const TITLE: f32 = 22.0;
    pub const HEADING: f32 = 17.0;
    pub const BODY: f32 = 14.0;
    pub const SMALL: f32 = 12.0;
}

/// One theme of the design system. Names follow `assets/ui-tokens.json`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub surface_3: Color32,
    pub seg_on: Color32,
    pub input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_2: Color32,
    pub text_3: Color32,
    pub wave: Color32,
    pub cue: Color32,
    pub monitor: Color32,
    pub focus: Color32,
    pub info: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub ok: Color32,
    pub warn_soft: Color32,
    pub error_soft: Color32,
    pub scrim: Color32,
    pub seq: [Color32; 3],
    pub seq_ink: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub shadow: Shadow,
}
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
/// `alpha` is the CSS opacity in hundredths.
const fn rgba(hex: u32, alpha: u32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(
        (hex >> 16) as u8,
        (hex >> 8) as u8,
        hex as u8,
        ((alpha * 255 + 50) / 100) as u8,
    )
}
pub const DARK: Tokens = Tokens {
    bg: rgb(0x17181A),
    surface: rgb(0x1F2023),
    surface_2: rgb(0x292A2E),
    surface_3: rgb(0x33353A),
    seg_on: rgb(0x3A3C41),
    input: rgb(0x121315),
    border: rgb(0x34363A),
    border_strong: rgb(0x6A6C72),
    text: rgb(0xEDEEEA),
    text_2: rgb(0xB1B3AE),
    text_3: rgb(0x989A94),
    wave: rgb(0x6F726D),
    cue: rgb(0x8D8F8A),
    monitor: rgb(0x000000),
    focus: rgb(0x8AB8FF),
    info: rgb(0x8AB8FF),
    warn: rgb(0xE9C46A),
    error: rgb(0xF4948A),
    ok: rgb(0x8FD18C),
    warn_soft: rgba(0xE9C46A, 14),
    error_soft: rgba(0xF4948A, 12),
    scrim: rgba(0x000000, 55),
    // The third sequence colour keeps seq-ink contrast at or above 4.5:1.
    seq: [rgb(0x4E6A8A), rgb(0x7E5F45), rgb(0x53704C)],
    seq_ink: rgb(0xEDEEEA),
    accent: rgb(0x5FCABE),
    on_accent: rgb(0x06241F),
    accent_text: rgb(0x7AD6CB),
    accent_soft: rgba(0x5FCABE, 16),
    shadow: Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: rgba(0x000000, 45),
    },
};
pub const LIGHT: Tokens = Tokens {
    bg: rgb(0xF4F4F1),
    surface: rgb(0xFFFFFF),
    surface_2: rgb(0xEBEBE7),
    surface_3: rgb(0xE1E1DC),
    seg_on: rgb(0xFFFFFF),
    input: rgb(0xFFFFFF),
    border: rgb(0xD9D8D2),
    border_strong: rgb(0x878680),
    text: rgb(0x1B1C1E),
    text_2: rgb(0x53565A),
    text_3: rgb(0x63666B),
    wave: rgb(0x8A8C87),
    cue: rgb(0x7C7F83),
    monitor: rgb(0x000000),
    focus: rgb(0x1F6FEB),
    info: rgb(0x1F5FBF),
    warn: rgb(0x8A5A00),
    error: rgb(0xB3261E),
    ok: rgb(0x2E7D32),
    warn_soft: rgb(0xF6EEDC),
    error_soft: rgb(0xFBEAE8),
    scrim: rgba(0x18181A, 32),
    seq: [rgb(0xC9D7E8), rgb(0xE8D5C4), rgb(0xCFE0CB)],
    seq_ink: rgb(0x1B1C1E),
    accent: rgb(0x0B6C66),
    on_accent: rgb(0xFFFFFF),
    accent_text: rgb(0x0A635D),
    accent_soft: rgba(0x0B6C66, 12),
    shadow: Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: rgba(0x18181A, 18),
    },
};
pub fn tokens_for(dark: bool) -> &'static Tokens {
    if dark { &DARK } else { &LIGHT }
}
pub fn tokens(ui: &Ui) -> &'static Tokens {
    tokens_for(ui.visuals().dark_mode)
}

pub fn background(ui: &Ui) -> Color32 {
    tokens(ui).bg
}
pub fn surface(ui: &Ui) -> Color32 {
    tokens(ui).surface
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(semibold(size))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Success,
}
pub fn color(ui: &Ui, severity: Severity) -> Color32 {
    let t = tokens(ui);
    match severity {
        Severity::Info => t.info,
        Severity::Warning => t.warn,
        Severity::Error => t.error,
        Severity::Success => t.ok,
    }
}
pub fn icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "ⓘ",
        Severity::Warning => "⚠",
        Severity::Error => "×",
        Severity::Success => "✓",
    }
}
pub fn accent(ui: &Ui) -> Color32 {
    tokens(ui).accent
}
pub fn focus(ui: &Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            radius::M,
            Stroke::new(2.0, tokens(ui).focus),
            egui::StrokeKind::Outside,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `accent` fill, `on-accent` 14/600: the main action of its zone.
    Primary,
    /// `surface-2` fill.
    Secondary,
    /// No fill: low emphasis.
    Quiet,
}
/// A design-system button. Disabled buttons keep their label, show `text-3`
/// on `surface-2` (not a faded accent) and stay non-interactive.
pub fn button_kind(
    ui: &mut Ui,
    label: impl Into<String>,
    enabled: bool,
    kind: Kind,
    min_width: f32,
    height: f32,
) -> egui::Response {
    let t = tokens(ui);
    let label = label.into();
    let (fill, text) = match (kind, enabled) {
        (Kind::Quiet, false) => (Color32::TRANSPARENT, RichText::new(label).color(t.text_3)),
        (_, false) => (t.surface_2, RichText::new(label).color(t.text_3)),
        (Kind::Primary, true) => (t.accent, strong(label, text::BODY).color(t.on_accent)),
        (Kind::Secondary, true) => (t.surface_2, RichText::new(label).color(t.text)),
        (Kind::Quiet, true) => (Color32::TRANSPARENT, RichText::new(label).color(t.text)),
    };
    let button = egui::Button::new(text)
        .fill(fill)
        // A transparent 1 px stroke keeps egui's button size (the bar pre-measures it).
        .stroke(Stroke::new(1.0, Color32::TRANSPARENT))
        .corner_radius(radius::M)
        .min_size(Vec2::new(min_width, height));
    // Draw the specified disabled colours instead of egui's global fade. No child
    // scope: it would stop wrapping and right-to-left rows from placing the button.
    let saved = ui.visuals().disabled_alpha;
    ui.visuals_mut().disabled_alpha = 1.0;
    let response = ui.add_enabled(enabled, button);
    ui.visuals_mut().disabled_alpha = saved;
    focus(ui, &response);
    response
}
pub fn button(
    ui: &mut Ui,
    label: impl Into<String>,
    enabled: bool,
    primary: bool,
    width: f32,
) -> egui::Response {
    let kind = if primary {
        Kind::Primary
    } else {
        Kind::Secondary
    };
    button_kind(ui, label, enabled, kind, width, CONTROL_H)
}
/// Icon button (`ibtn`): transparent, `text-2` glyph, `surface-2` on hover.
/// The name is both the tooltip and the accessible label.
pub fn icon_button(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    name: &str,
    side: f32,
) -> egui::Response {
    let t = tokens(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, radius::M, t.surface_2);
    }
    let color = if ui.is_enabled() { t.text_2 } else { t.text_3 };
    crate::app_icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
        icon,
        color,
    );
    focus(ui, &response);
    response.on_hover_text(name)
}
/// Floating layers (menus, popovers): `surface`, 1 px `border`, radius 12,
/// overlay shadow, 6 px inset.
pub fn popup_frame(ui: &Ui) -> egui::Frame {
    let t = tokens(ui);
    egui::Frame::NONE
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(radius::XL)
        .shadow(t.shadow)
        .inner_margin(6.0)
}
/// Menu item: 18 px icon, 14 px title and an optional 12 px sub-line;
/// `surface-2` on hover or focus.
pub fn menu_item(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    title: &str,
    sub: Option<&str>,
    enabled: bool,
) -> egui::Response {
    menu_entry(ui, icon, title, sub, enabled, false)
}
/// The menu's main action: accent icon on its own
/// line, 600 title, sub-line, on `surface-2`.
pub fn menu_item_primary(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    title: &str,
    sub: &str,
    enabled: bool,
) -> egui::Response {
    menu_entry(ui, icon, title, Some(sub), enabled, true)
}
fn menu_entry(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    title: &str,
    sub: Option<&str>,
    enabled: bool,
    primary: bool,
) -> egui::Response {
    let t = tokens(ui);
    let width = ui.available_width().max(160.0);
    let color = if enabled { t.text } else { t.text_3 };
    // Texts wrap: the height follows them, so long translations never overlap.
    let text_left = if primary { 16.0 } else { 40.0 };
    let wrap = width - text_left - 10.0;
    let title_galley = ui.painter().layout(
        title.into(),
        if primary {
            semibold(text::BODY)
        } else {
            FontId::proportional(text::BODY)
        },
        color,
        wrap,
    );
    let sub_galley = sub.map(|sub| {
        ui.painter().layout(
            sub.into(),
            FontId::proportional(text::SMALL),
            t.text_2,
            wrap,
        )
    });
    let title_top = if primary { 38.0 } else { 8.0 };
    let height = (title_top
        + title_galley.size().y
        + sub_galley.as_ref().map_or(0.0, |g| 3.0 + g.size().y)
        + 9.0)
        .max(36.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, title));
    if primary || (enabled && (response.hovered() || response.has_focus())) {
        let fill = if primary && enabled && (response.hovered() || response.has_focus()) {
            t.surface_3
        } else {
            t.surface_2
        };
        ui.painter().rect_filled(rect, radius::M, fill);
    }
    let icon_color = match (enabled, primary) {
        (false, _) => t.text_3,
        (true, true) => t.accent,
        (true, false) => t.text_2,
    };
    let icon_offset = if primary {
        Vec2::new(16.0, 12.0)
    } else {
        Vec2::new(10.0, 9.0)
    };
    crate::app_icons::paint(
        ui.painter(),
        egui::Rect::from_min_size(rect.min + icon_offset, Vec2::splat(18.0)),
        icon,
        icon_color,
    );
    let title_h = title_galley.size().y;
    ui.painter().galley(
        egui::pos2(rect.left() + text_left, rect.top() + title_top),
        title_galley,
        color,
    );
    if let Some(galley) = sub_galley {
        ui.painter().galley(
            egui::pos2(
                rect.left() + text_left,
                rect.top() + title_top + title_h + 3.0,
            ),
            galley,
            t.text_2,
        );
    }
    focus(ui, &response);
    response
}
/// Menu row with a trailing switch (`menuitemcheckbox`): the whole row toggles.
pub fn menu_switch(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    title: &str,
    on: &mut bool,
    enabled: bool,
) -> egui::Response {
    let t = tokens(ui);
    let width = ui.available_width().max(160.0);
    let color = if enabled { t.text } else { t.text_3 };
    let galley = ui.painter().layout(
        title.into(),
        FontId::proportional(text::BODY),
        color,
        width - 40.0 - 52.0,
    );
    let height = (galley.size().y + 16.0).max(36.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, title)
    });
    if enabled && (response.hovered() || response.has_focus()) {
        ui.painter().rect_filled(rect, radius::M, t.surface_2);
    }
    crate::app_icons::paint(
        ui.painter(),
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.center().y - 9.0),
            Vec2::splat(18.0),
        ),
        icon,
        if enabled { t.text_2 } else { t.text_3 },
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 40.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    let track = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 42.0, rect.center().y - 9.0),
        Vec2::new(32.0, 18.0),
    );
    let track_fill = match (*on, enabled) {
        (true, true) => t.accent,
        (true, false) => t.accent.gamma_multiply(0.5),
        (false, _) => t.surface_3,
    };
    ui.painter().rect_filled(track, 9, track_fill);
    ui.painter().circle_filled(
        egui::pos2(
            if *on {
                track.right() - 9.0
            } else {
                track.left() + 9.0
            },
            track.center().y,
        ),
        7.0,
        if *on { t.on_accent } else { t.text_2 },
    );
    focus(ui, &response);
    response
}
/// Button with a leading 18 px icon (`+ Choisir un extrait`). Quiet: no fill
/// until hovered; secondary: `surface-2`.
pub fn icon_text_button(
    ui: &mut Ui,
    icon: crate::app_icons::Icon,
    label: &str,
    kind: Kind,
    enabled: bool,
) -> egui::Response {
    let t = tokens(ui);
    let color = if enabled { t.text } else { t.text_3 };
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), FontId::proportional(text::BODY), color);
    let padding = if kind == Kind::Quiet { 10.0 } else { 14.0 };
    let size = Vec2::new(padding * 2.0 + 18.0 + space::S + galley.size().x, CONTROL_H);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let fill = match kind {
        Kind::Primary if enabled => t.accent,
        Kind::Quiet if !(enabled && response.hovered()) => Color32::TRANSPARENT,
        _ => t.surface_2,
    };
    ui.painter().rect_filled(rect, radius::M, fill);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + padding, rect.center().y - 9.0),
        Vec2::splat(18.0),
    );
    crate::app_icons::paint(ui.painter(), icon_rect, icon, color);
    ui.painter().galley(
        egui::pos2(
            icon_rect.right() + space::S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    focus(ui, &response);
    response
}
/// Segmented control: `surface-2` track, the selected segment on `seg-on`.
/// Returns the newly selected index, if any.
pub fn segmented(ui: &mut Ui, id: &str, options: &[&str], selected: usize) -> Option<usize> {
    let t = tokens(ui);
    let mut clicked = None;
    ui.push_id(id, |ui| {
        egui::Frame::NONE
            .fill(t.surface_2)
            .corner_radius(radius::L)
            .inner_margin(2.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // Segments are 28 high inside the 2 px inset: a 32 px control overall.
                ui.spacing_mut().interact_size.y = ROW_ICON;
                ui.spacing_mut().button_padding = Vec2::new(12.0, 0.0);
                ui.horizontal(|ui| {
                    for (index, option) in options.iter().enumerate() {
                        let on = index == selected;
                        let label = if on {
                            strong(*option, text::BODY).color(t.text)
                        } else {
                            RichText::new(*option).color(t.text_2)
                        };
                        let response = ui.add(
                            egui::Button::new(label)
                                .fill(if on { t.seg_on } else { Color32::TRANSPARENT })
                                .stroke(Stroke::new(1.0, Color32::TRANSPARENT))
                                .corner_radius(radius::M)
                                .min_size(Vec2::new(0.0, ROW_ICON)),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::RadioButton,
                                true,
                                on,
                                *option,
                            )
                        });
                        focus(ui, &response);
                        if response.clicked() && !on {
                            clicked = Some(index);
                        }
                    }
                });
            });
    });
    clicked
}
/// Width of [`segmented`] whichever option is selected (each measured at its
/// wider, selected weight), for layouts decided before it is drawn.
pub fn segmented_width(ui: &Ui, options: &[&str]) -> f32 {
    let width = |font: FontId, text: &str| {
        ui.painter()
            .layout_no_wrap(text.into(), font, Color32::WHITE)
            .size()
            .x
    };
    // Each segment: its text, 12 px padding and the 1 px (transparent) stroke
    // on both sides.
    let segments: f32 = options
        .iter()
        .map(|o| {
            width(semibold(text::BODY), o).max(width(FontId::proportional(text::BODY), o)) + 26.0
        })
        .sum();
    segments + 2.0 * (options.len().saturating_sub(1)) as f32 + 4.0
}
/// Checkbox: 18 px box, `border-strong` outline, `accent` fill when checked.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> egui::Response {
    let t = tokens(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(label.into(), FontId::proportional(text::BODY), t.text);
    let size = Vec2::new(18.0 + space::S + galley.size().x, CONTROL_H);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *checked, label)
    });
    let boxed = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - 9.0),
        Vec2::splat(18.0),
    );
    let painter = ui.painter();
    if *checked {
        painter.rect_filled(boxed, radius::S, t.accent);
        painter.add(egui::Shape::line(
            vec![
                boxed.left_top() + Vec2::new(3.75, 9.4),
                boxed.left_top() + Vec2::new(7.1, 12.75),
                boxed.left_top() + Vec2::new(14.25, 5.6),
            ],
            Stroke::new(1.75, t.on_accent),
        ));
    } else {
        painter.rect_stroke(
            boxed.shrink(0.75),
            radius::S,
            Stroke::new(1.5, t.border_strong),
            egui::StrokeKind::Middle,
        );
    }
    painter.galley(
        egui::pos2(
            boxed.right() + space::S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.text,
    );
    focus(ui, &response);
    response
}
/// Switch: 32 × 18 track; `accent` when on.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = tokens(ui);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(32.0, 18.0), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label)
    });
    let painter = ui.painter();
    painter.rect_filled(rect, 9, if *on { t.accent } else { t.surface_3 });
    let knob = if *on {
        rect.right() - 9.0
    } else {
        rect.left() + 9.0
    };
    painter.circle_filled(
        egui::pos2(knob, rect.center().y),
        7.0,
        if *on { t.on_accent } else { t.text_2 },
    );
    focus(ui, &response);
    response
}
/// Tag: 18 high, `warn` on `warn-soft`, or neutral on `surface-2`.
pub fn tag(ui: &mut Ui, label: &str, warn: bool) -> egui::Response {
    let t = tokens(ui);
    let (fill, color) = if warn {
        (t.warn_soft, t.warn)
    } else {
        (t.surface_2, t.text_2)
    };
    // Painted at its fixed 18 px height: a Frame would stretch to the row height.
    let galley =
        ui.painter()
            .layout_no_wrap(label.into(), FontId::proportional(text::SMALL), color);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 12.0, 18.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, radius::S, fill);
    ui.painter().galley(
        egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    response
}
pub fn message(ui: &mut Ui, severity: Severity, text: &str, block: bool) {
    let c = color(ui, severity);
    let contents = |ui: &mut Ui| {
        ui.horizontal_top(|ui| {
            ui.label(RichText::new(icon(severity)).color(c));
            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(c)).wrap());
        });
    };
    if block {
        egui::Frame::new()
            .fill(c.gamma_multiply(0.08))
            .stroke(Stroke::new(1.0, c.gamma_multiply(0.4)))
            .corner_radius(radius::M)
            .inner_margin(8.0)
            .show(ui, contents);
    } else {
        contents(ui);
    }
}
/// A raised zone. Backgrounds carry the structure: no border.
#[allow(dead_code)]
pub fn card<R>(
    ui: &mut Ui,
    id: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.push_id(id, |ui| {
        egui::Frame::new()
            .fill(surface(ui))
            .corner_radius(radius::L)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width((ui.available_width()).max(0.0));
                ui.spacing_mut().item_spacing = Vec2::new(space::S, space::S);
                contents(ui)
            })
    })
    .inner
}
#[allow(dead_code)]
pub fn header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(CONTROL_H);
        ui.label(strong(title, 15.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
    ui.add_space(space::XS);
}
#[allow(dead_code)]
pub fn form_row<R>(
    ui: &mut Ui,
    label: &str,
    label_width: f32,
    compact: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    if compact || label_width > 168.0 {
        ui.label(label);
        ui.add_space(space::XS);
        contents(ui)
    } else {
        ui.horizontal_top(|ui| {
            ui.allocate_ui(
                Vec2::new(label_width.clamp(112.0, 168.0), CONTROL_H),
                |ui| {
                    ui.add_space(6.0);
                    ui.label(label);
                },
            );
            ui.add_space(space::S);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                contents(ui)
            })
            .inner
        })
        .inner
    }
}
fn configure(style: &mut egui::Style) {
    let t = tokens_for(style.visuals.dark_mode);
    style.spacing.item_spacing = Vec2::new(space::S, space::S);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.interact_size.y = CONTROL_H;
    for (text_style, font) in [
        (egui::TextStyle::Body, FontId::proportional(text::BODY)),
        (egui::TextStyle::Button, FontId::proportional(text::BODY)),
        (egui::TextStyle::Small, FontId::proportional(text::SMALL)),
        (egui::TextStyle::Heading, semibold(text::HEADING)),
        (egui::TextStyle::Monospace, FontId::monospace(text::SMALL)),
    ] {
        style.text_styles.insert(text_style, font);
    }
    let v = &mut style.visuals;
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.faint_bg_color = t.surface;
    v.extreme_bg_color = t.input;
    v.text_edit_bg_color = Some(t.input);
    v.code_bg_color = t.surface_2;
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_2);
    v.hyperlink_color = t.accent_text;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.error;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.text);
    v.text_cursor.stroke = Stroke::new(2.0, t.text);
    v.window_corner_radius = CornerRadius::same(radius::XL);
    v.menu_corner_radius = CornerRadius::same(radius::XL);
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_shadow = t.shadow;
    v.popup_shadow = t.shadow;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.surface;
    w.noninteractive.weak_bg_fill = t.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    // Inputs keep a `border-strong` outline; design-system buttons draw none.
    for (state, fill, stroke) in [
        (&mut w.inactive, t.surface_2, t.border_strong),
        (&mut w.hovered, t.surface_3, t.border_strong),
        (&mut w.active, t.surface_3, t.text_2),
        (&mut w.open, t.surface_2, t.border_strong),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.5, t.text);
        state.corner_radius = CornerRadius::same(radius::M);
        state.expansion = 0.0;
    }
}
/// Apply the design system to both themes; each follows its own `dark_mode`.
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(configure);
}

#[cfg(test)]
mod token_tests {
    use super::*;

    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }
    /// WCAG contrast of an opaque foreground over an opaque background.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The ratios promised by the usage notes in `assets/ui-tokens.json`, in both themes.
    #[test]
    fn token_pairs_meet_the_design_system_contrast_claims() {
        let mut failures = Vec::new();
        for (name, t) in [("dark", DARK), ("light", LIGHT)] {
            let claims: &[(&str, Color32, Color32, f64)] = &[
                ("text/bg", t.text, t.bg, 13.0),
                ("text/surface", t.text, t.surface, 13.0),
                ("text/surface-2", t.text, t.surface_2, 12.0),
                ("text-2/bg", t.text_2, t.bg, 6.0),
                ("text-2/surface", t.text_2, t.surface, 6.0),
                ("text-2/surface-2", t.text_2, t.surface_2, 6.0),
                ("text-3/bg", t.text_3, t.bg, 4.5),
                ("text-3/surface-2", t.text_3, t.surface_2, 4.5),
                ("border-strong/bg", t.border_strong, t.bg, 3.0),
                ("border-strong/input", t.border_strong, t.input, 3.0),
                ("wave/surface", t.wave, t.surface, 3.0),
                ("on-accent/accent", t.on_accent, t.accent, 6.0),
                ("seq-ink/seq-1", t.seq_ink, t.seq[0], 4.5),
                ("seq-ink/seq-2", t.seq_ink, t.seq[1], 4.5),
                ("seq-ink/seq-3", t.seq_ink, t.seq[2], 4.5),
            ];
            for (pair, fg, bg, minimum) in claims {
                let ratio = contrast(*fg, *bg);
                if ratio < *minimum {
                    failures.push(format!("{name} {pair}: {ratio:.4} < {minimum}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// Every colour token equals the checked-in design token file.
    #[test]
    fn tokens_match_the_repository_tokens_file() {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../assets/ui-tokens.json")).unwrap();
        let parse = |css: &str| -> Color32 {
            if let Some(hex) = css.strip_prefix('#') {
                return rgb(u32::from_str_radix(hex, 16).unwrap());
            }
            let inner = css.trim_start_matches("rgba(").trim_end_matches(')');
            let parts: Vec<f64> = inner
                .split(',')
                .map(|p| p.trim().parse().unwrap())
                .collect();
            let hex = ((parts[0] as u32) << 16) | ((parts[1] as u32) << 8) | parts[2] as u32;
            rgba(hex, (parts[3] * 100.0).round() as u32)
        };
        let field = |t: &Tokens, name: &str| -> Option<Color32> {
            Some(match name {
                "bg" => t.bg,
                "surface" => t.surface,
                "surface-2" => t.surface_2,
                "surface-3" => t.surface_3,
                "seg-on" => t.seg_on,
                "input" => t.input,
                "border" => t.border,
                "border-strong" => t.border_strong,
                "text" => t.text,
                "text-2" => t.text_2,
                "text-3" => t.text_3,
                "wave" => t.wave,
                "cue" => t.cue,
                "monitor" => t.monitor,
                "focus" => t.focus,
                "info" => t.info,
                "warn" => t.warn,
                "error" => t.error,
                "ok" => t.ok,
                "warn-soft" => t.warn_soft,
                "error-soft" => t.error_soft,
                "scrim" => t.scrim,
                "seq-1" => t.seq[0],
                "seq-2" => t.seq[1],
                "seq-3" => t.seq[2],
                "seq-ink" => t.seq_ink,
                "accent" => t.accent,
                "on-accent" => t.on_accent,
                "accent-text" => t.accent_text,
                "accent-soft" => t.accent_soft,
                // The alternative accents are not used by the application.
                _ => return None,
            })
        };
        let mut checked = 0;
        for token in file["color"]["tokens"].as_array().unwrap() {
            let name = token["name"].as_str().unwrap();
            for (theme, tokens) in [("dark", &DARK), ("light", &LIGHT)] {
                let value = token["value"][theme]
                    .as_str()
                    .or_else(|| token["value"].as_str())
                    .unwrap();
                if let Some(actual) = field(tokens, name) {
                    assert_eq!(actual, parse(value), "{theme} {name}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 60, "30 colour tokens in two themes");
    }

    #[test]
    fn accent_and_translucent_tokens_match_the_design_system() {
        assert_eq!(DARK.accent, Color32::from_rgb(0x5F, 0xCA, 0xBE));
        assert_eq!(LIGHT.accent, Color32::from_rgb(0x0B, 0x6C, 0x66));
        // rgba(95, 202, 190, 0.16): 0.16 × 255 ≈ 41; rgba(24, 24, 26, 0.32) ≈ 82.
        assert_eq!(DARK.accent_soft.a(), 41);
        assert_eq!(LIGHT.scrim.a(), 82);
    }

    #[test]
    fn components_keep_the_design_system_geometry_and_disabled_state() {
        let ctx = egui::Context::default();
        crate::app_locale::install_fonts(&ctx);
        apply(&ctx);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.horizontal(|ui| {
                assert_eq!(tag(ui, "anciens réglages", true).rect.height(), 18.0);
                assert_eq!(
                    switch(ui, &mut true, "on").rect.size(),
                    Vec2::new(32.0, 18.0)
                );
                let primary = button_kind(ui, "Exporter", true, Kind::Primary, 0.0, CONTROL_H_LG);
                assert_eq!(primary.rect.height(), CONTROL_H_LG);
                let disabled = button_kind(ui, "Exporter", false, Kind::Primary, 0.0, CONTROL_H);
                assert!(
                    !disabled.enabled(),
                    "a disabled button must stay non-interactive"
                );
            });
            // The disabled override must not leak into later widgets.
            assert_eq!(ui.visuals().disabled_alpha, 0.5);
            let before = ui.cursor().top();
            let track = ui
                .scope(|ui| segmented(ui, "seg", &["Vidéo", "Extrait"], 0))
                .response
                .rect;
            // 28 px segments inside a 2 px inset track.
            assert_eq!(track.height(), CONTROL_H);
            assert!(track.top() >= before);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn apply_styles_each_theme_from_its_own_tokens() {
        let ctx = egui::Context::default();
        apply(&ctx);
        ctx.set_theme(egui::Theme::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, DARK.bg);
        ctx.set_theme(egui::Theme::Light);
        assert_eq!(ctx.global_style().visuals.panel_fill, LIGHT.bg);
        assert_eq!(
            ctx.global_style().visuals.hyperlink_color,
            LIGHT.accent_text
        );
    }
}

#[cfg(test)]
mod feasibility_tests {
    //! Font coverage, compact layout and primitive geometry regressions.
    use super::*;
    use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

    fn input(events: Vec<Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, 850.0))),
            events,
            ..Default::default()
        }
    }
    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }
    fn click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn long_japanese_labels_wrap_inside_the_asset_column() {
        let ctx = egui::Context::default();
        crate::app_locale::install_fonts(&ctx);
        ctx.begin_pass(Default::default());
        let text = "このコンピューターで処理します。あとで確認してください。ファイルはすべてこのコンピューター内で処理されます";
        for width in [296.0, 356.0] {
            let galley = ctx.fonts_mut(|fonts| {
                fonts.layout(
                    text.into(),
                    FontId::proportional(14.0),
                    Color32::WHITE,
                    width,
                )
            });
            assert!(galley.rows.len() > 1, "no wrap at {width}");
            assert!(
                galley.size().x <= width + 0.5,
                "{} > {width}",
                galley.size().x
            );
        }
        ctx.end_pass().textures_delta.clear();
    }

    struct ModalProbe {
        below_clicked: bool,
        close: bool,
        modal_rect: Rect,
    }
    fn modal_frame(ctx: &egui::Context, events: Vec<Event>) -> ModalProbe {
        let mut probe = ModalProbe {
            below_clicked: false,
            close: false,
            modal_rect: Rect::NOTHING,
        };
        let mut output = ctx.run_ui(input(events), |ui| {
            probe.below_clicked = ui
                .put(
                    Rect::from_min_size(pos2(8.0, 8.0), vec2(120.0, 32.0)),
                    egui::Button::new("below"),
                )
                .clicked();
            let modal = egui::Modal::new(egui::Id::new("sheet")).show(ui.ctx(), |ui| {
                ui.set_width(460.0);
                ui.label("Emplacement de la vidéo");
            });
            probe.modal_rect = modal.response.rect;
            probe.close = modal.should_close();
        });
        output.textures_delta.clear();
        probe
    }

    #[test]
    fn modal_blocks_input_below_and_closes_on_escape_or_backdrop() {
        let ctx = egui::Context::default();
        modal_frame(&ctx, vec![]);
        let open = modal_frame(&ctx, vec![]);
        assert!(!open.close);
        assert!(!open.modal_rect.contains(pos2(60.0, 24.0)));
        let backdrop = modal_frame(&ctx, click(pos2(60.0, 24.0)));
        assert!(!backdrop.below_clicked, "input reached the layer below");
        assert!(backdrop.close, "backdrop click must close");
        let escape = modal_frame(&ctx, vec![key(Key::Escape)]);
        assert!(escape.close, "Escape must close");
    }

    #[derive(Debug)]
    struct MenuProbe {
        open: bool,
        toggle: Rect,
        item_rect: Rect,
        item_clicked: bool,
        first_focused: bool,
    }
    fn menu_frame(ctx: &egui::Context, events: Vec<Event>) -> MenuProbe {
        let mut probe = MenuProbe {
            open: false,
            toggle: Rect::NOTHING,
            item_rect: Rect::NOTHING,
            item_clicked: false,
            first_focused: false,
        };
        let mut output = ctx.run_ui(input(events), |ui| {
            let toggle = ui.put(
                Rect::from_min_size(pos2(600.0, 60.0), vec2(296.0, 40.0)),
                egui::Button::new("Paroles"),
            );
            probe.toggle = toggle.rect;
            let shown = egui::Popup::menu(&toggle)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let first = ui.button("Générer depuis la chanson");
                    probe.item_rect = first.rect;
                    probe.item_clicked = first.clicked();
                    probe.first_focused = first.has_focus();
                    let _ = ui.button("Importer un fichier .srt…");
                });
            probe.open = shown.is_some();
        });
        output.textures_delta.clear();
        probe
    }

    #[test]
    fn popup_menu_opens_from_its_chip_activates_items_and_closes_on_escape() {
        let ctx = egui::Context::default();
        let closed = menu_frame(&ctx, vec![]);
        assert!(!closed.open);
        menu_frame(&ctx, click(closed.toggle.center()));
        let open = menu_frame(&ctx, vec![]);
        assert!(open.open, "click on the chip opens its menu");
        // Keyboard: Tab reaches the first item; Enter activates the focused item.
        let mut focused = false;
        for _ in 0..4 {
            focused |= menu_frame(&ctx, vec![key(Key::Tab)]).first_focused;
            if focused {
                break;
            }
        }
        assert!(focused, "Tab must reach the first menu item");
        let activated = menu_frame(&ctx, vec![key(Key::Enter)]);
        assert!(
            activated.item_clicked,
            "Enter must activate the focused item"
        );
        // Items do not close a CloseOnClickOutside menu: the app calls
        // `ui.close()` after an action.
        assert!(menu_frame(&ctx, vec![]).open);
        menu_frame(&ctx, vec![key(Key::Escape)]);
        assert!(!menu_frame(&ctx, vec![]).open, "Escape must close the menu");
        // Record optional arrow-key navigation alongside required keyboard behavior.
        menu_frame(&ctx, click(closed.toggle.center()));
        assert!(menu_frame(&ctx, vec![]).open, "click reopens the menu");
        let arrow = menu_frame(&ctx, vec![key(Key::ArrowDown)]).first_focused
            || menu_frame(&ctx, vec![]).first_focused;
        eprintln!("ArrowDown focuses the first item: {arrow}");
    }
}

/// A framed select (the design's `select`): 1 px `border-strong` box on
/// `input`, radius 6, 32 high, with a chevron. Wraps an egui `ComboBox`.
pub fn framed_select<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens(ui);
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for (state, stroke) in [
            (&mut widgets.inactive, t.border_strong),
            (&mut widgets.hovered, t.text_2),
            (&mut widgets.active, t.focus),
            (&mut widgets.open, t.focus),
        ] {
            state.weak_bg_fill = t.input;
            state.bg_fill = t.input;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.corner_radius = radius::M.into();
            state.expansion = 0.0;
        }
        ui.spacing_mut().button_padding = Vec2::new(10.0, 7.0);
        ui.spacing_mut().interact_size.y = CONTROL_H;
        add(ui)
    })
    .inner
}
/// The select's chevron, for `ComboBox::icon`.
pub fn select_chevron(ui: &Ui, rect: egui::Rect, _: &egui::style::WidgetVisuals, _open: bool) {
    let side = rect.height().min(16.0);
    crate::app_icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), Vec2::splat(side)),
        crate::app_icons::Icon::ChevronDown,
        tokens(ui).text_2,
    );
}
