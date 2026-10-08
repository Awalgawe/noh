//! Persistent silent scrubbing with bounded, source-level all-intra working copies.
//! All media IO and ordinary libmpv calls run on the controller; only render API
//! calls run on egui's GL thread. Exports never use these temporary files.
use crate::{
    inspection::FileStamp,
    preview::{ScrubPlan, scrub_plan},
    project::ProjectRequest,
};
use eframe::{egui, egui_glow, glow};
use glow::HasContext;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
type H = *mut c_void;
type Resolver = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Explicit runtime first; otherwise try the native bundle and system loader.
/// Loading remains optional: the GUI retains its frame-renderer fallback.
pub fn runtime_candidates() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("NOH_LIBMPV").filter(|p| !p.is_empty()) {
        return vec![path.into()];
    }
    let names: &[&str] = if cfg!(windows) {
        &["libmpv-2.dll"]
    } else if cfg!(target_os = "macos") {
        &["libmpv.2.dylib", "libmpv.dylib"]
    } else {
        &["libmpv.so.2", "libmpv.so.1", "libmpv.so"]
    };
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(folder) = exe.parent() {
            #[cfg(target_os = "macos")]
            paths.extend(
                names
                    .iter()
                    .map(|name| folder.join("../Frameworks").join(name)),
            );
            paths.extend(names.iter().map(|name| folder.join("bin").join(name)));
            #[cfg(target_os = "windows")]
            paths.splice(
                0..0,
                names
                    .iter()
                    .map(|name| folder.join("bin/preview").join(name)),
            );
        }
    }
    #[cfg(target_os = "macos")]
    for folder in ["/opt/homebrew/lib", "/usr/local/lib"] {
        paths.extend(names.iter().map(|name| PathBuf::from(folder).join(name)));
    }
    paths.retain(|path| path.is_file());
    // A bare soname lets the Unix loader handle multiarch paths and ldconfig.
    #[cfg(unix)]
    paths.extend(names.iter().map(PathBuf::from));
    paths
}

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
        #[cfg(target_os = "windows")]
        let library: libloading::Library = unsafe {
            // Resolve dependencies beside this DLL without mixing the export
            // closure into its library search path.
            libloading::os::windows::Library::load_with_flags(
                path.canonicalize()?,
                0x00000100 | 0x00001000, // DLL_LOAD_DIR | DEFAULT_DIRS
            )?
            .into()
        };
        #[cfg(not(target_os = "windows"))]
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
        self.send_id(handle, 1, args)
    }
    fn send_id(&self, handle: usize, id: u64, args: &[&str]) -> Result<()> {
        let strings: Vec<_> = args
            .iter()
            .map(|v| CString::new(*v))
            .collect::<std::result::Result<_, _>>()?;
        let mut args: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
        args.push(std::ptr::null());
        check(unsafe { (self.command)(handle as H, id, args.as_ptr()) })
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

struct Request {
    project: ProjectRequest,
    time_ms: u64,
    epoch: u64,
    at: Instant,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub epoch: u64,
    pub time_ms: u64,
    pub frame: u64,
    pub error: Option<String>,
    pub latency_ms: f64,
    pub decoded_frame: Option<u32>,
    pub source_seconds: f64,
    pub fallback: bool,
}
struct Shared {
    pending: Mutex<Option<Request>>,
    snapshot: Mutex<Snapshot>,
    dimensions: Mutex<(u32, u32)>,
    rendered: AtomicU64,
    epoch: AtomicU64,
    quit: AtomicBool,
    cancel: AtomicBool,
    render_closed: AtomicBool,
    error: Mutex<Option<String>>,
    decoded_frame: Mutex<Option<u32>>,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            pending: Mutex::new(None),
            snapshot: Mutex::new(Snapshot::default()),
            dimensions: Mutex::new((640, 360)),
            rendered: AtomicU64::new(0),
            epoch: AtomicU64::new(1),
            quit: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            render_closed: AtomicBool::new(false),
            error: Mutex::new(None),
            decoded_frame: Mutex::new(None),
        }
    }
}
struct Proxy {
    source: FileStamp,
    ffmpeg: PathBuf,
    directory: tempfile::TempDir,
    bytes: u64,
    // Actual presentation times, including VFR gaps; never infer tolerance
    // from the montage target's average frame rate.
    times: Vec<f64>,
}
impl Proxy {
    fn path(&self) -> PathBuf {
        self.directory.path().join("preview.mp4")
    }
}
const PROXY_BYTES: u64 = 128 * 1024 * 1024;
fn proxy(
    plan: &ScrubPlan,
    shared: &Shared,
    cache: &mut std::collections::VecDeque<Arc<Proxy>>,
) -> Result<Arc<Proxy>> {
    if let Some(index) = cache
        .iter()
        .position(|p| p.source == plan.source && p.ffmpeg == plan.ffmpeg)
    {
        let item = cache.remove(index).unwrap();
        cache.push_back(item.clone());
        return Ok(item);
    }
    let directory = tempfile::Builder::new().prefix("noh-scrub-").tempdir()?;
    let path = directory.path().join("preview.mp4");
    let stderr = std::fs::File::create(directory.path().join("encoder.log"))?;
    let mut child=crate::command(&plan.ffmpeg).args(["-hide_banner","-nostdin","-v","error","-n","-threads","2","-ss",&format!("{:.9}",plan.origin),"-i"])
        .arg(&plan.source.resolved).args(["-map","0:v:0","-an","-sn","-dn","-vf","setpts=PTS-STARTPTS,scale=w='max(2,trunc(min(640,640*dar)/2)*2)':h='max(2,trunc(min(640,640/dar)/2)*2)',setsar=1","-fps_mode","passthrough","-c:v","libx264","-threads","2","-preset","fast","-crf","18","-g","1","-pix_fmt","yuv420p","-fs",&PROXY_BYTES.to_string()])
        .arg(&path).stdout(Stdio::null()).stderr(stderr).spawn()?;
    let began = Instant::now();
    let status = loop {
        if shared.quit.load(Ordering::Relaxed)
            || shared.cancel.load(Ordering::Relaxed)
            || began.elapsed() > Duration::from_secs(60)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Preview preparation cancelled or timed out".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.into());
            }
        }
    };
    if !status.success() {
        return Err("Preview working copy could not be prepared".into());
    }
    let bytes = std::fs::metadata(&path)?.len();
    if bytes >= PROXY_BYTES - 1024 * 1024 {
        return Err("Preview working copy exceeds its size limit".into());
    }
    if FileStamp::read(&plan.source.path)? != plan.source {
        return Err("Preview input changed".into());
    }
    let times = proxy_times(&plan.ffmpeg, &path, &shared.cancel)?;
    let proxy = Arc::new(Proxy {
        source: plan.source.clone(),
        ffmpeg: plan.ffmpeg.clone(),
        directory,
        bytes,
        times,
    });
    cache.push_back(proxy.clone());
    trim_proxies(cache);
    Ok(proxy)
}

fn proxy_times(
    ffmpeg: &std::path::Path,
    path: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<Vec<f64>> {
    let mut command = crate::command(ffmpeg);
    command
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-c:v",
            "copy",
            "-f",
            "framehash",
            "-hash",
            "adler32",
            "-",
        ]);
    let mut time_base = None;
    let mut times = Vec::new();
    let (status, _) =
        crate::process::stream_interruptible(command, Duration::from_secs(15), cancel, |line| {
            if let Some(value) = line.strip_prefix("#tb 0: ") {
                let (num, den) = value.split_once('/').ok_or("Invalid preview time base")?;
                let tb = num.parse::<f64>()? / den.parse::<f64>()?;
                if !tb.is_finite() || tb <= 0.0 {
                    return Err("Invalid preview time base".into());
                }
                time_base = Some(tb);
            } else if !line.starts_with('#') && !line.trim().is_empty() {
                if times.len() >= 1_000_000 {
                    return Err("Preview frame index exceeds its size limit".into());
                }
                let pts = line
                    .split(',')
                    .nth(2)
                    .ok_or("Missing preview PTS")?
                    .trim()
                    .parse::<i64>()?;
                let seconds = pts as f64 * time_base.ok_or("Missing preview time base")?;
                if !seconds.is_finite() || times.last().is_some_and(|last| seconds <= *last) {
                    return Err("Invalid preview frame order".into());
                }
                times.push(seconds);
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    if !status.success() || times.is_empty() {
        return Err("Preview frame index unavailable".into());
    }
    Ok(times)
}

fn seek_position(times: &[f64], requested: f64) -> f64 {
    // Exact seek presents the first available image at/after the requested PTS.
    // Seek directly to it; at EOF retain the last representable image.
    let index = times
        .partition_point(|pts| *pts < requested - 1e-7)
        .min(times.len() - 1);
    times[index]
}

fn seek_timed_out(elapsed: Duration) -> bool {
    elapsed > Duration::from_secs(5)
}
fn trim_proxies(cache: &mut std::collections::VecDeque<Arc<Proxy>>) {
    // Count alone would constantly regenerate a three-clip montage. Keep small
    // sources together while bounding total disk use and metadata entries.
    let mut bytes: u64 = cache.iter().map(|p| p.bytes).sum();
    while cache.len() > 16 || bytes > 256 * 1024 * 1024 {
        if let Some(old) = cache.pop_front() {
            bytes = bytes.saturating_sub(old.bytes);
        } else {
            break;
        }
    }
}
fn control(api: Arc<Api>, handle: usize, shared: Arc<Shared>, ctx: egui::Context) {
    let mut cache = std::collections::VecDeque::new();
    let mut source = PathBuf::new();
    let mut filter = String::new();
    let mut workspace = None;
    let mut active_proxy = None;
    let mut loading_proxy = None;
    let mut captions: Option<PathBuf> = None;
    let mut caption_delay = f64::NAN;
    while !shared.quit.load(Ordering::Relaxed) {
        // Release the mailbox before waiting for an event. A temporary guard
        // in the let-else scrutinee otherwise spans the blocking else branch.
        let pending = shared.pending.lock().unwrap().take();
        let Some(request) = pending else {
            unsafe {
                (api.event)(handle as H, 0.1);
            }
            continue;
        };
        shared.cancel.store(false, Ordering::Relaxed);
        if request.epoch != shared.epoch.load(Ordering::Acquire) {
            continue;
        }
        let epoch = request.epoch;
        let mut requested_time = request.time_ms;
        let result = (|| -> Result<u64> {
            let mut plan = scrub_plan(&request.project, request.time_ms, &shared.cancel)?;
            let working = proxy(&plan, &shared, &mut cache)?;
            if request.epoch != shared.epoch.load(Ordering::Acquire) {
                return Err("Stale preview".into());
            }
            // Preparing a working copy can take longer than a pointer movement.
            // Consume only the newest target in this generation before seeking.
            let latest = take_generation(&mut shared.pending.lock().unwrap(), request.epoch);
            let request = latest.unwrap_or(request);
            requested_time = request.time_ms;
            plan = scrub_plan(&request.project, request.time_ms, &shared.cancel)?;
            // A newly selected source must have its own working copy.
            let working = if plan.source == working.source {
                working
            } else {
                proxy(&plan, &shared, &mut cache)?
            };
            let path = working.path();
            let seek_seconds = seek_position(&working.times, plan.seconds);
            while unsafe { (*(api.event)(handle as H, 0.0)).id } != 0 {}
            let mut before = shared.rendered.load(Ordering::Acquire);
            *shared.dimensions.lock().unwrap() = (plan.width, plan.height);
            if filter != plan.filter {
                api.send(handle, &["set", "vf", &format!("lavfi=[{}]", plan.filter)])?;
                filter = plan.filter.clone();
            }
            let loading = source != path;
            let mut load_captions =
                plan.captions.is_some() && (loading || captions != plan.captions);
            if !caption_delay.is_finite() || (caption_delay - plan.caption_delay).abs() > 1e-7 {
                api.send(
                    handle,
                    &["set", "sub-delay", &format!("{:.9}", plan.caption_delay)],
                )?;
                caption_delay = plan.caption_delay;
            }
            if load_captions {
                api.send(
                    handle,
                    &[
                        "set",
                        "sub-fonts-dir",
                        &plan.workspace.path().join("fonts").to_string_lossy(),
                    ],
                )?;
                if !loading && captions.is_some() {
                    api.send(handle, &["sub-remove"])?;
                }
            } else if !loading && plan.captions.is_none() && captions.is_some() {
                api.send(handle, &["sub-remove"])?;
                captions = None;
            }
            if loading {
                captions = None;
                api.send(
                    handle,
                    &[
                        "loadfile",
                        &path.to_string_lossy(),
                        "replace",
                        "-1",
                        &format!("start={seek_seconds:.9}"),
                    ],
                )?;
                // Hold both demuxer inputs until FILE_LOADED confirms the old
                // one is released, even if a later request supersedes this seek.
                loading_proxy = Some(working.clone());
                source = path;
            } else {
                if load_captions {
                    api.send_id(
                        handle,
                        42,
                        &[
                            "sub-add",
                            &plan.captions.as_ref().unwrap().to_string_lossy(),
                            "cached",
                        ],
                    )?;
                    load_captions = false;
                }
                api.send(
                    handle,
                    &["seek", &format!("{seek_seconds:.9}"), "absolute+exact"],
                )?;
            }
            workspace = Some(plan.workspace);
            let began = Instant::now();
            let mut restarted = false;
            let mut file_loaded = !loading;
            let mut subtitles_ready =
                plan.captions.is_none() || (!loading && captions == plan.captions);
            loop {
                if shared.quit.load(Ordering::Relaxed) || shared.cancel.load(Ordering::Relaxed) {
                    return Err("Preview cancelled".into());
                }
                // Complete the async file/subtitle transition before yielding:
                // the next request can then safely reuse source/captions state.
                if file_loaded
                    && subtitles_ready
                    && shared
                        .pending
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|r| r.epoch == epoch && r.time_ms != request.time_ms)
                {
                    return Ok(epoch);
                }
                if seek_timed_out(began.elapsed()) {
                    return Err("Preview seek timed out".into());
                }
                let event = unsafe { &*(api.event)(handle as H, 0.005) };
                if let Some(error) = shared.error.lock().unwrap().take() {
                    return Err(error.into());
                }
                if event.error < 0 {
                    return Err(format!("Preview decoder error {}", event.error).into());
                }
                if event.id == 8 {
                    file_loaded = true;
                    if let Some(proxy) = loading_proxy.take() {
                        active_proxy = Some(proxy);
                    }
                }
                if event.id == 8 && load_captions {
                    api.send_id(
                        handle,
                        42,
                        &[
                            "sub-add",
                            &plan.captions.as_ref().unwrap().to_string_lossy(),
                            "cached",
                        ],
                    )?;
                    load_captions = false;
                }
                if event.id == 5 && event.userdata == 42 {
                    subtitles_ready = true;
                    captions = plan.captions.clone();
                    before = shared.rendered.load(Ordering::Acquire);
                    restarted = false;
                    api.send(
                        handle,
                        &["seek", &format!("{seek_seconds:.9}"), "absolute+exact"],
                    )?;
                }
                restarted |= event.id == 21;
                let frame = shared.rendered.load(Ordering::Acquire);
                if subtitles_ready && restarted && frame > before {
                    let pts = api
                        .property(handle, "time-pos")
                        .and_then(|v| v.parse::<f64>().ok())
                        .ok_or("Missing preview timestamp")?;
                    if !pts.is_finite() || (pts - seek_seconds).abs() > 0.001 {
                        continue;
                    }
                    *shared.snapshot.lock().unwrap() = Snapshot {
                        epoch: request.epoch,
                        time_ms: request.time_ms,
                        frame,
                        error: None,
                        latency_ms: request.at.elapsed().as_secs_f64() * 1000.0,
                        decoded_frame: *shared.decoded_frame.lock().unwrap(),
                        source_seconds: seek_seconds,
                        fallback: false,
                    };
                    active_proxy = Some(working);
                    ctx.request_repaint();
                    return Ok(request.epoch);
                }
            }
        })();
        if let Err(error) = result {
            source = PathBuf::new();
            captions = None;
            filter.clear();
            if epoch != shared.epoch.load(Ordering::Acquire) {
                continue;
            }
            // The UI retains the exact existing renderer as a fallback.
            *shared.snapshot.lock().unwrap() = Snapshot {
                epoch,
                time_ms: requested_time,
                fallback: error.downcast_ref::<crate::preview::PreviewError>()
                    == Some(&crate::preview::PreviewError::StillImage),
                error: Some(error.to_string()),
                ..Default::default()
            };
            ctx.request_repaint();
        }
    }
    // Keep the active file alive until libmpv releases its demuxer.
    api.send(handle, &["stop"]).ok();
    while !shared.render_closed.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(1));
    }
    unsafe {
        (api.destroy)(handle as H);
    }
    drop(workspace);
    drop(active_proxy);
    drop(loading_proxy);
    drop(cache);
}
fn take_generation(pending: &mut Option<Request>, epoch: u64) -> Option<Request> {
    if pending.as_ref().is_some_and(|r| r.epoch == epoch) {
        pending.take()
    } else {
        None
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
    probe_pixels: bool,
}
impl Render {
    fn new(
        cc: &eframe::CreationContext<'_>,
        api: Arc<Api>,
        handle: usize,
        shared: Arc<Shared>,
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
        let resources = (|| -> Result<_> {
            unsafe {
                let fbo = gl.create_framebuffer()?;
                let texture = match gl.create_texture() {
                    Ok(t) => t,
                    Err(e) => {
                        gl.delete_framebuffer(fbo);
                        return Err(e.into());
                    }
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    640,
                    640,
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
                let complete =
                    gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.bind_texture(glow::TEXTURE_2D, None);
                if !complete {
                    gl.delete_texture(texture);
                    gl.delete_framebuffer(fbo);
                    return Err("Preview framebuffer unavailable".into());
                }
                Ok((fbo, texture))
            }
        })();
        let (fbo, texture) = match resources {
            Ok(r) => r,
            Err(e) => {
                unsafe {
                    (api.render_free)(context);
                }
                return Err(e);
            }
        };
        Ok(Self {
            api,
            context: context as usize,
            fbo,
            texture,
            _resolver: resolver,
            _repaint: repaint,
            shared,
            probe_pixels: std::env::var("NOH_SCRUB_FRAME_PROBE").as_deref() == Ok("1"),
        })
    }
    fn paint(
        &mut self,
        info: egui::PaintCallbackInfo,
        painter: &egui_glow::Painter,
        visible: bool,
        corner_radius: f32,
    ) {
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
                let (width, height) = *self.shared.dimensions.lock().unwrap();
                let mut fbo = Fbo {
                    fbo: self.fbo.0.get() as i32,
                    w: width as i32,
                    h: height as i32,
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
                let result = (self.api.render)(self.context as H, params.as_ptr());
                if result < 0 {
                    *self.shared.error.lock().unwrap() = Some(format!("render failed {result}"));
                }
                if frame.flags & 1 != 0 && frame.flags & 2 == 0 {
                    if self.probe_pixels && width == 640 && height == 360 {
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
                        *self.shared.decoded_frame.lock().unwrap() = white(12)
                            .then(|| (0..9).fold(0, |n, bit| n | (u32::from(white(bit)) << bit)));
                    }
                    self.shared.rendered.fetch_add(1, Ordering::Release);
                }
            }
            if !visible {
                gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                return;
            }
            let (width, height) = *self.shared.dimensions.lock().unwrap();
            let viewport = info.viewport_in_pixels();
            let clip = info.clip_rect_in_pixels();
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, painter.intermediate_fbo());
            gl.enable(glow::SCISSOR_TEST);
            let blit = || {
                gl.blit_framebuffer(
                    0,
                    0,
                    width as i32,
                    height as i32,
                    viewport.left_px,
                    viewport.from_bottom_px,
                    viewport.left_px + viewport.width_px,
                    viewport.from_bottom_px + viewport.height_px,
                    glow::COLOR_BUFFER_BIT,
                    glow::LINEAR,
                )
            };
            // Clip the same GPU copy to the monitor's rounded corners. A wide
            // middle band and at most 2*radius narrow bands avoid a CPU image.
            let radius = (corner_radius * info.pixels_per_point)
                .round()
                .min(viewport.width_px.min(viewport.height_px) as f32 / 2.0)
                as i32;
            let band = |left: i32, bottom: i32, w: i32, h: i32| {
                let x = left.max(clip.left_px);
                let y = bottom.max(clip.from_bottom_px);
                let right = (left + w).min(clip.left_px + clip.width_px);
                let top = (bottom + h).min(clip.from_bottom_px + clip.height_px);
                if right > x && top > y {
                    gl.scissor(x, y, right - x, top - y);
                    blit();
                }
            };
            band(
                viewport.left_px,
                viewport.from_bottom_px + radius,
                viewport.width_px,
                viewport.height_px - 2 * radius,
            );
            for row in 0..radius {
                let d = radius as f32 - row as f32 - 0.5;
                let inset = (radius as f32 - ((radius * radius) as f32 - d * d).max(0.0).sqrt())
                    .ceil() as i32;
                for y in [
                    viewport.from_bottom_px + row,
                    viewport.from_bottom_px + viewport.height_px - 1 - row,
                ] {
                    band(
                        viewport.left_px + inset,
                        y,
                        viewport.width_px - 2 * inset,
                        1,
                    );
                }
            }
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

/// Created and closed on the native GL thread; normal client calls are off-thread.
pub struct Scrubber {
    api: Arc<Api>,
    handle: usize,
    render: Arc<Mutex<Render>>,
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Scrubber {
    pub fn new(cc: &eframe::CreationContext<'_>, runtime: PathBuf) -> Result<Self> {
        let api = Arc::new(unsafe { Api::load(&runtime)? });
        let handle = unsafe { (api.create)() } as usize;
        if handle == 0 {
            return Err("mpv_create failed".into());
        }
        let prepare = (|| -> Result<Render> {
            for (key, value) in [
                ("config", "no"),
                ("vo", "libmpv"),
                ("audio", "no"),
                ("pause", "yes"),
                ("idle", "yes"),
                ("keep-open", "yes"),
                ("terminal", "no"),
                ("load-scripts", "no"),
                ("sub-auto", "no"),
                ("sub-ass-override", "no"),
                ("sub-font-provider", "none"),
                ("osd-level", "0"),
                // Small all-intra working copies decode cheaply on the CPU.
                // Hardware decoder setup stalls scrubbing across source files;
                // presentation still uses the native OpenGL render context.
                ("hwdec", "no"),
                ("vd-lavc-threads", "2"),
                ("demuxer-max-bytes", "16MiB"),
                ("demuxer-max-back-bytes", "8MiB"),
                ("keepaspect", "no"),
            ] {
                api.option(handle, key, value)?;
            }
            check(unsafe { (api.initialize)(handle as H) })?;
            Render::new(cc, api.clone(), handle, Arc::new(Shared::default()))
        })();
        let render = match prepare {
            Ok(r) => r,
            Err(e) => {
                unsafe {
                    (api.destroy)(handle as H);
                }
                return Err(e);
            }
        };
        let shared = render.shared.clone();
        let worker = {
            let api = api.clone();
            let shared = shared.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || control(api, handle, shared, ctx))
        };
        Ok(Self {
            api,
            handle,
            shared,
            render: Arc::new(Mutex::new(render)),
            worker: Some(worker),
        })
    }
    pub fn epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot.lock().unwrap().clone()
    }
    pub fn request(&self, project: ProjectRequest, time_ms: u64) {
        *self.shared.pending.lock().unwrap() = Some(Request {
            project,
            time_ms,
            epoch: self.epoch(),
            at: Instant::now(),
        });
        unsafe {
            (self.api.wakeup)(self.handle as H);
        }
    }
    pub fn invalidate(&self) {
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.pending.lock().unwrap().take();
    }
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, visible: bool, corner_radius: f32) {
        let render = self.render.clone();
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                render
                    .lock()
                    .unwrap()
                    .paint(info, painter, visible, corner_radius)
            })),
        });
    }
    /// Service decoder presentation even when the monitor is scrolled out of
    /// view. A clipped egui callback is otherwise skipped, stalling a seek.
    pub fn service(&self, ctx: &egui::Context) {
        let render = self.render.clone();
        let rect = egui::Rect::from_min_size(ctx.viewport_rect().min, egui::vec2(1.0, 1.0));
        ctx.layer_painter(egui::LayerId::background())
            .add(egui::PaintCallback {
                rect,
                callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                    render.lock().unwrap().paint(info, painter, false, 0.0)
                })),
            });
    }
    pub fn close(&mut self, gl: Option<&glow::Context>) {
        if self.worker.is_none() {
            return;
        }
        self.shared.quit.store(true, Ordering::Relaxed);
        self.shared.cancel.store(true, Ordering::Relaxed);
        unsafe {
            (self.api.wakeup)(self.handle as H);
        }
        if let Some(gl) = gl {
            self.render.lock().unwrap().close(gl);
        }
        self.shared.render_closed.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_seek_uses_representable_pts_including_low_rate_vfr_and_eof() {
        assert_eq!(seek_position(&[0.0, 0.5, 1.0], 0.2), 0.5);
        assert_eq!(
            seek_position(&[0.0, 0.04, 0.08, 0.12, 0.16, 0.2, 0.24], 0.2),
            0.2
        );
        assert_eq!(seek_position(&[0.0, 0.04, 0.7, 0.73], 0.2), 0.7);
        assert_eq!(seek_position(&[0.0, 0.5], 0.99), 0.5);
        let ntsc = 1001.0 / 30000.0;
        assert_eq!(seek_position(&[0.0, ntsc, 2.0 * ntsc], ntsc + 1e-9), ntsc);
    }
    #[test]
    fn seek_deadline_does_not_depend_on_restart_or_a_rendered_frame() {
        assert!(!seek_timed_out(Duration::from_secs(4)));
        assert!(seek_timed_out(Duration::from_millis(5001)));
    }
    fn request(epoch: u64, time_ms: u64) -> Request {
        Request {
            epoch,
            time_ms,
            at: Instant::now(),
            project: ProjectRequest {
                montage: crate::engine::ExportRequest {
                    items: vec!["source.mp4".into()],
                    wav: "song.wav".into(),
                    output: "out.mp4".into(),
                    ffmpeg: "ffmpeg".into(),
                    fade_in: 0.0,
                    fade_out: 0.0,
                    partial_fades: true,
                    preview: true,
                    clip_audio: false,
                    force_encode: false,
                },
                short: None,
                captions: None,
            },
        }
    }
    #[test]
    fn an_obsolete_decoder_cannot_consume_a_new_projects_pending_target() {
        let mut pending = Some(request(2, 7000));
        assert!(take_generation(&mut pending, 1).is_none());
        assert_eq!(pending.as_ref().unwrap().time_ms, 7000);
        assert_eq!(take_generation(&mut pending, 2).unwrap().time_ms, 7000);
        assert!(pending.is_none());
    }
    #[test]
    fn an_active_working_copy_survives_lru_eviction_until_decoder_release() {
        let input = tempfile::NamedTempFile::new().unwrap();
        let item = Arc::new(Proxy {
            source: FileStamp::read(input.path()).unwrap(),
            ffmpeg: "ffmpeg".into(),
            directory: tempfile::tempdir().unwrap(),
            bytes: 0,
            times: vec![0.0],
        });
        let path = item.directory.path().to_path_buf();
        let mut cache = std::collections::VecDeque::from([item.clone()]);
        cache.pop_front();
        assert!(path.exists());
        drop(item);
        assert!(!path.exists());
    }
    #[test]
    fn cache_keeps_small_multi_clip_projects_but_enforces_count_and_bytes() {
        let input = tempfile::NamedTempFile::new().unwrap();
        let source = FileStamp::read(input.path()).unwrap();
        let make = |bytes| {
            Arc::new(Proxy {
                source: source.clone(),
                ffmpeg: "ffmpeg".into(),
                directory: tempfile::tempdir().unwrap(),
                bytes,
                times: vec![0.0],
            })
        };
        let mut cache = std::collections::VecDeque::from([
            make(4 * 1024 * 1024),
            make(4 * 1024 * 1024),
            make(4 * 1024 * 1024),
        ]);
        trim_proxies(&mut cache);
        assert_eq!(cache.len(), 3);
        cache.push_back(make(127 * 1024 * 1024));
        cache.push_back(make(127 * 1024 * 1024));
        trim_proxies(&mut cache);
        assert_eq!(cache.len(), 2);
        cache.clear();
        for _ in 0..20 {
            cache.push_back(make(1));
        }
        trim_proxies(&mut cache);
        assert_eq!(cache.len(), 16);
    }
}
