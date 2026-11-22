//! The scene jobs, in the worker: check a scene (validate it at the output's
//! frames and measure it), or look at frames of it drawn by the export's engine
//! path over the source's picture. Everything is written into `scene/` in the
//! job's workspace; the supervisor publishes that folder whole, or nothing.
use super::draw::{Draw, measure};
use super::layer::{MotionLayer, read_scene};
use super::song::{Song, gather};
use super::thread::ScriptThread;
use super::validate::{Report, grid};
use crate::engine::{EngineError, Event, ExportResult, Reporter};
use crate::fx::export::Overlay;
use crate::fx::gl::GlRenderer;
use crate::fx::shaders::Output;
use crate::scene_request::{STRIP_FRAMES, SceneMode, SceneRequest, safe_area_required};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The staged folder, inside the job's workspace.
pub use crate::scene_request::STAGED;
/// Frames timed whole (script, painting, GPU) in a check.
const TIMED_FRAMES: usize = 8;

fn fail(code: &str, path: Option<&Path>, detail: impl ToString) -> EngineError {
    EngineError::new(code, "scene", path, detail)
}

/// What every scene job starts from: the source inspected, the scene loaded on
/// its script thread and validated at the output's frames.
struct Loaded {
    thread: ScriptThread,
    report: Report,
    notes: Vec<String>,
    size: (u32, u32),
    from: f64,
    to: f64,
    fps: f64,
}

fn load(request: &SceneRequest, report: &mut Reporter<'_>) -> Result<Loaded, EngineError> {
    report.progress(0, "scene.preparing");
    let media = crate::media::inspect_timed(&request.ffmpeg, &request.source)
        .map_err(|e| fail("error.scene_input", Some(&request.source), e))?;
    crate::captions::supported_video(&media.video)?;
    let canvas = crate::captions::Canvas::new(
        media.video.display_width,
        media.video.display_height,
        crate::captions::CaptionStyle::default(),
    )?;
    let size = canvas.size();
    safe_area_required(size.0, size.1, request.safe_area)?;
    let duration = media.duration;
    let from = request.from_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
    let to = request
        .to_ms
        .map_or(duration, |ms| ms as f64 / 1000.0)
        .min(duration);
    if from >= to {
        return Err(fail(
            "error.scene_request",
            None,
            format!("The interval starts after the video ends ({duration:.3} s)."),
        ));
    }
    let srt = crate::captions::read_srt(&request.subtitles)?;
    let duration_ms = (duration * 1000.0).ceil() as u64;
    let mut track = crate::subtitle_srt::parse(&srt, duration_ms).map_err(|e| {
        fail(
            "error.caption_track",
            Some(&request.subtitles),
            format!("The subtitles cannot be read: {e}"),
        )
    })?;
    let words = crate::subtitle_words::read_beside(&request.subtitles, &mut track);
    let Song {
        inputs,
        audio,
        notes,
    } = gather(
        &request.ffmpeg,
        &request.source,
        size,
        duration,
        &track,
        words,
        request.safe_area,
    )
    .map_err(|e| fail("error.scene_input", Some(&request.source), e))?;
    let src = read_scene(&request.scene)
        .map_err(|e| fail("error.scene_refused", Some(&request.scene), e))?;
    let thread = ScriptThread::start(src, inputs, audio, true)
        .map_err(|e| fail("error.scene_refused", Some(&request.scene), e))?;
    let fps = if media.video.fps.is_finite() && media.video.fps > 0.0 {
        media.video.fps.min(120.0)
    } else {
        30.0
    };
    report.progress(5, "scene.checking");
    let checked = thread
        .validate(grid(from, to, fps))
        .map_err(|e| fail("error.scene_refused", Some(&request.scene), e))?;
    Ok(Loaded {
        thread,
        report: checked,
        notes,
        size,
        from,
        to,
        fps,
    })
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, EngineError> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| fail("error.scene_output", Some(&path), e))?;
    Ok(path)
}

fn write_json(dir: &Path, name: &str, value: &serde_json::Value) -> Result<(), EngineError> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| fail("error.scene_output", None, e))?;
    write(dir, name, &text).map(|_| ())
}

/// The counted part of a validation, and what it said.
fn report_json(r: &Report) -> serde_json::Value {
    json!({
        "frames": r.frames,
        "max": {
            "ops": r.max_ops, "ops_at": r.max_ops_at,
            "draws": r.max_draws, "particles": r.max_particles, "glyphs": r.max_glyphs,
            "area": r.max_area, "area_at": r.max_area_at,
            "blurs": r.max_blurs, "passes": r.max_passes,
            "paths": r.max_paths, "segments": r.max_segments, "shape_area": r.max_shape_area,
            "calls": r.max_calls, "reads": r.max_reads,
            "text_blocks": r.blocks, "peak_text_scale": r.peak_text_scale,
        },
        "limits": {
            "ops": super::script::MAX_OPS, "draws": super::draw::MAX_DRAWS,
            "particles": super::draw::MAX_PARTICLES, "glyphs": super::draw::MAX_GLYPHS,
            "area": super::draw::MAX_AREA, "blurs": super::draw::MAX_BLURS,
            "passes": super::draw::MAX_PASSES, "calls": super::script::MAX_CALLS,
            "text_blocks": super::draw::MAX_BLOCKS,
        },
        "script_ms": { "p95": r.script_ms_p95, "worst": r.script_ms_worst, "worst_at": r.script_ms_worst_at },
        "warnings": r.warnings,
        "layout_warnings": r.layout_warnings,
        "not_checked": r.not_checked,
    })
}

/// The instants of a check timed whole: where the counted maxima and the worst
/// script frame fell, then spread evenly, `TIMED_FRAMES` at most.
fn timed_instants(l: &Loaded) -> Vec<f64> {
    let mut v = vec![
        l.report.max_ops_at,
        l.report.max_area_at,
        l.report.script_ms_worst_at,
    ];
    let spread = TIMED_FRAMES - v.len();
    for i in 0..spread {
        v.push(l.from + (i as f64 + 0.5) * (l.to - l.from) / spread as f64);
    }
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

fn p95(mut v: Vec<f64>) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    v.sort_by(f64::total_cmp);
    (
        v[((v.len() - 1) as f64 * 0.95).round() as usize],
        v[v.len() - 1],
    )
}

fn check(request: &SceneRequest, l: Loaded, dir: &Path) -> Result<(), EngineError> {
    let mut out = report_json(&l.report);
    out["scene"] = json!(request.scene);
    out["format"] = json!(super::script::FORMAT);
    out["canvas"] = json!([l.size.0, l.size.1]);
    out["interval_s"] = json!([l.from, l.to]);
    out["fps"] = json!(l.fps);
    out["notes"] = json!(l.notes);
    let instants = timed_instants(&l);
    let (w, h) = l.size;
    // The whole frame on this machine, for information: script, painting, GPU.
    match GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma) {
        Ok(mut gpu) => {
            let mut layer = MotionLayer::new(l.thread, l.size, 0.0)
                .map_err(|e| fail("error.scene_render", None, e))?;
            layer
                .setup(&mut gpu)
                .map_err(|e| fail("error.scene_render", None, e))?;
            let gl = gpu.context().clone();
            // One frame first, untimed: the engine's first draw compiles and
            // uploads what every later one reuses.
            layer
                .draw(&mut gpu, instants[0])
                .map_err(|e| fail("error.scene_refused", None, e))?;
            unsafe { glow::HasContext::finish(&*gl) };
            let mut times = Vec::new();
            for &t in &instants {
                let a = Instant::now();
                layer
                    .draw(&mut gpu, t)
                    .map_err(|e| fail("error.scene_refused", None, e))?;
                unsafe { glow::HasContext::finish(&*gl) };
                times.push(a.elapsed().as_secs_f64() * 1000.0);
            }
            let (p95, worst) = p95(times);
            out["frame_ms"] = json!({
                "p95": p95, "worst": worst, "frames": instants,
                "what": "script, painting and GPU of the whole frame, on this machine, at the instants listed (where the maxima fell, then spread evenly): for information, never a limit",
                "adapter": gpu.adapter,
            });
        }
        Err(e) => {
            out["frame_ms"] = json!({ "missing": format!("no OpenGL context on this machine ({e}): the whole frame was not timed") });
        }
    }
    write_json(dir, "report.json", &out)
}

/// One frame of the source at `t` seconds, scaled to `w` x `h`, as RGB.
fn source_frame(
    request: &SceneRequest,
    t: f64,
    (w, h): (u32, u32),
) -> Result<Vec<u8>, EngineError> {
    let mut cmd = crate::command(&request.ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-ss",
        &format!("{t:.6}"),
        "-i",
    ])
    .arg(&request.source)
    .args([
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={w}:{h},setsar=1,format=rgb24"),
        "-f",
        "rawvideo",
        "pipe:1",
    ]);
    let (status, bytes, log) = super::song::capture(cmd, std::time::Duration::from_secs(60))
        .map_err(|e| fail("error.scene_input", Some(&request.source), e))?;
    let size = (w * h * 3) as usize;
    if !status.success() || bytes.len() < size {
        return Err(fail(
            "error.scene_input",
            Some(&request.source),
            format!("FFmpeg gave no frame at {t:.3} s: {}", log.trim()),
        ));
    }
    Ok(bytes[..size].to_vec())
}

/// The overlay (premultiplied RGBA, display-encoded) over `under` (RGB), in
/// display encoding as the mix puts it over the video.
fn over(overlay: &[u8], under: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(under.len());
    for (o, u) in overlay
        .as_chunks::<4>()
        .0
        .iter()
        .zip(under.as_chunks::<3>().0)
    {
        let keep = 255 - u32::from(o[3]);
        for c in 0..3 {
            out.push((u32::from(o[c]) + (u32::from(u[c]) * keep + 127) / 255).min(255) as u8);
        }
    }
    out
}

/// The frame's edges, a line every tenth of the frame (half seen through) and
/// the safe area, drawn thin.
fn draw_grid(rgb: &mut [u8], (w, h): (u32, u32), safe: [f32; 4]) {
    let mut paint = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3], half: bool| {
        for y in y0.min(h - 1)..=y1.min(h - 1) {
            for x in x0.min(w - 1)..=x1.min(w - 1) {
                let i = ((y * w + x) * 3) as usize;
                for k in 0..3 {
                    rgb[i + k] = if half {
                        ((u16::from(rgb[i + k]) + u16::from(c[k])) / 2) as u8
                    } else {
                        c[k]
                    };
                }
            }
        }
    };
    for k in 1..10 {
        let (x, y) = (w * k / 10, h * k / 10);
        paint(x, 0, x, h - 1, [255, 255, 255], true);
        paint(0, y, w - 1, y, [255, 255, 255], true);
    }
    let mut line = |x0, y0, x1, y1, c| paint(x0, y0, x1, y1, c, false);
    let edge = [255, 255, 255];
    line(0, 0, w - 1, 1, edge);
    line(0, h - 2, w - 1, h - 1, edge);
    line(0, 0, 1, h - 1, edge);
    line(w - 2, 0, w - 1, h - 1, edge);
    if safe != [0.0, 0.0, 1.0, 1.0] {
        let px = |f: f32, n: u32| ((f * n as f32).round() as u32).min(n - 1);
        let (x0, y0, x1, y1) = (
            px(safe[0], w),
            px(safe[1], h),
            px(safe[2], w),
            px(safe[3], h),
        );
        let c = [0, 220, 255];
        line(x0, y0, x1, y0 + 1, c);
        line(x0, y1.saturating_sub(1), x1, y1, c);
        line(x0, y0, x0 + 1, y1, c);
        line(x1.saturating_sub(1), y0, x1, y1, c);
    }
}

fn png(rgb: &[u8], (w, h): (u32, u32)) -> Result<Vec<u8>, EngineError> {
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| fail("error.scene_output", None, e))?;
        writer
            .write_image_data(rgb)
            .map_err(|e| fail("error.scene_output", None, e))?;
    }
    Ok(bytes)
}

/// Tiles of `w` x `h` frames side by side, four to a row, each shrunk to fit
/// 480 pixels wide (box filter).
fn sheet(frames: &[Vec<u8>], (w, h): (u32, u32)) -> (Vec<u8>, (u32, u32)) {
    let k = (w as f32 / 480.0).ceil().max(1.0) as u32;
    let (tw, th) = (w / k, h / k);
    let cols = frames.len().min(4) as u32;
    let rows = frames.len().div_ceil(4) as u32;
    let (sw, sh) = (cols * tw + (cols + 1) * 4, rows * th + (rows + 1) * 4);
    let mut out = vec![18u8; (sw * sh * 3) as usize];
    for (n, f) in frames.iter().enumerate() {
        let (cx, cy) = (n as u32 % 4, n as u32 / 4);
        let (ox, oy) = (4 + cx * (tw + 4), 4 + cy * (th + 4));
        for ty in 0..th {
            for tx in 0..tw {
                let mut acc = [0u32; 3];
                for dy in 0..k {
                    for dx in 0..k {
                        let i = (((ty * k + dy) * w + tx * k + dx) * 3) as usize;
                        for c in 0..3 {
                            acc[c] += u32::from(f[i + c]);
                        }
                    }
                }
                let o = (((oy + ty) * sw + ox + tx) * 3) as usize;
                for c in 0..3 {
                    out[o + c] = (acc[c] / (k * k)) as u8;
                }
            }
        }
    }
    (out, (sw, sh))
}

fn ms(t: f64) -> u64 {
    (t * 1000.0).round().max(0.0) as u64
}

fn look(
    request: &SceneRequest,
    l: Loaded,
    dir: &Path,
    report: &mut Reporter<'_>,
) -> Result<(), EngineError> {
    let (from, to) = (l.from, l.to);
    let instants: Vec<f64> = match &request.mode {
        SceneMode::Stills { at_ms } | SceneMode::Measure { at_ms } => {
            at_ms.iter().map(|&m| m as f64 / 1000.0).collect()
        }
        SceneMode::Strip { at_ms } => (0..STRIP_FRAMES)
            .map(|i| *at_ms as f64 / 1000.0 + i as f64 / STRIP_FRAMES as f64)
            .collect(),
        SceneMode::Sheet { count } => (0..*count)
            .map(|i| from + (i as f64 + 0.5) * (to - from) / *count as f64)
            .collect(),
        SceneMode::Check => unreachable!(),
    };
    if let Some(t) = instants.iter().find(|t| **t < 0.0 || **t >= l.to + 1e-9) {
        return Err(fail(
            "error.scene_request",
            None,
            format!("{t:.3} s is outside the interval looked at ({from:.3} to {to:.3} s)."),
        ));
    }
    let size = l.size;
    let safe = super::song::safe_fractions(request.safe_area, size.0, size.1);
    if let SceneMode::Measure { .. } = request.mode {
        let fonts = std::cell::RefCell::new(
            crate::fx::text::Fonts::load().map_err(|e| fail("error.scene_render", None, e))?,
        );
        let (w, h) = (size.0 as f32, size.1 as f32);
        let mut frames = Vec::new();
        for &t in &instants {
            let (f, counts) = l
                .thread
                .frame(t)
                .map_err(|e| fail("error.scene_refused", None, e))?;
            let texts: Vec<serde_json::Value> = f
                .draws
                .iter()
                .filter_map(|d| match d {
                    Draw::Text(td) => Some(td),
                    _ => None,
                })
                .map(|td| {
                    let (b, _) = measure(&mut fonts.borrow_mut(), &td.block, td.scale, w, h);
                    let inside = |r: [f32; 4]| {
                        b.x0 >= r[0] - 0.002
                            && b.y0 >= r[1] - 0.002
                            && b.x1 <= r[2] + 0.002
                            && b.y1 <= r[3] + 0.002
                    };
                    json!({
                        "text": td.block.text,
                        "box": [b.x0, b.y0, b.x1, b.y1],
                        "rotated": td.block.rot != 0.0,
                        "opacity": td.opacity,
                        "inside_frame": inside([0.0, 0.0, 1.0, 1.0]),
                        "inside_safe_area": inside(safe),
                    })
                })
                .collect();
            frames.push(json!({
                "t": t, "texts": texts, "draws": f.draws.len(),
                "glyphs": counts.glyphs, "particles": counts.particles, "area": counts.area,
            }));
        }
        return write_json(
            dir,
            "measure.json",
            &json!({
                "canvas": [size.0, size.1], "safe_area": safe,
                "boxes": "fractions of the frame: x0, y0, x1, y1, before any rotation",
                "frames": frames, "notes": l.notes,
            }),
        );
    }
    let (w, h) = size;
    let mut gpu =
        GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).map_err(|e| {
            fail(
                "error.motion_gl_unavailable",
                None,
                format!("This computer cannot draw scenes ({e})."),
            )
        })?;
    let mut layer =
        MotionLayer::new(l.thread, size, 0.0).map_err(|e| fail("error.scene_render", None, e))?;
    layer
        .setup(&mut gpu)
        .map_err(|e| fail("error.scene_render", None, e))?;
    let mut files = Vec::new();
    let mut frames = Vec::new();
    for (i, &t) in instants.iter().enumerate() {
        layer
            .draw(&mut gpu, t)
            .map_err(|e| fail("error.scene_refused", None, e))?;
        let under = source_frame(request, t, size)?;
        // Bare: the video alone, to compose on it.
        let mut rgb = if request.bare {
            under
        } else {
            over(&layer.read(), &under)
        };
        if request.grid {
            draw_grid(&mut rgb, size, safe);
        }
        let name = format!("frame-{:02}-{}ms.png", i + 1, ms(t));
        write(dir, &name, &png(&rgb, size)?)?;
        files.push(json!({ "t": t, "file": name }));
        frames.push(rgb);
        report.progress(10 + (85 * (i + 1) / instants.len()) as u32, "scene.looking");
    }
    let mut out = json!({
        "canvas": [w, h], "frames": files, "bare": request.bare, "grid": request.grid,
        "notes": l.notes, "warnings": l.report.warnings, "layout_warnings": l.report.layout_warnings,
    });
    if matches!(
        request.mode,
        SceneMode::Strip { .. } | SceneMode::Sheet { .. }
    ) {
        let (rgb, s) = sheet(&frames, size);
        write(dir, "sheet.png", &png(&rgb, s)?)?;
        out["sheet"] = json!({ "file": "sheet.png", "order": "left to right, top to bottom, four to a row, in the order of frames" });
    }
    write_json(dir, "look.json", &out)
}

/// Run a scene job; its files are written into `workspace/scene`.
pub fn execute(
    request: &SceneRequest,
    workspace: &Path,
    emit: &mut dyn FnMut(Event),
) -> Result<ExportResult, EngineError> {
    let mut report = Reporter::new(emit);
    let dir = workspace.join(STAGED);
    std::fs::create_dir(&dir).map_err(|e| fail("error.scene_output", Some(&dir), e))?;
    let loaded = load(request, &mut report)?;
    let span = loaded.to - loaded.from;
    for note in &loaded.notes {
        report.diagnostic(note);
    }
    match request.mode {
        SceneMode::Check => check(request, loaded, &dir)?,
        _ => look(request, loaded, &dir, &mut report)?,
    }
    report.progress(99, "scene.writing");
    Ok(ExportResult {
        output: request.output.clone(),
        duration: span,
    })
}
