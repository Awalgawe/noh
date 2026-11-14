//! Step B1 (Task 39): host values, arithmetic and choice-based values.
use super::line::split_words;
use super::plugin::{Effect, PluginScene, R};
use serde_json::json;
use std::path::Path;

fn karaoke() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe/effect.json");
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

/// Karaoke plus a choice parameter `place` (bottom/top) and a preset that sets it.
fn scene(insets: Option<[u32; 4]>, preset: Option<&str>) -> PluginScene {
    let mut j = karaoke();
    j["params"].as_array_mut().unwrap().push(json!({"id": "place", "type": "choice", "default": "bottom", "choices": ["bottom", "top"], "level": 1, "label": {"fr": "Position"}}));
    j["presets"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "haut", "name": {"fr": "Haut"}, "values": {"place": "top"}}));
    let effect = load(&j).unwrap();
    let w = vec!["a".to_string()];
    PluginScene::new(
        effect,
        1080,
        1920,
        vec![split_words(0.0, 1.0, &w, " ")],
        preset,
    )
    .with_safe_insets(insets)
}

fn n(s: &PluginScene, v: serde_json::Value) -> f64 {
    match s.eval_value(&v).unwrap() {
        R::Num(n) => n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn host_values_give_the_canvas_and_the_safe_insets() {
    let plain = scene(None, None);
    assert_eq!(
        [
            n(&plain, json!("@w")),
            n(&plain, json!("@h")),
            n(&plain, json!("@short"))
        ],
        [1080.0, 1920.0, 1080.0]
    );
    assert_eq!(
        [
            n(&plain, json!("@inset_l")),
            n(&plain, json!("@inset_b")),
            n(&plain, json!("@safe"))
        ],
        [0.0, 0.0, 0.0]
    );
    let yt = scene(Some([48, 288, 192, 672]), None);
    let ins = ["@inset_l", "@inset_t", "@inset_r", "@inset_b"].map(|k| n(&yt, json!(k)));
    assert_eq!(ins, [48.0, 288.0, 192.0, 672.0]);
    assert_eq!(n(&yt, json!("@safe")), 1.0);
    assert!(plain.eval_value(&json!("@screen")).is_err());
}

#[test]
fn arithmetic_is_finite_and_follows_libass_style_rounding() {
    let s = scene(None, None);
    // libass font size for "medium": round(short × 0.055), at least 8.
    assert_eq!(
        n(
            &s,
            json!({"max": [{"round": [{"mul": ["@short", 0.055]}]}, 8]})
        ),
        59.0
    );
    // outline = ceil(size / 16), at least 1.
    assert_eq!(
        n(&s, json!({"max": [{"ceil": [{"div": [59, 16]}]}, 1]})),
        4.0
    );
    assert_eq!(n(&s, json!({"sub": [10, 3, 2]})), 5.0);
    assert_eq!(n(&s, json!({"add": [1, 2, 3]})), 6.0);
    assert_eq!(n(&s, json!({"min": [4, -2, 9]})), -2.0);
    assert_eq!(n(&s, json!({"floor": [2.7]})), 2.0);
    // A zero divisor gives 0, never infinity.
    assert_eq!(n(&s, json!({"div": [5, 0]})), 0.0);
    // Arity is checked.
    assert!(s.eval_value(&json!({"round": [1, 2]})).is_err());
    assert!(s.eval_value(&json!({"add": [1]})).is_err());
}

#[test]
fn choose_follows_a_choice_parameter_and_its_default() {
    let v = json!({"choose": ["$place", {"bottom": 0.9, "top": 0.1}]});
    assert_eq!(n(&scene(None, None), v.clone()), 0.9);
    assert_eq!(n(&scene(None, Some("haut")), v), 0.1);
    // An unknown text falls back to "default", else to the first case.
    let s = scene(None, None);
    assert_eq!(
        n(&s, json!({"choose": ["middle", {"top": 1, "default": 2}]})),
        2.0
    );
    // Without "default": the first case in key order ("bottom" before "top").
    assert_eq!(
        n(&s, json!({"choose": ["middle", {"top": 1, "bottom": 3}]})),
        3.0
    );
    assert!(s.eval_value(&json!({"choose": ["$place", {}]})).is_err());
}

#[test]
fn an_effect_can_size_its_box_from_the_host() {
    // The loader accepts host values and operators anywhere a value goes.
    let mut j = karaoke();
    j["layout"]["size"] = json!({"max": [{"round": [{"mul": ["@short", 0.055]}]}, 8]});
    j["layout"]["box"]["y"] = json!({"sub": [1, {"div": ["@inset_b", "@h"]}]});
    assert!(load(&j).is_ok());
    j["layout"]["size"] = json!({"mul": ["@nope", 2]});
    assert!(load(&j).is_err());
}
