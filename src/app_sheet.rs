//! The location sheet: file name, short file name, folder and
//! Details (Mode, output summary, per-clip detail, processing log). Edits are
//! drafts until "Export", which applies them and starts that export.
use super::*;
use app_destination::{Destination, Issue, ResultView};
use app_icons::Icon;
use app_style::{self as style, Kind, Severity, space};
use egui::{Align, Align2, FontId, Layout, Rect, Sense, Stroke, vec2};

const WIDTH: f32 = 460.0;
const NAME: &str = "sheet-name";
const SHORT_NAME: &str = "sheet-short-name";
/// Reasons that belong to the pre-filled destination: the sheet checks its
/// own drafts instead.
const DESTINATION_REASONS: &[&str] = &[
    "ui.destination_checking",
    "ui.name_empty",
    "ui.name_invalid",
    "error.mp4",
    "error.output_folder",
    "ui.destination_unavailable",
    "error.output_exists",
];

#[derive(Default)]
pub struct Sheet {
    pub open: bool,
    /// "Export" exports the short (opened from its conflict) instead of the video.
    pub short: bool,
    name: String,
    short_name: String,
    folder: PathBuf,
    /// The person typed a name or picked a folder: the name stops being automatic.
    touched: bool,
    short_touched: bool,
    name_check: Option<Destination>,
    short_check: Option<Destination>,
    name_result: Option<ResultView>,
    short_result: Option<ResultView>,
    /// Details stays as the person left it, for the session.
    pub details: bool,
    /// Focus goes back here on close ("Change…" or the export button).
    return_focus: Option<egui::Id>,
    focus_name: bool,
    focus_confirm: bool,
    /// "Export" in the footer, for Enter in a name field.
    confirm_id: Option<egui::Id>,
    /// Widgets of the last frame, for tests.
    #[cfg(test)]
    pub rects: Vec<(&'static str, Rect)>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
/// "25", "29,97": at most three decimals, without trailing zeros.
fn frame_rate(num: i64, den: i64, l: Language) -> String {
    let text = format!("{:.3}", num as f64 / den.max(1) as f64);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    l.localize_decimal(text.to_owned())
}
fn short_default(main: &str) -> String {
    format!(
        "{}-short.mp4",
        Path::new(main)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
    )
}
/// A draft name's problem: its syntax, then the background check.
fn draft_issue(name: &str, result: Option<&ResultView>) -> Option<Message> {
    if let Some(issue) = app_destination::syntax(name) {
        return Some(issue.key().into());
    }
    match result {
        None => Some("ui.destination_checking".into()),
        Some(result) => result.issue.as_ref().map(|issue| {
            if *issue == Issue::Exists {
                "sheet.exists".into()
            } else {
                issue.key().into()
            }
        }),
    }
}

impl NohApp {
    /// Opens the sheet on the current destination. `short`: Export exports the short.
    pub(super) fn open_sheet(&mut self, short: bool, from: Option<egui::Id>) {
        let short_name = file_name(
            &self
                .project
                .short_output(&self.output_name, &self.output_folder),
        );
        let sheet = &mut self.sheet;
        sheet.open = true;
        sheet.short = short && self.project.range.is_some();
        sheet.name = self.output_name.clone();
        sheet.short_name = short_name;
        sheet.folder = self.output_folder.clone();
        sheet.touched = false;
        sheet.short_touched = false;
        sheet.name_result = None;
        sheet.short_result = None;
        sheet.return_focus = from;
        sheet.focus_name = true;
        sheet.focus_confirm = false;
    }

    fn close_sheet(&mut self, ctx: &egui::Context) {
        self.sheet.open = false;
        for check in [&self.sheet.name_check, &self.sheet.short_check]
            .into_iter()
            .flatten()
        {
            check.update(None, false);
        }
        if let Some(id) = self.sheet.return_focus.take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    /// Background checks of the drafts; an automatic name that is taken gives
    /// way to the first free one, as in the bar.
    fn check_sheet(&mut self, ctx: &egui::Context) {
        let range = self.project.range.is_some();
        let sheet = &mut self.sheet;
        let path = app_destination::syntax(&sheet.name)
            .is_none()
            .then(|| sheet.folder.join(&sheet.name));
        let check = sheet
            .name_check
            .get_or_insert_with(|| Destination::new(ctx.clone()));
        if check.update(path, false) {
            sheet.name_result = None;
        }
        if let Some(result) = check.receive() {
            sheet.name_result = Some(result);
        }
        if let Some(result) = &sheet.name_result
            && let Some(name) = app_destination::adopt(
                self.output_auto && !sheet.touched,
                &sheet.name,
                &self.output_base,
                result,
            )
        {
            sheet.name = name;
            sheet.name_result = None;
        }
        if !range {
            return;
        }
        let path = app_destination::syntax(&sheet.short_name)
            .is_none()
            .then(|| sheet.folder.join(&sheet.short_name));
        let check = sheet
            .short_check
            .get_or_insert_with(|| Destination::new(ctx.clone()));
        if check.update(path, false) {
            sheet.short_result = None;
        }
        if let Some(result) = check.receive() {
            sheet.short_result = Some(result);
        }
        if let Some(result) = &sheet.short_result
            && let Some(name) = app_destination::adopt(
                !self.project.short_named && !sheet.short_touched,
                &sheet.short_name,
                &short_default(&sheet.name),
                result,
            )
        {
            sheet.short_name = name;
            sheet.short_result = None;
        }
    }

    /// Both drafts have their background result (capture waits for it).
    pub(super) fn sheet_checked(&self) -> bool {
        self.sheet.name_result.is_some()
            && (self.project.range.is_none() || self.sheet.short_result.is_some())
    }

    /// Why "Export" is disabled: a draft name first, then what the bar says
    /// (the check, a file being read…), without its destination reasons.
    pub(super) fn sheet_reason(&self) -> Option<Message> {
        let sheet = &self.sheet;
        draft_issue(&sheet.name, sheet.name_result.as_ref())
            .or_else(|| {
                self.project
                    .range
                    .and_then(|_| draft_issue(&sheet.short_name, sheet.short_result.as_ref()))
            })
            .or_else(|| {
                let view = self.status();
                let availability = if sheet.short {
                    view.montage.and_then(|m| m.short).unwrap_or(view.export)
                } else {
                    view.export
                };
                (!availability.enabled
                    && !DESTINATION_REASONS.contains(&availability.reason.key.as_str()))
                .then_some(availability.reason)
            })
    }

    /// Applies the drafts, closes and starts the export.
    fn confirm_sheet(&mut self, ctx: &egui::Context) {
        let sheet = &mut self.sheet;
        let (name, folder, short_name) = (
            sheet.name.clone(),
            sheet.folder.clone(),
            sheet.short_name.clone(),
        );
        let (name_result, short_result) = (sheet.name_result.take(), sheet.short_result.take());
        let (touched, short_touched, short) = (sheet.touched, sheet.short_touched, sheet.short);
        if touched {
            self.output_auto = false;
        }
        self.output_name = name;
        self.output_folder = folder;
        self.destination_edited();
        // Already checked for exactly this path.
        self.destination_result = name_result;
        if self.project.range.is_some() {
            if short_touched {
                self.project.short_named = true;
            }
            self.project.short_name = if short_name == short_default(&self.output_name) {
                String::new()
            } else {
                short_name
            };
            self.project.short_destination_result = short_result;
        }
        self.close_sheet(ctx);
        self.start_project(ctx, false, short);
    }

    fn pick_sheet_folder(&mut self) {
        let l = self.locale.language;
        if let Some(folder) = rfd::FileDialog::new()
            .set_title(l.text("sheet.pick_folder"))
            .set_directory(&self.sheet.folder)
            .pick_folder()
        {
            self.sheet.folder = folder;
            self.sheet.touched = true;
            self.sheet.short_touched = true;
        }
    }

    /// The sheet as a modal: centred 460 wide (top 96, 56 with Details open),
    /// or a bottom sheet at full width in compact.
    pub(super) fn location_sheet(&mut self, ctx: &egui::Context) {
        if !self.sheet.open {
            return;
        }
        #[cfg(test)]
        self.sheet.rects.clear();
        self.check_sheet(ctx);
        let t = style::tokens_for(ctx.global_style().visuals.dark_mode);
        let content = ctx.content_rect();
        let compact = content.width() - 32.0 < style::COMPACT_BELOW;
        // Keys first: a field may consume Enter.
        let (enter, command_enter) = ctx.input(|i| {
            i.events
                .iter()
                .fold((false, false), |(plain, command), event| match event {
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } => (plain || !modifiers.command, command || modifiers.command),
                    _ => (plain, command),
                })
        });
        // A delayed name check must not steal focus after a newer user action.
        if self.sheet.focus_confirm
            && ctx.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::PointerButton { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                    )
                })
            })
        {
            self.sheet.focus_confirm = false;
        }
        let id = egui::Id::new("location-sheet");
        let (area, frame, width) = if compact {
            (
                egui::Modal::default_area(id).anchor(Align2::CENTER_BOTTOM, vec2(0.0, 0.0)),
                egui::Frame::NONE
                    .fill(t.surface)
                    .corner_radius(egui::CornerRadius {
                        nw: style::radius::XL,
                        ne: style::radius::XL,
                        sw: 0,
                        se: 0,
                    })
                    .shadow(t.shadow)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 16,
                        top: 20,
                        bottom: 16,
                    }),
                content.width() - 32.0,
            )
        } else {
            let top = if self.sheet.details { 56.0 } else { 96.0 };
            (
                egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, top)),
                egui::Frame::NONE
                    .fill(t.surface)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(style::radius::XL)
                    .shadow(t.shadow)
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 24,
                        top: 20,
                        bottom: 24,
                    }),
                WIDTH - 48.0,
            )
        };
        let max_height = (content.height() - if compact { 56.0 } else { 80.0 }).max(200.0);
        let mut close = false;
        let mut confirm = false;
        let response = egui::Modal::new(id)
            .area(area)
            .backdrop_color(t.scrim)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = space::L;
                self.sheet_head(ui, &mut close);
                // The area reuses last frame's size as its limit: give the body
                // its full height so it can grow when Details opens.
                ui.allocate_ui(vec2(width, max_height - 120.0), |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sheet-body")
                        .max_height(max_height - 120.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = space::L;
                            self.sheet_fields(ui, enter);
                            self.sheet_details(ui);
                        })
                });
                confirm = self.sheet_footer(ui, compact, &mut close);
            });
        if response.should_close() {
            close = true;
        }
        if command_enter && self.sheet_reason().is_none() {
            confirm = true;
        }
        if confirm && self.sheet_reason().is_none() {
            self.confirm_sheet(ctx);
        } else if close {
            // Name and folder edits are discarded; Mode applied at once.
            self.close_sheet(ctx);
        }
    }

    fn sheet_head(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        let l = self.locale.language;
        ui.horizontal(|ui| {
            ui.label(style::strong(l.text("sheet.title"), style::text::HEADING));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if style::icon_button(ui, Icon::Close, l.text("sheet.close"), style::ROW_ICON)
                    .clicked()
                {
                    *close = true;
                }
            });
        });
    }

    fn sheet_fields(&mut self, ui: &mut egui::Ui, enter: bool) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let range = self.project.range.is_some();
        for short in [false, true] {
            if short && !range {
                continue;
            }
            let (key, id) = if short {
                ("sheet.short_name", SHORT_NAME)
            } else {
                ("sheet.name", NAME)
            };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::S;
                ui.label(
                    egui::RichText::new(l.text(key))
                        .size(style::text::SMALL)
                        .color(t.text_2),
                );
                let sheet = &mut self.sheet;
                let (text, result) = if short {
                    (&mut sheet.short_name, sheet.short_result.as_ref())
                } else {
                    (&mut sheet.name, sheet.name_result.as_ref())
                };
                let issue =
                    app_destination::syntax(text).or_else(|| result.and_then(|r| r.issue.clone()));
                let suggestion = result.and_then(|r| r.suggestion.clone());
                let field_id = egui::Id::new(id);
                // Select before TextEdit consumes input. Selecting after drawing
                // races typing on the first focused frame, especially on reopen.
                if !short && sheet.focus_name {
                    let mut state =
                        egui::TextEdit::load_state(ui.ctx(), field_id).unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(text.chars().count()),
                        )));
                    state.store(ui.ctx(), field_id);
                }
                let response = ui.add_sized(
                    vec2(ui.available_width(), style::CONTROL_H),
                    egui::TextEdit::singleline(text)
                        .id(field_id)
                        .margin(vec2(10.0, 7.0)),
                );
                if !short && sheet.focus_name {
                    if response.has_focus() {
                        sheet.focus_name = false;
                    } else {
                        response.request_focus();
                    }
                }
                if issue.is_some() {
                    ui.painter().rect_stroke(
                        response.rect,
                        style::radius::M,
                        Stroke::new(2.0, t.error),
                        egui::StrokeKind::Inside,
                    );
                }
                style::focus(ui, &response);
                #[cfg(test)]
                sheet.rects.push((id, response.rect));
                if response.changed() {
                    if short {
                        sheet.short_touched = true;
                    } else {
                        sheet.touched = true;
                    }
                }
                // Enter commits the name and moves to "Export"; it never exports.
                if response.lost_focus() && enter {
                    sheet.focus_confirm = true;
                }
                if let Some(issue) = &issue {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        let (icon, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        app_icons::paint(ui.painter(), icon, Icon::Error, t.error);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(l.text(if *issue == Issue::Exists {
                                    "sheet.exists"
                                } else {
                                    issue.key()
                                }))
                                .color(t.error),
                            )
                            .wrap(),
                        );
                    });
                    if *issue == Issue::Exists
                        && let Some(name) = suggestion
                        && style::button(
                            ui,
                            l.format("ui.use_name", std::slice::from_ref(&name)),
                            true,
                            false,
                            0.0,
                        )
                        .clicked()
                    {
                        if short {
                            sheet.short_name = name;
                            sheet.short_touched = true;
                        } else {
                            sheet.name = name;
                            sheet.touched = true;
                        }
                    }
                }
            });
        }
        // Folder: read-only, middle-ellipsis path, "Change…" picks a folder.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = space::S;
            ui.label(
                egui::RichText::new(l.text("sheet.folder"))
                    .size(style::text::SMALL)
                    .color(t.text_2),
            );
            let mut pick = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                let change = l.text("bar.change");
                let button_width = ui
                    .painter()
                    .layout_no_wrap(change.into(), FontId::proportional(14.0), t.text)
                    .size()
                    .x
                    + 2.0 * ui.spacing().button_padding.x;
                let (field, response) = ui.allocate_exact_size(
                    vec2(
                        ui.available_width() - button_width - space::S,
                        style::CONTROL_H,
                    ),
                    Sense::hover(),
                );
                ui.painter().rect(
                    field,
                    style::radius::M,
                    t.input,
                    Stroke::new(1.0, t.border),
                    egui::StrokeKind::Inside,
                );
                app_icons::paint(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(field.left() + 19.0, field.center().y),
                        vec2(16.0, 16.0),
                    ),
                    Icon::Folder,
                    t.text_2,
                );
                let text = app_ui::middle_path(ui, &self.sheet.folder, field.width() - 48.0);
                ui.painter().text(
                    egui::pos2(field.left() + 36.0, field.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    FontId::proportional(style::text::BODY),
                    t.text,
                );
                let full = self.sheet.folder.display().to_string();
                response.on_hover_text(&full);
                #[cfg(test)]
                self.sheet.rects.push(("sheet-folder", field));
                let button = style::button(ui, change, true, false, button_width);
                #[cfg(test)]
                self.sheet.rects.push(("sheet-change", button.rect));
                pick = button.clicked();
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(l.text("ui.no_overwrite"))
                        .size(style::text::SMALL)
                        .color(t.text_2),
                )
                .wrap(),
            );
            if pick {
                self.pick_sheet_folder();
            }
        });
    }

    /// Details (closed by default): Mode, the output summary and counts from
    /// the current check, per-clip and technical detail, the processing log.
    fn sheet_details(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let header = egui::CollapsingHeader::new(style::strong(l.text("sheet.details"), 14.0))
            .id_salt("sheet-details")
            .icon(disclosure)
            .open(Some(self.sheet.details))
            .show_unindented(ui, |ui| {
                egui::Frame::NONE
                    .fill(t.surface_2)
                    .corner_radius(style::radius::L)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = space::S;
                        self.details_content(ui);
                    });
            });
        style::focus(ui, &header.header_response);
        if header.header_response.clicked() {
            self.sheet.details = !self.sheet.details;
        }
    }

    fn details_content(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let small = |text: String| egui::RichText::new(text).size(style::text::SMALL);
        self.mode_control(ui, "sheet-mode");
        let current = self.diagnosis_current();
        if current
            && let Some(diagnosis) = &self.diagnosis
            && let Some(plan) = &diagnosis.plan
        {
            // Container, size, frame rate and audio in the language's own
            // punctuation and units ("25 i/s" in French).
            let summary = l.format(
                "sheet.summary",
                &[
                    diagnosis.container.to_uppercase(),
                    plan.target.width.to_string(),
                    plan.target.height.to_string(),
                    frame_rate(plan.target.rate_num, plan.target.rate_den, l),
                    l.text("diagnosis.aac_320").into(),
                ],
            );
            ui.add(egui::Label::new(small(summary).color(t.text)).wrap());
            let copied = plan
                .clips
                .iter()
                .filter(|c| {
                    !self.project.apply_subtitles && c.treatment == noh::plan::Treatment::Copy
                })
                .count();
            let converted = plan.clips.len() - copied;
            let counts = match (copied, converted) {
                (0, n) => l.plural("sheet.converted", n),
                (n, 0) => l.plural("sheet.copied", n),
                (a, b) => l.format(
                    "sheet.list",
                    &[l.plural("sheet.copied", a), l.plural("sheet.converted", b)],
                ),
            };
            ui.add(egui::Label::new(small(counts).color(t.text)).wrap());
            if converted > 0 && !self.project.apply_subtitles {
                style::message(
                    ui,
                    Severity::Warning,
                    &l.format(
                        if converted == 1 {
                            "ui.conversion.one"
                        } else {
                            "ui.conversion.other"
                        },
                        &[converted.to_string()],
                    ),
                    false,
                );
            }
            let detail = egui::CollapsingHeader::new(
                l.format("ui.per_clip", &[plan.clips.len().to_string()]),
            )
            .id_salt("per-clip")
            .icon(disclosure)
            .show(ui, |ui| {
                for (i, clip) in plan.clips.iter().enumerate() {
                    ui.push_id(self.clips.get(i).map(|c| c.id).unwrap_or(i as u64), |ui| {
                        app_ui::diagnosis_clip_row(
                            ui,
                            i,
                            &clip.path,
                            !self.project.apply_subtitles
                                && clip.treatment == noh::plan::Treatment::Copy,
                            l,
                        );
                        for reason in &clip.reasons {
                            ui.add(
                                egui::Label::new(small(
                                    l.text(&noh::plan::reason_key(reason)).into(),
                                ))
                                .wrap(),
                            );
                        }
                    });
                }
            });
            style::focus(ui, &detail.header_response);
            let detail = egui::CollapsingHeader::new(l.text("ui.technical"))
                .id_salt("technical")
                .icon(disclosure)
                .show(ui, |ui| {
                    if diagnosis.notes.iter().any(|n| n == "diagnosis.partial") {
                        ui.add(
                            egui::Label::new(small(l.text("ui.endpoint_encoding").into())).wrap(),
                        );
                    }
                    if plan.clips.iter().any(|c| c.timestamp_normalization) {
                        ui.add(
                            egui::Label::new(small(l.text("diagnosis.timestamps").into())).wrap(),
                        );
                    }
                    for note in &diagnosis.notes {
                        ui.add(egui::Label::new(small(l.text(note).into())).wrap());
                    }
                });
            style::focus(ui, &detail.header_response);
        } else {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(t.accent));
                ui.label(small(l.text("bar.checking").into()).color(t.text_2));
            });
        }
        self.processing_log(ui);
    }

    /// Mode (Automatic | Convert everything) and its help; a project setting
    /// that applies at once and needs a new check. Shared by Details and Options.
    pub(super) fn mode_control(&mut self, ui: &mut egui::Ui, id: &str) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let small = |text: String| egui::RichText::new(text).size(style::text::SMALL);
        ui.horizontal(|ui| {
            ui.label(l.text("ui.mode"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let options = [l.text("ui.automatic"), l.text("ui.convert_all")];
                let selected = usize::from(!self.partial);
                // Right to left: lay the segments out in reading order anyway.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if let Some(index) = style::segmented(ui, id, &options, selected) {
                        let partial = index == 0;
                        if partial != self.partial {
                            // A project setting: it applies now and needs a new check.
                            self.partial = partial;
                            self.invalidated();
                        }
                    }
                });
            });
        });
        ui.add(
            egui::Label::new(
                small(
                    l.text(if self.project.apply_subtitles {
                        "project.caption_encoding"
                    } else if self.partial {
                        "sheet.mode_help"
                    } else {
                        "encode_hint"
                    })
                    .into(),
                )
                .color(t.text_2),
            )
            .wrap(),
        );
    }

    /// The processing log and warnings, 120 high, sticking to the bottom.
    pub(super) fn processing_log(&self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let log = egui::CollapsingHeader::new(l.text("ui.processing_log"))
            .id_salt("engine-details")
            .icon(disclosure)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("journal")
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.add(
                                egui::Label::new(egui::RichText::new(line).monospace().size(12.0))
                                    .wrap(),
                            );
                        }
                    });
                for warning in &self.warnings {
                    style::message(ui, Severity::Warning, &warning.render(l), false);
                }
            });
        style::focus(ui, &log.header_response);
    }
    /// Cancel (quiet) then Export (primary, disabled with its reason), in
    /// reading order at the right; halves of the width in compact.
    fn sheet_footer(&mut self, ui: &mut egui::Ui, compact: bool, close: &mut bool) -> bool {
        let l = self.locale.language;
        let reason = self.sheet_reason();
        let pad = 2.0 * ui.spacing().button_padding.x;
        let measure = |text: &str, font: FontId| {
            ui.painter()
                .layout_no_wrap(text.into(), font, egui::Color32::WHITE)
                .size()
                .x
                + pad
        };
        let (cancel_label, export_label) = (l.text("bar.cancel"), l.text("sheet.confirm"));
        let (cancel_width, export_width) = if compact {
            let half = (ui.available_width() - space::S) / 2.0;
            (half, half)
        } else {
            (
                measure(cancel_label, FontId::proportional(14.0)),
                measure(export_label, style::semibold(14.0)).max(112.0),
            )
        };
        let mut confirm = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            ui.add_space((ui.available_width() - cancel_width - export_width - space::S).max(0.0));
            let cancel = style::button_kind(
                ui,
                cancel_label,
                true,
                Kind::Quiet,
                cancel_width,
                style::CONTROL_H_LG,
            );
            *close |= cancel.clicked();
            let export = style::button_kind(
                ui,
                export_label,
                reason.is_none(),
                Kind::Primary,
                export_width,
                style::CONTROL_H_LG,
            );
            self.sheet.confirm_id = Some(export.id);
            if self.sheet.focus_confirm {
                if reason.is_none() {
                    export.request_focus();
                    self.sheet.focus_confirm = false;
                } else if reason
                    .as_ref()
                    .is_some_and(|m| m.key != "ui.destination_checking")
                {
                    self.sheet.focus_confirm = false;
                }
            }
            #[cfg(test)]
            {
                self.sheet.rects.push(("sheet-cancel", cancel.rect));
                self.sheet.rects.push(("sheet-confirm", export.rect));
            }
            confirm = export.clicked() && reason.is_none();
            if let Some(reason) = &reason {
                export.on_disabled_hover_text(reason.render(l));
            }
        });
        confirm
    }
}

/// Disclosure chevron (›, ⌄) instead of egui's triangle, as in the design.
pub(super) fn disclosure(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let icon = if openness < 0.5 {
        Icon::ChevronRight
    } else {
        Icon::ChevronDown
    };
    app_icons::paint(
        ui.painter(),
        Rect::from_center_size(response.rect.center(), vec2(16.0, 16.0)),
        icon,
        style::tokens(ui).text_2,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2};

    fn input(width: f32, height: f32, events: Vec<Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, height))),
            events,
            ..Default::default()
        }
    }
    /// One frame of the whole window; returns the painted texts with their rects.
    fn frame_at(
        ctx: &egui::Context,
        app: &mut NohApp,
        size: (f32, f32),
        events: Vec<Event>,
    ) -> Vec<(String, Rect)> {
        let mut output = ctx.run_ui(input(size.0, size.1, events), |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();
        output
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
    fn frame(ctx: &egui::Context, app: &mut NohApp, events: Vec<Event>) -> Vec<(String, Rect)> {
        frame_at(ctx, app, (980.0, 850.0), events)
    }
    fn click(ctx: &egui::Context, app: &mut NohApp, at: Pos2) {
        frame(ctx, app, vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                vec![Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
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
    fn find(texts: &[(String, Rect)], text: &str) -> Option<Rect> {
        texts.iter().rev().find(|(t, _)| t == text).map(|(_, r)| *r)
    }
    fn rect(app: &NohApp, name: &str) -> Rect {
        app.sheet
            .rects
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("{name} not drawn"))
    }
    /// A settled "ready" project in a real folder: the check is current and no
    /// worker process can start (the worker path does not exist).
    fn ready(folder: &Path, language: Language) -> (egui::Context, NohApp) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = NohApp::default();
        app.locale.language = language;
        app.worker = folder.join("no-worker.exe");
        app.clips = ["a.mp4", "b.mp4"]
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
                    item: folder.join(name).into(),
                    info: Some(info),
                    error: None,
                }
            })
            .collect();
        app.wav = Some(folder.join("song.wav"));
        app.wav_seconds = Some(30.0);
        app.project.duration_ms = 30_000;
        app.output_folder = folder.into();
        app.output_base = "song_noh.mp4".into();
        app.output_name = app.output_base.clone();
        app.output = Some(folder.join(&app.output_name));
        let request = app.diagnostic_request().unwrap();
        app.preflight = Some(app_diagnosis::Preflight::dormant(Some(request.clone())));
        app.diagnosis = Some(Box::new(noh::inspection::Diagnosis {
            version: 1,
            level: noh::inspection::Level::Exact,
            request,
            snapshot: noh::inspection::Snapshot(vec![]),
            duration: 30.0,
            container: "mp4".into(),
            audio: "aac_320".into(),
            quick_media: vec![],
            notes: vec![],
            plan: None,
        }));
        app.diagnosis_revision = Some(app.revision);
        (ctx, app)
    }
    /// Frames until `done`, for the background name checks (a few ms each).
    fn until(ctx: &egui::Context, app: &mut NohApp, done: impl Fn(&NohApp) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            frame(ctx, app, vec![]);
            if done(app) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "never settled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A taken automatic name gives way to the first free one; the project
    /// revision and the check stay as they were.
    #[test]
    fn an_automatic_name_adopts_the_free_name_without_a_revision_change() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("song_noh.mp4"), b"earlier").unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        let revision = app.revision;
        until(&ctx, &mut app, |app| app.status().export.enabled);
        assert_eq!(app.output_name, "song_noh-2.mp4");
        assert_eq!(app.revision, revision);
        assert!(app.diagnosis_current());
        assert_eq!(app.status().headline.key, "bar.ready");
        assert_eq!(
            std::fs::read(folder.path().join("song_noh.mp4")).unwrap(),
            b"earlier"
        );
    }

    /// A name the person chose that is taken: Export opens the
    /// sheet with the conflict instead of exporting.
    #[test]
    fn a_chosen_taken_name_opens_the_sheet_and_never_exports() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("mine.mp4"), b"keep").unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        app.output_auto = false;
        app.output_name = "mine.mp4".into();
        app.destination_edited();
        until(&ctx, &mut app, |app| {
            app.status().headline.key == "bar.name_taken"
        });
        assert_eq!(app.output_name, "mine.mp4", "never renamed silently");
        let texts = frame(&ctx, &mut app, vec![]);
        find(&texts, "Use mine-2.mp4").expect("contextual suggestion");
        click(
            &ctx,
            &mut app,
            find(&texts, "Export the video").unwrap().center(),
        );
        assert!(app.sheet.open && app.job.is_none() && !app.project.busy());
        until(&ctx, &mut app, |app| app.sheet_checked());
        let texts = frame(&ctx, &mut app, vec![]);
        find(&texts, "This name is already taken in this folder.").expect("conflict");
        assert_eq!(
            app.sheet_reason().map(|m| m.key),
            Some("sheet.exists".into())
        );
        // Its suggestion fills the name; nothing is exported until confirmed.
        click(
            &ctx,
            &mut app,
            find(&texts, "Use mine-2.mp4").unwrap().center(),
        );
        assert_eq!(app.sheet.name, "mine-2.mp4");
        assert!(app.job.is_none());
        assert_eq!(
            std::fs::read(folder.path().join("mine.mp4")).unwrap(),
            b"keep"
        );
    }

    /// Enter in the name commits it and moves to Export without exporting;
    /// Ctrl+Enter exports. Escape discards the edits.
    #[test]
    fn enter_moves_to_export_ctrl_enter_exports_and_escape_discards() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        until(&ctx, &mut app, |app| app.status().export.enabled);
        app.open_sheet(false, None);
        until(&ctx, &mut app, |app| {
            ctx.memory(|m| m.has_focus(egui::Id::new(NAME))) && app.sheet_checked()
        });
        // Opening focuses the name with its text selected: typing replaces it.
        frame(&ctx, &mut app, vec![Event::Text("clip.mp4".into())]);
        assert_eq!(app.sheet.name, "clip.mp4");
        frame(&ctx, &mut app, vec![key(egui::Key::Enter, Modifiers::NONE)]);
        // Enter may beat the background check. Focus transfers when enabled.
        until(&ctx, &mut app, |app| {
            app.sheet
                .confirm_id
                .is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
        });
        assert!(app.sheet.open, "Enter never exports");
        assert!(app.job.is_none() && app.output_name == "song_noh.mp4");
        let confirm = app.sheet.confirm_id.expect("Export drawn");
        assert!(ctx.memory(|m| m.has_focus(confirm)), "focus on Export");
        // Escape discards the typed name.
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Escape, Modifiers::NONE)],
        );
        assert!(!app.sheet.open);
        assert_eq!(app.output_name, "song_noh.mp4");
        assert!(app.output_auto);
        // Again, then Ctrl+Enter applies the name and starts the export.
        app.open_sheet(false, None);
        until(&ctx, &mut app, |app| {
            ctx.memory(|m| m.has_focus(egui::Id::new(NAME))) && app.sheet_checked()
        });
        frame(&ctx, &mut app, vec![Event::Text("clip.mp4".into())]);
        until(&ctx, &mut app, |app| app.sheet_checked());
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Enter, Modifiers::COMMAND)],
        );
        assert!(!app.sheet.open);
        assert_eq!(app.output_name, "clip.mp4");
        assert!(!app.output_auto, "a typed name belongs to the person");
        assert!(app.job.is_some(), "Ctrl+Enter exports");
    }

    #[test]
    fn export_focus_waits_for_a_pending_check_without_exporting() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        until(&ctx, &mut app, |app| app.status().export.enabled);
        app.open_sheet(false, None);
        app.sheet.focus_confirm = true;
        // Draw only the footer to hold the result pending deterministically.
        let footer = |app: &mut NohApp| {
            let mut output = ctx.run_ui(input(980.0, 850.0, vec![]), |ui| {
                let mut close = false;
                assert!(!app.sheet_footer(ui, false, &mut close));
                assert!(!close);
            });
            // Headless frames have no renderer to consume their texture deltas.
            output.textures_delta.clear();
        };
        for _ in 0..3 {
            footer(&mut app);
            assert!(app.sheet.focus_confirm, "pending check retains the request");
            assert!(app.job.is_none());
        }
        app.sheet.name_result = Some(ResultView {
            path: folder.path().join(&app.sheet.name),
            issue: None,
            suggestion: None,
            bytes: None,
        });
        footer(&mut app);
        let confirm = app.sheet.confirm_id.unwrap();
        assert!(ctx.memory(|m| m.has_focus(confirm)));
        assert!(!app.sheet.focus_confirm && app.job.is_none());
    }

    #[test]
    fn newer_typing_cancels_a_pending_export_focus_request() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        until(&ctx, &mut app, |app| app.status().export.enabled);
        app.open_sheet(false, None);
        until(&ctx, &mut app, |app| {
            ctx.memory(|m| m.has_focus(egui::Id::new(NAME))) && app.sheet_checked()
        });
        app.sheet.focus_confirm = true;
        frame(&ctx, &mut app, vec![Event::Text("new-name.mp4".into())]);
        until(&ctx, &mut app, |app| app.sheet_checked());
        assert!(!app.sheet.focus_confirm && app.job.is_none());
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(NAME))));
    }

    /// Mode in Details applies at once and needs a new check; the sheet's
    /// Export then waits for it.
    #[test]
    fn mode_in_details_invalidates_the_check() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        app.open_sheet(false, None);
        app.sheet.details = true;
        until(&ctx, &mut app, |app| app.sheet_checked());
        let revision = app.revision;
        let texts = frame(&ctx, &mut app, vec![]);
        click(
            &ctx,
            &mut app,
            find(&texts, "Convert everything").unwrap().center(),
        );
        assert!(!app.partial);
        assert_eq!(app.revision, revision.wrapping_add(1));
        assert!(!app.diagnosis_current());
        assert!(app.sheet.open, "Mode keeps the sheet open");
        assert_eq!(
            app.sheet_reason().map(|m| m.key),
            Some("ui.analysis_required".into())
        );
        // Cancel keeps the Mode (a project setting), not the name edits.
        let texts = frame(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Cancel").unwrap().center());
        assert!(!app.sheet.open && !app.partial);
    }

    /// "Export the short" in the bar starts the short's own export.
    #[test]
    fn the_bar_exports_the_short() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = ready(folder.path(), Language::En);
        app.project.range = noh::timeline::Range::new(10_000, 15_000, 30_000);
        until(&ctx, &mut app, |app| {
            app.status()
                .montage
                .and_then(|m| m.short)
                .is_some_and(|s| s.enabled)
        });
        let texts = frame(&ctx, &mut app, vec![]);
        click(
            &ctx,
            &mut app,
            find(&texts, "Export the short").unwrap().center(),
        );
        assert!(
            app.project
                .running
                .as_ref()
                .is_some_and(|r| r.short && !r.preview),
            "the short export starts"
        );
    }

    /// The sheet and its Details fit 420 × 540 and 980 × 850 in seven
    /// languages, with the short's name and a conflict shown.
    #[test]
    fn sheet_fits_in_every_language() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("song_noh.mp4"), []).unwrap();
        for language in Language::ALL {
            for size in [(420.0, 540.0), (980.0, 850.0)] {
                let (ctx, mut app) = ready(folder.path(), language);
                app.project.range = noh::timeline::Range::new(10_000, 15_000, 30_000);
                app.output_auto = false;
                app.destination_edited();
                app.open_sheet(false, None);
                app.sheet.details = false;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !app.sheet_checked() {
                    frame_at(&ctx, &mut app, size, vec![]);
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                // Details opens after the sheet is shown: the body must grow.
                app.sheet.details = true;
                frame_at(&ctx, &mut app, size, vec![]);
                frame_at(&ctx, &mut app, size, vec![]);
                let modal = ctx
                    .memory(|m| m.area_rect(egui::Id::new("location-sheet")))
                    .expect("sheet drawn");
                let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
                let margin = if size.0 < 560.0 { 16.0 } else { 24.0 };
                for name in [
                    NAME,
                    SHORT_NAME,
                    "sheet-folder",
                    "sheet-change",
                    "sheet-cancel",
                    "sheet-confirm",
                ] {
                    let r = rect(&app, name);
                    assert!(
                        r.left() >= margin - 0.5
                            && r.right() <= size.0 - margin + 0.5
                            && window.contains_rect(r)
                            && modal.contains_rect(r),
                        "{} {size:?} {name}: {r:?}",
                        language.code()
                    );
                }
                let cancel = rect(&app, "sheet-cancel");
                let confirm = rect(&app, "sheet-confirm");
                assert!(cancel.right() <= confirm.left(), "{}", language.code());
            }
        }
    }

    /// The desktop app built beside this test, which runs checks and exports.
    /// The quick suite has no FFmpeg; `cargo dev verify --media` sets
    /// NOH_MEDIA_TESTS so this can never be skipped silently there.
    fn media_worker() -> Option<PathBuf> {
        let worker = std::env::current_exe()
            .ok()
            .and_then(|exe| {
                Some(
                    exe.parent()?
                        .parent()?
                        .join(format!("noh-app{}", std::env::consts::EXE_SUFFIX)),
                )
            })
            .filter(|path| path.exists());
        let ffmpeg = noh::inspection::resolve_ffmpeg(Path::new(""));
        if worker.is_none() || ffmpeg.is_err() {
            assert!(
                std::env::var_os("NOH_MEDIA_TESTS").is_none(),
                "This media test needs FFmpeg and the built noh-app: {worker:?} {:?}",
                ffmpeg.err()
            );
            eprintln!("SKIPPED: no FFmpeg or noh-app; `cargo dev verify --media` runs this test");
            return None;
        }
        worker
    }
    fn wait(ctx: &egui::Context, app: &mut NohApp, seconds: u64, done: impl Fn(&NohApp) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        loop {
            frame(ctx, app, vec![]);
            if done(app) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out: {:?} / {:?}",
                app.status().headline,
                app.status().detail
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Drop a picture and a song, then one click on "Export the
    /// video" writes `<song>_noh.mp4` beside the song, through the app's own
    /// worker. Then a file that appears after the check is never replaced:
    /// the export fails, says so, and the sheet opens with the conflict.
    #[test]
    fn one_click_exports_dropped_files_and_never_replaces_a_late_file() {
        let Some(worker) = media_worker() else {
            return;
        };
        let folder = tempfile::tempdir().unwrap();
        let picture = folder.path().join("picture.png");
        image::RgbaImage::from_pixel(320, 180, image::Rgba([40, 120, 200, 255]))
            .save(&picture)
            .unwrap();
        // Six seconds of 48 kHz stereo silence (longer than the default fades).
        let song = folder.path().join("song.wav");
        let data_bytes = 6_u32 * 48_000 * 4;
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&192_000_u32.to_le_bytes());
        wav.extend_from_slice(b"\x04\0\x10\0data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        wav.resize(wav.len() + data_bytes as usize, 0);
        std::fs::write(&song, wav).unwrap();
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        app.worker = worker.clone();
        app.metadata = app_media::Metadata::with_worker(worker);
        app.drop_files(vec![picture, song], &ctx);
        wait(&ctx, &mut app, 120, |app| app.status().export.enabled);
        let texts = frame(&ctx, &mut app, vec![]);
        click(
            &ctx,
            &mut app,
            find(&texts, "Export the video").unwrap().center(),
        );
        assert!(app.job.is_some(), "one click starts the export");
        wait(&ctx, &mut app, 180, |app| app.job.is_none());
        let output = folder.path().join("song_noh.mp4");
        assert!(
            std::fs::metadata(&output).is_ok_and(|m| m.len() > 0),
            "{:?}",
            app.status().detail
        );
        assert_eq!(app.status().headline.key, "bar.done_video");
        // The race: a chosen name, checked free, taken while exporting.
        app.output_auto = false;
        app.output_name = "late.mp4".into();
        app.destination_edited();
        wait(&ctx, &mut app, 30, |app| {
            app.status().export.enabled
                && app
                    .destination_result
                    .as_ref()
                    .is_some_and(|r| r.path.ends_with("late.mp4") && r.issue.is_none())
        });
        let texts = frame(&ctx, &mut app, vec![]);
        click(
            &ctx,
            &mut app,
            find(&texts, "Export the video").unwrap().center(),
        );
        assert!(app.job.is_some());
        let late = folder.path().join("late.mp4");
        std::fs::write(&late, b"keep").unwrap();
        wait(&ctx, &mut app, 180, |app| app.job.is_none());
        let view = app.status();
        assert_eq!(view.headline.key, "ui.failed");
        assert_eq!(view.detail.key, "error.output_exists");
        assert!(
            view.actions
                .contains(&app_state::ContextAction::ChooseFolder)
        );
        assert!(app.sheet.open, "the sheet opens with the conflict");
        wait(&ctx, &mut app, 10, |app| app.sheet_checked());
        assert_eq!(
            app.sheet_reason().map(|m| m.key),
            Some("sheet.exists".into())
        );
        assert_eq!(std::fs::read(&late).unwrap(), b"keep", "never replaced");
    }
}
