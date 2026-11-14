//! Step A2 (Task 39): effects know their surface and choose a space per element.
use super::line::{Line, split_words};
use super::plugin::{Effect, PluginScene};
use super::shaders::{Inst, Upload};
use super::text::Fonts;
use std::path::Path;

struct NoGpu;
impl Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

/// A test-only effect of tests/fixtures/fx.
fn fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fx")
        .join(name)
        .join("effect.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn load(json: &serde_json::Value) -> Result<Effect, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("effect.json"),
        serde_json::to_vec(json).unwrap(),
    )
    .unwrap();
    Effect::load(dir.path()).map_err(|e| e.to_string())
}

fn lines() -> Vec<Line> {
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    vec![split_words(
        0.5,
        2.5,
        &w("a reasonably long caption line that must wrap inside"),
        " ",
    )]
}

/// YouTube Shorts insets on 1080x1920 (safe_area.rs): left, top, right, bottom.
const YT: [u32; 4] = [48, 288, 192, 672];

fn glyphs(effect: Effect, insets: Option<[u32; 4]>, t: f64) -> Vec<Inst> {
    let mut fonts = Fonts::load().unwrap();
    let mut scene = PluginScene::new(effect, 1080, 1920, lines(), None).with_safe_insets(insets);
    scene.frame(&mut fonts, &NoGpu, t).2
}

#[test]
fn space_is_validated_and_the_safe_rect_comes_from_insets() {
    let mut j = fixture("wipe");
    j["layout"]["box"]["space"] = serde_json::json!("screen");
    assert!(load(&j).unwrap_err().contains("space"));
    j["layout"]["box"]["space"] = serde_json::json!("canvas");
    assert!(load(&j).is_ok());
    let scene = PluginScene::new(load(&fixture("wipe")).unwrap(), 1080, 1920, lines(), None);
    assert_eq!(scene.safe_rect(), [0.0, 0.0, 1080.0, 1920.0]);
    let scene = scene.with_safe_insets(Some(YT));
    assert_eq!(scene.safe_rect(), [48.0, 288.0, 840.0, 960.0]);
}

#[test]
fn a_safe_box_wraps_and_sits_inside_the_safe_rect() {
    // The text box spans the safe width (0.86 of 840 px, centred in it), not the canvas.
    let safe = glyphs(load(&fixture("wipe")).unwrap(), Some(YT), 1.5);
    let full = glyphs(load(&fixture("wipe")).unwrap(), None, 1.5);
    let (sx0, sx1) = (48.0, 48.0 + 840.0);
    let xs = |g: &[Inst]| {
        g.iter()
            .map(|i| i.center[0])
            .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
    };
    let (lo, hi) = xs(&safe);
    assert!(
        lo > sx0 && hi < sx1,
        "glyph centres {lo}..{hi} outside {sx0}..{sx1}"
    );
    // Same text, narrower box: more lines, so the glyphs span more distinct rows.
    let rows = |g: &[Inst]| {
        let mut ys: Vec<i32> = g
            .iter()
            .map(|i| (i.center[1] / 10.0).round() as i32)
            .collect();
        ys.sort();
        ys.dedup();
        ys.len()
    };
    assert!(rows(&safe) >= rows(&full));
    // The box's y is a fraction of the safe height, from the safe top.
    let mean_y = |g: &[Inst]| g.iter().map(|i| i.center[1]).sum::<f32>() / g.len() as f32;
    assert!(mean_y(&safe) > 288.0 && mean_y(&safe) < 288.0 + 960.0);
}

#[test]
fn canvas_space_ignores_the_safe_rect() {
    let mut j = fixture("wipe");
    j["layout"]["box"]["space"] = serde_json::json!("canvas");
    let with = glyphs(load(&j).unwrap(), Some(YT), 1.5);
    let without = glyphs(load(&j).unwrap(), None, 1.5);
    let key = |g: &[Inst]| bytemuck::cast_slice::<Inst, u8>(g).to_vec();
    assert_eq!(key(&with), key(&without));
}

#[test]
fn composition_shapes_and_words_follow_their_own_space() {
    // Shapes in the default space (safe) shrink into the safe rect with the words.
    // (The compose fixture puts its plate in canvas space, as step C did.)
    let mut j = fixture("compose");
    for s in j["layout"]["compose"]["shapes"].as_array_mut().unwrap() {
        s.as_object_mut().unwrap().remove("space");
    }
    let effect = load(&j).unwrap();
    let plates = |g: &[Inst]| {
        g.iter()
            .filter(|i| i.params[0] == 2.0)
            .map(|i| (i.center, i.size))
            .collect::<Vec<_>>()
    };
    let safe = glyphs(effect.clone(), Some(YT), 1.5);
    for (c, s) in plates(&safe) {
        assert!(
            c[0] - s[0] / 2.0 >= 48.0 - 0.01 && c[0] + s[0] / 2.0 <= 888.0 + 0.01,
            "{c:?} {s:?}"
        );
        assert!(
            c[1] - s[1] / 2.0 >= 288.0 - 0.01 && c[1] + s[1] / 2.0 <= 1248.0 + 0.01,
            "{c:?} {s:?}"
        );
    }
    // Shapes in canvas space keep their full-canvas geometry under a safe area.
    let mut j = fixture("compose");
    for s in j["layout"]["compose"]["shapes"].as_array_mut().unwrap() {
        s["space"] = serde_json::json!("canvas");
    }
    let canvas_shapes = load(&j).unwrap();
    let a = plates(&glyphs(canvas_shapes.clone(), Some(YT), 1.5));
    let b = plates(&glyphs(canvas_shapes, None, 1.5));
    assert_eq!(a, b);
}

/// Step C: a compose effect with its plate in canvas space keeps it on the whole
/// canvas under a safe area, while its words move into the safe surface.
#[test]
fn compose_keeps_its_plate_full_canvas() {
    let effect = load(&fixture("compose")).unwrap();
    let plates = |g: &[Inst]| {
        g.iter()
            .filter(|i| i.params[0] == 2.0)
            .map(|i| (i.center, i.size))
            .collect::<Vec<_>>()
    };
    let words = |g: &[Inst]| {
        g.iter()
            .filter(|i| i.params[0] != 2.0)
            .map(|i| i.center)
            .collect::<Vec<_>>()
    };
    let safe = glyphs(effect.clone(), Some(YT), 1.5);
    let full = glyphs(effect, None, 1.5);
    assert_eq!(plates(&safe), plates(&full), "the plate follows the canvas");
    assert_ne!(
        words(&safe),
        words(&full),
        "the words follow the safe surface"
    );
}
