//! Opt-in measurement: how fast VLC (libvlc 3) repositions while it PLAYS one
//! file. No window: pictures are delivered to memory at 640x640, sound goes to
//! VLC's dummy output. It times the request to the first picture VLC delivers
//! at the new position, NOT a window on screen.
//! Usage: cargo run --profile qa --features gui --example vlc_probe --
//!        LIBVLC_DLL REPORT.json FILE [SONG_URL]
//! `PROBE_TARGETS_MS` lists the positions to jump to; `PROBE_OPTIONS` adds
//! libvlc arguments separated by spaces.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
type H = *mut c_void;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const EDGE: u32 = 640;
/// VLC's decoder logs this for the first picture it lets through after a seek;
/// that picture is then shown at once (`src/input/decoder.c`).
const FIRST_PICTURE: &str = "Received first picture";

#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Display,
    Log(String),
}
struct Shared {
    marks: Mutex<Vec<(Instant, Mark)>>,
    pixels: *mut u8,
}
unsafe extern "C" fn lock(opaque: H, planes: *mut H) -> H {
    unsafe {
        *planes = (*(opaque as *const Shared)).pixels.cast();
    }
    std::ptr::null_mut()
}
unsafe extern "C" fn unlock(_: H, _: H, _: *const H) {}
unsafe extern "C" fn display(opaque: H, _: H) {
    let shared = unsafe { &*(opaque as *const Shared) };
    shared
        .marks
        .lock()
        .unwrap()
        .push((Instant::now(), Mark::Display));
}
/// Only the format string is read: the messages used here take no argument.
unsafe extern "C" fn log(data: H, _level: c_int, _context: H, format: *const c_char, _args: H) {
    if format.is_null() {
        return;
    }
    let shared = unsafe { &*(data as *const Shared) };
    let text = unsafe { CStr::from_ptr(format) }
        .to_string_lossy()
        .into_owned();
    let mut marks = shared.marks.lock().unwrap();
    if marks.len() < 2_000_000 {
        marks.push((Instant::now(), Mark::Log(text)));
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: LIBVLC_DLL REPORT.json FILE [SONG_URL]".into());
    }
    let dll = Path::new(&args[0]).canonicalize()?;
    let library: libloading::Library = unsafe {
        #[cfg(target_os = "windows")]
        {
            libloading::os::windows::Library::load_with_flags(
                &dll,
                0x00000100 | 0x00001000, // DLL_LOAD_DIR | DEFAULT_DIRS
            )?
            .into()
        }
        #[cfg(not(target_os = "windows"))]
        {
            libloading::Library::new(&dll)?
        }
    };
    macro_rules! sym {
        ($name:literal, $kind:ty) => {
            unsafe { *library.get::<$kind>(concat!($name, "\0").as_bytes())? }
        };
    }
    let new = sym!(
        "libvlc_new",
        unsafe extern "C" fn(c_int, *const *const c_char) -> H
    );
    let log_set = sym!(
        "libvlc_log_set",
        unsafe extern "C" fn(H, unsafe extern "C" fn(H, c_int, H, *const c_char, H), H)
    );
    let media_new = sym!(
        "libvlc_media_new_path",
        unsafe extern "C" fn(H, *const c_char) -> H
    );
    let slaves_add = sym!(
        "libvlc_media_slaves_add",
        unsafe extern "C" fn(H, c_int, u32, *const c_char) -> c_int
    );
    let player_new = sym!(
        "libvlc_media_player_new_from_media",
        unsafe extern "C" fn(H) -> H
    );
    type Lock = unsafe extern "C" fn(H, *mut H) -> H;
    type Unlock = unsafe extern "C" fn(H, H, *const H);
    type Display = unsafe extern "C" fn(H, H);
    let set_callbacks = sym!(
        "libvlc_video_set_callbacks",
        unsafe extern "C" fn(H, Lock, Unlock, Display, H)
    );
    let set_format = sym!(
        "libvlc_video_set_format",
        unsafe extern "C" fn(H, *const c_char, u32, u32, u32)
    );
    let play = sym!("libvlc_media_player_play", unsafe extern "C" fn(H) -> c_int);
    let stop = sym!("libvlc_media_player_stop", unsafe extern "C" fn(H));
    let set_time = sym!("libvlc_media_player_set_time", unsafe extern "C" fn(H, i64));
    let get_time = sym!(
        "libvlc_media_player_get_time",
        unsafe extern "C" fn(H) -> i64
    );
    let version = sym!(
        "libvlc_get_version",
        unsafe extern "C" fn() -> *const c_char
    );

    let extra = std::env::var("PROBE_OPTIONS").unwrap_or_default();
    let mut options = vec![
        "--no-video-title-show".to_owned(),
        "--aout=adummy".into(),
        "--no-sub-autodetect-file".into(),
    ];
    options.extend(extra.split_whitespace().map(str::to_owned));
    let strings: Vec<CString> = options
        .iter()
        .map(|o| CString::new(o.as_str()))
        .collect::<std::result::Result<_, _>>()?;
    let pointers: Vec<*const c_char> = strings.iter().map(|s| s.as_ptr()).collect();
    let instance = unsafe { new(pointers.len() as c_int, pointers.as_ptr()) };
    if instance.is_null() {
        return Err("libvlc_new failed".into());
    }
    let pixels = vec![0u8; (EDGE * EDGE * 4) as usize].leak().as_mut_ptr();
    let shared: &'static Shared = Box::leak(Box::new(Shared {
        marks: Mutex::new(Vec::new()),
        pixels,
    }));
    let opaque = shared as *const Shared as H;
    unsafe { log_set(instance, log, opaque) };
    let media = unsafe { media_new(instance, CString::new(args[2].as_str())?.as_ptr()) };
    if media.is_null() {
        return Err("The file could not be opened".into());
    }
    if let Some(song) = args.get(3) {
        // An audio slave, at the priority VLC gives a file the user adds.
        if unsafe { slaves_add(media, 1, 4, CString::new(song.as_str())?.as_ptr()) } != 0 {
            return Err("The song could not be added".into());
        }
    }
    let player = unsafe { player_new(media) };
    unsafe {
        set_callbacks(player, lock, unlock, display, opaque);
        set_format(player, c"RV32".as_ptr(), EDGE, EDGE, EDGE * 4);
    }
    let shown_after = |since: Instant, limit: Duration| -> Option<Duration> {
        let began = Instant::now();
        while began.elapsed() < limit {
            let marks = shared.marks.lock().unwrap();
            if let Some((at, _)) = marks
                .iter()
                .find(|(at, mark)| *at >= since && *mark == Mark::Display)
            {
                return Some(at.duration_since(since));
            }
            drop(marks);
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    };
    let began = Instant::now();
    if unsafe { play(player) } != 0 {
        return Err("Playback could not start".into());
    }
    let started = shown_after(began, Duration::from_secs(30)).ok_or("No picture was delivered")?;
    std::thread::sleep(Duration::from_millis(2000));
    let targets: Vec<i64> = std::env::var("PROBE_TARGETS_MS")
        .unwrap_or_else(|_| "5000,2000,9500,15000,35000,39500,1000".into())
        .split(',')
        .map(|t| t.trim().parse())
        .collect::<std::result::Result<_, _>>()?;
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut results = Vec::new();
    let mut seen = std::collections::BTreeMap::<String, u32>::new();
    for target in targets {
        let from = unsafe { get_time(player) };
        let asked = Instant::now();
        unsafe { set_time(player, target) };
        // The first picture VLC lets through after this seek, then its delivery.
        let limit = Duration::from_secs(20);
        let mut first = None;
        while asked.elapsed() < limit && first.is_none() {
            first = shared
                .marks
                .lock()
                .unwrap()
                .iter()
                .find(|(at, mark)| {
                    *at >= asked && matches!(mark, Mark::Log(text) if text == FIRST_PICTURE)
                })
                .map(|(at, _)| *at);
            std::thread::sleep(Duration::from_millis(1));
        }
        let shown = first.and_then(|at| shown_after(at, limit).map(|d| at + d));
        let position = unsafe { get_time(player) };
        std::thread::sleep(Duration::from_millis(1500));
        let later = unsafe { get_time(player) };
        let marks = shared.marks.lock().unwrap();
        let end = shown.unwrap_or(asked + limit);
        // Pictures still delivered between the request and the new position.
        let stale = marks
            .iter()
            .filter(|(at, mark)| *at >= asked && *at < end && *mark == Mark::Display)
            .count();
        for (at, mark) in marks.iter() {
            if let Mark::Log(text) = mark
                && *at >= asked
                && *at <= end
            {
                *seen.entry(text.trim().to_owned()).or_default() += 1;
            }
        }
        results.push(serde_json::json!({
            "target_ms": target,
            "from_ms": from,
            "first_picture_ms": first.map(|at| ms(at.duration_since(asked))),
            "shown_ms": shown.map(|at| ms(at.duration_since(asked))),
            "pictures_before": stale,
            "position_after_ms": position,
            "position_1500_ms_later": later,
        }));
    }
    unsafe { stop(player) };
    let value = serde_json::json!({
        "version": unsafe { CStr::from_ptr(version()) }.to_string_lossy(),
        "options": options,
        "file": args[2],
        "song": args.get(3),
        "started_ms": ms(started),
        "results": results,
        "messages_between_request_and_picture": seen,
    });
    std::fs::write(&args[1], serde_json::to_string_pretty(&value)?)?;
    println!("{} {:?}", value["version"], options);
    for r in value["results"].as_array().unwrap() {
        println!(
            "{:>6} from {:>6} | first picture {:>6.0} ms | delivered {:>6.0} ms | stale {:>2} | then {:>6} | +1.5 s {:>6}",
            r["target_ms"],
            r["from_ms"],
            r["first_picture_ms"].as_f64().unwrap_or(-1.0),
            r["shown_ms"].as_f64().unwrap_or(-1.0),
            r["pictures_before"],
            r["position_after_ms"],
            r["position_1500_ms_later"],
        );
    }
    Ok(())
}
