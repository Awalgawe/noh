//! Caption engine prototype (direction-v10 step 2, plan-moteur-v3). Local
//! measurement tool only: not part of the application.
//!
//! cargo run --release --example caption_proto -- <command> [options]
//!   bench  --test N --orient land|short --glyphs bitmap|msdf --out DIR [--start S --end S]
//!   video  --test N --orient land|short --glyphs bitmap|msdf --out FILE.mp4 --start S --end S
//!   still  --test N --orient land|short --glyphs bitmap|msdf --t S --out FILE.png
//!   shape-dump --out FILE.json
mod effects;
mod engine;
mod gl;
mod gpu;
mod mkv;
mod plugin;
mod plugin_shader;
mod preview;
mod text;

/// NOH's own caption line layout, compiled from the unmodified source file.
#[path = "../../src/caption_layout.rs"]
#[allow(dead_code)]
mod caption_layout;
/// Shim for the one item caption_layout.rs imports from the crate.
mod subtitle_track {
    pub const MAX_CUE_TEXT_BYTES: usize = 4096;
}

use effects::{Line, Scene, Test, split_words};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const FPS: f64 = 24.0;
const SONG_S: f64 = 225.917;

/// Original Japanese lines written for test 4 (no third-party text), pre-split into words.
const JA: [&[&str]; 2] = [
    &["夜の", "光が", "静かに", "揺れて、", "忘れた", "名前を", "もう一度", "呼ぶ。", "崩れた", "星の", "欠片から、", "新しい", "形が", "生まれていく"],
    &["ぶつかり", "合った", "二つの", "体、", "止まらない", "速さと", "重さだけ。", "それでも", "君は", "残された、", "傷では", "なく", "続きとして"],
];

struct Args(HashMap<String, String>);

impl Args {
    fn parse(v: &[String]) -> Self {
        let mut m = HashMap::new();
        let mut i = 0;
        while i < v.len() {
            if let Some(k) = v[i].strip_prefix("--") {
                let val = v.get(i + 1).cloned().unwrap_or_default();
                m.insert(k.to_string(), val);
                i += 2;
            } else {
                i += 1;
            }
        }
        Self(m)
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }
    fn req(&self, k: &str) -> &str {
        self.get(k).unwrap_or_else(|| panic!("missing --{k}"))
    }
    fn f(&self, k: &str, d: f64) -> f64 {
        self.get(k).map(|s| s.parse().expect("number")).unwrap_or(d)
    }
}

fn env_path(k: &str, d: &str) -> PathBuf {
    std::env::var_os(k).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(d))
}

fn lines_for(test: Test) -> Vec<Line> {
    let srt_path = env_path("NOH_PROTO_SRT", "media/lyrics.srt");
    let text = std::fs::read_to_string(&srt_path).expect("SRT");
    let track = noh::subtitle_srt::parse(&text, (SONG_S * 1000.0) as u64).expect("parse SRT");
    track
        .cues
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (s, e) = (c.start_ms as f64 / 1000.0, c.end_ms as f64 / 1000.0);
            if test == Test::Wave {
                let words: Vec<String> = JA[i % 2].iter().map(|w| w.to_string()).collect();
                split_words(s, e, &words, "")
            } else {
                let words: Vec<String> = c.text.split_whitespace().map(str::to_string).collect();
                split_words(s, e, &words, " ")
            }
        })
        .collect()
}

fn parse_wh(s: &str) -> (u32, u32) {
    let (w, h) = s.split_once('x').expect("WxH");
    (w.parse().unwrap(), h.parse().unwrap())
}

/// Straight-alpha RGBA overlay frames on stdout, `frames` frames from `start` at 24 fps,
/// for FFmpeg's `-f rawvideo -pix_fmt rgba` input. Timings go to --stats.
fn overlay_raw(a: &Args) {
    let (_, w, h, _, mut fonts, mut gpu, mut scene) = setup(a);
    let start = a.f("start", 0.0);
    let frames: usize = a.req("frames").parse().unwrap();
    // --pipeline N: a writer thread drains up to N rendered frames while the next renders.
    let depth: usize = a.get("pipeline").map(|v| v.parse().unwrap()).unwrap_or(0);
    if depth > 0 {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(depth);
        let writer = std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let mut write_ms = Vec::new();
            for px in rx {
                let t0 = Instant::now();
                if out.write_all(&px).is_err() {
                    break;
                }
                write_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            out.flush().ok();
            write_ms
        });
        let (mut render, mut handoff) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let wall = Instant::now();
        for f in 0..frames {
            let t = start + f as f64 / FPS;
            let t0 = Instant::now();
            let (glow, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
            let (px, _) = gpu.frame(&glow, gain, &main, true).expect("frame");
            let t1 = Instant::now();
            if tx.send(px).is_err() {
                break;
            }
            render.push(t1.duration_since(t0).as_secs_f64() * 1000.0);
            handoff.push(t1.elapsed().as_secs_f64() * 1000.0);
        }
        drop(tx);
        let write = writer.join().unwrap();
        let json = format!(
            "{{\"size\": [{w}, {h}], \"frames\": {frames}, \"pipeline\": {depth}, \"wall_s\": {:.2},
{},
{},
{}
}}
",
            wall.elapsed().as_secs_f64(),
            summary("render_readback_ms", &render),
            summary("handoff_wait_ms", &handoff),
            summary("pipe_write_ms", &write)
        );
        if let Some(p) = a.get("stats") {
            std::fs::write(p, &json).unwrap();
        }
        eprint!("{json}");
        return;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let (mut render, mut write) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let wall = Instant::now();
    for f in 0..frames {
        let t = start + f as f64 / FPS;
        let t0 = Instant::now();
        let (glow, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
        let (px, _) = gpu.frame(&glow, gain, &main, true).expect("frame");
        let t1 = Instant::now();
        if out.write_all(&px).is_err() {
            eprintln!("overlay-raw: consumer closed at frame {f}");
            break;
        }
        render.push(t1.duration_since(t0).as_secs_f64() * 1000.0);
        write.push(t1.elapsed().as_secs_f64() * 1000.0);
    }
    out.flush().ok();
    let json = format!(
        "{{\"size\": [{w}, {h}], \"frames\": {frames}, \"wall_s\": {:.2},\n{},\n{}\n}}\n",
        wall.elapsed().as_secs_f64(),
        summary("render_readback_ms", &render),
        summary("pipe_write_ms", &write)
    );
    if let Some(p) = a.get("stats") {
        std::fs::write(p, &json).unwrap();
    }
    eprint!("{json}");
}

fn dims(orient: &str) -> (u32, u32) {
    match orient {
        "short" => (1080, 1920),
        _ => (1920, 1080),
    }
}

fn setup(a: &Args) -> (Test, u32, u32, bool, text::Fonts, engine::Engine, Scene) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test 1..5");
    let (w, h) = match a.get("size") {
        Some(s) => parse_wh(s),
        None => dims(a.get("orient").unwrap_or("land")),
    };
    let msdf = a.get("glyphs") == Some("msdf");
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    // --logical WxH: render a WxH canvas layout into the --size output (same layout, other resolution).
    if let Some(l) = a.get("logical") {
        let (lw, lh) = parse_wh(l);
        let gpu = engine::Engine::with_canvas(w, h, lw as f32, lh as f32, gpu::Output::StraightSrgb).expect("gpu");
        let scene = Scene::new(test, lw, lh, msdf, lines_for(test)).with_px_scale(h as f32 / lh as f32);
        return (test, w, h, msdf, fonts, gpu, scene);
    }
    let gpu = engine::Engine::new(w, h).expect("gpu");
    let scene = Scene::new(test, w, h, msdf, lines_for(test));
    (test, w, h, msdf, fonts, gpu, scene)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[idx.clamp(1, v.len()) - 1]
}

fn summary(name: &str, v: &[f64]) -> String {
    let mut c = v.to_vec();
    let (p50, p95, p99, max) = (pct(&mut c, 50.0), pct(&mut c, 95.0), pct(&mut c, 99.0), pct(&mut c, 100.0));
    format!("\"{name}\": {{\"n\": {}, \"p50\": {p50:.3}, \"p95\": {p95:.3}, \"p99\": {p99:.3}, \"max\": {max:.3}}}", v.len())
}

fn bench(a: &Args) {
    let (test, w, h, msdf, mut fonts, mut gpu, mut scene) = setup(a);
    let out = PathBuf::from(a.req("out"));
    std::fs::create_dir_all(&out).unwrap();
    let start = a.f("start", 0.0);
    let end = a.f("end", SONG_S);
    let frames = ((end - start) * FPS).round() as usize;
    let mut csv = String::from("frame,t,has_line,instances,build_ms,gpu_ms,submit_to_mapped_ms,copy_out_ms,total_ms,line_cost_ms,pre_submit_ms\n");
    let (mut build, mut gpums, mut mapped, mut copy, mut total, mut total_text) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut lines_csv = String::from("line,layout_ms,raster_ms,raster_glyphs\n");
    let mut line_total = vec![];
    let wall = Instant::now();
    for f in 0..frames {
        let t = start + f as f64 / FPS;
        let t0 = Instant::now();
        let (glow, gain, main, cost) = scene.frame(&mut fonts, &gpu, t);
        let b = t0.elapsed().as_secs_f64() * 1000.0;
        let (_, ft) = gpu.frame(&glow, gain, &main, true).expect("frame");
        let tot = t0.elapsed().as_secs_f64() * 1000.0;
        let lc = cost.map(|c| c.layout_ms + c.raster_ms).unwrap_or(0.0);
        if let Some(c) = cost {
            lines_csv += &format!("{},{:.3},{:.3},{}\n", c.line, c.layout_ms, c.raster_ms, c.raster_glyphs);
            line_total.push(c.layout_ms + c.raster_ms);
        }
        let has = scene.line_at(t).is_some();
        csv += &format!(
            "{f},{t:.4},{},{},{b:.3},{},{:.3},{:.3},{tot:.3},{lc:.3},{:.3}\n",
            u8::from(has),
            glow.len() + main.len(),
            ft.gpu_ms.map(|v| format!("{v:.3}")).unwrap_or_default(),
            ft.submit_to_mapped_ms,
            ft.copy_out_ms,
            ft.pre_submit_ms
        );
        build.push(b);
        if let Some(g) = ft.gpu_ms {
            gpums.push(g);
        }
        mapped.push(ft.submit_to_mapped_ms);
        copy.push(ft.copy_out_ms);
        total.push(tot);
        if has {
            total_text.push(tot);
        }
    }
    let wall_s = wall.elapsed().as_secs_f64();
    std::fs::write(out.join("frames.csv"), csv).unwrap();
    std::fs::write(out.join("lines.csv"), lines_csv).unwrap();
    let st = scene.atlas.stats;
    let json = format!(
        "{{\n\"test\": {}, \"width\": {w}, \"height\": {h}, \"glyphs\": \"{}\", \"start\": {start}, \"end\": {end}, \"frames\": {frames},\n\"adapter\": {:?}, \"setup_ms\": {:.1}, \"wall_s\": {wall_s:.2},\n{},\n{},\n{},\n{},\n{},\n{},\n{},\n\"atlas\": {{\"glyphs\": {}, \"raster_ms\": {:.2}, \"texels\": {}, \"resets\": {}}}\n}}\n",
        test as u32,
        if msdf { "msdf" } else { "bitmap" },
        gpu.adapter(),
        gpu.setup_ms(),
        summary("build_ms", &build),
        summary("gpu_ms", &gpums),
        summary("submit_to_mapped_ms", &mapped),
        summary("copy_out_ms", &copy),
        summary("total_ms", &total),
        summary("total_ms_text_frames", &total_text),
        summary("line_cost_ms", &line_total),
        st.glyphs,
        st.ms,
        st.texels,
        st.atlas_resets
    );
    std::fs::write(out.join("summary.json"), &json).unwrap();
    println!("{json}");
}

fn ffmpeg() -> PathBuf {
    env_path("NOH_FFMPEG", "ffmpeg")
}

fn video(a: &Args) {
    let (_, w, h, _, mut fonts, mut gpu, mut scene) = setup(a);
    let out = a.req("out");
    let start = a.f("start", 0.0);
    let end = a.f("end", 20.0);
    let dur = end - start;
    let clip = env_path("NOH_PROTO_CLIP", "media/clip.mp4");
    let wav = env_path("NOH_PROTO_WAV", "media/audio.wav");
    let filter = format!(
        "[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps=24,setpts=PTS-STARTPTS[bg];[bg][1:v]overlay=0:0:format=auto:shortest=1,format=yuv420p[v]"
    );
    let mut child = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-n", "-stream_loop", "-1"])
        .arg("-i")
        .arg(&clip)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", "24", "-i", "pipe:0"])
        .args(["-ss", &format!("{start:.3}"), "-t", &format!("{dur:.3}")])
        .arg("-i")
        .arg(&wav)
        .args(["-filter_complex", &filter, "-map", "[v]", "-map", "2:a", "-t", &format!("{dur:.3}")])
        .args(["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0", "-bf", "0"])
        .args(["-c:a", "aac", "-b:a", "192k", out])
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut stdin = child.stdin.take().unwrap();
    let frames = (dur * FPS).round() as usize;
    for f in 0..frames {
        let t = start + f as f64 / FPS;
        let (glow, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
        let (px, _) = gpu.frame(&glow, gain, &main, true).expect("frame");
        if stdin.write_all(&px).is_err() {
            break;
        }
    }
    drop(stdin);
    let status = child.wait().unwrap();
    println!("video {out}: {frames} frames, ffmpeg {status}");
}

fn still(a: &Args) {
    let (_, w, h, _, mut fonts, mut gpu, mut scene) = setup(a);
    let t = a.f("t", 1.0);
    // Warm through the line from its start so entry animations are evaluated at t.
    let (glow, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
    let (px, _) = gpu.frame(&glow, gain, &main, true).expect("frame");
    let out = PathBuf::from(a.req("out"));
    image::save_buffer(&out, &px, w, h, image::ColorType::Rgba8).unwrap();
    // Flatten over mid-grey for viewing.
    let mut flat = Vec::with_capacity((w * h * 3) as usize);
    for p in px.chunks_exact(4) {
        let al = p[3] as f32 / 255.0;
        for c in 0..3 {
            flat.push((p[c] as f32 * al + 70.0 * (1.0 - al)).round() as u8);
        }
    }
    let grey = out.with_extension("grey.png");
    image::save_buffer(&grey, &flat, w, h, image::ColorType::Rgb8).unwrap();
    println!("still {} and {}", out.display(), grey.display());
}

/// Glyph ids and positions for the HarfBuzz comparison (one unwrapped line each).
fn shape_dump(a: &Args) {
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let size = 64.0;
    let cases: [(&str, String); 3] = [
        ("ja", JA[0].concat()),
        ("fr", "Ou ta mara, je me rappelle pas. Officiellement j'y étais pas".into()),
        ("en", "And the night rearranges itself around your silence".into()),
    ];
    let mut out = String::from("[\n");
    for (ci, (lang, s)) in cases.iter().enumerate() {
        let laid = text::layout(&mut fonts, &[s.clone()], "", text::Face::Cjk, size, None, 1.2);
        out += &format!("{{\"lang\": \"{lang}\", \"text\": {:?}, \"size\": {size}, \"lines\": {}, \"glyphs\": [", s, laid.lines.len());
        for (i, g) in laid.glyphs.iter().enumerate() {
            out += &format!(
                "{}{{\"gid\": {}, \"x\": {:.5}, \"baseline\": {:.5}, \"advance\": {:.5}, \"cluster\": {}}}",
                if i == 0 { "" } else { ", " },
                g.glyph_id,
                g.x,
                g.baseline,
                g.advance,
                g.cluster_start
            );
        }
        out += &format!("]}}{}\n", if ci + 1 < cases.len() { "," } else { "" });
    }
    out += "]\n";
    std::fs::write(a.req("out"), out).unwrap();
    println!("shape dump written");
}

/// Line breaks: NOH's caption_layout (reference chain) against cosmic-text, same font/size/width.
fn breaks(a: &Args) {
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let cases: [(&str, String); 3] = [
        ("ja", JA[0].concat() + &JA[1].concat()),
        ("fr", "Mais je vous avoue il me semble qu'une fois au coin d'une rue à deux pas de là, officiellement j'y étais pas".into()),
        ("en", "Somewhere in the dark, water climbs the harbour steps and the night rearranges itself around your silence".into()),
    ];
    let size = 64.0f32;
    let mut out = String::from("[
");
    let mut first = true;
    for width in [700.0f32, 900.0, 1200.0, 1600.0] {
        for (lang, s) in &cases {
            let reference = caption_layout::layout_text(s, size, width, 10_000.0).map_err(|e| e.to_string());
            let noh_json = match &reference {
                Ok(lines) => format!("{lines:?}"),
                Err(e) => format!("{{\"error\": {e:?}}}"),
            };
            let laid = text::layout(&mut fonts, &[s.clone()], "", text::Face::Cjk, size, Some(width), 1.2);
            // NOH keeps trailing spaces on each line; compare without them.
            let same = reference
                .as_ref()
                .map(|r| r.iter().map(|l| l.trim_end()).eq(laid.line_texts.iter().map(String::as_str)))
                .unwrap_or(false);
            out += &format!(
                "{}{{\"lang\": \"{lang}\", \"width\": {width}, \"same\": {same}, \"noh\": {:?}, \"cosmic\": {:?}}}
",
                if first { "" } else { "," },
                noh_json,
                laid.line_texts
            );
            first = false;
        }
    }
    out += "]
";
    std::fs::write(a.req("out"), &out).unwrap();
    print!("{out}");
}

fn preview_cmd(a: &Args) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test");
    let switch_to = Test::from_num(a.get("switch-to").unwrap_or("3").parse().unwrap()).expect("switch");
    let logical = dims(a.get("orient").unwrap_or("land"));
    let parse = |s: &str| -> (u32, u32) {
        let (w, h) = s.split_once('x').expect("WxH");
        (w.parse().unwrap(), h.parse().unwrap())
    };
    let overlay = parse(a.req("overlay"));
    let cfg = preview::Config {
        test,
        switch_to,
        switch_at_s: a.f("switch-at", 20.0),
        logical,
        overlay,
        start_s: a.f("start", 0.0),
        seconds: a.f("seconds", 60.0),
        out: PathBuf::from(a.req("out")),
        assets: env_path("NOH_PROTO_ASSETS", "assets"),
        serif: env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf"),
        clip: env_path("NOH_PROTO_CLIP", "media/clip.mp4"),
        ffmpeg: ffmpeg(),
        lines_for,
        backend: a.get("backend").unwrap_or("dx12").to_string(),
        screenshot_at: a.get("screenshot-at").map(|v| v.parse().unwrap()),
    };
    preview::run(cfg);
}

/// Preview-side blend for the 'même rendu' check: background = a decoded source
/// frame (raw RGBA, `--bg`), overlay rendered for egui (premultiplied gamma), both at
/// `--size`, drawn 1:1 by egui/glow and captured with a viewport screenshot.
fn blend_capture(a: &Args) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test");
    let (w, h) = parse_wh(a.req("size"));
    let logical = match a.get("logical") {
        Some(s) => parse_wh(s),
        None => (w, h),
    };
    let t = a.f("t", 1.0);
    let bg = std::fs::read(a.req("bg")).expect("bg");
    assert_eq!(bg.len(), (w * h * 4) as usize, "bg must be raw RGBA WxH");
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let mut gpu = engine::Engine::with_canvas(w, h, logical.0 as f32, logical.1 as f32, gpu::Output::PremulGamma).expect("gpu");
    let mut scene = Scene::new(test, logical.0, logical.1, false, lines_for(test)).with_px_scale(h as f32 / logical.1 as f32);
    let (glow, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
    let (overlay, _) = gpu.frame(&glow, gain, &main, true).expect("frame");
    let out = PathBuf::from(a.req("out"));
    std::fs::write(out.with_extension("overlay.rgba"), &overlay).unwrap();
    preview::capture(w, h, bg, overlay, out);
}

fn plugin_scene(a: &Args, w: u32, h: u32, test: Test) -> (plugin::PluginScene, Option<String>) {
    let dir = PathBuf::from(a.req("plugin"));
    let effect = plugin::Effect::load(&dir).unwrap_or_else(|e| panic!("plugin {}: {e}", dir.display()));
    let shader = effect.shader.clone();
    eprintln!("loaded plugin {} in {:.2} ms", effect.id, effect.load_ms);
    (plugin::PluginScene::new(effect, w, h, lines_for(test), a.get("preset")), shader)
}

/// Same measurement as `bench`, with the generic plugin engine.
fn pbench(a: &Args) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test");
    let (w, h) = dims(a.get("orient").unwrap_or("land"));
    let out = PathBuf::from(a.req("out"));
    std::fs::create_dir_all(&out).unwrap();
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let mut gpu = engine::Engine::new(w, h).expect("gpu");
    let (mut scene, shader) = plugin_scene(a, w, h, test);
    gpu.set_hooks(shader.as_deref(), [0.0; 16]).expect("hooks");
    let load_ms = scene.effect.load_ms;
    let frames = (SONG_S * FPS).round() as usize;
    let mut total = Vec::with_capacity(frames);
    let mut line_change = Vec::new();
    let mut per_frame = String::from("frame,total_ms,build_ms,record_ms,wait_ms,gpu_ms,line_change,copy_or_finish_ms
");
    for f in 0..frames {
        let t = f as f64 / FPS;
        let t0 = Instant::now();
        let (glow, gain, main, changed) = scene.frame(&mut fonts, &gpu, t);
        let build = t0.elapsed().as_secs_f64() * 1000.0;
        let (_, ft) = gpu.frame(&glow, gain, &main, true).expect("frame");
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        total.push(ms);
        per_frame.push_str(&format!("{f},{ms:.3},{build:.3},{:.3},{:.3},{},{},{:.3}
", ft.pre_submit_ms, ft.submit_to_mapped_ms, ft.gpu_ms.map(|g| format!("{g:.3}")).unwrap_or_default(), changed as u8, ft.copy_out_ms));
        if changed {
            line_change.push(ms);
        }
    }
    std::fs::write(out.join("frames.csv"), per_frame).unwrap();
    let json = format!(
        "{{\n\"plugin\": {:?}, \"test\": {}, \"size\": [{w}, {h}], \"load_ms\": {load_ms:.3},\n{},\n{},\n{},\n\"frames_over_10_4\": {}, \"frames_over_50\": {}\n}}\n",
        scene.effect.id,
        test as u32,
        summary("total_ms", &total),
        summary("track_eval_us", &scene.tracks_us),
        summary("line_change_frame_ms", &line_change),
        total.iter().filter(|v| **v > 10.4).count(),
        total.iter().filter(|v| **v > 50.0).count()
    );
    std::fs::write(out.join("summary.json"), &json).unwrap();
    print!("{json}");
}

/// Instants fixed in advance (archi-greffons-v3 §6.3), from the word times.
fn instants(test: Test) -> Vec<(String, f64)> {
    let lines = lines_for(test);
    // line used for the states: first line starting at or after 114.4 s (the chorus excerpt)
    let li = lines.iter().position(|l| l.start >= 114.4).unwrap();
    let (l, next) = (&lines[li], &lines[li + 1]);
    let w = l.words.get(1).unwrap_or(&l.words[0]);
    let mut v = vec![
        ("line_start+0.05".to_string(), l.start + 0.05),
        ("line_end-0.05".to_string(), l.end - 0.05),
        ("next_line_first_frame".to_string(), (next.start * FPS).ceil() / FPS),
    ];
    match test {
        Test::Karaoke => v.push(("word1_mid_wipe".into(), (w.start + w.end) / 2.0)),
        Test::PopBlur | Test::Compo => {
            v.push(("word1_start+0.1".into(), w.start + 0.1));
            v.push(("word1_start+0.2".into(), w.start + 0.2));
        }
        Test::Neon => v.push(("word1_active_past_future".into(), (w.start + w.end) / 2.0)),
        Test::Wave => v.push(("line_mid".into(), (l.start + l.end) / 2.0)),
    }
    if test == Test::Compo {
        // Second line with several content words (SRT 23, "Made from both, becoming neither"):
        // carrier entry mid-animation, the `second` role (outline) and the full composition.
        let l2 = lines.iter().find(|l| l.start >= 108.0 && l.start < 109.5).expect("line 23");
        let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase();
        let rank = |i: usize| -> (bool, f64, usize) {
            let k = key(&l2.words[i].text);
            (!effects::STOP.contains(&k.as_str()) && !k.is_empty(), l2.words[i].end - l2.words[i].start, k.len())
        };
        let mut ranked: Vec<usize> = (0..l2.words.len()).collect();
        ranked.sort_by(|a, b| rank(*b).partial_cmp(&rank(*a)).unwrap());
        let (carrier, second) = (&l2.words[ranked[0]], &l2.words[ranked[1]]);
        v.push(("l2_carrier_start+0.05".into(), carrier.start + 0.05));
        v.push(("l2_carrier_start+0.1".into(), carrier.start + 0.1));
        v.push(("l2_second_start+0.05".into(), second.start + 0.05));
        v.push(("l2_line_end-0.05".into(), l2.end - 0.05));
    }
    v
}

fn source_frame(w: u32, h: u32, t: f64) -> Vec<u8> {
    let clip = env_path("NOH_PROTO_CLIP", "media/clip.mp4");
    let out = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{:.3}", t % 10.0), "-i"])
        .arg(&clip)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .output()
        .expect("ffmpeg frame");
    assert_eq!(out.stdout.len(), (w * h * 3) as usize, "source frame size");
    out.stdout
}

/// Straight-alpha sRGB overlay over an RGB background, in 8-bit sRGB (same for both renderers).
fn over(bg: &[u8], ov: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bg.len());
    for (b, o) in bg.chunks_exact(3).zip(ov.chunks_exact(4)) {
        let a = o[3] as f32 / 255.0;
        for c in 0..3 {
            out.push((o[c] as f32 * a + b[c] as f32 * (1.0 - a)).round() as u8);
        }
    }
    out
}

/// Plugin vs oracle (effects.rs) at the fixed instants, over the source frame and a flat background.
fn pcompare(a: &Args) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test");
    let orient = a.get("orient").unwrap_or("land");
    let (w, h) = dims(orient);
    let out = PathBuf::from(a.req("out"));
    std::fs::create_dir_all(&out).unwrap();
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let mut gpu_o = engine::Engine::new(w, h).expect("gpu");
    let mut gpu_p = engine::Engine::new(w, h).expect("gpu");
    let (mut pscene, shader) = plugin_scene(a, w, h, test);
    gpu_p.set_hooks(shader.as_deref(), [0.0; 16]).expect("hooks");
    let inst = instants(test);
    let list: Vec<String> = inst.iter().map(|(n, t)| format!("{{\"name\": {n:?}, \"t\": {t:.6}}}")).collect();
    std::fs::write(out.join("instants.json"), format!("[{}]\n", list.join(", "))).unwrap();
    for (name, t) in &inst {
        // fresh oracle scene per instant: no hidden state from earlier frames
        let mut oscene = Scene::new(test, w, h, false, lines_for(test));
        let (g, gain, m, _) = oscene.frame(&mut fonts, &gpu_o, *t);
        let (ov_o, _) = gpu_o.frame(&g, gain, &m, true).expect("oracle");
        let (g, gain, m, _) = pscene.frame(&mut fonts, &gpu_p, *t);
        let (ov_p, _) = gpu_p.frame(&g, gain, &m, true).expect("plugin");
        let mask: Vec<u8> = ov_o.chunks_exact(4).zip(ov_p.chunks_exact(4)).flat_map(|(x, y)| [0, 0, 0, x[3].max(y[3])]).collect();
        let tag = format!("{orient}-{name}");
        std::fs::write(out.join(format!("{tag}.mask.rgba")), &mask).unwrap();
        let src = source_frame(w, h, *t);
        let flat = vec![70u8; (w * h * 3) as usize];
        for (bg_name, bg) in [("source", &src), ("flat", &flat)] {
            std::fs::write(out.join(format!("{tag}.{bg_name}.oracle.rgb")), over(bg, &ov_o)).unwrap();
            std::fs::write(out.join(format!("{tag}.{bg_name}.plugin.rgb")), over(bg, &ov_p)).unwrap();
        }
        println!("{tag} t={t:.3}");
    }
}

/// Plan GL §A3: the same plugin scene rendered by wgpu (written as "oracle") and by
/// OpenGL (written as "plugin"), for one output type: --output straight|premul.
/// Premultiplied-gamma outputs are un-premultiplied before the blend, so the files
/// stay comparable with compare_blend.py.
fn glcompare(a: &Args) {
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test");
    let orient = a.get("orient").unwrap_or("land");
    let (w, h) = dims(orient);
    let out = PathBuf::from(a.req("out"));
    std::fs::create_dir_all(&out).unwrap();
    let mode = match a.get("output").unwrap_or("straight") {
        "premul" => gpu::Output::PremulGamma,
        _ => gpu::Output::StraightSrgb,
    };
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let mut wg = engine::Engine::Wgpu(gpu::Renderer::with_canvas(w, h, w as f32, h as f32, mode).expect("wgpu"));
    let mut gl = engine::Engine::Gl(gl::GlRenderer::headless(w, h, w as f32, h as f32, mode).expect("gl"));
    eprintln!("wgpu: {}
gl: {}", wg.adapter(), gl.adapter());
    let (_, shader) = plugin_scene(a, w, h, test);
    wg.set_hooks(shader.as_deref(), [0.0; 16]).expect("hooks wgpu");
    gl.set_hooks(shader.as_deref(), [0.0; 16]).expect("hooks gl");
    let straight = |px: Vec<u8>| -> Vec<u8> {
        if mode == gpu::Output::StraightSrgb {
            return px;
        }
        px.chunks_exact(4)
            .flat_map(|p| {
                let a = p[3] as f32;
                let un = |c: u8| if a == 0.0 { 0 } else { ((c as f32 * 255.0 / a).round()).min(255.0) as u8 };
                [un(p[0]), un(p[1]), un(p[2]), p[3]]
            })
            .collect()
    };
    let inst = instants(test);
    let list: Vec<String> = inst.iter().map(|(n, t)| format!("{{\"name\": {n:?}, \"t\": {t:.6}}}")).collect();
    std::fs::write(out.join("instants.json"), format!("[{}]
", list.join(", "))).unwrap();
    let mut identical = 0;
    for (name, t) in &inst {
        // fresh scenes per instant and engine: no hidden state from earlier frames
        let (mut sw, _) = plugin_scene(a, w, h, test);
        let (mut sg, _) = plugin_scene(a, w, h, test);
        let (g, gain, m, _) = sw.frame(&mut fonts, &wg, *t);
        let (ov_w, _) = wg.frame(&g, gain, &m, true).expect("wgpu frame");
        let (g, gain, m, _) = sg.frame(&mut fonts, &gl, *t);
        let (ov_g, _) = gl.frame(&g, gain, &m, true).expect("gl frame");
        if ov_w == ov_g {
            identical += 1;
        }
        let (ov_w, ov_g) = (straight(ov_w), straight(ov_g));
        let mask: Vec<u8> = ov_w.chunks_exact(4).zip(ov_g.chunks_exact(4)).flat_map(|(x, y)| [0, 0, 0, x[3].max(y[3])]).collect();
        let tag = format!("{orient}-{name}");
        std::fs::write(out.join(format!("{tag}.mask.rgba")), &mask).unwrap();
        let src = source_frame(w, h, *t);
        let flat = vec![70u8; (w * h * 3) as usize];
        for (bg_name, bg) in [("source", &src), ("flat", &flat)] {
            std::fs::write(out.join(format!("{tag}.{bg_name}.oracle.rgb")), over(bg, &ov_w)).unwrap();
            std::fs::write(out.join(format!("{tag}.{bg_name}.plugin.rgb")), over(bg, &ov_g)).unwrap();
        }
        println!("{tag} t={t:.3}");
    }
    println!("raw RGBA identical: {identical}/{}", inst.len());
}

/// Load every package under --dir and print accept/reject with the message.
fn pvalidate(a: &Args) {
    let dir = PathBuf::from(a.req("dir"));
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    entries.sort();
    let mut out = String::from("[\n");
    for (i, p) in entries.iter().enumerate() {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let res = plugin::Effect::load(p);
        let (ok, msg) = match &res {
            Ok(e) => (true, format!("loaded {} in {:.2} ms", e.id, e.load_ms)),
            Err(e) => (false, e.to_string()),
        };
        out += &format!("{}{{\"package\": {name:?}, \"accepted\": {ok}, \"message\": {msg:?}}}\n", if i == 0 { "" } else { "," });
    }
    out += "]\n";
    if let Some(o) = a.get("out") {
        std::fs::write(o, &out).unwrap();
    }
    print!("{out}");
}

fn pipe_cmd(a: &Args) {
    let exe = std::env::current_exe().unwrap();
    let n: usize = a.get("frames").unwrap_or("480").parse().unwrap();
    let mut out = String::from("{
");
    let sizes = [(1280u32, 720u32), (1920, 1080), (720, 1280), (1080, 1920)];
    for (i, (w, h)) in sizes.iter().enumerate() {
        let t = preview::pipe_roundtrip(&exe, *w, *h, n);
        out += &format!("{}{}
", summary(&format!("roundtrip_{w}x{h}_ms"), &t), if i + 1 < sizes.len() { "," } else { "" });
    }
    out += "}
";
    std::fs::write(a.req("out"), &out).unwrap();
    print!("{out}");
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = argv.first().cloned().unwrap_or_default();
    let a = Args::parse(&argv[1.min(argv.len())..]);
    match cmd.as_str() {
        "bench" => bench(&a),
        "video" => video(&a),
        "still" => still(&a),
        "shape-dump" => shape_dump(&a),
        "breaks" => breaks(&a),
        "preview" => preview_cmd(&a),
        "pipe" => pipe_cmd(&a),
        "overlay-raw" => overlay_raw(&a),
        "pbench" => pbench(&a),
        "pcompare" => pcompare(&a),
        "glcompare" => glcompare(&a),
        "relay" => relay(&a),
        "e4" => e4(&a),
        "pvalidate" => pvalidate(&a),
        "blend-capture" => blend_capture(&a),
        "echo" => preview::echo(a.req("bytes").parse().unwrap()),
        _ => {
            eprintln!("commands: bench | video | still | shape-dump");
            std::process::exit(2);
        }
    }
    let _ = Path::new(".");
}

/// Raw stdout/stdin as files: no line buffering on multi-megabyte frames.
fn raw_stdio() -> (std::mem::ManuallyDrop<std::fs::File>, std::mem::ManuallyDrop<std::fs::File>) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    let i = unsafe { std::fs::File::from_raw_handle(std::io::stdin().as_raw_handle()) };
    let o = unsafe { std::fs::File::from_raw_handle(std::io::stdout().as_raw_handle()) };
    (std::mem::ManuallyDrop::new(i), std::mem::ManuallyDrop::new(o))
}

/// Frame timestamps (plan GL v3 §B2 step 2): one integer tick per line, in 1/`tb`.
fn read_ts(path: &str) -> Vec<i64> {
    std::fs::read_to_string(path).expect("ts list").lines().filter(|l| !l.trim().is_empty()).map(|l| l.trim().parse().expect("tick")).collect()
}

/// D2 (plan GL v3 §B1): raw yuv420p frames in on stdin, the same frames out on stdout
/// in Matroska with their exact timestamps. No drawing. Fails if the number of frames
/// read differs from the timestamp list.
fn relay(a: &Args) {
    use std::io::{Read, Write};
    let (w, h) = parse_wh(a.req("size"));
    let ts = read_ts(a.req("ts"));
    let tb: f64 = a.f("tb", 49152.0);
    let frame = (w * h + 2 * (w.div_ceil(2) * h.div_ceil(2))) as usize;
    let (mut i, o) = raw_stdio();
    let mut out = mkv::Writer::new(std::io::BufWriter::with_capacity(1 << 16, &*o), w, h).expect("mkv header");
    let mut buf = vec![0u8; frame];
    let mut n = 0usize;
    let t0 = std::time::Instant::now();
    loop {
        match i.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => panic!("read: {e}"),
        }
        let Some(&tick) = ts.get(n) else {
            eprintln!("relay: more frames than timestamps ({} listed)", ts.len());
            std::process::exit(3);
        };
        let us = (tick as f64 * 1e6 / tb).round() as u64;
        out.frame(us, &buf).expect("write frame");
        n += 1;
    }
    out.finish().expect("flush");
    let _ = (&*o).flush();
    eprintln!("relay: {n} frames in {:.2} s", t0.elapsed().as_secs_f64());
    if n != ts.len() {
        eprintln!("relay: frame count {n} differs from timestamp list {}", ts.len());
        std::process::exit(3);
    }
}

/// E4 (plan GL v3 §B2): raw I420 frames in on stdin (from FFmpeg #1, `-fps_mode
/// passthrough`), the caption layer drawn by the OpenGL engine for each frame's own
/// timestamp, mixed in YUV on the GPU, and written out in Matroska with the same
/// timestamps. `--keep a,b,c --keep-dir D` saves, for those frame indices, the input
/// frame, the mixed frame (lossless) and the premultiplied overlay, for the checks.
fn e4(a: &Args) {
    use std::io::{Read, Write};
    let test = Test::from_num(a.req("test").parse().unwrap()).expect("test 1..5");
    let (w, h) = parse_wh(a.req("size"));
    let ts = read_ts(a.req("ts"));
    let tb: f64 = a.f("tb", 49152.0);
    let start = a.f("start", 0.0);
    let keep: Vec<usize> = a.get("keep").map(|k| k.split(',').map(|v| v.parse().unwrap()).collect()).unwrap_or_default();
    let keep_dir = a.get("keep-dir").map(PathBuf::from);
    let setup_started = Instant::now();
    let assets = env_path("NOH_PROTO_ASSETS", "assets");
    let serif = env_path("NOH_PROTO_SERIF", "NotoSerif-Italic.ttf");
    let mut fonts = text::Fonts::load(&assets, &serif).expect("fonts");
    let mut gpu = gl::GlRenderer::headless(w, h, w as f32, h as f32, gpu::Output::PremulGamma).expect("gl");
    let mut scene = Scene::new(test, w, h, false, lines_for(test));
    let mut mix = gl::YuvMix::new(&gpu).expect("yuv mix");
    eprintln!("e4: {} ; {} frames listed", gpu.adapter, ts.len());
    let (mut i, o) = raw_stdio();
    let mut out = mkv::Writer::new(std::io::BufWriter::with_capacity(1 << 16, &*o), w, h).expect("mkv header");
    let size = mix.frame_bytes();
    // --ready FILE: written once the engine, fonts, mixer and Matroska header are set up,
    // so a caller can start FFmpeg #1 only then (plan GL B2, start-up test).
    if let Some(r) = a.get("ready") {
        std::fs::write(r, format!("{:.3}
", setup_started.elapsed().as_secs_f64())).expect("ready file");
    }
    let mut buf = vec![0u8; size];
    let mut mixed = vec![0u8; size];
    let (mut read_ms, mut render_ms, mut mix_ms, mut write_ms) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut n = 0usize;
    let mut prev: Option<(usize, i64)> = None;
    let keep_mixed = |k: usize, data: &[u8]| {
        if let (true, Some(d)) = (keep.contains(&k), &keep_dir) {
            std::fs::write(d.join(format!("f{k}.mixed.yuv")), data).unwrap();
        }
    };
    let wall = Instant::now();
    loop {
        let t0 = Instant::now();
        match i.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => panic!("read: {e}"),
        }
        let Some(&tick) = ts.get(n) else {
            eprintln!("e4: more frames than timestamps ({} listed)", ts.len());
            std::process::exit(3);
        };
        let t1 = Instant::now();
        let t = start + tick as f64 / tb;
        let (glow_i, gain, main, _) = scene.frame(&mut fonts, &gpu, t);
        gpu.frame(&glow_i, gain, &main, false).expect("frame");
        let t2 = Instant::now();
        let slot = n % 2;
        mix.submit(&buf, gpu.output_texture(), slot);
        if keep.contains(&n) {
            if let Some(d) = &keep_dir {
                std::fs::create_dir_all(d).unwrap();
                std::fs::write(d.join(format!("f{n}.in.yuv")), &buf).unwrap();
                let (px, _) = gpu.frame(&glow_i, gain, &main, true).expect("overlay readback");
                std::fs::write(d.join(format!("f{n}.overlay_premul.rgba")), &px).unwrap();
                std::fs::write(d.join(format!("f{n}.t")), format!("{t:.6}\n")).unwrap();
            }
        }
        // Frame n-1 is done on the GPU by now (or nearly): copy it out and write it.
        if let Some((pn, ptick)) = prev.take() {
            mix.fetch(1 - slot, &mut mixed);
            keep_mixed(pn, &mixed);
            out.frame((ptick as f64 * 1e6 / tb).round() as u64, &mixed).expect("write frame");
        }
        prev = Some((n, tick));
        let t3 = Instant::now();
        let t4 = t3;
        read_ms.push((t1 - t0).as_secs_f64() * 1000.0);
        render_ms.push((t2 - t1).as_secs_f64() * 1000.0);
        mix_ms.push((t3 - t2).as_secs_f64() * 1000.0);
        write_ms.push((t4 - t3).as_secs_f64() * 1000.0);
        n += 1;
    }
    if let Some((pn, ptick)) = prev.take() {
        mix.fetch((pn) % 2, &mut mixed);
        keep_mixed(pn, &mixed);
        out.frame((ptick as f64 * 1e6 / tb).round() as u64, &mixed).expect("write frame");
    }
    out.finish().expect("flush");
    let _ = (&*o).flush();
    eprintln!(
        "e4: {n} frames in {:.2} s\n{}\n{}\n{}\n{}",
        wall.elapsed().as_secs_f64(),
        summary("read_wait_ms", &read_ms),
        summary("render_ms", &render_ms),
        summary("mix_submit_fetch_write_prev_ms", &mix_ms),
        summary("unused_ms", &write_ms)
    );
    if n != ts.len() {
        eprintln!("e4: frame count {n} differs from timestamp list {}", ts.len());
        std::process::exit(3);
    }
}
