//! Step A1 (Task 39): a window of lines (past, active, upcoming) with a lead time.
use super::line::{Line, split_words};
use super::plugin::{ACTIVE, Effect, NEXT, PAST, PluginScene};
use super::shaders::{Inst, Upload};
use super::text::Fonts;
use std::path::Path;

struct NoGpu;
impl Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

fn karaoke_json() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe/effect.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Load an effect from JSON through the real loader (a temporary package folder).
fn load(json: &serde_json::Value) -> Result<Effect, String> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("effect.json"),
        serde_json::to_vec(json).unwrap(),
    )
    .unwrap();
    Effect::load(dir.path()).map_err(|e| e.to_string())
}

fn with_window(window: serde_json::Value) -> serde_json::Value {
    let mut j = karaoke_json();
    j["layout"]["window"] = window;
    j
}

fn lines() -> Vec<Line> {
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    vec![
        split_words(1.0, 3.0, &w("first line here"), " "),
        split_words(4.0, 6.0, &w("second line now"), " "),
        split_words(10.0, 12.0, &w("third after a gap"), " "),
    ]
}

#[test]
fn window_is_validated() {
    assert!(
        load(&with_window(
            serde_json::json!({"before": 1, "after": 1, "lead": 2.5, "gap": 0.35})
        ))
        .is_ok()
    );
    for bad in [
        serde_json::json!({"after": 4}),
        serde_json::json!({"lead": 11}),
        serde_json::json!({"lead": -1}),
        serde_json::json!({"gap": 5}),
        serde_json::json!({"after": 1.5}),
        serde_json::json!({"later": 1}),
    ] {
        assert!(load(&with_window(bad.clone())).is_err(), "{bad} accepted");
    }
    // A window is line mode only: Composition refuses it.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/compose/effect.json");
    let mut j: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    j["layout"]["window"] = serde_json::json!({"after": 1});
    let e = load(&j).unwrap_err();
    assert!(e.contains("layout.window"), "{e}");
}

#[test]
fn by_state_needs_active_and_known_states() {
    let mut j = karaoke_json();
    j["layout"]["size"] = serde_json::json!({"by_state": {"next": 40}});
    assert!(load(&j).unwrap_err().contains("active is required"));
    j["layout"]["size"] = serde_json::json!({"by_state": {"active": 64, "later": 40}});
    assert!(load(&j).is_err());
    j["layout"]["size"] = serde_json::json!({"by_state": {"active": 64, "next": 40}});
    assert!(load(&j).is_ok());
}

#[test]
fn visible_lines_follow_the_window_and_the_lead() {
    let effect = load(&with_window(
        serde_json::json!({"before": 1, "after": 1, "lead": 2.5}),
    ))
    .unwrap();
    let scene = PluginScene::new(effect, 1080, 1920, lines(), None);
    let v = |t: f64| scene.visible_lines(t);
    // Before the lead of the first line: nothing. Within it: line 0 in the active slot, still NEXT.
    assert!(v(-2.0).is_empty());
    assert_eq!(v(0.0), vec![(0, NEXT, 0)]);
    // Line 0 active; line 1 starts at 4.0, shown from 1.5.
    assert_eq!(v(1.2), vec![(0, ACTIVE, 0)]);
    assert_eq!(v(2.0), vec![(0, ACTIVE, 0), (1, NEXT, 1)]);
    // Between lines (3.0..4.0): line 1 takes the active slot as NEXT.
    assert_eq!(v(3.5), vec![(1, NEXT, 0)]);
    // Line 1 active: line 0 above as PAST; line 2 (10.0) is beyond its lead.
    assert_eq!(v(5.0), vec![(0, PAST, -1), (1, ACTIVE, 0)]);
    // Long gap: nothing until line 2's lead (7.5), then line 2 alone.
    assert!(v(7.0).is_empty());
    assert_eq!(v(8.0), vec![(2, NEXT, 0)]);
    assert_eq!(v(11.0), vec![(1, PAST, -1), (2, ACTIVE, 0)]);
    assert!(v(12.5).is_empty());
}

#[test]
fn without_a_window_only_the_active_line_shows() {
    let effect = load(&karaoke_json()).unwrap();
    let scene = PluginScene::new(effect, 1080, 1920, lines(), None);
    assert_eq!(scene.visible_lines(2.0), vec![(0, ACTIVE, 0)]);
    assert!(scene.visible_lines(3.5).is_empty());
}

/// Vertical centre of the glyphs drawn for each word text, at time t.
fn glyph_rows(scene: &mut PluginScene, fonts: &mut Fonts, t: f64) -> Vec<f32> {
    let (_, _, main, _) = scene.frame(fonts, &NoGpu, t);
    let mut ys: Vec<f32> = main
        .iter()
        .filter(|i: &&Inst| i.params[0] != 2.0)
        .map(|i| i.center[1])
        .collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys
}

#[test]
fn upcoming_lines_stack_below_and_take_their_state_style() {
    let mut fonts = Fonts::load().unwrap();
    let mut j = with_window(serde_json::json!({"after": 1, "lead": 2.5, "gap": 0.5}));
    // The upcoming line is smaller: the layout size depends on the line's state.
    j["layout"]["size"] = serde_json::json!({"by_state": {"active": 80, "next": 40}});
    let effect = load(&j).unwrap();
    let mut scene = PluginScene::new(effect.clone(), 1080, 1920, lines(), None);
    // At 2.0: line 0 active and line 1 upcoming.
    let both = glyph_rows(&mut scene, &mut fonts, 2.0);
    // At 1.2: line 0 alone, at the same place as at 2.0 (the active slot does not move).
    let alone = glyph_rows(&mut scene, &mut fonts, 1.2);
    let active_y = alone.iter().cloned().fold(f32::MIN, f32::max);
    assert!(
        both.iter().any(|y| (y - active_y).abs() < 0.01),
        "active line moved"
    );
    // Line 1's glyphs sit below line 0's.
    let next_ys: Vec<f32> = both
        .iter()
        .cloned()
        .filter(|y| *y > active_y + 1.0)
        .collect();
    assert!(!next_ys.is_empty(), "no upcoming line below the active one");
    // Smaller font: the upcoming glyph quads are smaller than the active ones.
    let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, 2.0);
    let height_at = |y0: f32, y1: f32| {
        main.iter()
            .filter(|i| i.center[1] >= y0 && i.center[1] <= y1)
            .map(|i| i.size[1] * i.xform[2])
            .fold(0.0f32, f32::max)
    };
    assert!(height_at(active_y + 1.0, f32::MAX) < height_at(f32::MIN, active_y + 0.5));
}

#[test]
fn by_offset_styles_each_upcoming_line_apart() {
    let mut j = with_window(serde_json::json!({"after": 2, "lead": 9, "gap": 0.2}));
    j["layout"]["size"] = serde_json::json!({"by_offset": [80, 50, 30]});
    let effect = load(&j).unwrap();
    // Three lines close together: at 2.0 line 0 is active, lines 1 and 2 within the lead.
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let near = vec![
        split_words(1.0, 3.0, &w("one"), " "),
        split_words(4.0, 6.0, &w("two"), " "),
        split_words(7.0, 9.0, &w("three"), " "),
    ];
    let mut scene = PluginScene::new(effect, 1080, 1920, near, None);
    assert_eq!(
        scene.visible_lines(2.0),
        vec![(0, ACTIVE, 0), (1, NEXT, 1), (2, NEXT, 2)]
    );
    let mut fonts = Fonts::load().unwrap();
    let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, 2.0);
    // Glyph heights grouped by row, top to bottom: each upcoming row is smaller.
    let mut rows: Vec<(f32, f32)> = main
        .iter()
        .map(|i| (i.center[1], i.size[1] * i.xform[2]))
        .collect();
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut tallest: Vec<f32> = Vec::new();
    let mut last_y = f32::MIN;
    for (y, h) in rows {
        if y - last_y > 20.0 {
            tallest.push(h);
        } else if let Some(t) = tallest.last_mut() {
            *t = t.max(h);
        }
        last_y = y;
    }
    assert_eq!(tallest.len(), 3, "{tallest:?}");
    assert!(
        tallest[0] > tallest[1] && tallest[1] > tallest[2],
        "{tallest:?}"
    );
    // The list length is checked.
    j["layout"]["size"] = serde_json::json!({"by_offset": []});
    assert!(load(&j).is_err());
}

/// Step C: the bundled "karaoke with next line" shows the next line 2.5 s ahead,
/// below the sung one, smaller and paler; the karaoke itself is unchanged.
#[test]
fn karaoke_with_next_line_shows_it_below_smaller_and_paler() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/window");
    let effect = Effect::load(&dir).unwrap();
    let mut scene = PluginScene::new(effect, 1920, 1080, lines(), None);
    // Line 1 starts at 4.0: shown from 1.5, during line 0.
    assert_eq!(scene.visible_lines(1.2), vec![(0, ACTIVE, 0)]);
    assert_eq!(scene.visible_lines(2.0), vec![(0, ACTIVE, 0), (1, NEXT, 1)]);
    let mut fonts = Fonts::load().unwrap();
    let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, 2.0);
    // Fill glyphs (alpha > 0 in color[3]): two rows, the lower one smaller and paler.
    let fills: Vec<&Inst> = main
        .iter()
        .filter(|i| i.color[3] > 0.0 && i.params[1] == 0.0)
        .collect();
    let mid = (fills.iter().map(|i| i.center[1]).fold(f32::MAX, f32::min)
        + fills.iter().map(|i| i.center[1]).fold(f32::MIN, f32::max))
        / 2.0;
    let (upper, lower): (Vec<&&Inst>, Vec<&&Inst>) = fills.iter().partition(|i| i.center[1] < mid);
    assert!(!upper.is_empty() && !lower.is_empty());
    // Ink height: the sprite minus its atlas padding (coverage 24 texels, MSDF range).
    let ink = |i: &Inst| {
        let pad = if i.params[0] == 1.0 {
            super::shaders::MSDF_RANGE
        } else {
            super::text::COV_PAD as f32
        } * i.params[3];
        (i.size[1] - 2.0 * pad) * i.xform[2]
    };
    let size = |v: &[&&Inst]| v.iter().map(|i| ink(i)).fold(0.0f32, f32::max);
    let alpha = |v: &[&&Inst]| v.iter().map(|i| i.color[3]).fold(0.0f32, f32::max);
    assert!(
        size(&lower) < size(&upper) * 0.8,
        "next line not smaller: {} vs {}",
        size(&lower),
        size(&upper)
    );
    assert!(alpha(&lower) < alpha(&upper) * 0.7, "next line not paler");
    // The karaoke itself still has no window.
    let karaoke =
        Effect::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe"))
            .unwrap();
    assert_eq!(
        PluginScene::new(karaoke, 1920, 1080, lines(), None).visible_lines(2.0),
        vec![(0, ACTIVE, 0)]
    );
}
