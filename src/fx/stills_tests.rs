//! Mockup check for the caption plugins (lyrics step 7): each bundled plugin drawn
//! on the report mockup's own lyrics and word times, at the instants the mockup
//! stills were taken, over the mockup's background. Opt-in (needs a GPU and
//! FFmpeg on PATH): `NOH_FX_STILLS=<dir>` and `NOH_FX_STILLS_BG=<background.png>`
//! (optional `NOH_FX_STILLS_ROOT=<plugins folder>`, `NOH_FX_STILLS_MS=<ms,ms>`,
//! `NOH_FX_STILLS_SIZE=<w>x<h>`, `NOH_FX_STILLS_VALUES=<id>=<choice>,…`,
//! `NOH_FX_STILLS_CUES=words`), run with
//! `--include-ignored --test-threads=1 fx::stills_tests`. Each run also writes
//! `overflow.txt`: whether the engine reports letters outside the frame per still.
use super::catalog::scan;
use super::line::{Line, Word};
use super::plugin::PluginScene;
use super::shaders::Output;
use super::text::Fonts;
use std::path::{Path, PathBuf};

/// The mockup's lines (index.html, LINES): sung word times in seconds; each cue
/// ends where the next line starts, the last at 11 s (the mockup's END).
pub(crate) fn mockup_lines() -> Vec<Line> {
    let raw: [&[(&str, f64, f64)]; 4] = [
        &[
            ("On", 0.4, 0.65),
            ("a", 0.65, 0.8),
            ("gardé", 0.8, 1.3),
            ("la", 1.3, 1.45),
            ("lumière", 1.45, 2.6),
        ],
        &[
            ("même", 2.9, 3.2),
            ("quand", 3.2, 3.45),
            ("la", 3.45, 3.6),
            ("nuit", 3.6, 4.0),
            ("tombait", 4.0, 5.1),
        ],
        &[
            ("et", 5.4, 5.55),
            ("nos", 5.55, 5.8),
            ("voix", 5.8, 6.6),
            ("dans", 6.6, 6.8),
            ("la", 6.8, 6.95),
            ("rue", 6.95, 7.9),
        ],
        &[
            ("faisaient", 8.2, 8.6),
            ("trembler", 8.6, 9.2),
            ("les", 9.2, 9.35),
            ("vitrines", 9.35, 10.6),
        ],
    ];
    // NOH_FX_STILLS_CUES=words: each cue ends with its last word instead, as in
    // a real subtitle file (a gap before the next line).
    let word_ends = std::env::var("NOH_FX_STILLS_CUES").as_deref() == Ok("words");
    let mut lines = Vec::new();
    for (k, words) in raw.iter().enumerate() {
        let end = if word_ends {
            words[words.len() - 1].2
        } else {
            raw.get(k + 1).map_or(11.0, |next| next[0].1)
        };
        lines.push(Line {
            voices: Vec::new(),
            start: words[0].1,
            end,
            words: words
                .iter()
                .map(|&(text, start, end)| Word {
                    backing: false,
                    text: text.into(),
                    start,
                    end,
                    syllables: Vec::new(),
                })
                .collect(),
            sep: " ".into(),
            breaks: Vec::new(),
        });
    }
    lines
}

/// The instants of the mockup stills, in ms.
pub(crate) const INSTANTS_MS: [u32; 6] = [300, 900, 1500, 1900, 2800, 3700];

#[test]
#[ignore = "mockup stills: needs NOH_FX_STILLS, NOH_FX_STILLS_BG, FFmpeg on PATH and a GPU"]
fn draw_the_plugins_on_the_mockup_lyrics() {
    use super::gl::GlRenderer;
    let Some(dir) = std::env::var_os("NOH_FX_STILLS").map(PathBuf::from) else {
        return;
    };
    let bg = PathBuf::from(std::env::var_os("NOH_FX_STILLS_BG").expect("NOH_FX_STILLS_BG"));
    std::fs::create_dir_all(&dir).unwrap();
    let ffmpeg = crate::find_ffmpeg(None).unwrap();
    // NOH_FX_STILLS_SIZE=<w>x<h> (default 1280x720; the background is cropped to fill).
    let sized = std::env::var_os("NOH_FX_STILLS_SIZE").is_some();
    let (w, h) = std::env::var("NOH_FX_STILLS_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
        })
        .unwrap_or((1280, 720));
    // NOH_FX_STILLS_VALUES=<id>=<choice>,…: string parameter values (e.g. size=large).
    let values: std::collections::BTreeMap<String, serde_json::Value> =
        std::env::var("NOH_FX_STILLS_VALUES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), serde_json::Value::from(v.trim())))
            .collect();
    // One line per still: whether the engine reports letters outside the frame.
    let mut overflow = String::new();
    let mut fonts = Fonts::load().unwrap();
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
    // NOH_FX_STILLS_ROOT: another folder of plugins (drafts), else the bundled ones.
    let root = std::env::var_os("NOH_FX_STILLS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects"));
    let catalog = scan(&[root]);
    for entry in catalog
        .effects
        .iter()
        .filter(|e| e.effect.id != "noh.classic")
    {
        let preset = entry.effect.presets[0].0.clone();
        // Values a plugin does not have (size on a plugin without it) are skipped.
        let own: std::collections::BTreeMap<String, serde_json::Value> = values
            .iter()
            .filter(|(k, _)| entry.effect.params.iter().any(|p| &p.id == *k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut scene = PluginScene::new(entry.effect.clone(), w, h, mockup_lines(), Some(&preset))
            .with_values(&own)
            .unwrap()
            .with_frame_check();
        gpu.set_hooks(entry.effect.shader.as_deref(), scene.hook_user())
            .unwrap();
        // NOH_FX_STILLS_MS: other instants, comma-separated ms.
        let instants: Vec<u32> = match std::env::var("NOH_FX_STILLS_MS") {
            Ok(list) => list
                .split(',')
                .map(|ms| ms.trim().parse().unwrap())
                .collect(),
            Err(_) => INSTANTS_MS.to_vec(),
        };
        for ms in instants {
            let t = f64::from(ms) / 1000.0;
            let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, t);
            let (overlay, _) = gpu.frame(&g, gain, &m, true).unwrap();
            let raw = dir.join("overlay.rgba");
            std::fs::write(&raw, &overlay).unwrap();
            let name = entry.effect.id.trim_start_matches("noh.");
            overflow.push_str(&format!("{name} {ms} overflow={}\n", scene.overflowed()));
            let out = dir.join(format!("{name}-{ms}.png"));
            // The overlay is premultiplied, gamma-encoded RGBA, top row first.
            let status = std::process::Command::new(&ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(&bg)
                .args([
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-s",
                    &format!("{w}x{h}"),
                    "-i",
                ])
                .arg(&raw)
                .args([
                    "-filter_complex",
                    // The mockup background is 1280x721: stretched at the default
                    // size (the stills as they always were), cropped to fill otherwise.
                    &if sized {
                        format!(
                            "[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,\
                             crop={w}:{h}[b];[b][1:v]overlay=alpha=premultiplied:format=rgb"
                        )
                    } else {
                        "[0:v]scale=1280:720[b];[b][1:v]overlay=alpha=premultiplied:format=rgb"
                            .to_string()
                    },
                    "-frames:v",
                    "1",
                ])
                .arg(&out)
                .status()
                .unwrap();
            assert!(status.success(), "{}", out.display());
        }
    }
    std::fs::write(dir.join("overflow.txt"), overflow).unwrap();
}
