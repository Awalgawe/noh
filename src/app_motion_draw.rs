//! The window's half of a used scene (motion overlay plan § 5.2, M4). Once the
//! worker has passed the scene, the window prepares it on its own script
//! thread (the song's sound analysed, the scene validated over the song at the
//! monitor's frame: every text it will draw is then known) and draws it over
//! the monitor at the export's size, on the song's clock, through the export's
//! engine on egui's context. Until it is ready, the style stays on screen.
//!
//! Every frame drawn is checked as the export checks it: a frame over a cap is
//! shown without the scene, and said. Shader layers are left out unless the
//! user asks for them (plan § 3.4, Q2 (c): a driver reset in the window could
//! not be seen). A preview that cannot keep up draws the scene on fewer frames
//! and says so; the export stays exact (plan § 6.1).
//!
//! The scene's overlay (`MotionLayer`) is not `Send`: it lives in a slot of the
//! UI thread, where egui runs both the update and its paint callbacks; the
//! callback only carries the instant and which scene it is.
use super::NohApp;
use eframe::egui;
use noh::safe_area::SafeArea;
use std::{path::PathBuf, sync::mpsc};

/// Which scene, for which frame: a change prepares it again (a new painter).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Key {
    pub path: PathBuf,
    pub stamp: super::app_motion::Stamp,
    pub track: u64,
    pub canvas: (u32, u32),
    pub safe: SafeArea,
    pub seconds_bits: u64,
    pub fps_bits: u64,
}

/// What the monitor says about the scene, under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Feedback {
    /// The last frame over a cap: its instant and the reason.
    pub refused: Option<(f64, String)>,
    /// The preview cannot draw the scene at all (the export still can).
    pub failed: Option<String>,
    /// Shader layers left out of the last frame.
    pub hidden: usize,
    /// The scene is drawn on fewer frames than the picture.
    pub slow: bool,
    /// The last frame reframes the picture (`video()`): the preview does not
    /// yet, the export does.
    pub reframes: bool,
}

enum Stage {
    Preparing(mpsc::Receiver<Result<engine::Prepared, String>>),
    Ready,
    Failed(String),
}

#[derive(Default)]
pub(super) struct Preview {
    at: Option<(Key, Stage)>,
    feedback: Feedback,
}

impl Preview {
    /// The scene is no longer used: its thread and its GL objects go (on the UI
    /// thread, with egui's context current between frames).
    pub fn release(&mut self) {
        self.at = None;
        self.feedback = Feedback::default();
        engine::release();
    }

    pub fn hidden_shaders(&self) -> usize {
        self.feedback.hidden
    }

    /// The window draws the scene (prepared, and not given up).
    pub fn drawing(&self) -> bool {
        matches!(&self.at, Some((_, Stage::Ready))) && self.feedback.failed.is_none()
    }

    fn ready(&self, key: &Key) -> bool {
        matches!(&self.at, Some((k, Stage::Ready)) if k == key)
    }
}

impl NohApp {
    /// The key of the scene the monitor would draw now, once the worker passed it.
    fn motion_key(&self) -> Option<Key> {
        let f = self.motion.found.as_ref()?;
        let (inputs, _) = f.passed()?;
        if !self.motion.used || !self.project.apply_subtitles {
            return None;
        }
        Some(Key {
            path: f.path.clone(),
            stamp: inputs.stamp.clone(),
            track: inputs.track,
            canvas: self.effect_canvas()?,
            safe: self
                .project
                .effective_caption_style(self.mini_preview.short)
                .safe_area,
            seconds_bits: self.wav_seconds?.to_bits(),
            fps_bits: inputs.fps.to_bits(),
        })
    }

    /// Each frame: start or finish the window's preparation of the used scene.
    pub(super) fn motion_draw_frame(&mut self, ctx: &egui::Context) {
        let Some(key) = self.motion_key() else {
            if self.motion.draw.at.is_some() {
                self.motion.draw.release();
            }
            return;
        };
        self.motion.draw.feedback = engine::feedback();
        let current = self.motion.draw.at.as_ref().is_some_and(|(k, _)| *k == key);
        if !current {
            engine::release();
            self.motion.draw.feedback = Feedback::default();
            let (Some(wav), Some(track)) = (self.wav.clone(), self.lyrics_track()) else {
                return;
            };
            let ffmpeg = self.ffmpeg.clone().unwrap_or_default();
            let (tx, rx) = mpsc::sync_channel(1);
            let (k, repaint) = (key.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(engine::prepare(&k, &wav, &ffmpeg, &track));
                repaint.request_repaint();
            });
            self.motion.draw.at = Some((key, Stage::Preparing(rx)));
            return;
        }
        if let Some((key, Stage::Preparing(rx))) = &self.motion.draw.at {
            match rx.try_recv() {
                Ok(Ok(prepared)) => {
                    engine::hand_over(key.clone(), prepared);
                    let key = key.clone();
                    self.motion.draw.at = Some((key, Stage::Ready));
                    ctx.request_repaint();
                }
                Ok(Err(why)) => {
                    let key = key.clone();
                    self.motion.draw.at = Some((key, Stage::Failed(why)));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let key = key.clone();
                    self.motion.draw.at = Some((key, Stage::Failed("stopped".into())));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    /// Draw the used scene over the monitor at `t` seconds on its clock (the
    /// monitor's; a short's starts at its start). False while it is not ready:
    /// the style stays on screen meanwhile.
    pub(super) fn paint_scene(&mut self, ui: &egui::Ui, monitor: egui::Rect, t: f64) -> bool {
        let Some(key) = self.motion_key() else {
            return false;
        };
        let l = self.locale.language;
        let preparing = matches!(&self.motion.draw.at, Some((k, Stage::Preparing(_))) if *k == key);
        if let Some((k, Stage::Failed(why))) = &self.motion.draw.at
            && *k == key
            && why != engine::CHANGED
        {
            let why = why.clone();
            badge(ui, monitor, l.text("motion.preview_failed"), &why);
            return false;
        }
        if !self.motion.draw.ready(&key) {
            // A new version waits for its check: the style stays meanwhile.
            let changed = matches!(&self.motion.draw.at, Some((k, Stage::Failed(w))) if *k == key && w == engine::CHANGED);
            if preparing || changed {
                badge(ui, monitor, l.text("motion.preparing"), "");
            }
            return false;
        }
        let song_t = t + if self.mini_preview.short {
            self.mini_bounds().0 as f64 / 1000.0
        } else {
            0.0
        };
        engine::paint(ui, monitor, key, song_t, self.motion.show_shaders);
        let fb = self.motion.draw.feedback.clone();
        let note = if let Some(why) = &fb.failed {
            Some((l.text("motion.preview_failed").to_string(), why.clone()))
        } else if let Some((at, why)) = fb
            .refused
            .as_ref()
            .filter(|(at, _)| (*at - song_t).abs() < 0.5)
        {
            Some((l.format("motion.frame_refused", &[clock(*at)]), why.clone()))
        } else if fb.slow {
            Some((l.text("motion.slow").to_string(), String::new()))
        } else if fb.reframes {
            Some((l.text("motion.no_reframe").to_string(), String::new()))
        } else if fb.hidden > 0 && !self.motion.show_shaders {
            Some((l.text("motion.shaders_hidden").to_string(), String::new()))
        } else {
            None
        };
        if let Some((text, detail)) = note {
            badge(ui, monitor, &text, &detail);
        }
        fb.failed.is_none()
    }
}

fn clock(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).round() as u64;
    format!("{}:{:02},{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// A line over the bottom left of the monitor, never silent (as the effect's).
fn badge(ui: &egui::Ui, monitor: egui::Rect, text: &str, detail: &str) {
    let at = monitor.left_bottom() + egui::vec2(8.0, -8.0);
    let galley = ui.painter().layout(
        text.into(),
        egui::FontId::proportional(super::app_style::text::SMALL),
        egui::Color32::WHITE,
        (monitor.width() - 16.0).max(40.0),
    );
    let rect =
        egui::Rect::from_min_size(at - egui::vec2(0.0, galley.size().y), galley.size()).expand(4.0);
    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_black_alpha(200));
    ui.painter().galley(
        rect.min + egui::vec2(4.0, 4.0),
        galley,
        egui::Color32::WHITE,
    );
    if !detail.is_empty() {
        ui.interact(rect, ui.id().with("motion-badge"), egui::Sense::hover())
            .on_hover_text(detail.to_string());
    }
}

#[cfg(feature = "motion")]
mod engine {
    use super::{Feedback, Key};
    use eframe::{egui, egui_glow, glow};
    use noh::fx::{export::Overlay, gl::GlRenderer};
    use noh::motion::{layer::MotionLayer, thread::ScriptThread};
    use noh::subtitle_track::SubtitleTrack;
    use std::{cell::RefCell, collections::VecDeque, path::Path, sync::Arc, time::Instant};

    /// Why the window does not prepare a scene whose files moved since the
    /// worker checked them (the new version is checked first).
    pub const CHANGED: &str = "the scene changed since its check: it is checked again first";

    /// A scene validated by the window, ready to draw.
    pub struct Prepared {
        thread: ScriptThread,
        shaders: Vec<(String, String)>,
    }

    /// The song's sound analysed, the scene loaded and validated over the song
    /// at the monitor's frame: what the export does before its first frame.
    pub fn prepare(
        key: &Key,
        wav: &Path,
        ffmpeg: &Path,
        track: &SubtitleTrack,
    ) -> Result<Prepared, String> {
        use noh::motion::{layer, shader, song, validate};
        // Only the version the worker checked: the scene and its shaders as
        // they were then, before and after they are read here.
        let unchanged = || {
            if super::super::app_motion::stamp(&key.path) == key.stamp {
                Ok(())
            } else {
                Err(CHANGED.to_string())
            }
        };
        unchanged()?;
        let src = layer::read_scene(&key.path)?;
        let shaders = shader::read_beside(&key.path)?;
        unchanged()?;
        let seconds = f64::from_bits(key.seconds_bits);
        let words = track.cues.iter().filter(|c| c.words.is_some()).count();
        let gathered = song::gather(
            ffmpeg, wav, true, key.canvas, key.canvas, seconds, track, words, key.safe,
        )?;
        let mut inputs = gathered.inputs;
        inputs.shaders = shaders.iter().map(|(n, _)| n.clone()).collect();
        let thread = ScriptThread::start(src, inputs, gathered.audio, false)?;
        let fps =
            validate::known_fps(f64::from_bits(key.fps_bits)).unwrap_or(validate::FALLBACK_FPS);
        thread.validate(validate::grid(0.0, seconds, fps))?;
        Ok(Prepared { thread, shaders })
    }

    /// Frames timed to decide whether the preview keeps up.
    const WINDOW: usize = 48;
    /// Frames in a row over a cap before the preview gives the scene up.
    const GIVE_UP: usize = 96;

    struct Drawn {
        key: Key,
        pending: Option<Prepared>,
        gpu: Option<GlRenderer>,
        layer: Option<MotionLayer>,
        /// The instant of the overlay last drawn (none: nothing to show again).
        last: Option<f64>,
        /// Draw times (ms) of the last frames, and every other frame skipped.
        times: VecDeque<f64>,
        skip: bool,
        refused_run: usize,
        feedback: Feedback,
    }

    thread_local! {
        static DRAWN: RefCell<Option<Drawn>> = const { RefCell::new(None) };
    }

    pub fn hand_over(key: Key, prepared: Prepared) {
        DRAWN.with(|d| {
            *d.borrow_mut() = Some(Drawn {
                key,
                pending: Some(prepared),
                gpu: None,
                layer: None,
                last: None,
                times: VecDeque::new(),
                skip: false,
                refused_run: 0,
                feedback: Feedback::default(),
            })
        });
    }

    pub fn release() {
        DRAWN.with(|d| d.borrow_mut().take());
    }

    pub fn feedback() -> Feedback {
        DRAWN.with(|d| {
            d.borrow()
                .as_ref()
                .map(|d| d.feedback.clone())
                .unwrap_or_default()
        })
    }

    pub fn paint(ui: &egui::Ui, rect: egui::Rect, key: Key, t: f64, show_shaders: bool) {
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                DRAWN.with(|d| {
                    let mut d = d.borrow_mut();
                    let Some(d) = d.as_mut().filter(|d| d.key == key) else {
                        return;
                    };
                    if d.feedback.failed.is_some() {
                        return;
                    }
                    if let Err(e) = draw(d, &info, painter, t, show_shaders) {
                        d.feedback.failed = Some(e);
                        d.layer = None;
                        d.gpu = None;
                    }
                })
            })),
        });
    }

    fn draw(
        d: &mut Drawn,
        info: &egui::PaintCallbackInfo,
        painter: &egui_glow::Painter,
        t: f64,
        show_shaders: bool,
    ) -> Result<(), String> {
        let viewport = info.viewport_in_pixels();
        if viewport.width_px <= 0 || viewport.height_px <= 0 {
            return Ok(());
        }
        let (w, h) = d.key.canvas;
        if d.gpu.is_none() {
            d.gpu = Some(GlRenderer::on_egui(
                painter.gl().clone(),
                w,
                h,
                w as f32,
                h as f32,
            )?);
        }
        let gpu = d.gpu.as_mut().unwrap();
        if d.layer.is_none() {
            let Some(p) = d.pending.take() else {
                return Ok(());
            };
            // A new painter for each preparation: its atlas holds only what this
            // validation saw (plan M4, a re-check prepares a fresh painter).
            let mut layer = MotionLayer::new(p.thread, (w, h), 0.0, p.shaders)?;
            layer.setup(gpu)?;
            d.layer = Some(layer);
        }
        let layer = d.layer.as_mut().unwrap();
        layer.show_shaders(show_shaders);
        // The clock has not moved, or the preview skips this frame to keep up:
        // the last overlay is shown again.
        let fps_ms = 1000.0 / f64::from_bits(d.key.fps_bits).clamp(1.0, 240.0);
        let moved = d.last != Some(t);
        let draw_now = moved && !(d.skip && d.feedback.slow);
        let texture = if draw_now {
            d.skip = !d.skip;
            let started = Instant::now();
            match layer.draw(gpu, t) {
                Ok(texture) => {
                    d.last = Some(t);
                    d.refused_run = 0;
                    d.feedback.hidden = layer.stats.hidden_shader_layers;
                    d.feedback.reframes = layer.video().is_some();
                    d.times.push_back(started.elapsed().as_secs_f64() * 1000.0);
                    if d.times.len() > WINDOW {
                        d.times.pop_front();
                    }
                    // Slow: half the frames' draws take over half a frame's time.
                    if d.times.len() == WINDOW {
                        let mut v: Vec<f64> = d.times.iter().copied().collect();
                        v.sort_by(f64::total_cmp);
                        d.feedback.slow = v[WINDOW / 2] > fps_ms * 0.5;
                    }
                    Some(texture)
                }
                Err(why) => {
                    // Shown without the scene, never drawn wrong.
                    d.last = None;
                    d.refused_run += 1;
                    d.feedback.refused = Some((t, why.clone()));
                    if d.refused_run >= GIVE_UP {
                        return Err(why);
                    }
                    None
                }
            }
        } else if d.last.is_some() {
            layer.texture()
        } else {
            None
        };
        let Some(texture) = texture else {
            return Ok(());
        };
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
        gpu.present_texture(texture);
        Ok(())
    }
}

#[cfg(not(feature = "motion"))]
mod engine {
    use super::{Feedback, Key};
    use eframe::egui;
    use noh::subtitle_track::SubtitleTrack;
    use std::path::Path;

    pub enum Prepared {}
    pub const CHANGED: &str = "";

    pub fn prepare(_: &Key, _: &Path, _: &Path, _: &SubtitleTrack) -> Result<Prepared, String> {
        Err("This build of NOH cannot draw scenes.".into())
    }
    pub fn hand_over(_: Key, p: Prepared) {
        match p {}
    }
    pub fn release() {}
    pub fn feedback() -> Feedback {
        Feedback::default()
    }
    pub fn paint(_: &egui::Ui, _: egui::Rect, _: Key, _: f64, _: bool) {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_monitor_says_instants_to_the_tenth() {
        assert_eq!(super::clock(72.44), "1:12,4");
        assert_eq!(super::clock(0.0), "0:00,0");
    }

    /// The window prepares a scene on its script thread, with its fixed stack:
    /// a script hostile to the depth limits is refused there with the reason,
    /// and the window goes on (plan M4); a legal one is prepared.
    #[cfg(feature = "motion")]
    #[test]
    fn the_window_prepares_a_scene_or_refuses_a_hostile_one() {
        use super::{Key, engine};
        let dir = tempfile::tempdir().unwrap();
        // One second of silence, 48 kHz mono.
        let mut wav = Vec::new();
        let data = 48_000u32 * 2;
        for part in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt "] {
            wav.extend_from_slice(part);
        }
        for v in [16u32, 0x0001_0001, 48_000, 96_000, 0x0010_0002] {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        wav.resize(wav.len() + data as usize, 0);
        let song = dir.path().join("song.wav");
        std::fs::write(&song, wav).unwrap();
        let ffmpeg = noh::find_ffmpeg(None).expect("FFmpeg (NOH_FFMPEG)");
        let track = noh::subtitle_track::SubtitleTrack {
            language: None,
            duration_ms: 1000,
            cues: Vec::new(),
            voices: Vec::new(),
        };
        let key = |path: std::path::PathBuf| Key {
            stamp: super::super::app_motion::stamp(&path),
            path,
            track: 0,
            canvas: (640, 360),
            safe: noh::safe_area::SafeArea::None,
            seconds_bits: 1.0f64.to_bits(),
            fps_bits: 24.0f64.to_bits(),
        };
        let write = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let hostile = write(
            "deep.noh-motion.rhai",
            "// noh-motion 0
fn f(n) { f(n + 1) }
fn frame(t, ctx) { f(0); }
",
        );
        let why = engine::prepare(&key(hostile), &song, &ffmpeg, &track)
            .err()
            .expect("a hostile scene was prepared");
        assert!(why.contains("Stack overflow"), "{why}");
        let fine = write(
            "fine.noh-motion.rhai",
            "// noh-motion 0
fn frame(t, ctx) { draw(rect(0.1, 0.1).at(0.5, 0.5)); }
",
        );
        assert!(engine::prepare(&key(fine.clone()), &song, &ffmpeg, &track).is_ok());
        // A version the worker did not check is not prepared: the scene file,
        // or a shader beside it, changed after the key was taken.
        let shaders = fine.with_extension("shaders");
        std::fs::create_dir(&shaders).unwrap();
        let shader = shaders.join("fill.wgsl");
        std::fs::write(
            &shader,
            "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 0.0); }",
        )
        .unwrap();
        let checked = key(fine.clone());
        assert!(engine::prepare(&checked, &song, &ffmpeg, &track).is_ok());
        std::fs::write(
            &shader,
            "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }",
        )
        .unwrap();
        assert_eq!(
            engine::prepare(&checked, &song, &ffmpeg, &track)
                .err()
                .as_deref(),
            Some(engine::CHANGED)
        );
        let checked = key(fine.clone());
        std::fs::write(
            &fine,
            "// noh-motion 0
fn frame(t, ctx) { draw(rect(0.2, 0.2).at(0.5, 0.5)); }
",
        )
        .unwrap();
        assert_eq!(
            engine::prepare(&checked, &song, &ffmpeg, &track)
                .err()
                .as_deref(),
            Some(engine::CHANGED)
        );
    }
}
