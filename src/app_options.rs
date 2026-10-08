//! Options, a modal: language, Export (Mode, processing log),
//! Lyrics (advanced settings), video engine and build information. Fades and
//! clip sound stay on the main screen.
use super::*;
use app_icons::Icon;
use app_style::{self as style, Kind, space};
use egui::{Align, Align2, Layout, Stroke, vec2};

const WIDTH: f32 = 460.0;

#[derive(Default)]
pub struct State {
    /// When Options appeared (input time), for captures after its fade-in.
    opened_at: Option<f64>,
    /// The header's Options button, where focus returns on close.
    pub button: Option<egui::Id>,
    /// Controls of the last frame with their enabled state, for tests.
    #[cfg(test)]
    pub rects: Vec<(&'static str, egui::Rect, bool)>,
}

impl NohApp {
    /// FFmpeg and Mode stay as they are while anything runs.
    pub(super) fn engine_locked(&self) -> bool {
        self.job.is_some()
            || self.project.busy()
            || self.cancelling
            || self.subtitles.running
            || self.captions.running.is_some()
            || self.shorts.running.is_some()
    }

    /// Seconds since Options appeared, 0 while it is closed.
    pub(super) fn options_shown_for(&self, ctx: &egui::Context) -> f64 {
        self.options
            .opened_at
            .map_or(0.0, |at| ctx.input(|i| i.time) - at)
    }

    fn close_options(&mut self, ctx: &egui::Context) {
        self.settings_open = false;
        if let Some(id) = self.options.button {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    /// Centred 460 wide at top 56, or a sheet from under the header to the
    /// bottom in compact. Escape, ✕ and a click outside close it.
    pub(super) fn options_modal(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            self.options.opened_at = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        self.options.opened_at.get_or_insert(now);
        #[cfg(test)]
        self.options.rects.clear();
        let t = style::tokens_for(ctx.global_style().visuals.dark_mode);
        let content = ctx.content_rect();
        let compact = content.width() - 32.0 < style::COMPACT_BELOW;
        let id = egui::Id::new("options");
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
            (
                egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 56.0)),
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
        // Compact fills the window under the header.
        let height = content.height() - style::HEADER_H - 8.0 - 36.0;
        let mut close = false;
        let response = egui::Modal::new(id)
            .area(area)
            .backdrop_color(t.scrim)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(width);
                if compact {
                    ui.set_min_height(height);
                }
                ui.spacing_mut().item_spacing.y = space::M;
                let l = self.locale.language;
                ui.horizontal(|ui| {
                    ui.label(style::strong(l.text("settings"), style::text::HEADING));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let button = style::icon_button(
                            ui,
                            Icon::Close,
                            l.text("sheet.close"),
                            style::ROW_ICON,
                        );
                        #[cfg(test)]
                        self.options.rects.push(("close", button.rect, true));
                        close |= button.clicked();
                    });
                });
                // The area reuses last frame's size as its limit: give the body
                // its full height so it can grow when a section opens.
                ui.allocate_ui(vec2(width, height - 40.0), |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("options-body")
                        .max_height(height - 40.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = space::M;
                            self.options_body(ui, ctx);
                        })
                });
            });
        if response.should_close() || close {
            self.close_options(ctx);
        }
    }

    fn options_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let locked = self.engine_locked();
        let heading = |ui: &mut egui::Ui, key: &str| {
            ui.add_space(space::XS);
            ui.label(style::strong(l.text(key), style::text::BODY));
        };
        // 1. Language, usable at any time.
        ui.horizontal(|ui| {
            ui.label(l.text("language"));
            let row = vec2(ui.available_width(), style::CONTROL_H);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                self.language_selector(ui, ctx);
                #[cfg(test)]
                self.options.rects.push(("language", ui.min_rect(), true));
            });
        });
        egui::CollapsingHeader::new(l.text("resources.title"))
            .id_salt("options-resources")
            .default_open(self.resources.unavailable())
            .show(ui, |ui| self.resource_settings(ui));
        // 2. Export: Mode (locked while exporting) and the processing log.
        heading(ui, "options.export");
        ui.add_enabled_ui(!locked, |ui| self.mode_control(ui, "options-mode"));
        self.processing_log(ui);
        // 3. Lyrics: the transcription setup.
        heading(ui, "lyrics.title");
        let lyrics = egui::CollapsingHeader::new(l.text("options.lyrics_advanced"))
            .id_salt("options-lyrics")
            .icon(app_sheet::disclosure)
            .show(ui, |ui| self.subtitles.settings(ui, ctx, l, locked));
        style::focus(ui, &lyrics.header_response);
        // 4. Video engine: found automatically, or the chosen FFmpeg.
        heading(ui, "options.engine");
        let mut pick = false;
        ui.horizontal(|ui| {
            let change = l.text("bar.change");
            let button_width = ui
                .painter()
                .layout_no_wrap(change.into(), egui::FontId::proportional(14.0), t.text)
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x;
            let text_width = ui.available_width() - button_width - space::S;
            let text = match &self.ffmpeg {
                Some(path) => app_ui::middle_path(ui, path, text_width),
                None => l.text("options.engine_auto").into(),
            };
            let label = ui
                .allocate_ui(vec2(text_width, style::CONTROL_H), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).color(t.text_2)).wrap())
                })
                .inner;
            if let Some(path) = &self.ffmpeg {
                label.on_hover_text(path.display().to_string());
            }
            let row = vec2(ui.available_width(), style::CONTROL_H);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                let button =
                    style::button_kind(ui, change, !locked, Kind::Secondary, 0.0, style::CONTROL_H)
                        .on_disabled_hover_text(l.text("ui.locked"));
                #[cfg(test)]
                self.options
                    .rects
                    .push(("engine", button.rect, button.enabled()));
                pick = button.clicked();
            });
        });
        if pick
            && !locked
            && let Some(path) = rfd::FileDialog::new()
                .set_title(l.text("choose_ffmpeg"))
                .pick_file()
        {
            self.set_ffmpeg(path, ctx);
        }
        // 5. Updates use their own worker and do not lock media operations.
        let updates = ui.scope(|ui| {
            heading(ui, "updates.title");
            #[cfg(feature = "updates")]
            self.update_controls(ui, ctx);
            #[cfg(not(feature = "updates"))]
            ui.label(l.text("updates.unconfigured"));
        });
        #[cfg(feature = "updates")]
        if self.updates.capture_fixture {
            // The normal Settings body scrolls. Layout captures explicitly bring
            // the new section into view, including in a 420 x 540 sheet.
            ui.scroll_to_rect(updates.response.rect, Some(egui::Align::Center));
        }
        #[cfg(not(feature = "updates"))]
        let _ = updates;
        // 6. Build information.
        let build = egui::CollapsingHeader::new(l.text("ui.build_information"))
            .id_salt("options-build")
            .icon(app_sheet::disclosure)
            .show(ui, |ui| self.build_information(ui, ctx));
        style::focus(ui, &build.header_response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, Rect};

    fn frame(
        ctx: &egui::Context,
        app: &mut NohApp,
        size: (f32, f32),
        events: Vec<Event>,
    ) -> Vec<(String, Rect)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1))),
                events,
                ..Default::default()
            },
            |ui| {
                app.draw(ui);
            },
        );
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
    fn click(ctx: &egui::Context, app: &mut NohApp, size: (f32, f32), at: Pos2) {
        frame(ctx, app, size, vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                size,
                vec![Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
            );
        }
    }
    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        texts
            .iter()
            .rev()
            .find(|(t, _)| t == text)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| panic!("{text} not painted"))
    }
    fn rect(app: &NohApp, name: &str) -> (Rect, bool) {
        app.options
            .rects
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, r, enabled)| (*r, *enabled))
            .unwrap_or_else(|| panic!("{name} not drawn"))
    }
    /// A project with a current check; the language file goes to a temporary
    /// folder, never to the user's settings.
    fn app(folder: &Path, language: Language) -> (egui::Context, NohApp) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut app = NohApp::default();
        app.locale.language = language;
        app.locale.choice = noh::i18n::Choice(Some(language));
        app.locale.path = Some(folder.join("language"));
        app.worker = folder.join("no-worker.exe");
        let mut info = noh::media::MediaInfo::default();
        info.seconds = 5.0;
        app.clips.push(Clip {
            id: 1,
            request_id: 1,
            item: folder.join("a.mp4").into(),
            info: Some(info),
            error: None,
        });
        app.wav = Some(folder.join("song.wav"));
        app.wav_seconds = Some(30.0);
        app.project.duration_ms = 30_000;
        app.output_folder = folder.into();
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
        app.settings_open = true;
        (ctx, app)
    }

    /// Picking another language re-renders Options and the screen in it; no
    /// revision changes and the check stays current.
    #[test]
    fn language_switch_changes_no_revision() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(folder.path(), Language::Fr);
        let size = (980.0, 850.0);
        frame(&ctx, &mut app, size, vec![]);
        let texts = frame(&ctx, &mut app, size, vec![]);
        let (revision, short_revision) = (app.revision, app.project.short_revision);
        click(&ctx, &mut app, size, find(&texts, "Français").center());
        let texts = frame(&ctx, &mut app, size, vec![]);
        click(&ctx, &mut app, size, find(&texts, "English").center());
        assert_eq!(app.locale.language, Language::En);
        assert_eq!(app.revision, revision);
        assert_eq!(app.project.short_revision, short_revision);
        assert!(app.diagnosis_current());
        let texts = frame(&ctx, &mut app, size, vec![]);
        find(&texts, "Settings");
        find(&texts, "Export the video");
        assert!(app.settings_open, "choosing a language keeps Options open");
    }

    /// FFmpeg (and Mode) cannot change while an export runs; the language can.
    #[test]
    fn engine_is_locked_during_an_operation() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(folder.path(), Language::En);
        let size = (980.0, 850.0);
        frame(&ctx, &mut app, size, vec![]);
        frame(&ctx, &mut app, size, vec![]);
        assert!(rect(&app, "engine").1, "free while idle");
        app.project.running = Some(app_project::Running {
            preview: false,
            short: false,
            stamp: app.project.stamp(app.revision, false),
            preview_file: None,
        });
        frame(&ctx, &mut app, size, vec![]);
        let (engine, enabled) = rect(&app, "engine");
        assert!(!enabled, "locked during the export");
        click(&ctx, &mut app, size, engine.center());
        assert!(app.ffmpeg.is_none());
        let texts = frame(&ctx, &mut app, size, vec![]);
        let before = app.partial;
        click(
            &ctx,
            &mut app,
            size,
            find(&texts, "Convert everything").center(),
        );
        assert_eq!(app.partial, before, "Mode is locked too");
        assert!(rect(&app, "language").1, "the language stays usable");
    }

    /// Escape, ✕ and a click on the backdrop close Options.
    #[test]
    fn escape_close_button_and_backdrop_close() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(folder.path(), Language::En);
        let size = (980.0, 850.0);
        frame(&ctx, &mut app, size, vec![]);
        frame(
            &ctx,
            &mut app,
            size,
            vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(!app.settings_open, "Escape");
        app.settings_open = true;
        frame(&ctx, &mut app, size, vec![]);
        click(&ctx, &mut app, size, Pos2::new(40.0, 700.0));
        assert!(!app.settings_open, "backdrop");
        app.settings_open = true;
        frame(&ctx, &mut app, size, vec![]);
        frame(&ctx, &mut app, size, vec![]);
        let close = rect(&app, "close").0;
        click(&ctx, &mut app, size, close.center());
        assert!(!app.settings_open, "✕");
    }

    /// Every control of Options fits 420 × 540 and 980 × 850 in seven languages.
    #[test]
    fn options_fit_in_every_language() {
        let folder = tempfile::tempdir().unwrap();
        for language in Language::ALL {
            for size in [(420.0, 540.0), (980.0, 850.0)] {
                let (ctx, mut app) = app(folder.path(), language);
                app.ffmpeg = Some(folder.path().join("a very long folder name/ffmpeg.exe"));
                frame(&ctx, &mut app, size, vec![]);
                frame(&ctx, &mut app, size, vec![]);
                let margin = if size.0 < 560.0 { 16.0 } else { 24.0 };
                for _ in 0..6 {
                    frame(&ctx, &mut app, size, vec![]);
                }
                let modal = ctx
                    .memory(|m| m.area_rect(egui::Id::new("options")))
                    .expect("Options drawn");
                for name in ["language", "engine"] {
                    let (r, _) = rect(&app, name);
                    assert!(
                        r.left() >= margin - 0.5
                            && r.right() <= size.0 - margin + 0.5
                            && r.bottom() <= size.1
                            && modal.contains_rect(r),
                        "{} {size:?} {name}: {r:?} modal {modal:?}",
                        language.code()
                    );
                }
            }
        }
    }
}
