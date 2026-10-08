//! Opt-in persistent seek experiment in the actual egui/glow renderer.
//! Usage: cargo run --profile qa --features gui --example scrub_probe -- DLL VIDEO REPORT.json
//! It drives egui pointer input, coalesces pending seeks, saves a native screenshot,
//! and measures input-to-GL-render completion, NOT physical screen presentation.
use eframe::{egui, egui_glow, glow};
use glow::HasContext;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
type H = *mut c_void;
type Resolver = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[repr(C)]
#[derive(Clone, Copy)]
struct Param {
    kind: i32,
    data: H,
}
impl Param {
    fn value<T>(kind: i32, data: &mut T) -> Self {
        Self {
            kind,
            data: (data as *mut T).cast(),
        }
    }
    fn end() -> Self {
        Self {
            kind: 0,
            data: std::ptr::null_mut(),
        }
    }
}
#[repr(C)]
struct Event {
    id: i32,
    error: i32,
    userdata: u64,
    data: H,
}
#[repr(C)]
#[derive(Default)]
struct FrameInfo {
    flags: u64,
    target_time: i64,
}
#[repr(C)]
struct Init {
    get: unsafe extern "C" fn(H, *const c_char) -> H,
    data: H,
}
#[repr(C)]
struct Fbo {
    fbo: i32,
    w: i32,
    h: i32,
    format: i32,
}
struct Api {
    _library: libloading::Library,
    create: unsafe extern "C" fn() -> H,
    initialize: unsafe extern "C" fn(H) -> i32,
    option: unsafe extern "C" fn(H, *const c_char, *const c_char) -> i32,
    command: unsafe extern "C" fn(H, u64, *const *const c_char) -> i32,
    event: unsafe extern "C" fn(H, f64) -> *const Event,
    property: unsafe extern "C" fn(H, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(H),
    destroy: unsafe extern "C" fn(H),
    wakeup: unsafe extern "C" fn(H),
    render_create: unsafe extern "C" fn(*mut H, H, *const Param) -> i32,
    render_update: unsafe extern "C" fn(H) -> u64,
    render_callback: unsafe extern "C" fn(H, unsafe extern "C" fn(H), H),
    render_info: unsafe extern "C" fn(H, Param) -> i32,
    render: unsafe extern "C" fn(H, *const Param) -> i32,
    render_free: unsafe extern "C" fn(H),
}
impl Api {
    unsafe fn load(path: &PathBuf) -> Result<Self> {
        let library = unsafe { libloading::Library::new(path)? };
        macro_rules! sym {
            ($name:literal) => {
                unsafe { *library.get(concat!($name, "\0").as_bytes())? }
            };
        }
        Ok(Self {
            create: sym!("mpv_create"),
            initialize: sym!("mpv_initialize"),
            option: sym!("mpv_set_option_string"),
            command: sym!("mpv_command_async"),
            event: sym!("mpv_wait_event"),
            property: sym!("mpv_get_property_string"),
            free: sym!("mpv_free"),
            destroy: sym!("mpv_terminate_destroy"),
            wakeup: sym!("mpv_wakeup"),
            render_create: sym!("mpv_render_context_create"),
            render_update: sym!("mpv_render_context_update"),
            render_callback: sym!("mpv_render_context_set_update_callback"),
            render_info: sym!("mpv_render_context_get_info"),
            render: sym!("mpv_render_context_render"),
            render_free: sym!("mpv_render_context_free"),
            _library: library,
        })
    }
    fn option(&self, handle: usize, key: &str, value: &str) -> Result<()> {
        check(unsafe {
            (self.option)(
                handle as H,
                CString::new(key)?.as_ptr(),
                CString::new(value)?.as_ptr(),
            )
        })
    }
    fn send(&self, handle: usize, args: &[&str]) -> Result<()> {
        let strings: Vec<_> = args
            .iter()
            .map(|v| CString::new(*v))
            .collect::<std::result::Result<_, _>>()?;
        let mut args: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
        args.push(std::ptr::null());
        check(unsafe { (self.command)(handle as H, 1, args.as_ptr()) })
    }
    fn property(&self, handle: usize, name: &str) -> Option<String> {
        let value = unsafe { (self.property)(handle as H, CString::new(name).ok()?.as_ptr()) };
        if value.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe {
            (self.free)(value.cast());
        }
        Some(text)
    }
}
fn check(code: i32) -> Result<()> {
    if code < 0 {
        Err(format!("libmpv error {code}").into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Request {
    position: f64,
    at: Instant,
}
struct Shared {
    pending: Mutex<Option<Request>>,
    rendered: Mutex<Vec<(Instant, f64, Option<u32>)>>,
    rows: Mutex<Vec<serde_json::Value>>,
    ready: AtomicBool,
    quit: AtomicBool,
    error: Mutex<Option<String>>,
    properties: Mutex<serde_json::Value>,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            pending: Mutex::new(None),
            rendered: Mutex::new(Vec::new()),
            rows: Mutex::new(Vec::new()),
            ready: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            error: Mutex::new(None),
            properties: Mutex::new(serde_json::Value::Null),
        }
    }
}
fn control(api: Arc<Api>, handle: usize, source: PathBuf, shared: Arc<Shared>, ctx: egui::Context) {
    let run = || -> Result<()> {
        let began = Instant::now();
        api.send(handle, &["loadfile", &source.to_string_lossy(), "replace"])?;
        let mut active = Some((
            Request {
                position: 0.0,
                at: began,
            },
            began,
            0,
            true,
        ));
        let mut restarted = false;
        while !shared.quit.load(Ordering::Relaxed) {
            if active.is_none() {
                if let Some(request) = shared.pending.lock().unwrap().take() {
                    let before = shared.rendered.lock().unwrap().len();
                    let started = Instant::now();
                    api.send(
                        handle,
                        &[
                            "seek",
                            &format!("{:.6}", request.position),
                            "absolute+exact",
                        ],
                    )?;
                    active = Some((request, started, before, false));
                    restarted = false;
                }
            }
            let event = unsafe { &*(api.event)(handle as H, 0.01) };
            if event.error < 0 {
                return Err(format!("event {} failed: {}", event.id, event.error).into());
            }
            restarted |= event.id == 21;
            if let Some((request, started, before, loading)) = active {
                if started.elapsed() > Duration::from_secs(10) {
                    return Err("seek timeout".into());
                }
                let rendered = shared.rendered.lock().unwrap().get(before).copied();
                if restarted && let Some((painted, render_ms, decoded_frame)) = rendered {
                    let pts = api
                        .property(handle, "time-pos")
                        .and_then(|v| v.parse::<f64>().ok())
                        .ok_or("missing timestamp")?;
                    if (pts - request.position).abs() > 0.04 {
                        return Err(format!("seek mismatch {} -> {pts}", request.position).into());
                    }
                    if let Some(actual) = decoded_frame {
                        let expected = (request.position * 30.0).round() as u32;
                        if actual.abs_diff(expected) > 1 {
                            return Err(format!(
                                "pixel mismatch: requested {expected}, rendered {actual}"
                            )
                            .into());
                        }
                    }
                    shared.rows.lock().unwrap().push(serde_json::json!({
                        "position":request.position,"pts":pts,"loading":loading,"decoded_frame":decoded_frame,
                        "input_to_gl_ms":painted.duration_since(request.at).as_secs_f64()*1000.0,
                        "seek_to_gl_ms":painted.duration_since(started).as_secs_f64()*1000.0,"render_ms":render_ms,
                        "painted_seconds":painted.duration_since(began).as_secs_f64(),
                    }));
                    if loading {
                        *shared.properties.lock().unwrap() = serde_json::json!({"version":api.property(handle,"mpv-version"),"hwdec":api.property(handle,"hwdec-current")});
                        shared.ready.store(true, Ordering::Relaxed);
                    }
                    active = None;
                    ctx.request_repaint();
                }
            }
        }
        Ok(())
    };
    if let Err(error) = run() {
        *shared.error.lock().unwrap() = Some(error.to_string());
        ctx.request_repaint();
    }
}

unsafe extern "C" fn resolve(data: H, name: *const c_char) -> H {
    unsafe { (&*(data as *const Resolver))(CStr::from_ptr(name)) as H }
}
unsafe extern "C" fn wake(data: H) {
    unsafe {
        (&*(data as *const egui::Context)).request_repaint();
    }
}

struct Render {
    api: Arc<Api>,
    context: usize,
    fbo: glow::Framebuffer,
    texture: glow::Texture,
    _resolver: Box<Resolver>,
    _repaint: Box<egui::Context>,
    shared: Arc<Shared>,
    indexed: bool,
}
impl Render {
    fn new(
        cc: &eframe::CreationContext<'_>,
        api: Arc<Api>,
        handle: usize,
        shared: Arc<Shared>,
        indexed: bool,
    ) -> Result<Self> {
        let mut resolver = Box::new(
            cc.get_proc_address
                .clone()
                .ok_or("GL resolver unavailable")?,
        );
        let mut repaint = Box::new(cc.egui_ctx.clone());
        let gl = cc.gl.as_ref().ok_or("GL unavailable")?;
        let mut init = Init {
            get: resolve,
            data: (&mut *resolver as *mut Resolver).cast(),
        };
        let mut advanced = 1i32;
        let params = [
            Param {
                kind: 1,
                data: c"opengl".as_ptr() as H,
            },
            Param::value(2, &mut init),
            Param::value(10, &mut advanced),
            Param::end(),
        ];
        let mut context = std::ptr::null_mut();
        check(unsafe { (api.render_create)(&mut context, handle as H, params.as_ptr()) })?;
        unsafe {
            (api.render_callback)(context, wake, (&mut *repaint as *mut egui::Context).cast());
        }
        let (fbo, texture) = unsafe {
            let fbo = gl.create_framebuffer()?;
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                640,
                360,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            (fbo, texture)
        };
        Ok(Self {
            api,
            context: context as usize,
            fbo,
            texture,
            _resolver: resolver,
            _repaint: repaint,
            shared,
            indexed,
        })
    }
    fn paint(&mut self, info: egui::PaintCallbackInfo, painter: &egui_glow::Painter) {
        let gl = painter.gl();
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.use_program(None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if (self.api.render_update)(self.context as H) & 1 != 0 {
                let mut frame = FrameInfo::default();
                let _ = (self.api.render_info)(self.context as H, Param::value(11, &mut frame));
                let mut fbo = Fbo {
                    fbo: self.fbo.0.get() as i32,
                    w: 640,
                    h: 360,
                    format: glow::RGBA8 as i32,
                };
                let mut wait = 0i32;
                let mut flip = 1i32;
                let params = [
                    Param::value(3, &mut fbo),
                    Param::value(4, &mut flip),
                    Param::value(12, &mut wait),
                    Param::end(),
                ];
                let before = Instant::now();
                let result = (self.api.render)(self.context as H, params.as_ptr());
                let decoded_frame = if self.indexed && frame.flags & 1 != 0 && frame.flags & 2 == 0
                {
                    let mut pixels = [0u8; 640 * 4];
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
                    gl.read_pixels(
                        0,
                        350,
                        640,
                        1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixels)),
                    );
                    let white = |bit: usize| {
                        pixels[((bit as f64 + 0.5) * 32.0 / 3.0).round() as usize * 4] > 128
                    };
                    if !white(12) {
                        *self.shared.error.lock().unwrap() =
                            Some("missing pixel frame marker".into());
                    }
                    Some((0..9).fold(0, |value, bit| value | (u32::from(white(bit)) << bit)))
                } else {
                    None
                };
                let done = Instant::now();
                if result < 0 {
                    *self.shared.error.lock().unwrap() = Some(format!("render failed {result}"));
                }
                if frame.flags & 1 != 0 && frame.flags & 2 == 0 {
                    self.shared.rendered.lock().unwrap().push((
                        done,
                        done.duration_since(before).as_secs_f64() * 1000.0,
                        decoded_frame,
                    ));
                }
            }
            let viewport = info.viewport_in_pixels();
            let clip = info.clip_rect_in_pixels();
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, painter.intermediate_fbo());
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.blit_framebuffer(
                0,
                0,
                640,
                360,
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.left_px + viewport.width_px,
                viewport.from_bottom_px + viewport.height_px,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
        }
    }
    fn close(&mut self, gl: &glow::Context) {
        unsafe {
            (self.api.render_free)(self.context as H);
            self.context = 0;
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.texture);
        }
    }
}

struct App {
    api: Arc<Api>,
    handle: usize,
    render: Arc<Mutex<Render>>,
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
    report: PathBuf,
    start: Option<Instant>,
    opened: Instant,
    timeline: egui::Rect,
    last_position: Option<f64>,
    last_input: f64,
    inputs: usize,
    capture: bool,
    captured: bool,
}
impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        dll: PathBuf,
        source: PathBuf,
        report: PathBuf,
    ) -> Result<Self> {
        let api = Arc::new(unsafe { Api::load(&dll)? });
        let handle = unsafe { (api.create)() } as usize;
        if handle == 0 {
            return Err("mpv_create failed".into());
        }
        let hwdec = std::env::var("NOH_SCRUB_HWDEC").unwrap_or_else(|_| "auto-copy".into());
        for (name, value) in [
            ("config", "no"),
            ("vo", "libmpv"),
            ("audio", "no"),
            ("pause", "yes"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("terminal", "no"),
            ("load-scripts", "no"),
            ("sub-auto", "no"),
            ("osd-level", "0"),
            ("hwdec", hwdec.as_str()),
            ("vd-lavc-threads", "2"),
            ("demuxer-max-bytes", "32MiB"),
            ("demuxer-max-back-bytes", "16MiB"),
        ] {
            api.option(handle, name, value)?;
        }
        check(unsafe { (api.initialize)(handle as H) })?;
        let shared = Arc::new(Shared::default());
        let render = Arc::new(Mutex::new(Render::new(
            cc,
            api.clone(),
            handle,
            shared.clone(),
            source.to_string_lossy().contains("indexed"),
        )?));
        let worker = {
            let api = api.clone();
            let shared = shared.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || control(api, handle, source, shared, ctx))
        };
        Ok(Self {
            api,
            handle,
            render,
            shared,
            worker: Some(worker),
            report,
            start: None,
            opened: Instant::now(),
            timeline: egui::Rect::NOTHING,
            last_position: None,
            last_input: -1.0,
            inputs: 0,
            capture: false,
            captured: false,
        })
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        if self.start.is_none() && self.shared.ready.load(Ordering::Relaxed) {
            self.start = Some(Instant::now());
        }
        ui.heading("Persistent preview: native GL qualification");
        ui.label("Automated pointer sweep; rendering and seeking run independently.");
        let width = ui.available_width().min(800.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, width * 9.0 / 16.0), egui::Sense::hover());
        let renderer = self.render.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                renderer.lock().unwrap().paint(info, painter)
            })),
        });
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 40.0), egui::Sense::hover());
        self.timeline = rect;
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_gray(45));
        if let Some(pos) = response.hover_pos() {
            let frame = (((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * 299.0).floor();
            let position = f64::from(frame) / 30.0;
            ui.painter()
                .vline(pos.x, rect.y_range(), (2.0, egui::Color32::LIGHT_GREEN));
            if self.last_position != Some(position) && self.start.is_some() {
                *self.shared.pending.lock().unwrap() = Some(Request {
                    position,
                    at: Instant::now(),
                });
                unsafe {
                    (self.api.wakeup)(self.handle as H);
                }
                self.last_position = Some(position);
                self.inputs += 1;
            }
        }
        let count = self.shared.rows.lock().unwrap().len();
        ui.label(format!(
            "Inputs: {}   Finished frames: {count}   Target: {:?}",
            self.inputs, self.last_position
        ));
        let error = self.shared.error.lock().unwrap().clone();
        if let Some(error) = &error {
            ui.colored_label(egui::Color32::RED, error);
        }
        let elapsed = self.start.map_or(0.0, |s| s.elapsed().as_secs_f64());
        if elapsed > 13.0 && !self.capture {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.capture = true;
        }
        if self.captured || error.is_some() || self.opened.elapsed() > Duration::from_secs(30) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(8));
    }
    fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
        for event in &input.events {
            if let egui::Event::Screenshot { image, .. } = event {
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                image::save_buffer(
                    self.report.with_extension("png"),
                    &rgba,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                self.captured = true;
            }
        }
        if let Some(start) = self.start {
            let t = start.elapsed().as_secs_f64();
            if t - self.last_input >= 1.0 / 60.0 {
                let p = if t < 4.0 {
                    0.1 + 0.8 * t / 4.0
                } else if t < 8.0 {
                    0.9 - 0.8 * (t - 4.0) / 4.0
                } else if t < 12.0 {
                    0.1 + 0.8 * (1.0 - ((t - 8.0) % 1.0 * 2.0 - 1.0).abs())
                } else {
                    0.333
                };
                input.events.push(egui::Event::PointerMoved(egui::pos2(
                    self.timeline.left() + self.timeline.width() * p as f32,
                    self.timeline.center().y,
                )));
                self.last_input = t;
            }
        }
    }
    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        self.shared.quit.store(true, Ordering::Relaxed);
        unsafe {
            (self.api.wakeup)(self.handle as H);
        }
        if let Some(gl) = gl {
            self.render.lock().unwrap().close(gl);
        }
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        unsafe {
            (self.api.destroy)(self.handle as H);
        }
        let rows = self.shared.rows.lock().unwrap();
        let report = serde_json::json!({"measurement":"input to OpenGL render completion; not physical presentation","input_count":self.inputs,"properties":*self.shared.properties.lock().unwrap(),"error":*self.shared.error.lock().unwrap(),"screenshot":self.captured,"samples":*rows});
        std::fs::write(&self.report, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        println!(
            "{}",
            serde_json::json!({"report":self.report,"inputs":self.inputs,"frames":rows.len(),"error":report["error"]})
        );
    }
}
fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    assert_eq!(args.len(), 3, "DLL VIDEO REPORT.json");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([860.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "NOH scrub experiment",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc,
                args[0].clone(),
                args[1].clone(),
                args[2].clone(),
            )?))
        }),
    )
}
