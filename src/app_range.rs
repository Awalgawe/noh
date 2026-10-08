//! The row under the timeline: "Choose a short", or the short's
//! times, framing and restart; the zoom group on the right. The times popover
//! edits the range by typing. No media work or IO.
use super::*;
use app_icons::Icon;
use app_style::{self as style, Kind, space};
use egui::{Align, FontId, Layout, Rect, Sense, Stroke, pos2, vec2};
use noh::{shorts::Framing, timeline::Range};

pub(super) const TIMES: &str = "range-times";
const TIMES_W: f32 = 320.0;

/// The times popover's fields: typed text, per-field errors, focus on open.
#[derive(Default)]
pub struct Times {
    pub start_text: String,
    pub end_text: String,
    pub start_error: Option<Message>,
    pub end_error: Option<Message>,
    /// The range the texts show; a drag or arrow move while open resyncs them.
    shown: Option<Range>,
    /// Open last frame (focus Start on open, commit on close).
    open: bool,
    focus_start: bool,
    preset_open: bool,
}

/// Typed time, snapped to 0.1 s so the display equals the value; `None`
/// with the reason when it cannot be used.
pub(super) fn typed_time(text: &str, duration_ms: u64) -> Result<u64, Message> {
    let ms = noh::timeline::parse_time(text).ok_or_else(|| Message::from("range.error_format"))?;
    if ms > duration_ms {
        return Err(Message::new(
            "range.error_bounds",
            &[app_timeline::clock(duration_ms, duration_ms, Language::En)],
        ));
    }
    let snapped = (ms + 50) / 100 * 100;
    Ok(snapped.min(duration_ms))
}

impl NohApp {
    /// Timeline and range row as one zone: the row follows the lanes directly
    /// (its own 12 px), and the lyrics lane and overview strip take room only
    /// when they exist.
    pub(super) fn timeline_zone(&mut self, ui: &mut egui::Ui, locked: bool) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.project_timeline(ui, locked);
            self.range_row(ui, locked);
        });
    }

    pub(super) fn range_row(&mut self, ui: &mut egui::Ui, locked: bool) {
        let l = self.locale.language;
        let duration = self.project.duration_ms;
        if duration == 0 {
            return;
        }
        let t = style::tokens(ui);
        let compact = ui.available_width() < style::COMPACT_BELOW;
        // The zoom group's width decides whether it fits beside the left group.
        let visible =
            app_timeline::visible_label(self.project.viewport, duration, l).filter(|_| !compact);
        let visible_width = visible.as_ref().map_or(0.0, |text| {
            ui.painter()
                .layout_no_wrap(
                    text.clone(),
                    FontId::proportional(style::text::SMALL),
                    t.text_3,
                )
                .size()
                .x
                + 6.0
        });
        // Separator 13, spacing, optional label, three 28 px buttons 2 px apart.
        let zoom_width = 13.0 + 2.0 + visible_width + 3.0 * style::ROW_ICON + 2.0 * 2.0 + 2.0;
        let width = ui.available_width();
        ui.add_space(space::M);
        #[cfg(test)]
        self.project_ui.range_buttons.clear();
        if let Some(range) = self.project.range {
            // The short's groups on one line with the zoom group at the right
            // when they fit (widths measured last frame); otherwise they wrap as
            // unbreakable groups and the zoom group takes its own line. egui's
            // wrapping layout does not wrap a `horizontal` group (it claims the
            // rest of the line), so the breaks are decided here.
            // Per language and mode, so a switch never reuses stale widths.
            let measured_id = ui.id().with(("range-tools-width", l.code(), compact));
            let measured: Option<[f32; 3]> = ui.data(|d| d.get_temp(measured_id));
            // When one line is tight (es, de at 980), drop first the visible
            // span, then the "Framing" label (its two segments name
            // themselves), before wrapping. Both widths come from the text, so
            // the choice cannot flip between frames.
            let label_width = if compact {
                0.0
            } else {
                ui.painter()
                    .layout_no_wrap(
                        l.text("range.framing").into(),
                        FontId::proportional(style::text::BODY),
                        t.text_2,
                    )
                    .size()
                    .x
                    + space::S
            };
            let zoom_short = zoom_width - visible_width;
            let one_row = measured.and_then(|w| {
                let tools = w.iter().sum::<f32>() + 2.0 * space::L + space::L;
                [(true, true), (false, true), (false, false)]
                    .into_iter()
                    .find(|&(span, label)| {
                        let label = if label { label_width } else { 0.0 };
                        let zoom = if span { zoom_width } else { zoom_short };
                        tools + label + zoom <= width
                    })
            });
            let widths = if let Some((span, label)) = one_row {
                let rect = ui.allocate_space(vec2(width, style::CONTROL_H + 8.0)).1;
                let mut left = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("range-tools")
                        .max_rect(rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                left.spacing_mut().item_spacing.x = space::L;
                let used = self.short_tools(&mut left, range, locked, compact, label, [false; 2]);
                let (zoom, span) = if span {
                    (zoom_width, visible.as_deref())
                } else {
                    (zoom_short, None)
                };
                self.place_zoom(ui, rect, zoom, span);
                used
            } else {
                // Before the first measurement, one group per line (the frame
                // is discarded); then a group moves down only when it would
                // pass the row's right edge.
                let mut breaks = [true; 2];
                // The zoom group joins the last line when it fits there.
                let mut shares_line = false;
                if let Some([first, framing, restart]) = measured {
                    let mut x = first;
                    for (index, w) in [framing + label_width, restart].into_iter().enumerate() {
                        breaks[index] = x + space::L + w > width;
                        x = if breaks[index] { w } else { x + space::L + w };
                    }
                    shares_line = x + space::L + zoom_width <= width;
                }
                let tools = ui.allocate_ui_with_layout(
                    vec2(width, style::CONTROL_H),
                    Layout::left_to_right(Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.set_max_width(width);
                        ui.spacing_mut().item_spacing = vec2(space::L, space::S);
                        self.short_tools(ui, range, locked, compact, !compact, breaks)
                    },
                );
                let rect = if shares_line {
                    let bottom = tools.response.rect.bottom();
                    Rect::from_min_size(
                        pos2(tools.response.rect.left(), bottom - style::CONTROL_H),
                        vec2(width, style::CONTROL_H),
                    )
                } else {
                    ui.add_space(space::S);
                    ui.allocate_space(vec2(width, style::CONTROL_H)).1
                };
                self.place_zoom(ui, rect, zoom_width, visible.as_deref());
                tools.inner
            };
            let changed = measured.is_none_or(|old| {
                old.iter()
                    .zip(widths)
                    .any(|(old, new)| (old - new).abs() > 0.5)
            });
            if changed {
                ui.data_mut(|d| d.insert_temp(measured_id, widths));
                ui.ctx().request_discard("range row measured");
            }
            return;
        }
        let text = |key: &str| {
            ui.painter()
                .layout_no_wrap(
                    l.text(key).into(),
                    FontId::proportional(style::text::BODY),
                    t.text,
                )
                .size()
                .x
        };
        let left_width =
            20.0 + 18.0 + space::S + text("range.choose") + space::L + text("range.hint");
        let one_row = left_width + space::L + zoom_width <= width;
        let row = |ui: &mut egui::Ui, app: &mut Self| {
            ui.horizontal_wrapped(|ui| {
                ui.set_min_height(style::CONTROL_H);
                ui.spacing_mut().item_spacing.x = space::L;
                let choose = style::icon_text_button(
                    ui,
                    Icon::Plus,
                    l.text("range.choose"),
                    Kind::Quiet,
                    !locked,
                );
                let choose = if locked {
                    choose.on_hover_text(l.text("bar.locked"))
                } else {
                    choose
                };
                #[cfg(test)]
                app.project_ui
                    .range_buttons
                    .push(("range.choose", choose.rect));
                if choose.clicked() {
                    let range = app_timeline::place_short(app.project.viewport, duration);
                    if app.project.set_range(range)
                        && let Some(body) = app.project_ui.timeline.body_id
                    {
                        ui.memory_mut(|memory| memory.request_focus(body));
                    }
                }
                ui.label(egui::RichText::new(l.text("range.hint")).color(t.text_2));
            });
        };
        if one_row {
            let rect = ui.allocate_space(vec2(width, style::CONTROL_H + 8.0)).1;
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("range-left")
                    .max_rect(rect)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            row(&mut left, self);
            self.place_zoom(ui, rect, zoom_width, visible.as_deref());
        } else {
            row(ui, self);
            let rect = ui.allocate_space(vec2(width, style::CONTROL_H)).1;
            self.place_zoom(ui, rect, zoom_width, visible.as_deref());
        }
    }

    /// The zoom group at the right end of `rect`.
    fn place_zoom(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        zoom_width: f32,
        visible: Option<&str>,
    ) {
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("range-zoom")
                .max_rect(Rect::from_min_max(
                    pos2(rect.right() - zoom_width, rect.top()),
                    rect.max,
                ))
                .layout(Layout::left_to_right(Align::Center)),
        );
        self.zoom_group(&mut right, visible);
    }

    /// Separator, visible span, −, + and Show all, left to right so Tab
    /// reaches them in reading order.
    fn zoom_group(&mut self, ui: &mut egui::Ui, visible: Option<&str>) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let duration = self.project.duration_ms;
        let dragging = self.project_ui.timeline.dragging();
        ui.spacing_mut().item_spacing.x = 2.0;
        let (bar, _) = ui.allocate_exact_size(vec2(13.0, 24.0), Sense::hover());
        ui.painter().line_segment(
            [
                pos2(bar.left() + 0.5, bar.top()),
                pos2(bar.left() + 0.5, bar.bottom()),
            ],
            Stroke::new(1.0, t.border),
        );
        if let Some(text) = visible {
            ui.label(
                egui::RichText::new(text)
                    .size(style::text::SMALL)
                    .color(t.text_3),
            );
            ui.add_space(4.0);
        }
        for (icon, key, factor) in [
            (Icon::Minus, "timeline.zoom_out", Some(0.5)),
            (Icon::Plus, "timeline.zoom_in", Some(2.0)),
            (Icon::Fit, "timeline.show_all", None),
        ] {
            let response = ui
                .add_enabled_ui(!dragging, |ui| {
                    style::icon_button(ui, icon, l.text(key), style::ROW_ICON)
                })
                .inner;
            #[cfg(test)]
            self.project_ui.range_buttons.push((key, response.rect));
            if response.clicked() {
                self.project.viewport = match factor {
                    Some(factor) => app_timeline::zoom_by(
                        self.project.viewport,
                        factor,
                        self.project.range,
                        duration,
                    ),
                    None => noh::timeline::Viewport::full(duration),
                };
            }
        }
    }

    /// The short's groups: title, times button and ✕; framing; restart.
    /// `label` shows "Framing" before its segments; `breaks` starts framing
    /// and restart on a new line. Returns the three groups' widths (framing
    /// without its label), for next frame's row decision.
    fn short_tools(
        &mut self,
        ui: &mut egui::Ui,
        range: Range,
        locked: bool,
        compact: bool,
        label: bool,
        breaks: [bool; 2],
    ) -> [f32; 3] {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let duration = self.project.duration_ms;
        let before = (self.project.restart_loops, self.project.framing);
        let first = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            ui.label(style::strong(l.text("project.short"), style::text::BODY));
            let text = format!(
                "{} – {}",
                app_timeline::clock(range.start_ms, duration, l),
                app_timeline::clock(range.end_ms, duration, l)
            );
            let button = ui
                .add_enabled_ui(!locked, |ui| {
                    times_button(
                        ui,
                        &text,
                        egui::Popup::is_id_open(ui.ctx(), egui::Id::new(TIMES)),
                    )
                })
                .inner;
            #[cfg(test)]
            self.project_ui
                .range_buttons
                .push(("range.times", button.rect));
            let button = if locked {
                button.on_disabled_hover_text(l.text("bar.locked"))
            } else {
                button.on_hover_text(l.text("range.times_help"))
            };
            self.times_popover(ui, &button, range, compact, locked);
            let remove = ui
                .add_enabled_ui(!locked, |ui| {
                    style::icon_button(ui, Icon::Close, l.text("range.remove"), style::ROW_ICON)
                })
                .inner;
            #[cfg(test)]
            self.project_ui
                .range_buttons
                .push(("range.remove", remove.rect));
            let remove = if locked {
                remove.on_disabled_hover_text(l.text("bar.locked"))
            } else {
                remove
            };
            if remove.clicked() {
                // The montage, waveform, track and exports stay; the monitor
                // returns to the Video view.
                self.project.set_range(None);
                self.project.short_preview = None;
                self.project.short_preview_artifact = None;
                if self.mini_preview.short {
                    self.mini_preview.short = false;
                    self.mini_preview.invalidate();
                }
            }
        });
        if breaks[0] {
            ui.end_row();
        }
        let framing = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            let mut label_width = 0.0;
            if label {
                let text = ui.label(egui::RichText::new(l.text("range.framing")).color(t.text_2));
                label_width = text.rect.width() + ui.spacing().item_spacing.x;
            }
            let selected = usize::from(self.project.framing == Framing::Crop);
            let choice = ui
                .add_enabled_ui(!locked, |ui| {
                    style::segmented(
                        ui,
                        "range-framing",
                        &[l.text("range.fit"), l.text("range.fill")],
                        selected,
                    )
                })
                .inner;
            if locked {
                ui.interact(
                    ui.min_rect(),
                    ui.id().with("locked-framing-reason"),
                    Sense::hover(),
                )
                .on_hover_text(l.text("bar.locked"));
            }
            if let Some(index) = choice {
                self.project.framing = if index == 1 {
                    Framing::Crop
                } else {
                    Framing::Pad
                };
            }
            label_width
        });
        if breaks[1] {
            ui.end_row();
        }
        let restart = ui
            .add_enabled_ui(!locked, |ui| {
                style::checkbox(ui, &mut self.project.restart_loops, l.text("range.restart"))
            })
            .inner;
        #[cfg(test)]
        {
            self.project_ui
                .range_buttons
                .push(("range.framing", framing.response.rect));
            self.project_ui
                .range_buttons
                .push(("range.restart", restart.rect));
        }
        let widths = [
            first.response.rect.width(),
            framing.response.rect.width() - framing.inner,
            restart.rect.width(),
        ];
        if locked {
            restart.on_disabled_hover_text(l.text("bar.locked"));
        } else {
            restart.on_hover_text(l.text("range.restart_help"));
        }
        if before != (self.project.restart_loops, self.project.framing) {
            // Only the short changes, with one revision.
            self.project.short_revision = self.project.short_revision.wrapping_add(1);
            self.project.failure = None;
            self.project.outcome = None;
        }
        widths
    }

    /// Times popover: Start and End fields committed on Enter or focus
    /// loss, snapped to 0.1 s; errors keep the typed text; Escape discards.
    fn times_popover(
        &mut self,
        ui: &mut egui::Ui,
        button: &egui::Response,
        range: Range,
        compact: bool,
        locked: bool,
    ) {
        let l = self.locale.language;
        let ctx = ui.ctx().clone();
        let id = egui::Id::new(TIMES);
        if locked {
            egui::Popup::close_id(&ctx, id);
            self.project_ui.times.open = false;
            self.project_ui.times.preset_open = false;
            return;
        }
        let duration = self.project.duration_ms;
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let was_open = self.project_ui.times.open;
        if was_open && escape {
            // Discard what was not committed, close, and return to the button.
            egui::Popup::close_id(&ctx, id);
            self.project_ui.times.shown = None;
            button.request_focus();
        }
        // Opened by the capture harness, or by this click (the toggle happens
        // inside `show`, after the fields would have kept stale text).
        let opening = !was_open && (egui::Popup::is_id_open(&ctx, id) || button.clicked());
        if opening || (self.project_ui.times.shown != Some(range) && !self.times_editing(&ctx)) {
            let times = &mut self.project_ui.times;
            times.start_text = app_timeline::clock(range.start_ms, duration, l);
            times.end_text = app_timeline::clock(range.end_ms, duration, l);
            times.start_error = None;
            times.end_error = None;
            times.shown = Some(range);
            if opening {
                times.focus_start = true;
            }
        }
        // Compact: the content width, between the 16 px margins, so the
        // popover is anchored on that span rather than on the button.
        let content = ui.ctx().content_rect();
        let (width, anchor) = if compact {
            let span = Rect::from_x_y_ranges(
                content.left() + space::L..=content.right() - space::L,
                button.rect.y_range(),
            );
            (span.width(), span)
        } else {
            (TIMES_W, button.rect)
        };
        // egui keeps only one memory-owned popup per viewport. The nested
        // ComboBox owns that slot while open, so retain the parent locally.
        let mut open = was_open || egui::Popup::is_id_open(&ctx, id);
        if button.clicked() {
            open = !was_open;
        }
        let nested = self.project_ui.times.preset_open;
        let shown = egui::Popup::from_toggle_button_response(button)
            .id(id)
            .open_bool(&mut open)
            .anchor(anchor)
            .align(egui::RectAlign::TOP_START)
            .close_behavior(if nested {
                egui::PopupCloseBehavior::IgnoreClicks
            } else {
                egui::PopupCloseBehavior::CloseOnClickOutside
            })
            .frame(style::popup_frame(ui).inner_margin(16.0))
            .width(width - 34.0)
            .show(|ui| {
                ui.set_width(width - 34.0);
                egui::ScrollArea::vertical()
                    .max_height((content.height() - 64.0).max(120.0))
                    .show(ui, |ui| {
                        self.times_fields(ui, range, width - 34.0);
                        ui.add_space(space::M);
                        ui.separator();
                        ui.add_space(space::S);
                        self.short_preset_fields(ui, range);
                    });
            });
        let open = open && shown.is_some();
        if open && !self.project_ui.times.preset_open {
            egui::Popup::open_id(&ctx, id);
        } else if !open {
            egui::Popup::close_id(&ctx, id);
            self.project_ui.times.preset_open = false;
        }
        if was_open && !open && !escape {
            // A click outside closes after committing valid text.
            for start in [true, false] {
                self.commit_time(start);
            }
        }
        self.project_ui.times.open = open;
    }

    fn short_preset_fields(&mut self, ui: &mut egui::Ui, range: Range) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        ui.label(style::strong(l.text("preset.label"), style::text::BODY));
        let mut preset = self.project.short_safe_area;
        let _combo = egui::ComboBox::from_id_salt("project-short-preset")
            .width(ui.available_width())
            .selected_text(l.text(preset.key()))
            .show_ui(ui, |ui| {
                for area in noh::safe_area::SafeArea::ALL {
                    let _option = ui.selectable_value(&mut preset, area, l.text(area.key()));
                    #[cfg(test)]
                    self.project_ui
                        .range_buttons
                        .push((area.key(), _option.rect));
                }
            });
        self.project.set_short_safe_area(preset);
        self.project_ui.times.preset_open = egui::ComboBox::is_open(ui.ctx(), _combo.response.id);
        #[cfg(test)]
        self.project_ui
            .range_buttons
            .push(("preset.combo", _combo.response.rect));
        ui.add_space(space::S);
        let mut guides = !self.project.hide_safe_area;
        ui.add_enabled_ui(preset != noh::safe_area::SafeArea::None, |ui| {
            let response = style::checkbox(ui, &mut guides, l.text("preset.guides"));
            #[cfg(test)]
            self.project_ui
                .range_buttons
                .push(("preset.guides", response.rect));
            if response.changed() {
                self.project.hide_safe_area = !guides;
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(l.text("preset.help"))
                    .size(style::text::SMALL)
                    .color(t.text_2),
            )
            .wrap(),
        );
        ui.label(
            egui::RichText::new(l.text("preset.format"))
                .size(style::text::SMALL)
                .color(t.text_2),
        );
        if preset.exceeds_short_duration(range.duration_ms()) {
            ui.add(
                egui::Label::new(egui::RichText::new(l.text("preset.duration")).color(t.warn))
                    .wrap(),
            );
        }
    }

    /// A field of the popover has the keyboard: keep what is being typed.
    fn times_editing(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| {
            m.has_focus(egui::Id::new("range-start")) || m.has_focus(egui::Id::new("range-end"))
        })
    }

    fn times_fields(&mut self, ui: &mut egui::Ui, range: Range, width: f32) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let _ = range;
        let column = (width - space::L) / 2.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = space::L;
            for start in [true, false] {
                ui.vertical(|ui| {
                    ui.set_width(column);
                    ui.spacing_mut().item_spacing.y = space::XS;
                    ui.label(
                        egui::RichText::new(l.text(if start {
                            "range.start"
                        } else {
                            "range.end"
                        }))
                        .size(style::text::SMALL)
                        .color(t.text_2),
                    );
                    let times = &mut self.project_ui.times;
                    let (text, error) = if start {
                        (&mut times.start_text, &times.start_error)
                    } else {
                        (&mut times.end_text, &times.end_error)
                    };
                    let invalid = error.is_some();
                    let field_id = egui::Id::new(if start { "range-start" } else { "range-end" });
                    let response = ui.add(
                        egui::TextEdit::singleline(text)
                            .id(field_id)
                            .desired_width(column - 20.0)
                            .font(FontId::proportional(style::text::BODY))
                            .margin(vec2(10.0, 8.0)),
                    );
                    if invalid {
                        ui.painter().rect_stroke(
                            response.rect,
                            style::radius::M,
                            Stroke::new(2.0, t.error),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let error_text = error.as_ref().map(|e| e.render(l));
                    if let Some(error_text) = error_text {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(error_text)
                                    .size(style::text::SMALL)
                                    .color(t.error),
                            )
                            .wrap(),
                        );
                    }
                    // Opening focuses Start, once it holds (first frame is a sizing pass).
                    if start && self.project_ui.times.focus_start {
                        if response.has_focus() {
                            self.project_ui.times.focus_start = false;
                        } else {
                            response.request_focus();
                        }
                    }
                    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if response.lost_focus() && !escape {
                        self.commit_time(start);
                    }
                });
            }
        });
        ui.add_space(space::S);
        ui.add(
            egui::Label::new(
                egui::RichText::new(l.text("range.popover_help"))
                    .size(style::text::SMALL)
                    .color(t.text_2),
            )
            .wrap(),
        );
    }

    /// Commit one field: parse, snap to 0.1 s, check bounds and order; on
    /// success the range changes once (one short revision), else the typed
    /// text stays with its reason. No silent clamping.
    pub(super) fn commit_time(&mut self, start: bool) {
        let Some(range) = self.project.range else {
            return;
        };
        let l = self.locale.language;
        let duration = self.project.duration_ms;
        let times = &self.project_ui.times;
        let text = if start {
            &times.start_text
        } else {
            &times.end_text
        };
        let result = typed_time(text, duration)
            .map_err(|error| {
                if error.key == "range.error_bounds" {
                    Message::new(
                        "range.error_bounds",
                        &[app_timeline::clock(duration, duration, l)],
                    )
                } else {
                    error
                }
            })
            .and_then(|ms| {
                let (s, e) = if start {
                    (ms, range.end_ms)
                } else {
                    (range.start_ms, ms)
                };
                Range::new(s, e, duration)
                    .filter(|r| r.duration_ms() >= 100.min(duration))
                    .ok_or_else(|| Message::from("range.error_order"))
            });
        let times = &mut self.project_ui.times;
        match result {
            Ok(new) => {
                if start {
                    times.start_error = None;
                    times.start_text = app_timeline::clock(new.start_ms, duration, l);
                } else {
                    times.end_error = None;
                    times.end_text = app_timeline::clock(new.end_ms, duration, l);
                }
                if new != range {
                    self.project.set_range(Some(new));
                }
                self.project_ui.times.shown = Some(new);
            }
            Err(error) => {
                if start {
                    times.start_error = Some(error);
                } else {
                    times.end_error = Some(error);
                }
            }
        }
    }
}

/// Times button `0:13,0 – 0:18,0 ▾` (secondary, tabular digits).
fn times_button(ui: &mut egui::Ui, text: &str, open: bool) -> egui::Response {
    let t = style::tokens(ui);
    let color = if ui.is_enabled() { t.text } else { t.text_3 };
    let galley =
        ui.painter()
            .layout_no_wrap(text.into(), FontId::proportional(style::text::BODY), color);
    let size = vec2(12.0 + galley.size().x + 8.0 + 16.0 + 10.0, style::CONTROL_H);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    let fill = if response.hovered() || open {
        t.surface_3
    } else {
        t.surface_2
    };
    ui.painter().rect_filled(rect, style::radius::M, fill);
    ui.painter().galley(
        pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    app_icons::paint(
        ui.painter(),
        Rect::from_min_size(
            pos2(rect.right() - 26.0, rect.center().y - 8.0),
            vec2(16.0, 16.0),
        ),
        Icon::ChevronDown,
        t.text_2,
    );
    style::focus(ui, &response);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_range_row_explains_the_lock_and_keeps_zoom_usable() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.project.duration_ms = 20_000;
        let mut frame = |events: Vec<egui::Event>, time: f64| {
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(932.0, 400.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    style::apply(ui.ctx());
                    ui.ctx().all_styles_mut(|style| {
                        style.interaction.tooltip_delay = 0.0;
                        style.interaction.show_tooltips_only_when_still = false;
                    });
                    app.range_row(ui, true);
                },
            );
            rendered.textures_delta.clear();
            let choose = button(&app, "range.choose");
            let zoom = button(&app, "timeline.zoom_in");
            (rendered.shapes, choose, zoom)
        };
        let (_, choose, _) = frame(vec![], 0.0);
        let choose = choose.center();
        frame(vec![egui::Event::PointerMoved(choose)], 0.1);
        let (shapes, _, zoom) = frame(vec![], 1.0);
        let texts: Vec<_> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            shapes.iter().any(|s| match &s.shape {
                egui::Shape::Text(text) => text.galley.text() == Language::En.text("bar.locked"),
                _ => false,
            }),
            "texts={texts:?}, pointer={:?}",
            ctx.input(|i| i.pointer.hover_pos())
        );
        let zoom = zoom.center();
        frame(vec![egui::Event::PointerMoved(zoom)], 1.1);
        frame(
            vec![egui::Event::PointerButton {
                pos: zoom,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            1.2,
        );
        frame(
            vec![egui::Event::PointerButton {
                pos: zoom,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            1.3,
        );
        drop(frame);
        assert!(app.project.range.is_none());
        assert!(app.project.viewport.unwrap().0.duration_ms() < 20_000);
    }

    #[test]
    fn locked_existing_short_times_explain_the_export_lock_on_hover() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.project.duration_ms = 20_000;
        app.project.range = Range::new(5000, 10_000, 20_000);
        let mut frame = |events: Vec<egui::Event>, time: f64| {
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(932.0, 400.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    style::apply(ui.ctx());
                    ui.ctx().all_styles_mut(|style| {
                        style.interaction.tooltip_delay = 0.0;
                        style.interaction.show_tooltips_only_when_still = false;
                    });
                    app.range_row(ui, true);
                },
            );
            rendered.textures_delta.clear();
            (rendered.shapes, button(&app, "range.times"))
        };
        frame(vec![], 0.0);
        let (_, times) = frame(vec![], 0.1);
        frame(vec![egui::Event::PointerMoved(times.center())], 0.2);
        let (shapes, _) = frame(vec![], 1.0);
        assert!(shapes.iter().any(|s| match &s.shape {
            egui::Shape::Text(text) => text.galley.text() == Language::En.text("bar.locked"),
            _ => false,
        }));
    }

    fn frame(ctx: &egui::Context, app: &mut NohApp, width: f32, events: Vec<egui::Event>) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                app_style::apply(ui.ctx());
                app.range_row(ui, false);
                assert!(
                    ui.min_rect().right() <= width + 0.5,
                    "{width} {}: {:?} {:?}",
                    app.locale.language.code(),
                    ui.min_rect(),
                    app.project_ui.range_buttons
                );
            },
        );
        output.textures_delta.clear();
    }
    fn button(app: &NohApp, key: &str) -> egui::Rect {
        app.project_ui
            .range_buttons
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{key} not drawn"))
    }
    fn click(ctx: &egui::Context, app: &mut NohApp, key: &str) {
        let at = button(app, key).center();
        frame(ctx, app, 932.0, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                932.0,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
    }

    #[test]
    fn choose_a_short_places_fifteen_seconds_and_zoom_buttons_follow_reading_order() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.project.duration_ms = 182_400;
        frame(&ctx, &mut app, 932.0, vec![]);
        // Creation order is the Tab order: Choose a short, then −, +, Show all,
        // laid out left to right in the same order.
        let order: Vec<&str> = app
            .project_ui
            .range_buttons
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(
            order,
            [
                "range.choose",
                "timeline.zoom_out",
                "timeline.zoom_in",
                "timeline.show_all"
            ]
        );
        let xs: Vec<f32> = app
            .project_ui
            .range_buttons
            .iter()
            .map(|(_, r)| r.center().x)
            .collect();
        assert!(xs.windows(2).all(|pair| pair[0] < pair[1]), "{xs:?}");
        click(&ctx, &mut app, "range.choose");
        let range = app.project.range.expect("a short");
        assert_eq!(range.duration_ms(), 15_000);
        assert_eq!(range.start_ms + range.duration_ms() / 2, 91_200);
        assert!(app.project.short_revision > 0);
        click(&ctx, &mut app, "timeline.zoom_in");
        let view = app.project.viewport.expect("zoomed");
        assert_eq!(view.0.duration_ms(), 91_200);
        // Zoom-in centres on the visible range and changes nothing else.
        assert!(view.0.start_ms <= range.start_ms && view.0.end_ms >= range.end_ms);
        assert_eq!(app.project.range, Some(range));
        click(&ctx, &mut app, "timeline.show_all");
        assert_eq!(app.project.viewport.unwrap().0.duration_ms(), 182_400);
    }

    /// Ready without lyrics: lanes end at the waveform and the row follows at
    /// 12 px; the lyrics lane and the overview add their height only when present.
    #[test]
    fn timeline_zone_reserves_no_room_for_absent_lanes() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let measure = |zoomed: bool| {
            let mut app = NohApp::default();
            app.locale.language = Language::Fr;
            app.project.duration_ms = 182_400;
            if zoomed {
                app.project.viewport =
                    noh::timeline::Range::new(0, 30_000, 182_400).map(noh::timeline::Viewport);
            }
            let mut zone = egui::Rect::NOTHING;
            for _ in 0..2 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(932.0, 600.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        app_style::apply(ui.ctx());
                        app.timeline_zone(ui, false);
                        zone = ui.min_rect();
                    },
                );
                output.textures_delta.clear();
            }
            let lanes_bottom = app.project_ui.timeline.lanes_bottom().expect("drawn");
            let choose = button(&app, "timeline.zoom_out");
            (zone, lanes_bottom, choose)
        };
        let (zone, lanes_bottom, zoom) = measure(false);
        // Ruler 20 + pictures 28 + gap 4 + waveform 64: no lyrics lane.
        assert_eq!(lanes_bottom - zone.top(), 116.0);
        // The row starts 12 px below the lanes; its 28 px buttons sit in 40 px.
        assert_eq!(zoom.center().y - lanes_bottom, 12.0 + 20.0);
        assert_eq!(zone.bottom() - lanes_bottom, 12.0 + 40.0);
        // Zoomed: the overview strip (4 + 14) is the only addition.
        let (zoomed, zoomed_lanes, _) = measure(true);
        assert_eq!(zoomed_lanes - zoomed.top(), 116.0);
        assert_eq!(zoomed.height() - zone.height(), 18.0);
    }

    #[test]
    fn range_row_fits_in_every_language_with_and_without_a_short() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        for language in Language::ALL {
            for width in [388.0, 932.0] {
                for short in [false, true] {
                    let mut app = NohApp::default();
                    app.locale.language = language;
                    app.project.duration_ms = 182_400;
                    if short {
                        app.project.range = noh::timeline::Range::new(13_000, 18_000, 182_400);
                        app.project.viewport = noh::timeline::Range::new(0, 30_000, 182_400)
                            .map(noh::timeline::Viewport);
                    }
                    // The second frame uses the width measured by the first.
                    frame(&ctx, &mut app, width, vec![]);
                    frame(&ctx, &mut app, width, vec![]);
                    let keys: &[&str] = if short {
                        &[
                            "range.times",
                            "range.remove",
                            "range.framing",
                            "range.restart",
                            "timeline.zoom_out",
                            "timeline.show_all",
                        ]
                    } else {
                        &["range.choose", "timeline.zoom_out", "timeline.show_all"]
                    };
                    for key in keys {
                        let rect = button(&app, key);
                        assert!(
                            rect.right() <= width + 0.5,
                            "{} {width} {key}: {rect:?}",
                            language.code()
                        );
                    }
                    // Groups that share a line never overlap (zoom beside restart).
                    let drawn = &app.project_ui.range_buttons;
                    for (i, (a, first)) in drawn.iter().enumerate() {
                        for (b, second) in &drawn[i + 1..] {
                            assert!(
                                !first.shrink(0.5).intersects(second.shrink(0.5)),
                                "{} {width}: {a} {first:?} overlaps {b} {second:?}",
                                language.code()
                            );
                        }
                    }
                }
            }
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    /// Select the field's text and type over it; `end` commits or not.
    fn type_text(ctx: &egui::Context, app: &mut NohApp, text: &str, end: Option<egui::Key>) {
        let mut events = vec![
            key(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Text(text.into()),
        ];
        if let Some(end) = end {
            events.push(key(end, egui::Modifiers::NONE));
        }
        frame(ctx, app, 932.0, events);
        frame(ctx, app, 932.0, vec![]);
    }
    fn short_app(language: Language) -> (egui::Context, NohApp) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        // No clock runs here: popups would stay mid fade-in.
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = NohApp::default();
        app.locale.language = language;
        app.project.duration_ms = 182_400;
        app.project.range = noh::timeline::Range::new(13_000, 18_000, 182_400);
        frame(&ctx, &mut app, 932.0, vec![]);
        frame(&ctx, &mut app, 932.0, vec![]);
        (ctx, app)
    }
    fn open_times(ctx: &egui::Context, app: &mut NohApp) {
        click(ctx, app, "range.times");
        for _ in 0..3 {
            frame(ctx, app, 932.0, vec![]);
        }
        assert!(egui::Popup::is_id_open(ctx, egui::Id::new(TIMES)));
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new("range-start"))),
            "opening focuses Start"
        );
    }
    fn range(app: &NohApp) -> (u64, u64) {
        let r = app.project.range.expect("a short");
        (r.start_ms, r.end_ms)
    }

    #[test]
    fn typed_times_accept_every_format_and_snap_to_a_tenth() {
        for (text, ms) in [
            ("13", 13_000),
            ("0:13", 13_000),
            ("0:13,5", 13_500),
            ("13.5", 13_500),
            ("0:13,55", 13_600),
            ("0:13,54", 13_500),
            (" 0:13 ", 13_000),
        ] {
            assert_eq!(typed_time(text, 182_400), Ok(ms), "{text}");
        }
        assert_eq!(typed_time("1:02:03,4", 4_000_000), Ok(3_723_400));
        assert_eq!(typed_time("3:02,4", 182_400), Ok(182_400));
        assert_eq!(
            typed_time("abc", 182_400).unwrap_err().key,
            "range.error_format"
        );
        assert_eq!(
            typed_time("4:00", 182_400).unwrap_err().key,
            "range.error_bounds"
        );
    }

    /// Real keys: Enter commits (snapped, one short revision), Tab away commits,
    /// errors keep the text without clamping, Escape discards and returns to
    /// the times button, a click outside commits valid text.
    #[test]
    fn times_popover_commits_explains_and_discards() {
        let (ctx, mut app) = short_app(Language::Fr);
        let revision = app.revision;
        open_times(&ctx, &mut app);
        let short_revision = app.project.short_revision;
        type_text(&ctx, &mut app, "0:13,54", Some(egui::Key::Enter));
        assert_eq!(range(&app), (13_500, 18_000), "Enter commits, snapped");
        assert_eq!(app.project.short_revision, short_revision.wrapping_add(1));
        assert_eq!(app.project_ui.times.start_text, "0:13,5");
        // Type in End, then Tab away: the blur commits.
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("range-end")));
        frame(&ctx, &mut app, 932.0, vec![]);
        type_text(&ctx, &mut app, "19", Some(egui::Key::Tab));
        assert_eq!(range(&app), (13_500, 19_000), "focus loss commits");
        // Errors: the typed text stays, the range does not move.
        for (text, error) in [
            ("abc", "range.error_format"),
            ("4:00", "range.error_bounds"),
            ("0:20", "range.error_order"),
        ] {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("range-start")));
            frame(&ctx, &mut app, 932.0, vec![]);
            type_text(&ctx, &mut app, text, Some(egui::Key::Enter));
            let times = &app.project_ui.times;
            assert_eq!(
                times.start_error.as_ref().map(|e| e.key.as_str()),
                Some(error)
            );
            assert_eq!(times.start_text, text, "the typed text stays");
            assert_eq!(range(&app), (13_500, 19_000), "no silent clamping");
        }
        // Escape discards the uncommitted text and closes.
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("range-start")));
        frame(&ctx, &mut app, 932.0, vec![]);
        type_text(&ctx, &mut app, "0:02", None);
        frame(
            &ctx,
            &mut app,
            932.0,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        frame(&ctx, &mut app, 932.0, vec![]);
        assert!(!egui::Popup::is_id_open(&ctx, egui::Id::new(TIMES)));
        assert_eq!(range(&app), (13_500, 19_000), "Escape discards");
        let focused = ctx
            .memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id));
        assert_eq!(
            focused.map(|r| r.rect),
            Some(button(&app, "range.times")),
            "focus returns to the times button"
        );
        // A click outside commits valid text.
        open_times(&ctx, &mut app);
        assert!(
            app.project_ui.times.start_error.is_none(),
            "reopening resets"
        );
        type_text(&ctx, &mut app, "0:14", None);
        let outside = egui::pos2(900.0, 390.0);
        frame(
            &ctx,
            &mut app,
            932.0,
            vec![egui::Event::PointerMoved(outside)],
        );
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut app,
                932.0,
                vec![egui::Event::PointerButton {
                    pos: outside,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        frame(&ctx, &mut app, 932.0, vec![]);
        assert!(!egui::Popup::is_id_open(&ctx, egui::Id::new(TIMES)));
        assert_eq!(range(&app), (14_000, 19_000), "a click outside commits");
        assert_eq!(app.revision, revision, "times change the short only");
    }

    /// Compact: the popover spans the content width between the 16 px
    /// margins, in every language, with an error shown under a field.
    #[test]
    fn compact_times_popover_stays_inside_the_margins() {
        for language in Language::ALL {
            let (ctx, mut app) = short_app(language);
            app.project_ui.times.start_error = Some(Message::from("range.error_format"));
            egui::Popup::open_id(&ctx, egui::Id::new(TIMES));
            for _ in 0..3 {
                frame(&ctx, &mut app, 420.0, vec![]);
            }
            let rect = ctx
                .memory(|m| m.area_rect(egui::Id::new(TIMES)))
                .expect("popover drawn");
            assert!(
                rect.left() >= 15.5 && rect.right() <= 404.5,
                "{}: {rect:?}",
                language.code()
            );
        }
    }

    #[test]
    fn preset_dropdown_applies_to_short_and_guide_toggle_is_display_only() {
        let (ctx, mut app) = short_app(Language::Fr);
        let full = app.project.stamp(app.revision, false);
        let range = app.project.range;
        let size = app.project.caption_style.size;
        open_times(&ctx, &mut app);
        click(&ctx, &mut app, "preset.combo");
        frame(&ctx, &mut app, 932.0, vec![]);
        click(&ctx, &mut app, "preset.youtube");
        frame(&ctx, &mut app, 932.0, vec![]);
        assert_eq!(
            app.project.short_safe_area,
            noh::safe_area::SafeArea::YoutubeShorts
        );
        assert_eq!(app.project.range, range);
        assert_eq!(app.project.caption_style.size, size);
        assert_eq!(app.project.stamp(app.revision, false), full);
        let short = app.project.stamp(app.revision, true);
        click(&ctx, &mut app, "preset.guides");
        assert!(app.project.hide_safe_area);
        assert_eq!(app.project.stamp(app.revision, true), short);
    }

    #[test]
    fn bounds_error_names_the_song_end_in_the_interface_format() {
        let (ctx, mut app) = short_app(Language::Fr);
        open_times(&ctx, &mut app);
        type_text(&ctx, &mut app, "4:00", Some(egui::Key::Enter));
        let error = app.project_ui.times.start_error.clone().expect("bounds");
        assert_eq!(error.key, "range.error_bounds");
        assert_eq!(error.args, ["3:02,4"]);
    }

    /// Framing and restart change the short only; ✕ clears the range and
    /// returns the monitor to the Video view.
    #[test]
    fn framing_restart_and_remove() {
        let (ctx, mut app) = short_app(Language::En);
        let revision = app.revision;
        let short_revision = app.project.short_revision;
        let framing = button(&app, "range.framing");
        let fill = egui::pos2(framing.right() - 20.0, framing.center().y);
        frame(&ctx, &mut app, 932.0, vec![egui::Event::PointerMoved(fill)]);
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut app,
                932.0,
                vec![egui::Event::PointerButton {
                    pos: fill,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        assert_eq!(app.project.framing, Framing::Crop);
        assert_eq!(app.project.short_revision, short_revision.wrapping_add(1));
        click(&ctx, &mut app, "range.restart");
        assert!(app.project.restart_loops);
        assert_eq!(app.project.short_revision, short_revision.wrapping_add(2));
        assert_eq!(app.revision, revision, "the video is untouched");
        app.mini_preview.short = true;
        click(&ctx, &mut app, "range.remove");
        assert!(app.project.range.is_none());
        assert!(!app.mini_preview.short, "the monitor returns to Video");
    }

    /// "Choose a short" moves the keyboard to the range body.
    #[test]
    fn choose_a_short_focuses_the_range_body() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.project.duration_ms = 182_400;
        let run = |app: &mut NohApp, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(932.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app_style::apply(ui.ctx());
                    app.timeline_zone(ui, false);
                },
            );
            output.textures_delta.clear();
        };
        run(&mut app, vec![]);
        let at = button(&app, "range.choose").center();
        run(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            run(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        run(&mut app, vec![]);
        assert_eq!(app.project.range.map(|r| r.duration_ms()), Some(15_000));
        let body = app.project_ui.timeline.body_id.expect("range body");
        assert!(ctx.memory(|m| m.has_focus(body)), "focus on the range body");
    }

    /// Restart adds the ↺ mark over the pictures lane; the lane itself is
    /// painted identically (REVIEW blocking item 1).
    #[test]
    fn pictures_lane_is_identical_with_restart_on_and_off() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let paint = |restart: bool| {
            let mut app = NohApp::default();
            app.locale.language = Language::En;
            app.project.duration_ms = 30_000;
            app.project.range = noh::timeline::Range::new(13_000, 18_000, 30_000);
            app.project.restart_loops = restart;
            app.clips = ["a.mp4", "b.mp4"]
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let mut info = noh::media::MediaInfo::default();
                    info.seconds = [4.0, 6.0][i];
                    Clip {
                        id: i as u64 + 1,
                        request_id: i as u64 + 1,
                        item: (*name).into(),
                        info: Some(info),
                        error: None,
                    }
                })
                .collect();
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(932.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        app_style::apply(ui.ctx());
                        ui.add_space(30.0);
                        app.project_timeline(ui, false);
                    },
                );
                output.textures_delta.clear();
                shapes = output.shapes;
            }
            let lane = app.project_ui.timeline.pictures_rect().expect("lane");
            let mark = app.project_ui.timeline.restart_mark;
            assert_eq!(mark.is_some(), restart);
            shapes
                .iter()
                .filter(|s| {
                    let bounds = s.shape.visual_bounding_rect();
                    bounds.intersects(lane)
                        && !mark.is_some_and(|m| m.expand(1.0).contains_rect(bounds))
                })
                .map(|s| format!("{:?}", s.shape))
                .collect::<Vec<_>>()
        };
        let off = paint(false);
        let on = paint(true);
        assert!(!off.is_empty());
        assert_eq!(off, on);
    }

    /// 980 × 850: strip, stage, timeline and range row sit above the bar,
    /// without scrolling, in the ready and short states, with and without the
    /// lyrics lane, in every language.
    #[test]
    fn ready_and_short_fit_980_by_850_without_scrolling() {
        let folder = tempfile::tempdir().unwrap();
        // A silent 182.4 s PCM WAV: the track reader checks the song's length.
        let wav = folder.path().join("ma-chanson.wav");
        let data_bytes = 1_824_u32 * 800 * 2;
        let mut header = b"RIFF".to_vec();
        header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        header.extend_from_slice(&8_000_u32.to_le_bytes());
        header.extend_from_slice(&16_000_u32.to_le_bytes());
        header.extend_from_slice(b"\x02\0\x10\0data");
        header.extend_from_slice(&data_bytes.to_le_bytes());
        std::fs::write(&wav, header).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&wav)
            .unwrap()
            .set_len(44 + u64::from(data_bytes))
            .unwrap();
        for (language, short) in Language::ALL
            .into_iter()
            .flat_map(|language| [(language, false), (language, true)])
        {
            for lyrics in [false, true] {
                let ctx = egui::Context::default();
                app_locale::install_fonts(&ctx);
                let mut app = NohApp::default();
                app.locale.language = language;
                app.clips = ["vagues.mp4", "ville-nuit.mp4"]
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let mut info = noh::media::MediaInfo::default();
                        info.seconds = 5.0;
                        info.width = 1280;
                        info.height = 720;
                        Clip {
                            id: i as u64 + 1,
                            request_id: i as u64 + 1,
                            item: (*name).into(),
                            info: Some(info),
                            error: None,
                        }
                    })
                    .collect();
                app.wav = Some(wav.clone());
                app.wav_seconds = Some(182.4);
                app.project.duration_ms = 182_400;
                // A settled ready state: destination checked, diagnosis current
                // and no worker, so no late result changes the bar's height.
                app.output = Some(folder.path().join("ma-chanson_noh.mp4"));
                app.destination_result = Some(app_destination::ResultView {
                    path: app.output.clone().unwrap(),
                    issue: None,
                    suggestion: None,
                    bytes: None,
                });
                let request = app.diagnostic_request().unwrap();
                app.preflight = Some(app_diagnosis::Preflight::dormant(Some(request.clone())));
                app.diagnosis = Some(Box::new(noh::inspection::Diagnosis {
                    version: 1,
                    level: noh::inspection::Level::Exact,
                    request,
                    snapshot: noh::inspection::Snapshot(vec![]),
                    duration: 182.4,
                    container: "mp4".into(),
                    audio: "aac_320".into(),
                    quick_media: vec![],
                    notes: vec![],
                    plan: None,
                }));
                app.diagnosis_revision = Some(app.revision);
                if lyrics {
                    // Attached as a drop would: the app's own worker reads it.
                    let path = folder.path().join("ma-chanson.srt");
                    std::fs::write(&path, "1\n00:00:01,000 --> 00:00:02,000\nla\n\n").unwrap();
                    app.project.track.path = Some(path);
                }
                let mut viewport = egui::Rect::NOTHING;
                let mut draw = |app: &mut NohApp| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(980.0, 850.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            viewport = app.draw(ui).inner_rect;
                        },
                    );
                    output.textures_delta.clear();
                };
                // The first frame adopts the song (which resets range and
                // view); the track arrives from its worker.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    draw(&mut app);
                    if !lyrics || app.project.track.loaded.is_some() {
                        break;
                    }
                    assert!(std::time::Instant::now() < deadline, "track not read");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if short {
                    // Zoomed, so the overview strip is drawn as well.
                    app.project.range = noh::timeline::Range::new(13_000, 18_000, 182_400);
                    app.project.viewport =
                        noh::timeline::Range::new(0, 30_000, 182_400).map(noh::timeline::Viewport);
                }
                for _ in 0..3 {
                    draw(&mut app);
                }
                let case = format!("{} short={short} lyrics={lyrics}", language.code());
                assert_eq!(app.project.track.loaded.is_some(), lyrics, "{case}");
                if short {
                    let view = app.project.viewport.expect("zoomed");
                    assert_eq!(view.0.duration_ms(), 30_000, "{case}");
                }
                let last = if short {
                    "range.restart"
                } else {
                    "range.choose"
                };
                for key in [last, "timeline.zoom_out"] {
                    let rect = button(&app, key);
                    assert!(
                        rect.bottom() <= viewport.bottom() && rect.right() <= 956.5,
                        "{case}: {key} {rect:?} outside {viewport:?}"
                    );
                }
            }
        }
    }
}
