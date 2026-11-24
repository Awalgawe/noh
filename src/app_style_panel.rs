//! The caption style panel beside the monitor (fused mockup v9, Task 41 S1): the
//! switch, one card per effect with a sample drawn by the real engine, a style
//! tried on the monitor before it is applied, and the applied effect's settings.
//! The exports always use the applied style (`project.effect`).
use crate::app_effects::Choice;
use crate::app_style::{self as style, space};
use eframe::egui::{self, Color32, Rect, Sense, Vec2, pos2, vec2};
use noh::fx::{gl::GlRenderer, text::Fonts};
use noh::style::{
    line::split_words,
    plugin::{Effect, Param, PluginScene, localized},
};
use serde_json::Value as J;
use std::collections::{BTreeMap, HashMap};

/// The side panel's minimum width.
pub const PANEL_MIN: f32 = 300.0;
const GAP: f32 = 16.0;
const CARD_MIN: f32 = 96.0;
const CARD_H: f32 = 64.0;
const TILE_H: f32 = 40.0;
const SAMPLE_W: u32 = 1280;
const SAMPLE_H: u32 = 720;

/// The stage row: monitor column on the left, panel on the right, both
/// `height` tall; the transport stays under them across the whole width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub height: f32,
    pub column: f32,
    pub panel: f32,
}

/// `full`: content width; `fixed`: the zones above and below the monitor for
/// the transport mode already chosen on the full width (530 or 570, as before).
/// The column is h·16/9 wide (wider for wider pictures), also in the Short, so
/// the panel does not jump between Vidéo and Extrait. None: no room for a
/// 300 px panel beside a 180 px monitor; the panel goes under the transport.
pub fn split(full: f32, viewport_h: f32, aspect: f32, fixed: f32) -> Option<Split> {
    let wide = aspect.max(16.0 / 9.0);
    let room = (full - GAP - PANEL_MIN) / wide;
    if room < 180.0 {
        return None;
    }
    let height = (viewport_h - fixed).clamp(180.0, 360.0).min(room);
    let column = height * wide;
    Some(Split {
        height,
        column,
        panel: full - column - GAP,
    })
}

#[derive(Default)]
pub struct State {
    /// A style shown on the monitor, not applied: the exports keep `project.effect`.
    pub trying: Option<Choice>,
    /// The panel shows the applied style's settings instead of the cards.
    pub settings: bool,
    /// Level-2 settings are open.
    pub more: bool,
    /// Open the font picker's list on the next frame (capture and tests).
    pub font_open: bool,
    /// Scroll the settings to the font picker (capture: it may sit below the fold).
    pub font_reveal: bool,
    /// The picker's names drawn in their own faces.
    pub font_rows: super::app_font_rows::Rows,
    /// Compact: the settings sheet is open.
    pub sheet: bool,
    /// The tried style could not be drawn: the monitor shows the applied one,
    /// and the applied effect's own state is untouched.
    pub try_failed: Option<String>,
    samples: Samples,
}

/// Card samples drawn once each by the real engine, offscreen, on egui's own
/// OpenGL context (a second context needs a second event loop, which winit
/// refuses inside the app), a few per frame inside a paint callback.
#[derive(Default)]
struct Samples {
    gl: std::sync::Arc<std::sync::Mutex<SampleGl>>,
    /// Per effect id: the key of the last sample drawn and its texture (None: failed).
    done: HashMap<String, (String, Option<egui::TextureHandle>)>,
    /// Per effect id: the key last asked for.
    asked: HashMap<String, String>,
}

#[derive(Default)]
struct SampleGl {
    gpu: Option<GlRenderer>,
    fonts: Option<Fonts>,
    queue: std::collections::VecDeque<SampleJob>,
    results: Vec<(String, String, Result<egui::ColorImage, String>)>,
}

struct SampleJob {
    id: String,
    key: String,
    effect: Effect,
    preset: String,
    values: BTreeMap<String, J>,
}

fn sample_key(c: &Choice) -> String {
    format!(
        "{}|{}|{}",
        c.id,
        c.preset,
        serde_json::to_string(&c.values).unwrap_or_default()
    )
}

impl Samples {
    fn texture(
        &mut self,
        ctx: &egui::Context,
        choice: &Choice,
        effect: Option<&Effect>,
    ) -> Option<egui::TextureHandle> {
        let mut gl = self.gl.lock().unwrap();
        for (id, key, result) in gl.results.drain(..) {
            // One texture per effect: a newer sample replaces the older one.
            if self.asked.get(&id) == Some(&key) {
                let tex = result.ok().map(|img| {
                    ctx.load_texture(
                        format!("style-sample-{id}"),
                        img,
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.done.insert(id, (key, tex));
            }
        }
        let key = sample_key(choice);
        let fresh = self.done.get(&choice.id).is_some_and(|(k, _)| *k == key);
        if !fresh
            && let Some(effect) = effect
            && self.asked.get(&choice.id) != Some(&key)
        {
            self.asked.insert(choice.id.clone(), key.clone());
            // Only the latest request per effect is worth drawing.
            gl.queue.retain(|j| j.id != choice.id);
            gl.queue.push_back(SampleJob {
                id: choice.id.clone(),
                key,
                effect: effect.clone(),
                preset: choice.preset.clone(),
                values: choice.values.clone(),
            });
        }
        // The previous sample stays shown while the new one is drawn.
        self.done.get(&choice.id).and_then(|(_, t)| t.clone())
    }

    /// Render queued samples (two per frame) during this frame's painting.
    fn paint(&self, ui: &egui::Ui, at: Rect) {
        if self.gl.lock().unwrap().queue.is_empty() {
            return;
        }
        ui.ctx().request_repaint();
        let gl = self.gl.clone();
        ui.painter().add(egui::PaintCallback {
            rect: at,
            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                move |info, painter| {
                    let mut state = gl.lock().unwrap();
                    for _ in 0..2 {
                        let Some(job) = state.queue.pop_front() else {
                            break;
                        };
                        let result = state.render(painter, &job);
                        state.results.push((job.id, job.key, result));
                    }
                    // Give egui its framebuffer back, as the monitor layer does.
                    let viewport = info.viewport_in_pixels();
                    let clip = info.clip_rect_in_pixels();
                    unsafe {
                        use eframe::glow::HasContext;
                        let g = painter.gl();
                        g.bind_framebuffer(eframe::glow::FRAMEBUFFER, painter.intermediate_fbo());
                        g.viewport(
                            viewport.left_px,
                            viewport.from_bottom_px,
                            viewport.width_px,
                            viewport.height_px,
                        );
                        g.enable(eframe::glow::SCISSOR_TEST);
                        g.scissor(
                            clip.left_px,
                            clip.from_bottom_px,
                            clip.width_px,
                            clip.height_px,
                        );
                    }
                },
            )),
        });
    }
}

impl SampleGl {
    fn render(
        &mut self,
        painter: &eframe::egui_glow::Painter,
        job: &SampleJob,
    ) -> Result<egui::ColorImage, String> {
        if self.gpu.is_none() {
            self.gpu = Some(GlRenderer::on_egui(
                painter.gl().clone(),
                SAMPLE_W,
                SAMPLE_H,
                SAMPLE_W as f32,
                SAMPLE_H as f32,
            )?);
        }
        if self.fonts.is_none() {
            self.fonts = Some(Fonts::load()?);
        }
        render_sample(
            self.gpu.as_mut().unwrap(),
            self.fonts.as_mut().unwrap(),
            job,
        )
    }
}

/// "La la la" mid-line (second word sung), cropped to the ink with a margin.
fn render_sample(
    gpu: &mut GlRenderer,
    fonts: &mut Fonts,
    job: &SampleJob,
) -> Result<egui::ColorImage, String> {
    let words: Vec<String> = ["La", "la", "la"].map(String::from).to_vec();
    let line = split_words(0.2, 2.2, &words, " ");
    let mut scene = PluginScene::new(
        job.effect.clone(),
        SAMPLE_W,
        SAMPLE_H,
        vec![line],
        Some(&job.preset),
    )
    .with_values(&job.values)?;
    gpu.set_hooks(job.effect.shader.as_deref(), scene.hook_user())?;
    let (frame, _) = scene.describe(fonts, &*gpu, 1.3);
    let noh::fx::paint::Drawn::Direct(px, _) = noh::fx::paint::draw(gpu, &frame, 1.3, true, None)?
    else {
        return Err("the sample was not drawn by the engine's direct path".into());
    };
    crop(&px, SAMPLE_W as usize, SAMPLE_H as usize).ok_or_else(|| "nothing drawn".into())
}

/// The smallest box around pixels with alpha, plus 16 px, as an egui image.
fn crop(px: &[u8], w: usize, h: usize) -> Option<egui::ColorImage> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if px[(y * w + x) * 4 + 3] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let (x0, y0) = (x0.saturating_sub(16), y0.saturating_sub(16));
    let (x1, y1) = ((x1 + 16).min(w - 1), (y1 + 16).min(h - 1));
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in y0..=y1 {
        out.extend_from_slice(&px[(y * w + x0) * 4..(y * w + x1 + 1) * 4]);
    }
    Some(egui::ColorImage::from_rgba_premultiplied([cw, ch], &out))
}

/// One card per effect: the applied choice for the applied effect, the tried
/// choice for the tried one, else the effect's first preset.
pub fn cards(
    options: &[(String, Choice)],
    applied: Option<&Choice>,
    trying: Option<&Choice>,
) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    for (_, c) in options {
        if out.iter().any(|o| o.id == c.id) {
            continue;
        }
        let pick = [trying, applied]
            .into_iter()
            .flatten()
            .find(|x| x.id == c.id)
            .cloned();
        out.push(pick.unwrap_or_else(|| c.clone()));
    }
    out
}

/// The name of an effect (without its preset).
fn effect_name(effect: Option<&Effect>, id: &str, lang: &str) -> String {
    effect.map_or(id.to_string(), |e| {
        localized(&e.names, lang, &e.id).to_string()
    })
}

fn param_label(p: &Param, lang: &str) -> String {
    localized(&p.label, lang, &p.id).to_string()
}

fn hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}
fn color_of(v: &J) -> Option<Color32> {
    let s = v.as_str()?.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}
/// The landscape value of a number that may be a `{landscape, portrait}` pair.
fn number_of(v: &J) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.get("landscape").and_then(J::as_f64))
}
/// A new number keeping the portrait/landscape ratio of a pair.
fn with_number(old: &J, n: f64, p: &Param) -> J {
    let bounded = |x: f64| x.clamp(p.min.unwrap_or(f64::MIN), p.max.unwrap_or(f64::MAX));
    match old {
        J::Object(m) => {
            let l = m.get("landscape").and_then(J::as_f64).unwrap_or(n);
            let r = m.get("portrait").and_then(J::as_f64).unwrap_or(n);
            let ratio = if l.abs() > f64::EPSILON { r / l } else { 1.0 };
            // The range already keeps both inside the bounds; the clamp only
            // absorbs floating-point and rounding edges.
            serde_json::json!({ "landscape": bounded(n), "portrait": bounded((n * ratio * 100.0).round() / 100.0) })
        }
        _ => J::from(bounded(n)),
    }
}

/// The landscape slider's range: for a `{landscape, portrait}` pair the portrait
/// value follows at the same ratio, so both stay within the parameter's bounds.
fn slider_range(p: &Param, value: &J) -> (f64, f64, f64) {
    let n = number_of(value).unwrap_or(0.0);
    let (mut lo, mut hi) = (p.min.unwrap_or(0.0), p.max.unwrap_or(n.max(1.0) * 2.0));
    if let J::Object(m) = value
        && let (Some(l), Some(r)) = (
            m.get("landscape").and_then(J::as_f64),
            m.get("portrait").and_then(J::as_f64),
        )
        && l.abs() > f64::EPSILON
    {
        let ratio = r / l;
        if ratio > 1.0 {
            hi = hi.min(p.max.map_or(hi, |m| m / ratio));
        } else if ratio < 1.0 && ratio > 0.0 {
            lo = lo.max(p.min.map_or(lo, |m| m / ratio));
        }
    }
    let hi = hi.max(lo);
    // egui clamps a slider value, then rounds it to the step from the range's
    // start (widgets/slider.rs set_value): put the top end on that grid so the
    // rounding can never land past it.
    let step = if hi - lo > 20.0 { 1.0 } else { 0.05 };
    (lo, lo + ((hi - lo) / step + 1e-9).floor() * step, step)
}

/// The value a parameter has now: the choice's own, else its preset's, else the default.
fn current(effect: &Effect, choice: &Choice, p: &Param) -> J {
    choice
        .values
        .get(&p.id)
        .cloned()
        .or_else(|| {
            effect
                .presets
                .iter()
                .find(|(id, _)| *id == choice.preset)
                .and_then(|(_, v)| v.get(&p.id).cloned())
        })
        .unwrap_or_else(|| p.default.clone())
}

impl super::NohApp {
    /// The effect definition for a choice, from the catalogue.
    pub(super) fn effect_of(&mut self, choice: &Choice) -> Option<Effect> {
        self.effects
            .catalog()
            .find(&choice.id)
            .map(|e| e.effect.clone())
    }

    /// The style shown on the monitor: the tried one, else the applied one.
    pub(super) fn shown_effect(&self) -> Option<&Choice> {
        match self.style_panel.try_failed {
            Some(_) => self.project.effect.as_ref(),
            None => self
                .style_panel
                .trying
                .as_ref()
                .or(self.project.effect.as_ref()),
        }
    }

    /// Ends a try-on (cancelled, applied or replaced): its failure goes with it.
    pub(super) fn end_try(&mut self) {
        self.style_panel.trying = None;
        self.style_panel.try_failed = None;
    }

    fn apply_try(&mut self) {
        let failed = self.style_panel.try_failed.is_some();
        if let Some(c) = self.style_panel.trying.take() {
            // Applying a style gives the styles back (scene or style, not both).
            self.motion_use(false);
            self.style_panel.try_failed = None;
            self.choose_effect(Some(c));
            // A style the preview could not draw is applied like any other: the
            // preview then says so the usual way (libass on the monitor).
            if failed {
                self.project.effect_preview_failed = true;
                self.mini_preview.invalidate();
            }
        }
    }

    /// Échap cancels a try-on, unless a text field or a popup has the keyboard.
    fn escape_cancels_try(&mut self, ui: &egui::Ui) {
        if self.style_panel.trying.is_some()
            && !ui.ctx().egui_wants_keyboard_input()
            && !egui::Popup::is_any_open(ui.ctx())
            && !self.style_panel.sheet
            && !self.settings_open
            && ui.ctx().memory(|m| m.top_modal_layer().is_none())
            && ui.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.end_try();
        }
    }

    /// The wide panel beside the monitor column, `height` tall.
    pub(super) fn style_panel(&mut self, ui: &mut egui::Ui, height: f32) {
        let before = (
            self.project.apply_subtitles,
            self.project.caption_style.clone(),
        );
        self.style_panel_inner(ui, height);
        self.after_style_change(before);
    }

    /// As the lyrics menu did: showing, hiding or restyling shown captions
    /// invalidates the preview and the last exports' "up to date" status.
    pub(super) fn after_style_change(&mut self, before: (bool, noh::captions::CaptionStyle)) {
        if before.0 != self.project.apply_subtitles
            || (self.project.apply_subtitles && before.1 != self.project.caption_style)
        {
            self.invalidated();
        }
    }

    /// The switch, only once lyrics are there (as in the lyrics menu before):
    /// on without lyrics, exports would wait for a track that is not coming.
    fn style_switch(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let has_lyrics = self.has_lyrics();
        let mut on = self.project.apply_subtitles;
        let response = ui
            .add_enabled_ui(has_lyrics, |ui| {
                style::switch(ui, &mut on, l.text("style.show"))
            })
            .inner;
        if response.changed() {
            self.project.apply_subtitles = on;
            if !on {
                self.end_try();
            }
        }
    }

    fn style_panel_inner(&mut self, ui: &mut egui::Ui, height: f32) {
        self.escape_cancels_try(ui);
        let l = self.locale.language;
        let t = style::tokens(ui);
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, style::radius::L, t.surface);
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("style-panel")
                .max_rect(rect.shrink(space::M))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let ui = &mut inner;
        ui.spacing_mut().item_spacing.y = space::S;
        // Header: the switch, and Styles / Réglages (on a second row when narrow).
        let tabs = [l.text("style.tab_styles"), l.text("style.tab_settings")];
        let label_w = ui
            .painter()
            .layout_no_wrap(
                l.text("style.show").into(),
                style::semibold(style::text::BODY),
                t.text,
            )
            .size()
            .x;
        let one_row = 32.0 + space::S + label_w + space::M + style::segmented_width(ui, &tabs)
            <= ui.available_width();
        let tab_row = |ui: &mut egui::Ui, this: &mut Self| {
            if let Some(i) = style::segmented(
                ui,
                "style-tab",
                &tabs,
                usize::from(this.style_panel.settings),
            ) {
                this.style_panel.settings = i == 1;
            }
        };
        ui.horizontal(|ui| {
            self.style_switch(ui);
            ui.label(style::strong(l.text("style.show"), style::text::BODY).color(t.text));
            if one_row {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right to left: draw the control as a whole at the right edge.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        tab_row(ui, self)
                    });
                });
            }
        });
        // Beside the preview, « more settings » sits at the right of the tabs
        // (the list below has the height of two settings).
        let more_in_tabs = !one_row && self.style_panel.settings && self.settings_level2() > 0;
        if !one_row {
            ui.horizontal(|ui| {
                tab_row(ui, self);
                if more_in_tabs {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.more_toggle(ui);
                    });
                }
            });
        }
        // With a scene listed under the styles, the footer's room goes to the
        // list while it would only say « one style for both » (a try-on or a
        // warning keeps it).
        let quiet = self.motion.found.is_some()
            && self.style_panel.trying.is_none()
            && self.has_lyrics()
            && self.project.apply_subtitles
            && !(self.project.effect.is_some() && self.effects.preview_failure().is_some())
            && !(self.project.effect.is_some()
                && self.mini_preview.short
                && self.effects.overflow());
        let footer_h = if quiet { 0.0 } else { 44.0 };
        let body_h = (ui.available_height() - footer_h - space::S).max(40.0);
        // A solid bar, as the page body's: beside the preview the list has a fixed
        // height, and a floating bar (shown on hover only) would hide that a third
        // setting or a seventh style lies below.
        let nested = ui.spacing().scroll;
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        if self.style_panel.settings {
            egui::ScrollArea::vertical()
                .id_salt("style-settings")
                .max_height(body_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().scroll = nested;
                    self.style_settings(ui, !more_in_tabs)
                });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("style-cards")
                .max_height(body_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().scroll = nested;
                    self.style_cards(ui, false);
                    self.motion_section(ui);
                });
        }
        ui.spacing_mut().scroll = nested;
        ui.add_space((ui.available_height() - footer_h).max(0.0));
        if !quiet {
            self.style_footer(ui, false);
        }
    }

    /// Compact: a row under the transport (header, one scrolling line of cards).
    pub(super) fn style_row(&mut self, ui: &mut egui::Ui) {
        let before = (
            self.project.apply_subtitles,
            self.project.caption_style.clone(),
        );
        self.style_row_inner(ui);
        self.after_style_change(before);
    }

    fn style_row_inner(&mut self, ui: &mut egui::Ui) {
        self.escape_cancels_try(ui);
        let l = self.locale.language;
        let t = style::tokens(ui);
        egui::Frame::new()
            .fill(t.surface)
            .corner_radius(style::radius::L)
            .inner_margin(space::M)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = space::S;
                ui.horizontal(|ui| {
                    self.style_switch(ui);
                    ui.label(style::strong(l.text("style.show"), style::text::BODY).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.style_panel.trying.is_some() {
                            if style::button(ui, l.text("style.apply"), true, true, 0.0).clicked() {
                                self.apply_try();
                            }
                            if style::button(ui, l.text("style.cancel"), true, false, 0.0).clicked()
                            {
                                self.end_try();
                            }
                        } else if style::button(
                            ui,
                            l.text("style.tab_settings"),
                            self.project.apply_subtitles || !self.has_lyrics(),
                            false,
                            0.0,
                        )
                        .clicked()
                        {
                            self.style_panel.sheet = true;
                        }
                    });
                });
                egui::ScrollArea::horizontal()
                    .id_salt("style-row")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| self.style_cards(ui, true));
                    });
                self.style_footer(ui, true);
            });
        if self.style_panel.sheet {
            let mut open = true;
            egui::Window::new(l.text("style.tab_settings"))
                .id(egui::Id::new("style-sheet"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -16.0))
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.set_width(ui.available_width().min(388.0));
                    self.style_settings(ui, true);
                    if style::button(ui, l.text("style.close"), true, false, 0.0).clicked() {
                        self.style_panel.sheet = false;
                    }
                });
            if !open {
                self.style_panel.sheet = false;
            }
        }
    }

    fn style_cards(&mut self, ui: &mut egui::Ui, row: bool) {
        let l = self.locale.language;
        let lang = l.code().to_string();
        let t = style::tokens(ui);
        let options = self.effects.options(&lang);
        let cards = cards(
            &options,
            self.project.effect.as_ref(),
            self.style_panel.trying.as_ref(),
        );
        // Without lyrics the styles stay choosable (applied as soon as lyrics are
        // there); with lyrics and the switch off they are greyed, as in mockup v9.
        let enabled = self.project.apply_subtitles || !self.has_lyrics();
        let width = ui.available_width();
        let cols = if row {
            cards.len().max(1)
        } else {
            (((width + space::S) / (CARD_MIN + space::S)).floor() as usize).max(1)
        };
        let card_w = if row {
            104.0
        } else {
            (width - space::S * (cols as f32 - 1.0)) / cols as f32
        };
        let mut clicked: Option<Choice> = None;
        for chunk in cards.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                for choice in chunk {
                    let effect = self.effect_of(choice);
                    let name = effect_name(effect.as_ref(), &choice.id, &lang);
                    // A scene in use sets the styles aside: none is « in use ».
                    let applied = !self.motion.used
                        && self
                            .project
                            .effect
                            .as_ref()
                            .is_some_and(|a| a.id == choice.id);
                    let tried = self
                        .style_panel
                        .trying
                        .as_ref()
                        .is_some_and(|a| a.id == choice.id);
                    let styled = self.styled(choice);
                    let tex = self
                        .style_panel
                        .samples
                        .texture(ui.ctx(), &styled, effect.as_ref());
                    let sense = if enabled {
                        Sense::click()
                    } else {
                        Sense::hover()
                    };
                    let (rect, response) = ui.allocate_exact_size(vec2(card_w, CARD_H), sense);
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::RadioButton,
                            enabled,
                            applied || tried,
                            &name,
                        )
                    });
                    let p = ui.painter();
                    let fade = if enabled { 1.0 } else { 0.45 };
                    p.rect_filled(rect, style::radius::L, t.surface);
                    let outline = if tried {
                        egui::Stroke::new(2.0, t.accent)
                    } else if applied {
                        egui::Stroke::new(2.0, t.text)
                    } else {
                        egui::Stroke::new(
                            1.0,
                            if response.hovered() {
                                t.border_strong
                            } else {
                                t.border
                            },
                        )
                    };
                    p.rect_stroke(rect, style::radius::L, outline, egui::StrokeKind::Inside);
                    let tile = Rect::from_min_size(
                        rect.min + vec2(5.0, 5.0),
                        vec2(rect.width() - 10.0, TILE_H),
                    );
                    p.rect_filled(
                        tile,
                        5,
                        Color32::from_rgb(0x2B, 0x33, 0x40).gamma_multiply(fade),
                    );
                    if let Some(tex) = &tex {
                        let size = tex.size_vec2();
                        let scale = (tile.width() / size.x).min(tile.height() / size.y);
                        let shown = Rect::from_center_size(tile.center(), size * scale);
                        p.image(
                            tex.id(),
                            shown,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE.gamma_multiply(fade),
                        );
                    }
                    // The state pill sits on the name row, right-aligned, so it never
                    // covers the sample drawn in the tile.
                    let pill = if tried {
                        Some((l.text("style.trying"), t.accent_text))
                    } else if applied {
                        Some((l.text("style.used"), t.ok))
                    } else {
                        None
                    }
                    .map(|(text, color)| {
                        (
                            p.layout_no_wrap(text.into(), egui::FontId::proportional(11.0), color),
                            color,
                        )
                    });
                    let pill_w = pill.as_ref().map_or(0.0, |(g, _)| g.size().x + 8.0 + 4.0);
                    // One line, ellipsised: the full name is the card's accessible label.
                    let mut job = egui::text::LayoutJob::simple(
                        name.clone(),
                        egui::FontId::proportional(style::text::SMALL),
                        t.text.gamma_multiply(fade),
                        (rect.width() - 10.0 - pill_w).max(10.0),
                    );
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    let galley = p.layout_job(job);
                    let name_h = galley.size().y;
                    p.galley(pos2(rect.left() + 5.0, tile.bottom() + 2.0), galley, t.text);
                    if let Some((g, color)) = pill {
                        let size = g.size() + vec2(8.0, 2.0);
                        let top = tile.bottom() + 2.0 + (name_h - size.y) / 2.0;
                        let r = Rect::from_min_size(pos2(rect.right() - 5.0 - size.x, top), size);
                        p.rect_filled(r, 4, t.bg);
                        p.galley(r.min + vec2(4.0, 1.0), g, color);
                    }
                    style::focus(ui, &response);
                    if response.clicked() {
                        clicked = Some(choice.clone());
                    }
                }
            });
        }
        let at = Rect::from_min_size(ui.min_rect().min, vec2(1.0, 1.0));
        self.style_panel.samples.paint(ui, at);
        if let Some(choice) = clicked {
            let applied = self
                .project
                .effect
                .as_ref()
                .is_some_and(|a| a.id == choice.id);
            // The applied card ends a try-on; another card is tried, not applied.
            // With a scene in use, the style that was applied comes back at once.
            self.end_try();
            if applied && self.motion.used {
                self.motion_use(false);
            } else if !applied {
                self.style_panel.trying = Some(choice);
            }
        }
    }

    /// The applied style's settings: presets, level-1 then level-2 parameters.
    /// A font setting: the plugin's own face first, NOH's other faces, then the
    /// installed families (read off the UI thread). A chosen installed family
    /// this computer lacks is said under the picker; NOH draws the plugin's face.
    fn font_picker(
        &mut self,
        ui: &mut egui::Ui,
        p: &noh::style::plugin::Param,
        value: &serde_json::Value,
        next: &mut Choice,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let now = value.as_str().unwrap_or_default().to_string();
        let own = p.default.as_str().unwrap_or("sans").to_string();
        let own_label = format!("{} ({})", l.text("style.font_plugin"), face_name(&own));
        let shown = match now.strip_prefix("system:") {
            Some(family) => family.to_string(),
            None if now == own => own_label.clone(),
            None => face_name(&now).to_string(),
        };
        let families = self
            .effects
            .installed_families(ui.ctx())
            .map(<[String]>::to_vec);
        let salt = ("style-font", p.id.clone());
        let open_now = std::mem::take(&mut self.style_panel.font_open);
        let mut pick = None;
        let installed = self.effects.installed_fonts();
        let mut rows = std::mem::take(&mut self.style_panel.font_rows);
        rows.poll(ui.ctx());
        let symbols = l.text("style.font_symbols");
        // A row: its name in its own face once drawn, else in the interface font;
        // drawn the first time it shows.
        let mut row = |ui: &mut egui::Ui, selected: bool, value: &str, text: &str| {
            let px = style::text::BODY * ui.ctx().pixels_per_point();
            let key = super::app_font_rows::Key::new(value, &own, text, px);
            let response = match rows.peek(&key) {
                Some(super::app_font_rows::Row::Drawn(texture, size)) => {
                    let response = ui.add(egui::Button::selectable(
                        selected,
                        egui::Image::new(egui::load::SizedTexture::new(texture.id(), *size))
                            .tint(t.text),
                    ));
                    // The drawn name is an image: assistive tools get the text.
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            selected,
                            text,
                        )
                    });
                    response
                }
                Some(super::app_font_rows::Row::Symbol) => {
                    ui.selectable_label(selected, format!("{text} ({symbols})"))
                }
                _ => ui.selectable_label(selected, text),
            };
            if rows.peek(&key).is_none() && ui.is_rect_visible(response.rect) {
                rows.want(ui.ctx(), key, installed.clone());
            }
            response.clicked()
        };
        let small = |text: &str| {
            egui::RichText::new(text)
                .size(style::text::SMALL)
                .color(t.text_2)
        };
        let picker = egui::ComboBox::from_id_salt(salt)
            .selected_text(&shown)
            .width(ui.available_width().min(260.0))
            .height(420.0)
            .show_ui(ui, |ui| {
                // Rows as tight as a menu's, so the installed fonts show below
                // NOH's seven without scrolling the list.
                ui.spacing_mut().button_padding.y = 3.0;
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.spacing_mut().interact_size.y = 24.0;
                if row(ui, now == own, &own, &own_label) {
                    pick = Some(own.clone());
                }
                for face in BUNDLED_FACES.iter().filter(|f| **f != own) {
                    if row(ui, now == *face, face, face_name(face)) {
                        pick = Some(face.to_string());
                    }
                }
                ui.separator();
                ui.label(small(l.text("style.fonts_installed")));
                match &families {
                    None => {
                        ui.label(small(l.text("style.fonts_reading")));
                    }
                    Some(list) if list.is_empty() => {
                        ui.label(small(l.text("style.fonts_none")));
                    }
                    Some(list) => {
                        for family in list {
                            let v = format!("system:{family}");
                            if row(ui, now == v, &v, family) {
                                pick = Some(v);
                            }
                        }
                    }
                }
            })
            .response
            .on_hover_text(l.text("style.font_licence"));
        self.style_panel.font_rows = rows;
        // Opened after the picker is laid out, as a click opens it (capture, tests):
        // egui keeps a combo box's list under its button's id + "popup".
        if open_now {
            egui::Popup::open_id(ui.ctx(), picker.id.with("popup"));
        }
        let reveal = std::mem::take(&mut self.style_panel.font_reveal);
        if let Some(v) = pick {
            next.values.insert(p.id.clone(), serde_json::json!(v));
        }
        if let Some(family) = now.strip_prefix("system:") {
            let missing = families
                .as_ref()
                .is_some_and(|list| !list.iter().any(|f| f == family))
                || self.effects.font_missing().as_deref() == Some(family);
            if missing {
                let said = egui::Frame::new()
                    .fill(t.warn_soft)
                    .corner_radius(style::radius::M)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    l.format("style.font_missing", &[family.to_string()]),
                                )
                                .size(style::text::SMALL)
                                .color(t.text),
                            )
                            .wrap(),
                        );
                    });
                if reveal {
                    said.response.scroll_to_me(Some(egui::Align::Max));
                }
                return;
            }
        }
        if reveal {
            picker.scroll_to_me(Some(egui::Align::Max));
        }
    }

    /// The applied effect's second-level settings (none for Classique here).
    fn settings_level2(&mut self) -> usize {
        let Some(choice) = self.project.effect.clone() else {
            return 0;
        };
        if choice.is_classic() {
            return 0;
        }
        self.effect_of(&choice)
            .map_or(0, |e| e.params.iter().filter(|p| p.level == 2).count())
    }

    /// « More settings (n) » / « Fewer settings ».
    fn more_toggle(&mut self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let text = if self.style_panel.more {
            l.text("style.less").to_string()
        } else {
            l.format("style.more", &[self.settings_level2().to_string()])
        };
        if ui
            .link(egui::RichText::new(text).size(style::text::SMALL))
            .clicked()
        {
            self.style_panel.more = !self.style_panel.more;
        }
    }

    /// `more_here`: the « more settings » toggle under the settings (in the
    /// sheet, or when the tabs share the header row).
    fn style_settings(&mut self, ui: &mut egui::Ui, more_here: bool) {
        let l = self.locale.language;
        let lang = l.code().to_string();
        let t = style::tokens(ui);
        let off = !self.project.apply_subtitles && self.has_lyrics();
        if self.style_panel.trying.is_some() || off {
            let text = if off {
                l.text("style.off")
            } else {
                l.text("style.settings_while_trying")
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(style::text::SMALL)
                        .color(t.text_2),
                )
                .wrap(),
            );
            return;
        }
        let Some(choice) = self.project.effect.clone() else {
            return;
        };
        let Some(effect) = self.effect_of(&choice) else {
            return;
        };
        let name = effect_name(Some(&effect), &choice.id, &lang);
        ui.label(
            style::strong(l.format("style.settings_of", &[name]), style::text::BODY).color(t.text),
        );
        if choice.is_classic() {
            self.classic_settings(ui, &effect, &lang);
            return;
        }
        let mut next = choice.clone();
        if effect.presets.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for ((id, _), names) in effect.presets.iter().zip(&effect.preset_names) {
                    let on = choice.preset == *id;
                    if ui
                        .add(egui::Button::selectable(on, localized(names, &lang, id)))
                        .clicked()
                        && !on
                    {
                        next.preset = id.clone();
                        next.values.clear();
                    }
                }
            });
        }
        let level2 = effect.params.iter().filter(|p| p.level == 2).count();
        let more = self.style_panel.more;
        for p in effect.params.iter().filter(|p| p.level == 1 || more) {
            let value = current(&effect, &choice, p);
            let label = param_label(p, &lang);
            ui.label(
                egui::RichText::new(&label)
                    .size(style::text::SMALL)
                    .color(t.text_2),
            );
            // A label sits closer to its own control (half the item spacing) than
            // to the setting above, so two settings fit beside the preview at 980.
            ui.add_space(-ui.spacing().item_spacing.y / 2.0);
            match p.kind.as_str() {
                "number" => {
                    let mut n = number_of(&value).unwrap_or(0.0);
                    let (lo, hi, step) = slider_range(p, &value);
                    if ui
                        .add(egui::Slider::new(&mut n, lo..=hi).step_by(step))
                        .changed()
                    {
                        next.values
                            .insert(p.id.clone(), with_number(&value, n.clamp(lo, hi), p));
                    }
                }
                "color" => {
                    let mut swatches: Vec<Color32> = std::iter::once(&p.default)
                        .chain(effect.presets.iter().filter_map(|(_, v)| v.get(&p.id)))
                        .filter_map(color_of)
                        .collect();
                    swatches.dedup();
                    let now = color_of(&value);
                    ui.horizontal_wrapped(|ui| {
                        for c in swatches {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                            ui.painter().circle_filled(r.center(), 13.0, c);
                            ui.painter().circle_stroke(
                                r.center(),
                                13.0,
                                egui::Stroke::new(1.0, t.border_strong),
                            );
                            if now == Some(c) {
                                ui.painter().circle_stroke(
                                    r.center(),
                                    16.0,
                                    egui::Stroke::new(2.0, t.text),
                                );
                            }
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::RadioButton,
                                    true,
                                    now == Some(c),
                                    hex(c),
                                )
                            });
                            style::focus(ui, &resp);
                            if resp.clicked() {
                                next.values.insert(p.id.clone(), J::from(hex(c)));
                            }
                        }
                        let mut free = now.unwrap_or(Color32::WHITE);
                        ui.label(
                            egui::RichText::new(l.text("style.other_color"))
                                .size(style::text::SMALL)
                                .color(t.text_2),
                        );
                        if egui::color_picker::color_edit_button_srgba(
                            ui,
                            &mut free,
                            egui::color_picker::Alpha::Opaque,
                        )
                        .changed()
                        {
                            next.values.insert(p.id.clone(), J::from(hex(free)));
                        }
                    });
                }
                "font" => self.font_picker(ui, p, &value, &mut next),
                "palette" => {
                    // Palettes change with the presets above.
                    ui.horizontal(|ui| {
                        for c in value.as_array().into_iter().flatten().filter_map(color_of) {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 9.0, c);
                        }
                    });
                }
                "bool" => {
                    let mut b = value.as_bool().unwrap_or(false);
                    if style::switch(ui, &mut b, &label).changed() {
                        next.values.insert(p.id.clone(), J::from(b));
                    }
                }
                "choice" if !p.choices.is_empty() => {
                    let options: Vec<&str> = p.choices.iter().map(String::as_str).collect();
                    // Shown in the interface language when the effect names its choices.
                    let shown: Vec<&str> = options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            p.choice_labels
                                .get(i)
                                .map_or(*o, |names| localized(names, &lang, o))
                        })
                        .collect();
                    let at = value
                        .as_str()
                        .and_then(|s| options.iter().position(|o| *o == s))
                        .unwrap_or(0);
                    if let Some(i) = style::segmented(ui, &format!("style-{}", p.id), &shown, at) {
                        next.values.insert(p.id.clone(), J::from(options[i]));
                    }
                }
                _ => {}
            }
        }
        if level2 > 0 && more_here {
            self.more_toggle(ui);
        }
        if next != choice {
            self.choose_effect(Some(next));
        }
    }

    /// Classique keeps today's Taille and Position (the caption style).
    fn classic_settings(&mut self, ui: &mut egui::Ui, effect: &Effect, lang: &str) {
        use noh::captions::{CaptionPlacement, CaptionSize};
        let l = self.locale.language;
        let t = style::tokens(ui);
        let sizes = [CaptionSize::Small, CaptionSize::Medium, CaptionSize::Large];
        let places = [CaptionPlacement::Bottom, CaptionPlacement::Top];
        ui.label(
            egui::RichText::new(l.text("ui.caption_size"))
                .size(style::text::SMALL)
                .color(t.text_2),
        );
        let at = sizes
            .iter()
            .position(|s| *s == self.project.caption_style.size)
            .unwrap_or(1);
        if let Some(i) = style::segmented(
            ui,
            "style-size",
            &[
                l.text("ui.caption_small"),
                l.text("ui.caption_medium"),
                l.text("ui.caption_large"),
            ],
            at,
        ) {
            self.project.caption_style.size = sizes[i];
        }
        ui.label(
            egui::RichText::new(l.text("ui.caption_placement"))
                .size(style::text::SMALL)
                .color(t.text_2),
        );
        let at = places
            .iter()
            .position(|p| *p == self.project.caption_style.placement)
            .unwrap_or(0);
        if let Some(i) = style::segmented(
            ui,
            "style-place",
            &[l.text("ui.caption_bottom"), l.text("ui.caption_top")],
            at,
        ) {
            self.project.caption_style.placement = places[i];
        }
        // « Plus de réglages »: the line width, in % of the usable area. It lives in
        // the caption style, so the libass fallback wraps where Classique does.
        let Some(p) = effect.params.iter().find(|p| p.id == "width") else {
            return;
        };
        if self.style_panel.more {
            ui.label(
                egui::RichText::new(param_label(p, lang))
                    .size(style::text::SMALL)
                    .color(t.text_2),
            );
            let (lo, hi) = (p.min.unwrap_or(40.0), p.max.unwrap_or(100.0));
            let default = p.default.as_f64().unwrap_or(69.0);
            let mut n = self
                .project
                .caption_style
                .line_width
                .map_or(default, |h| f64::from(h) / 100.0);
            // Edits: drawing the slider never rounds the historical 69.06 % to a
            // step (only the user's input is); the user's 69 is that default again.
            let slider = egui::Slider::new(&mut n, lo..=hi)
                .step_by(1.0)
                .fixed_decimals(0)
                .suffix(" %")
                .clamping(egui::SliderClamping::Edits);
            if ui.add(slider).changed() {
                let n = n.clamp(lo, hi).round();
                self.project.caption_style.line_width =
                    (n != default.round()).then(|| (n * 100.0) as u32);
            }
        }
        let text = if self.style_panel.more {
            l.text("style.less").to_string()
        } else {
            l.format("style.more", &["1".to_string()])
        };
        if ui
            .link(egui::RichText::new(text).size(style::text::SMALL))
            .clicked()
        {
            self.style_panel.more = !self.style_panel.more;
        }
    }

    /// One message, most urgent first; Annuler / Appliquer while trying (wide).
    fn style_footer(&mut self, ui: &mut egui::Ui, compact: bool) {
        let l = self.locale.language;
        let lang = l.code().to_string();
        let t = style::tokens(ui);
        let trying = self.style_panel.trying.clone();
        let warn = |text: &str| (text.to_string(), true);
        let (text, is_warn) = if let (Some(tc), Some(_)) = (&trying, &self.style_panel.try_failed) {
            let te = self.effect_of(tc);
            warn(&l.format(
                "style.try_failed",
                &[effect_name(te.as_ref(), &tc.id, &lang)],
            ))
        } else if let Some(tc) = &trying {
            let te = self.effect_of(tc);
            let tried = effect_name(te.as_ref(), &tc.id, &lang);
            let applied = self.project.effect.clone();
            let ae = applied.as_ref().and_then(|a| self.effect_of(a));
            let applied = applied.map_or(l.text("effect.none").to_string(), |a| {
                effect_name(ae.as_ref(), &a.id, &lang)
            });
            (l.format("style.try_note", &[tried, applied]), false)
        } else if !self.has_lyrics() {
            (l.text("style.no_lyrics").to_string(), false)
        } else if !self.project.apply_subtitles {
            (l.text("style.off").to_string(), false)
        } else if self.project.effect.is_some() && self.effects.preview_failure().is_some() {
            warn(l.text("effect.preview_failure"))
        } else if self.project.effect.is_some()
            && self.mini_preview.short
            && self.effects.overflow()
        {
            warn(l.text("effect.overflow"))
        } else {
            (l.text("style.one_style").to_string(), false)
        };
        if compact && trying.is_none() && !is_warn && self.project.apply_subtitles {
            return;
        }
        let rejected = self.effects.rejected();
        let frame = egui::Frame::new()
            .fill(if is_warn {
                t.warn_soft
            } else {
                Color32::TRANSPARENT
            })
            .corner_radius(style::radius::M)
            .inner_margin(egui::Margin::symmetric(8, 6));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                if !compact && trying.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if style::button(ui, l.text("style.apply"), true, true, 0.0).clicked() {
                            self.apply_try();
                        }
                        if style::button(ui, l.text("style.cancel"), true, false, 0.0).clicked() {
                            self.end_try();
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&text)
                                    .size(style::text::SMALL)
                                    .color(t.text_2),
                            )
                            .wrap(),
                        );
                    });
                } else {
                    let color = if is_warn { t.text } else { t.text_2 };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&text)
                                .size(style::text::SMALL)
                                .color(color),
                        )
                        .wrap(),
                    );
                }
            });
            if !rejected.is_empty() {
                ui.label(
                    egui::RichText::new(l.plural("effect.rejected", rejected.len()))
                        .size(style::text::SMALL)
                        .color(t.text_2),
                )
                .on_hover_text(rejected.join("\n"));
            }
        });
    }

    /// The try-on tag over the monitor.
    pub(super) fn paint_try_tag(&self, ui: &egui::Ui, monitor: Rect) {
        let Some(tc) = &self.style_panel.trying else {
            return;
        };
        if self.style_panel.try_failed.is_some() {
            return;
        }
        let lang = self.locale.language.code();
        let name = self
            .effects
            .catalog_ref()
            .and_then(|c| c.find(&tc.id))
            .map_or(tc.id.clone(), |e| {
                localized(&e.effect.names, lang, &e.effect.id).to_string()
            });
        let text = self.locale.language.format("style.try_tag", &[name]);
        let galley = ui.painter().layout_no_wrap(
            text,
            egui::FontId::proportional(style::text::SMALL),
            Color32::WHITE,
        );
        let rect = Rect::from_min_size(
            monitor.min + vec2(8.0, 8.0),
            galley.size() + vec2(12.0, 6.0),
        );
        ui.painter()
            .rect_filled(rect, style::radius::S, Color32::from_black_alpha(170));
        ui.painter()
            .galley(rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::fx::shaders::Output;

    #[test]
    fn split_at_980x850_keeps_a_wide_panel() {
        // 980 wide: 932 of content (24 margins); 850 high; 16:9; S1 budget 530.
        let s = split(932.0, 850.0, 16.0 / 9.0, 530.0).unwrap();
        assert!((s.height - 320.0).abs() < 0.5, "{s:?}");
        assert!((s.column - 568.9).abs() < 0.5, "{s:?}");
        assert!((s.panel - 347.1).abs() < 0.5, "{s:?}");
    }

    #[test]
    fn taller_windows_stop_the_monitor_so_the_panel_keeps_300() {
        let s = split(932.0, 1200.0, 16.0 / 9.0, 530.0).unwrap();
        assert!((s.panel - PANEL_MIN).abs() < 0.01, "{s:?}");
        assert!(
            (s.height - (932.0 - 316.0) * 9.0 / 16.0).abs() < 0.5,
            "{s:?}"
        );
        let wide = split(1352.0, 1000.0, 16.0 / 9.0, 530.0).unwrap();
        assert!(
            (wide.height - 360.0).abs() < 0.5 && wide.panel > 690.0,
            "{wide:?}"
        );
    }

    #[test]
    fn the_short_keeps_a_16_9_column_and_narrow_windows_get_no_side_panel() {
        let short = split(932.0, 850.0, 9.0 / 16.0, 530.0).unwrap();
        let video = split(932.0, 850.0, 16.0 / 9.0, 530.0).unwrap();
        assert_eq!(short, video);
        // Below 636 there is no room for 300 beside a 180 px monitor.
        assert!(split(630.0, 850.0, 16.0 / 9.0, 530.0).is_none());
        assert!(split(640.0, 850.0, 16.0 / 9.0, 530.0).is_some());
        // The two-row transport's budget lowers the monitor, as today.
        let two = split(932.0, 850.0, 16.0 / 9.0, 570.0).unwrap();
        assert!((two.height - 280.0).abs() < 0.5, "{two:?}");
    }

    fn choice(id: &str, preset: &str) -> Choice {
        Choice {
            id: id.into(),
            preset: preset.into(),
            dir: Default::default(),
            values: Default::default(),
        }
    }

    #[test]
    fn one_card_per_effect_showing_the_applied_and_tried_presets() {
        let options = vec![
            (String::new(), choice("noh.classic", "classique")),
            (String::new(), choice("test.wipe", "jaune")),
            (String::new(), choice("test.wipe", "rose")),
            (String::new(), choice("test.glow", "arc")),
            (String::new(), choice("test.glow", "glace")),
        ];
        let applied = choice("test.wipe", "rose");
        let tried = choice("test.glow", "glace");
        let c = cards(&options, Some(&applied), Some(&tried));
        assert_eq!(
            c.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["noh.classic", "test.wipe", "test.glow"]
        );
        assert_eq!(c[1].preset, "rose");
        assert_eq!(c[2].preset, "glace");
    }

    #[test]
    fn number_pairs_keep_their_portrait_ratio() {
        let v = serde_json::json!({"landscape": 80, "portrait": 92});
        let p = Param {
            id: "size".into(),
            kind: "number".into(),
            default: v.clone(),
            level: 1,
            min: Some(24.0),
            max: Some(160.0),
            choices: vec![],
            label: Default::default(),
            choice_labels: vec![],
        };
        assert_eq!(
            with_number(&v, 40.0, &p),
            serde_json::json!({"landscape": 40.0, "portrait": 46.0})
        );
        assert_eq!(with_number(&J::from(2.4), 1.0, &p), J::from(24.0));
        assert_eq!(number_of(&v), Some(80.0));
    }

    fn app(language: noh::i18n::Language) -> (egui::Context, super::super::NohApp) {
        let mut app = super::super::NohApp::default();
        app.locale.language = language;
        app.project.apply_subtitles = true;
        // Classique, plus the test-only effects (tests/fixtures/fx).
        app.effects.set_catalog(noh::style::catalog::scan(&[
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("effects"),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("fx"),
        ]));
        app.ensure_default_effect();
        let ctx = egui::Context::default();
        super::super::app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        // Rows read as text here; `the_font_picker_draws_each_name_in_its_own_face`
        // checks the drawn rows.
        app.style_panel.font_rows.plain = true;
        (ctx, app)
    }

    fn card(app: &mut super::super::NohApp, id: &str) -> Choice {
        let options = app.effects.options("en");
        cards(&options, app.project.effect.as_ref(), None)
            .into_iter()
            .find(|c| c.id == id)
            .unwrap()
    }

    /// One frame of the wide panel, 383 x 600 (room for every setting); returns the painted texts.
    fn panel(
        ctx: &egui::Context,
        app: &mut super::super::NohApp,
        events: Vec<egui::Event>,
    ) -> Vec<(String, Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(383.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.style_panel(ui, 600.0));
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect()
    }

    /// Plan D: a plugin's font setting lists its own face first, NOH's other
    /// faces, then the installed families; nothing is chosen for the user.
    #[test]
    fn the_font_picker_offers_the_effect_face_then_noh_and_installed_fonts() {
        let (ctx, mut app) = app(noh::i18n::Language::Fr);
        let c = card(&mut app, "noh.drop");
        app.choose_effect(Some(c));
        app.style_panel.settings = true;
        app.style_panel.more = true;
        app.effects
            .set_installed_for_test(&["Barlow Condensed", "Noto Sans"]);
        let closed: Vec<String> = panel(&ctx, &mut app, vec![])
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert!(
            closed
                .iter()
                .any(|t| t == "Police de l\u{2019}effet (Jost ExtraBold)"),
            "{closed:?}"
        );
        app.style_panel.font_open = true;
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = panel(&ctx, &mut app, vec![]);
        }
        let shown: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for want in [
            "Jost Medium",
            "Barlow Condensed ExtraBold",
            "Polices install\u{e9}es",
            "Barlow Condensed",
            "Noto Sans",
        ] {
            assert!(shown.contains(&want), "{want}: {shown:?}");
        }
        let own = shown
            .iter()
            .position(|t| *t == "Police de l\u{2019}effet (Jost ExtraBold)")
            .unwrap();
        let installed = shown.iter().position(|t| *t == "Barlow Condensed").unwrap();
        assert!(own < installed, "the effect's face first: {shown:?}");
        // The value is untouched until the user picks.
        assert_eq!(
            app.project.effect.as_ref().unwrap().values.get("font"),
            None
        );
    }

    /// Maintainer, 2026-11-12 (« pour les polices, oui, dessine »): each row's name is
    /// drawn in its own face once rasterized, in the same order and row height;
    /// a face that cannot draw its name keeps the interface font.
    #[test]
    fn the_font_picker_draws_each_name_in_its_own_face() {
        let (ctx, mut app) = app(noh::i18n::Language::Fr);
        app.style_panel.font_rows.plain = false;
        app.style_panel.font_rows.sync = true;
        let c = card(&mut app, "noh.drop");
        app.choose_effect(Some(c));
        app.style_panel.settings = true;
        app.style_panel.more = true;
        app.effects.set_installed_for_test(&["Absent Family"]);
        let images = |ctx: &egui::Context, app: &mut super::super::NohApp| {
            app.style_panel.font_open = true;
            let mut out = Vec::new();
            for _ in 0..5 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(383.0, 600.0))),
                    ..Default::default()
                };
                let mut full = ctx.run_ui(input, |ui| app.style_panel(ui, 600.0));
                full.textures_delta.clear();
                out = full
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        // egui paints an image as a textured rectangle.
                        egui::Shape::Rect(r)
                            if r.fill_texture_id() != egui::TextureId::default() =>
                        {
                            Some(r.rect)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
            }
            out
        };
        let drawn = images(&ctx, &mut app);
        // The effect's face and NOH's six others are drawn; the absent family is not.
        assert_eq!(drawn.len(), 7, "{drawn:?}");
        for pair in drawn.windows(2) {
            assert!(pair[0].min.y < pair[1].min.y, "in list order: {drawn:?}");
            let gap = pair[1].min.y - pair[0].min.y;
            assert!(
                (gap - (drawn[1].min.y - drawn[0].min.y)).abs() < 1.5,
                "one row height"
            );
        }
        let texts: Vec<String> = panel(&ctx, &mut app, vec![])
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert!(texts.iter().any(|t| t == "Absent Family"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Jost Medium"), "drawn, not text");
        // A drawn row still carries its name for assistive tools, and picks it.
        let at = drawn[1].center();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut said = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(at), pointer(true)],
            vec![pointer(false)],
        ] {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(383.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| app.style_panel(ui, 600.0));
            out.textures_delta.clear();
            for event in &out.platform_output.events {
                if let egui::output::OutputEvent::Clicked(info) = event {
                    said.extend(info.label.clone());
                }
            }
        }
        assert!(said.iter().any(|l| l == "Jost Medium"), "{said:?}");
        assert_eq!(
            app.project.effect.as_ref().unwrap().values.get("font"),
            Some(&serde_json::json!("jost_medium"))
        );
    }

    /// Plan D item 3: an installed family this computer lacks is said under the
    /// picker (the plugin's own face is drawn); an installed one is not.
    #[test]
    fn a_missing_installed_font_is_said_under_the_picker() {
        let (ctx, mut app) = app(noh::i18n::Language::Fr);
        let c = card(&mut app, "noh.drop");
        app.choose_effect(Some(c));
        app.style_panel.settings = true;
        app.style_panel.more = true;
        app.effects
            .set_installed_for_test(&["Barlow Condensed", "Noto Sans"]);
        let missing = "La police \u{ab} Police Absente \u{bb} n\u{2019}est pas install\u{e9}e sur cet ordinateur : le texte est dessin\u{e9} dans la police de l\u{2019}effet. Installez-la ou choisissez-en une autre.";
        for (value, said) in [
            ("system:Police Absente", true),
            ("system:Barlow Condensed", false),
        ] {
            app.project
                .effect
                .as_mut()
                .unwrap()
                .values
                .insert("font".into(), serde_json::json!(value));
            let texts = panel(&ctx, &mut app, vec![]);
            let shown = texts.iter().any(|(t, _)| t == missing);
            assert_eq!(shown, said, "{value}: {texts:?}");
        }
    }

    /// While the installed fonts are read, the list says so and a chosen installed
    /// family is not called missing (it may be installed); a machine with none
    /// says that, and then a chosen one is missing.
    #[test]
    fn the_font_picker_says_reading_then_none_and_never_missing_too_early() {
        let (ctx, mut app) = app(noh::i18n::Language::Fr);
        let c = card(&mut app, "noh.drop");
        app.choose_effect(Some(c));
        app.style_panel.settings = true;
        app.style_panel.more = true;
        app.project
            .effect
            .as_mut()
            .unwrap()
            .values
            .insert("font".into(), serde_json::json!("system:Police Absente"));
        let missing = |texts: &[(String, Rect)]| {
            texts
                .iter()
                .any(|(t, _)| t.starts_with("La police \u{ab} Police Absente \u{bb}"))
        };
        for (reading, list, said) in [
            (true, &[][..], "Lecture des polices install\u{e9}es\u{2026}"),
            (false, &[][..], "Aucune police install\u{e9}e trouv\u{e9}e"),
        ] {
            if reading {
                app.effects.set_installed_reading_for_test();
            } else {
                app.effects.set_installed_for_test(list);
            }
            app.style_panel.font_open = true;
            let mut texts = Vec::new();
            for _ in 0..3 {
                texts = panel(&ctx, &mut app, vec![]);
            }
            assert!(texts.iter().any(|(t, _)| t == said), "{said}: {texts:?}");
            assert_eq!(missing(&texts), !reading, "reading={reading}: {texts:?}");
        }
    }

    /// An effect's choices show in the interface language when it names them
    /// (`choice_labels`); a choice it does not name shows its own text.
    #[test]
    fn named_choices_show_in_the_interface_language() {
        let (ctx, mut app) = app(noh::i18n::Language::Fr);
        let folder = tempfile::tempdir().unwrap();
        let dir = folder.path().join("labels");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("effect.json"),
            serde_json::json!({
                "engine": 1, "id": "test.labels", "name": {"en": "Labels", "fr": "Libellés"},
                "version": "0.1.0", "license": "GPL-3.0-only", "target": "text",
                "params": [{"id": "size", "type": "choice", "default": "medium",
                            "choices": ["small", "medium", "large"], "level": 1,
                            "choice_labels": {"small": {"fr": "Petite"}, "large": {"fr": "Grande"}},
                            "label": {"en": "Size", "fr": "Taille"}}],
                "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
                "layout": {"font": "sans", "size": 64},
                "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
            })
            .to_string(),
        )
        .unwrap();
        app.effects
            .set_catalog(noh::style::catalog::scan(&[folder.path().to_path_buf()]));
        let c = card(&mut app, "test.labels");
        app.choose_effect(Some(c));
        app.style_panel.settings = true;
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = panel(&ctx, &mut app, vec![]);
        }
        let shown: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            shown.contains(&"Petite") && shown.contains(&"Grande"),
            "{shown:?}"
        );
        assert!(
            shown.contains(&"medium"),
            "an unnamed choice keeps its text: {shown:?}"
        );
        assert!(!shown.contains(&"small"), "{shown:?}");
    }

    /// Task 43: showing « Largeur des lignes » changes nothing; a user's value is
    /// kept, and 69 % is the historical default again.
    #[test]
    fn showing_the_line_width_keeps_the_default() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        assert!(app.project.effect.as_ref().unwrap().is_classic());
        app.style_panel.more = true;
        app.style_panel.settings = true;
        for _ in 0..3 {
            let texts = panel(&ctx, &mut app, vec![]);
            assert!(
                texts.iter().any(|(t, _)| t.starts_with("Line width")),
                "{texts:?}"
            );
        }
        assert_eq!(app.project.caption_style.line_width, None);
        app.project.caption_style.line_width = Some(5500);
        panel(&ctx, &mut app, vec![]);
        assert_eq!(app.project.caption_style.line_width, Some(5500));
    }

    fn click(ctx: &egui::Context, app: &mut super::super::NohApp, at: egui::Pos2) {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        panel(ctx, app, vec![egui::Event::PointerMoved(at), press(true)]);
        panel(ctx, app, vec![press(false)]);
    }

    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        texts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("{text:?} not in {texts:?}"))
            .1
    }

    #[test]
    fn a_tried_style_shows_on_the_monitor_while_exports_keep_the_applied_one() {
        let (_, mut app) = app(noh::i18n::Language::En);
        let karaoke = card(&mut app, "test.wipe");
        app.style_panel.trying = Some(karaoke);
        assert_eq!(app.shown_effect().unwrap().id, "test.wipe");
        assert!(app.project.effect.as_ref().unwrap().is_classic());
        // The player, the preview render and both exports read project.effect.
        assert_eq!(app.preview_effect().unwrap().id, "noh.classic");
        app.apply_try();
        assert_eq!(app.project.effect.as_ref().unwrap().id, "test.wipe");
        assert!(app.style_panel.trying.is_none());
    }

    #[test]
    fn a_card_click_tries_the_style_and_escape_cancels_it() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        app.project.apply_subtitles = true;
        let texts = panel(&ctx, &mut app, vec![]);
        find(&texts, "Captions on the video");
        find(&texts, "In use");
        click(&ctx, &mut app, find(&texts, "Test wipe").center());
        assert_eq!(app.style_panel.trying.as_ref().unwrap().id, "test.wipe");
        assert!(
            app.project.effect.as_ref().unwrap().is_classic(),
            "a click does not apply"
        );
        let texts = panel(&ctx, &mut app, vec![]);
        find(&texts, "Preview");
        assert!(
            texts.iter().any(|(t, _)| t.contains("exports keep")),
            "{texts:?}"
        );
        let esc = |pressed| egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        panel(&ctx, &mut app, vec![esc(true), esc(false)]);
        assert!(app.style_panel.trying.is_none());
        // Trying again, then Apply in the footer.
        let texts = panel(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Test wipe").center());
        let texts = panel(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Apply").center());
        assert_eq!(app.project.effect.as_ref().unwrap().id, "test.wipe");
    }

    /// A loaded two-line track, as the reader publishes it.
    fn attach_track(app: &mut super::super::NohApp, folder: &std::path::Path) {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nla la la\n\n2\n00:00:03,000 --> 00:00:04,000\nlo lo\n\n";
        let path = folder.join("song.srt");
        std::fs::write(&path, srt).unwrap();
        let track = noh::subtitle_srt::parse(srt, 60_000).unwrap();
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
    }

    #[test]
    fn the_switch_and_classique_settings_live_in_the_panel_and_invalidate() {
        let folder = tempfile::tempdir().unwrap();
        let (ctx, mut app) = app(noh::i18n::Language::En);
        attach_track(&mut app, folder.path());
        app.project.apply_subtitles = false;
        let texts = panel(&ctx, &mut app, vec![]);
        let switch = find(&texts, "Captions on the video");
        let revision = app.revision;
        click(&ctx, &mut app, switch.left_center() - vec2(20.0, 0.0));
        assert!(app.project.apply_subtitles);
        assert_ne!(app.revision, revision, "showing the lyrics invalidates");
        let texts = panel(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Settings").center());
        assert!(app.style_panel.settings);
        let texts = panel(&ctx, &mut app, vec![]);
        let revision = app.revision;
        click(&ctx, &mut app, find(&texts, "Large").center());
        assert_eq!(
            app.project.caption_style.size,
            noh::captions::CaptionSize::Large
        );
        assert_ne!(app.revision, revision, "restyling shown lyrics invalidates");
        assert_eq!(
            app.styled(app.project.effect.as_ref().unwrap()).values["size"],
            "large"
        );
        // Hiding them invalidates too, and ends any try-on.
        app.style_panel.trying = Some(card(&mut app, "test.wipe"));
        let texts = panel(&ctx, &mut app, vec![]);
        let revision = app.revision;
        click(
            &ctx,
            &mut app,
            find(&texts, "Captions on the video").left_center() - vec2(20.0, 0.0),
        );
        assert!(!app.project.apply_subtitles && app.style_panel.trying.is_none());
        assert_ne!(app.revision, revision);
    }

    #[test]
    fn without_lyrics_the_switch_waits_and_the_styles_stay_choosable() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        app.project.apply_subtitles = false;
        let texts = panel(&ctx, &mut app, vec![]);
        find(
            &texts,
            "The style will apply to the lyrics as soon as they are there.",
        );
        click(
            &ctx,
            &mut app,
            find(&texts, "Captions on the video").left_center() - vec2(20.0, 0.0),
        );
        assert!(
            !app.project.apply_subtitles,
            "no switch without lyrics: exports would wait for a track"
        );
        let texts = panel(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Test wipe").center());
        assert_eq!(app.style_panel.trying.as_ref().unwrap().id, "test.wipe");
        let texts = panel(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, find(&texts, "Apply").center());
        assert_eq!(app.project.effect.as_ref().unwrap().id, "test.wipe");
    }

    #[test]
    fn a_try_on_is_not_offered_as_shown_while_the_applied_preview_has_failed() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        app.project.effect_preview_failed = true;
        app.style_panel.trying = Some(card(&mut app, "test.glow"));
        let monitor = Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.paint_effect(ui, monitor, false);
            app.paint_try_tag(ui, monitor);
        });
        out.textures_delta.clear();
        assert!(app.style_panel.try_failed.is_some());
        let tagged = out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().starts_with("Preview of")));
        assert!(
            !tagged,
            "no « not applied yet » tag over captions the try cannot change"
        );
        let texts = panel(&ctx, &mut app, vec![]);
        assert!(
            texts
                .iter()
                .any(|(t, _)| t.starts_with("The preview cannot draw")),
            "{texts:?}"
        );
    }

    #[test]
    fn a_failed_try_on_leaves_the_applied_effect_untouched() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        app.style_panel.trying = Some(card(&mut app, "test.glow"));
        app.effects.fail_for_test("no GL");
        let monitor = Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0));
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
            ..Default::default()
        };
        ctx.run_ui(input(), |ui| app.paint_effect(ui, monitor, false))
            .textures_delta
            .clear();
        assert!(app.style_panel.try_failed.is_some());
        assert!(
            !app.project.effect_preview_failed,
            "the applied effect did not fail"
        );
        assert!(app.effects.preview_failure().is_none());
        assert!(
            app.shown_effect().unwrap().is_classic(),
            "the monitor goes back to the applied style"
        );
        let texts = panel(&ctx, &mut app, vec![]);
        assert!(
            texts
                .iter()
                .any(|(t, _)| t.starts_with("The preview cannot draw")),
            "{texts:?}"
        );
        click(&ctx, &mut app, find(&texts, "Cancel").center());
        assert!(app.style_panel.trying.is_none() && app.style_panel.try_failed.is_none());
        assert!(
            !app.project.effect_preview_failed && app.project.effect.as_ref().unwrap().is_classic()
        );
    }

    #[test]
    fn size_sliders_never_leave_the_parameter_bounds() {
        let catalog = noh::style::catalog::scan(&[
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("effects"),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("fx"),
        ]);
        let mut pairs = 0;
        for entry in &catalog.effects {
            for p in entry.effect.params.iter().filter(|p| p.kind == "number") {
                let values = std::iter::once(p.default.clone()).chain(
                    entry
                        .effect
                        .presets
                        .iter()
                        .filter_map(|(_, v)| v.get(&p.id).cloned()),
                );
                for value in values {
                    let (lo, hi, step) = slider_range(p, &value);
                    // What egui sets: clamp to the range, then round to the step from its start.
                    let egui = |v: f64| lo + ((v.clamp(lo, hi) - lo) / step).round() * step;
                    for typed in [
                        lo,
                        hi,
                        (lo + hi) / 2.0,
                        hi + step * 0.49,
                        f64::MAX,
                        f64::MIN,
                    ] {
                        let n = egui(typed);
                        assert!(
                            n <= hi + 1e-9 && n >= lo - 1e-9,
                            "{} {} slider at {n}",
                            entry.effect.id,
                            p.id
                        );
                        let v = with_number(&value, n, p);
                        assert!(p.accepts(&v), "{} {} at {n}: {v}", entry.effect.id, p.id);
                    }
                    pairs += usize::from(value.is_object());
                }
            }
        }
        assert!(pairs >= 4, "{pairs}");
    }

    #[test]
    fn effect_settings_and_presets_change_the_applied_choice() {
        let (ctx, mut app) = app(noh::i18n::Language::En);
        app.project.apply_subtitles = true;
        let neon = card(&mut app, "test.glow");
        app.choose_effect(Some(neon));
        app.style_panel.settings = true;
        let texts = panel(&ctx, &mut app, vec![]);
        find(&texts, "Glow of the sung word");
        // Level 2 is behind "More settings (2)", named in every language.
        click(&ctx, &mut app, find(&texts, "More settings (2)").center());
        let texts = panel(&ctx, &mut app, vec![]);
        find(&texts, "Glow of words already sung");
        click(&ctx, &mut app, find(&texts, "Ice").center());
        let applied = app.project.effect.clone().unwrap();
        assert_eq!(applied.preset, "glace");
        // Values set in the panel survive the caption-style pass used by exports.
        let mut tuned = applied.clone();
        tuned.values.insert("active_glow".into(), J::from(1.5));
        app.choose_effect(Some(tuned.clone()));
        assert_eq!(
            app.styled(app.project.effect.as_ref().unwrap()).values,
            tuned.values
        );
    }

    #[test]
    fn panel_texts_exist_in_every_language() {
        for language in noh::i18n::Language::ALL {
            for key in [
                "show",
                "tab_styles",
                "tab_settings",
                "used",
                "trying",
                "try_tag",
                "try_note",
                "apply",
                "cancel",
                "settings_of",
                "more",
                "less",
                "other_color",
                "one_style",
                "off",
                "no_lyrics",
                "settings_while_trying",
                "close",
            ] {
                assert!(
                    !language.text(&format!("style.{key}")).contains("missing"),
                    "{} style.{key}",
                    language.code()
                );
            }
        }
    }

    /// Needs a GPU (run with --ignored, as the other GPU checks).
    #[test]
    #[ignore]
    fn samples_draw_every_bundled_effect() {
        let catalog = noh::style::catalog::scan(&[
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("effects"),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("fx"),
        ]);
        let mut gpu = GlRenderer::headless(
            SAMPLE_W,
            SAMPLE_H,
            SAMPLE_W as f32,
            SAMPLE_H as f32,
            Output::PremulGamma,
        )
        .unwrap();
        let mut fonts = Fonts::load().unwrap();
        assert_eq!(catalog.effects.len(), 6);
        for entry in &catalog.effects {
            for (preset, _) in &entry.effect.presets {
                let job = SampleJob {
                    id: String::new(),
                    key: String::new(),
                    effect: entry.effect.clone(),
                    preset: preset.clone(),
                    values: Default::default(),
                };
                let img = render_sample(&mut gpu, &mut fonts, &job)
                    .unwrap_or_else(|e| panic!("{} {preset}: {e}", entry.effect.id));
                assert!(
                    img.size[0] > 60 && img.size[1] > 30,
                    "{} {preset}: {:?}",
                    entry.effect.id,
                    img.size
                );
            }
        }
    }

    #[test]
    fn crop_finds_the_ink_with_a_margin() {
        let (w, h) = (64usize, 48usize);
        let mut px = vec![0u8; w * h * 4];
        px[(20 * w + 30) * 4 + 3] = 255;
        let img = crop(&px, w, h).unwrap();
        assert_eq!(img.size, [33, 33]);
        assert!(crop(&vec![0u8; w * h * 4], w, h).is_none());
    }
}

/// The faces NOH ships that a font setting offers, after the plugin's own.
const BUNDLED_FACES: [&str; 7] = [
    "jost_extrabold",
    "jost_medium",
    "inter_semibold",
    "condensed_extrabold",
    "sans",
    "sans_semibold",
    "serif_italic",
];

/// A bundled face's name as its designers call it.
fn face_name(name: &str) -> &str {
    match name {
        "jost_extrabold" => "Jost ExtraBold",
        "jost_medium" => "Jost Medium",
        "inter_semibold" => "Inter SemiBold",
        "condensed_extrabold" => "Barlow Condensed ExtraBold",
        "sans" => "Noto Sans",
        "sans_semibold" => "Noto Sans SemiBold",
        "serif_italic" => "Noto Serif Italic",
        "cjk" => "Noto Sans CJK",
        other => other,
    }
}
