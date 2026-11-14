//! Step 2 preview harness: an eframe/glow window like NOH's, a 640 px video
//! proxy as background (decoded by FFmpeg, uploaded like a video texture), and
//! the caption overlay rendered ahead on a worker thread (wgpu DX12) and shown
//! by frame number. The UI thread never waits for the GPU.
use crate::effects::{Scene, Test};
use crate::gpu::{Output, Renderer};
use crate::text::Fonts;
use eframe::egui;
use eframe::glow::{self, HasContext};
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const FPS: f64 = 24.0;
const AHEAD: u64 = 3;
const VIDEO_W: u32 = 640;
const VIDEO_H: u32 = 360;

pub struct Config {
    pub test: Test,
    pub switch_to: Test,
    pub switch_at_s: f64,
    pub logical: (u32, u32),
    pub overlay: (u32, u32),
    pub start_s: f64,
    pub seconds: f64,
    pub out: PathBuf,
    pub assets: PathBuf,
    pub serif: PathBuf,
    pub clip: PathBuf,
    pub ffmpeg: PathBuf,
    pub lines_for: fn(Test) -> Vec<crate::effects::Line>,
    /// "dx12" (worker + readback) or "gl" (wgpu on egui's GL context, no copy).
    pub backend: String,
    /// GL variant: save one window screenshot at this time (visual check).
    pub screenshot_at: Option<f64>,
}

struct OverlayFrame {
    index: u64,
    generation: u64,
    pixels: Vec<u8>,
}

#[derive(Default)]
struct WorkerStats {
    render_ms: Vec<f64>,
    build_ms: Vec<f64>,
    pre_submit_ms: Vec<f64>,
    wait_ms: Vec<f64>,
    copy_ms: Vec<f64>,
    gpu_ms: Vec<f64>,
    skipped: u64,
    setup_ms: f64,
    adapter: String,
}

struct Shared {
    displayed: AtomicU64,
    generation: AtomicU64,
    stop: AtomicBool,
    stats: Mutex<WorkerStats>,
}

fn worker(cfg: Arc<Config>, shared: Arc<Shared>, tx: SyncSender<OverlayFrame>) {
    let (lw, lh) = cfg.logical;
    let (pw, ph) = cfg.overlay;
    let mut gpu = Renderer::with_canvas(pw, ph, lw as f32, lh as f32, Output::PremulGamma).expect("gpu");
    {
        let mut st = shared.stats.lock().unwrap();
        st.setup_ms = gpu.setup_ms;
        st.adapter = gpu.adapter.clone();
    }
    let mut fonts = Fonts::load(&cfg.assets, &cfg.serif).expect("fonts");
    let px_scale = ph as f32 / lh as f32;
    let make = |t: Test| Scene::new(t, lw, lh, false, (cfg.lines_for)(t)).with_px_scale(px_scale);
    let mut generation = shared.generation.load(Ordering::Acquire);
    let mut scene = make(if generation == 0 { cfg.test } else { cfg.switch_to });
    let mut next = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        let g = shared.generation.load(Ordering::Acquire);
        if g != generation {
            generation = g;
            scene = make(cfg.switch_to);
            next = shared.displayed.load(Ordering::Acquire) + 1;
        }
        // `displayed` is the video frame index on screen now; it needs its overlay too.
        let shown = shared.displayed.load(Ordering::Acquire);
        if next < shown {
            shared.stats.lock().unwrap().skipped += shown - next;
            next = shown;
        }
        if next > shown + AHEAD {
            std::thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }
        let t = cfg.start_s + next as f64 / FPS;
        let started = Instant::now();
        let (glow_i, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
        let build_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (pixels, ft) = gpu.frame(&glow_i, gain, &main, true).expect("frame");
        {
            let mut st = shared.stats.lock().unwrap();
            st.render_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            st.build_ms.push(build_ms);
            st.pre_submit_ms.push(ft.pre_submit_ms);
            st.wait_ms.push(ft.submit_to_mapped_ms);
            st.copy_ms.push(ft.copy_out_ms);
            if let Some(g) = ft.gpu_ms {
                st.gpu_ms.push(g);
            }
        }
        if next % 120 == 0 {
            eprintln!("[worker] rendered {next}");
        }
        if tx.send(OverlayFrame { index: next, generation, pixels }).is_err() {
            break;
        }
        next += 1;
    }
}

/// Decodes the clip proxy at 24 fps (looping), frame by frame, in order.
fn video_decoder(cfg: Arc<Config>, tx: SyncSender<Vec<u8>>, stop: Arc<Shared>) {
    let mut child = std::process::Command::new(&cfg.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-stream_loop", "-1", "-i"])
        .arg(&cfg.clip)
        .args(["-vf", &format!("scale={VIDEO_W}:{VIDEO_H},fps=24"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut out = child.stdout.take().unwrap();
    let size = (VIDEO_W * VIDEO_H * 4) as usize;
    while !stop.stop.load(Ordering::Acquire) {
        let mut buf = vec![0u8; size];
        if out.read_exact(&mut buf).is_err() || tx.send(buf).is_err() {
            break;
        }
    }
    let _ = child.kill();
}

struct GlTex {
    tex: glow::Texture,
    id: egui::TextureId,
    w: u32,
    h: u32,
}

fn gl_texture(gl: &glow::Context, frame: &mut eframe::Frame, w: u32, h: u32) -> GlTex {
    unsafe {
        let tex = gl.create_texture().expect("texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        let id = frame.register_native_glow_texture(tex);
        GlTex { tex, id, w, h }
    }
}

fn upload(gl: &glow::Context, t: &GlTex, pixels: &[u8]) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, t.w as i32, t.h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(pixels)));
    }
}

pub struct App {
    cfg: Arc<Config>,
    shared: Arc<Shared>,
    overlays: Receiver<OverlayFrame>,
    video: Receiver<Vec<u8>>,
    pending: VecDeque<OverlayFrame>,
    video_tex: Option<GlTex>,
    overlay_tex: Option<GlTex>,
    started: Option<Instant>,
    last_update: Option<Instant>,
    shown_video: i64,
    shown_overlay: Option<(u64, u64)>,
    switch_sent: Option<Instant>,
    switch_latency_ms: Option<f64>,
    switch_latency_frames: Option<i64>,
    switch_index: u64,
    // metrics
    intervals: Vec<f64>,
    cpu_frame: Vec<f64>,
    upload_overlay_ms: Vec<f64>,
    upload_video_ms: Vec<f64>,
    hits: u64,
    misses: u64,
    frames: u64,
    done: bool,
    shot_requested: bool,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let cfg = Arc::new(cfg);
        let shared = Arc::new(Shared {
            displayed: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            stats: Mutex::new(WorkerStats::default()),
        });
        let (otx, orx) = sync_channel(AHEAD as usize + 1);
        let (vtx, vrx) = sync_channel(8);
        {
            let (c, s) = (cfg.clone(), shared.clone());
            std::thread::spawn(move || worker(c, s, otx));
        }
        {
            let (c, s) = (cfg.clone(), shared.clone());
            std::thread::spawn(move || video_decoder(c, vtx, s));
        }
        Self {
            cfg,
            shared,
            overlays: orx,
            video: vrx,
            pending: VecDeque::new(),
            video_tex: None,
            overlay_tex: None,
            started: None,
            last_update: None,
            shown_video: -1,
            shown_overlay: None,
            switch_sent: None,
            switch_latency_ms: None,
            switch_latency_frames: None,
            switch_index: 0,
            intervals: Vec::new(),
            cpu_frame: Vec::new(),
            upload_overlay_ms: Vec::new(),
            upload_video_ms: Vec::new(),
            hits: 0,
            misses: 0,
            frames: 0,
            done: false,
            shot_requested: false,
        }
    }

    fn write_report(&self) {
        let st = self.shared.stats.lock().unwrap();
        let s = |name: &str, v: &[f64]| crate::summary(name, v);
        let gaps50 = self.intervals.iter().filter(|v| **v > 50.0).count();
        let json = format!(
            "{{\n\"test\": {}, \"switch_to\": {}, \"logical\": [{}, {}], \"overlay\": [{}, {}], \"seconds\": {},\n\"adapter\": {:?}, \"worker_setup_ms\": {:.1},\n{},\n{},\n{},\n{},\n{},\n\"ui_gaps_over_50ms\": {gaps50}, \"frames\": {}, \"overlay_hits\": {}, \"overlay_misses\": {}, \"worker_skipped\": {},\n\"style_switch_latency_ms\": {}, \"style_switch_latency_video_frames\": {}\n}}\n",
            self.cfg.test as u32,
            self.cfg.switch_to as u32,
            self.cfg.logical.0,
            self.cfg.logical.1,
            self.cfg.overlay.0,
            self.cfg.overlay.1,
            self.cfg.seconds,
            st.adapter,
            st.setup_ms,
            s("ui_interval_ms", &self.intervals),
            s("ui_cpu_frame_ms", &self.cpu_frame),
            s("upload_overlay_ms", &self.upload_overlay_ms),
            s("upload_video_ms", &self.upload_video_ms),
            s("worker_render_ms", &st.render_ms) + ",\n" + &s("worker_build_ms", &st.build_ms) + ",\n" + &s("worker_pre_submit_ms", &st.pre_submit_ms) + ",\n" + &s("worker_gpu_ms", &st.gpu_ms) + ",\n" + &s("worker_wait_ms", &st.wait_ms) + ",\n" + &s("worker_copy_ms", &st.copy_ms),
            self.frames,
            self.hits,
            self.misses,
            st.skipped,
            self.switch_latency_ms.map(|v| format!("{v:.1}")).unwrap_or("null".into()),
            self.switch_latency_frames.map(|v| v.to_string()).unwrap_or("null".into()),
        );
        std::fs::create_dir_all(&self.cfg.out).unwrap();
        std::fs::write(self.cfg.out.join("summary.json"), &json).unwrap();
        let mut csv = String::from("ui_frame,interval_ms\n");
        for (i, v) in self.intervals.iter().enumerate() {
            csv += &format!("{i},{v:.3}\n");
        }
        std::fs::write(self.cfg.out.join("ui_intervals.csv"), csv).unwrap();
        println!("{json}");
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Optional CPU-side pacing when the display does not throttle (vsync off, screen asleep).
        if let (Some(hz), Some(prev)) = (std::env::var("NOH_PROTO_UI_HZ").ok().and_then(|v| v.parse::<f64>().ok()), self.last_update) {
            let next = prev + std::time::Duration::from_secs_f64(1.0 / hz);
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
        }
        let now = Instant::now();
        if self.frames % 120 == 0 {
            eprintln!("[ui] frame {} started={} pending={} shown_video={}", self.frames, self.started.is_some(), self.pending.len(), self.shown_video);
        }
        let ctx = ui.ctx().clone();
        ctx.request_repaint();
        let gl = frame.gl().expect("glow").clone();
        if self.video_tex.is_none() {
            self.video_tex = Some(gl_texture(&gl, frame, VIDEO_W, VIDEO_H));
            self.overlay_tex = Some(gl_texture(&gl, frame, self.cfg.overlay.0, self.cfg.overlay.1));
        }
        // Wait until the first overlay and video frames exist, then start the clock.
        if self.started.is_none() {
            while let Ok(o) = self.overlays.try_recv() {
                self.pending.push_back(o);
            }
            if self.pending.iter().any(|o| o.index == 0) {
                self.started = Some(now);
            } else {
                self.frames += 1;
                return;
            }
        }
        if let Some(prev) = self.last_update {
            self.intervals.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            if let Some(c) = frame.info().cpu_usage {
                self.cpu_frame.push(c as f64 * 1000.0);
            }
        }
        self.last_update = Some(now);
        self.frames += 1;
        let elapsed = now.duration_since(self.started.unwrap()).as_secs_f64();
        let k = (elapsed * FPS).floor() as i64;
        // Video: consume decoded frames up to k (one upload for the latest).
        let mut latest = None;
        while self.shown_video < k {
            match self.video.try_recv() {
                Ok(buf) => {
                    self.shown_video += 1;
                    latest = Some(buf);
                }
                Err(_) => break,
            }
        }
        if let Some(buf) = latest {
            let t0 = Instant::now();
            upload(&gl, self.video_tex.as_ref().unwrap(), &buf);
            self.upload_video_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        let shown_video = self.shown_video.max(0) as u64;
        self.shared.displayed.store(shown_video, Ordering::Release);
        // Overlay: the frame with the displayed video index, from the current generation.
        while let Ok(o) = self.overlays.try_recv() {
            self.pending.push_back(o);
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        self.pending.retain(|o| o.generation == generation && o.index >= shown_video);
        if self.shown_overlay != Some((shown_video, generation)) {
            if let Some(pos) = self.pending.iter().position(|o| o.index == shown_video) {
                let o = self.pending.remove(pos).unwrap();
                let t0 = Instant::now();
                upload(&gl, self.overlay_tex.as_ref().unwrap(), &o.pixels);
                self.upload_overlay_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
                self.shown_overlay = Some((shown_video, generation));
                self.hits += 1;
                if generation > 0 && self.switch_latency_ms.is_none() {
                    if let Some(sent) = self.switch_sent {
                        self.switch_latency_ms = Some(now.duration_since(sent).as_secs_f64() * 1000.0);
                        self.switch_latency_frames = Some(shown_video as i64 - self.switch_index as i64);
                    }
                }
            } else {
                self.misses += 1;
            }
        }
        // Style change once, mid-run.
        if self.switch_sent.is_none() && elapsed >= self.cfg.switch_at_s {
            self.switch_sent = Some(now);
            self.switch_index = shown_video;
            self.shared.generation.store(1, Ordering::Release);
        }
        let rect = ui.available_rect_before_wrap();
        let (lw, lh) = (self.cfg.logical.0 as f32, self.cfg.logical.1 as f32);
        let scale = (rect.width() / lw).min(rect.height() / lh);
        let size = egui::vec2(lw * scale, lh * scale);
        let area = egui::Rect::from_center_size(rect.center(), size);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        // Video proxy fills (cropped) the canvas like NOH's preview of the composition.
        let painter = ui.painter_at(area);
        let vid_aspect = VIDEO_W as f32 / VIDEO_H as f32;
        let can_aspect = lw / lh;
        let vuv = if vid_aspect > can_aspect {
            let w = can_aspect / vid_aspect;
            egui::Rect::from_min_max(egui::pos2(0.5 - w * 0.5, 0.0), egui::pos2(0.5 + w * 0.5, 1.0))
        } else {
            let h = vid_aspect / can_aspect;
            egui::Rect::from_min_max(egui::pos2(0.0, 0.5 - h * 0.5), egui::pos2(1.0, 0.5 + h * 0.5))
        };
        painter.image(self.video_tex.as_ref().unwrap().id, area, vuv, egui::Color32::WHITE);
        painter.image(self.overlay_tex.as_ref().unwrap().id, area, uv, egui::Color32::WHITE);
        if self.cfg.screenshot_at.is_some_and(|at| elapsed >= at) && !self.shot_requested {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }).collect());
        if let Some(img) = shots.first() {
            let [iw, ih] = img.size;
            let rgb: Vec<u8> = img.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]).collect();
            std::fs::create_dir_all(&self.cfg.out).ok();
            image::save_buffer(self.cfg.out.join("screenshot.png"), &rgb, iw as u32, ih as u32, image::ColorType::Rgb8).ok();
        }
        if elapsed >= self.cfg.seconds && !self.done {
            self.done = true;
            self.shared.stop.store(true, Ordering::Release);
            self.write_report();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

pub fn run(cfg: Config) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1700.0, 1000.0]).with_title("NOH caption preview harness"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if cfg.backend == "glnative" {
        eframe::run_native("caption-preview-harness-glnative", options, Box::new(|_cc| Ok(Box::new(NativeGlApp::new(cfg))))).expect("eframe");
    } else if cfg.backend == "gl" {
        eframe::run_native("caption-preview-harness-gl", options, Box::new(|_cc| Ok(Box::new(GlApp::new(cfg))))).expect("eframe");
    } else {
        eframe::run_native("caption-preview-harness", options, Box::new(|_cc| Ok(Box::new(App::new(cfg))))).expect("eframe");
    }
}

/// OpenGL entry points for wgpu's external adapter: wglGetProcAddress, then opengl32 exports.
fn gl_loader() -> impl FnMut(&str) -> *const std::ffi::c_void {
    let lib: &'static libloading::Library = Box::leak(Box::new(unsafe { libloading::Library::new("opengl32.dll") }.expect("opengl32")));
    let wgl: unsafe extern "system" fn(*const std::ffi::c_char) -> *const std::ffi::c_void = unsafe { *lib.get(b"wglGetProcAddress\0").expect("wglGetProcAddress") };
    move |name: &str| {
        let c = std::ffi::CString::new(name).unwrap();
        let p = unsafe { wgl(c.as_ptr()) };
        if matches!(p as isize, -1 | 0 | 1 | 2 | 3) {
            unsafe { lib.get::<*const std::ffi::c_void>(c.as_bytes_with_nul()) }.map(|s| *s).unwrap_or(std::ptr::null())
        } else {
            p
        }
    }
}

/// Variant: the overlay is rendered by wgpu on egui's own GL context, on the UI thread,
/// into a GL texture that egui draws directly: no readback, no upload.
pub struct GlApp {
    cfg: Arc<Config>,
    shared: Arc<Shared>,
    video: Receiver<Vec<u8>>,
    video_tex: Option<GlTex>,
    overlay_id: Option<egui::TextureId>,
    gpu: Option<Renderer>,
    fonts: Option<Fonts>,
    scene: Option<Scene>,
    started: Option<Instant>,
    last_update: Option<Instant>,
    shown_video: i64,
    rendered: Option<(i64, u64)>,
    generation: u64,
    switch_sent: Option<Instant>,
    switch_latency_ms: Option<f64>,
    intervals: Vec<f64>,
    cpu_frame: Vec<f64>,
    update_ms: Vec<f64>,
    upload_video_ms: Vec<f64>,
    setup_ms: f64,
    adapter: String,
    frames: u64,
    done: bool,
    shot_requested: bool,
}

impl GlApp {
    pub fn new(cfg: Config) -> Self {
        let cfg = Arc::new(cfg);
        let shared = Arc::new(Shared { displayed: AtomicU64::new(0), generation: AtomicU64::new(0), stop: AtomicBool::new(false), stats: Mutex::new(WorkerStats::default()) });
        let (vtx, vrx) = sync_channel(8);
        {
            let (c, s) = (cfg.clone(), shared.clone());
            std::thread::spawn(move || video_decoder(c, vtx, s));
        }
        Self {
            cfg,
            shared,
            video: vrx,
            video_tex: None,
            overlay_id: None,
            gpu: None,
            fonts: None,
            scene: None,
            started: None,
            last_update: None,
            shown_video: -1,
            rendered: None,
            generation: 0,
            switch_sent: None,
            switch_latency_ms: None,
            intervals: Vec::new(),
            cpu_frame: Vec::new(),
            update_ms: Vec::new(),
            upload_video_ms: Vec::new(),
            setup_ms: 0.0,
            adapter: String::new(),
            frames: 0,
            done: false,
            shot_requested: false,
        }
    }

    fn write_report(&self) {
        let s = |name: &str, v: &[f64]| crate::summary(name, v);
        let gaps50 = self.intervals.iter().filter(|v| **v > 50.0).count();
        let json = format!(
            "{{\n\"backend\": \"gl\", \"test\": {}, \"switch_to\": {}, \"logical\": [{}, {}], \"overlay\": [{}, {}], \"seconds\": {},\n\"adapter\": {:?}, \"setup_ms\": {:.1},\n{},\n{},\n{},\n{},\n\"ui_gaps_over_50ms\": {gaps50}, \"frames\": {}, \"style_switch_latency_ms\": {}\n}}\n",
            self.cfg.test as u32,
            self.cfg.switch_to as u32,
            self.cfg.logical.0,
            self.cfg.logical.1,
            self.cfg.overlay.0,
            self.cfg.overlay.1,
            self.cfg.seconds,
            self.adapter,
            self.setup_ms,
            s("ui_interval_ms", &self.intervals),
            s("ui_cpu_frame_ms", &self.cpu_frame),
            s("overlay_update_ms", &self.update_ms),
            s("upload_video_ms", &self.upload_video_ms),
            self.frames,
            self.switch_latency_ms.map(|v| format!("{v:.1}")).unwrap_or("null".into()),
        );
        std::fs::create_dir_all(&self.cfg.out).unwrap();
        std::fs::write(self.cfg.out.join("summary.json"), &json).unwrap();
        println!("{json}");
    }
}

impl eframe::App for GlApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let ctx = ui.ctx().clone();
        ctx.request_repaint();
        let gl = frame.gl().expect("glow").clone();
        let (lw, lh) = self.cfg.logical;
        let (pw, ph) = self.cfg.overlay;
        if self.gpu.is_none() {
            // Remember egui's current WGL context so it can be made current again.
            let restore = unsafe {
                let lib: &'static libloading::Library = Box::leak(Box::new(libloading::Library::new("opengl32.dll").unwrap()));
                let get_ctx: unsafe extern "system" fn() -> *mut std::ffi::c_void = *lib.get(b"wglGetCurrentContext\0").unwrap();
                let get_dc: unsafe extern "system" fn() -> *mut std::ffi::c_void = *lib.get(b"wglGetCurrentDC\0").unwrap();
                let make: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32 = *lib.get(b"wglMakeCurrent\0").unwrap();
                let (ctx_h, dc_h) = (get_ctx() as usize, get_dc() as usize);
                move || {
                    let ok = make(dc_h as *mut _, ctx_h as *mut _);
                    assert!(ok != 0, "wglMakeCurrent failed");
                }
            };
            let gpu = unsafe { Renderer::on_current_gl(gl_loader(), restore, pw, ph, lw as f32, lh as f32) }.expect("wgpu on egui GL");
            self.setup_ms = gpu.setup_ms;
            self.adapter = gpu.adapter.clone();
            let tex = gpu.output_gl_texture().expect("GL output texture");
            self.overlay_id = Some(frame.register_native_glow_texture(tex));
            self.video_tex = Some(gl_texture(&gl, frame, VIDEO_W, VIDEO_H));
            self.fonts = Some(Fonts::load(&self.cfg.assets, &self.cfg.serif).expect("fonts"));
            self.scene = Some(Scene::new(self.cfg.test, lw, lh, false, (self.cfg.lines_for)(self.cfg.test)).with_px_scale(ph as f32 / lh as f32));
            self.gpu = Some(gpu);
            self.started = Some(now);
        }
        if let Some(prev) = self.last_update {
            self.intervals.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            if let Some(c) = frame.info().cpu_usage {
                self.cpu_frame.push(c as f64 * 1000.0);
            }
        }
        self.last_update = Some(now);
        self.frames += 1;
        let elapsed = now.duration_since(self.started.unwrap()).as_secs_f64();
        let k = (elapsed * FPS).floor() as i64;
        let mut latest = None;
        while self.shown_video < k {
            match self.video.try_recv() {
                Ok(buf) => {
                    self.shown_video += 1;
                    latest = Some(buf);
                }
                Err(_) => break,
            }
        }
        if let Some(buf) = latest {
            let t0 = Instant::now();
            upload(&gl, self.video_tex.as_ref().unwrap(), &buf);
            self.upload_video_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        if self.switch_sent.is_none() && elapsed >= self.cfg.switch_at_s {
            self.switch_sent = Some(now);
            self.generation = 1;
            self.scene = Some(Scene::new(self.cfg.switch_to, lw, lh, false, (self.cfg.lines_for)(self.cfg.switch_to)).with_px_scale(ph as f32 / lh as f32));
        }
        let shown = self.shown_video.max(0);
        if self.rendered != Some((shown, self.generation)) {
            let t0 = Instant::now();
            let t = self.cfg.start_s + shown as f64 / FPS;
            let gpu = self.gpu.as_mut().unwrap();
            let (g, gain, m, _) = self.scene.as_mut().unwrap().frame(self.fonts.as_mut().unwrap(), gpu, t);
            gpu.frame_nowait(&g, gain, &m).expect("frame");
            // wgpu leaves its own GL bindings; hand egui a clean default framebuffer.
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.bind_vertex_array(None);
                gl.use_program(None);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                gl.disable(glow::SCISSOR_TEST);
            }
            self.update_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            self.rendered = Some((shown, self.generation));
            if self.generation == 1 && self.switch_latency_ms.is_none() {
                self.switch_latency_ms = Some(now.duration_since(self.switch_sent.unwrap()).as_secs_f64() * 1000.0 + t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let rect = ui.available_rect_before_wrap();
        let scale = (rect.width() / lw as f32).min(rect.height() / lh as f32);
        let area = egui::Rect::from_center_size(rect.center(), egui::vec2(lw as f32 * scale, lh as f32 * scale));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let painter = ui.painter_at(area);
        let (vid_aspect, can_aspect) = (VIDEO_W as f32 / VIDEO_H as f32, lw as f32 / lh as f32);
        let vuv = if vid_aspect > can_aspect {
            let w = can_aspect / vid_aspect;
            egui::Rect::from_min_max(egui::pos2(0.5 - w * 0.5, 0.0), egui::pos2(0.5 + w * 0.5, 1.0))
        } else {
            let h = vid_aspect / can_aspect;
            egui::Rect::from_min_max(egui::pos2(0.0, 0.5 - h * 0.5), egui::pos2(1.0, 0.5 + h * 0.5))
        };
        painter.image(self.video_tex.as_ref().unwrap().id, area, vuv, egui::Color32::WHITE);
        painter.image(self.overlay_id.unwrap(), area, uv, egui::Color32::WHITE);
        if self.cfg.screenshot_at.is_some_and(|at| elapsed >= at) && !self.shot_requested {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }).collect());
        if let Some(img) = shots.first() {
            let [iw, ih] = img.size;
            let rgb: Vec<u8> = img.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]).collect();
            image::save_buffer(self.cfg.out.join("screenshot.png"), &rgb, iw as u32, ih as u32, image::ColorType::Rgb8).ok();
        }
        if elapsed >= self.cfg.seconds && !self.done {
            self.done = true;
            self.shared.stop.store(true, Ordering::Release);
            self.write_report();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.shared.stop.store(true, Ordering::Release);
        // drop wgpu objects while egui's GL context is still current
        self.scene = None;
        self.gpu = None;
    }
}

struct CaptureApp {
    w: u32,
    h: u32,
    bg: Vec<u8>,
    overlay: Vec<u8>,
    out: PathBuf,
    tex: Option<(GlTex, GlTex)>,
    frames: u32,
    requested: bool,
}

impl eframe::App for CaptureApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // One egui point = one physical pixel, so the canvas is drawn 1:1.
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        if (ctx.zoom_factor() - 1.0 / native).abs() > 1e-6 {
            ctx.set_zoom_factor(1.0 / native);
        }
        let gl = frame.gl().expect("glow").clone();
        if self.tex.is_none() {
            let b = gl_texture(&gl, frame, self.w, self.h);
            let o = gl_texture(&gl, frame, self.w, self.h);
            upload(&gl, &b, &self.bg);
            upload(&gl, &o, &self.overlay);
            self.tex = Some((b, o));
        }
        let (b, o) = self.tex.as_ref().unwrap();
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(self.w as f32, self.h as f32));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let painter = ui.ctx().layer_painter(egui::LayerId::background());
        painter.image(b.id, area, uv, egui::Color32::WHITE);
        painter.image(o.id, area, uv, egui::Color32::WHITE);
        self.frames += 1;
        ctx.request_repaint();
        if self.frames == 10 && !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None })
                .collect()
        });
        if let Some(img) = shots.first() {
            let [iw, ih] = img.size;
            let mut rgb = Vec::with_capacity((self.w * self.h * 3) as usize);
            for y in 0..self.h as usize {
                for x in 0..self.w as usize {
                    let p = img.pixels[y.min(ih - 1) * iw + x.min(iw - 1)];
                    rgb.extend_from_slice(&[p.r(), p.g(), p.b()]);
                }
            }
            image::save_buffer(&self.out, &rgb, self.w, self.h, image::ColorType::Rgb8).unwrap();
            std::fs::write(self.out.with_extension("rgb"), &rgb).unwrap();
            println!("captured {}x{} (screenshot {}x{}, native ppp {native})", self.w, self.h, iw, ih);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub fn capture(w: u32, h: u32, bg: Vec<u8>, overlay: Vec<u8>, out: PathBuf) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([w as f32 + 40.0, h as f32 + 40.0]).with_title("NOH blend capture"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let app = CaptureApp { w, h, bg, overlay, out, tex: None, frames: 0, requested: false };
    eframe::run_native("caption-blend-capture", options, Box::new(|_cc| Ok(Box::new(app)))).expect("eframe");
}

/// Isolation cost: send `n` overlay frames of `w`×`h` RGBA to a child process
/// over a pipe and get them echoed back; per-frame round trip.
pub fn pipe_roundtrip(exe: &std::path::Path, w: u32, h: u32, n: usize) -> Vec<f64> {
    use std::io::Write;
    let size = (w * h * 4) as usize;
    let mut child = std::process::Command::new(exe)
        .args(["echo", "--bytes", &size.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("echo child");
    let mut cin = child.stdin.take().unwrap();
    let mut cout = child.stdout.take().unwrap();
    let frame: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let mut back = vec![0u8; size];
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        cin.write_all(&frame).unwrap();
        cout.read_exact(&mut back).unwrap();
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(back, frame);
    drop(cin);
    let _ = child.wait();
    times
}

pub fn echo(bytes: usize) {
    use std::io::Write;
    let mut buf = vec![0u8; bytes];
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut i = stdin.lock();
    let mut o = stdout.lock();
    while i.read_exact(&mut buf).is_ok() {
        if o.write_all(&buf).is_err() || o.flush().is_err() {
            break;
        }
    }
}

/// Plan GL §A4: the OpenGL engine (gl.rs) on egui's own context, on the UI thread.
/// The overlay is redrawn only when the video frame (or the style) changes, into the
/// engine's output texture, which egui draws as a native texture: no readback, no
/// upload, no render-ahead. GPU time comes from TIME_ELAPSED queries read later.
pub struct NativeGlApp {
    cfg: Arc<Config>,
    shared: Arc<Shared>,
    video: Receiver<Vec<u8>>,
    video_tex: Option<GlTex>,
    overlay_id: Option<egui::TextureId>,
    gpu: Option<crate::gl::GlRenderer>,
    fonts: Option<Fonts>,
    scene: Option<Scene>,
    started: Option<Instant>,
    last_update: Option<Instant>,
    shown_video: i64,
    rendered: Option<(i64, u64)>,
    generation: u64,
    switch_sent: Option<Instant>,
    switch_latency_ms: Option<f64>,
    intervals: Vec<f64>,
    cpu_frame: Vec<f64>,
    update_ms: Vec<f64>,
    build_ms: Vec<f64>,
    gpu_ms: Vec<f64>,
    update_total_ms: Vec<f64>,
    pending_cpu: VecDeque<f64>,
    upload_video_ms: Vec<f64>,
    setup_ms: f64,
    adapter: String,
    frames: u64,
    done: bool,
    shot_requested: bool,
    frame_log: String,
    frame_update_ms: f64,
    frame_upload_ms: f64,
}

impl NativeGlApp {
    pub fn new(cfg: Config) -> Self {
        let cfg = Arc::new(cfg);
        let shared = Arc::new(Shared { displayed: AtomicU64::new(0), generation: AtomicU64::new(0), stop: AtomicBool::new(false), stats: Mutex::new(WorkerStats::default()) });
        let (vtx, vrx) = sync_channel(8);
        {
            let (c, s) = (cfg.clone(), shared.clone());
            std::thread::spawn(move || video_decoder(c, vtx, s));
        }
        Self {
            cfg,
            shared,
            video: vrx,
            video_tex: None,
            overlay_id: None,
            gpu: None,
            fonts: None,
            scene: None,
            started: None,
            last_update: None,
            shown_video: -1,
            rendered: None,
            generation: 0,
            switch_sent: None,
            switch_latency_ms: None,
            intervals: Vec::new(),
            cpu_frame: Vec::new(),
            update_ms: Vec::new(),
            build_ms: Vec::new(),
            gpu_ms: Vec::new(),
            update_total_ms: Vec::new(),
            pending_cpu: VecDeque::new(),
            upload_video_ms: Vec::new(),
            setup_ms: 0.0,
            adapter: String::new(),
            frames: 0,
            done: false,
            shot_requested: false,
            frame_log: String::from("ui_frame,interval_ms,body_ms,update_ms,video_upload_ms,gpu_done
"),
            frame_update_ms: 0.0,
            frame_upload_ms: 0.0,
        }
    }

    fn write_report(&self) {
        let s = |name: &str, v: &[f64]| crate::summary(name, v);
        let gaps50 = self.intervals.iter().filter(|v| **v > 50.0).count();
        let json = format!(
            "{{\n\"backend\": \"glnative\", \"test\": {}, \"switch_to\": {}, \"logical\": [{}, {}], \"overlay\": [{}, {}], \"seconds\": {},\n\"adapter\": {:?}, \"setup_ms\": {:.1},\n{},\n{},\n{},\n{},\n{},\n{},\n{},\n\"ui_gaps_over_50ms\": {gaps50}, \"frames\": {}, \"style_switch_latency_ms\": {}\n}}\n",
            self.cfg.test as u32,
            self.cfg.switch_to as u32,
            self.cfg.logical.0,
            self.cfg.logical.1,
            self.cfg.overlay.0,
            self.cfg.overlay.1,
            self.cfg.seconds,
            self.adapter,
            self.setup_ms,
            s("ui_interval_ms", &self.intervals),
            s("ui_cpu_frame_ms", &self.cpu_frame),
            s("overlay_update_cpu_ms", &self.update_ms),
            s("overlay_build_ms", &self.build_ms),
            s("overlay_gpu_ms", &self.gpu_ms),
            s("overlay_update_cpu_plus_gpu_ms", &self.update_total_ms),
            s("upload_video_ms", &self.upload_video_ms),
            self.frames,
            self.switch_latency_ms.map(|v| format!("{v:.1}")).unwrap_or("null".into()),
        );
        std::fs::create_dir_all(&self.cfg.out).unwrap();
        std::fs::write(self.cfg.out.join("summary.json"), &json).unwrap();
        let mut csv = String::from("ui_frame,interval_ms\n");
        for (i, v) in self.intervals.iter().enumerate() {
            csv += &format!("{i},{v:.3}\n");
        }
        std::fs::write(self.cfg.out.join("ui_intervals.csv"), csv).unwrap();
        std::fs::write(self.cfg.out.join("ui_frames.csv"), &self.frame_log).unwrap();
        println!("{json}");
    }
}

impl eframe::App for NativeGlApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let (Some(hz), Some(prev)) = (std::env::var("NOH_PROTO_UI_HZ").ok().and_then(|v| v.parse::<f64>().ok()), self.last_update) {
            let next = prev + std::time::Duration::from_secs_f64(1.0 / hz);
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
        }
        let now = Instant::now();
        let ctx = ui.ctx().clone();
        ctx.request_repaint();
        let gl = frame.gl().expect("glow").clone();
        let (lw, lh) = self.cfg.logical;
        let (pw, ph) = self.cfg.overlay;
        if self.gpu.is_none() {
            let gpu = crate::gl::GlRenderer::on_egui(gl.clone(), pw, ph, lw as f32, lh as f32).expect("GL engine on egui context");
            self.setup_ms = gpu.setup_ms;
            self.adapter = gpu.adapter.clone();
            eprintln!("[glnative] {}", self.adapter);
            self.overlay_id = Some(frame.register_native_glow_texture(gpu.output_texture()));
            self.video_tex = Some(gl_texture(&gl, frame, VIDEO_W, VIDEO_H));
            self.fonts = Some(Fonts::load(&self.cfg.assets, &self.cfg.serif).expect("fonts"));
            self.scene = Some(Scene::new(self.cfg.test, lw, lh, false, (self.cfg.lines_for)(self.cfg.test)).with_px_scale(ph as f32 / lh as f32));
            self.gpu = Some(gpu);
            // Setup (fonts, programs) is not a UI frame: the clock and intervals start after it.
            self.started = Some(Instant::now());
            self.last_update = self.started;
            self.frames += 1;
            return;
        }
        if let Some(prev) = self.last_update {
            self.intervals.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            if let Some(c) = frame.info().cpu_usage {
                self.cpu_frame.push(c as f64 * 1000.0);
            }
        }
        self.last_update = Some(now);
        self.frames += 1;
        let elapsed = now.duration_since(self.started.unwrap()).as_secs_f64();
        let k = (elapsed * FPS).floor() as i64;
        let mut latest = None;
        while self.shown_video < k {
            match self.video.try_recv() {
                Ok(buf) => {
                    self.shown_video += 1;
                    latest = Some(buf);
                }
                Err(_) => break,
            }
        }
        if let Some(buf) = latest {
            let t0 = Instant::now();
            upload(&gl, self.video_tex.as_ref().unwrap(), &buf);
            unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
            self.frame_upload_ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.upload_video_ms.push(self.frame_upload_ms);
        }
        if self.switch_sent.is_none() && elapsed >= self.cfg.switch_at_s {
            self.switch_sent = Some(now);
            self.generation = 1;
            self.scene = Some(Scene::new(self.cfg.switch_to, lw, lh, false, (self.cfg.lines_for)(self.cfg.switch_to)).with_px_scale(ph as f32 / lh as f32));
        }
        let gpu = self.gpu.as_mut().unwrap();
        let shown = self.shown_video.max(0);
        if self.rendered != Some((shown, self.generation)) {
            let t0 = Instant::now();
            let t = self.cfg.start_s + shown as f64 / FPS;
            let (g, gain, m, _) = self.scene.as_mut().unwrap().frame(self.fonts.as_mut().unwrap(), &*gpu, t);
            self.build_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            gpu.frame(&g, gain, &m, false).expect("frame");
            let cpu = t0.elapsed().as_secs_f64() * 1000.0;
            self.frame_update_ms = cpu;
            self.update_ms.push(cpu);
            self.pending_cpu.push_back(cpu);
            self.rendered = Some((shown, self.generation));
            if self.generation == 1 && self.switch_latency_ms.is_none() {
                self.switch_latency_ms = Some(now.duration_since(self.switch_sent.unwrap()).as_secs_f64() * 1000.0 + cpu);
            }
        }
        // Each finished query belongs to the oldest update still waiting for its GPU time.
        let mut gpu_done = 0;
        while let Some(ms) = gpu.take_gpu_ms() {
            gpu_done += 1;
            self.gpu_ms.push(ms);
            if let Some(cpu) = self.pending_cpu.pop_front() {
                self.update_total_ms.push(cpu + ms);
            }
        }
        let rect = ui.available_rect_before_wrap();
        let scale = (rect.width() / lw as f32).min(rect.height() / lh as f32);
        let area = egui::Rect::from_center_size(rect.center(), egui::vec2(lw as f32 * scale, lh as f32 * scale));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let painter = ui.painter_at(area);
        let (vid_aspect, can_aspect) = (VIDEO_W as f32 / VIDEO_H as f32, lw as f32 / lh as f32);
        let vuv = if vid_aspect > can_aspect {
            let w = can_aspect / vid_aspect;
            egui::Rect::from_min_max(egui::pos2(0.5 - w * 0.5, 0.0), egui::pos2(0.5 + w * 0.5, 1.0))
        } else {
            let h = vid_aspect / can_aspect;
            egui::Rect::from_min_max(egui::pos2(0.0, 0.5 - h * 0.5), egui::pos2(1.0, 0.5 + h * 0.5))
        };
        painter.image(self.video_tex.as_ref().unwrap().id, area, vuv, egui::Color32::WHITE);
        painter.image(self.overlay_id.unwrap(), area, uv, egui::Color32::WHITE);
        if self.cfg.screenshot_at.is_some_and(|at| elapsed >= at) && !self.shot_requested {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }).collect());
        if let Some(img) = shots.first() {
            let [iw, ih] = img.size;
            let rgb: Vec<u8> = img.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]).collect();
            std::fs::create_dir_all(&self.cfg.out).ok();
            image::save_buffer(self.cfg.out.join("screenshot.png"), &rgb, iw as u32, ih as u32, image::ColorType::Rgb8).ok();
        }
        let interval = self.intervals.last().copied().unwrap_or(0.0);
        self.frame_log += &format!("{},{interval:.3},{:.3},{:.3},{:.3},{gpu_done}
", self.frames, now.elapsed().as_secs_f64() * 1000.0, self.frame_update_ms, self.frame_upload_ms);
        self.frame_update_ms = 0.0;
        self.frame_upload_ms = 0.0;
        if elapsed >= self.cfg.seconds && !self.done {
            self.done = true;
            self.shared.stop.store(true, Ordering::Release);
            self.write_report();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.shared.stop.store(true, Ordering::Release);
    }
}
