//! Lyrics chip and menu: generation, import, review, "Show on the
//! video" and removal, all reached from the chip under the song.
use super::*;
use app_icons::Icon;
use app_style::{self as style, space};
use egui::{FontId, Rect, Sense, Stroke, pos2, vec2};
use noh::captions::{CaptionPlacement, CaptionSize};

pub(super) const MENU: &str = "lyrics-menu";
const CHIP_H: f32 = 40.0;
/// Menu width on the wide layout; compact menus take the chip's width.
pub(super) const MENU_W: f32 = 340.0;

/// What the chip shows, from the attachment and generation state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Chip {
    None,
    Reading,
    Generating,
    Ready { lines: usize },
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Generate,
    Import,
    Review,
    Remove,
    Settings,
}

impl NohApp {
    /// Shown with a song, or while a track is attached.
    pub(super) fn lyrics_visible(&self) -> bool {
        self.wav.is_some() || self.project.track.path.is_some()
    }

    pub(super) fn lyrics_state(&self) -> Chip {
        let track = &self.project.track;
        if self.project.generating {
            Chip::Generating
        } else if let Some(loaded) = &track.loaded {
            Chip::Ready {
                lines: loaded.track.cues.len(),
            }
        } else if track.path.is_none() {
            Chip::None
        } else if track.error.is_some() {
            Chip::Unreadable
        } else {
            Chip::Reading
        }
    }

    /// The attached lyrics were generated from another song than the current
    /// one: their timing needs checking (`lyrics.recheck`).
    pub(super) fn lyrics_recheck(&self) -> bool {
        let Some(wav) = &self.wav else {
            return false;
        };
        self.subtitles.artifact.as_ref().is_some_and(|artifact| {
            self.project.track.path.as_ref() == Some(&artifact.path)
                && !self.subtitles.source.is_empty()
                && Path::new(&self.subtitles.source) != wav
        })
    }

    pub(super) fn lyrics_chip(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        width: f32,
        compact: bool,
        locked: bool,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let state = self.lyrics_state();
        let (rect, _) = ui.allocate_exact_size(vec2(width, CHIP_H), Sense::hover());
        let menu_id = egui::Id::new(MENU);
        let interactive = !locked && !matches!(state, Chip::Generating | Chip::Reading);
        let chip_id = ui.id().with("lyrics-chip");
        let chip = ui.interact(
            rect,
            chip_id,
            if interactive {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let (status, status_color) = match state {
            Chip::None => (l.text("lyrics.add").to_owned(), t.text_2),
            Chip::Reading => (l.text("strip.reading").to_owned(), t.text_2),
            Chip::Generating => (
                if self.fraction > 0.0 {
                    format!(
                        "{} {} %",
                        l.text("lyrics.generating"),
                        (self.fraction * 100.0).round() as u32
                    )
                } else {
                    l.text("lyrics.generating").to_owned()
                },
                t.text_2,
            ),
            Chip::Ready { lines } => (l.plural("lyrics.lines", lines), t.text_2),
            Chip::Unreadable => (l.text("lyrics.unreadable").to_owned(), t.error),
        };
        let name = format!("{} ({status})", l.text("lyrics.menu"));
        chip.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, interactive, &name)
        });
        let open = interactive && egui::Popup::is_id_open(ctx, menu_id);
        // Frame: dashed while empty, error outline when unreadable.
        match state {
            Chip::None => {
                if chip.hovered() && interactive {
                    ui.painter()
                        .rect_filled(rect, style::radius::L, t.surface_2);
                }
                super::app_strip::dashed_rect(
                    ui.painter(),
                    rect.shrink(0.75),
                    f32::from(style::radius::L),
                    Stroke::new(1.5, t.border_strong),
                );
            }
            _ => {
                ui.painter().rect_filled(
                    rect,
                    style::radius::L,
                    if chip.hovered() && interactive {
                        t.surface_2
                    } else {
                        t.surface
                    },
                );
                if state == Chip::Unreadable {
                    ui.painter().rect_stroke(
                        rect,
                        style::radius::L,
                        Stroke::new(2.0, t.error),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        if open {
            // The focus-coloured ring while the menu is open (design system).
            ui.painter().rect_stroke(
                rect.expand(2.0),
                style::radius::M,
                Stroke::new(2.0, t.focus),
                egui::StrokeKind::Outside,
            );
        } else {
            style::focus(ui, &chip);
        }
        app_icons::paint(
            ui.painter(),
            Rect::from_min_size(
                pos2(rect.left() + 12.0, rect.center().y - 9.0),
                vec2(18.0, 18.0),
            ),
            Icon::Lyrics,
            if state == Chip::Unreadable {
                t.error
            } else {
                t.text_2
            },
        );
        // Trailing: chevron (empty), ⋯ (a track), or a spinner.
        let trailing = Rect::from_min_size(
            pos2(rect.right() - 32.0, rect.center().y - 14.0),
            vec2(28.0, 28.0),
        );
        match state {
            Chip::None => app_icons::paint(
                ui.painter(),
                trailing.shrink(6.0),
                Icon::ChevronDown,
                t.text_2,
            ),
            Chip::Ready { .. } | Chip::Unreadable => {
                app_icons::paint(ui.painter(), trailing.shrink(6.0), Icon::More, t.text_2)
            }
            Chip::Generating | Chip::Reading => {
                ui.put(trailing.shrink(6.0), egui::Spinner::new().size(16.0));
            }
        }
        // Title, status and the check-again tag, truncated before the trailing icon.
        let title = ui.painter().layout_no_wrap(
            l.text("lyrics.title").into(),
            style::semibold(style::text::BODY),
            t.text,
        );
        let text_left = rect.left() + 40.0;
        let text_right = trailing.left() - space::S;
        let title_w = title.size().x;
        ui.painter().galley(
            pos2(text_left, rect.center().y - title.size().y / 2.0),
            title,
            t.text,
        );
        let mut x = text_left + title_w + space::S;
        let status_galley = ui.painter().layout(
            status,
            FontId::proportional(style::text::SMALL),
            status_color,
            f32::INFINITY,
        );
        let recheck = matches!(state, Chip::Ready { .. }) && self.lyrics_recheck();
        let tag = recheck.then(|| {
            ui.painter().layout_no_wrap(
                l.text("lyrics.recheck").into(),
                FontId::proportional(style::text::SMALL),
                t.warn,
            )
        });
        if x + status_galley.size().x <= text_right {
            let status_w = status_galley.size().x;
            ui.painter().galley(
                pos2(x, rect.center().y - status_galley.size().y / 2.0),
                status_galley,
                status_color,
            );
            x += status_w + space::S;
        }
        if let Some(tag) = tag {
            let pill = Rect::from_min_size(
                pos2(x, rect.center().y - 9.0),
                vec2(tag.size().x + 12.0, 18.0),
            );
            if pill.right() <= text_right {
                ui.painter()
                    .rect_filled(pill, style::radius::S, t.warn_soft);
                ui.painter().galley(
                    pos2(pill.left() + 6.0, pill.center().y - tag.size().y / 2.0),
                    tag,
                    t.warn,
                );
            }
        }
        let chip = if let Some(error) = &self.project.track.error {
            if error.code == "lyrics.after_end" {
                let reason = l.text("lyrics.after_end");
                chip.on_hover_text(if recheck {
                    format!("{reason}\n{}", l.text("lyrics.recheck_help"))
                } else {
                    reason.to_owned()
                })
            } else if state == Chip::Unreadable {
                chip.on_hover_text(format!(
                    "{}\n{}",
                    Message::from(error.message_code()).render(l),
                    error.detail
                ))
            } else {
                chip
            }
        } else if recheck {
            chip.on_hover_text(l.text("lyrics.recheck_help"))
        } else {
            chip
        };
        if !interactive {
            if egui::Popup::is_id_open(ctx, menu_id) {
                egui::Popup::close_id(ctx, menu_id);
            }
            self.strip.lyrics_menu_open = false;
            return;
        }
        // Escape closes the menu and gives the focus back to the chip.
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if self.strip.lyrics_menu_open && escape {
            egui::Popup::close_id(ctx, menu_id);
            chip.request_focus();
        }
        // ↑/↓ move between the items, like Shift+Tab/Tab.
        if self.strip.lyrics_menu_open && !escape {
            let (up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                )
            });
            if up || down {
                ctx.memory_mut(|m| {
                    m.move_focus(if up {
                        egui::FocusDirection::Previous
                    } else {
                        egui::FocusDirection::Next
                    })
                });
            }
        }
        if !self.strip.lyrics_menu_open {
            self.strip.lyrics_focus_first = true;
        }
        let mut action = None;
        let before_burn = self.project.apply_subtitles;
        let before_style = self.project.caption_style;
        // The menu stays under the chip and scrolls rather than covering it.
        let below = (ctx.content_rect().bottom() - rect.bottom() - 24.0).max(120.0);
        let shown = egui::Popup::menu(&chip)
            .id(menu_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(if compact {
                egui::RectAlign::BOTTOM_START
            } else {
                egui::RectAlign::BOTTOM_END
            })
            .frame(style::popup_frame(ui))
            .width(if compact { width - 14.0 } else { MENU_W - 14.0 })
            .show(|ui| {
                ui.set_width(if compact { width - 14.0 } else { MENU_W - 14.0 });
                // Scroll only when the content is taller than the room below:
                // a ScrollArea keeps its previous height when rows appear.
                let tall = self.strip.lyrics_menu_h > below;
                let top = ui.cursor().top();
                if tall {
                    egui::ScrollArea::vertical()
                        .id_salt("lyrics-menu-scroll")
                        .max_height(below)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            action = self.lyrics_menu(ui, state);
                            self.strip.lyrics_menu_h = ui.min_rect().height();
                        });
                } else {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    action = self.lyrics_menu(ui, state);
                    self.strip.lyrics_menu_h = ui.min_rect().bottom() - top;
                }
                if action.is_some() {
                    ui.close();
                }
            });
        self.strip.lyrics_menu_open = shown.is_some();
        if before_burn != self.project.apply_subtitles
            || (self.project.apply_subtitles && before_style != self.project.caption_style)
        {
            self.invalidated();
        }
        if let Some(action) = action {
            // Any lyrics action answers the "Lyrics ready" row.
            self.lyrics_ready = None;
            match action {
                Action::Generate => self.project_generate(ctx),
                Action::Import => self.pick_lyrics(ctx),
                Action::Review => {
                    if let Some(path) = self.project.track.path.clone() {
                        self.open(&path);
                    }
                }
                Action::Remove => {
                    self.project.track.clear();
                    if self.project.apply_subtitles {
                        self.project.apply_subtitles = false;
                        self.invalidated();
                    }
                }
                Action::Settings => self.settings_open = true,
            }
            chip.request_focus();
        }
    }

    /// Menu content: the empty menu, the track menu, or Replace/Remove for an
    /// unreadable file. The first enabled item takes the focus when it opens.
    fn lyrics_menu(&mut self, ui: &mut egui::Ui, state: Chip) -> Option<Action> {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let configured = self.subtitles.configured() && self.resources.speech_issue().is_none();
        let can_generate = configured && self.wav.is_some();
        let mut action = None;
        // Requested until it holds: the popup's first frame is an invisible
        // sizing pass, where a focus request does not stick.
        let pending = std::cell::Cell::new(self.strip.lyrics_focus_first);
        let focus_first = |response: &egui::Response| {
            if pending.get() && response.enabled() {
                if response.has_focus() {
                    pending.set(false);
                } else {
                    response.request_focus();
                }
            }
        };
        let generate_sub = if configured {
            l.text("lyrics.generate_help")
        } else {
            l.text("lyrics.not_set_up")
        };
        match state {
            Chip::None => {
                let generate = style::menu_item_primary(
                    ui,
                    Icon::Generate,
                    l.text("lyrics.generate"),
                    generate_sub,
                    can_generate,
                );
                focus_first(&generate);
                if generate.clicked() {
                    action = Some(Action::Generate);
                }
                if !configured && ui.link(l.text("settings")).clicked() {
                    action = Some(Action::Settings);
                }
                self.lyrics_language(ui);
                separator(ui, t.border);
                let import = style::menu_item(ui, Icon::File, l.text("lyrics.import"), None, true);
                focus_first(&import);
                if import.clicked() {
                    action = Some(Action::Import);
                }
            }
            Chip::Ready { .. } => {
                let review = style::menu_item(
                    ui,
                    Icon::Eye,
                    l.text("lyrics.review"),
                    Some(l.text("lyrics.review_help")),
                    true,
                );
                focus_first(&review);
                if review.clicked() {
                    action = Some(Action::Review);
                }
                style::menu_switch(
                    ui,
                    Icon::Captions,
                    l.text("lyrics.show"),
                    &mut self.project.apply_subtitles,
                    true,
                );
                if self.project.apply_subtitles {
                    self.caption_rows(ui);
                }
                separator(ui, t.border);
                let regenerate = style::menu_item(
                    ui,
                    Icon::Regenerate,
                    l.text("lyrics.regenerate"),
                    (!configured).then(|| l.text("lyrics.not_set_up")),
                    can_generate,
                );
                if regenerate.clicked() {
                    action = Some(Action::Generate);
                }
                if style::menu_item(ui, Icon::Replace, l.text("lyrics.replace"), None, true)
                    .clicked()
                {
                    action = Some(Action::Import);
                }
                if style::menu_item(ui, Icon::Remove, l.text("lyrics.remove"), None, true).clicked()
                {
                    action = Some(Action::Remove);
                }
                self.lyrics_language(ui);
            }
            Chip::Unreadable => {
                let replace =
                    style::menu_item(ui, Icon::Replace, l.text("lyrics.replace"), None, true);
                focus_first(&replace);
                if replace.clicked() {
                    action = Some(Action::Import);
                }
                if style::menu_item(ui, Icon::Remove, l.text("lyrics.remove"), None, true).clicked()
                {
                    action = Some(Action::Remove);
                }
            }
            Chip::Generating | Chip::Reading => {}
        }
        self.strip.lyrics_focus_first = pending.get();
        action
    }

    /// Sung language, for the next generation.
    fn lyrics_language(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            ui.label(
                egui::RichText::new(l.text("lyrics.language"))
                    .size(style::text::SMALL)
                    .color(t.text_2),
            );
            let selected = if self.subtitles.language == "auto" {
                l.text("lyrics.auto").to_owned()
            } else {
                Language::ALL
                    .into_iter()
                    .find(|lang| lang.code() == self.subtitles.language)
                    .map(|lang| lang.name().to_owned())
                    .unwrap_or_else(|| self.subtitles.language.clone())
            };
            // Bounded by the menu: long names truncate instead of widening it.
            let room = ui.available_width() - ui.spacing().item_spacing.x - 8.0;
            style::framed_select(ui, |ui| {
                egui::ComboBox::from_id_salt("lyrics-language")
                    .icon(style::select_chevron)
                    .width(room.clamp(80.0, 200.0))
                    .truncate()
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.subtitles.language,
                            "auto".into(),
                            l.text("lyrics.auto"),
                        );
                        for lang in Language::ALL {
                            ui.selectable_value(
                                &mut self.subtitles.language,
                                lang.code().into(),
                                lang.name(),
                            );
                        }
                    });
            });
        });
    }

    /// Caption size and position under "Show on the video". Each
    /// label sits beside its control, or above it when the row would not fit.
    fn caption_rows(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let sizes = [CaptionSize::Small, CaptionSize::Medium, CaptionSize::Large];
        let places = [CaptionPlacement::Bottom, CaptionPlacement::Top];
        let size_options = [
            l.text("ui.caption_small"),
            l.text("ui.caption_medium"),
            l.text("ui.caption_large"),
        ];
        let place_options = [l.text("ui.caption_bottom"), l.text("ui.caption_top")];
        let width = |ui: &egui::Ui, text: &str, font: FontId| {
            ui.painter()
                .layout_no_wrap(text.into(), font, t.text)
                .size()
                .x
        };
        for (label, id) in [
            ("ui.caption_size", "lyrics-size"),
            ("ui.caption_placement", "lyrics-place"),
        ] {
            let options: &[&str] = if id == "lyrics-size" {
                &size_options
            } else {
                &place_options
            };
            // Segments: text + 12 padding each side + 1 px stroke, 2 apart, 2 inset.
            let control = options
                .iter()
                .map(|o| width(ui, o, style::semibold(style::text::BODY)) + 26.0)
                .sum::<f32>()
                + 2.0 * (options.len() - 1) as f32
                + 4.0;
            let label_w = width(ui, l.text(label), FontId::proportional(style::text::SMALL));
            let inline = 40.0 + label_w + space::S + control <= ui.available_width();
            let row = |ui: &mut egui::Ui, this: &mut Self| {
                let selected = if id == "lyrics-size" {
                    sizes
                        .iter()
                        .position(|s| *s == this.project.caption_style.size)
                        .unwrap_or(1)
                } else {
                    places
                        .iter()
                        .position(|p| *p == this.project.caption_style.placement)
                        .unwrap_or(0)
                };
                if let Some(index) = style::segmented(ui, id, options, selected) {
                    if id == "lyrics-size" {
                        this.project.caption_style.size = sizes[index];
                    } else {
                        this.project.caption_style.placement = places[index];
                    }
                }
            };
            let caption = egui::RichText::new(l.text(label))
                .size(style::text::SMALL)
                .color(t.text_2);
            if inline {
                ui.horizontal(|ui| {
                    ui.add_space(40.0);
                    ui.label(caption);
                    row(ui, self);
                });
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(40.0);
                    ui.add(egui::Label::new(caption).wrap());
                });
                ui.horizontal(|ui| {
                    ui.add_space(40.0);
                    row(ui, self);
                });
            }
        }
    }

    pub(super) fn pick_lyrics(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        if let Some(path) = rfd::FileDialog::new()
            .set_title(l.text("lyrics.pick_title"))
            .add_filter("SRT", &["srt"])
            .pick_file()
        {
            self.drop_files(vec![path], ctx);
        }
    }
}

fn separator(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(
        rect.x_range().shrink(6.0),
        rect.center().y,
        Stroke::new(1.0, color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2};

    fn input(width: f32, events: Vec<Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 850.0))),
            events,
            ..Default::default()
        }
    }
    /// One frame of the strip inside the body's side margins (24, 16 compact).
    fn run(
        ctx: &egui::Context,
        app: &mut NohApp,
        width: f32,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        let margin = if width < style::COMPACT_BELOW { 16 } else { 24 };
        let mut output = ctx.run_ui(input(width, events), |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(margin, 0))
                .show(ui, |ui| app.strip_ui(ui, &ui.ctx().clone(), false));
        });
        output.textures_delta.clear();
        output
    }
    /// One frame of the strip; returns the painted texts with their rects.
    fn frame(
        ctx: &egui::Context,
        app: &mut NohApp,
        width: f32,
        events: Vec<Event>,
    ) -> Vec<(String, Rect)> {
        run(ctx, app, width, events)
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect()
    }
    /// Compact strip positioned at y=56 in the 420×540 main view, below its
    /// 48 px header and 8 px body inset. This makes the popup's scroll room
    /// match the native capture rather than the strip-only 850 px helper.
    fn compact_frame(
        ctx: &egui::Context,
        app: &mut NohApp,
        events: Vec<Event>,
    ) -> Vec<(String, Rect)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(420.0, 540.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(16, 0))
                    .show(ui, |ui| {
                        ui.add_space(56.0);
                        app.strip_ui(ui, &ui.ctx().clone(), false);
                    });
            },
        );
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    shape
                        .clip_rect
                        .contains(rect.center())
                        .then(|| (text.galley.text().to_owned(), rect))
                }
                _ => None,
            })
            .collect()
    }
    fn compact_click(ctx: &egui::Context, app: &mut NohApp, at: Pos2) {
        for pressed in [true, false] {
            compact_frame(
                ctx,
                app,
                vec![
                    Event::PointerMoved(at),
                    Event::PointerButton {
                        pos: at,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
    }
    fn click(ctx: &egui::Context, app: &mut NohApp, width: f32, at: Pos2) {
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                width,
                vec![
                    Event::PointerMoved(at),
                    Event::PointerButton {
                        pos: at,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
    }
    fn key(key: egui::Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        texts
            .iter()
            .find(|(t, _)| t == text)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("{text:?} not painted: {texts:?}"))
    }
    fn app(language: Language) -> (egui::Context, NohApp) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        // No clock runs here: popups would stay mid fade-in.
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = NohApp::default();
        app.locale.language = language;
        (ctx, app)
    }
    fn open(ctx: &egui::Context) -> bool {
        egui::Popup::is_id_open(ctx, egui::Id::new(MENU))
    }
    /// A loaded 39-line track, attached the way the reader publishes it.
    fn attach_track(app: &mut NohApp, folder: &Path) {
        let srt: String = (0..39)
            .map(|i| {
                format!(
                    "{}\n00:00:{:02},000 --> 00:00:{:02},500\nline {i}\n\n",
                    i + 1,
                    i,
                    i
                )
            })
            .collect();
        let path = folder.join("song.srt");
        std::fs::write(&path, &srt).unwrap();
        let track = noh::subtitle_srt::parse(&srt, 60_000).unwrap();
        app.project.track.path = Some(path.clone());
        app.project.track.loaded = Some(app_track::Loaded {
            track: std::sync::Arc::new(track),
            snapshot: noh::inspection::Snapshot(vec![
                noh::inspection::FileStamp::read(&path).unwrap(),
            ]),
        });
    }

    /// The first-use path: a click on the chip, a click on "Generate from the
    /// song", and the bundled transcriber runs on the current song.
    #[test]
    fn one_click_transcribes_the_current_soundtrack_with_bundled_setup() {
        let folder = tempfile::tempdir().unwrap();
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        for file in [
            &app_subtitles::transcriber_name(),
            "ggml-small.bin",
            "ggml-silero-v6.2.0.bin",
        ] {
            std::fs::write(speech.join(file), []).unwrap();
        }
        std::fs::write(folder.path().join("voice.wav"), []).unwrap();
        let (ctx, mut app) = app(Language::En);
        app.output_folder = folder.path().into();
        app.wav = Some(folder.path().join("voice.wav"));
        assert!(app.subtitles.load_bundled(folder.path()));
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        click(&ctx, &mut app, 980.0, find(&texts, "Lyrics").center());
        // The popup sizes itself invisibly on its first frame.
        frame(&ctx, &mut app, 980.0, vec![]);
        assert!(open(&ctx));
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        click(
            &ctx,
            &mut app,
            980.0,
            find(&texts, "Generate from the song").center(),
        );
        assert!(app.project.generating && app.subtitles.running);
        assert_eq!(Path::new(&app.subtitles.source), app.wav.as_ref().unwrap());
        assert!(!app.settings_open && app.clips.is_empty());
        assert_eq!(app.subtitles.language, "auto");
        // While generating, the chip says so and offers no menu.
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, "Generating…");
        assert!(!open(&ctx));
        assert_eq!(app.lyrics_state(), Chip::Generating);
    }

    /// Without the speech bundle, Generate is disabled and says why.
    #[test]
    fn generate_is_disabled_with_its_reason_when_not_set_up() {
        let (ctx, mut app) = app(Language::En);
        app.wav = Some("song.wav".into());
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        click(&ctx, &mut app, 980.0, find(&texts, "Lyrics").center());
        frame(&ctx, &mut app, 980.0, vec![]);
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, Language::En.text("lyrics.not_set_up"));
        click(
            &ctx,
            &mut app,
            980.0,
            find(&texts, "Generate from the song").center(),
        );
        assert!(!app.project.generating && !app.subtitles.running);
    }

    /// Remove and "Show on the video" keep the track semantics: showing or
    /// restyling shown lyrics invalidates; removing clears the burn setting.
    #[test]
    fn track_menu_toggles_show_restyles_and_removes() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(Language::En);
        app.wav = Some(folder.path().join("song.wav"));
        attach_track(&mut app, folder.path());
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, "39 lines");
        let open_menu = |app: &mut NohApp| {
            let texts = frame(&ctx, app, 980.0, vec![]);
            click(&ctx, app, 980.0, find(&texts, "Lyrics").center());
            frame(&ctx, app, 980.0, vec![]);
            assert!(open(&ctx));
            frame(&ctx, app, 980.0, vec![])
        };
        let texts = open_menu(&mut app);
        assert!(
            !texts.iter().any(|(t, _)| t == "Size"),
            "size rows only when shown"
        );
        let revision = app.revision;
        click(
            &ctx,
            &mut app,
            980.0,
            find(&texts, "Show on the video").center(),
        );
        assert!(app.project.apply_subtitles);
        assert_ne!(app.revision, revision, "showing the lyrics invalidates");
        assert!(open(&ctx), "the toggle keeps the menu open");
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        let revision = app.revision;
        click(&ctx, &mut app, 980.0, find(&texts, "Large").center());
        assert_eq!(app.project.caption_style.size, CaptionSize::Large);
        assert_ne!(app.revision, revision, "restyling shown lyrics invalidates");
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        click(&ctx, &mut app, 980.0, find(&texts, "Remove").center());
        assert!(app.project.track.path.is_none() && !app.project.apply_subtitles);
        frame(&ctx, &mut app, 980.0, vec![]);
        assert!(!open(&ctx), "an action closes the menu");
        assert_eq!(app.lyrics_state(), Chip::None);
    }

    /// In the real compact vertical budget, the track menu scrolls to its
    /// bottom actions; the sung-language row and Remove stay reachable.
    #[test]
    fn compact_track_menu_scroll_reaches_sung_language_and_remove() {
        let folder = tempfile::tempdir().unwrap();
        let speech = folder.path().join("bin/speech");
        std::fs::create_dir_all(&speech).unwrap();
        for file in [
            &app_subtitles::transcriber_name(),
            "ggml-small.bin",
            "ggml-silero-v6.2.0.bin",
        ] {
            std::fs::write(speech.join(file), []).unwrap();
        }
        for language in Language::ALL {
            let (ctx, mut app) = app(language);
            app.wav = Some(folder.path().join("song.wav"));
            assert!(app.subtitles.load_bundled(folder.path()));
            attach_track(&mut app, folder.path());
            app.project.apply_subtitles = true;

            let texts = compact_frame(&ctx, &mut app, vec![]);
            let chip = find(&texts, language.text("lyrics.title"));
            assert!(
                (210.0..=255.0).contains(&chip.center().y),
                "{} chip should match the native compact position: {chip:?}",
                language.code()
            );
            compact_click(&ctx, &mut app, chip.center());
            compact_frame(&ctx, &mut app, vec![]);
            compact_frame(&ctx, &mut app, vec![]);
            assert!(open(&ctx), "{} menu did not open", language.code());

            let initial = compact_frame(&ctx, &mut app, vec![]);
            let remove = language.text("lyrics.remove");
            let language_label = language.text("lyrics.language");
            assert!(
                !(initial.iter().any(|(text, _)| text == remove)
                    && initial.iter().any(|(text, _)| text == language_label)),
                "{} compact menu should require scrolling to its bottom rows",
                language.code()
            );
            let area = ctx
                .memory(|memory| memory.area_rect(egui::Id::new(MENU)))
                .expect("lyrics menu popup area");
            let scroll_point = pos2(area.center().x, area.bottom() - 16.0);
            let mut texts = initial;
            for _ in 0..8 {
                texts = compact_frame(
                    &ctx,
                    &mut app,
                    vec![
                        Event::PointerMoved(scroll_point),
                        Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0.0, -160.0),
                            modifiers: Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                        },
                    ],
                );
                if texts.iter().any(|(text, _)| text == remove)
                    && texts.iter().any(|(text, _)| text == language_label)
                {
                    break;
                }
            }
            let remove_rect = find(&texts, remove);
            find(&texts, language_label);
            assert!(
                remove_rect.center().y < 540.0,
                "{} Remove should be visible within the compact window: {remove_rect:?}",
                language.code()
            );
            compact_click(&ctx, &mut app, remove_rect.center());
            assert!(
                app.project.track.path.is_none() && !app.project.apply_subtitles,
                "{} Remove should respond after scrolling",
                language.code()
            );
        }
    }

    /// Enter opens the menu with the focus on the first item; Tab moves on;
    /// Escape closes it and gives the focus back to the chip.
    #[test]
    fn the_menu_follows_the_keyboard_contract() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(Language::En);
        app.wav = Some(folder.path().join("song.wav"));
        attach_track(&mut app, folder.path());
        frame(&ctx, &mut app, 980.0, vec![]);
        // Frames with these events; the names of the widgets that gained focus.
        let step = |app: &mut NohApp, events: Vec<Event>| -> Vec<String> {
            let mut names = Vec::new();
            for events in [events, vec![], vec![]] {
                for event in &run(&ctx, app, 980.0, events).platform_output.events {
                    if let egui::output::OutputEvent::FocusGained(info) = event {
                        names.extend(info.label.clone());
                    }
                }
            }
            names
        };
        let chip = |names: &[String]| names.iter().any(|n| n.starts_with("Lyrics: actions"));
        // Tab reaches the chip after the song's menu button.
        let mut reached = false;
        for _ in 0..6 {
            if chip(&step(&mut app, vec![key(egui::Key::Tab, Modifiers::NONE)])) {
                reached = true;
                break;
            }
        }
        assert!(reached, "Tab reaches the lyrics chip");
        step(&mut app, vec![key(egui::Key::Enter, Modifiers::NONE)]);
        assert!(open(&ctx));
        // Focus given in code emits no FocusGained event: look for the ring.
        assert!(
            ringed(&ctx, &mut app, "Review the lyrics"),
            "the first item has the focus"
        );
        let names = step(&mut app, vec![key(egui::Key::Tab, Modifiers::NONE)]);
        assert_eq!(
            names.last().map(String::as_str),
            Some("Show on the video"),
            "{names:?}"
        );
        // ↑/↓ move between items too (egui's arrow-key focus navigation).
        let names = step(&mut app, vec![key(egui::Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(
            names.last().map(String::as_str),
            Some("Review the lyrics"),
            "{names:?}"
        );
        let names = step(&mut app, vec![key(egui::Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(
            names.last().map(String::as_str),
            Some("Show on the video"),
            "{names:?}"
        );
        step(&mut app, vec![key(egui::Key::Escape, Modifiers::NONE)]);
        assert!(!open(&ctx));
        assert!(
            ringed(&ctx, &mut app, "Lyrics"),
            "Escape gives the focus back to the chip"
        );
        // Space on the focused chip opens the menu again.
        step(&mut app, vec![key(egui::Key::Space, Modifiers::NONE)]);
        assert!(open(&ctx));
    }

    /// The painted focus ring (design token `focus`) surrounds this text.
    fn ringed(ctx: &egui::Context, app: &mut NohApp, text: &str) -> bool {
        let focus = style::tokens_for(ctx.global_style().visuals.dark_mode).focus;
        let output = run(ctx, app, 980.0, vec![]);
        let mut label = None;
        let mut rings = Vec::new();
        for shape in &output.shapes {
            match &shape.shape {
                egui::Shape::Text(t) if t.galley.text() == text => {
                    label = Some(t.galley.rect.translate(t.pos.to_vec2()));
                }
                egui::Shape::Rect(r) if r.stroke.color == focus && r.stroke.width > 0.0 => {
                    rings.push(r.rect);
                }
                _ => {}
            }
        }
        label.is_some_and(|label| rings.iter().any(|ring| ring.contains(label.center())))
    }

    /// Both menus fit their width (340 wide, the chip's 388 in compact) in
    /// every language, with every text inside the popup.
    #[test]
    fn menus_fit_in_every_language() {
        let folder = tempfile::tempdir().unwrap();
        for language in Language::ALL {
            for (width, menu) in [(980.0, MENU_W), (420.0, 388.0)] {
                for track in [false, true] {
                    let (ctx, mut app) = app(language);
                    app.wav = Some(folder.path().join("song.wav"));
                    if track {
                        attach_track(&mut app, folder.path());
                        app.project.apply_subtitles = true;
                    }
                    frame(&ctx, &mut app, width, vec![]);
                    egui::Popup::open_id(&ctx, egui::Id::new(MENU));
                    frame(&ctx, &mut app, width, vec![]);
                    let texts = frame(&ctx, &mut app, width, vec![]);
                    let area = ctx
                        .memory(|m| m.area_rect(egui::Id::new(MENU)))
                        .expect("menu area");
                    let code = language.code();
                    assert!(
                        area.width() <= menu + 1.0 && area.left() >= 0.0 && area.right() <= width,
                        "{code} {width} track={track}: {area:?}"
                    );
                    for (text, rect) in &texts {
                        if area.contains(rect.center()) {
                            assert!(
                                rect.right() <= area.right() + 0.5,
                                "{code} {width} {text:?} {rect:?} outside {area:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The chip names its state and flags lyrics made from another song.
    #[test]
    fn chip_states_and_the_check_again_tag() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(Language::Fr);
        app.wav = Some(folder.path().join("new.wav"));
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, "Ajouter");
        attach_track(&mut app, folder.path());
        app.subtitles.artifact = Some(app_subtitles::Artifact {
            path: app.project.track.path.clone().unwrap(),
            revision: 0,
            cues: 39,
        });
        app.subtitles.source = folder.path().join("old.wav").display().to_string();
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, "39 lignes");
        find(&texts, "à revérifier");
        app.subtitles.source = folder.path().join("new.wav").display().to_string();
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        assert!(!texts.iter().any(|(t, _)| t == "à revérifier"));
        app.project.track.loaded = None;
        app.project.track.error = Some(noh::engine::EngineError::new(
            "error.caption_track",
            "read_subtitles",
            None,
            "Cue 3 must end after its start and within the source duration",
        ));
        let texts = frame(&ctx, &mut app, 980.0, vec![]);
        find(&texts, "Fichier illisible");
        assert_eq!(app.lyrics_state(), Chip::Unreadable);
    }
}
