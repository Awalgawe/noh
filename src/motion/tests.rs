//! The scene language, its counted limits, the checks of every frame and the
//! script thread. Every bound of the sandbox has a hostile script here.
use super::audio::Audio;
use super::check::Checker;
use super::draw::{Block, MAX_AREA, MAX_BLURS, MAX_GLYPHS, MAX_PARTICLES, MAX_PASSES, measure};
use super::script::{FUNCTIONS, MAX_CALLS, MAX_OPS, NOT_YET, RANGE_LIBRARY, READS, ScriptScene};
use super::thread::{STACK, ScriptThread};
use super::validate::{Report, grid, validate};
use super::{Cue, Inputs};
use crate::fx::text::Fonts;
use std::cell::RefCell;
use std::rc::Rc;

fn inputs(w: u32, h: u32) -> Inputs {
    Inputs {
        w,
        h,
        src_w: w,
        src_h: h,
        duration: 2.0,
        cues: vec![Cue {
            start: 0.0,
            end: 1.0,
            text: "one two".into(),
            words: vec![("one".into(), 0.0, 0.5), ("two".into(), 0.5, 1.0)],
        }],
        words_estimated: false,
        safe: [0.0, 0.0, 1.0, 1.0],
        shaders: Vec::new(),
    }
}

/// One set of fonts for the test thread (loading them is the slow part).
fn fonts() -> Rc<RefCell<Fonts>> {
    thread_local! {
        static FONTS: Rc<RefCell<Fonts>> = Rc::new(RefCell::new(Fonts::load().unwrap()));
    }
    FONTS.with(Rc::clone)
}

/// The scene with its format line: the snippets of these tests leave it out.
fn with_header(src: &str) -> String {
    if src.starts_with("// noh-motion") {
        src.to_string()
    } else {
        format!("// noh-motion 0\n{src}")
    }
}

fn load_at(src: &str, w: u32, h: u32) -> Result<ScriptScene, String> {
    ScriptScene::load(
        &with_header(src),
        &inputs(w, h),
        Rc::new(Audio::silent()),
        fonts(),
        true,
    )
}

fn load(src: &str) -> Result<ScriptScene, String> {
    load_at(src, 640, 360)
}

/// Two seconds from `from` at 24 frames per second. Scenes that do not read the
/// lyrics are checked at 5 s on a 1916 x 1080 frame, where the heaviest legal
/// frames were written to sit just under the caps.
fn run_at(src: &str, from: f64) -> Result<Report, String> {
    let (w, h) = if from > 0.0 { (1916, 1080) } else { (640, 360) };
    let scene = load_at(src, w, h)?;
    let mut checker = Checker::new(fonts(), w, h);
    validate(
        &scene,
        &mut checker,
        &fonts(),
        &grid(from, from + 2.0, 24.0),
        [0.0, 0.0, 1.0, 1.0],
        (w, h),
    )
}

fn run(src: &str) -> Result<Report, String> {
    run_at(src, 0.0)
}

fn refused(src: &str, why: &str) {
    match run(src) {
        Ok(_) => panic!("accepted: {src}"),
        Err(e) => assert!(e.contains(why), "{src}\n  refused for another reason: {e}"),
    }
}

/// Refused, whatever the wording: for what the language no longer has.
fn rejected(src: &str) {
    if run(src).is_ok() {
        panic!("accepted: {src}");
    }
}

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/motion")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_example_scenes_pass() {
    run(&fixture("demo.rhai")).unwrap();
    run(&fixture("layers.rhai")).unwrap();
    run(&fixture("shapes.rhai")).unwrap();
}

/// The allow-list, positively: every named function a script can call. A
/// function added to the engine without being added here fails this test.
#[test]
fn a_script_can_call_these_functions_and_no_other() {
    let scene = load("fn frame(t, ctx) { }").unwrap();
    let expected: Vec<&str> = [FUNCTIONS, NOT_YET, READS, RANGE_LIBRARY].concat();
    let callable = scene.callable();
    let extra: Vec<&String> = callable
        .iter()
        .filter(|n| !expected.contains(&n.as_str()))
        .collect();
    let missing: Vec<&&str> = expected
        .iter()
        .filter(|n| !callable.iter().any(|c| c == **n))
        .collect();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "extra {extra:?}, missing {missing:?}"
    );
}

/// The author page (served to agents by noh_list_effects) names every function
/// a scene can call, and says the ones NOH does not draw yet are refused.
#[test]
fn the_author_page_names_every_function() {
    let page = include_str!("../../docs/MOTION_AUTHORING.md");
    let missing: Vec<&&str> = FUNCTIONS
        .iter()
        .filter(|n| {
            !(page.contains(&format!("`{n}"))
                || page.contains(&format!(".{n}("))
                || page.contains(&format!("{n}(")))
        })
        .collect();
    assert!(missing.is_empty(), "not on the author page: {missing:?}");
    assert!(page.starts_with(
        "# Writing a motion scene

**Experimental"
    ));
    assert!(page.contains("`video()`") && page.contains("## Layer shaders"));
}

/// One hostile script per bound of the sandbox.
#[test]
fn hostile_scripts_are_refused() {
    refused(
        "fn frame(t, ctx) { let i = 0; while true { i += 1; } }",
        "Too many operations",
    );
    refused(
        "fn frame(t, ctx) { let s = \"x\"; for i in 0..40 { s += s; } }",
        "too large",
    );
    refused(
        "fn frame(t, ctx) { let s = \"x\"; for i in 0..40 { s = s + s; } }",
        "too large",
    );
    refused(
        "fn f(n) { f(n + 1) }\nfn frame(t, ctx) { f(0); }",
        "Stack overflow",
    );
    refused(
        "fn frame(t, ctx) { let n = timestamp(); }",
        "Function not found",
    );
    refused("fn frame(t, ctx) { let n = rand(); }", "Function not found");
    refused(
        "fn frame(t, ctx) { let f = open_file(\"x.txt\"); }",
        "Function not found",
    );
    let big = format!("fn frame(t, ctx) {{ }}//{}", "x".repeat(40_000));
    assert!(load(&big).err().unwrap().contains("exceed"));
}

/// What the cut-down language no longer has: each is refused.
#[test]
fn the_language_has_no_large_values_nor_the_standard_library() {
    rejected("fn frame(t, ctx) { let a = []; }");
    rejected("fn frame(t, ctx) { let a = [1.0, 2.0]; let b = a; a[0] = b; }");
    rejected("fn frame(t, ctx) { let s = \"abc\"; let c = s[0]; }");
    rejected("fn frame(t, ctx) { let m = #{}; }");
    rejected("fn frame(t, ctx) { let m = #{a: 1}; m[\"b\"] = 2; }");
    rejected("fn frame(t, ctx) { let m = parse_json(\"{}\"); }");
    rejected("fn frame(t, ctx) { let b = blob(1000); }");
    rejected("fn frame(t, ctx) { let s = \"x\"; s.pad(1000, \"y\"); }");
    rejected("fn frame(t, ctx) { let a = \"a b c\".split(\" \"); }");
    rejected("fn frame(t, ctx) { let s = \"abc\".to_string(); }");
    rejected("fn frame(t, ctx) { let s = \"abc\".sub_string(0, 1); }");
    rejected("fn frame(t, ctx) { let f = Fn(\"frame\"); }");
    rejected("fn g(x) { x }\nfn frame(t, ctx) { let f = g; let h = f.curry(\"x\"); }");
    rejected("fn g(x) { x }\nfn frame(t, ctx) { let f = g; f.call(1.0); }");
    rejected("fn frame(t, ctx) { let f = |x| x + 1.0; }");
    rejected("fn g(x) { x }\nfn frame(t, ctx) { call(g, 1.0); }");
    rejected("fn frame(t, ctx) { let s = \"x\"; let f = |x| s; }");
    rejected("fn g(x, y) { x }\nfn frame(t, ctx) { let f = Fn(\"g\").curry(\"x\"); }");
    rejected("import \"fs\" as fs;\nfn frame(t, ctx) { }");
    rejected("fn frame(t, ctx) { eval(\"1+1\"); }");
}

/// A script that spends too much is refused on its operation count, not on a clock.
#[test]
fn a_too_slow_script_is_refused_on_operations() {
    refused(
        "fn frame(t, ctx) { let s = 0.0; for i in 0..150000 { s += 1.0; } }",
        "Too many operations",
    );
}

/// The picture's framing: one per frame, its settings within their ranges, never
/// in a layer; one that leaves black at an edge is said, not refused.
#[test]
fn the_picture_is_framed_once_per_frame_within_its_ranges() {
    let scene = load(
        "fn frame(t, ctx) { draw(video().scale(1.2).focus(0.6, 0.4).at(0.5, 0.5).rot(0.1)\
         .crop(0.0, 0.0, 1.0, 1.0)); }",
    )
    .unwrap();
    let v = scene.frame(0.0).unwrap().video.unwrap();
    assert_eq!((v.scale, v.focus, v.rot), (1.2, [0.6, 0.4], 0.1));
    // Without video(), no framing.
    assert!(
        load("fn frame(t, ctx) { }")
            .unwrap()
            .frame(0.0)
            .unwrap()
            .video
            .is_none()
    );
    refused(
        "fn frame(t, ctx) { draw(video()); draw(video()); }",
        "one video() per frame",
    );
    refused(
        "fn frame(t, ctx) { draw(video().scale(1.6)); }",
        "video scale",
    );
    refused(
        "fn frame(t, ctx) { draw(video().rot(0.5)); }",
        "video rotation",
    );
    refused(
        "fn frame(t, ctx) { draw(video().scale(0.0 / 0.0)); }",
        "not finite",
    );
    refused(
        "fn frame(t, ctx) { draw(video().scale(0.1)); }",
        "video scale",
    );
    refused(
        "fn frame(t, ctx) { draw(video().focus(1.2, 0.5)); }",
        "video focus (0 to 1) or position (-0.5 to 1.5) out of range",
    );
    refused(
        "fn frame(t, ctx) { draw(video().at(0.5, 1.6)); }",
        "video focus (0 to 1) or position (-0.5 to 1.5) out of range",
    );
    refused(
        "fn frame(t, ctx) { draw(video().crop(0.5, 0.0, 0.52, 1.0)); }",
        "video crop out of range",
    );
    rejected("fn frame(t, ctx) { let l = layer(); l.add(video()); draw(l); }");
    // A framing that leaves black is said, never refused.
    let r = run("fn frame(t, ctx) { draw(video().scale(0.8)); }").unwrap();
    assert!(
        r.layout_warnings
            .iter()
            .any(|w| w.contains("does not cover the frame")),
        "{:?}",
        r.layout_warnings
    );
}

/// A layer's shader is named among the files beside the scene; its settings
/// follow it, within their ranges.
#[test]
fn a_layer_shader_is_named_beside_the_scene_and_its_settings_follow_it() {
    refused(
        "fn frame(t, ctx) { let l = layer().shader(\"glow\"); draw(l); }",
        "no shader \"glow\" beside the scene",
    );
    refused(
        "fn frame(t, ctx) { draw(layer().param(0, 1.0)); }",
        "param() follows shader()",
    );
    let mut inp = inputs(640, 360);
    inp.shaders = vec!["glow".into()];
    let scene = ScriptScene::load(
        &with_header(
            "fn frame(t, ctx) { draw(layer().shader(\"glow\").resolution(\"full\").param(7, 2.5)); }",
        ),
        &inp,
        Rc::new(Audio::silent()),
        fonts(),
        false,
    )
    .unwrap();
    let f = scene.frame(0.0).unwrap();
    let spec = f.layers[0].shader.as_ref().unwrap();
    assert_eq!(
        (spec.name.as_str(), spec.full, spec.k[7]),
        ("glow", true, 2.5)
    );
    // An empty layer with a shader is a pass of its own.
    assert_eq!(super::check::pass_count(&f), 1);
    for (bad, why) in [
        (
            "layer().shader(\"glow\").param(8, 1.0)",
            "index from 0 to 7",
        ),
        (
            "layer().shader(\"glow\").resolution(\"quarter\")",
            "unknown (half, full)",
        ),
        // A number that is not finite reaches no shader.
        (
            "layer().shader(\"glow\").param(0, 0.0 / 0.0)",
            "a finite number",
        ),
    ] {
        let s = ScriptScene::load(
            &with_header(&format!("fn frame(t, ctx) {{ draw({bad}); }}")),
            &inp,
            Rc::new(Audio::silent()),
            fonts(),
            false,
        )
        .unwrap();
        let e = s.frame(0.0).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    // A shader layer goes in no layer, as no layer does.
    let nested = "fn frame(t, ctx) { let l = layer(); l.add(layer().shader(\"glow\")); draw(l); }";
    let s = ScriptScene::load(
        &with_header(nested),
        &inp,
        Rc::new(Audio::silent()),
        fonts(),
        false,
    )
    .unwrap();
    assert!(s.frame(0.0).is_err(), "a shader layer nested in a layer");
    // Over the counted caps: five shader layers (the layers' cap), two at full
    // resolution (2 frames of pixels, over the 1.5 cap).
    for (src, why) in [
        (
            "fn frame(t, ctx) { for k in 0..5 { draw(layer().shader(\"glow\")); } }",
            "shader layers exceed",
        ),
        (
            "fn frame(t, ctx) { for i in 0..2 { draw(layer().shader(\"glow\").resolution(\"full\")); } }",
            "frames of pixels exceed",
        ),
    ] {
        let s = ScriptScene::load(
            &with_header(src),
            &inp,
            Rc::new(Audio::silent()),
            fonts(),
            false,
        )
        .unwrap();
        let mut checker = Checker::new(fonts(), 640, 360);
        let e = checker.check(&s.frame(0.0).unwrap(), 0.0).unwrap_err();
        assert!(e.contains(why), "{src}: {e}");
    }
    // At the pixel cap exactly (one full, two halves) it passes.
    let at_cap = "fn frame(t, ctx) { draw(layer().shader(\"glow\").resolution(\"full\"));                   for i in 0..2 { draw(layer().shader(\"glow\")); } }";
    let s = ScriptScene::load(
        &with_header(at_cap),
        &inp,
        Rc::new(Audio::silent()),
        fonts(),
        false,
    )
    .unwrap();
    let mut checker = Checker::new(fonts(), 640, 360);
    checker.check(&s.frame(0.0).unwrap(), 0.0).unwrap();
}

/// A scene names the format it is written for on its first line; another
/// format, or none, is refused with that reason.
#[test]
fn a_scene_names_its_format_on_its_first_line() {
    let fonts = fonts();
    let load = |src: &str| {
        ScriptScene::load(
            src,
            &inputs(640, 360),
            Rc::new(Audio::silent()),
            fonts.clone(),
            false,
        )
        .err()
    };
    assert_eq!(load("// noh-motion 0\nfn frame(t, ctx) { }"), None);
    assert_eq!(
        load("\u{feff}// noh-motion 0\r\nfn frame(t, ctx) { }"),
        None
    );
    let e = load("// noh-motion 1\nfn frame(t, ctx) { }").unwrap();
    assert!(e.contains("written for scene format \"1\""), "{e}");
    let e = load("fn frame(t, ctx) { }").unwrap();
    assert!(e.contains("the first line must be"), "{e}");
}

#[test]
fn out_of_bounds_draws_are_refused() {
    refused(
        "fn frame(t, ctx) { for i in 0..300 { draw(rect(0.1, 0.1)); } }",
        "draws in one frame",
    );
    refused(
        "fn frame(t, ctx) { draw(particles_in(0.0, 0.0, 1.0, 1.0).during(0.0, 100.0).rate(5000.0).life(10.0)); }",
        "exceeds 2000",
    );
    refused(
        "fn frame(t, ctx) { draw(rect(0.1, 0.1).bogus(1.0)); }",
        "Function not found",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").face(\"nope\")); }",
        "unknown",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").color(\"red\")); }",
        "#RRGGBB",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").scale(9.0)); }",
        "exceeds",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").word(32, 1.0, 0.0, 1.0)); }",
        "word index",
    );
    refused(
        "fn frame(t, ctx) { let p = particles_in(0.0, 0.0, 1.0, 1.0); for i in 0..9 { p.color(\"#FFFFFF\"); } }",
        "colours",
    );
    refused("fn frame(t, ctx) { 1.0 }", "returns nothing");
}

/// Whatever a script writes into its own values is not there at the next frame.
#[test]
fn a_frame_cannot_carry_state_to_the_next() {
    run("fn frame(t, ctx) { let d = rect(0.1, 0.1); d.at(ctx.w / 6400.0, 0.5); draw(d); }")
        .unwrap();
    rejected("fn frame(t, ctx) { ctx.w = 0.0; }");
}

#[test]
fn the_lyrics_are_read_through_bounded_functions() {
    run(
        "fn frame(t, ctx) { let c = cue(cue_at(0.2)); let w = word(0, 1); \
         draw(text(c.text + \" \" + w.text)); }",
    )
    .unwrap();
    refused("fn frame(t, ctx) { cue(7); }", "there are 1 cues");
    refused(
        "fn frame(t, ctx) { for i in 0..2000 { cue(0); } }",
        "reads of the lyrics",
    );
}

/// Measuring is bounded like any costly function, and makes no glyph image.
#[test]
fn measuring_is_bounded_and_draws_nothing() {
    let r = run(
        "fn frame(t, ctx) { let d = text(\"one two\").size(0.2).anchor(\"left\").at(0.1, 0.5); \
         let b = measure(d); let w = word_box(d, 1); \
         draw(rect(b.w, b.h).at((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0)); draw(d); }",
    )
    .unwrap();
    assert_eq!(r.max_calls, 2);
    refused(
        "fn frame(t, ctx) { for i in 0..70 { measure(text(\"a\").size(0.05 + i.to_float() / 1000.0)); } }",
        "measures in one frame",
    );
    refused(
        "fn frame(t, ctx) { let s = \"0123456789abcdef\"; for i in 0..5 { s += s; } measure(text(s)); }",
        "characters",
    );
    refused(
        "fn frame(t, ctx) { word_box(text(\"a b\"), 5); }",
        "has 2 words",
    );
    refused(
        "fn frame(t, ctx) { for i in 0..1000 { measure(text(\"a\").at(i.to_float() / 1000.0, 0.5)); } }",
        "measures in one frame",
    );
}

/// The examples the authoring page gives run as written.
#[test]
fn the_page_examples_run() {
    run("fn frame(t, ctx) { if t >= 1.0 && t < 4.0 { \
         draw(text(\"hello\").at(0.5, 0.8).size(0.06).opacity(smooth(1.0, 1.5, t))); } }")
    .unwrap();
    run(
        "fn frame(t, ctx) { let a = text(\"left part\").anchor(\"left\").size(0.05); \
         let b = text(\"right part\").anchor(\"left\").size(0.09); let x = 0.1; \
         draw(a.at(x, 0.5)); draw(b.at(x + measure(a).w + 0.02, 0.5)); }",
    )
    .unwrap();
    run("fn line(t, i, y) { let c = cue(i); \
         if t < c.start - 0.3 || t > c.end + 0.5 { return; } \
         let d = text(c.text).at(0.5, y).size(0.05); \
         for k in 0..c.words { let w = word(i, k); \
         d.word(k, smooth(w.start - 0.1, w.start + 0.2, t), 0.0, 1.0); } draw(d); }
         fn frame(t, ctx) { line(t, 0, 0.8); let s = sin(t) + t.cos() + pick(1, 1.0, 2.0, 3.0); \
         draw(rect(0.1, 0.1).at(ctx.safe_x0 + 0.1, 0.5).opacity(clampf(s, 0.0, 1.0))); }")
    .unwrap();
}

/// Words of different sizes share a baseline with the baseline anchors.
#[test]
fn baseline_anchors_put_the_first_baseline_at_y() {
    let f = fonts();
    let mut big = Block::new("Hxg");
    big.size = 0.2;
    big.anchor = "baseline_left".into();
    big.y = 0.5;
    let mut small = big.clone();
    small.size = 0.05;
    let (b, _) = measure(&mut f.borrow_mut(), &big, 1.0, 640.0, 360.0);
    let (s, _) = measure(&mut f.borrow_mut(), &small, 1.0, 640.0, 360.0);
    assert!(
        b.y0 < s.y0 && s.y0 < 0.5 && 0.5 < s.y1 && s.y1 < b.y1,
        "{b:?} {s:?}"
    );
    run("fn frame(t, ctx) { draw(text(\"a\").anchor(\"baseline\")); }").unwrap();
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").anchor(\"middle\")); }",
        "anchor",
    );
}

/// The counted caps on what a frame draws, each refused by name.
#[test]
fn a_frame_cannot_draw_more_than_its_caps() {
    refused(
        "fn frame(t, ctx) { let s = \"The quick brown fox jumps over a lazy dog and \"; \
         let l = s + s + s + s + s; for i in 0..9 { draw(text(l).size(0.02).at(0.5, 0.1 * i.to_float()) \
         .shadow(0.5, 0.08, 0.04).outline(\"#000000\", 0.04).dissolve(0.3).glow(1.0)); } }",
        "letters drawn",
    );
    refused(
        "fn frame(t, ctx) { for i in 0..3 { draw(particles_in(0.0, 0.0, 1.0, 1.0) \
         .during(0.0, 100.0).rate(1500.0).life(1.3).seed(i)); } }",
        "particles exceed",
    );
    refused(
        "fn frame(t, ctx) { let f = (t * 30.0).to_int(); for i in 0..12 { \
         draw(text(\"a\").tracking((f * 12 + i).to_float() * 0.001)); } }",
        "different texts",
    );
}

/// Shapes: every bound refuses, a fill that crosses itself or has a hole is
/// refused, and runs of shapes are passes like any other.
#[test]
fn shapes_are_bounded() {
    let r = run(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.2).fill(\"#FFFFFF\").stroke(\"#000000\", 0.01)); \
         draw(line(0.1, 0.1, 0.9, 0.1).stroke(\"#FFFFFF\", 0.004).caps(\"round\").trim(0.0, 0.5)); }",
    )
    .unwrap();
    assert_eq!((r.max_paths, r.max_passes), (2, 1));
    refused(
        "fn frame(t, ctx) { for i in 0..65 { draw(line(0.1, 0.1, 0.2, 0.2).stroke(\"#FFFFFF\", 0.004)); } }",
        "paths exceed",
    );
    refused(
        "fn frame(t, ctx) { let p = path().move_to(0.1, 0.1); for i in 0..256 { p.line_to(0.2, 0.2); } draw(p); }",
        "more than 256 points",
    );
    refused(
        "fn frame(t, ctx) { let p = path().move_to(0.1, 0.5); for i in 0..120 { \
         let x = 0.1 + i.to_float() * 0.0066; p.quad_to(x + 0.0033, 0.05 + (i % 2).to_float() * 0.9, x + 0.0066, 0.5); } \
         draw(p.line_to(0.9, 0.95).line_to(0.1, 0.95).close().fill(\"#FFFFFF\")); }",
        "exceeds 128",
    );
    refused(
        "fn frame(t, ctx) { for k in 0..7 { draw(circle(0.5, 0.5, 0.5).scale(10.0).stroke(\"#FFFFFF\", 0.001)); } }",
        "segments once flattened",
    );
    refused(
        "fn frame(t, ctx) { for k in 0..8 { draw(path().move_to(0.1, 0.5).quad_to(0.5, 5.0, 0.9, 0.5) \
         .line_to(0.9, 0.4).line_to(0.1, 0.4).close().fill(\"#FFFFFF\")); } }",
        "points of filled paths",
    );
    refused(
        "fn frame(t, ctx) { for i in 0..5 { draw(rounded_rect(0.5, 0.5, 1.0, 1.0, 0.0).fill(\"#FFFFFF\")); } }",
        "boxes cover",
    );
    refused(
        "fn frame(t, ctx) { draw(path().move_to(0.1, 0.1).line_to(0.9, 0.9).line_to(0.9, 0.1).line_to(0.1, 0.9) \
         .close().fill(\"#FFFFFF\")); }",
        "crosses itself",
    );
    refused(
        "fn frame(t, ctx) { draw(path().move_to(0.1, 0.1).line_to(0.9, 0.1).line_to(0.5, 0.5).line_to(0.9, 0.9) \
         .line_to(0.1, 0.9).line_to(0.5, 0.5).close().fill(\"#FFFFFF\")); }",
        "corner touches",
    );
    refused(
        "fn frame(t, ctx) { draw(path().move_to(0.1, 0.1).line_to(0.3, 0.1).line_to(0.3, 0.3).close() \
         .move_to(0.5, 0.5).line_to(0.7, 0.5).line_to(0.7, 0.7).close().fill(\"#FFFFFF\")); }",
        "one contour",
    );
    refused(
        "fn frame(t, ctx) { draw(line(0.1, 0.1, 0.9, 0.9).stroke(\"#FFFFFF\", 0.3)); }",
        "stroke width",
    );
    refused(
        "fn frame(t, ctx) { let p = circle(0.5, 0.5, 0.1).linear(0.0, 0.0, 1.0, 1.0); \
         for i in 0..9 { p.stop(i.to_float() / 9.0, \"#FFFFFF\", 1.0); } draw(p); }",
        "more than 8 stops",
    );
    refused(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.1).linear(0.0, 0.0, 1.0, 1.0).stop(0.0, \"#FFFFFF\", 1.0)); }",
        "2 to 8 stops",
    );
    refused(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.1).stop(0.0, \"#FFFFFF\", 1.0)); }",
        "follows linear",
    );
    refused(
        "fn frame(t, ctx) { draw(line(0.1, 0.1, 0.9, 0.9).stroke(\"#FFFFFF\", 0.01).caps(\"square\")); }",
        "caps",
    );
    refused(
        "fn frame(t, ctx) { draw(line(0.1, 0.1, 0.9, 0.9).stroke(\"#FFFFFF\", 0.01).trim(0.6, 0.2)); }",
        "trim",
    );
    refused(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.1).fill(\"#FFFFFF\").scale(20.0)); }",
        "path scale",
    );
    refused(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.0 / 0.0).fill(\"#FFFFFF\")); }",
        "not finite",
    );
    refused(
        "fn frame(t, ctx) { draw(circle(0.5, 0.5, 0.1).fill(\"red\")); }",
        "colour",
    );
    refused(
        "fn frame(t, ctx) { draw(line(0.1, 0.1, 0.9, 0.9).stroke(\"#FFFFFF\", 0.0001)); }",
        "stroke width",
    );
    let r = run(
        "fn frame(t, ctx) { let l = layer(); l.add(text(\"a\")); l.add(circle(0.5, 0.5, 0.1).fill(\"#FFFFFF\")); \
         draw(l.opacity(0.5)); }",
    )
    .unwrap();
    assert_eq!(r.max_passes, 2);
    refused(
        "fn frame(t, ctx) { for i in 0..5 { draw(text(\"a\")); draw(circle(0.5, 0.5, 0.1).fill(\"#FFFFFF\")); } }",
        "passes exceed",
    );
    for name in [
        "worst_fills.rhai",
        "worst_hard_fills.rhai",
        "worst_strokes.rhai",
        "worst_shapes.rhai",
        "worst_shape_area.rhai",
    ] {
        let r = run_at(&fixture(name), 5.0).unwrap();
        assert!(
            r.layout_warnings.is_empty(),
            "{name}: {:?}",
            r.layout_warnings
        );
    }
}

/// The shapes the cutting finds hardest are cut cleanly at the cap.
#[test]
fn hard_shapes_are_cut_cleanly() {
    use super::shapes::{MAX_FILL_POINTS, ear_clip_counted, hard_polygons, self_crossing};
    for (name, poly) in hard_polygons(MAX_FILL_POINTS) {
        let px: Vec<[f32; 2]> = poly
            .iter()
            .map(|p| [p[0] * 1916.0, p[1] * 1080.0])
            .collect();
        assert!(!self_crossing(&px), "{name}");
        let (tri, forced) = ear_clip_counted(&px);
        assert_eq!((tri.len() / 3, forced), (px.len() - 2, 0), "{name}");
    }
}

/// Layers: counted as passes, capped, never nested, their draws counted.
#[test]
fn layers_are_counted_and_capped() {
    use super::draw::{Span, passes};
    let s = |first, len| Span {
        first,
        len,
        opacity: 1.0,
        shader: None,
    };
    assert_eq!(passes(0, &[]).len(), 0);
    assert_eq!(passes(5, &[]), vec![(0..5, 1.0)]);
    assert_eq!(
        passes(9, &[s(2, 3), s(5, 0), s(6, 2)]),
        vec![
            (0..2, 1.0),
            (2..5, 1.0),
            (5..6, 1.0),
            (6..8, 1.0),
            (8..9, 1.0)
        ]
    );
    let r = run(
        "fn frame(t, ctx) { let l = layer(); l.add(text(\"a\")); l.add(rect(0.1, 0.1)); \
         draw(l.opacity(0.5)); draw(text(\"b\").at(0.5, 0.8)); }",
    )
    .unwrap();
    assert_eq!((r.max_passes, r.max_draws), (2, 3));
    refused(
        "fn frame(t, ctx) { for i in 0..9 { let l = layer(); l.add(rect(0.1, 0.1)); draw(l); } }",
        "passes exceed",
    );
    refused(
        "fn frame(t, ctx) { let l = layer(); for i in 0..100 { l.add(rect(0.01, 0.01)); } \
         draw(l); draw(l); draw(l); }",
        "draws",
    );
    rejected("fn frame(t, ctx) { let l = layer(); let m = layer(); l.add(m); draw(l); }");
    refused(
        "fn frame(t, ctx) { let l = layer(); l.add(rect(0.1, 0.1)); draw(l.opacity(0.0 / 0.0)); }",
        "opacity is not finite",
    );
    let w = run_at(&fixture("worst_layers.rhai"), 5.0).unwrap();
    assert_eq!(w.max_passes, MAX_PASSES);
    assert!(w.max_area > MAX_AREA * 0.95, "{}", w.max_area);
}

/// What a frame covers is capped, whatever the number of letters.
#[test]
fn a_frame_cannot_cover_more_than_its_cap() {
    refused(
        "fn frame(t, ctx) { for i in 0..17 { draw(rect(1.0, 1.0).opacity(0.1)); } }",
        "frames (every pass counted)",
    );
    // A glow draws the rectangle twice: nine glowing frames are eighteen.
    refused(
        "fn frame(t, ctx) { for i in 0..9 { draw(rect(1.0, 1.0).opacity(0.1).glow(1.0)); } }",
        "frames (every pass counted)",
    );
    refused(
        "fn frame(t, ctx) { for i in 0..7 { draw(text(\"M\").size(0.5).scale(4.0) \
         .shadow(0.5, 0.01, 0.04).outline(\"#000000\", 0.04).dissolve(0.3).glow(1.0)); } }",
        "frames (every pass counted)",
    );
    refused(
        "fn frame(t, ctx) { draw(particles_in(0.0, 0.0, 1.0, 1.0).during(-9.0, 100.0) \
         .rate(100.0).life(4.0).size(0.5)); }",
        "frames (every pass counted)",
    );
    run("fn frame(t, ctx) { for i in 0..200 { draw(rect(1.0, 1.0).at(3.0, 3.0)); } }").unwrap();
    refused(
        "fn frame(t, ctx) { for i in 0..9 { draw(text(\"a\").size(0.05) \
         .shadow(0.5, 0.05 + i.to_float() * 0.02, 0.04)); } }",
        "different shadow blurs",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").shadow(0.5, 1.5, 0.0)); }",
        "shadow blur",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").outline(\"#000000\", 0.3)); }",
        "outline width",
    );
    refused(
        "fn frame(t, ctx) { draw(text(\"a\").line_height(0.1)); }",
        "line height",
    );
    refused(
        "fn frame(t, ctx) { draw(particles_in(0.0, 0.0, 1.0, 1.0).during(0.0, 9.0).size(0.6)); }",
        "particle size",
    );
}

/// The heaviest legal frames are legal: they pass, at or just under each cap.
#[test]
fn the_worst_legal_frames_pass_at_their_caps() {
    let all = run_at(&fixture("worst_all.rhai"), 5.0).unwrap();
    assert!(all.max_area > MAX_AREA * 0.95, "{}", all.max_area);
    assert!(all.max_blurs + 1 >= MAX_BLURS, "{}", all.max_blurs);
    assert!(all.max_glyphs * 10 > MAX_GLYPHS * 9, "{}", all.max_glyphs);
    assert!(all.max_particles * 10 > MAX_PARTICLES * 9);
    assert_eq!(all.max_calls, MAX_CALLS);
    assert!(all.max_ops * 10 > MAX_OPS * 9, "{}", all.max_ops);
    let area = run_at(&fixture("worst_area.rhai"), 5.0).unwrap();
    assert_eq!(area.max_blurs, MAX_BLURS);
    let rects = run_at(&fixture("worst_rects.rhai"), 5.0).unwrap();
    assert_eq!(rects.max_area, MAX_AREA);
    for name in [
        "worst_zoom.rhai",
        "worst_text.rhai",
        "worst_particles.rhai",
        "worst_measure.rhai",
    ] {
        run_at(&fixture(name), 5.0).unwrap();
    }
}

/// A cross-fade is not an overlap; rotated text is listed as not checked; a
/// text leaving the frame or covering another is said, not refused.
#[test]
fn layout_problems_are_said_never_refused() {
    let r = run(
        "fn frame(t, ctx) { draw(text(\"a\").opacity(0.3)); draw(text(\"b\")); \
         draw(text(\"c\").rot(0.5)); }",
    )
    .unwrap();
    assert!(r.layout_warnings.is_empty(), "{:?}", r.layout_warnings);
    assert!(r.not_checked.iter().any(|n| n.contains("rotated")));
    let r = run(
        "fn frame(t, ctx) { draw(text(\"wide open\").size(0.5).width(4.0)); \
         draw(text(\"a\").at(0.5, 0.5)); draw(text(\"b\").at(0.5, 0.5)); }",
    )
    .unwrap();
    assert!(
        r.layout_warnings
            .iter()
            .any(|w| w.contains("leaves the frame"))
    );
    assert!(r.layout_warnings.iter().any(|w| w.contains("overlaps")));
}

/// The heaviest scripts the caps still allow pass and spend nearly all the
/// operations: one per kind of value a script can build.
#[test]
fn the_heaviest_legal_scripts_pass() {
    let budget = (MAX_OPS as usize).saturating_sub(500);
    for (src, per_turn_x10) in [
        (
            "fn frame(t, ctx) { let s = \"0123456789abcdef\"; \
             for i in 0..6 { s += s; } for i in 0..N { let b = s + \"\"; } }",
            40,
        ),
        (
            "fn frame(t, ctx) { let s = \"0123456789abcdef\"; \
             for i in 0..4 { s += s; } let d = text(s); \
             for k in 0..32 { d.word(k, 1.0, 0.0, 1.0); d.word_color(k, \"#FFFFFF\"); } \
             for i in 0..N { let b = d; } }",
            23,
        ),
    ] {
        let src = src.replace('N', &(budget * 10 / per_turn_x10).to_string());
        let r = run(&src).unwrap();
        assert!(
            r.max_ops * 10 > MAX_OPS * 7,
            "{src}: {} operations",
            r.max_ops
        );
    }
}

/// A pure script can exceed a cap between two instants of the validated grid:
/// the validation does not see it, the check of the frame itself does. The
/// preview, set on the export's grid, never evaluates such an instant.
#[test]
fn a_cap_exceeded_between_two_grid_instants_is_caught_by_the_frame_check() {
    // At 24 frames per second the grid holds 0.5 and 0.541 6..., never 0.52.
    // Three emitters born since long before, all alive: 3 x 1 950 particles.
    let src = "fn frame(t, ctx) { if t > 0.51 && t < 0.53 { \
               for i in 0..3 { draw(particles_in(0.0, 0.0, 1.0, 1.0) \
               .during(-10.0, 100.0).rate(1500.0).life(1.3).seed(i)); } } }";
    let report = run(src).unwrap();
    assert_eq!(report.max_particles, 0);
    let scene = load(src).unwrap();
    let mut checker = Checker::new(fonts(), 640, 360);
    let off_grid = scene.frame(0.52).unwrap();
    let e = checker.check(&off_grid, 0.52).unwrap_err();
    assert!(
        e.contains("at t=0.520") && e.contains("particles exceed"),
        "{e}"
    );
    for t in grid(0.0, 2.0, 24.0) {
        checker.check(&scene.frame(t).unwrap(), t).unwrap();
    }
}

/// An expression nesting `inner` `depth` times in `wrap` (which holds `{}`).
fn nested(depth: usize, wrap: &str, inner: &str) -> String {
    let mut s = inner.to_string();
    for _ in 0..depth {
        s = wrap.replace("{}", &s);
    }
    s
}

/// The deepest script of one form the limits allow: every call level used, each
/// function body nesting `wrap` as deep as it may; and, at the top level (which
/// the engine evaluates before each call of `frame`), one statement nesting
/// `wrap` as deep as the top level may. `probe()` marks the stack.
fn deepest(calls: usize, depth: usize, top: usize, wrap: &str) -> String {
    format!(
        "// noh-motion 0\n\
         let top = {};\n\
         fn f(n) {{ if n <= 0 {{ probe() }} else {{ {} }} }}\n\
         fn frame(t, ctx) {{ let x = f({calls}); }}",
        nested(top, wrap, "probe()"),
        nested(depth, wrap, "f(n - 1)")
    )
}

/// The deepest legal scripts run on the script thread without exhausting its
/// stack, and use at most half of it. Beyond the limits they are refused by the
/// engine, never by the stack. Two forms: nested operators and nested calls of
/// a native function (each nests the interpreter differently).
#[test]
fn the_deepest_legal_script_fits_the_script_thread() {
    use std::cell::Cell;
    thread_local! {
        static LOWEST: Cell<usize> = const { Cell::new(usize::MAX) };
    }
    fn probe() -> f64 {
        let x = 0u8;
        let at = std::hint::black_box(&x) as *const u8 as usize;
        LOWEST.with(|l| l.set(l.get().min(at)));
        0.0
    }
    let used = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            let base = {
                let marker = 0u8;
                std::hint::black_box(&marker) as *const u8 as usize
            };
            let fonts = Rc::new(RefCell::new(Fonts::load().unwrap()));
            for wrap in ["(1.0 + {})", "abs({})"] {
                let (mut calls, mut depth, mut top) = (
                    super::script::MAX_CALL_LEVELS,
                    super::script::MAX_FN_EXPR_DEPTH,
                    super::script::MAX_EXPR_DEPTH,
                );
                loop {
                    let src = deepest(calls, depth, top, wrap);
                    // Parsing it fits a quarter of the script thread's stack.
                    let parsed = std::thread::Builder::new()
                        .stack_size(STACK / 4)
                        .spawn({
                            let src = src.clone();
                            move || {
                                let fonts = Rc::new(RefCell::new(Fonts::load().unwrap()));
                                ScriptScene::load(
                                    &src,
                                    &inputs(640, 360),
                                    Rc::new(Audio::silent()),
                                    fonts,
                                    false,
                                )
                                .err()
                            }
                        })
                        .unwrap()
                        .join()
                        .unwrap();
                    if let Some(e) = parsed {
                        assert!(e.contains("exceeds maximum complexity"), "{e}");
                        // Lower the top level first, then the function bodies.
                        if top > depth {
                            top -= 1;
                        } else {
                            depth -= 1;
                        }
                        continue;
                    }
                    let mut s = ScriptScene::load(
                        &src,
                        &inputs(640, 360),
                        Rc::new(Audio::silent()),
                        fonts.clone(),
                        false,
                    )
                    .unwrap();
                    s.register_test_fn("probe", probe);
                    match s.frame(0.0) {
                        Ok(_) => break,
                        Err(e) if e.contains("Stack overflow") => calls -= 1,
                        Err(e) => panic!("{e}"),
                    }
                }
                eprintln!(
                    "deepest legal {wrap}: {calls} call levels, bodies nested {depth} deep, top level {top} deep"
                );
            }
            base - LOWEST.with(Cell::get)
        })
        .unwrap()
        .join()
        .unwrap();
    eprintln!(
        "script thread: deepest legal evaluation used {} KiB of {} KiB",
        used / 1024,
        STACK / 1024
    );
    assert!(used * 2 <= STACK, "{used} bytes used of {STACK}");
}

/// The script thread loads, checks every frame it evaluates, validates, and
/// refuses a scene that does not compile.
#[test]
fn the_script_thread_serves_checked_frames() {
    let src = "fn frame(t, ctx) { draw(text(\"one\").opacity(smooth(0.0, 1.0, t))); \
               if t > 1.5 { for i in 0..300 { draw(rect(0.1, 0.1)); } } }";
    let th =
        ScriptThread::start(with_header(src), inputs(640, 360), Audio::silent(), true).unwrap();
    let (f, c) = th.frame(0.5).unwrap();
    assert_eq!((f.draws.len(), c.glyphs), (1, 3));
    assert!(th.frame(1.6).unwrap_err().contains("draws in one frame"));
    let r = th.validate(grid(0.0, 1.0, 24.0)).unwrap();
    assert_eq!(r.frames, 24);
    assert!(th.validate(grid(0.0, 2.0, 24.0)).is_err());
    let e = ScriptThread::start(
        with_header("fn frame("),
        inputs(640, 360),
        Audio::silent(),
        false,
    )
    .err()
    .unwrap();
    assert!(e.starts_with("script:"), "{e}");
}

/// The example scenes count what the prototype they come from counted (its
/// validation on a 1916 x 1080 frame, 5 to 7 s at 24 frames per second): ops,
/// draws, particles, letters, surface, blurs, passes, paths, segments, shape
/// surface, calls, distinct texts.
#[test]
fn the_example_scenes_count_what_the_prototype_counted() {
    #[rustfmt::skip]
    let expected: [(&str, [usize; 9], [f32; 2], usize); 14] = [
        // name, [ops, draws, particles, glyphs, blurs, passes, paths, segments, calls], [area, shape area], blocks
        ("shapes.rhai", [744, 5, 0, 22, 0, 3, 4, 155, 0], [0.031_002, 0.234_580], 1),
        ("worst_all.rhai", [5779, 14, 3994, 5552, 7, 1, 0, 0, 64], [15.977_318, 0.0], 19),
        ("worst_area.rhai", [614, 13, 0, 624, 8, 1, 0, 0, 0], [15.163_318, 0.0], 1),
        ("worst_fills.rhai", [277, 4, 0, 0, 0, 1, 4, 464, 0], [0.0, 0.197_466], 0),
        ("worst_hard_fills.rhai", [3659, 4, 0, 0, 0, 1, 4, 510, 0], [0.0, 0.529_114], 0),
        ("worst_layers.rhai", [5797, 14, 3994, 5552, 7, 8, 0, 0, 64], [15.977_318, 0.0], 19),
        ("worst_measure.rhai", [1513, 1, 0, 0, 0, 1, 0, 0, 64], [0.001, 0.0], 0),
        ("worst_particles.rhai", [97, 2, 3994, 0, 0, 1, 0, 0, 0], [6.001_106, 0.0], 0),
        ("worst_rects.rhai", [163, 8, 0, 0, 0, 1, 0, 0, 0], [16.0, 0.0], 0),
        ("worst_shape_area.rhai", [303, 4, 0, 0, 0, 1, 4, 416, 0], [0.0, 3.807_342], 0),
        ("worst_shapes.rhai", [3789, 6, 0, 0, 0, 1, 6, 1022, 0], [0.0, 0.613_551], 0),
        ("worst_strokes.rhai", [261, 4, 0, 0, 0, 1, 4, 1024, 0], [0.0, 0.168_875], 0),
        ("worst_text.rhai", [396, 8, 0, 5472, 3, 1, 0, 0, 0], [2.305_077, 0.0], 17),
        ("worst_zoom.rhai", [188, 5, 0, 20, 5, 1, 0, 0, 0], [13.384_731, 0.0], 1),
    ];
    for (name, n, a, blocks) in expected {
        let r = run_at(&fixture(name), 5.0).unwrap();
        let got = [
            r.max_ops as usize,
            r.max_draws,
            r.max_particles,
            r.max_glyphs,
            r.max_blurs,
            r.max_passes,
            r.max_paths,
            r.max_segments,
            r.max_calls as usize,
        ];
        assert_eq!((got, r.blocks), (n, blocks), "{name}");
        assert!(
            (r.max_area - a[0]).abs() < 1e-4 && (r.max_shape_area - a[1]).abs() < 1e-4,
            "{name}: {} {}",
            r.max_area,
            r.max_shape_area
        );
    }
}

/// The hooks NOH gives the engine for scenes stay inside the plugin profile.
#[test]
fn the_motion_hooks_are_inside_the_plugin_shader_profile() {
    crate::fx::plugin_shader::validate(super::paint::MOTION_HOOKS).unwrap();
}

/// An atlas that uploads nowhere: enough to lay out, rasterise and pack letters.
struct NoGpu;
impl crate::fx::shaders::Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

/// A text the validation never saw (an instant off the grid) is laid out and
/// rasterised on its first frame, whole; a scene whose letters would not fit
/// the glyph atlas is refused when it is prepared.
#[test]
fn a_text_first_seen_after_validation_is_drawn_whole_and_a_full_atlas_is_refused() {
    use super::paint::Painter;
    let mut p = Painter::new(1920, 1080, fonts());
    let scene = load_at(
        "fn frame(t, ctx) { draw(text(\"never validated\").size(0.08).scale(2.0)); }",
        1920,
        1080,
    )
    .unwrap();
    let f = scene.frame(0.0).unwrap();
    let (_, main, stats) = p.paint(&NoGpu, &f.draws, 0.0).unwrap();
    assert_eq!(stats.glyphs, "nevervalidated".len());
    assert_eq!((main.len(), stats.layouts), (stats.glyphs, 1));
    // Large letters at four times their size, in many sizes: past the budget.
    let needs: Vec<(Block, f32)> = (0..60)
        .map(|i| {
            let mut b = Block::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
            b.size = 0.2 + i as f32 * 0.002;
            (b, 4.0)
        })
        .collect();
    let e = Painter::new(1920, 1080, fonts())
        .prepare(&NoGpu, &needs)
        .unwrap_err();
    assert!(e.contains("glyph atlas"), "{e}");
}

/// A frame whose new letters fill the glyph atlas makes it start over: that
/// frame is refused, so that no letter laid before the reset (in the frame or
/// in the painter's cache) is drawn with its old place in the atlas; the next
/// frame lays its letters again, in the fresh atlas.
#[test]
fn a_frame_that_fills_the_glyph_atlas_is_refused_and_the_next_lays_its_letters_again() {
    use super::paint::Painter;
    let mut p = Painter::new(1920, 1080, fonts());
    let kept = load_at(
        "fn frame(t, ctx) { draw(text(\"kept\").size(0.08)); }",
        1920,
        1080,
    )
    .unwrap()
    .frame(0.0)
    .unwrap();
    let mut needs = Vec::new();
    if let super::draw::Draw::Text(td) = &kept.draws[0] {
        needs.push((td.block.clone(), 1.0));
    }
    p.prepare(&NoGpu, &needs).unwrap();
    assert_eq!(p.paint(&NoGpu, &kept.draws, 0.0).unwrap().2.layouts, 0);
    // Letters the size of the frame at four times their size, never prepared:
    // the atlas fills within a few of them.
    let mut refused = None;
    for (k, c) in ('A'..='Z').enumerate() {
        let f = load_at(
            &format!("fn frame(t, ctx) {{ draw(text(\"kept\").size(0.08)); draw(text(\"{c}{c}\").size(1.0).scale(4.0)); }}"),
            1920,
            1080,
        )
        .unwrap()
        .frame(0.0)
        .unwrap();
        if let Err(e) = p.paint(&NoGpu, &f.draws, 0.0) {
            refused = Some((k, e));
            break;
        }
    }
    let (k, e) = refused.expect("the atlas never filled up");
    assert!(e.contains("glyph atlas filled up"), "{e}");
    assert!(
        k > 0,
        "refused at the first large letter: the atlas was not tested full"
    );
    // The prepared text is laid again (not drawn from the stale cache).
    assert_eq!(p.paint(&NoGpu, &kept.draws, 0.0).unwrap().2.layouts, 1);
}

/// Draws a scene's frame through the engine and the compositor.
fn render(src: &str, w: u32, h: u32, canvas: (u32, u32), t: f64) -> Vec<u8> {
    use super::paint::{MOTION_HOOKS, Painter, draw_frame};
    use crate::fx::compose::Compositor;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    let mut gpu =
        GlRenderer::headless(w, h, canvas.0 as f32, canvas.1 as f32, Output::PremulGamma).unwrap();
    gpu.set_hooks(Some(MOTION_HOOKS), [0.0; 16]).unwrap();
    let mut comp = Compositor::new(&gpu).unwrap();
    let mut painter = Painter::new(canvas.0, canvas.1, fonts());
    let scene = load_at(src, canvas.0, canvas.1).unwrap();
    // As a real render does: validate, then prepare every layout at the
    // largest scale it reaches.
    let mut checker = Checker::new(fonts(), canvas.0, canvas.1);
    validate(
        &scene,
        &mut checker,
        &fonts(),
        &grid(0.0, 2.0, 24.0),
        [0.0, 0.0, 1.0, 1.0],
        canvas,
    )
    .unwrap();
    painter.prepare(&gpu, &checker.blocks()).unwrap();
    let f = scene.frame(t).unwrap();
    draw_frame(&mut gpu, &mut painter, &mut comp, &f.draws, &f.layers, t).unwrap();
    comp.read()
}

fn worst_difference(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).fold((0, 0), |(w, n), (x, y)| {
        let d = x.abs_diff(*y);
        (w.max(d), n + usize::from(d > 0))
    })
}

/// The checks of motion scenes that draw through OpenGL. The engine's hidden
/// context belongs to the thread that made it first, and the engine's own GPU
/// checks share this test binary: these run inside `fx::tests::gpu_checks`, on
/// its thread, never as a test of their own.
pub(crate) fn motion_gpu_checks() {
    layers_at_full_opacity_stack_like_one_pass();
    dissolve_is_the_same_at_every_pixel_scale();
    frames_match_the_prototype();
    a_layer_of_text_and_shapes_fades_as_one_picture();
    shapes_past_the_stencil_range_are_all_drawn();
    a_frame_after_a_failed_group_is_whole();
    the_export_overlay_is_the_m2_render();
    shader_layers_draw_or_hide();
    the_paths_frame_the_picture_alike();
    compositor_memory();
    shape_timings();
}

/// The worst legal shape frames, timed as the prototype timed them (at 1916 x
/// 1080, 96 frames from 0 s, the cutting measured around the shape cache, the
/// GPU by a timer query, here around the whole frame drawn again with its
/// shapes already cut: the shapes' pass, its resolve, its stacking and the
/// final encoding). Measured by hand only (plan § 6.2: no time in CI): with
/// NOH_MOTION_BENCH set they are reported, and with NOH_MOTION_BENCH=1, in a
/// release build, the plan's bars (cutting ≤ 1 ms and GPU ≤ 2 ms at p95, on the
/// reference machine) are checked (a debug build's GPU figures include the
/// processor time spent inside the query).
fn shape_timings() {
    use super::paint::{Painter, draw_frame};
    use crate::fx::compose::Compositor;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    use glow::HasContext;
    let Some(bench) = std::env::var_os("NOH_MOTION_BENCH") else {
        eprintln!("NOH_MOTION_BENCH not set: shape timings not measured");
        return;
    };
    let p95 = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[(v.len() * 95).div_ceil(100) - 1], v[v.len() - 1])
    };
    for name in [
        "worst_shapes",
        "worst_hard_fills",
        "worst_strokes",
        "worst_shape_area",
    ] {
        let mut gpu =
            GlRenderer::headless(1916, 1080, 1916.0, 1080.0, Output::PremulGamma).unwrap();
        let gl = gpu.context().clone();
        let mut comp = Compositor::new(&gpu).unwrap();
        let mut painter = Painter::new(1916, 1080, fonts());
        let scene = load_at(&fixture(&format!("{name}.rhai")), 1916, 1080).unwrap();
        let query = unsafe { gl.create_query().unwrap() };
        let (mut cut, mut gpu_ms) = (Vec::new(), Vec::new());
        for i in 0..96 {
            let t = f64::from(i) / 24.0;
            let f = scene.frame(t).unwrap();
            // Once to cut (timed on the processor), once more with the shapes
            // found cut, timed on the GPU: the query then holds little but the
            // GPU's own work.
            let s = draw_frame(&mut gpu, &mut painter, &mut comp, &f.draws, &f.layers, t).unwrap();
            unsafe { gl.finish() };
            unsafe { gl.begin_query(glow::TIME_ELAPSED, query) };
            draw_frame(&mut gpu, &mut painter, &mut comp, &f.draws, &f.layers, t).unwrap();
            unsafe { gl.end_query(glow::TIME_ELAPSED) };
            let ns = unsafe { gl.get_query_parameter_u64(query, glow::QUERY_RESULT) };
            cut.push(s.tess_ms);
            gpu_ms.push(ns as f64 / 1e6);
        }
        unsafe { gl.delete_query(query) };
        let ((cut95, cut_max), (gpu95, gpu_max)) = (p95(&mut cut), p95(&mut gpu_ms));
        eprintln!(
            "{name}: cutting p95 {cut95:.3} ms (worst {cut_max:.3}), GPU p95 {gpu95:.3} ms (worst {gpu_max:.3})"
        );
        if bench == "1" {
            if cfg!(debug_assertions) {
                panic!("NOH_MOTION_BENCH needs a release build");
            }
            assert!(cut95 <= 1.0, "{name}: cutting p95 {cut95:.3} ms over 1 ms");
            assert!(gpu95 <= 2.0, "{name}: GPU p95 {gpu95:.3} ms over 2 ms");
        }
    }
}

/// The export's overlay (`layer::MotionLayer`: the scene on its script thread,
/// validated, its letters prepared from that validation) is byte for byte the
/// frame `render` draws, the M2 drawing that the prototype comparison holds.
fn the_export_overlay_is_the_m2_render() {
    use super::layer::MotionLayer;
    use super::thread::ScriptThread;
    use crate::fx::export::Overlay;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    for (name, t) in [
        ("layers.rhai", 1.0),
        ("text_passes.rhai", 1.0),
        ("shapes.rhai", 1.5),
    ] {
        let src = with_header(&fixture(name));
        let thread =
            ScriptThread::start(src.clone(), inputs(1916, 1080), Audio::silent(), false).unwrap();
        thread.validate(grid(0.0, 2.0, 24.0)).unwrap();
        let mut gpu =
            GlRenderer::headless(1916, 1080, 1916.0, 1080.0, Output::PremulGamma).unwrap();
        let mut layer = MotionLayer::new(thread, (1916, 1080), 0.0, Vec::new()).unwrap();
        layer.setup(&mut gpu).unwrap();
        layer.draw(&mut gpu, t).unwrap();
        let ours = layer.read();
        let m2 = render(&src, 1916, 1080, (1916, 1080), t);
        let (worst, differing) = worst_difference(&ours, &m2);
        eprintln!(
            "{name} at {t}: export overlay vs M2 render, worst {worst}/255, {differing} bytes"
        );
        assert_eq!(differing, 0, "{name} at {t}");
    }
}

/// The same framing in every path (« the same framing, not the same pixels »):
/// the export resamples the source's planes on the GPU (`YuvMix`), the libmpv
/// preview draws its picture through the framing's quad (`QuadPainter`), a look
/// resamples its frame on the CPU (`reframe::resample`); all from one framing.
/// Without a framing the export's bytes are unchanged, and the identity framing
/// gives them back exactly.
fn the_paths_frame_the_picture_alike() {
    use super::reframe::{Framing, VideoDraw, resample};
    use crate::fx::compose::Compositor;
    use crate::fx::gl::{GlRenderer, Matrix, Reframe, YuvMix};
    use crate::fx::quad::QuadPainter;
    use crate::fx::shaders::Output;
    use glow::HasContext;
    let (w, h) = (640usize, 360usize);
    // A picture with structure everywhere: gradients and a checker.
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let check = if (x / 16 + y / 16) % 2 == 0 { 40 } else { 0 };
            [
                (x * 200 / w + check) as u8,
                (y * 200 / h + check) as u8,
                ((x + y) * 120 / (w + h) + 60) as u8,
            ]
        })
        .collect();
    // Its planes (BT.601, limited range), chroma averaged over 2 x 2.
    let luma = |p: &[u8]| {
        (16.0
            + (219.0 / 255.0)
                * (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])))
        .round() as u8
    };
    let y: Vec<u8> = rgb.chunks(3).map(luma).collect();
    let (cw, ch) = (w / 2, h / 2);
    let mut u = Vec::with_capacity(cw * ch);
    let mut v = Vec::with_capacity(cw * ch);
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = &rgb[((cy * 2 + dy) * w + cx * 2 + dx) * 3..][..3];
                (r, g, b) = (
                    r + f32::from(p[0]) / 4.0,
                    g + f32::from(p[1]) / 4.0,
                    b + f32::from(p[2]) / 4.0,
                );
            }
            let yp = 0.299 * r + 0.587 * g + 0.114 * b;
            u.push((128.0 + (224.0 / 255.0) * (b - yp) / 1.772).round() as u8);
            v.push((128.0 + (224.0 / 255.0) * (r - yp) / 1.402).round() as u8);
        }
    }
    let i420: Vec<u8> = [y.clone(), u.clone(), v.clone()].concat();
    let video = VideoDraw {
        scale: 1.3,
        at: [0.45, 0.55],
        focus: [0.6, 0.4],
        rot: 0.2,
        crop: [0.05, 0.0, 0.95, 1.0],
    };
    let f = video.framing((w as f32, h as f32), (w as f32, h as f32));
    let reframe = |f: &Framing| Reframe {
        inv: f.inverse(),
        center: f.center,
        focus: f.focus,
        crop: f.crop,
    };
    let gpu =
        GlRenderer::headless(w as u32, h as u32, w as f32, h as f32, Output::PremulGamma).unwrap();
    let mut comp = Compositor::new(&gpu).unwrap();
    comp.begin();
    comp.finish();
    let clear = comp.texture();
    let mut mix = YuvMix::new(&gpu, Matrix::Bt601).unwrap();
    let mut run = |r: Option<Reframe>| {
        mix.set_reframe(r);
        mix.submit(&i420, clear, 0).unwrap();
        let mut out = vec![0u8; i420.len()];
        mix.fetch(0, &mut out).unwrap();
        out
    };
    // No framing, and the identity framing: the bytes as they came.
    let plain = run(None);
    assert_eq!(plain, i420, "a transparent overlay changed the picture");
    assert_eq!(
        run(Some(reframe(&Framing::identity((w as f32, h as f32))))),
        i420
    );
    // The export's planes against the CPU's, through the same framing.
    let framed = run(Some(reframe(&f)));
    let want_y = resample(&y, 1, &f, &[16]);
    let half = f.for_plane(2.0);
    let want_u = resample(&u, 1, &half, &[128]);
    let want_v = resample(&v, 1, &half, &[128]);
    let worst = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
    let (wy, wu, wv) = (
        worst(&framed[..w * h], &want_y),
        worst(&framed[w * h..w * h + cw * ch], &want_u),
        worst(&framed[w * h + cw * ch..], &want_v),
    );
    eprintln!("reframed planes, GPU vs CPU: Y {wy}/255, U {wu}/255, V {wv}/255");
    assert!(wy <= 2 && wu <= 2 && wv <= 2, "Y {wy} U {wu} V {wv}");
    // The libmpv preview's path: its picture through the framing's quad.
    let gl = gpu.context().clone();
    let painter = QuadPainter::new(&gl).unwrap();
    let (src, dst, fbo) = unsafe {
        let rgba: Vec<u8> = rgb
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        let make = |data: Option<&[u8]>| {
            let t = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(data),
            );
            t
        };
        let src = make(Some(&rgba));
        let dst = make(None);
        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(dst),
            0,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (src, dst, fbo)
    };
    painter
        .draw(Some(fbo), (w as i32, h as i32), src, f.quad())
        .unwrap();
    let mut shown = vec![0u8; w * h * 4];
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
        gl.read_pixels(
            0,
            0,
            w as i32,
            h as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut shown)),
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo);
        gl.delete_texture(src);
        gl.delete_texture(dst);
    }
    let want = resample(&rgb, 3, &f, &[0, 0, 0]);
    // Away from the quad's edges, where rasterising and the crop test may
    // decide a pixel differently.
    let mut worst_rgb = 0u8;
    for py in 0..h {
        for px in 0..w {
            let edge = [-1.5f32, 0.0, 1.5].iter().any(|d| {
                [[*d, 0.0], [0.0, *d]].iter().any(|o| {
                    let s = f.to_source([px as f32 + 0.5 + o[0], py as f32 + 0.5 + o[1]]);
                    let c = f.crop;
                    s[0] < c[0] || s[1] < c[1] || s[0] > c[2] || s[1] > c[3]
                })
            });
            if edge {
                continue;
            }
            for c in 0..3 {
                worst_rgb = worst_rgb
                    .max(shown[(py * w + px) * 4 + c].abs_diff(want[(py * w + px) * 3 + c]));
            }
        }
    }
    eprintln!("reframed preview quad vs CPU: worst {worst_rgb}/255 away from its edges");
    assert!(worst_rgb <= 2, "{worst_rgb}");
}

/// A fixture scene with the shaders beside it, through the export's path (the
/// script thread, its validation, `MotionLayer` with the shaders compiled on
/// the engine's context): the overlay at `t` and what the frame drew.
fn render_scene(name: &str, t: f64, show: bool) -> (Vec<u8>, super::paint::PaintStats) {
    use super::layer::MotionLayer;
    use super::thread::ScriptThread;
    use crate::fx::export::Overlay;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/motion")
        .join(format!("{name}.rhai"));
    let shaders = super::shader::read_beside(&path).unwrap();
    let mut inp = inputs(1916, 1080);
    inp.shaders = shaders.iter().map(|(n, _)| n.clone()).collect();
    let thread = ScriptThread::start(
        fixture(&format!("{name}.rhai")),
        inp,
        Audio::silent(),
        false,
    )
    .unwrap();
    thread.validate(grid(0.0, 2.0, 24.0)).unwrap();
    let mut gpu = GlRenderer::headless(1916, 1080, 1916.0, 1080.0, Output::PremulGamma).unwrap();
    let mut layer = MotionLayer::new(thread, (1916, 1080), 0.0, shaders).unwrap();
    layer.show_shaders(show);
    layer.setup(&mut gpu).unwrap();
    layer.draw(&mut gpu, t).unwrap();
    (layer.read(), layer.stats)
}

/// Layer shaders: drawn, they match the prototype's images (when its dumps are
/// given) and the worst legal frames are timed on request; hidden (the
/// preview's default), their layers leave nothing and are counted. Compared:
/// `glow` at 1 s (its background shader reads the sound's energy, silent here
/// and the song's in the prototype's dump: at 2 s they part, so it is left out)
/// and the worst legal shader frames, which read their content 12 times: the
/// prototype read that content through 8 bits, NOH in half floats, up to 12
/// levels apart. Shown once by hand: reading NOH's engine passes through 8 bits
/// as the prototype did brings both to 0 bytes apart.
fn shader_layers_draw_or_hide() {
    // Half-transparent red everywhere: premultiplied, display-encoded.
    let (fill, stats) = render_scene("fill", 0.5, true);
    assert_eq!(stats.shader_layers, 1);
    let mid = ((540 * 1916 + 958) * 4) as usize;
    eprintln!(
        "fill: {:?} at the middle, {:?} at the corner",
        &fill[mid..mid + 4],
        &fill[fill.len() - 4..]
    );
    for at in [0, mid, fill.len() - 4] {
        let p = &fill[at..at + 4];
        assert!(
            p[3].abs_diff(128) <= 1 && p[1] == 0 && p[2] == 0,
            "{p:?} at byte {at}"
        );
    }
    let (shown, stats) = render_scene("glow", 1.0, true);
    assert_eq!(stats.shader_layers, 2, "{stats:?}");
    assert!(shown.iter().skip(3).step_by(4).any(|&a| a > 0));
    let (hidden, stats) = render_scene("glow", 1.0, false);
    assert_eq!(
        (stats.shader_layers, stats.hidden_shader_layers),
        (0, 2),
        "{stats:?}"
    );
    assert!(
        hidden.iter().all(|&b| b == 0),
        "a hidden shader layer drew something"
    );
    if let Some(dir) = std::env::var_os("NOH_MOTION_PROTOTYPE_REF").map(std::path::PathBuf::from) {
        for (name, t, most) in [
            ("glow", 1.0, 2),
            ("worst_shader_both", 0.5, 12),
            ("worst_shader_random", 0.5, 12),
        ] {
            let (ours, _) = render_scene(name, t, true);
            let theirs = std::fs::read(dir.join(format!("{name}-{t:.2}-new.rgba"))).unwrap();
            let (worst, differing) = worst_difference(&ours, &theirs);
            let above2 = ours
                .iter()
                .zip(&theirs)
                .filter(|(a, b)| a.abs_diff(**b) > 2)
                .count();
            eprintln!(
                "{name} at {t:.2} (shaders): worst {worst}/255, {differing} bytes differ, {above2} by more than 2"
            );
            assert!(worst <= most, "{name} at {t}: off by {worst}/255");
        }
    }
    if std::env::var_os("NOH_MOTION_BENCH").is_some() {
        reframe_timing();
        for name in [
            "worst_shader_scalar",
            "worst_shader_vector",
            "worst_shader_random",
            "worst_shader_both",
        ] {
            shader_timing(name);
        }
    }
}

/// GPU time of the export's mix of a 1920 x 1080 frame, without and with a
/// reframing (between two GPU timestamps around the submission, 48 frames
/// each): what reframing adds to an export.
fn reframe_timing() {
    use super::reframe::VideoDraw;
    use crate::fx::compose::Compositor;
    use crate::fx::gl::{GlRenderer, Matrix, Reframe, YuvMix};
    use crate::fx::shaders::Output;
    use glow::HasContext;
    let (w, h) = (1920usize, 1080usize);
    let gpu =
        GlRenderer::headless(w as u32, h as u32, w as f32, h as f32, Output::PremulGamma).unwrap();
    let gl = gpu.context().clone();
    let mut comp = Compositor::new(&gpu).unwrap();
    comp.begin();
    comp.finish();
    let clear = comp.texture();
    let mut mix = YuvMix::new(&gpu, Matrix::Bt709).unwrap();
    let i420: Vec<u8> = (0..mix.frame_bytes())
        .map(|i| (i * 7 % 251) as u8)
        .collect();
    let f = VideoDraw {
        scale: 1.4,
        rot: 0.2,
        ..VideoDraw::default()
    }
    .framing((w as f32, h as f32), (w as f32, h as f32));
    let r = Reframe {
        inv: f.inverse(),
        center: f.center,
        focus: f.focus,
        crop: f.crop,
    };
    let (q0, q1) = unsafe { (gl.create_query().unwrap(), gl.create_query().unwrap()) };
    let mut out = vec![0u8; i420.len()];
    for (label, reframe) in [("without", None), ("with", Some(r))] {
        mix.set_reframe(reframe);
        let mut ms = Vec::new();
        for _ in 0..48 {
            unsafe { gl.finish() };
            unsafe { gl.query_counter(q0, glow::TIMESTAMP) };
            mix.submit(&i420, clear, 0).unwrap();
            unsafe { gl.query_counter(q1, glow::TIMESTAMP) };
            mix.fetch(0, &mut out).unwrap();
            let (a, b) = unsafe {
                (
                    gl.get_query_parameter_u64(q0, glow::QUERY_RESULT),
                    gl.get_query_parameter_u64(q1, glow::QUERY_RESULT),
                )
            };
            ms.push(b.saturating_sub(a) as f64 / 1e6);
        }
        ms.sort_by(f64::total_cmp);
        eprintln!(
            "mix of a 1920 x 1080 frame {label} reframing: GPU p95 {:.3} ms",
            ms[(ms.len() * 95).div_ceil(100) - 1]
        );
    }
    unsafe {
        gl.delete_query(q0);
        gl.delete_query(q1);
    }
}

/// GPU time of a worst legal shader frame, between two GPU timestamps around a
/// frame drawn again (its programs compiled, its letters laid), 48 frames. Not
/// a TIME_ELAPSED query: the engine runs one of its own inside each frame, and
/// two cannot be open at once.
fn shader_timing(name: &str) {
    use super::layer::MotionLayer;
    use super::thread::ScriptThread;
    use crate::fx::export::Overlay;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    use glow::HasContext;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/motion")
        .join(format!("{name}.rhai"));
    let shaders = super::shader::read_beside(&path).unwrap();
    let mut inp = inputs(1916, 1080);
    inp.shaders = shaders.iter().map(|(n, _)| n.clone()).collect();
    let thread = ScriptThread::start(
        fixture(&format!("{name}.rhai")),
        inp,
        Audio::silent(),
        false,
    )
    .unwrap();
    thread.validate(grid(0.0, 2.0, 24.0)).unwrap();
    let mut gpu = GlRenderer::headless(1916, 1080, 1916.0, 1080.0, Output::PremulGamma).unwrap();
    let gl = gpu.context().clone();
    let mut layer = MotionLayer::new(thread, (1916, 1080), 0.0, shaders).unwrap();
    layer.setup(&mut gpu).unwrap();
    let (q0, q1) = unsafe { (gl.create_query().unwrap(), gl.create_query().unwrap()) };
    let mut ms = Vec::new();
    for i in 0..48 {
        let t = f64::from(i) / 24.0;
        layer.draw(&mut gpu, t).unwrap();
        unsafe { gl.finish() };
        unsafe { gl.query_counter(q0, glow::TIMESTAMP) };
        layer.draw(&mut gpu, t).unwrap();
        unsafe { gl.query_counter(q1, glow::TIMESTAMP) };
        let (a, b) = unsafe {
            (
                gl.get_query_parameter_u64(q0, glow::QUERY_RESULT),
                gl.get_query_parameter_u64(q1, glow::QUERY_RESULT),
            )
        };
        ms.push(b.saturating_sub(a) as f64 / 1e6);
    }
    unsafe {
        gl.delete_query(q0);
        gl.delete_query(q1);
    }
    ms.sort_by(f64::total_cmp);
    eprintln!(
        "{name}: whole frame on the GPU p95 {:.3} ms, worst {:.3} ms",
        ms[(ms.len() * 95).div_ceil(100) - 1],
        ms[ms.len() - 1]
    );
}

/// A frame that failed inside a group (a pass refused) leaves the group open:
/// the next frame still stacks on its own stack, not on the group's.
fn a_frame_after_a_failed_group_is_whole() {
    use crate::fx::compose::{Compositor, Fill, Shape};
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    let gpu = GlRenderer::headless(64, 64, 64.0, 64.0, Output::PremulGamma).unwrap();
    let mut comp = Compositor::new(&gpu).unwrap();
    let tri = [[0.0, 0.0], [64.0, 0.0], [0.0, 64.0]];
    let white = [Shape {
        fill: &tri,
        paint: Fill::Solid([1.0; 4]),
        stroke: &[],
        stroke_color: [1.0; 4],
        opacity: 1.0,
    }];
    comp.begin();
    comp.begin_group().unwrap();
    comp.draw_shapes(&white, (64.0, 64.0), 1.0).unwrap();
    // The frame fails here: no end_group, no finish.
    comp.begin();
    comp.draw_shapes(&white, (64.0, 64.0), 1.0).unwrap();
    comp.finish();
    let px = comp.read();
    assert_eq!(
        &px[(8 * 64 + 8) * 4..][..4],
        &[255; 4],
        "the next frame was lost"
    );
}

/// The shapes' pass stamps each primitive in an 8-bit stencil, and GL clamps a
/// larger stamp to 255: past 255 it must start over, or a primitive past the
/// 255th would not paint over another one past it (a frame may hold 256 paths,
/// each a fill and a stroke). The last of 300 shapes lies on the one before it.
fn shapes_past_the_stencil_range_are_all_drawn() {
    use crate::fx::compose::{Compositor, Fill, Shape};
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    let gpu = GlRenderer::headless(256, 256, 256.0, 256.0, Output::PremulGamma).unwrap();
    let mut comp = Compositor::new(&gpu).unwrap();
    // Squares of 8 pixels on a grid of 12, one shape each; the 300th on the 299th.
    let cell = |i: usize| ((i % 20) * 12, (i / 20) * 12);
    let squares: Vec<[[f32; 2]; 6]> = (0..300)
        .map(|i| {
            let (x, y) = cell(i.min(298));
            let (x, y) = (x as f32, y as f32);
            let (x1, y1) = (x + 8.0, y + 8.0);
            [[x, y], [x1, y], [x, y1], [x1, y], [x1, y1], [x, y1]]
        })
        .collect();
    let shapes: Vec<Shape> = squares
        .iter()
        .enumerate()
        .map(|(i, q)| Shape {
            fill: q,
            paint: Fill::Solid(match i {
                298 => [1.0, 0.0, 0.0, 1.0],
                299 => [0.0, 1.0, 0.0, 1.0],
                _ => [1.0; 4],
            }),
            stroke: &[],
            stroke_color: [1.0; 4],
            opacity: 1.0,
        })
        .collect();
    comp.begin();
    comp.draw_shapes(&shapes, (256.0, 256.0), 1.0).unwrap();
    comp.finish();
    let px = comp.read();
    let at = |i: usize| {
        let (x, y) = cell(i);
        let o = ((y + 4) * 256 + x + 4) * 4;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    };
    let missing = (0..298).filter(|&i| at(i) != [255; 4]).count();
    eprintln!(
        "300 shapes in one pass: {missing} of the first 298 not drawn; the 300th over the 299th: {:?}",
        at(298)
    );
    assert_eq!(missing, 0);
    assert_eq!(
        at(298),
        [0, 255, 0, 255],
        "the 300th shape did not paint over the 299th"
    );
}

/// What the compositor holds on the GPU: its two targets, as counted; and, where
/// the driver tells (NVIDIA's memory info), what the driver saw taken.
fn compositor_memory() {
    use crate::fx::compose::Compositor;
    use crate::fx::gl::GlRenderer;
    use crate::fx::shaders::Output;
    use glow::HasContext;
    const AVAILABLE_KB: u32 = 0x9049; // GL_GPU_MEMORY_INFO_CURRENT_AVAILABLE_VIDMEM_NVX
    let gpu = GlRenderer::headless(1920, 1080, 1920.0, 1080.0, Output::PremulGamma).unwrap();
    let gl = gpu.context().clone();
    let free = || unsafe {
        let v = gl.get_parameter_i32(AVAILABLE_KB);
        (gl.get_error() == glow::NO_ERROR).then_some(i64::from(v))
    };
    let before = free();
    let mut comp = Compositor::new(&gpu).unwrap();
    unsafe { gl.finish() };
    let after = free();
    let px = 1920 * 1080;
    assert_eq!(comp.bytes(), px * 12);
    let report = |what: &str, bytes: usize, taken: Option<i64>| match taken {
        Some(kb) => eprintln!(
            "{what} at 1920 x 1080: {} MiB counted, {} MiB taken as the driver reports",
            bytes / (1024 * 1024),
            kb / 1024
        ),
        None => eprintln!(
            "{what} at 1920 x 1080: {} MiB counted (the driver gives no memory info)",
            bytes / (1024 * 1024)
        ),
    };
    let taken = |b: Option<i64>, a: Option<i64>| b.zip(a).map(|(b, a)| b - a);
    report("compositor", comp.bytes(), taken(before, after));
    // A group and a shapes' pass are made at their first use.
    let before = free();
    comp.begin();
    comp.begin_group().unwrap();
    let tri = [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]];
    comp.draw_shapes(
        &[crate::fx::compose::Shape {
            fill: &tri,
            paint: crate::fx::compose::Fill::Solid([1.0; 4]),
            stroke: &[],
            stroke_color: [1.0; 4],
            opacity: 1.0,
        }],
        (1920.0, 1080.0),
        1.0,
    )
    .unwrap();
    comp.end_group(1.0);
    comp.finish();
    unsafe { gl.finish() };
    let after = free();
    assert_eq!(comp.bytes(), px * (12 + 8 + 56));
    report(
        "group and shapes' pass",
        comp.bytes() - px * 12,
        taken(before, after),
    );
}

/// A layer that holds both text and shapes is stacked as one group: faded, it
/// is the same picture at a lower opacity (premultiplied, every byte scaled),
/// where its overlapping draws do not show through each other.
fn a_layer_of_text_and_shapes_fades_as_one_picture() {
    let draws = "let l = layer();\n\
         l.add(rect(0.5, 0.3).at(0.5, 0.5).color(\"#101828\"));\n\
         l.add(circle(0.5, 0.5, 0.2).fill(\"#C8102E\"));\n\
         l.add(text(\"OVER\").size(0.12).at(0.5, 0.5).color(\"#F2E8D8\"));\n";
    let scene = |opacity: f32| {
        format!("fn frame(t, ctx) {{\n{draws}l.opacity({opacity:.1});\ndraw(l);\n}}\n")
    };
    let full = render(&scene(1.0), 1916, 1080, (1916, 1080), 1.0);
    let half = render(&scene(0.5), 1916, 1080, (1916, 1080), 1.0);
    let mut worst = 0u8;
    for (f, h) in full.iter().zip(&half) {
        let want = (*f as f32 * 0.5).round() as u8;
        worst = worst.max(want.abs_diff(*h));
    }
    eprintln!(
        "a layer of text and shapes at half opacity: worst {worst}/255 from the full picture halved"
    );
    assert!(worst <= 1, "the faded layer differs by {worst}/255");
}

/// Layers at full opacity stack like one pass: each pass is read from the
/// engine's half-float layer, so cutting a scene into layers costs at most one
/// level of 255 (the prototype, reading 8-bit passes, was off by 3).
fn layers_at_full_opacity_stack_like_one_pass() {
    // Past 2 s every layer is at full opacity; the flat scene draws the same
    // draws, in the same order, with no layer.
    let layered = render(&fixture("layers.rhai"), 1916, 1080, (1916, 1080), 2.5);
    let flat = render(
        "fn frame(t, ctx) {\n\
         draw(rect(0.7, 0.22).at(0.5, 0.5).color(\"#101828\").opacity(0.8));\n\
         draw(rect(0.7, 0.012).at(0.5, 0.39).color(\"#C8102E\"));\n\
         draw(text(\"FIRST\").size(0.14).at(0.46, 0.5).color(\"#F2E8D8\"));\n\
         draw(text(\"LIGHT\").size(0.14).at(0.54, 0.52).color(\"#C8102E\"));\n\
         draw(text(\"three layers\").size(0.03).at(0.5, 0.66).opacity(0.7)); }",
        1916,
        1080,
        (1916, 1080),
        2.5,
    );
    let (worst, differing) = worst_difference(&layered, &flat);
    eprintln!("layers vs one pass: worst {worst}/255, {differing} bytes differ");
    assert!(worst <= 1, "worst {worst}/255 on {differing} bytes");
}

/// Mean alpha of each block of `n` x `n` pixels.
fn blocks(px: &[u8], w: usize, h: usize, n: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for by in 0..h / n {
        for bx in 0..w / n {
            let mut s = 0u64;
            for y in by * n..(by + 1) * n {
                for x in bx * n..(bx + 1) * n {
                    s += u64::from(px[(y * w + x) * 4 + 3]);
                }
            }
            out.push(s as f64 / (n * n) as f64);
        }
    }
    out
}

/// A dissolving title dissolves alike at two pixel scales of the same canvas
/// (the preview draws the export's canvas at the monitor's size): its noise
/// comes from where the pixel is on the canvas, not from the glyph atlas.
/// Compared block by block (16 canvas pixels, a few grains of the noise): a
/// noise laid differently would leave other blocks dissolved; a 0.05 change of
/// the dissolve is the scale of difference the check must stay under.
fn dissolve_is_the_same_at_every_pixel_scale() {
    let at = |d: f64| {
        format!(
            "fn frame(t, ctx) {{ draw(text(\"DISSOLVE\").size(0.3).dissolve({d:.2}).color(\"#FFFFFF\")); }}"
        )
    };
    let big = blocks(
        &render(&at(0.45), 1280, 720, (1280, 720), 0.0),
        1280,
        720,
        16,
    );
    let small = blocks(&render(&at(0.45), 640, 360, (1280, 720), 0.0), 640, 360, 8);
    let shifted = blocks(
        &render(&at(0.50), 1280, 720, (1280, 720), 0.0),
        1280,
        720,
        16,
    );
    let mean = |a: &[f64], b: &[f64]| {
        let (s, n) = a
            .iter()
            .zip(b)
            .filter(|(x, y)| **x > 0.0 || **y > 0.0)
            .fold((0.0, 0), |(s, n), (x, y)| (s + (x - y).abs(), n + 1));
        s / f64::from(n.max(1))
    };
    let scales = mean(&big, &small);
    let step = mean(&big, &shifted);
    eprintln!(
        "dissolve: two pixel scales differ by {scales:.2}/255 per block; dissolve 0.45 vs 0.50 by {step:.2}/255"
    );
    assert!(scales < step / 2.0, "scales {scales}, step {step}");
}

/// What NOH draws matches what the prototype drew, scene by scene, at the
/// prototype's size (its `--compare … --dump` images; their folder is given by
/// NOH_MOTION_PROTOTYPE_REF, a local check: the images are not in the
/// repository). Dissolving text is left out: its noise now comes from the
/// canvas, not from the atlas, so it is a different noise by design. With
/// layers at full opacity the reference is the prototype's
/// one-pass image (`-old`): its layered one (`-new`) carries the 8-bit error of
/// its passes (its own log: 11 bytes off by 3 at 2.5 s), which NOH no longer has.
/// Shapes alone match byte for byte. Where the text of `shapes` lies over its
/// gradient band, the prototype's layered image is the only reference (its
/// one-pass image has no shapes) and it carries the 8-bit read of its text
/// pass: up to 5 levels there, every such byte between rows 866 and 903, the
/// text's line. Shown once by hand: reading NOH's engine pass through 8 bits as
/// the prototype did (encode, round to 1/255, decode) brings both instants to
/// 1/255; clamping the colour without the rounding leaves them at 4 and 5.
fn frames_match_the_prototype() {
    let Some(dir) = std::env::var_os("NOH_MOTION_PROTOTYPE_REF").map(std::path::PathBuf::from)
    else {
        eprintln!("NOH_MOTION_PROTOTYPE_REF not set: nothing compared with the prototype");
        return;
    };
    for (name, t, which, most) in [
        ("layers", 0.5, "new", 2),
        ("layers", 1.0, "new", 2),
        ("layers", 2.5, "old", 2),
        ("worst_particles", 5.0, "new", 2),
        ("worst_particles", 6.0, "new", 2),
        ("text_passes", 0.5, "new", 2),
        ("text_passes", 1.0, "new", 2),
        ("text_passes", 2.5, "new", 2),
        ("shapes", 0.5, "new", 2),
        ("shapes", 1.5, "new", 5),
        ("shapes", 3.0, "new", 5),
        ("worst_hard_fills", 5.0, "new", 2),
        ("worst_strokes", 5.0, "new", 2),
        ("worst_shape_area", 5.0, "new", 2),
    ] {
        let ours = render(
            &fixture(&format!("{name}.rhai")),
            1916,
            1080,
            (1916, 1080),
            t,
        );
        let theirs = std::fs::read(dir.join(format!("{name}-{t:.2}-{which}.rgba"))).unwrap();
        let (worst, differing) = worst_difference(&ours, &theirs);
        let above2 = ours
            .iter()
            .zip(&theirs)
            .filter(|(a, b)| a.abs_diff(**b) > 2)
            .count();
        eprintln!(
            "{name} at {t:.2} ({which}): worst {worst}/255, {differing} bytes differ, {above2} by more than 2"
        );
        assert!(
            worst <= most,
            "{name} at {t}: off by {worst}/255, more than {most} ({above2} bytes by more than 2)"
        );
    }
}
