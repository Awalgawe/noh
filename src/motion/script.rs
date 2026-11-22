//! A scene is a sandboxed script. `frame(t, ctx)` is a pure function of `t`
//! that describes each draw with typed builders (`text("…").size(0.1)`) and
//! hands it to the host with `draw(…)`; it runs once per frame under counted
//! limits, with no file, no network, no clock, no randomness, no module loading.
//! The engine and every value it makes stay on the thread that loaded the scene
//! (`super::thread`); a frame leaves it as host data.
//!
//! The engine starts empty (`Engine::new_raw`) and is given an allow-list: the
//! functions registered below are all a script can call, and a test asserts the
//! list. Rhai's standard library is not loaded: twice a hole came from it (a
//! clock; a map growing past its size cap by index assignment, when a copied
//! value costs one operation whatever its size).
//!
//! So a script can build no large value: no arrays, no object maps, no blobs,
//! no closures; numbers, strings under a small cap, and the host's fixed-size
//! draw values.
use super::Inputs;
use super::audio::Audio;
use super::draw::{
    BoxF, Draw, EmitterDraw, MAX_DRAWS, Outline, RectDraw, Shadow, Source, Span, TextDraw, WordMod,
};
use crate::fx::text::Fonts;
use rhai::{AST, Dynamic, Engine, EvalAltResult, Scope};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Operations one frame may spend (Rhai counts roughly one per expression node).
/// Set with the size caps below so that the heaviest legal script stays within a
/// stated time on the reference machine (4 ms at worst, measured on the
/// prototype). A real scene of the first trial spent about 1 400.
pub const MAX_OPS: u64 = 6_000;
/// Size caps of any one value a script builds, counted with what it contains.
pub const MAX_STRING: usize = 1024;
/// Calls one frame may make to the costly functions offered to scripts (a native
/// call costs one operation whatever it does, so the operation cap cannot bound it).
pub const MAX_CALLS: u32 = 64;
/// Reads one frame may make of the lyrics and onsets.
pub const MAX_READS: u32 = 1024;
/// Words of one text that may carry their own settings, and colours of an emitter.
pub const MAX_WORDS: usize = 32;
pub const MAX_COLORS: usize = 8;
/// Largest scene source.
pub const MAX_SOURCE: usize = 32 * 1024;
/// The scene format this NOH reads. A scene names its format on its first line
/// (`// noh-motion 0`), so that a later NOH can refuse an older scene with that
/// reason instead of an error about a function or a type. Experimental: the
/// format may change, and its number with it.
pub const FORMAT: u32 = 0;
const HEADER: &str = "// noh-motion ";
/// Call levels and expression depths: each costs stack on the script thread,
/// whose size is fixed for that reason (`super::thread`).
pub const MAX_CALL_LEVELS: usize = 24;
pub const MAX_EXPR_DEPTH: usize = 64;
pub const MAX_FN_EXPR_DEPTH: usize = 32;

type Res<T> = Result<T, Box<EvalAltResult>>;

#[derive(Clone)]
struct Ctx {
    w: f64,
    h: f64,
    /// The source picture's size in pixels.
    src_w: f64,
    src_h: f64,
    duration: f64,
    /// The surface text should stay in (the platform's safe area), fractions.
    safe: [f64; 4],
}

#[derive(Clone)]
struct CueV {
    i: i64,
    start: f64,
    end: f64,
    text: String,
    words: i64,
}

#[derive(Clone)]
struct WordV {
    text: String,
    start: f64,
    end: f64,
}

pub struct ScriptScene {
    engine: Engine,
    ast: AST,
    ctx: Ctx,
    ops: Rc<Cell<u64>>,
    calls: Rc<Cell<u32>>,
    reads: Rc<Cell<u32>>,
    /// The draws of the frame being evaluated, filled by `draw()`.
    out: Rc<RefCell<Vec<Draw>>>,
    /// Which runs of them are layers.
    layers: Rc<RefCell<Vec<Span>>>,
}

/// A path as the script holds it: a handle, like a layer, so that building a
/// long path does not copy it at every step. `draw(p)` hands over what it holds
/// at that moment.
#[derive(Clone)]
pub struct PathV(Rc<RefCell<super::shapes::PathDraw>>);

fn path_push(p: &mut PathV, s: super::shapes::Seg) -> Res<PathV> {
    let mut inner = p.0.borrow_mut();
    inner.segs.push(s);
    if inner.points() > super::shapes::MAX_POINTS {
        return Err(format!("a path of more than {} points", super::shapes::MAX_POINTS).into());
    }
    drop(inner);
    Ok(p.clone())
}

fn path_set(p: &mut PathV, f: impl FnOnce(&mut super::shapes::PathDraw)) -> PathV {
    f(&mut p.0.borrow_mut());
    p.clone()
}

fn register_shapes(e: &mut Engine, w: f32, h: f32) {
    use super::shapes::{Paint, PathDraw, Seg, ellipse_segs, rounded_rect_segs};
    let new = |segs: Vec<Seg>| {
        PathV(Rc::new(RefCell::new(PathDraw {
            segs,
            ..Default::default()
        })))
    };
    e.register_type_with_name::<PathV>("Path");
    e.register_fn("path", move || new(Vec::new()));
    e.register_fn("circle", move |x: f64, y: f64, r: f64| {
        new(ellipse_segs(x as f32, y as f32, r as f32, r as f32, w, h))
    });
    e.register_fn("ellipse", move |x: f64, y: f64, rx: f64, ry: f64| {
        new(ellipse_segs(x as f32, y as f32, rx as f32, ry as f32, w, h))
    });
    e.register_fn(
        "rounded_rect",
        move |x: f64, y: f64, rw: f64, rh: f64, r: f64| {
            new(rounded_rect_segs(
                x as f32, y as f32, rw as f32, rh as f32, r as f32, w, h,
            ))
        },
    );
    e.register_fn("line", move |x0: f64, y0: f64, x1: f64, y1: f64| {
        new(vec![
            Seg::Move([x0 as f32, y0 as f32]),
            Seg::Line([x1 as f32, y1 as f32]),
        ])
    });
    let f = |x: f64| x as f32;
    e.register_fn("move_to", move |p: &mut PathV, x: f64, y: f64| {
        path_push(p, Seg::Move([f(x), f(y)]))
    });
    e.register_fn("line_to", move |p: &mut PathV, x: f64, y: f64| {
        path_push(p, Seg::Line([f(x), f(y)]))
    });
    e.register_fn(
        "quad_to",
        move |p: &mut PathV, cx: f64, cy: f64, x: f64, y: f64| {
            path_push(p, Seg::Quad([f(cx), f(cy)], [f(x), f(y)]))
        },
    );
    e.register_fn(
        "cubic_to",
        move |p: &mut PathV, ax: f64, ay: f64, bx: f64, by: f64, x: f64, y: f64| {
            path_push(p, Seg::Cubic([f(ax), f(ay)], [f(bx), f(by)], [f(x), f(y)]))
        },
    );
    e.register_fn("close", |p: &mut PathV| path_push(p, Seg::Close));
    e.register_fn("fill", |p: &mut PathV, c: &str| {
        path_set(p, |d| d.fill = Paint::Solid(c.into()))
    });
    e.register_fn(
        "linear",
        move |p: &mut PathV, x0: f64, y0: f64, x1: f64, y1: f64| {
            path_set(p, |d| {
                d.fill = Paint::Linear([f(x0), f(y0)], [f(x1), f(y1)], Vec::new())
            })
        },
    );
    e.register_fn("radial", move |p: &mut PathV, x: f64, y: f64, r: f64| {
        path_set(p, |d| {
            d.fill = Paint::Radial([f(x), f(y)], f(r), Vec::new())
        })
    });
    e.register_fn(
        "stop",
        move |p: &mut PathV, o: f64, c: &str, a: f64| -> Res<PathV> {
            let mut inner = p.0.borrow_mut();
            let stops = match &mut inner.fill {
                Paint::Linear(_, _, s) | Paint::Radial(_, _, s) => s,
                _ => return Err("stop() follows linear() or radial()".into()),
            };
            if stops.len() >= super::shapes::MAX_STOPS {
                return Err(format!("more than {} stops", super::shapes::MAX_STOPS).into());
            }
            stops.push((f(o), c.into(), f(a)));
            drop(inner);
            Ok(p.clone())
        },
    );
    e.register_fn("stroke", move |p: &mut PathV, c: &str, wd: f64| {
        path_set(p, |d| d.stroke = Some((c.into(), f(wd))))
    });
    e.register_fn("caps", |p: &mut PathV, kind: &str| -> Res<PathV> {
        let round = match kind {
            "round" => true,
            "butt" => false,
            _ => return Err(format!("caps {kind:?} unknown (round, butt)").into()),
        };
        Ok(path_set(p, |d| d.round_caps = round))
    });
    e.register_fn("trim", move |p: &mut PathV, a: f64, b: f64| {
        path_set(p, |d| d.trim = [f(a), f(b)])
    });
    e.register_fn("opacity", move |p: &mut PathV, v: f64| {
        path_set(p, |d| d.opacity = f(v))
    });
    e.register_fn("pivot", move |p: &mut PathV, x: f64, y: f64| {
        path_set(p, |d| d.pivot = [f(x), f(y)])
    });
    e.register_fn("rot", move |p: &mut PathV, v: f64| {
        path_set(p, |d| d.rot = f(v))
    });
    e.register_fn("scale", move |p: &mut PathV, v: f64| {
        path_set(p, |d| d.scale = f(v))
    });
    e.register_fn("offset", move |p: &mut PathV, x: f64, y: f64| {
        path_set(p, |d| d.offset = [f(x), f(y)])
    });
}

/// Features of the format that NOH does not have yet: a scene that uses one is
/// refused with this reason when it is checked.
const NOT_YET_VIDEO: &str =
    "video() is not in NOH yet: reframing the picture comes in a later version";
const NOT_YET_SHADER: &str = "layer shaders are not in NOH yet: they come in a later version";

fn register_not_yet(e: &mut Engine) {
    e.register_fn("video", || -> Res<()> { Err(NOT_YET_VIDEO.into()) });
}

/// A layer as the script holds it: a handle, so that copying it costs nothing
/// whatever it contains. `draw(layer)` hands over what it holds at that moment.
#[derive(Clone, Default)]
pub struct LayerV(Rc<RefCell<(Vec<Draw>, f32)>>);

fn layer_add(l: &mut LayerV, d: Draw) -> Res<LayerV> {
    let mut inner = l.0.borrow_mut();
    if inner.0.len() >= MAX_DRAWS {
        return Err(format!("more than {MAX_DRAWS} draws in one layer").into());
    }
    inner.0.push(d);
    drop(inner);
    Ok(l.clone())
}

fn register_layer(e: &mut Engine) {
    e.register_type_with_name::<LayerV>("Layer");
    e.register_fn("layer", || LayerV(Rc::new(RefCell::new((Vec::new(), 1.0)))));
    e.register_fn("shader", |_: &mut LayerV, _: &str| -> Res<LayerV> {
        Err(NOT_YET_SHADER.into())
    });
    e.register_fn("resolution", |_: &mut LayerV, _: &str| -> Res<LayerV> {
        Err(NOT_YET_SHADER.into())
    });
    e.register_fn("param", |_: &mut LayerV, _: i64, _: f64| -> Res<LayerV> {
        Err(NOT_YET_SHADER.into())
    });
    e.register_fn("add", |l: &mut LayerV, d: TextDraw| {
        layer_add(l, Draw::Text(d))
    });
    e.register_fn("add", |l: &mut LayerV, d: RectDraw| {
        layer_add(l, Draw::Rect(d))
    });
    e.register_fn("add", |l: &mut LayerV, d: EmitterDraw| {
        layer_add(l, Draw::Emitter(d))
    });
    e.register_fn("add", |l: &mut LayerV, d: PathV| {
        let snap = d.0.borrow().clone();
        layer_add(l, Draw::Path(snap))
    });
    e.register_fn("opacity", |l: &mut LayerV, v: f64| {
        l.0.borrow_mut().1 = v as f32;
        l.clone()
    });
}

/// One evaluated frame, as host data, with what it cost.
#[derive(Clone, Debug)]
pub struct Frame {
    pub draws: Vec<Draw>,
    /// The runs of `draws` that are layers, in order.
    pub layers: Vec<Span>,
    /// Operations spent (0 when the scene was loaded without counting).
    pub ops: u64,
    /// Calls to costly offered functions, and reads of lyrics and onsets.
    pub calls: u32,
    pub reads: u32,
    /// Milliseconds running the script: measured, reported, never a reason to refuse.
    pub run_ms: f64,
}

pub fn ease_named(curve: &str, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    match curve {
        "ease_in" => x * x,
        "ease_out" => 1.0 - (1.0 - x) * (1.0 - x),
        "ease_in_out" => x * x * (3.0 - 2.0 * x),
        "ease_in_cubic" => x * x * x,
        "ease_out_cubic" => 1.0 - (1.0 - x).powi(3),
        "back" => {
            let (c1, c3) = (1.70158, 2.70158);
            1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
        }
        _ => x,
    }
}

fn word_mut(d: &mut TextDraw, k: i64) -> Res<&mut WordMod> {
    let k = usize::try_from(k).ok().filter(|k| *k < MAX_WORDS);
    let k = k.ok_or_else(|| format!("word index out of 0..{MAX_WORDS}"))?;
    while d.words.len() <= k {
        d.words.push(WordMod::neutral());
    }
    Ok(&mut d.words[k])
}

/// The builders of text draws: each sets one thing and gives the draw back, so
/// they chain (`text("a").size(0.1).color("#fff")`) or stand alone (`d.size(0.1);`).
fn register_text(e: &mut Engine) {
    e.register_type_with_name::<TextDraw>("Text");
    e.register_fn("text", |s: &str| TextDraw::new(s));
    macro_rules! num {
        ($name:literal, $($field:ident).+) => {
            e.register_fn($name, |d: &mut TextDraw, v: f64| {
                d.$($field).+ = v as f32;
                d.clone()
            });
        };
    }
    num!("size", block.size);
    num!("width", block.width);
    num!("line_height", block.line_height);
    num!("rot", block.rot);
    num!("tracking", block.tracking);
    num!("opacity", opacity);
    num!("dissolve", dissolve);
    num!("glow", glow);
    num!("scale", scale);
    e.register_fn("at", |d: &mut TextDraw, x: f64, y: f64| {
        (d.block.x, d.block.y) = (x as f32, y as f32);
        d.clone()
    });
    e.register_fn("face", |d: &mut TextDraw, f: &str| {
        d.block.face = f.into();
        d.clone()
    });
    e.register_fn("align", |d: &mut TextDraw, a: &str| {
        d.block.align = a.into();
        d.clone()
    });
    e.register_fn("anchor", |d: &mut TextDraw, a: &str| {
        d.block.anchor = a.into();
        d.clone()
    });
    e.register_fn("color", |d: &mut TextDraw, c: &str| {
        d.color = c.into();
        d.clone()
    });
    e.register_fn("edge", |d: &mut TextDraw, c: &str| {
        d.edge = c.into();
        d.clone()
    });
    e.register_fn(
        "shadow",
        |d: &mut TextDraw, opacity: f64, blur: f64, dy: f64| {
            d.shadow = Some(Shadow {
                color: "#000000".into(),
                opacity: opacity as f32,
                blur: blur as f32,
                dy: dy as f32,
            });
            d.clone()
        },
    );
    e.register_fn("outline", |d: &mut TextDraw, color: &str, width: f64| {
        d.outline = Some(Outline {
            color: color.into(),
            opacity: 1.0,
            width: width as f32,
        });
        d.clone()
    });
    // Per-word settings: opacity, vertical shift in em, scale.
    e.register_fn(
        "word",
        |d: &mut TextDraw, k: i64, opacity: f64, dy: f64, scale: f64| -> Res<TextDraw> {
            let m = word_mut(d, k)?;
            (m.opacity, m.dy, m.scale) = (opacity as f32, dy as f32, scale as f32);
            Ok(d.clone())
        },
    );
    e.register_fn(
        "word_dissolve",
        |d: &mut TextDraw, k: i64, v: f64| -> Res<TextDraw> {
            word_mut(d, k)?.dissolve = Some(v as f32);
            Ok(d.clone())
        },
    );
    e.register_fn(
        "word_color",
        |d: &mut TextDraw, k: i64, c: &str| -> Res<TextDraw> {
            word_mut(d, k)?.color = Some(super::draw::hex(c)?);
            Ok(d.clone())
        },
    );
}

fn register_rect(e: &mut Engine) {
    e.register_type_with_name::<RectDraw>("Rect");
    e.register_fn("rect", |w: f64, h: f64| RectDraw::new(w as f32, h as f32));
    e.register_fn("at", |d: &mut RectDraw, x: f64, y: f64| {
        (d.x, d.y) = (x as f32, y as f32);
        d.clone()
    });
    e.register_fn("rot", |d: &mut RectDraw, v: f64| {
        d.rot = v as f32;
        d.clone()
    });
    e.register_fn("opacity", |d: &mut RectDraw, v: f64| {
        d.opacity = v as f32;
        d.clone()
    });
    e.register_fn("glow", |d: &mut RectDraw, v: f64| {
        d.glow = v as f32;
        d.clone()
    });
    e.register_fn("color", |d: &mut RectDraw, c: &str| {
        d.color = c.into();
        d.clone()
    });
}

fn register_emitter(e: &mut Engine) {
    e.register_type_with_name::<EmitterDraw>("Particles");
    // Born from the ink of a text (as it is laid out, whatever its opacity).
    e.register_fn("particles", |t: TextDraw| {
        EmitterDraw::new(Source::Text(t.block))
    });
    // Every particle has left by this time, whatever its birth.
    e.register_fn("gone_by", |d: &mut EmitterDraw, t: f64| {
        d.gone_by = Some(t);
        d.clone()
    });
    // Born in a rectangle: x0, y0, x1, y1.
    e.register_fn("particles_in", |x0: f64, y0: f64, x1: f64, y1: f64| {
        EmitterDraw::new(Source::Rect {
            rect: [x0 as f32, y0 as f32, x1 as f32, y1 as f32],
        })
    });
    macro_rules! num {
        ($name:literal, $field:ident) => {
            e.register_fn($name, |d: &mut EmitterDraw, v: f64| {
                d.$field = v as f32;
                d.clone()
            });
        };
    }
    num!("rate", rate);
    num!("life", life);
    num!("size", size);
    num!("rise", rise);
    num!("swirl", swirl);
    num!("spin", spin);
    num!("opacity", opacity);
    num!("glow", glow);
    e.register_fn("during", |d: &mut EmitterDraw, a: f64, b: f64| {
        (d.start, d.end) = (a, b);
        d.clone()
    });
    e.register_fn("wind", |d: &mut EmitterDraw, x: f64, y: f64| {
        d.wind = [x as f32, y as f32];
        d.clone()
    });
    e.register_fn("shape", |d: &mut EmitterDraw, s: &str| {
        d.shape = s.into();
        d.clone()
    });
    e.register_fn("seed", |d: &mut EmitterDraw, s: i64| {
        d.seed = s as u32;
        d.clone()
    });
    // Adds one colour; particles take the colours in turn.
    e.register_fn(
        "color",
        |d: &mut EmitterDraw, c: &str| -> Res<EmitterDraw> {
            if d.colors.len() >= MAX_COLORS {
                return Err(format!("more than {MAX_COLORS} colours on an emitter").into());
            }
            d.colors.push(c.into());
            Ok(d.clone())
        },
    );
}

fn push(out: &Rc<RefCell<Vec<Draw>>>, d: Draw) -> Res<()> {
    let mut list = out.borrow_mut();
    if list.len() >= MAX_DRAWS {
        return Err(format!("more than {MAX_DRAWS} draws in one frame").into());
    }
    list.push(d);
    Ok(())
}

fn read(reads: &Rc<Cell<u32>>) -> Res<()> {
    reads.set(reads.get() + 1);
    if reads.get() > MAX_READS {
        return Err(format!("more than {MAX_READS} reads of the lyrics in one frame").into());
    }
    Ok(())
}

/// Refuses a scene that does not name the format this NOH reads on its first line.
fn check_format(src: &str) -> Result<(), String> {
    let first = src.lines().next().unwrap_or("").trim();
    match first.strip_prefix(HEADER).map(str::trim) {
        Some(n) if n == FORMAT.to_string() => Ok(()),
        Some(n) => Err(format!(
            "script: written for scene format {n:?}; this NOH reads format {FORMAT}"
        )),
        None => Err(format!(
            "script: the first line must be « {HEADER}{FORMAT} » (the scene format it is written for)"
        )),
    }
}

/// Syntax a scene has no use for, switched off: object maps, evaluating text as
/// code, and everything that makes or handles function pointers (a pointer can
/// be curried with values, which would be a value growing without a cap).
pub const DISABLED: [&str; 9] = [
    "#{",
    "eval",
    "Fn",
    "call",
    "curry",
    "is_shared",
    "is_def_fn",
    "is_def_var",
    "|",
];

/// The numeric and text functions offered, beside the operators the engine has
/// built in for numbers, booleans and strings.
fn register_basics(e: &mut Engine) {
    e.register_fn("to_float", |x: i64| x as f64);
    e.register_fn("to_int", |x: f64| x as i64);
    e.register_fn("sin", |x: f64| x.sin());
    e.register_fn("cos", |x: f64| x.cos());
    e.register_fn("abs", |x: f64| x.abs());
    e.register_fn("sqrt", |x: f64| x.max(0.0).sqrt());
    e.register_fn("exp", |x: f64| x.exp());
    e.register_fn("floor", |x: f64| x.floor());
    e.register_fn("round", |x: f64| x.round());
    e.register_fn("pow", |x: f64, y: f64| x.powf(y));
    e.register_fn("min", |a: f64, b: f64| a.min(b));
    e.register_fn("max", |a: f64, b: f64| a.max(b));
    e.register_fn("pi", || std::f64::consts::PI);
    // A small table without arrays: the i-th of the values given (the last one
    // when i is beyond them).
    e.register_fn("pick", |i: i64, a: f64, b: f64| if i <= 0 { a } else { b });
    e.register_fn("pick", |i: i64, a: f64, b: f64, c: f64| {
        [a, b, c][i.clamp(0, 2) as usize]
    });
    e.register_fn("pick", |i: i64, a: f64, b: f64, c: f64, d: f64| {
        [a, b, c, d][i.clamp(0, 3) as usize]
    });
    e.register_fn("pick", |i: i64, a: f64, b: f64, c: f64, d: f64, f: f64| {
        [a, b, c, d, f][i.clamp(0, 4) as usize]
    });
    e.register_fn("pick", |i: i64, a: &str, b: &str| {
        (if i <= 0 { a } else { b }).to_string()
    });
    e.register_fn("pick", |i: i64, a: &str, b: &str, c: &str| {
        [a, b, c][i.clamp(0, 2) as usize].to_string()
    });
    e.register_fn("pick", |i: i64, a: &str, b: &str, c: &str, d: &str| {
        [a, b, c, d][i.clamp(0, 3) as usize].to_string()
    });
    e.register_fn("len", |s: &str| s.chars().count() as i64);
    e.register_fn("upper", |s: &str| s.to_uppercase());
    e.register_fn("lower", |s: &str| s.to_lowercase());
}

/// Measuring a text from inside a script: the layout alone (no glyph image, no
/// atlas), one result kept per distinct text and placement, a counted number of
/// calls per frame.
fn register_measure(
    e: &mut Engine,
    fonts: Rc<RefCell<Fonts>>,
    w: f32,
    h: f32,
    calls: Rc<Cell<u32>>,
) {
    type Cache = Rc<RefCell<HashMap<String, Rc<(BoxF, Vec<BoxF>)>>>>;
    let cache: Cache = Rc::default();
    let look = move |d: &TextDraw| -> Res<Rc<(BoxF, Vec<BoxF>)>> {
        calls.set(calls.get() + 1);
        if calls.get() > MAX_CALLS {
            return Err(format!("more than {MAX_CALLS} measures in one frame").into());
        }
        d.block.check()?;
        let b = &d.block;
        let key = format!(
            "{}|{}|{}|{}|{}",
            super::draw::layout_key(b, w, h),
            b.x,
            b.y,
            b.anchor,
            d.scale
        );
        if let Some(m) = cache.borrow().get(&key) {
            return Ok(m.clone());
        }
        let m = Rc::new(super::draw::measure(
            &mut fonts.borrow_mut(),
            b,
            d.scale,
            w,
            h,
        ));
        let mut c = cache.borrow_mut();
        if c.len() >= 4096 {
            c.clear();
        }
        c.insert(key, m.clone());
        Ok(m)
    };
    e.register_type_with_name::<BoxF>("Box");
    e.register_get("x0", |b: &mut BoxF| b.x0 as f64);
    e.register_get("y0", |b: &mut BoxF| b.y0 as f64);
    e.register_get("x1", |b: &mut BoxF| b.x1 as f64);
    e.register_get("y1", |b: &mut BoxF| b.y1 as f64);
    e.register_get("w", |b: &mut BoxF| (b.x1 - b.x0) as f64);
    e.register_get("h", |b: &mut BoxF| (b.y1 - b.y0) as f64);
    let l = look.clone();
    // The box of a text as it would be drawn (before any rotation).
    e.register_fn("measure", move |d: TextDraw| -> Res<BoxF> { Ok(l(&d)?.0) });
    // The box of its k-th word.
    e.register_fn("word_box", move |d: TextDraw, k: i64| -> Res<BoxF> {
        let m = look(&d)?;
        let b = usize::try_from(k).ok().and_then(|k| m.1.get(k).copied());
        b.ok_or_else(|| format!("word_box: the text has {} words", m.1.len()).into())
    });
}

impl ScriptScene {
    /// An engine with nothing registered but the limits: no standard library.
    fn bare_engine() -> Engine {
        let mut engine = Engine::new_raw();
        engine.set_max_operations(MAX_OPS);
        engine.set_max_call_levels(MAX_CALL_LEVELS);
        engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH);
        engine.set_max_string_size(MAX_STRING);
        engine.set_max_map_size(8);
        for s in DISABLED {
            engine.disable_symbol(s);
        }
        engine.on_print(|_| {});
        engine.on_debug(|_, _, _| {});
        // `for i in 0..n`: the range iterators, and nothing else of the library.
        engine.register_global_module(rhai::packages::Package::as_shared_module(
            &rhai::packages::BasicIteratorPackage::new(),
        ));
        engine
    }

    /// Every function a script can call, by name (operators aside), sorted.
    #[cfg(test)]
    pub fn callable(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .engine
            .gen_fn_signatures(true)
            .into_iter()
            .filter_map(|sig| sig.split('(').next().map(|n| n.trim().to_string()))
            .filter(|n| n.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// A function offered to scripts by a test only (a probe), never in a build.
    #[cfg(test)]
    pub fn register_test_fn(&mut self, name: &str, f: fn() -> f64) {
        self.engine.register_fn(name, f);
    }

    /// `count`: record the operations of each frame (the validation pass does;
    /// drawing does not pay for the per-operation callback).
    pub fn load(
        src: &str,
        inp: &Inputs,
        audio: Rc<Audio>,
        fonts: Rc<RefCell<Fonts>>,
        count: bool,
    ) -> Result<Self, String> {
        if src.len() > MAX_SOURCE {
            return Err(format!("script: {} bytes exceed {MAX_SOURCE}", src.len()));
        }
        // A byte order mark is no part of the script (an editor may write one).
        let src = src.trim_start_matches('\u{feff}');
        check_format(src)?;
        let mut engine = Self::bare_engine();
        let ops = Rc::new(Cell::new(0u64));
        let calls = Rc::new(Cell::new(0u32));
        if count {
            let o = ops.clone();
            engine.on_progress(move |n| {
                o.set(n);
                None
            });
        }
        register_basics(&mut engine);
        register_measure(
            &mut engine,
            fonts,
            inp.w as f32,
            inp.h as f32,
            calls.clone(),
        );
        register_text(&mut engine);
        register_rect(&mut engine);
        register_emitter(&mut engine);
        register_layer(&mut engine);
        register_not_yet(&mut engine);
        register_shapes(&mut engine, inp.w as f32, inp.h as f32);
        let out: Rc<RefCell<Vec<Draw>>> = Rc::default();
        let o = out.clone();
        engine.register_fn("draw", move |d: TextDraw| push(&o, Draw::Text(d)));
        let o = out.clone();
        engine.register_fn("draw", move |d: RectDraw| push(&o, Draw::Rect(d)));
        let o = out.clone();
        engine.register_fn("draw", move |d: EmitterDraw| push(&o, Draw::Emitter(d)));
        let o = out.clone();
        engine.register_fn("draw", move |d: PathV| {
            let snap = d.0.borrow().clone();
            push(&o, Draw::Path(snap))
        });
        let layers: Rc<RefCell<Vec<Span>>> = Rc::default();
        let (o, ls) = (out.clone(), layers.clone());
        engine.register_fn("draw", move |l: LayerV| -> Res<()> {
            let inner = l.0.borrow();
            let first = o.borrow().len();
            for d in &inner.0 {
                push(&o, d.clone())?;
            }
            ls.borrow_mut().push(Span {
                first,
                len: inner.0.len(),
                opacity: inner.1,
            });
            Ok(())
        });
        let audio_onsets = audio.onsets().to_vec();
        let a = audio.clone();
        engine.register_fn("energy", move |t: f64| a.energy_at(t));
        let a = audio;
        engine.register_fn("onset_age", move |t: f64| a.onset_age(t));
        engine.register_fn("ease", ease_named);
        engine.register_fn("clampf", |x: f64, a: f64, b: f64| x.clamp(a, b.max(a)));
        engine.register_fn("mixf", |a: f64, b: f64, x: f64| a + (b - a) * x);
        engine.register_fn("smooth", |a: f64, b: f64, x: f64| {
            let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        });

        // The scene's frame and the song, read one small value at a time.
        engine.register_type_with_name::<Ctx>("Context");
        engine.register_get("w", |c: &mut Ctx| c.w);
        engine.register_get("h", |c: &mut Ctx| c.h);
        engine.register_get("duration", |c: &mut Ctx| c.duration);
        engine.register_get("safe_x0", |c: &mut Ctx| c.safe[0]);
        engine.register_get("safe_y0", |c: &mut Ctx| c.safe[1]);
        engine.register_get("safe_x1", |c: &mut Ctx| c.safe[2]);
        engine.register_get("safe_y1", |c: &mut Ctx| c.safe[3]);
        engine.register_get("src_w", |c: &mut Ctx| c.src_w);
        engine.register_get("src_h", |c: &mut Ctx| c.src_h);
        engine.register_type_with_name::<CueV>("Cue");
        engine.register_get("i", |c: &mut CueV| c.i);
        engine.register_get("start", |c: &mut CueV| c.start);
        engine.register_get("end", |c: &mut CueV| c.end);
        engine.register_get("text", |c: &mut CueV| c.text.clone());
        engine.register_get("words", |c: &mut CueV| c.words);
        engine.register_type_with_name::<WordV>("Word");
        engine.register_get("start", |c: &mut WordV| c.start);
        engine.register_get("end", |c: &mut WordV| c.end);
        engine.register_get("text", |c: &mut WordV| c.text.clone());
        let reads = Rc::new(Cell::new(0u32));
        type Cues = Rc<Vec<(f64, f64, String, Vec<(String, f64, f64)>)>>;
        let cues: Cues = Rc::new(
            inp.cues
                .iter()
                .map(|c| (c.start, c.end, c.text.clone(), c.words.clone()))
                .collect(),
        );
        let n = cues.len() as i64;
        engine.register_fn("cue_count", move || n);
        let (c, r) = (cues.clone(), reads.clone());
        engine.register_fn("cue", move |i: i64| -> Res<CueV> {
            read(&r)?;
            let cue = usize::try_from(i).ok().and_then(|i| c.get(i));
            let cue = cue.ok_or_else(|| format!("cue({i}): there are {} cues", c.len()))?;
            Ok(CueV {
                i,
                start: cue.0,
                end: cue.1,
                text: cue.2.clone(),
                words: cue.3.len() as i64,
            })
        });
        let (c, r) = (cues.clone(), reads.clone());
        engine.register_fn("word", move |i: i64, k: i64| -> Res<WordV> {
            read(&r)?;
            let w = usize::try_from(i)
                .ok()
                .and_then(|i| c.get(i))
                .and_then(|cue| usize::try_from(k).ok().and_then(|k| cue.3.get(k)));
            let w = w.ok_or_else(|| format!("word({i}, {k}): no such word"))?;
            Ok(WordV {
                text: w.0.clone(),
                start: w.1,
                end: w.2,
            })
        });
        let c = cues.clone();
        // Index of the last cue that has started at `t`, or -1.
        engine.register_fn("cue_at", move |t: f64| {
            c.partition_point(|x| x.0 <= t) as i64 - 1
        });
        let onsets = Rc::new(audio_onsets);
        let n = onsets.len() as i64;
        engine.register_fn("onset_count", move || n);
        let (o, r) = (onsets, reads.clone());
        engine.register_fn("onset", move |i: i64| -> Res<f64> {
            read(&r)?;
            let v = usize::try_from(i).ok().and_then(|i| o.get(i).copied());
            v.ok_or_else(|| format!("onset({i}): no such onset").into())
        });
        let ast = engine.compile(src).map_err(|e| format!("script: {e}"))?;
        let ctx = Ctx {
            w: inp.w as f64,
            h: inp.h as f64,
            src_w: inp.src_w as f64,
            src_h: inp.src_h as f64,
            duration: inp.duration,
            safe: inp.safe.map(f64::from),
        };
        Ok(Self {
            engine,
            ast,
            ctx,
            ops,
            calls,
            reads,
            out,
            layers,
        })
    }

    pub fn frame(&self, t: f64) -> Result<Frame, String> {
        self.ops.set(0);
        self.calls.set(0);
        self.reads.set(0);
        self.out.borrow_mut().clear();
        self.layers.borrow_mut().clear();
        let started = std::time::Instant::now();
        let r: Dynamic = self
            .engine
            .call_fn(&mut Scope::new(), &self.ast, "frame", (t, self.ctx.clone()))
            .map_err(|e| format!("script at t={t:.3}: {e}"))?;
        let run_ms = started.elapsed().as_secs_f64() * 1000.0;
        if !r.is_unit() {
            return Err(format!(
                "script at t={t:.3}: frame() returns nothing; hand each draw to draw(…)"
            ));
        }
        Ok(Frame {
            draws: std::mem::take(&mut *self.out.borrow_mut()),
            layers: std::mem::take(&mut *self.layers.borrow_mut()),
            ops: self.ops.get(),
            calls: self.calls.get(),
            reads: self.reads.get(),
            run_ms,
        })
    }
}
