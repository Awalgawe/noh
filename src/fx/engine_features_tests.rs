//! Engine additions for the classic caption styles: a timed return to the past
//! state, plates, a soft wipe edge, a gliding window, the line's scale, rotation
//! and rank, left/right alignment, and what shader hooks are told about the
//! pixel, the line and the pass.
use super::line::{Line, split_words};
use super::plugin::{Effect, PluginScene, R};
use super::shaders::{Inst, PASS_FILL, PASS_GLOW, PASS_OUTLINE, PASS_PLATE, PASS_SHADOW, Upload};
use super::text::Fonts;
use serde_json::{Value, json};
use std::path::Path;

struct NoGpu;
impl Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

fn effect(layout: Value, animate: Value, passes: Value) -> Result<Effect, String> {
    effect_with(json!([]), layout, animate, passes)
}

fn effect_with(
    params: Value,
    layout: Value,
    animate: Value,
    passes: Value,
) -> Result<Effect, String> {
    let j = json!({
        "engine": 1, "id": "test.features", "name": {"en": "Features"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text", "params": params,
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": layout, "animate": animate, "render": {"passes": passes}
    });
    Effect::from_json(Path::new(""), j.to_string().as_bytes()).map_err(|e| e.0)
}

fn sans() -> Value {
    json!({"font": "sans", "size": 64})
}

fn fill() -> Value {
    json!([{"kind": "fill", "color": "#FFFFFF"}])
}

fn words(start: f64, end: f64, text: &str) -> Line {
    let w: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    split_words(start, end, &w, " ")
}

/// « ab cd » from 1 s to 2 s: « ab » sung 1.0 to 1.5, « cd » 1.5 to 2.0.
fn ab_cd() -> Vec<Line> {
    vec![words(1.0, 2.0, "ab cd")]
}

/// (glow, main) instances at `t`.
fn at(e: &Effect, lines: Vec<Line>, t: f64) -> (Vec<Inst>, Vec<Inst>) {
    let mut fonts = Fonts::load().unwrap();
    let mut scene = PluginScene::new(e.clone(), 1920, 1080, lines, Some("p"));
    let (g, _, m, _) = scene.frame(&mut fonts, &NoGpu, t);
    (g, m)
}

fn is_plate(i: &Inst) -> bool {
    i.params[0] > 1.5
}

/// Fill letters, left to right.
fn letters(m: &[Inst]) -> Vec<Inst> {
    let mut v: Vec<Inst> = m.iter().filter(|i| !is_plate(i)).copied().collect();
    v.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
    v
}

fn plates(m: &[Inst]) -> Vec<Inst> {
    let mut v: Vec<Inst> = m.iter().filter(|i| is_plate(i)).copied().collect();
    v.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
    v
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

// ---------------------------------------------------------------- M1

#[test]
fn a_past_track_with_a_duration_runs_its_keys_from_the_unit_s_end() {
    let e = effect(
        sans(),
        json!([{"unit": "word", "slot": "past", "property": "opacity", "duration": 0.2,
                "curve": "linear", "keys": [[0, 1], [1, 0.5]]}]),
        fill(),
    )
    .unwrap();
    // « ab » ends at 1.5: halfway through its return at 1.6, done from 1.7 on.
    let alpha_ab = |t: f64| letters(&at(&e, ab_cd(), t).1)[0].color[3];
    assert!(close(alpha_ab(1.4), 1.0, 1e-6));
    assert!(close(alpha_ab(1.6), 0.75, 1e-4), "{}", alpha_ab(1.6));
    assert!(close(alpha_ab(1.9), 0.5, 1e-4), "{}", alpha_ab(1.9));
    // Without its own duration a past track reads its first key, as before.
    let e = effect(
        sans(),
        json!([{"unit": "word", "slot": "past", "property": "opacity",
                "keys": [[0, 0.8], [1, 0.5]]}]),
        fill(),
    )
    .unwrap();
    let alpha_ab = |t: f64| letters(&at(&e, ab_cd(), t).1)[0].color[3];
    assert!(close(alpha_ab(1.6), 0.8, 1e-4));
    assert!(close(alpha_ab(1.9), 0.8, 1e-4));
}

#[test]
fn a_past_track_s_duration_must_be_above_zero() {
    let refused = effect(
        sans(),
        json!([{"unit": "word", "slot": "past", "property": "opacity", "duration": 0,
                "keys": [[0, 1], [1, 0.5]]}]),
        fill(),
    )
    .unwrap_err();
    assert!(refused.contains("duration must be above 0"), "{refused}");
}

#[test]
fn a_syllable_returns_from_its_own_end() {
    let e = effect(
        sans(),
        json!([{"unit": "syllable", "slot": "past", "property": "opacity", "duration": 0.2,
                "keys": [[0, 1], [1, 0]]}]),
        fill(),
    )
    .unwrap();
    let mut line = words(1.0, 2.0, "abcd");
    line.words[0].syllables = vec![
        super::line::Syllable {
            first_char: 0,
            start: 1.0,
            end: 1.4,
        },
        super::line::Syllable {
            first_char: 2,
            start: 1.4,
            end: 2.0,
        },
    ];
    let m = at(&e, vec![line], 1.5).1;
    let l = letters(&m);
    // « ab » ended at 1.4 (half returned at 1.5), « cd » is still sung.
    assert!(close(l[0].color[3], 0.5, 1e-4), "{}", l[0].color[3]);
    assert!(close(l[3].color[3], 1.0, 1e-6));
}

// ---------------------------------------------------------------- M2

fn word_plate(extra: Value) -> Value {
    let mut p = json!({"kind": "plate", "on": "word", "color": "#FF0000", "alpha": 0.5});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    json!([p, {"kind": "fill", "color": "#FFFFFF"}])
}

#[test]
fn a_word_plate_sits_under_the_letters_around_the_word() {
    let e = effect(sans(), json!([]), word_plate(json!({"radius": 0.25}))).unwrap();
    let m = at(&e, ab_cd(), 1.2).1;
    let p = plates(&m);
    assert_eq!(p.len(), 2, "one plate per word");
    // Under every letter of the line: all plates come first.
    let first_letter = m.iter().position(|i| !is_plate(i)).unwrap();
    assert!(m[..first_letter].iter().all(is_plate));
    assert!(m[first_letter..].iter().all(|i| !is_plate(i)));
    let l = letters(&m);
    // « ab »'s plate spans its two letters, at the pass's colour and opacity.
    assert!(p[0].center[0] > l[0].center[0] && p[0].center[0] < l[1].center[0]);
    assert!(p[0].center[0] - p[0].uv[0] * 0.5 < l[0].center[0]);
    assert!(p[0].center[0] + p[0].uv[0] * 0.5 > l[1].center[0]);
    assert!(close(p[0].color[3], 0.5, 1e-6));
    assert!(close(p[0].color[0], 0.5, 1e-6) && p[0].color[1] == 0.0);
    // Radius 0.25 em at 64 px; the quad is the plate grown by one pixel each side.
    assert!(close(p[0].uv[2], 16.0, 1e-4));
    assert!(close(p[0].size[0], p[0].uv[0] + 2.0, 1e-4));
    assert!(close(p[0].uv[3], 1.0, 1e-6));
    assert_eq!(p[0].aux[1], PASS_PLATE);
}

#[test]
fn plate_margins_grow_it_in_the_unit_s_em() {
    let bare = plates(
        &at(
            &effect(sans(), json!([]), word_plate(json!({}))).unwrap(),
            ab_cd(),
            1.2,
        )
        .1,
    );
    let grown = plates(
        &at(
            &effect(
                sans(),
                json!([]),
                word_plate(json!({"margin_x": 0.5, "margin_y": 0.25})),
            )
            .unwrap(),
            ab_cd(),
            1.2,
        )
        .1,
    );
    assert!(close(grown[0].uv[0] - bare[0].uv[0], 64.0, 1e-3));
    assert!(close(grown[0].uv[1] - bare[0].uv[1], 32.0, 1e-3));
    assert!(close(grown[0].center[0], bare[0].center[0], 1e-3));
}

#[test]
fn plate_properties_fade_grow_scale_and_wipe_it() {
    let base = effect(sans(), json!([]), word_plate(json!({}))).unwrap();
    let bare = plates(&at(&base, ab_cd(), 1.2).1)[0];
    let with = |track: Value| {
        let e = effect(sans(), json!([track]), word_plate(json!({}))).unwrap();
        plates(&at(&e, ab_cd(), 1.2).1)
    };
    let faded =
        with(json!({"unit": "word", "slot": "loop", "property": "plate_opacity", "value": 0.5}));
    assert!(close(faded[0].color[3], 0.25, 1e-6));
    let grown =
        with(json!({"unit": "word", "slot": "loop", "property": "plate_margin", "value": 0.5}));
    assert!(close(grown[0].uv[0] - bare.uv[0], 64.0, 1e-3));
    assert!(close(grown[0].uv[1] - bare.uv[1], 64.0, 1e-3));
    let scaled =
        with(json!({"unit": "word", "slot": "loop", "property": "plate_scale_x", "value": 0.5}));
    assert!(close(scaled[0].uv[0], bare.uv[0] * 0.5, 1e-3));
    assert!(close(scaled[0].center[0], bare.center[0], 1e-3));
    // Wiped halfway: the left half, from the plate's own left edge.
    let wiped =
        with(json!({"unit": "word", "slot": "loop", "property": "plate_wipe", "value": 0.5}));
    assert!(close(wiped[0].uv[0], bare.uv[0] * 0.5, 1e-3));
    assert!(close(
        wiped[0].center[0] - wiped[0].uv[0] * 0.5,
        bare.center[0] - bare.uv[0] * 0.5,
        1e-3
    ));
    // Nothing drawn at opacity 0 or wiped to nothing.
    assert!(
        with(json!({"unit": "word", "slot": "future", "property": "plate_opacity", "value": 0}))
            .len()
            == 1
    );
    assert!(
        with(json!({"unit": "word", "slot": "loop", "property": "plate_wipe", "value": 0}))
            .is_empty()
    );
}

#[test]
fn a_highlighter_marks_the_sung_word_then_lets_it_go() {
    // The Surligneur pattern: hidden before its word, wiped in over 90 ms, kept
    // while sung, faded over 140 ms once past.
    let e = effect(
        sans(),
        json!([
            {"unit": "word", "slot": "future", "property": "plate_opacity", "value": 0},
            {"unit": "word", "slot": "enter", "property": "plate_wipe", "duration": 0.09, "keys": [[0, 0], [1, 1]]},
            {"unit": "word", "slot": "past", "property": "plate_opacity", "duration": 0.14, "keys": [[0, 1], [1, 0]]}
        ]),
        word_plate(json!({})),
    )
    .unwrap();
    let p = |t: f64| plates(&at(&e, ab_cd(), t).1);
    // 1.2: « ab » marked in full, « cd » not yet.
    assert_eq!(p(1.2).len(), 1);
    // 1.545: « cd » half wiped in, « ab » fading (0.045 of its 0.14 s).
    let mid = p(1.545);
    assert_eq!(mid.len(), 2);
    assert!(
        close(mid[0].color[3], 0.5 * (1.0 - 0.045 / 0.14), 1e-3),
        "{}",
        mid[0].color[3]
    );
    assert!(close(mid[1].uv[0], p(1.8)[0].uv[0] * 0.5, 0.5));
    // 1.8: only « cd »; « ab » faded out.
    assert_eq!(p(1.8).len(), 1);
}

#[test]
fn a_line_plate_is_one_rounded_band_per_row() {
    let mut layout = sans();
    layout["line_breaks"] = json!("keep");
    let e = effect(
        layout,
        json!([]),
        json!([{"kind": "plate", "on": "line", "color": "#000000", "alpha": 0.6, "margin_x": 0.45, "radius": 0.3},
               {"kind": "fill", "color": "#FFFFFF"}]),
    )
    .unwrap();
    let mut line = words(1.0, 2.0, "ab cd");
    line.breaks = vec![1];
    let m = at(&e, vec![line], 1.5).1;
    let mut p = plates(&m);
    p.sort_by(|a, b| a.center[1].total_cmp(&b.center[1]));
    assert_eq!(p.len(), 2);
    assert!(p[1].center[1] > p[0].center[1] + 32.0, "rows stack");
    // Each band spans its row's letters plus 0.45 em each side.
    let l = letters(&m);
    let ab: Vec<&Inst> = l
        .iter()
        .filter(|i| close(i.center[1], l[0].center[1], 20.0))
        .collect();
    assert_eq!(ab.len(), 2);
    assert!(p[0].uv[0] > (ab[1].center[0] - ab[0].center[0]) + 2.0 * 0.45 * 64.0);
}

#[test]
fn a_line_plate_follows_the_line_fade_and_offsets() {
    let passes = json!([{"kind": "plate", "on": "line", "color": "#000000", "alpha": 1},
                        {"kind": "fill", "color": "#FFFFFF"}]);
    let bare = plates(
        &at(
            &effect(sans(), json!([]), passes.clone()).unwrap(),
            ab_cd(),
            1.5,
        )
        .1,
    )[0];
    let e = effect(
        sans(),
        json!([{"unit": "line", "slot": "loop", "property": "opacity", "value": 0.5},
               {"unit": "line", "slot": "loop", "property": "offset_y", "value": 1}]),
        passes,
    )
    .unwrap();
    let moved = plates(&at(&e, ab_cd(), 1.5).1)[0];
    assert!(close(moved.color[3], 0.5, 1e-6));
    assert!(close(moved.center[1] - bare.center[1], 64.0, 1e-3));
}

#[test]
fn a_letter_plate_sits_behind_each_letter() {
    let e = effect(
        sans(),
        json!([{"unit": "letter", "slot": "loop", "property": "plate_opacity",
                "value": 1}]),
        json!([{"kind": "plate", "on": "letter", "color": "#FFFFFF"}, {"kind": "fill", "color": "#000000"}]),
    )
    .unwrap();
    let m = at(&e, ab_cd(), 1.5).1;
    let (p, l) = (plates(&m), letters(&m));
    assert_eq!(p.len(), 4);
    for (p, l) in p.iter().zip(&l) {
        assert!(
            close(p.center[0], l.center[0], 12.0),
            "{} {}",
            p.center[0],
            l.center[0]
        );
    }
}

/// Plates (line, word, letter) and their letters at 1.5 s, with this layout
/// anchor and these tracks; each plate pass `on` its unit.
fn plated(on: &str, anchor: &str, animate: Value) -> (Vec<Inst>, Vec<Inst>) {
    let mut layout = sans();
    layout["anchor"] = json!(anchor);
    let e = effect(
        layout,
        animate,
        json!([{"kind": "plate", "on": on}, {"kind": "fill", "color": "#FFFFFF"}]),
    )
    .unwrap();
    let m = at(&e, ab_cd(), 1.5).1;
    (plates(&m), letters(&m))
}

fn spaced(v: f64) -> Value {
    json!([{"unit": "line", "slot": "loop", "property": "tracking", "value": v}])
}

fn edges(p: &Inst) -> (f32, f32) {
    (p.center[0] - p.uv[0] * 0.5, p.center[0] + p.uv[0] * 0.5)
}

#[test]
fn plates_follow_letter_spacing_on_every_unit() {
    // A line's row: its edges move with its first and last letters.
    let (p0, l0) = plated("line", "unit", spaced(0.0));
    let (p1, l1) = plated("line", "unit", spaced(0.5));
    let moved = |k: usize| l1[k].center[0] - l0[k].center[0];
    assert!(moved(3) - moved(0) > 60.0, "the letters spread");
    assert!(close(edges(&p1[0]).0 - edges(&p0[0]).0, moved(0), 1e-3));
    assert!(close(edges(&p1[0]).1 - edges(&p0[0]).1, moved(3), 1e-3));
    // A word: its edges move with its own first and last letters.
    let (p0, _) = plated("word", "unit", spaced(0.0));
    let (p1, _) = plated("word", "unit", spaced(0.5));
    for (w, (a, b)) in [(0usize, (0usize, 1usize)), (1, (2, 3))] {
        assert!(
            close(edges(&p1[w]).0 - edges(&p0[w]).0, moved(a), 1e-3),
            "word {w}"
        );
        assert!(
            close(edges(&p1[w]).1 - edges(&p0[w]).1, moved(b), 1e-3),
            "word {w}"
        );
    }
    // A letter: its plate moves with it.
    let (p0, _) = plated("letter", "unit", spaced(0.0));
    let (p1, _) = plated("letter", "unit", spaced(0.5));
    for k in 0..4 {
        assert!(
            close(p1[k].center[0] - p0[k].center[0], moved(k), 1e-3),
            "letter {k}"
        );
    }
}

#[test]
fn a_letter_plate_follows_its_letter_when_the_word_scales_about_its_centre() {
    let grown = json!([{"unit": "word", "slot": "loop", "property": "scale", "value": 2}]);
    for anchor in ["word", "unit"] {
        let (p0, l0) = plated("letter", anchor, json!([]));
        let (p1, l1) = plated("letter", anchor, grown.clone());
        // About the word's centre its letters spread; each letter scales in place.
        let gap = |l: &[Inst]| l[1].center[0] - l[0].center[0];
        let spread = if anchor == "word" { 2.0 } else { 1.0 };
        assert!(close(gap(&l1), spread * gap(&l0), 1e-3), "{anchor}");
        // The plate is scaled with its letter: rigidly attached, twice the size.
        for k in 0..4 {
            let (before, after) = (
                p0[k].center[0] - l0[k].center[0],
                p1[k].center[0] - l1[k].center[0],
            );
            assert!(
                close(after, 2.0 * before, 1e-3),
                "{anchor} letter {k}: {before} then {after}"
            );
            assert!(close(p1[k].uv[0], 2.0 * p0[k].uv[0], 1e-3));
            assert!(close(
                p1[k].center[1] - l1[k].center[1],
                2.0 * (p0[k].center[1] - l0[k].center[1]),
                1e-3
            ));
        }
        // A word plate spans its scaled letters, edge to edge.
        let (w1, _) = plated("word", anchor, grown.clone());
        for w in 0..2 {
            assert!(
                close(edges(&w1[w]).0, edges(&p1[2 * w]).0, 1e-3),
                "{anchor} word {w}"
            );
            assert!(
                close(edges(&w1[w]).1, edges(&p1[2 * w + 1]).1, 1e-3),
                "{anchor} word {w}"
            );
        }
    }
}

#[test]
fn plate_mistakes_are_refused_with_their_field() {
    let refused = |layout: Value, animate: Value, passes: Value, says: &str| {
        let e = effect(layout, animate, passes).unwrap_err();
        assert!(e.contains(says), "{says}: {e}");
    };
    refused(
        sans(),
        json!([]),
        json!([{"kind": "plate", "on": "syllable"}]),
        "unknown (line, word, letter)",
    );
    refused(
        sans(),
        json!([]),
        json!([{"kind": "plate", "blur": 2}]),
        "not a plate field",
    );
    refused(
        sans(),
        json!([]),
        json!([{"kind": "plate", "color2": "#000000"}]),
        "not a plate field",
    );
    refused(
        sans(),
        json!([]),
        json!([{"kind": "fill", "radius": 0.2}]),
        "plate passes only",
    );
    refused(
        sans(),
        json!([{"unit": "word", "slot": "loop", "property": "plate_opacity", "value": 1}]),
        json!([{"kind": "plate", "on": "line"}]),
        "no plate pass is drawn on this unit",
    );
    refused(
        sans(),
        json!([{"unit": "syllable", "slot": "loop", "property": "plate_wipe", "value": 1}]),
        json!([{"kind": "plate", "on": "word"}]),
        "plate properties are on the line, word or letter unit",
    );
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/compose");
    let mut j: Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    j["render"]["passes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "plate", "on": "word"}));
    let e = Effect::from_json(&dir, j.to_string().as_bytes()).unwrap_err();
    assert!(e.0.contains("plate passes are line mode only"), "{}", e.0);
}

// ---------------------------------------------------------------- M4

fn wipe(edge: Option<f64>) -> Effect {
    let mut animate = vec![
        json!({"unit": "word", "slot": "active", "property": "wipe", "keys": [[0, 0], [1, 1]]}),
    ];
    if let Some(e) = edge {
        animate.push(json!({"unit": "word", "slot": "loop", "property": "wipe_edge", "value": e}));
    }
    effect(
        sans(),
        Value::Array(animate),
        json!([{"kind": "fill", "color": "#FFFFFF", "color2": "#00FF00"}]),
    )
    .unwrap()
}

#[test]
fn a_hard_wipe_edge_is_unchanged_and_a_soft_one_spans_its_width() {
    let hard = letters(&at(&wipe(None), ab_cd(), 1.25).1);
    assert!(hard.iter().all(|i| i.aux[0] == 0.0));
    let soft = letters(&at(&wipe(Some(0.5)), ab_cd(), 1.25).1);
    // 0.5 em = 32 px of fade, in each letter's local width.
    for (s, h) in soft.iter().zip(&hard) {
        if s.extra[0] >= 0.0 || s.aux[0] > 0.0 {
            assert!(close(s.aux[0] * s.size[0], 32.0, 1e-3));
        }
        assert_eq!(s.center, h.center);
    }
    // Halfway through « ab », the soft split runs over the word's span widened by
    // the fade (it enters wholly left and leaves wholly right): in canvas px, both
    // splits sit at the same point halfway.
    let split_x = |i: &Inst| i.center[0] - i.size[0] * 0.5 + i.extra[0] * i.size[0];
    assert!(close(split_x(&soft[0]), split_x(&hard[0]), 1e-2));
    // Early in the word (2 % in) the soft split starts half its fade left of the
    // word, so the fade enters without a jump: 16 px left, less 2 % of 32 px.
    let t = 1.0 + 0.5 * 0.02;
    let soft = letters(&at(&wipe(Some(0.5)), ab_cd(), t).1);
    let hard = letters(&at(&wipe(None), ab_cd(), t).1);
    assert!(close(
        split_x(&soft[0]) - split_x(&hard[0]),
        -16.0 + 32.0 * 0.02,
        1e-2
    ));
}

#[test]
fn a_wipe_edge_on_the_line_unit_is_refused() {
    let e = effect(
        sans(),
        json!([{"unit": "line", "slot": "loop", "property": "wipe_edge", "value": 0.2}]),
        fill(),
    )
    .unwrap_err();
    assert!(
        e.contains("wipe_edge is on the word, syllable or letter unit"),
        "{e}"
    );
}

// ---------------------------------------------------------------- M5

fn glide_window() -> Effect {
    let mut layout = sans();
    layout["window"] = json!({"before": 1, "after": 1, "lead": 10, "gap": 0.3,
                              "glide": {"duration": 0.4, "curve": "linear"}});
    effect(layout, json!([]), fill()).unwrap()
}

fn three_lines() -> Vec<Line> {
    vec![
        words(1.0, 3.0, "first"),
        words(3.0, 5.0, "second"),
        words(5.0, 7.0, "third"),
    ]
}

/// The top of each line's letters at `t`, by the line's start (aux: seconds
/// since it showed), and that line's opacity.
fn tops(e: &Effect, t: f64) -> Vec<(f64, f32, f32)> {
    let m = at(e, three_lines(), t).1;
    let mut out: Vec<(f64, f32, f32)> = Vec::new();
    for i in &m {
        let start = (t - f64::from(i.aux[2])).round();
        let top = i.center[1];
        match out.iter_mut().find(|o| o.0 == start) {
            Some(o) => o.1 = o.1.min(top),
            None => out.push((start, top, i.color[3])),
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

#[test]
fn a_gliding_window_moves_its_lines_to_their_new_place() {
    let e = glide_window();
    // Settled before the change (line 1 active, line 3 below) and after it.
    let before = tops(&e, 2.99);
    let after = tops(&e, 3.5);
    assert_eq!(before.iter().map(|b| b.0).collect::<Vec<_>>(), [1.0, 3.0]);
    assert_eq!(
        after.iter().map(|a| a.0).collect::<Vec<_>>(),
        [1.0, 3.0, 5.0]
    );
    let y = |v: &[(f64, f32, f32)], line: f64| v.iter().find(|x| x.0 == line).unwrap().1;
    // At the change, every line is still where it was.
    let start = tops(&e, 3.0);
    assert!(close(y(&start, 3.0), y(&before, 3.0), 0.01));
    assert!(close(y(&start, 1.0), y(&before, 1.0), 0.01));
    // Halfway, halfway there.
    let mid = tops(&e, 3.2);
    assert!(close(
        y(&mid, 3.0),
        (y(&before, 3.0) + y(&after, 3.0)) * 0.5,
        0.05
    ));
    assert!(close(
        y(&mid, 1.0),
        (y(&before, 1.0) + y(&after, 1.0)) * 0.5,
        0.05
    ));
    // The new line below comes up with the column.
    assert!(y(&mid, 5.0) > y(&after, 5.0));
    // Without glide the lines jump.
    let mut layout = sans();
    layout["window"] = json!({"before": 1, "after": 1, "lead": 10, "gap": 0.3});
    let jump = effect(layout, json!([]), fill()).unwrap();
    assert!(close(y(&tops(&jump, 3.0), 3.0), y(&after, 3.0), 0.01));
}

#[test]
fn a_line_leaving_the_window_glides_out_and_fades() {
    let e = glide_window();
    // Line 1 leaves the window when line 3 takes the active slot (before = 1).
    let settled = tops(&e, 4.99);
    let mid = tops(&e, 5.2);
    let after = tops(&e, 5.5);
    assert!(after.iter().all(|a| a.0 != 1.0), "gone once settled");
    let leaving = mid
        .iter()
        .find(|m| m.0 == 1.0)
        .expect("still drawn while gliding");
    let was = settled.iter().find(|s| s.0 == 1.0).unwrap();
    assert!(
        close(leaving.2, was.2 * 0.5, 1e-3),
        "{} {}",
        leaving.2,
        was.2
    );
    assert!(leaving.1 < was.1, "moves up with the column");
}

#[test]
fn a_glide_needs_a_duration_from_above_0_to_2_s() {
    for bad in [
        json!({"duration": 0}),
        json!({"duration": 3}),
        json!({"curve": "linear"}),
    ] {
        let mut layout = sans();
        layout["window"] = json!({"after": 1, "glide": bad});
        let e = effect(layout, json!([]), fill()).unwrap_err();
        assert!(e.contains("glide.duration"), "{e}");
    }
}

// ---------------------------------------------------------------- M14

#[test]
fn line_scale_and_rotation_reach_every_letter_about_the_block_centre() {
    let plain = letters(&at(&effect(sans(), json!([]), fill()).unwrap(), ab_cd(), 1.5).1);
    let line = |prop: &str, v: f64| {
        let e = effect(
            sans(),
            json!([{"unit": "line", "slot": "loop", "property": prop, "value": v}]),
            fill(),
        )
        .unwrap();
        letters(&at(&e, ab_cd(), 1.5).1)
    };
    let spread = |l: &[Inst]| l[3].center[0] - l[0].center[0];
    let scaled = line("scale", 2.0);
    assert!(close(spread(&scaled), 2.0 * spread(&plain), 0.05));
    assert!(
        scaled
            .iter()
            .all(|i| i.xform[1] == 2.0 && i.xform[2] == 2.0)
    );
    let mid = |l: &[Inst]| (l[0].center[0] + l[3].center[0]) * 0.5;
    assert!(close(mid(&scaled), mid(&plain), 3.0));
    let wide = line("scale_x", 1.5);
    assert!(close(spread(&wide), 1.5 * spread(&plain), 0.05));
    assert!(wide.iter().all(|i| i.xform[1] == 1.5 && i.xform[2] == 1.0));
    // A quarter turn stands the line up: letters spread down instead of across.
    let turned = line("rotation", 90.0);
    let mut by_y = turned.clone();
    by_y.sort_by(|a, b| a.center[1].total_cmp(&b.center[1]));
    assert!(close(
        by_y[3].center[1] - by_y[0].center[1],
        spread(&plain),
        2.0
    ));
    assert!(
        turned
            .iter()
            .all(|i| close(i.xform[0], std::f32::consts::FRAC_PI_2, 1e-5))
    );
}

#[test]
fn line_scale_and_rotation_are_refused_in_compose_mode() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/compose");
    let base: Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    for prop in ["scale", "scale_x", "rotation"] {
        let mut j = base.clone();
        j["animate"] = json!([{"unit": "line", "slot": "loop", "property": prop, "value": 1}]);
        let e = Effect::from_json(&dir, j.to_string().as_bytes()).unwrap_err();
        assert!(e.0.contains("compose mode"), "{prop}: {}", e.0);
    }
}

#[test]
fn a_line_reads_its_rank_and_a_hash_gives_it_a_steady_value() {
    // Each line moves down by its rank, in em.
    let e = effect(
        sans(),
        json!([{"unit": "line", "slot": "loop", "property": "offset_y", "value": "@line_index"}]),
        fill(),
    )
    .unwrap();
    let plain = effect(sans(), json!([]), fill()).unwrap();
    let lines = || vec![words(1.0, 2.0, "ab"), words(2.0, 3.0, "ab")];
    let y = |e: &Effect, t: f64| letters(&at(e, lines(), t).1)[0].center[1];
    assert!(close(y(&e, 1.5), y(&plain, 1.5), 1e-4));
    assert!(close(y(&e, 2.5) - y(&plain, 2.5), 64.0, 1e-3));
    // A palette picked by line rank.
    let e = effect_with(
        json!([{"id": "pal", "type": "palette", "default": ["#FF0000", "#0000FF"]}]),
        sans(),
        json!([]),
        json!([{"kind": "fill", "color": {"pick": ["$pal", "line_index"]}}]),
    )
    .unwrap();
    let c = |t: f64| letters(&at(&e, lines(), t).1)[0].color;
    assert!(c(1.5)[0] > 0.9 && c(1.5)[2] == 0.0);
    assert!(c(2.5)[2] > 0.9 && c(2.5)[0] == 0.0);
    // hash: from -1 to 1, the same for the same whole number.
    let scene = PluginScene::new(plain, 1920, 1080, lines(), Some("p"));
    let h = |x: f64| match scene.eval_value(&json!({"hash": [x]})).unwrap() {
        R::Num(n) => n,
        _ => panic!("hash gives a number"),
    };
    assert_eq!(h(3.0), h(3.2));
    assert_ne!(h(3.0), h(4.0));
    assert!(
        (0..200)
            .map(|i| h(f64::from(i)))
            .all(|v| (-1.0..=1.0).contains(&v))
    );
    match scene.eval_value(&json!("@line_index")).unwrap() {
        R::Num(n) => assert_eq!(n, 0.0),
        _ => panic!(),
    }
}

// ---------------------------------------------------------------- M15

fn aligned(align: &str) -> Vec<Inst> {
    let mut layout = sans();
    layout["align"] = json!(align);
    letters(&at(&effect(layout, json!([]), fill()).unwrap(), ab_cd(), 1.5).1)
}

#[test]
fn rows_align_left_or_right_in_the_box() {
    // The box: centred, 0.86 of 1920 px wide, from 134.4 to 1785.6 px.
    // A bare line plate spans the letters' pen positions: its edges show where
    // the row starts and ends.
    let band = |align: &str| {
        let mut layout = sans();
        layout["align"] = json!(align);
        let e = effect(
            layout,
            json!([]),
            json!([{"kind": "plate", "on": "line"}, {"kind": "fill", "color": "#FFFFFF"}]),
        )
        .unwrap();
        let p = plates(&at(&e, ab_cd(), 1.5).1)[0];
        (p.center[0] - p.uv[0] * 0.5, p.center[0] + p.uv[0] * 0.5)
    };
    assert!(close(band("left").0, 134.4, 0.5), "{:?}", band("left"));
    assert!(close(band("right").1, 1785.6, 0.5), "{:?}", band("right"));
    let (left, right, centre) = (aligned("left"), aligned("right"), aligned("center"));
    let spread = |l: &[Inst]| l[3].center[0] - l[0].center[0];
    assert!(close(spread(&left), spread(&centre), 1e-3));
    assert!(close(spread(&right), spread(&centre), 1e-3));
    assert_eq!(
        aligned("center")
            .iter()
            .map(|i| i.center)
            .collect::<Vec<_>>(),
        letters(&at(&effect(sans(), json!([]), fill()).unwrap(), ab_cd(), 1.5).1)
            .iter()
            .map(|i| i.center)
            .collect::<Vec<_>>()
    );
}

#[test]
fn letter_spacing_grows_from_the_edge_a_row_is_set_against() {
    let spaced = |align: &str| {
        let mut layout = sans();
        layout["align"] = json!(align);
        let e = effect(
            layout,
            json!([{"unit": "line", "slot": "loop", "property": "tracking", "value": 0.5}]),
            fill(),
        )
        .unwrap();
        letters(&at(&e, ab_cd(), 1.5).1)
    };
    assert!(close(
        spaced("left")[0].center[0],
        aligned("left")[0].center[0],
        1e-3
    ));
    assert!(close(
        spaced("right")[3].center[0],
        aligned("right")[3].center[0],
        1e-3
    ));
}

#[test]
fn alignment_is_line_mode_only_and_named() {
    let mut layout = sans();
    layout["align"] = json!("justify");
    let e = effect(layout, json!([]), fill()).unwrap_err();
    assert!(e.contains("unknown (center, left, right)"), "{e}");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/compose");
    let mut j: Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    j["layout"]["align"] = json!("left");
    let e = Effect::from_json(&dir, j.to_string().as_bytes()).unwrap_err();
    assert!(e.0.contains("layout.align: line mode only"), "{}", e.0);
}

// ---------------------------------------------------------------- S1-S3

#[test]
fn every_instance_tells_its_pass_and_its_line_s_time() {
    let e = effect(
        sans(),
        json!([]),
        json!([
            {"kind": "plate", "on": "word"},
            {"kind": "shadow", "color": "#000000"},
            {"kind": "glow", "color": "#FFFFFF"},
            {"kind": "outline_only", "color": "#000000", "outline": 0.05},
            {"kind": "fill", "color": "#FFFFFF"}
        ]),
    )
    .unwrap();
    let (g, m) = at(&e, ab_cd(), 1.25);
    let kinds = |v: &[Inst]| {
        let mut k: Vec<f32> = v.iter().map(|i| i.aux[1]).collect();
        k.sort_by(f32::total_cmp);
        k.dedup();
        k
    };
    assert_eq!(kinds(&g), [PASS_GLOW]);
    assert_eq!(
        kinds(&m),
        [PASS_FILL, PASS_OUTLINE, PASS_SHADOW, PASS_PLATE]
    );
    // The line shows from 1 s to 2 s: 0.25 s in, 0.75 s to go.
    for i in g.iter().chain(&m) {
        assert!(
            close(i.aux[2], 0.25, 1e-5) && close(i.aux[3], 0.75, 1e-5),
            "{:?}",
            i.aux
        );
    }
    // Letters keep their number in the line for hooks.
    let l: Vec<&Inst> = m.iter().filter(|i| i.aux[1] == PASS_FILL).collect();
    let mut idx: Vec<f32> = l.iter().map(|i| i.extra[3]).collect();
    idx.sort_by(f32::total_cmp);
    assert_eq!(idx, [0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn a_hook_reads_the_letter_number_the_line_and_the_pass() {
    super::plugin_shader::validate(
        "fn shade(i: GlyphIn) -> vec4<f32> { \
           let edge = select(0.0, 1.0, i.pass_kind == 3.0); \
           return vec4<f32>(i.line, i.index * 0.01, i.line_time.x + i.line_time.y + edge, 1.0) * i.coverage; }",
    )
    .unwrap();
}

// ---------------------------------------------------------------- GPU

/// Rendered checks, run inside `tests::gpu_checks` (one GL thread).
pub(super) fn gpu_engine_features() {
    a_plate_draws_a_rounded_band_under_its_text();
    a_soft_wipe_edge_blends_over_its_width();
    a_hook_sees_the_line_progress_and_the_pass();
}

fn render(
    e: &Effect,
    lines: Vec<Line>,
    t: f64,
    w: u32,
    h: u32,
    hooks: Option<&str>,
) -> (Vec<Inst>, Vec<u8>) {
    use super::gl::GlRenderer;
    use super::shaders::Output;
    let mut fonts = Fonts::load().unwrap();
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::StraightSrgb).unwrap();
    gpu.set_hooks(hooks, [0.0; 16]).unwrap();
    let mut scene = PluginScene::new(e.clone(), w, h, lines, Some("p"));
    let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, t);
    let (px, _) = gpu.frame(&g, gain, &m, true).unwrap();
    (m, px)
}

fn pixel(px: &[u8], w: u32, x: f32, y: f32) -> [u8; 4] {
    let i = (y.round() as usize * w as usize + x.round() as usize) * 4;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn a_plate_draws_a_rounded_band_under_its_text() {
    let (w, h) = (480u32, 240u32);
    let e = effect(
        json!({"font": "sans", "size": 64, "box": {"y": 0.5, "anchor": "center"}}),
        json!([]),
        json!([{"kind": "plate", "on": "line", "color": "#FF0000", "margin_x": 0.5, "margin_y": 0.1, "radius": 0.4},
               {"kind": "fill", "color": "#FFFFFF"}]),
    )
    .unwrap();
    let (m, px) = render(&e, vec![words(1.0, 2.0, "ab")], 1.5, w, h, None);
    let p = plates(&m)[0];
    let (x0, y0) = (p.center[0] - p.uv[0] * 0.5, p.center[1] - p.uv[1] * 0.5);
    // Inside the margin, at mid height: the plate's red, opaque.
    let inside = pixel(&px, w, x0 + 6.0, p.center[1]);
    assert!(
        inside[0] > 250 && inside[1] < 5 && inside[3] > 250,
        "{inside:?}"
    );
    // Its corner is cut by the radius (25.6 px): nothing drawn there.
    let corner = pixel(&px, w, x0 + 1.5, y0 + 1.5);
    assert!(corner[3] < 5, "{corner:?}");
    // Just outside: nothing.
    assert!(pixel(&px, w, x0 - 3.0, p.center[1])[3] < 5);
    // A letter is drawn over the plate: white where the letter is.
    let l = letters(&m);
    let over = (0..9)
        .map(|k| pixel(&px, w, l[0].center[0] - 4.0 + k as f32, l[0].center[1]))
        .any(|c| c[0] > 240 && c[1] > 240 && c[2] > 240);
    assert!(over, "the letter shows over its plate");
}

fn a_soft_wipe_edge_blends_over_its_width() {
    let (w, h) = (480u32, 160u32);
    let lines = || vec![words(1.0, 2.0, "MMMM")];
    let blended = |edge: Option<f64>| {
        let (_, px) = render(&wipe(edge), lines(), 1.5, w, h, None);
        // Inked pixels where both colours mix: red well between white and green.
        px.chunks(4)
            .filter(|c| c[3] > 250 && c[1] > 200 && c[0] > 60 && c[0] < 195)
            .count()
    };
    let (hard, soft) = (blended(None), blended(Some(1.0)));
    assert!(soft > 4 * hard + 50, "soft {soft} hard {hard}");
}

fn a_hook_sees_the_line_progress_and_the_pass() {
    let (w, h) = (480u32, 240u32);
    let e = effect(
        json!({"font": "sans", "size": 64, "box": {"y": 0.5, "anchor": "center"}}),
        json!([]),
        json!([{"kind": "plate", "on": "line", "color": "#FFFFFF", "margin_x": 0.5},
               {"kind": "fill", "color": "#FFFFFF"}]),
    )
    .unwrap();
    // Red: the line's progress; green: a plate.
    let hooks = "fn shade(i: GlyphIn) -> vec4<f32> { \
                   return vec4<f32>(i.line, select(0.0, 1.0, i.pass_kind == 4.0), 0.0, 1.0) * i.coverage; }";
    let (m, px) = render(&e, vec![words(1.0, 2.0, "ab")], 1.25, w, h, Some(hooks));
    let p = plates(&m)[0];
    let on_plate = pixel(&px, w, p.center[0] - p.uv[0] * 0.5 + 6.0, p.center[1]);
    // 0.25 linear is 137 once sRGB-encoded.
    assert!(on_plate[0].abs_diff(137) <= 3, "{on_plate:?}");
    assert!(on_plate[1] > 250, "{on_plate:?}");
    // On a letter's core: the line's progress, not a plate.
    let l = letters(&m);
    let core = l
        .iter()
        .flat_map(|g| {
            (0..21 * 21).map(move |k| {
                (
                    g.center[0] - 10.0 + (k % 21) as f32,
                    g.center[1] - 10.0 + (k / 21) as f32,
                )
            })
        })
        .map(|(x, y)| pixel(&px, w, x, y))
        .find(|c| c[1] < 5 && c[3] > 250);
    let core = core.expect("a letter pixel");
    assert!(core[0].abs_diff(137) <= 3, "{core:?}");
}
