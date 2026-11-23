//! Step B2 (Task 39): the Classique effect reproduces libass's caption geometry.
use super::line::split_words;
use super::plugin::{Effect, PluginScene, R};
use crate::captions::{Canvas, CaptionPlacement, CaptionSize, CaptionStyle};
use crate::safe_area::SafeArea;
use std::path::Path;

fn classique_json() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects/classique/effect.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Classique with its size and placement parameters set to one case.
fn scene(
    w: u32,
    h: u32,
    size: &str,
    placement: &str,
    area: SafeArea,
) -> (PluginScene, serde_json::Value) {
    let mut j = classique_json();
    for p in j["params"].as_array_mut().unwrap() {
        match p["id"].as_str() {
            Some("size") => p["default"] = size.into(),
            Some("placement") => p["default"] = placement.into(),
            _ => {}
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("effect.json"),
        serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    let effect = Effect::load(dir.path()).unwrap();
    let words = vec!["a".to_string()];
    let scene = PluginScene::new(effect, w, h, vec![split_words(0.0, 1.0, &words, " ")], None)
        .with_safe_insets(area.insets(w, h));
    (scene, j)
}

fn num(scene: &PluginScene, v: &serde_json::Value) -> f64 {
    match scene.eval_value(v).unwrap() {
        R::Num(n) => n,
        other => panic!("{other:?}"),
    }
}

fn text(scene: &PluginScene, v: &serde_json::Value) -> String {
    match scene.eval_value(v).unwrap() {
        R::Text(t) => t,
        other => panic!("{other:?}"),
    }
}

#[test]
fn classique_matches_libass_geometry_in_every_case() {
    let sizes = [
        ("small", CaptionSize::Small),
        ("medium", CaptionSize::Medium),
        ("large", CaptionSize::Large),
    ];
    let places = [
        ("bottom", CaptionPlacement::Bottom),
        ("top", CaptionPlacement::Top),
    ];
    let canvases = [
        (1920, 1080),
        (1080, 1920),
        (1916, 1080),
        (720, 576),
        (1280, 720),
        (854, 480),
        (360, 640),
    ];
    let mut cases = 0;
    for (w, h) in canvases {
        for area in SafeArea::ALL {
            for (size_id, size) in &sizes {
                for (place_id, placement) in &places {
                    let style = CaptionStyle {
                        size: size.clone(),
                        placement: placement.clone(),
                        safe_area: area,
                        line_width: None,
                    };
                    let Ok(canvas) = Canvas::new(f64::from(w), f64::from(h), style) else {
                        continue; // libass refuses this canvas too (too small for the margins)
                    };
                    let (font, left, right, vertical, outline, max_width, alignment) =
                        canvas.geometry();
                    let (scene, j) = scene(w, h, size_id, place_id, area);
                    let lay = &j["layout"];
                    let tag = format!("{w}x{h} {area:?} {size_id} {place_id}");
                    // libass's Fontsize is the REAL_DIM height; the engine's size is the em.
                    let em = num(&scene, &lay["size"]);
                    assert!(
                        (em / real_dim_ratio() - f64::from(font)).abs() < 1e-6,
                        "{tag}: font size {em} em vs {font}"
                    );
                    let em_outline = num(&scene, &j["render"]["passes"][0]["outline"]);
                    assert!(
                        (em_outline * em - f64::from(outline)).abs() < 1e-6,
                        "{tag}: outline"
                    );
                    // NOH's libass pre-wrap measures letters at the em F, which libass draws
                    // F x ratio: libass lines wrap at max_width x ratio of drawn width.
                    let width = num(&scene, &lay["box"]["width"]) * f64::from(w);
                    let wrap = f64::from(max_width) * real_dim_ratio();
                    assert!(
                        (width - wrap).abs() < 0.01,
                        "{tag}: width {width} vs libass wrap {wrap}"
                    );
                    let centre = num(&scene, &lay["box"]["x"]) * f64::from(w);
                    let expected = (f64::from(left) + f64::from(w) - f64::from(right)) / 2.0;
                    assert!(
                        (centre - expected).abs() < 0.01,
                        "{tag}: centre {centre} vs {expected}"
                    );
                    let y = num(&scene, &lay["box"]["y"]) * f64::from(h);
                    let anchor = text(&scene, &lay["box"]["anchor"]);
                    if *placement == CaptionPlacement::Top {
                        assert_eq!((anchor.as_str(), alignment), ("top", 8), "{tag}");
                        assert!(
                            (y - f64::from(vertical)).abs() < 0.01,
                            "{tag}: top {y} vs {vertical}"
                        );
                    } else {
                        assert_eq!((anchor.as_str(), alignment), ("bottom", 2), "{tag}");
                        assert!(
                            (y - f64::from(h - vertical)).abs() < 0.01,
                            "{tag}: bottom {y} vs {}",
                            h - vertical
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    assert!(cases >= 180, "{cases} cases");
}

#[test]
fn classique_draws_white_letters_with_a_black_outline_in_the_cjk_face() {
    let j = classique_json();
    assert_eq!(j["layout"]["font"], "cjk");
    let passes = j["render"]["passes"].as_array().unwrap();
    assert_eq!(passes[0]["kind"], "outline_only");
    assert_eq!(passes[0]["color"], "#000000");
    assert_eq!(passes[1]["kind"], "fill");
    assert_eq!(passes[1]["color"], "#FFFFFF");
    assert!(
        j["layout"].get("window").is_none(),
        "Classique shows the active line only"
    );
}

/// Plan check 5: Classique never reports an overflow in the Short's safe areas,
/// for every size and placement, with one-line, three-line and CJK cues.
#[test]
fn classique_stays_inside_every_safe_area() {
    use super::shaders::Upload;
    use super::text::Fonts;
    struct NoGpu;
    impl Upload for NoGpu {
        fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    }
    let mut fonts = Fonts::load().unwrap();
    let cues = [
        "nothing feels the same tonight",
        "ever since the impact nothing feels the same tonight and the city lights keep falling down on me again",
        "夜空に光る星を数えながら君を待っている",
    ];
    for area in [
        SafeArea::YoutubeShorts,
        SafeArea::Tiktok,
        SafeArea::Reels,
        SafeArea::Universal,
    ] {
        for size in ["small", "medium", "large"] {
            for place in ["bottom", "top"] {
                for cue in cues {
                    let (scene, _) = scene(1080, 1920, size, place, area);
                    let words: Vec<String> = cue.split_whitespace().map(str::to_string).collect();
                    let mut scene = PluginScene::new(
                        scene.effect.clone(),
                        1080,
                        1920,
                        vec![split_words(0.0, 2.0, &words, " ")],
                        None,
                    )
                    .with_safe_insets(area.insets(1080, 1920));
                    let reported = (0..40).any(|f| {
                        scene.frame(&mut fonts, &NoGpu, f as f64 / 20.0);
                        scene.overflowed()
                    });
                    assert!(!reported, "{area:?} {size} {place}: {cue}");
                }
            }
        }
    }
}

/// upem / (hhea ascender - descender) of the caption font: libass sizes text by
/// FreeType's REAL_DIM height, so its em is Fontsize times this ratio.
fn real_dim_ratio() -> f64 {
    let face = ttf_parser::Face::parse(crate::captions::FONT, 0).unwrap();
    f64::from(face.units_per_em()) / (f64::from(face.ascender()) - f64::from(face.descender()))
}

/// Parity of what is drawn, not only of the formulas: at libass Fontsize F, a
/// letter advances F x ratio x advance / upem, and lines are F apart.
#[test]
fn classique_draws_letters_and_lines_at_libass_size() {
    use super::shaders::Upload;
    use super::text::Fonts;
    struct NoGpu;
    impl Upload for NoGpu {
        fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    }
    let face = ttf_parser::Face::parse(crate::captions::FONT, 0).unwrap();
    let h_adv = f64::from(
        face.glyph_hor_advance(face.glyph_index('H').unwrap())
            .unwrap(),
    );
    let upem = f64::from(face.units_per_em());
    let mut fonts = Fonts::load().unwrap();
    for (w, h) in [(1920u32, 1080u32), (1080, 1920)] {
        let canvas = Canvas::new(f64::from(w), f64::from(h), CaptionStyle::default()).unwrap();
        let font = f64::from(canvas.geometry().0);
        let (base, _) = scene(w, h, "medium", "bottom", SafeArea::None);
        // One line of H: centre to centre is one advance.
        let words = vec!["HHHHHH".to_string()];
        let mut s = PluginScene::new(
            base.effect.clone(),
            w,
            h,
            vec![split_words(0.0, 2.0, &words, " ")],
            None,
        );
        let (_, _, main, _) = s.frame(&mut fonts, &NoGpu, 1.0);
        let mut xs: Vec<f64> = main
            .iter()
            .filter(|i| i.color[0] > 0.5)
            .map(|i| f64::from(i.center[0]))
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(xs.len(), 6, "{w}x{h}: {xs:?}");
        let advance = (xs[5] - xs[0]) / 5.0;
        let expected = font * real_dim_ratio() * h_adv / upem;
        assert!(
            (advance / expected - 1.0).abs() < 0.01,
            "{w}x{h}: advance {advance} vs libass {expected}"
        );
        // Two lines (a forced wrap): their baselines are F apart, as libass's.
        let long: Vec<String> = std::iter::repeat_n("HHHHHHHH".to_string(), 12).collect();
        let mut s = PluginScene::new(
            base.effect.clone(),
            w,
            h,
            vec![split_words(0.0, 2.0, &long, " ")],
            None,
        );
        let (_, _, main, _) = s.frame(&mut fonts, &NoGpu, 1.0);
        let mut ys: Vec<i64> = main
            .iter()
            .filter(|i| i.color[0] > 0.5)
            .map(|i| (f64::from(i.center[1]) * 10.0).round() as i64)
            .collect();
        ys.sort();
        ys.dedup();
        assert!(ys.len() >= 2, "{w}x{h}: no wrap");
        let step = (ys[1] - ys[0]) as f64 / 10.0;
        assert!(
            (step / font - 1.0).abs() < 0.01,
            "{w}x{h}: line step {step} vs libass {font}"
        );
    }
}

/// The app's Size and Position reach the engine as values over the preset.
#[test]
fn values_from_the_app_override_the_preset() {
    let (base, j) = scene(1080, 1920, "medium", "bottom", SafeArea::None);
    let words = vec!["a".to_string()];
    let mut values = std::collections::BTreeMap::new();
    values.insert("placement".to_string(), serde_json::json!("top"));
    values.insert("size".to_string(), serde_json::json!("large"));
    let s = PluginScene::new(
        base.effect.clone(),
        1080,
        1920,
        vec![split_words(0.0, 1.0, &words, " ")],
        Some("classique"),
    )
    .with_values(&values)
    .unwrap();
    assert_eq!(text(&s, &j["layout"]["box"]["anchor"]), "top");
    let large = Canvas::new(
        1080.0,
        1920.0,
        CaptionStyle {
            size: CaptionSize::Large,
            ..CaptionStyle::default()
        },
    )
    .unwrap();
    assert!(
        (num(&s, &j["layout"]["size"]) / real_dim_ratio() - f64::from(large.geometry().0)).abs()
            < 1e-6
    );
    // No values: the preset's defaults, as before.
    let s = PluginScene::new(
        base.effect.clone(),
        1080,
        1920,
        vec![split_words(0.0, 1.0, &words, " ")],
        Some("classique"),
    )
    .with_values(&Default::default())
    .unwrap();
    assert_eq!(text(&s, &j["layout"]["box"]["anchor"]), "bottom");
}

/// A request's values are checked like the rest of the format: never ignored.
#[test]
fn invalid_values_are_refused() {
    let (classic, _) = scene(1080, 1920, "medium", "bottom", SafeArea::None);
    let karaoke = {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe");
        Effect::load(&dir).unwrap()
    };
    let words = vec!["a".to_string()];
    let try_values = |effect: &Effect, k: &str, v: serde_json::Value| {
        let mut values = std::collections::BTreeMap::new();
        values.insert(k.to_string(), v);
        PluginScene::new(
            effect.clone(),
            1080,
            1920,
            vec![split_words(0.0, 1.0, &words, " ")],
            None,
        )
        .with_values(&values)
        .map(|_| ())
    };
    assert!(try_values(&classic.effect, "size", "large".into()).is_ok());
    assert!(
        try_values(&classic.effect, "size", "huge".into())
            .unwrap_err()
            .contains("size")
    );
    assert!(try_values(&classic.effect, "size", serde_json::json!(3)).is_err());
    assert!(
        try_values(&classic.effect, "colour", "#FFFFFF".into())
            .unwrap_err()
            .contains("unknown")
    );
    assert!(try_values(&karaoke, "size", serde_json::json!(100)).is_ok());
    assert!(try_values(&karaoke, "size", serde_json::json!(500)).is_err());
    assert!(try_values(&karaoke, "sung", "#FF4F8B".into()).is_ok());
    assert!(try_values(&karaoke, "sung", "red".into()).is_err());
}

/// Task 43: the built-in Classique (command line and MCP) is the bundled file,
/// with « Largeur des lignes » defaulting to NOH's historical width.
#[test]
fn the_built_in_classique_is_the_bundled_one() {
    let built_in = super::catalog::builtin_classic();
    let bundled = super::catalog::load_one(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects/classique"),
        "noh.classic",
    )
    .unwrap();
    assert_eq!(built_in.id, bundled.id);
    let shape = |e: &Effect| {
        e.params
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    p.default.clone(),
                    p.min,
                    p.max,
                    p.level,
                    p.label
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<std::collections::BTreeMap<_, _>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(&built_in), shape(&bundled));
    let width = built_in.params.iter().find(|p| p.id == "width").unwrap();
    assert_eq!(
        (width.min, width.max, width.level),
        (Some(40.0), Some(100.0), 2)
    );
    assert!((width.default.as_f64().unwrap() - 100.0 * real_dim_ratio()).abs() < 1e-9);
}

/// Task 43: at 40, 69 and 100 % of the usable width, on the project, Short and
/// burn canvases, Classique's box and the libass fallback's pre-wrap give the
/// same drawn width and break the same cues after the same letters.
#[test]
fn line_width_setting_breaks_like_the_libass_fallback() {
    use super::shaders::Upload;
    use super::text::Fonts;
    struct NoGpu;
    impl Upload for NoGpu {
        fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    }
    let mut fonts = Fonts::load().unwrap();
    let cues = [
        "ever since the impact nothing feels the same tonight and the city lights keep falling down on me",
        "quand la nuit tombe sur la ville je repense à toi et aux promesses que l'on s'était faites",
        "夜空に光る星を数えながら君を待っている",
        // Where UAX #14 differs from word bounds: hyphens, dash, slash, digits,
        // punctuation, CJK brackets and marks, mixed scripts.
        "a well-known state-of-the-art song — 2024/25 edition?! and the long-awaited after-party",
        "「夜空」に光る星を、数えながら。君を待っている tonight（今夜）",
        // Typed lines (Task 43 step C): Classique keeps them, as libass does; a long
        // typed line still wraps inside its break; CJK lines join without a space.
        "nothing feels the same\ntonight",
        "ever since the impact nothing feels the same tonight and the city lights\nkeep falling\ndown on me",
        "夜空に光る\n星を数えながら",
        // The maintainer's choice B: a word wider than the line stays whole on both routes.
        "on a gardé SupercalifragilisticexpialidociousSupercalifragilisticexpialidocious la lumière",
    ];
    let canvases = [
        (1920.0, 1080.0),
        (1080.0, 1920.0),
        (1440.0, 1080.0),
        (3840.0, 2160.0),
        (1278.0, 719.0),
    ];
    let (mut cases, mut breaks, mut near) = (0, 0, 0);
    for (w, h) in canvases {
        for area in [SafeArea::None, SafeArea::YoutubeShorts] {
            for (size_id, size) in [
                ("small", CaptionSize::Small),
                ("medium", CaptionSize::Medium),
                ("large", CaptionSize::Large),
            ] {
                for percent in [40u32, 69, 100] {
                    let style = CaptionStyle {
                        size: size.clone(),
                        placement: CaptionPlacement::Bottom,
                        safe_area: area,
                        line_width: Some(percent * 100),
                    };
                    let Ok(canvas) = Canvas::new(w, h, style) else {
                        continue;
                    };
                    let (cw, ch, wrap, max_height) = canvas.wrap();
                    let font = canvas.geometry().0;
                    let (base, j) = scene(cw, ch, size_id, "bottom", area);
                    let mut values = std::collections::BTreeMap::new();
                    values.insert("width".to_string(), serde_json::json!(percent));
                    let tag = format!("{cw}x{ch} {area:?} {size_id} {percent} %");
                    let one = vec!["a".to_string()];
                    let probe = PluginScene::new(
                        base.effect.clone(),
                        cw,
                        ch,
                        vec![split_words(0.0, 1.0, &one, " ")],
                        None,
                    )
                    .with_safe_insets(area.insets(cw, ch))
                    .with_values(&values)
                    .unwrap();
                    let drawn = num(&probe, &j["layout"]["box"]["width"]) * f64::from(cw);
                    let libass = f64::from(wrap) * real_dim_ratio();
                    assert!(
                        (drawn - libass).abs() < 0.05,
                        "{tag}: Classique {drawn} px vs libass {libass} px"
                    );
                    cases += 1;
                    for cue in cues {
                        let Ok(lines) =
                            crate::caption_layout::layout_text(cue, font, wrap, max_height)
                        else {
                            continue;
                        };
                        let expected: Vec<usize> = lines
                            .iter()
                            .map(|l| l.chars().filter(|c| !c.is_whitespace()).count())
                            .collect();
                        let words: Vec<String> =
                            cue.split_whitespace().map(str::to_string).collect();
                        let mut line = split_words(0.0, 2.0, &words, " ");
                        line.breaks = super::line::typed_breaks(cue);
                        let mut s = PluginScene::new(base.effect.clone(), cw, ch, vec![line], None)
                            .with_safe_insets(area.insets(cw, ch))
                            .with_values(&values)
                            .unwrap();
                        let (_, _, main, _) = s.frame(&mut fonts, &NoGpu, 1.0);
                        if cue.contains("Supercali") {
                            let wrapped =
                                crate::caption_layout::wrap_lines(cue, font, wrap, max_height)
                                    .unwrap();
                            assert_eq!(wrapped.too_wide, s.too_wide(), "{tag}: both routes agree");
                        }
                        // Lines are F apart: a gap of more than half of F between two
                        // letter centres starts the next line.
                        let mut ys: Vec<f32> = main
                            .iter()
                            .filter(|i| i.color[0] > 0.5)
                            .map(|i| i.center[1])
                            .collect();
                        ys.sort_by(f32::total_cmp);
                        let mut got: Vec<usize> = Vec::new();
                        for (k, y) in ys.iter().enumerate() {
                            if k == 0 || y - ys[k - 1] > font * 0.5 {
                                got.push(0);
                            }
                            *got.last_mut().unwrap() += 1;
                        }
                        if got != expected {
                            // The engine shapes (kerning), caption_layout adds nominal
                            // advances: a line within 3 % of the width may fit in one
                            // only. Any other difference is a rule difference.
                            let kerning = (1..=6).any(|k| {
                                crate::caption_layout::layout_text(
                                    cue,
                                    font,
                                    wrap * (1.0 + 0.005 * k as f32),
                                    max_height,
                                )
                                .is_ok_and(|l| {
                                    l.iter()
                                        .map(|l| l.chars().filter(|c| !c.is_whitespace()).count())
                                        .collect::<Vec<_>>()
                                        == got
                                })
                            });
                            assert!(
                                kerning,
                                "{tag}: {cue}: engine {got:?} vs libass {expected:?} {lines:?}"
                            );
                            near += 1;
                        }
                        breaks += 1;
                    }
                }
            }
        }
    }
    assert!(
        cases >= 80 && breaks >= 150,
        "{cases} canvases, {breaks} cues"
    );
    println!("{cases} canvases, {breaks} cues, {near} broken differently by kerning alone");
    assert!(near * 10 <= breaks, "{near} of {breaks} cues differ");
}

/// Task 43 step C: `layout.line_breaks` is "keep" or "rewrap" (the default);
/// anything else is refused. Classique keeps; the other bundled effects rewrap.
#[test]
fn line_breaks_is_keep_or_rewrap() {
    let karaoke = || -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe/effect.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    let load = |j: &serde_json::Value| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("effect.json"),
            serde_json::to_vec(j).unwrap(),
        )
        .unwrap();
        Effect::load(dir.path()).map_err(|e| e.to_string())
    };
    for value in ["keep", "rewrap"] {
        let mut j = karaoke();
        j["layout"]["line_breaks"] = value.into();
        assert!(load(&j).is_ok(), "{value}");
    }
    let mut j = karaoke();
    j["layout"]["line_breaks"] = "always".into();
    assert!(load(&j).unwrap_err().contains("layout.line_breaks"));
    assert_eq!(classique_json()["layout"]["line_breaks"], "keep");
    let bundled = super::catalog::scan(&[
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx"),
    ]);
    for e in &bundled.effects {
        let j: serde_json::Value =
            serde_json::from_slice(&std::fs::read(e.dir.join("effect.json")).unwrap()).unwrap();
        let declared = j["layout"]["line_breaks"].as_str();
        // Classique keeps; the effects that shipped with it rewrap. A style added
        // later chooses for itself.
        let shipped_rewrap = [
            "noh.block",
            "noh.breath",
            "noh.drop",
            "noh.karaoke-simple",
            "noh.sweep",
        ];
        if e.effect.id != "noh.classic" && !shipped_rewrap.contains(&e.effect.id.as_str()) {
            continue;
        }
        assert_eq!(
            declared == Some("keep"),
            e.effect.id == "noh.classic",
            "{}: {declared:?}",
            e.effect.id
        );
    }
}

/// Task 43 step C: an effect that rewraps lays a two-line cue out as before (one
/// paragraph); Classique draws its two typed lines.
#[test]
fn typed_lines_are_kept_by_classique_only() {
    use super::shaders::Upload;
    use super::text::Fonts;
    struct NoGpu;
    impl Upload for NoGpu {
        fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    }
    let mut fonts = Fonts::load().unwrap();
    let lines = |text: &str| {
        super::line::from_track(&crate::subtitle_track::SubtitleTrack {
            language: None,
            duration_ms: 3000,
            cues: vec![crate::subtitle_track::SubtitleCue::new(0, 2000, text)],
        })
    };
    let mut drawn = |effect: &Effect, text: &str| {
        let mut s = PluginScene::new(effect.clone(), 1920, 1080, lines(text), None);
        let (_, _, main, _) = s.frame(&mut fonts, &NoGpu, 1.0);
        main.iter()
            .map(|i| {
                (
                    (i.center[0] * 10.0).round() as i64,
                    (i.center[1] * 10.0).round() as i64,
                )
            })
            .collect::<Vec<_>>()
    };
    let karaoke = super::catalog::load_one(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fx/wipe"),
        "test.wipe",
    )
    .unwrap();
    assert_eq!(
        drawn(&karaoke, "one two\nthree"),
        drawn(&karaoke, "one two three"),
        "karaoke rewraps as before"
    );
    let classic = super::catalog::builtin_classic();
    let (typed, joined) = (
        drawn(&classic, "one two\nthree"),
        drawn(&classic, "one two three"),
    );
    assert_ne!(typed, joined, "Classique keeps the typed line");
    let rows = |v: &[(i64, i64)]| {
        v.iter().map(|p| p.1).max().unwrap() - v.iter().map(|p| p.1).min().unwrap()
    };
    assert!(
        rows(&typed) > rows(&joined),
        "two lines are taller than one"
    );
}
