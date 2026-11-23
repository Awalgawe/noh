//! Step A3 (Task 39): report letters drawn outside the safe surface; change nothing.
use super::line::{Line, split_words};
use super::plugin::{Effect, PluginScene, ink_outside};
use crate::fx::shaders::Upload;
use crate::fx::text::Fonts;
use std::path::Path;

struct NoGpu;
impl Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

/// YouTube Shorts insets on 1080x1920, and the safe rect they give.
const YT: [u32; 4] = [48, 288, 192, 672];
const SAFE: [f32; 4] = [48.0, 288.0, 840.0, 960.0];

#[test]
fn ink_inside_with_padding_across_the_edge_is_not_reported() {
    // A 100 px sprite with 24 px padding: 52 px of ink. Ink 4 px inside the left
    // edge while the padded sprite reaches 20 px past it: no report.
    let center = [48.0 + 4.0 + 26.0, 600.0];
    assert!(!ink_outside(center, [26.0, 26.0], 0.0, SAFE));
    // Within the 2 px tolerance: still none. Beyond it: reported.
    assert!(!ink_outside(
        [48.0 - 1.5 + 26.0, 600.0],
        [26.0, 26.0],
        0.0,
        SAFE
    ));
    assert!(ink_outside(
        [48.0 - 3.0 + 26.0, 600.0],
        [26.0, 26.0],
        0.0,
        SAFE
    ));
    // Below the safe bottom (288 + 960 = 1248).
    assert!(ink_outside([400.0, 1248.0], [26.0, 26.0], 0.0, SAFE));
}

#[test]
fn rotation_moves_the_corners_that_are_checked() {
    // Unrotated, the box touches nothing; rotated 45°, a corner passes the edge.
    let center = [48.0 + 40.0, 600.0];
    assert!(!ink_outside(center, [36.0, 36.0], 0.0, SAFE));
    assert!(ink_outside(
        center,
        [36.0, 36.0],
        std::f32::consts::FRAC_PI_4,
        SAFE
    ));
}

fn karaoke(size: f64) -> Effect {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe/effect.json");
    let mut j: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    j["layout"]["size"] = serde_json::json!(size);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("effect.json"),
        serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    Effect::load(dir.path()).unwrap()
}

fn lines(text: &str) -> Vec<Line> {
    let w: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    vec![split_words(0.5, 2.5, &w, " ")]
}

/// Whether any frame of the cue (every 1/20 s) is reported.
fn reported(effect: Effect, insets: Option<[u32; 4]>, text: &str) -> bool {
    let mut fonts = Fonts::load().unwrap();
    let mut scene =
        PluginScene::new(effect, 1080, 1920, lines(text), None).with_safe_insets(insets);
    (0..60).any(|f| {
        scene.frame(&mut fonts, &NoGpu, f as f64 / 20.0);
        scene.overflowed()
    })
}

#[test]
fn an_ordinary_line_is_not_reported_and_a_giant_word_is() {
    assert!(!reported(
        karaoke(76.0),
        Some(YT),
        "nothing feels the same tonight"
    ));
    // One word far wider than the 840 px safe width at 400 px: drawn, and reported.
    assert!(reported(karaoke(400.0), Some(YT), "Extraordinairement"));
    // Without a safe area nothing is checked, whatever is drawn.
    assert!(!reported(karaoke(400.0), None, "Extraordinairement"));
}

#[test]
fn reporting_changes_no_instance() {
    // The check only reads: the same scene with and without a safe area that the
    // text does not reach draws the same instances when the box is in canvas space.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe/effect.json");
    let mut j: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    j["layout"]["box"]["space"] = serde_json::json!("canvas");
    j["layout"]["size"] = serde_json::json!(400.0);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("effect.json"),
        serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    let effect = Effect::load(dir.path()).unwrap();
    let mut fonts = Fonts::load().unwrap();
    let mut a = PluginScene::new(
        effect.clone(),
        1080,
        1920,
        lines("Extraordinairement"),
        None,
    )
    .with_safe_insets(Some(YT));
    let mut b = PluginScene::new(effect, 1080, 1920, lines("Extraordinairement"), None);
    let (_, _, ma, _) = a.frame(&mut fonts, &NoGpu, 1.5);
    let (_, _, mb, _) = b.frame(&mut fonts, &NoGpu, 1.5);
    assert!(a.overflowed() && !b.overflowed());
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&ma),
        bytemuck::cast_slice::<_, u8>(&mb)
    );
}

/// Measurement (plan check 5), run on demand: `NOH_FX_OVERFLOW_SRT=<lyrics.srt>`.
/// Every bundled effect and preset, in each Short safe area (1080x1920), over every
/// frame (25 fps) of every cue: prints the first time each one overflows, or "ok".
#[test]
#[ignore = "measurement: needs NOH_FX_OVERFLOW_SRT"]
fn overflow_report_on_real_lyrics() {
    let Some(srt) = std::env::var_os("NOH_FX_OVERFLOW_SRT") else {
        return;
    };
    let text = std::fs::read_to_string(srt).unwrap();
    let track = crate::subtitle_srt::parse(&text, 3_600_000).unwrap();
    let lines = super::line::from_track(&track);
    let end = lines.iter().map(|l| l.end).fold(0.0, f64::max);
    let catalog = super::catalog::scan(&[
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx"),
    ]);
    let mut effects: Vec<_> = catalog.effects.iter().collect();
    effects.sort_by(|a, b| a.effect.id.cmp(&b.effect.id));
    let mut fonts = Fonts::load().unwrap();
    for area in [
        crate::safe_area::SafeArea::YoutubeShorts,
        crate::safe_area::SafeArea::Tiktok,
        crate::safe_area::SafeArea::Universal,
    ] {
        for e in &effects {
            for (preset, _) in &e.effect.presets {
                let mut scene =
                    PluginScene::new(e.effect.clone(), 1080, 1920, lines.clone(), Some(preset))
                        .with_safe_insets(area.insets(1080, 1920));
                let first = (0..(end * 25.0) as usize)
                    .map(|f| f as f64 / 25.0)
                    .find(|t| {
                        scene.frame(&mut fonts, &NoGpu, *t);
                        scene.overflowed()
                    });
                println!(
                    "{area:?}\t{}\t{preset}\t{}",
                    e.effect.id,
                    first.map_or("ok".into(), |t| format!("overflow at {t:.2} s"))
                );
            }
        }
    }
}

/// Task 43: Classique's exports check letters against the whole picture when
/// there is no safe area (`with_frame_check`, Classique only). Classique at its
/// defaults draws ordinary cues, across their whole time, inside 1920x1080 and
/// 1080x1920: no "captions leave the picture" warning on ordinary exports. (Other
/// effects are not checked: Typographic composition slides words in from outside
/// the picture, measured at 1 s with letter centres from x -1818 to 3644 px.)
#[test]
fn ordinary_cues_stay_inside_the_picture_with_classique() {
    let mut fonts = Fonts::load().unwrap();
    let cues = [
        "ever since the impact nothing feels the same tonight",
        "and the city lights keep falling down on me",
        "夜空に光る星を数えながら",
    ];
    for (w, h) in [(1920u32, 1080u32), (1080, 1920)] {
        let lines: Vec<Line> = cues
            .iter()
            .enumerate()
            .map(|(k, c)| {
                split_words(
                    k as f64 * 2.5,
                    k as f64 * 2.5 + 2.4,
                    &c.split_whitespace().map(str::to_string).collect::<Vec<_>>(),
                    " ",
                )
            })
            .collect();
        for size in ["small", "medium", "large"] {
            let values = [("size".to_string(), serde_json::json!(size))]
                .into_iter()
                .collect();
            let mut scene =
                PluginScene::new(super::catalog::builtin_classic(), w, h, lines.clone(), None)
                    .with_values(&values)
                    .unwrap()
                    .with_frame_check();
            let out = (0..75 * 3).map(|f| f as f64 / 30.0).find(|t| {
                scene.frame(&mut fonts, &NoGpu, *t);
                scene.overflowed()
            });
            assert!(
                out.is_none(),
                "Classique {size} at {w}x{h}: off the picture at {out:?} s"
            );
        }
    }
}
