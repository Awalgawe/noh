//! The scene language, its counted limits, the checks of every frame and the
//! script thread. Every bound of the sandbox has a hostile script here.
use super::audio::Audio;
use super::check::Checker;
use super::draw::{Block, MAX_AREA, MAX_BLURS, MAX_GLYPHS, MAX_PARTICLES, MAX_PASSES, measure};
use super::script::{MAX_CALLS, MAX_OPS, ScriptScene};
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
    let expected = [
        "abs",
        "at",
        "clampf",
        "color",
        "cos",
        "cue",
        "cue_at",
        "cue_count",
        "dissolve",
        "draw",
        "during",
        "ease",
        "edge",
        "energy",
        "exp",
        "face",
        "floor",
        "glow",
        "len",
        "life",
        "line_height",
        "lower",
        "max",
        "min",
        "mixf",
        "onset",
        "onset_age",
        "onset_count",
        "opacity",
        "outline",
        "particles",
        "particles_in",
        "pi",
        "pow",
        "range",
        "rate",
        "rect",
        "rise",
        "rot",
        "round",
        "scale",
        "seed",
        "shadow",
        "shape",
        "sin",
        "size",
        "smooth",
        "spin",
        "sqrt",
        "swirl",
        "text",
        "to_float",
        "to_int",
        "upper",
        "width",
        "wind",
        "word",
        "word_color",
        "word_dissolve",
        // layout, measuring, small tables
        "align",
        "anchor",
        "gone_by",
        "measure",
        "pick",
        "tracking",
        "word_box",
        // layers; their shaders and the picture answer « not in NOH yet »
        "add",
        "layer",
        "shader",
        "resolution",
        "param",
        "video",
        // shapes
        "path",
        "move_to",
        "line_to",
        "quad_to",
        "cubic_to",
        "close",
        "circle",
        "ellipse",
        "rounded_rect",
        "line",
        "fill",
        "linear",
        "radial",
        "stop",
        "stroke",
        "caps",
        "trim",
        "pivot",
        "offset",
        // property reads of the context, a cue, a word and a measured box
        "get$duration",
        "get$end",
        "get$h",
        "get$i",
        "get$start",
        "get$text",
        "get$w",
        "get$words",
        "get$safe_x0",
        "get$safe_x1",
        "get$safe_y0",
        "get$safe_y1",
        "get$src_w",
        "get$src_h",
        "get$x0",
        "get$x1",
        "get$y0",
        "get$y1",
        // the range iterators (`for i in 0..n`), the one piece of the library kept
        "bits",
        "chars",
        "contains",
        "end",
        "get$bits",
        "get$chars",
        "get$is_empty",
        "get$is_exclusive",
        "get$is_inclusive",
        "is_empty",
        "is_exclusive",
        "is_inclusive",
        "start",
    ];
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

/// Reframing and layer shaders are later versions: a scene using them is
/// refused with that reason, not with « function not found ».
#[test]
fn what_noh_does_not_have_yet_is_refused_with_the_reason() {
    refused(
        "fn frame(t, ctx) { draw(video().scale(1.2)); }",
        "video() is not in NOH yet",
    );
    refused(
        "fn frame(t, ctx) { let l = layer().shader(\"glow\"); draw(l); }",
        "layer shaders are not in NOH yet",
    );
    refused(
        "fn frame(t, ctx) { draw(layer().param(0, 1.0)); }",
        "layer shaders are not in NOH yet",
    );
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
