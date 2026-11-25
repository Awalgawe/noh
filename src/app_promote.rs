//! « Promote NOH »: the setting, its checkbox on the export lines, and the
//! « made with NOH » mark painted over the preview (`noh::mark` holds the one
//! description exports burn with libass; the preview draws the same numbers).
use super::*;
use egui::{Color32, FontFamily, FontId, Painter, Rect, Vec2};
use noh::mark::{Color, Corner, Layout};

/// The setting's file, beside the language file (`<config>/noh/promote`):
/// `on` or `off`, then the corner (`auto` when absent), then the colour
/// (`#RRGGBB`, white when absent). Off by default.
pub fn path(language: &std::path::Path) -> std::path::PathBuf {
    language.with_file_name("promote")
}

/// The box's state, the corner and the colour, as last saved.
pub fn read(path: Option<&std::path::Path>) -> (bool, Corner, Color) {
    let text = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let mut words = text.split_whitespace();
    let on = words.next() == Some("on");
    let corner = words
        .next()
        .and_then(|w| serde_json::from_value(serde_json::Value::String(w.into())).ok())
        .unwrap_or_default();
    let color = words.next().and_then(Color::parse).unwrap_or_default();
    (on, corner, color)
}

fn save(path: &std::path::Path, on: bool, corner: Corner, color: Color) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let corner = serde_json::to_value(corner)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "auto".into());
    std::fs::write(
        path,
        format!("{} {corner} {}", if on { "on" } else { "off" }, color.hex()),
    )
}

impl NohApp {
    /// Tick or untick the box: remembered, and the exports already made, made
    /// without (or with) the mark, are no longer current. Previews are not
    /// touched: the mark is drawn over them live.
    pub(super) fn set_promote(&mut self, on: bool) {
        if self.promote == on {
            return;
        }
        self.promote = on;
        self.outdate_marked_exports();
        self.save_promote();
    }

    /// Choose the mark's corner: remembered. With the box ticked, exports
    /// made in another corner are no longer current (`auto` and the corner it
    /// stands for are the same on each picture); unticked, nothing changes.
    pub(super) fn set_promote_corner(&mut self, corner: Corner) {
        if self.promote_corner == corner {
            return;
        }
        let before = self.promote_corner;
        self.promote_corner = corner;
        let moved = [(1920, 1080), (1080, 1920)]
            .iter()
            .any(|&(w, h)| before.resolve(w, h) != corner.resolve(w, h));
        if self.promote && moved {
            self.outdate_marked_exports();
        }
        self.save_promote();
    }

    /// Choose the mark's colour (its opacity and stroke stay). With the box
    /// ticked, exports made in another colour are no longer current;
    /// unticked, nothing changes. Remembered once `settled` (a drag in the
    /// picker saves at its end, not on each frame).
    pub(super) fn set_promote_color(&mut self, color: Color, settled: bool) {
        if self.promote_color != color {
            self.promote_color = color;
            if self.promote {
                self.outdate_marked_exports();
            }
        }
        if settled {
            self.save_promote();
        }
    }

    fn outdate_marked_exports(&mut self) {
        self.outdate_exports();
        for a in [
            &mut self.shorts.export_artifact,
            &mut self.captions.export_artifact,
        ]
        .into_iter()
        .flatten()
        {
            a.revision = a.revision.wrapping_sub(1);
        }
    }

    fn save_promote(&mut self) {
        let (on, corner, color) = (self.promote, self.promote_corner, self.promote_color);
        let saved = self
            .locale
            .path
            .as_deref()
            .ok_or_else(|| std::io::Error::other("No user configuration directory"))
            .and_then(|language| save(&path(language), on, corner, color));
        if let Err(error) = saved {
            self.notice = Some(Message::new(
                "error.promote_preferences",
                &[error.to_string()],
            ));
        }
    }

    /// The mark over the preview's picture `picture`, for a `canvas`-sized
    /// export with these safe insets. Nothing without the setting.
    pub(super) fn paint_promote(
        &self,
        painter: &Painter,
        picture: Rect,
        canvas: (u32, u32),
        safe: Option<[u32; 4]>,
    ) {
        if self.promote {
            paint(
                painter,
                picture,
                canvas,
                safe,
                self.promote_corner,
                self.promote_color,
            );
        }
    }
}

/// Paint the mark of a `canvas`-sized picture into `picture`: the same
/// geometry the export's libass script draws, scaled to the monitor. The
/// frame's edges land on physical pixels and its stroke is never thinner than
/// one; the letters are the logo font's, at the wordmark's pens.
pub fn paint(
    painter: &Painter,
    picture: Rect,
    canvas: (u32, u32),
    safe: Option<[u32; 4]>,
    corner: Corner,
    color: Color,
) {
    let (cw, ch) = canvas;
    if cw == 0 || ch == 0 || picture.width() <= 0.0 {
        return;
    }
    let m = Layout::new(cw, ch, safe, corner);
    let k = picture.width() / cw as f32;
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let at = |x: f32, y: f32| picture.min + Vec2::new(x, y) * k;
    let alpha = |o: f32| (o * 255.0).round() as u8;
    // The frame: four bands, each at least one physical pixel.
    let stroke = (m.stroke as f32 * k).max(1.0 / ppp);
    let (a, b) = (at(m.x0 as f32, m.y0 as f32), at(m.x1 as f32, m.y1 as f32));
    let (x0, y0, x1, y1) = (snap(a.x), snap(a.y), snap(b.x), snap(b.y));
    let tint = |o: f32| Color32::from_rgba_unmultiplied(color.0, color.1, color.2, alpha(o));
    let ring = tint(noh::mark::STROKE);
    for band in [
        Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y0 + stroke)),
        Rect::from_min_max(egui::pos2(x0, y1 - stroke), egui::pos2(x1, y1)),
        Rect::from_min_max(
            egui::pos2(x0, y0 + stroke),
            egui::pos2(x0 + stroke, y1 - stroke),
        ),
        Rect::from_min_max(
            egui::pos2(x1 - stroke, y0 + stroke),
            egui::pos2(x1, y1 - stroke),
        ),
    ] {
        painter.rect_filled(band, 0.0, ring);
    }
    // The letters, in the mark's colour at their opacity, on their baselines.
    let white = tint(noh::mark::LETTERS);
    let logo = |size: f32| FontId::new(size, FontFamily::Name("logo".into()));
    let place = |text: &str, font: FontId, pen: egui::Pos2| {
        let galley = painter.layout_no_wrap(text.into(), font, white);
        let origin = galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|g| row.pos + g.pos.to_vec2()));
        if let Some(origin) = origin {
            painter.galley(pen - origin.to_vec2(), galley, white);
        }
    };
    let noh = logo(1000.0 * m.unit() * k);
    for (letter, x) in m.letters() {
        place(letter, noh.clone(), at(x, m.baseline));
    }
    // MADE WITH: each letter at its pen, the font's advance plus the tracking.
    let size = 1000.0 * m.made_scale * k;
    let made = logo(size);
    let (mut pen, tracking) = m.made_pen();
    for c in noh::mark::MADE_TEXT.chars() {
        place(&c.to_string(), made.clone(), at(pen, m.made_baseline));
        let advance = painter
            .layout_no_wrap(c.to_string(), made.clone(), white)
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|g| g.advance_width))
            .unwrap_or(0.0);
        pen += advance / k + tracking;
    }
}

/// The corner glyph: an 18 × 12 frame with a 6 × 4 block in `corner`, or an
/// « A » for automatic, centred in `rect`.
fn paint_corner_glyph(painter: &Painter, rect: Rect, corner: Corner, color: Color32) {
    let frame = Rect::from_center_size(rect.center(), Vec2::new(18.0, 12.0));
    painter.rect_stroke(
        frame.shrink(0.5),
        1.5,
        egui::Stroke::new(1.25, color),
        egui::StrokeKind::Middle,
    );
    let block = |x: f32, y: f32| Rect::from_min_size(egui::pos2(x, y), Vec2::new(6.0, 4.0));
    let (l, t) = (frame.left() + 2.0, frame.top() + 2.0);
    let (r, b) = (frame.right() - 8.0, frame.bottom() - 6.0);
    let filled = match corner {
        Corner::TopLeft => Some(block(l, t)),
        Corner::TopRight => Some(block(r, t)),
        Corner::BottomLeft => Some(block(l, b)),
        Corner::BottomRight => Some(block(r, b)),
        Corner::Auto => None,
    };
    match filled {
        Some(block) => {
            painter.rect_filled(block, 0.5, color);
        }
        None => {
            painter.text(
                frame.center(),
                egui::Align2::CENTER_CENTER,
                "A",
                FontId::proportional(9.0),
                color,
            );
        }
    }
}

/// The corners in the menu's order, with their label keys.
const CORNERS: [(Corner, &str); 4] = [
    (Corner::TopLeft, "promote.corner_top_left"),
    (Corner::TopRight, "promote.corner_top_right"),
    (Corner::BottomLeft, "promote.corner_bottom_left"),
    (Corner::BottomRight, "promote.corner_bottom_right"),
];

/// One row of the corner menu: the glyph, the title (and a sub-line), a tick
/// on the current one.
fn corner_row(
    ui: &mut egui::Ui,
    corner: Corner,
    title: &str,
    sub: Option<&str>,
    current: bool,
) -> egui::Response {
    let t = app_style::tokens(ui);
    let width = ui.available_width().max(200.0);
    let wrap = width - 40.0 - 28.0;
    let title_galley = ui.painter().layout(
        title.into(),
        FontId::proportional(app_style::text::BODY),
        t.text,
        wrap,
    );
    let sub_galley = sub.map(|sub| {
        ui.painter().layout(
            sub.into(),
            FontId::proportional(app_style::text::SMALL),
            t.text_2,
            wrap,
        )
    });
    let height =
        8.0 + title_galley.size().y + sub_galley.as_ref().map_or(0.0, |g| 2.0 + g.size().y) + 8.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, height.max(32.0)), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, current, title)
    });
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, app_style::radius::M, t.surface_2);
    }
    let painter = ui.painter();
    paint_corner_glyph(
        painter,
        Rect::from_center_size(
            egui::pos2(
                rect.left() + 20.0,
                rect.top() + 8.0 + title_galley.size().y / 2.0,
            ),
            Vec2::new(18.0, 12.0),
        ),
        corner,
        t.text_2,
    );
    let title_h = title_galley.size().y;
    painter.galley(
        egui::pos2(rect.left() + 40.0, rect.top() + 8.0),
        title_galley,
        t.text,
    );
    if let Some(sub) = sub_galley {
        painter.galley(
            egui::pos2(rect.left() + 40.0, rect.top() + 10.0 + title_h),
            sub,
            t.text_2,
        );
    }
    if current {
        let c = egui::pos2(rect.right() - 16.0, rect.top() + 8.0 + title_h / 2.0);
        painter.add(egui::Shape::line(
            vec![
                c + Vec2::new(-5.0, 0.0),
                c + Vec2::new(-1.5, 3.5),
                c + Vec2::new(5.0, -3.5),
            ],
            egui::Stroke::new(1.75, t.accent_text),
        ));
    }
    app_style::focus(ui, &response);
    response
}

/// The corner menu's popup id (one export line shows at a time), and its
/// colour component's id: stable, so a capture can open them.
pub(super) const MENU: &str = "promote-corner-menu";
pub(super) const PICKER: &str = "promote-custom-colour";

/// The mark's swatches in the menu: white and black at one click, then four
/// colours that sit well on video.
const PALETTE: [(Color, &str); 6] = [
    (Color::WHITE, "promote.color_white"),
    (Color::BLACK, "promote.color_black"),
    (Color(0x5F, 0xCA, 0xBE), "promote.color_teal"),
    (Color(0xEA, 0xD9, 0xB8), "promote.color_sand"),
    (Color(0xE9, 0xC4, 0x6A), "promote.color_gold"),
    (Color(0xF0, 0xAC, 0x8B), "promote.color_coral"),
];

fn c32(c: Color) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// What the corner menu changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Corner(Corner),
    /// The colour, and whether it is settled (`app_color::Outcome::settled`).
    Color(Color, bool),
}

/// The corner button that follows a ticked « Promote NOH » box: the corner's
/// glyph and a chevron; it opens the menu: the corners, then the colour (the
/// app's colour component). What was changed, if anything, and a hand-made
/// colour for the recent ones.
pub fn corner_button(
    ui: &mut egui::Ui,
    corner: Corner,
    color: Color,
    recent: &[Color32],
    enabled: bool,
    l: noh::i18n::Language,
) -> (Option<Pick>, Option<app_color::Remember>) {
    let t = app_style::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(34.0, 16.0), egui::Sense::click());
    let response = if enabled {
        response
    } else {
        response.on_disabled_hover_text("")
    };
    let name = l.text("promote.corner");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    let ink = if enabled { t.text_2 } else { t.text_3 };
    if enabled && (response.hovered() || response.has_focus()) {
        ui.painter()
            .rect_filled(rect.expand(2.0), app_style::radius::S, t.surface_2);
    }
    paint_corner_glyph(
        ui.painter(),
        Rect::from_min_size(rect.min, Vec2::new(22.0, 16.0)),
        corner,
        ink,
    );
    let c = egui::pos2(rect.right() - 6.0, rect.center().y);
    ui.painter().add(egui::Shape::line(
        vec![
            c + Vec2::new(-3.0, -1.5),
            c + Vec2::new(0.0, 1.5),
            c + Vec2::new(3.0, -1.5),
        ],
        egui::Stroke::new(1.25, ink),
    ));
    app_style::focus(ui, &response);
    let response = response.on_hover_text(name);
    if !enabled {
        return (None, None);
    }
    let mut picked = None;
    let mut remember = None;
    egui::Popup::menu(&response)
        .id(egui::Id::new(MENU))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_END)
        .frame(app_style::popup_frame(ui))
        .width(260.0)
        .show(|ui| {
            // Capped to the room above the button: a short window scrolls the
            // menu instead of cutting its top off.
            let room = response.rect.top() - ui.ctx().content_rect().top() - 24.0;
            egui::ScrollArea::vertical()
                .max_height(room.max(120.0))
                .min_scrolled_height(room.max(120.0))
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    ui.set_width(260.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(l.text("promote.corner_title"))
                                .size(app_style::text::SMALL)
                                .color(app_style::tokens(ui).text_2),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    if corner_row(
                        ui,
                        Corner::Auto,
                        l.text("promote.corner_auto"),
                        Some(l.text("promote.corner_auto_hint")),
                        corner == Corner::Auto,
                    )
                    .clicked()
                    {
                        picked = Some(Pick::Corner(Corner::Auto));
                    }
                    ui.separator();
                    for (c, key) in CORNERS {
                        if corner_row(ui, c, l.text(key), None, corner == c).clicked() {
                            picked = Some(Pick::Corner(c));
                        }
                    }
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(l.text("promote.color"))
                                .size(app_style::text::SMALL)
                                .color(app_style::tokens(ui).text_2),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    let swatches = PALETTE.map(|(c, key)| (c32(c), Some(l.text(key))));
                    let out = app_color::colour_row(
                        ui,
                        egui::Id::new(PICKER),
                        &swatches,
                        c32(color),
                        recent,
                        l,
                    );
                    if let Some(c) = out.color {
                        picked = Some(Pick::Color(Color(c.r(), c.g(), c.b()), out.settled));
                    }
                    remember = out.remember;
                    // A corner, white or black closes the menu; other colours keep it
                    // open, so the mark can be watched while choosing.
                    let one_click = match picked {
                        Some(Pick::Corner(_)) => true,
                        Some(Pick::Color(c, _)) => {
                            out.from_row && (c == Color::WHITE || c == Color::BLACK)
                        }
                        None => false,
                    };
                    if one_click {
                        ui.close();
                    }
                });
        });
    (picked, remember)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(revision: u64) -> Option<app_state::Artifact> {
        Some(app_state::Artifact {
            path: "made.mp4".into(),
            revision,
            bytes: None,
        })
    }

    #[test]
    fn ticking_the_box_outdates_the_exports_and_never_the_previews() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = NohApp::default();
        app.locale.path = Some(folder.path().join("noh").join("language"));
        let revision = app.revision;
        app.export_artifact = artifact(revision);
        app.preview_artifact = artifact(revision);
        app.captions.export_artifact = artifact(app.captions.revision);
        app.shorts.export_artifact = artifact(app.shorts.revision);
        assert!(!app.promote);
        app.set_promote(true);
        assert!(app.promote && app.notice.is_none());
        // The exports were made without the mark: no longer current.
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        assert_ne!(
            app.captions.export_artifact.as_ref().unwrap().revision,
            app.captions.revision
        );
        assert_ne!(
            app.shorts.export_artifact.as_ref().unwrap().revision,
            app.shorts.revision
        );
        // The preview render carries no mark (drawn live): still current, and
        // the project's revision did not move, so nothing renders again.
        assert_eq!(app.revision, revision);
        assert_eq!(
            app.preview_artifact.as_ref().unwrap().revision,
            app.revision
        );
        // Remembered.
        let file = path(app.locale.path.as_deref().unwrap());
        assert!(read(Some(&file)).0);
        app.set_promote(false);
        assert!(!read(Some(&file)).0);
    }

    #[test]
    fn the_setting_is_off_unless_the_file_says_on() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("promote");
        assert_eq!(read(None), (false, Corner::Auto, Color::WHITE));
        assert_eq!(read(Some(&file)), (false, Corner::Auto, Color::WHITE));
        std::fs::write(&file, "maybe").unwrap();
        assert!(!read(Some(&file)).0);
        std::fs::write(
            &file, "on
",
        )
        .unwrap();
        assert_eq!(read(Some(&file)), (true, Corner::Auto, Color::WHITE));
        save(&file, true, Corner::TopRight, Color(0x12, 0xAB, 0xEF)).unwrap();
        assert_eq!(
            read(Some(&file)),
            (true, Corner::TopRight, Color(0x12, 0xAB, 0xEF))
        );
    }

    #[test]
    fn moving_the_mark_outdates_exports_only_while_it_is_on() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = NohApp::default();
        app.locale.path = Some(folder.path().join("noh").join("language"));
        let revision = app.revision;
        app.export_artifact = artifact(revision);
        app.preview_artifact = artifact(revision);
        // Off: the corner is remembered, nothing is outdated.
        app.set_promote_corner(Corner::TopRight);
        assert_eq!(app.export_artifact.as_ref().unwrap().revision, revision);
        // On, and the box ticked made the old export stale; a fresh one now.
        app.set_promote(true);
        app.export_artifact = artifact(app.revision);
        // Auto and the corner it stands for on a landscape picture... differ
        // on a vertical one, so the move counts.
        app.set_promote_corner(Corner::BottomRight);
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        // The preview render never carries the mark: still current.
        assert_eq!(
            app.preview_artifact.as_ref().unwrap().revision,
            app.revision
        );
        assert_eq!(app.revision, revision);
        let file = path(app.locale.path.as_deref().unwrap());
        assert_eq!(read(Some(&file)).1, Corner::BottomRight);
    }

    #[test]
    fn the_menu_colours_are_named_in_every_language() {
        for language in noh::i18n::Language::ALL {
            for (_, key) in PALETTE {
                assert!(
                    !language.text(key).contains("missing"),
                    "{} {key}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn a_new_colour_outdates_exports_only_while_the_mark_is_on() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = NohApp::default();
        app.locale.path = Some(folder.path().join("noh").join("language"));
        let revision = app.revision;
        app.export_artifact = artifact(revision);
        app.preview_artifact = artifact(revision);
        app.set_promote_color(Color::BLACK, true);
        assert_eq!(app.export_artifact.as_ref().unwrap().revision, revision);
        app.set_promote(true);
        app.export_artifact = artifact(app.revision);
        app.set_promote_color(Color(0xE0, 0x6C, 0x3C), false);
        // A step of a drag is not saved yet; its end is.
        assert_eq!(
            read(Some(&path(app.locale.path.as_deref().unwrap()))).2,
            Color::BLACK
        );
        app.set_promote_color(Color(0xE0, 0x6C, 0x3C), true);
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        assert_eq!(
            app.preview_artifact.as_ref().unwrap().revision,
            app.revision
        );
        assert_eq!(app.revision, revision);
        let file = path(app.locale.path.as_deref().unwrap());
        assert_eq!(read(Some(&file)).2, Color(0xE0, 0x6C, 0x3C));
    }
}
