//! Caption effects in the app (level 0): the effect and preset chosen in the lyrics
//! menu, and the GPU caption layer drawn over the monitor, on the clock of the frame
//! shown. The plugins are read the first time the menu shows them, and the OpenGL
//! engine starts at the first frame drawn with an effect: nothing at app start.
use eframe::{egui, egui_glow, glow};
use noh::fx::{
    catalog::{self, Catalog},
    gl::GlRenderer,
    line::Line,
    plugin::{Effect, PluginScene, localized},
    text::{Fonts, InstalledFonts, families_of, system_fonts},
};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The effect chosen for the captions: which plugin and which of its presets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub preset: String,
    pub dir: PathBuf,
    /// Parameter values from the app's settings (Classique: size, placement).
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The effect the app starts with, which draws classic captions.
pub const CLASSIC: &str = "noh.classic";

impl Choice {
    pub fn request(&self) -> noh::project::EffectChoice {
        noh::project::EffectChoice {
            id: self.id.clone(),
            preset: Some(self.preset.clone()),
            dir: self.dir.clone(),
            values: self.values.clone(),
        }
    }

    pub fn is_classic(&self) -> bool {
        self.id == CLASSIC
    }

    /// Classique's parameters follow the caption Size, Position and line width settings; other
    /// effects keep the values set in the style panel.
    pub fn with_style(mut self, style: &noh::captions::CaptionStyle) -> Self {
        if self.is_classic() {
            self.values = noh::project::classic_values(style);
        }
        self
    }
}

/// What the layer must draw; a change rebuilds the scene (not the GPU engine).
#[derive(Clone, PartialEq)]
struct SceneKey {
    choice: Choice,
    track_revision: u64,
    range: Option<(u64, u64)>,
    canvas: (u32, u32),
    /// Safe area insets on `canvas`, as at export.
    safe: Option<[u32; 4]>,
}

struct Wanted {
    key: SceneKey,
    effect: Effect,
    lines: Vec<Line>,
}

/// The installed fonts, for font settings: read once per session, off the UI
/// thread, the first time a font setting needs them.
#[derive(Default)]
enum Installed {
    #[default]
    NotStarted,
    Reading(std::sync::mpsc::Receiver<(InstalledFonts, Vec<String>)>),
    /// Their family names, sorted (the picker's list), and the fonts themselves
    /// (the picker draws each name in its own face).
    Read(Vec<String>, Option<InstalledFonts>),
}

/// Capture mode reads a fixed folder as the installed fonts (NOH_CAPTURE_FONTS),
/// never the machine's.
fn installed_dirs() -> Option<Vec<PathBuf>> {
    std::env::var_os("NOH_CAPTURE_UI")?;
    std::env::var_os("NOH_CAPTURE_FONTS").map(|d| vec![PathBuf::from(d)])
}

#[derive(Default)]
pub struct State {
    catalog: Option<Catalog>,
    installed: Installed,
    /// Repainted when the installed fonts have been read.
    repaint: Option<egui::Context>,
    layer: Arc<Mutex<Layer>>,
    /// Capture-driven switches already applied (measurement only).
    switches_done: usize,
    /// The default effect was looked for once (the catalogue is read at most once).
    default_tried: bool,
}

#[derive(Default)]
struct Layer {
    wanted: Option<Wanted>,
    built: Option<SceneKey>,
    gpu: Option<GlRenderer>,
    scene: Option<PluginScene>,
    fonts: Option<Fonts>,
    /// Physical size waiting to settle before the engine is rebuilt at it.
    resize: Option<((u32, u32), Instant)>,
    /// A failure disables the effect in the preview (the export still reports it).
    failed: Option<String>,
    /// The built scene drew letters outside the safe area at least once.
    overflow: bool,
    /// Installed fonts read since the last frame, for the fonts and the scene.
    installed_new: Option<InstalledFonts>,
    /// The installed family the built scene asks for and this machine lacks.
    font_missing: Option<String>,
}

impl Layer {
    /// Installed fonts read meanwhile join the fonts; a scene laid out before
    /// them (drawn in the plugin's own face for now) is laid out again.
    fn take_installed(&mut self) {
        if let Some(db) = self.installed_new.take() {
            if let Some(fonts) = self.fonts.as_mut() {
                fonts.set_installed(db);
            } else {
                self.installed_new = Some(db);
                return;
            }
            if self.scene.as_ref().is_some_and(|s| s.font_unread()) {
                self.built = None;
            }
        }
    }
}

impl State {
    /// The plugins, if already read.
    pub fn catalog_ref(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }

    #[cfg(test)]
    pub fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = Some(catalog);
    }

    #[cfg(test)]
    pub fn fail_for_test(&self, message: &str) {
        self.layer.lock().unwrap().failed = Some(message.into());
    }

    /// The plugins, read once per session when first needed.
    pub fn catalog(&mut self) -> &Catalog {
        self.catalog
            .get_or_insert_with(|| catalog::scan(&catalog::default_roots()))
    }

    /// The preview can no longer draw the effect: libass takes over in the preview.
    pub fn preview_failure(&self) -> Option<String> {
        self.layer.lock().unwrap().failed.clone()
    }

    /// The effect drew letters outside the Short's safe area in this preview.
    pub fn overflow(&self) -> bool {
        self.layer.lock().unwrap().overflow
    }

    /// The installed family the previewed effect asks for and this machine lacks.
    pub fn font_missing(&self) -> Option<String> {
        self.layer.lock().unwrap().font_missing.clone()
    }

    /// The installed font families, for the picker; None while they are read
    /// (the read starts on the first call, off the UI thread).
    pub fn installed_families(&mut self, ctx: &egui::Context) -> Option<&[String]> {
        self.repaint.get_or_insert_with(|| ctx.clone());
        self.poll_installed();
        match &self.installed {
            Installed::Read(families, _) => Some(families),
            _ => None,
        }
    }

    /// The installed fonts still being read (their reader gone: it stays so).
    #[cfg(test)]
    pub fn set_installed_reading_for_test(&mut self) {
        let (_, rx) = std::sync::mpsc::channel();
        self.installed = Installed::Reading(rx);
    }

    #[cfg(test)]
    pub fn set_installed_for_test(&mut self, families: &[&str]) {
        self.installed = Installed::Read(families.iter().map(|f| f.to_string()).collect(), None);
    }

    fn poll_installed(&mut self) {
        match &self.installed {
            // Capture mode reads its fixed folder (two files) at once, so every
            // capture shows the same list.
            Installed::NotStarted if installed_dirs().is_some() => {
                let db = system_fonts(installed_dirs().as_deref());
                let families = families_of(&db);
                let db = Arc::new(db);
                self.layer.lock().unwrap().installed_new = Some(db.clone());
                self.installed = Installed::Read(families, Some(db));
            }
            Installed::NotStarted => {
                let (tx, rx) = std::sync::mpsc::channel();
                let repaint = self.repaint.clone();
                std::thread::spawn(move || {
                    let db = system_fonts(installed_dirs().as_deref());
                    let families = families_of(&db);
                    let _ = tx.send((Arc::new(db), families));
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                });
                self.installed = Installed::Reading(rx);
            }
            Installed::Reading(rx) => {
                if let Ok((db, families)) = rx.try_recv() {
                    self.layer.lock().unwrap().installed_new = Some(db.clone());
                    self.installed = Installed::Read(families, Some(db));
                }
            }
            Installed::Read(..) => {}
        }
    }

    /// The installed fonts once read (the picker draws their names with them).
    pub fn installed_fonts(&self) -> Option<InstalledFonts> {
        match &self.installed {
            Installed::Read(_, db) => db.clone(),
            _ => None,
        }
    }

    pub fn clear_failure(&self) {
        self.layer.lock().unwrap().failed = None;
    }

    /// Say what the layer must show; called every UI frame while an effect is on.
    pub fn want(
        &mut self,
        choice: &Choice,
        track: &noh::subtitle_track::SubtitleTrack,
        track_revision: u64,
        range: Option<(u64, u64)>,
        canvas: (u32, u32),
        safe: Option<[u32; 4]>,
    ) {
        // An installed font is chosen: read the installed fonts (once) for the
        // preview; until they land it draws the plugin's own face.
        if choice
            .values
            .values()
            .any(|v| v.as_str().is_some_and(|s| s.starts_with("system:")))
        {
            self.poll_installed();
        }
        let key = SceneKey {
            choice: choice.clone(),
            track_revision,
            range,
            canvas,
            safe,
        };
        let mut layer = self.layer.lock().unwrap();
        if layer.wanted.as_ref().is_some_and(|w| w.key == key) {
            return;
        }
        let effect = match self.catalog.as_ref().and_then(|c| c.find(&choice.id)) {
            Some(entry) if entry.dir == choice.dir => Ok(entry.effect.clone()),
            _ => catalog::load_one(&choice.dir, &choice.id),
        };
        let effect = match effect {
            Ok(e) => e,
            Err(e) => {
                layer.failed = Some(e);
                return;
            }
        };
        let trimmed = match range {
            Some((start, end)) => track.clone().trim(start, end).ok(),
            None => Some(track.clone()),
        };
        let lines = trimmed
            .map(|t| noh::fx::line::from_track(&t))
            .unwrap_or_default();
        layer.wanted = Some(Wanted { key, effect, lines });
    }

    /// Draw the layer over `rect` at `t` seconds on the caption clock.
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, t: f64) {
        let layer = self.layer.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let mut layer = layer.lock().unwrap();
                if layer.failed.is_none()
                    && let Err(e) = layer.draw(&info, painter, t)
                {
                    layer.failed = Some(e);
                    layer.gpu = None;
                    layer.scene = None;
                    layer.built = None;
                }
            })),
        });
    }

    /// Release the GPU engine (the effect was turned off).
    pub fn release(&self) {
        let mut layer = self.layer.lock().unwrap();
        layer.scene = None;
        layer.built = None;
        layer.wanted = None;
        // Dropping the renderer deletes its GL objects: only with egui's context
        // current, which it is on the UI thread between frames.
        layer.gpu = None;
    }

    /// Choice lists for the menu: (label, choice) per effect and preset.
    pub fn options(&mut self, lang: &str) -> Vec<(String, Choice)> {
        let mut out = Vec::new();
        for entry in &self.catalog().effects {
            let e = &entry.effect;
            let name = localized(&e.names, lang, &e.id);
            for ((preset, _), names) in e.presets.iter().zip(&e.preset_names) {
                let label = if e.presets.len() == 1 {
                    name.to_string()
                } else {
                    format!("{name} · {}", localized(names, lang, preset))
                };
                out.push((
                    label,
                    Choice {
                        id: e.id.clone(),
                        preset: preset.clone(),
                        dir: entry.dir.clone(),
                        values: Default::default(),
                    },
                ));
            }
        }
        // Classique first: it is the default, the classic captions.
        out.sort_by_key(|(_, c)| !c.is_classic());
        out
    }

    /// Classique from the catalogue, if installed.
    pub fn classic(&mut self) -> Option<Choice> {
        let entry = self.catalog().find(CLASSIC)?;
        Some(Choice {
            id: CLASSIC.into(),
            preset: entry.effect.presets.first()?.0.clone(),
            dir: entry.dir.clone(),
            values: Default::default(),
        })
    }

    /// Plugins refused when the folders were read, with their messages.
    pub fn rejected(&mut self) -> Vec<String> {
        self.catalog()
            .rejected
            .iter()
            .map(|r| format!("{}: {}", r.dir.display(), r.message))
            .collect()
    }
}

impl Layer {
    fn draw(
        &mut self,
        info: &egui::PaintCallbackInfo,
        painter: &egui_glow::Painter,
        t: f64,
    ) -> Result<(), String> {
        if self.wanted.is_some() && self.fonts.is_none() {
            self.fonts = Some(Fonts::load()?);
        }
        self.take_installed();
        let Some(wanted) = self.wanted.as_ref() else {
            return Ok(());
        };
        let viewport = info.viewport_in_pixels();
        if viewport.width_px <= 0 || viewport.height_px <= 0 {
            return Ok(());
        }
        let size = (viewport.width_px as u32, viewport.height_px as u32);
        let canvas = wanted.key.canvas;
        // A window being resized changes the size every frame: rebuild the engine
        // only once the size has held for 250 ms, and scale the old layer meanwhile.
        let rebuild = match &self.gpu {
            None => true,
            Some(gpu) if (gpu.width, gpu.height) != size => {
                let since = match self.resize {
                    Some((s, at)) if s == size => at,
                    _ => {
                        self.resize = Some((size, Instant::now()));
                        Instant::now()
                    }
                };
                since.elapsed() >= Duration::from_millis(250)
            }
            Some(_) => false,
        };
        if rebuild {
            self.scene = None;
            self.built = None;
            self.gpu = None;
            self.resize = None;
            let mut gpu = GlRenderer::on_egui(
                painter.gl().clone(),
                size.0,
                size.1,
                canvas.0 as f32,
                canvas.1 as f32,
            )?;
            if let Some(src) = wanted.effect.shader.as_deref() {
                gpu.set_hooks(Some(src), [0.0; 16])?;
            }
            self.gpu = Some(gpu);
        }
        if self.built.as_ref() != Some(&wanted.key) {
            let gpu = self.gpu.as_mut().unwrap();
            let shader_changed = self
                .scene
                .as_ref()
                .is_none_or(|s| s.effect.shader != wanted.effect.shader);
            if shader_changed {
                gpu.set_hooks(wanted.effect.shader.as_deref(), [0.0; 16])?;
            }
            let px_scale = gpu.width as f32 / canvas.0 as f32;
            let scene = scene_for(wanted, px_scale)?;
            // The settings shader hooks read change with the choice, not the shader.
            gpu.set_user(scene.hook_user());
            self.scene = Some(scene);
            self.built = Some(wanted.key.clone());
            self.overflow = false;
        }
        let started = Instant::now();
        let (gpu, scene, fonts) = (
            self.gpu.as_mut().unwrap(),
            self.scene.as_mut().unwrap(),
            self.fonts.as_mut().unwrap(),
        );
        let (glow_i, gain, main, _) = scene.frame(fonts, &*gpu, t);
        self.overflow |= scene.overflowed();
        self.font_missing = scene.font_missing().map(str::to_string);
        gpu.frame(&glow_i, gain, &main, false)?;
        let gl = painter.gl();
        unsafe {
            use glow::HasContext;
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
            gl.viewport(
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            );
            let clip = info.clip_rect_in_pixels();
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
        }
        gpu.present();
        let cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        let gpu_ms = gpu.poll_gpu_ms();
        let key = &wanted.key.choice;
        trace(
            "layer",
            t,
            Some(cpu_ms),
            gpu_ms,
            &format!("{}/{} {}x{}", key.id, key.preset, size.0, size.1),
        );
        Ok(())
    }
}

/// The preview's scene for what is wanted: the same preset, values (Classique's
/// Size and Position) and safe area as the export, at the layer's pixel scale.
fn scene_for(wanted: &Wanted, px_scale: f32) -> Result<PluginScene, String> {
    let (w, h) = wanted.key.canvas;
    Ok(PluginScene::new(
        wanted.effect.clone(),
        w,
        h,
        wanted.lines.clone(),
        Some(&wanted.key.choice.preset),
    )
    .with_values(&wanted.key.choice.values)?
    .with_safe_insets(wanted.key.safe)
    .with_px_scale(px_scale))
}

/// `NOH_FX_TRACE=<file.csv>` (measurement only): one row per event, process-wide.
/// `layer`: an effect frame drawn; `video`: a new player frame shown (with or
/// without an effect); `switch`: a capture-driven effect change.
struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    started: Instant,
}
static TRACE: std::sync::OnceLock<Option<Mutex<Trace>>> = std::sync::OnceLock::new();

pub(super) fn trace(event: &str, t: f64, cpu_ms: Option<f64>, gpu_ms: Option<f64>, key: &str) {
    let Some(trace) = TRACE.get_or_init(|| {
        let path = std::env::var_os("NOH_FX_TRACE")?;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
        writeln!(out, "wall_s,event,t,cpu_ms,gpu_ms,key").ok()?;
        Some(Mutex::new(Trace {
            out,
            started: Instant::now(),
        }))
    }) else {
        return;
    };
    let mut tr = trace.lock().unwrap();
    let wall = tr.started.elapsed().as_secs_f64();
    let f = |v: Option<f64>| v.map(|v| format!("{v:.3}")).unwrap_or_default();
    let _ = writeln!(
        tr.out,
        "{wall:.4},{event},{t:.4},{},{},{key}",
        f(cpu_ms),
        f(gpu_ms)
    );
    let _ = tr.out.flush();
}

impl super::NohApp {
    /// The effect for the preview's requests (player, scrub, preview render). None
    /// once the monitor failed to draw it: libass then shows the captions there.
    pub(super) fn preview_effect(&self) -> Option<noh::project::EffectChoice> {
        self.project
            .effect
            .as_ref()
            .filter(|_| self.project.apply_subtitles && !self.project.effect_preview_failed)
            .map(|c| self.styled(c).request())
    }

    /// The choice with the values the current caption settings give it.
    pub(super) fn styled(&self, choice: &Choice) -> Choice {
        choice.clone().with_style(&self.project.caption_style)
    }

    /// The app starts with Classique (classic captions as an effect). Without it
    /// installed, no effect is chosen and libass draws the captions, as before.
    pub(super) fn ensure_default_effect(&mut self) {
        if self.project.effect.is_none()
            && !self.effects.default_tried
            && let Some(classic) = self.effects.classic()
        {
            self.project.effect = Some(classic);
        }
        self.effects.default_tried = true;
    }

    /// Draw the effect over the monitor, on the clock of the frame it shows.
    pub(super) fn paint_effect(&mut self, ui: &egui::Ui, monitor: egui::Rect, scrub_visible: bool) {
        if !self.project.apply_subtitles {
            return;
        }
        // The tried style when there is one: the exports keep the applied one.
        let Some(choice) = self.shown_effect().map(|c| self.styled(c)) else {
            return;
        };
        // The preview already fell back to libass for the applied effect: a tried
        // style cannot be drawn either; the footer says so, with no tag.
        if self.project.effect_preview_failed
            && self.style_panel.trying.is_some()
            && self.style_panel.try_failed.is_none()
        {
            self.style_panel.try_failed = Some("the preview draws classic captions".into());
        }
        if self.project.effect_preview_failed {
            // Never silent: the preview shows classic captions, the export the effect.
            let text = self.locale.language.text("effect.preview_failure");
            let at = monitor.left_bottom() + egui::vec2(8.0, -8.0);
            let galley = ui.painter().layout(
                text.into(),
                egui::FontId::proportional(super::app_style::text::SMALL),
                egui::Color32::WHITE,
                (monitor.width() - 16.0).max(40.0),
            );
            let rect =
                egui::Rect::from_min_size(at - egui::vec2(0.0, galley.size().y), galley.size())
                    .expand(4.0);
            ui.painter()
                .rect_filled(rect, 4.0, egui::Color32::from_black_alpha(200));
            ui.painter().galley(
                rect.min + egui::vec2(4.0, 4.0),
                galley,
                egui::Color32::WHITE,
            );
            let detail = self.effects.preview_failure().unwrap_or_default();
            ui.interact(rect, ui.id().with("effect-failure"), egui::Sense::hover())
                .on_hover_text(detail);
            return;
        }
        // A tried style that cannot be drawn stays the try-on's failure: the
        // applied effect keeps its own state, and the monitor goes back to it.
        if self.style_panel.trying.is_some()
            && self.style_panel.try_failed.is_none()
            && let Some(detail) = self.effects.preview_failure()
        {
            self.style_panel.try_failed = Some(detail);
            self.effects.clear_failure();
            self.effects.release();
            return;
        }
        if self.effects.preview_failure().is_some() {
            // Libass takes over in the preview; the stamp change restarts it.
            self.project.effect_preview_failed = true;
            self.effects.release();
            self.mini_preview.invalidate();
            return;
        }
        let Some(track) = self.lyrics_track() else {
            return;
        };
        let (start, end) = self.mini_bounds();
        let range = self.mini_preview.short.then_some((start, end));
        // The clock of the picture on the monitor: the player's frame, also
        // while it is the one a stopped player left there.
        let t =
            if self.mini_preview.player_stamp.is_some() || self.mini_preview.shows_left_picture() {
                self.mini_preview.player_position
            } else if scrub_visible && let Some(scrubber) = &self.mini_preview.scrubber {
                scrubber.snapshot().time_ms.saturating_sub(start) as f64 / 1000.0
            } else {
                self.mini_preview.cursor_ms.saturating_sub(start) as f64 / 1000.0
            };
        let Some(canvas) = self.effect_canvas() else {
            return;
        };
        // The same safe area libass would use for these captions, on the layer canvas.
        let safe = self
            .project
            .effective_caption_style(self.mini_preview.short)
            .safe_area
            .insets(canvas.0, canvas.1);
        self.effects.want(
            &choice,
            &track,
            self.project
                .track
                .revision
                .wrapping_add(self.project.edit_revision),
            range,
            canvas,
            safe,
        );
        self.effects.paint(ui, monitor, t);
    }

    /// Apply a choice from the menu. Between two effects only the layer changes: no
    /// new preview, no restart. To or from "none", the preview restarts (libass).
    pub(super) fn choose_effect(&mut self, choice: Option<Choice>) {
        if choice == self.project.effect {
            return;
        }
        let switched = choice.is_some() != self.project.effect.is_some();
        if choice.is_none() {
            self.effects.release();
        }
        self.project.effect = choice;
        self.project.effect_preview_failed = false;
        self.effects.clear_failure();
        if switched {
            self.invalidated();
        } else {
            // Effect to effect keeps the preview playing (same revision), but the
            // last exports were made with the previous effect: no longer current.
            if let Some(a) = &mut self.export_artifact {
                a.revision = a.revision.wrapping_sub(1);
            }
            for a in [
                &mut self.project.full_export_artifact,
                &mut self.project.short_export_artifact,
            ]
            .into_iter()
            .flatten()
            {
                a.stamp.revision = a.stamp.revision.wrapping_sub(1);
            }
        }
    }

    /// Capture only: `NOH_CAPTURE_EFFECT_SWITCH="5=noh.classic/classique;10=none"`
    /// changes the effect that many seconds after the capture playback started,
    /// through the menu's path, and traces the change.
    pub(super) fn capture_effect_switch(&mut self) {
        if std::env::var_os("NOH_CAPTURE_UI").is_none() {
            return;
        }
        let Ok(spec) = std::env::var("NOH_CAPTURE_EFFECT_SWITCH") else {
            return;
        };
        let Some(started) = self.mini_preview.capture_started else {
            return;
        };
        let elapsed = started.elapsed().as_secs_f64();
        let steps: Vec<(f64, String)> = spec
            .split(';')
            .filter_map(|s| s.split_once('='))
            .filter_map(|(a, b)| Some((a.trim().parse().ok()?, b.trim().to_string())))
            .collect();
        let done = self.effects.switches_done;
        let Some((at, target)) = steps.get(done) else {
            return;
        };
        if elapsed < *at {
            return;
        }
        self.effects.switches_done += 1;
        let choice = if target == "none" {
            None
        } else {
            let (id, preset) = target.split_once('/').unwrap_or((target, ""));
            self.effects
                .options("en")
                .into_iter()
                .map(|(_, c)| c)
                .find(|c| c.id == id && (preset.is_empty() || c.preset == preset))
        };
        trace("switch", elapsed, None, None, target);
        self.choose_effect(choice);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NohApp, app_media::Clip, app_state};
    use super::Choice;

    /// Plan D: installed fonts read after a scene was laid out with an installed
    /// family (drawn in the plugin's own face meanwhile) make the layer lay it
    /// out again; a scene that did not wait for them is kept.
    #[test]
    fn installed_fonts_landing_rebuild_a_scene_that_waited_for_them() {
        use noh::fx::text::{Fonts, system_fonts};
        let effects = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects");
        let catalog = noh::fx::catalog::scan(&[effects.clone()]);
        let drop = catalog
            .effects
            .iter()
            .find(|e| e.effect.id == "noh.drop")
            .unwrap()
            .effect
            .clone();
        let line = noh::fx::line::split_words(1.0, 3.0, &["la".to_string()], " ");
        let choice = Choice {
            id: "noh.drop".into(),
            preset: "white".into(),
            dir: effects.join("drop"),
            values: Default::default(),
        };
        let key = super::SceneKey {
            choice,
            track_revision: 0,
            range: None,
            canvas: (1920, 1080),
            safe: None,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/BarlowCondensed-ExtraBold.ttf"),
            dir.path().join("BarlowCondensed-ExtraBold.ttf"),
        )
        .unwrap();
        for (value, waited) in [("system:Barlow Condensed", true), ("jost_extrabold", false)] {
            let values = [("font".to_string(), serde_json::json!(value))]
                .into_iter()
                .collect();
            let mut scene = noh::fx::plugin::PluginScene::new(
                drop.clone(),
                1920,
                1080,
                vec![line.clone()],
                Some("white"),
            )
            .with_values(&values)
            .unwrap();
            let mut fonts = Fonts::load().unwrap();
            scene.frame(&mut fonts, &NoUpload, 2.0);
            assert_eq!(scene.font_unread(), waited, "{value}");
            let mut layer = super::Layer {
                built: Some(key.clone()),
                scene: Some(scene),
                fonts: Some(fonts),
                installed_new: Some(std::sync::Arc::new(system_fonts(Some(&[dir
                    .path()
                    .to_path_buf()])))),
                ..Default::default()
            };
            layer.take_installed();
            assert!(
                layer.installed_new.is_none(),
                "{value}: handed to the fonts"
            );
            assert_eq!(layer.built.is_none(), waited, "{value}");
        }
        // Fonts not loaded yet: kept for them.
        let mut layer = super::Layer {
            installed_new: Some(std::sync::Arc::new(system_fonts(Some(&[dir
                .path()
                .to_path_buf()])))),
            ..Default::default()
        };
        layer.take_installed();
        assert!(layer.installed_new.is_some());
    }

    struct NoUpload;
    impl noh::fx::shaders::Upload for NoUpload {
        fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    }

    /// The bundled effects and their presets are named in every app language,
    /// and the menu's own words are translated (no French/English-only text).
    #[test]
    fn bundled_effect_menu_is_named_in_every_language() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("effects");
        let mut effects = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            effects.push(entry.file_name().to_string_lossy().into_owned());
            let path = entry.path().join("effect.json");
            let json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let mut names = vec![&json["name"]];
            names.extend(
                json["presets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| &p["name"]),
            );
            // Parameter labels, and the names of their choices where given.
            for param in json["params"].as_array().into_iter().flatten() {
                names.push(&param["label"]);
                if let Some(labels) = param["choice_labels"].as_object() {
                    names.extend(labels.values());
                }
            }
            for language in noh::i18n::Language::ALL {
                for name in &names {
                    assert!(
                        name[language.code()]
                            .as_str()
                            .is_some_and(|s| !s.is_empty()),
                        "{} lacks {} in {name}",
                        path.display(),
                        language.code()
                    );
                }
                for key in ["effect.none", "effect.preview_failure"] {
                    assert!(!language.text(key).contains("missing"), "{key}");
                }
                assert!(language.plural("effect.rejected", 2).contains('2'));
            }
        }
        // Every bundled folder was checked above, whatever styles it holds.
        assert!(
            effects.iter().any(|e| e == "classique"),
            "NOH ships Classique: {effects:?}"
        );
    }

    fn clip(width: f64, height: f64) -> Clip {
        // Display size is crate-private: set it through the worker's own format.
        let mut value = serde_json::to_value(noh::media::MediaInfo::default()).unwrap();
        value["display_width"] = width.into();
        value["display_height"] = height.into();
        value["width"] = (width.round() as u32).into();
        value["height"] = (height.round() as u32).into();
        let info: noh::media::MediaInfo = serde_json::from_value(value).unwrap();
        Clip {
            id: 1,
            request_id: 1,
            item: "clip.mp4".into(),
            info: Some(info),
            error: None,
        }
    }

    fn choice(id: &str, preset: &str) -> Choice {
        Choice {
            id: id.into(),
            preset: preset.into(),
            dir: "effects".into(),
            values: Default::default(),
        }
    }

    /// Classique, plus the test-only effects (tests/fixtures/fx).
    fn bundled_catalog() -> noh::fx::catalog::Catalog {
        noh::fx::catalog::scan(&[
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("effects"),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("fx"),
        ])
    }

    /// The preview scene takes the values (Size, Position) and the safe area, as
    /// the export does: Classique at Top is anchored at the top in the preview too.
    #[test]
    fn the_preview_scene_takes_values_and_safe_area() {
        let mut app = NohApp::default();
        app.effects.catalog = Some(bundled_catalog());
        app.ensure_default_effect();
        app.project.caption_style.placement = noh::captions::CaptionPlacement::Top;
        let choice = app.styled(app.project.effect.as_ref().unwrap());
        let entry = app.effects.catalog().find(super::CLASSIC).unwrap();
        let wanted = super::Wanted {
            key: super::SceneKey {
                choice: choice.clone(),
                track_revision: 0,
                range: None,
                canvas: (1080, 1920),
                safe: noh::safe_area::SafeArea::YoutubeShorts.insets(1080, 1920),
            },
            effect: entry.effect.clone(),
            lines: Vec::new(),
        };
        let scene = super::scene_for(&wanted, 1.0).unwrap();
        assert_eq!(scene.safe_rect(), [48.0, 288.0, 840.0, 960.0]);
        let anchor = scene
            .eval_value(
                &serde_json::json!({"choose": ["$placement", {"top": "top", "bottom": "bottom"}]}),
            )
            .unwrap();
        assert!(
            matches!(anchor, noh::fx::plugin::R::Text(ref t) if t == "top"),
            "{anchor:?}"
        );
        // An invalid value is refused, not ignored.
        let mut bad = wanted;
        bad.key.choice.values.insert("size".into(), "huge".into());
        assert!(super::scene_for(&bad, 1.0).is_err());
    }

    #[test]
    fn the_app_starts_with_classique_listed_first() {
        let mut app = NohApp::default();
        app.effects.catalog = Some(bundled_catalog());
        assert!(app.project.effect.is_none());
        app.ensure_default_effect();
        let chosen = app.project.effect.clone().expect("Classique chosen");
        assert!(chosen.is_classic());
        assert_eq!(chosen.preset, "classique");
        let options = app.effects.options("fr");
        assert!(options[0].1.is_classic(), "{:?}", options[0].0);
        assert_eq!(options.iter().filter(|(_, c)| c.is_classic()).count(), 1);
        // Without Classique installed nothing is chosen (libass draws), and it is
        // looked for once only.
        let mut bare = NohApp::default();
        bare.effects.catalog = Some(noh::fx::catalog::scan(&[]));
        bare.ensure_default_effect();
        assert!(bare.project.effect.is_none());
        bare.effects.catalog = Some(bundled_catalog());
        bare.ensure_default_effect();
        assert!(bare.project.effect.is_none());
    }

    #[test]
    fn classique_values_follow_the_caption_settings_and_reach_the_request() {
        use noh::captions::{CaptionPlacement, CaptionSize};
        let mut app = NohApp::default();
        app.effects.catalog = Some(bundled_catalog());
        app.ensure_default_effect();
        app.project.apply_subtitles = true;
        app.project.caption_style.size = CaptionSize::Small;
        app.project.caption_style.placement = CaptionPlacement::Top;
        let request = app.styled(app.project.effect.as_ref().unwrap()).request();
        assert_eq!(
            request.values.get("size"),
            Some(&serde_json::json!("small"))
        );
        assert_eq!(
            request.values.get("placement"),
            Some(&serde_json::json!("top"))
        );
        // Another effect carries no values: its preset decides.
        let other = app.styled(&choice("test.wipe", "rose")).request();
        assert!(other.values.is_empty());
        // The request keeps its old JSON form without values, and round-trips with them.
        let plain = serde_json::to_value(&other).unwrap();
        assert!(plain.get("values").is_none());
        let back: noh::project::EffectChoice =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        assert_eq!(back.values, request.values);
    }

    #[test]
    fn the_layer_canvas_needs_no_analysis() {
        let mut app = NohApp::default();
        assert_eq!(app.effect_canvas(), None, "no clip read yet");
        // A read without the picture size gives no canvas, not the 2x2 minimum.
        app.clips.push(clip(0.0, 0.0));
        assert_eq!(app.effect_canvas(), None, "picture size unknown");
        app.clips.clear();
        app.clips.push(clip(1915.0, 1080.0));
        // The export's normalized target: display size rounded up to even.
        assert_eq!(app.effect_canvas(), Some((1916, 1080)));
        assert!(app.diagnosis.is_none());
        app.mini_preview.short = true;
        assert_eq!(app.effect_canvas(), Some((1080, 1920)));
    }

    #[test]
    fn effect_to_effect_keeps_the_revision_but_dates_the_last_export() {
        let mut app = NohApp::default();
        app.project.apply_subtitles = true;
        app.choose_effect(Some(choice("test.glow", "glace")));
        let revision = app.revision;
        app.export_artifact = Some(app_state::Artifact {
            path: "video.mp4".into(),
            revision,
            bytes: None,
        });
        app.choose_effect(Some(choice("test.wipe", "rose")));
        assert_eq!(app.revision, revision, "the preview is not restarted");
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        // To or from "none" is a full settings change, as before.
        app.choose_effect(None);
        assert_ne!(app.revision, revision);
    }
}
