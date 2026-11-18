//! Opt-in measurement: how fast libmpv repositions while it PLAYS a montage, the
//! clips one after another under one song. No window and no sound device
//! (`vo=null`, `ao=null`): it times decoding and resynchronisation, NOT display.
//! Usage: cargo run --profile qa --features gui --example playback_probe --
//!        DLL REPORT.json SONG SECONDS CLIP_SECONDS CLIP...
//! `PROBE_TARGETS_MS` lists the positions to jump to; `PROBE_OPTIONS` adds mpv
//! options (`key=value,key=value`).
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
type H = *mut c_void;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[repr(C)]
struct Event {
    id: i32,
    error: i32,
    userdata: u64,
    data: H,
}
const FILE_LOADED: i32 = 8;
const END_FILE: i32 = 7;
const SEEK: i32 = 20;
const PLAYBACK_RESTART: i32 = 21;

struct Api {
    _library: libloading::Library,
    create: unsafe extern "C" fn() -> H,
    initialize: unsafe extern "C" fn(H) -> i32,
    option: unsafe extern "C" fn(H, *const c_char, *const c_char) -> i32,
    command: unsafe extern "C" fn(H, *const *const c_char) -> i32,
    event: unsafe extern "C" fn(H, f64) -> *const Event,
    property: unsafe extern "C" fn(H, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(H),
    destroy: unsafe extern "C" fn(H),
    handle: H,
}
impl Api {
    unsafe fn load(path: &Path) -> Result<Self> {
        #[cfg(target_os = "windows")]
        let library: libloading::Library = unsafe {
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
        let create: unsafe extern "C" fn() -> H = sym!("mpv_create");
        let handle = unsafe { create() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        Ok(Self {
            create,
            initialize: sym!("mpv_initialize"),
            option: sym!("mpv_set_option_string"),
            command: sym!("mpv_command"),
            event: sym!("mpv_wait_event"),
            property: sym!("mpv_get_property_string"),
            free: sym!("mpv_free"),
            destroy: sym!("mpv_terminate_destroy"),
            handle,
            _library: library,
        })
    }
    fn option(&self, key: &str, value: &str) -> Result<()> {
        let code = unsafe {
            (self.option)(
                self.handle,
                CString::new(key)?.as_ptr(),
                CString::new(value)?.as_ptr(),
            )
        };
        if code < 0 {
            return Err(format!("option {key}={value}: libmpv error {code}").into());
        }
        Ok(())
    }
    fn send(&self, args: &[&str]) -> Result<()> {
        let strings: Vec<_> = args
            .iter()
            .map(|v| CString::new(*v))
            .collect::<std::result::Result<_, _>>()?;
        let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
        pointers.push(std::ptr::null());
        let code = unsafe { (self.command)(self.handle, pointers.as_ptr()) };
        if code < 0 {
            return Err(format!("command {args:?}: libmpv error {code}").into());
        }
        Ok(())
    }
    fn property(&self, name: &str) -> Option<String> {
        let value = unsafe { (self.property)(self.handle, CString::new(name).ok()?.as_ptr()) };
        if value.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.free)(value.cast()) };
        Some(text)
    }
    fn number(&self, name: &str) -> Option<f64> {
        self.property(name)?.parse().ok()
    }
    /// Waits for `wanted`; the time it took, or None after `limit`.
    fn wait(&self, wanted: i32, limit: Duration) -> Result<Option<Duration>> {
        let began = Instant::now();
        while began.elapsed() < limit {
            let event = unsafe { &*(self.event)(self.handle, 0.005) };
            if event.id == wanted {
                return Ok(Some(began.elapsed()));
            }
            if event.id == END_FILE {
                return Err("Playback ended or the file could not be played".into());
            }
        }
        Ok(None)
    }
    /// Lets playback run, discarding events.
    fn play_for(&self, time: Duration) {
        let began = Instant::now();
        while began.elapsed() < time {
            unsafe { (self.event)(self.handle, 0.01) };
        }
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.handle) };
        let _ = self.create;
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 6 {
        return Err("usage: DLL REPORT.json SONG SECONDS CLIP_SECONDS CLIP...".into());
    }
    let (dll, report, song) = (PathBuf::from(&args[0]), &args[1], &args[2]);
    let seconds: f64 = args[3].parse()?;
    let clip_seconds: f64 = args[4].parse()?;
    let clips = &args[5..];
    // One virtual file: the clips in order, repeated to the song's length.
    let mut edl = String::from("edl://");
    let mut at = 0.0;
    let mut index = 0;
    while at < seconds {
        let clip = &clips[index % clips.len()];
        let length = clip_seconds.min(seconds - at);
        edl.push_str(&format!("%{}%{clip},start=0,length={length};", clip.len()));
        at += length;
        index += 1;
    }
    let api = unsafe { Api::load(&dll)? };
    for (key, value) in [
        ("config", "no"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("vo", "null"),
        ("ao", "null"),
        ("idle", "yes"),
        ("keep-open", "yes"),
        ("hr-seek", "yes"),
    ] {
        api.option(key, value)?;
    }
    let extra = std::env::var("PROBE_OPTIONS").unwrap_or_default();
    for pair in extra.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or("PROBE_OPTIONS: key=value")?;
        api.option(key, value)?;
    }
    if unsafe { (api.initialize)(api.handle) } < 0 {
        return Err("mpv_initialize failed".into());
    }
    let began = Instant::now();
    api.send(&["loadfile", &edl])?;
    api.wait(FILE_LOADED, Duration::from_secs(30))?
        .ok_or("The montage did not load")?;
    let loaded_ms = began.elapsed().as_secs_f64() * 1000.0;
    api.send(&["audio-add", song, "select"])?;
    api.wait(PLAYBACK_RESTART, Duration::from_secs(30))?
        .ok_or("Playback did not start")?;
    let started_ms = began.elapsed().as_secs_f64() * 1000.0;
    api.play_for(Duration::from_millis(2000));
    let targets: Vec<u64> = std::env::var("PROBE_TARGETS_MS")
        .unwrap_or_else(|_| "5000,2000,9500,15000,35000,39500,1000".into())
        .split(',')
        .map(|t| t.trim().parse())
        .collect::<std::result::Result<_, _>>()?;
    let mut results = Vec::new();
    for target in targets {
        let from = api.number("time-pos");
        let asked = Instant::now();
        api.send(&[
            "seek",
            &format!("{:.3}", target as f64 / 1000.0),
            "absolute+exact",
        ])?;
        // The restart that answers this seek: the one after its own SEEK event.
        let restart = match api.wait(SEEK, Duration::from_secs(20))? {
            Some(sought) => api
                .wait(PLAYBACK_RESTART, Duration::from_secs(20))?
                .map(|restarted| sought + restarted),
            None => None,
        };
        let at_restart = api.number("time-pos");
        api.play_for(Duration::from_millis(1500));
        let later = api.number("time-pos");
        results.push(serde_json::json!({
            "target_ms": target,
            "from_ms": from.map(|s| (s * 1000.0).round()),
            "restart_ms": restart.map(|d| d.as_secs_f64() * 1000.0),
            "total_ms": asked.elapsed().as_secs_f64() * 1000.0 - 1500.0,
            "position_at_restart_ms": at_restart.map(|s| (s * 1000.0).round()),
            "position_1500_ms_later": later.map(|s| (s * 1000.0).round()),
        }));
    }
    let facts = [
        "mpv-version",
        "duration",
        "video-codec",
        "hwdec-current",
        "audio-codec-name",
        "width",
        "height",
        "frame-drop-count",
        "decoder-frame-drop-count",
        "avsync",
        "pause",
    ]
    .map(|name| (name, api.property(name)));
    let value = serde_json::json!({
        "options": extra,
        "segments": index,
        "loaded_ms": loaded_ms,
        "started_ms": started_ms,
        "facts": facts.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
        "results": results,
    });
    std::fs::write(report, serde_json::to_string_pretty(&value)?)?;
    println!("{}", serde_json::to_string(&value["facts"])?);
    for r in value["results"].as_array().unwrap() {
        println!(
            "{:>6} from {:>6} | restart {:>6.0} ms | at restart {:>6} | +1.5 s {:>6}",
            r["target_ms"],
            r["from_ms"],
            r["restart_ms"].as_f64().unwrap_or(-1.0),
            r["position_at_restart_ms"],
            r["position_1500_ms_later"],
        );
    }
    Ok(())
}
