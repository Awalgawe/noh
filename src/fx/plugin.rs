//! Generic effect engine driven by plugin files (archi-greffons-v2). The engine
//! knows layouts, tracks and passes; it knows no effect by name.
use super::line::Line;
use super::shaders::{Inst, PASS_FILL, PASS_GLOW, PASS_OUTLINE, PASS_PLATE, PASS_SHADOW, Upload};
use super::text::{Atlas, Face, Fonts, Lack, PGlyph, RowAlign, Sprite, layout, layout_aligned};
use serde_json::Value as J;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

pub const ENGINE_API: u64 = 1;
const MAX_JSON: usize = 16 * 1024;
const MAX_TRACKS: usize = 64;
const MAX_PARAMS: usize = 32;
const MAX_LEVEL1: usize = 5;
const MAX_WGSL: usize = 64 * 1024;

#[derive(Debug)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(field: &str, why: impl std::fmt::Display) -> Result<T, LoadError> {
    Err(LoadError(format!("{field}: {why}")))
}

// ---------------------------------------------------------------- values

#[derive(Clone, Debug)]
pub enum V {
    Num(f64),
    Color([u8; 3]),
    Text(String),
    /// Param id and the field that refers to it (for error messages).
    Param(String, String),
    Orient(Box<V>, Box<V>),
    Mix(Box<V>, Box<V>, Box<V>),
    Scale(Box<V>, Box<V>),
    Pick(Box<V>, String),
    /// Past, active and next values: the state of the line being drawn.
    ByState(Box<V>, Box<V>, Box<V>),
    /// Values by distance from the active slot (0, 1, 2, 3; the last repeats).
    ByOffset(Vec<V>),
    /// A number the host gives: canvas size and safe-area insets (see HOST_VARS).
    Host(usize),
    /// A number of the word being drawn (see WORD_VARS).
    Word(usize),
    /// A number of the line being drawn (see LINE_VARS).
    Line(usize),
    /// Arithmetic on numbers: add, sub, mul, div, min, max (2+ args), round, ceil, floor (1).
    Op(Op, Vec<V>),
    /// The value for the current text of a choice parameter, else the `default`
    /// case, else the first case in key order (JSON objects are unordered).
    Choose(Box<V>, Vec<(String, V)>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Round,
    Ceil,
    Floor,
    /// A number from -1 to 1 that only depends on its argument rounded to a whole
    /// number: the same value for the same line or word, a different one for the next.
    Hash,
}

/// Numbers an effect may read, written `@name`, in canvas pixels. `inset_*` are
/// the safe area's insets from each edge (0 without a safe area) and `safe` is 1
/// when a safe area applies, else 0.
pub const HOST_VARS: [&str; 8] = [
    "w", "h", "short", "inset_l", "inset_t", "inset_r", "inset_b", "safe",
];

/// Numbers of the word being drawn, written `@name`: its sung duration in
/// seconds (word_lead not applied) and 1 for the longest-sung word of its line
/// (the first on a tie), else 0. Both are 0 where no word is being drawn.
pub const WORD_VARS: [&str; 2] = ["word_duration", "word_longest"];

/// Numbers of the line being drawn, written `@name`: its rank among the lines of
/// the lyrics (a chunk's among the chunks), from 0.
pub const LINE_VARS: [&str; 1] = ["line_index"];

#[derive(Clone, Debug)]
pub enum PVal {
    Num(f64),
    Color([u8; 3]),
    Text(String),
    Palette(Vec<[u8; 3]>),
    Bool(bool),
}

/// Whether a layout value names an installed font anywhere in it (`system:…`):
/// only a font parameter may (its default stays a bundled face).
fn names_installed_font(j: &J) -> bool {
    match j {
        J::String(s) => s.starts_with("system:"),
        J::Array(a) => a.iter().any(names_installed_font),
        J::Object(o) => o.values().any(names_installed_font),
        _ => false,
    }
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    // Byte slicing below: a non-ASCII character would split inside a code point.
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

fn parse_v(j: &J, field: &str) -> Result<V, LoadError> {
    Ok(match j {
        J::Number(n) => V::Num(n.as_f64().unwrap_or(0.0)),
        J::String(s) if s.starts_with('$') => V::Param(s[1..].to_string(), field.to_string()),
        J::String(s) if s.starts_with('@') => {
            match (
                HOST_VARS.iter().position(|v| *v == &s[1..]),
                WORD_VARS.iter().position(|v| *v == &s[1..]),
                LINE_VARS.iter().position(|v| *v == &s[1..]),
            ) {
                (Some(i), _, _) => V::Host(i),
                (None, Some(i), _) => V::Word(i),
                (None, None, Some(i)) => V::Line(i),
                (None, None, None) => {
                    return err(
                        field,
                        format!(
                            "{s:?} is not a host, word or line value ({}, {}, {})",
                            HOST_VARS.join(", "),
                            WORD_VARS.join(", "),
                            LINE_VARS.join(", ")
                        ),
                    );
                }
            }
        }
        J::String(s) if s.starts_with('#') => {
            V::Color(hex(s).ok_or_else(|| LoadError(format!("{field}: bad colour {s}")))?)
        }
        J::String(s) => V::Text(s.clone()),
        J::Object(o) if o.contains_key("landscape") || o.contains_key("portrait") => {
            let (Some(l), Some(p)) = (o.get("landscape"), o.get("portrait")) else {
                return err(field, "orientation value needs both landscape and portrait");
            };
            if o.len() != 2 {
                return err(field, "orientation value has extra keys");
            }
            V::Orient(Box::new(parse_v(l, field)?), Box::new(parse_v(p, field)?))
        }
        J::Object(o) if o.len() == 1 && o.contains_key("choose") => {
            let args = o["choose"]
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| {
                    LoadError(format!(
                        "{field}: choose needs [selector, {{\"value\": ...}}]"
                    ))
                })?;
            let map = args[1]
                .as_object()
                .filter(|m| !m.is_empty())
                .ok_or_else(|| {
                    LoadError(format!(
                        "{field}: choose needs a non-empty object of values"
                    ))
                })?;
            let cases = map
                .iter()
                .map(|(k, v)| Ok((k.clone(), parse_v(v, &format!("{field}.choose.{k}"))?)))
                .collect::<Result<Vec<_>, LoadError>>()?;
            V::Choose(
                Box::new(parse_v(&args[0], &format!("{field}.choose"))?),
                cases,
            )
        }
        J::Object(o) if o.len() == 1 && o.contains_key("by_offset") => {
            let list = o["by_offset"]
                .as_array()
                .filter(|a| (1..=4).contains(&a.len()))
                .ok_or_else(|| LoadError(format!("{field}: by_offset needs 1 to 4 values")))?;
            V::ByOffset(
                list.iter()
                    .enumerate()
                    .map(|(i, v)| parse_v(v, &format!("{field}.by_offset[{i}]")))
                    .collect::<Result<_, _>>()?,
            )
        }
        J::Object(o) if o.len() == 1 && o.contains_key("by_state") => {
            let so = o["by_state"]
                .as_object()
                .ok_or_else(|| LoadError(format!("{field}: by_state needs an object")))?;
            only(
                so,
                &format!("{field}.by_state"),
                &["past", "active", "next"],
            )?;
            let active = so
                .get("active")
                .ok_or_else(|| LoadError(format!("{field}.by_state: active is required")))?;
            let pick = |k: &str| {
                parse_v(
                    so.get(k).unwrap_or(active),
                    &format!("{field}.by_state.{k}"),
                )
            };
            V::ByState(
                Box::new(pick("past")?),
                Box::new(pick("active")?),
                Box::new(pick("next")?),
            )
        }
        J::Object(o) if o.len() == 1 => {
            let (k, v) = o.iter().next().unwrap();
            let args = v
                .as_array()
                .ok_or_else(|| LoadError(format!("{field}: operator {k} needs an array")))?;
            match (k.as_str(), args.len()) {
                ("mix", 3) => V::Mix(
                    Box::new(parse_v(&args[0], field)?),
                    Box::new(parse_v(&args[1], field)?),
                    Box::new(parse_v(&args[2], field)?),
                ),
                ("scale", 2) => V::Scale(
                    Box::new(parse_v(&args[0], field)?),
                    Box::new(parse_v(&args[1], field)?),
                ),
                ("pick", 2) => {
                    let scope = args[1].as_str().unwrap_or("");
                    if !matches!(
                        scope,
                        "word_index" | "index" | "index_in_word" | "line_index"
                    ) {
                        return err(field, format!("pick scope {scope:?} unknown"));
                    }
                    V::Pick(Box::new(parse_v(&args[0], field)?), scope.into())
                }
                (k @ ("add" | "sub" | "mul" | "div" | "min" | "max"), n)
                    if (2..=8).contains(&n) =>
                {
                    let op = match k {
                        "add" => Op::Add,
                        "sub" => Op::Sub,
                        "mul" => Op::Mul,
                        "div" => Op::Div,
                        "min" => Op::Min,
                        _ => Op::Max,
                    };
                    V::Op(
                        op,
                        args.iter()
                            .map(|a| parse_v(a, field))
                            .collect::<Result<_, _>>()?,
                    )
                }
                (k @ ("round" | "ceil" | "floor" | "hash"), 1) => {
                    let op = match k {
                        "round" => Op::Round,
                        "ceil" => Op::Ceil,
                        "hash" => Op::Hash,
                        _ => Op::Floor,
                    };
                    V::Op(op, vec![parse_v(&args[0], field)?])
                }
                (k, n) => return err(field, format!("unknown operator {k} with {n} arguments")),
            }
        }
        _ => return err(field, "unsupported value"),
    })
}

fn param_refs(v: &V, out: &mut Vec<(String, String)>) {
    match v {
        V::Param(p, f) => out.push((p.clone(), f.clone())),
        V::Orient(a, b) | V::Scale(a, b) => {
            param_refs(a, out);
            param_refs(b, out);
        }
        V::Mix(a, b, c) => {
            param_refs(a, out);
            param_refs(b, out);
            param_refs(c, out);
        }
        V::Pick(a, _) => param_refs(a, out),
        V::ByState(a, b, c) => {
            param_refs(a, out);
            param_refs(b, out);
            param_refs(c, out);
        }
        V::ByOffset(list) | V::Op(_, list) => list.iter().for_each(|v| param_refs(v, out)),
        V::Choose(sel, cases) => {
            param_refs(sel, out);
            cases.iter().for_each(|(_, v)| param_refs(v, out));
        }
        _ => {}
    }
}

/// Evaluation context for values.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub params: &'a HashMap<String, PVal>,
    pub portrait: bool,
    pub word_index: usize,
    pub index: usize,
    pub index_in_word: usize,
    /// The drawn line's state in the window: PAST, ACTIVE or NEXT.
    pub line_state: u8,
    /// The drawn line's slot: 0 for the active slot, +1.. below, -1.. above.
    pub line_offset: i32,
    /// HOST_VARS values, in the same order.
    pub host: [f64; 8],
    /// WORD_VARS values, in the same order.
    pub word: [f64; 2],
    /// LINE_VARS values, in the same order.
    pub line: [f64; 1],
}

pub const PAST: u8 = 0;
pub const ACTIVE: u8 = 1;
pub const NEXT: u8 = 2;

#[derive(Clone, Debug)]
pub enum R {
    Num(f64),
    Color([u8; 3]),
    Text(String),
    Palette(Vec<[u8; 3]>),
}

fn eval(v: &V, c: &Ctx) -> R {
    match v {
        V::Num(n) => R::Num(*n),
        V::Color(k) => R::Color(*k),
        V::Text(s) => R::Text(s.clone()),
        V::Param(p, _) => match &c.params[p] {
            PVal::Num(n) => R::Num(*n),
            PVal::Color(k) => R::Color(*k),
            PVal::Text(s) => R::Text(s.clone()),
            PVal::Palette(p) => R::Palette(p.clone()),
            PVal::Bool(b) => R::Num(f64::from(u8::from(*b))),
        },
        V::Orient(l, p) => eval(if c.portrait { p } else { l }, c),
        V::Scale(x, k) => match (eval(x, c), eval(k, c)) {
            (R::Num(a), R::Num(b)) => R::Num(a * b),
            (a, _) => a,
        },
        V::Mix(a, b, t) => {
            let (a, b, t) = (col(&eval(a, c)), col(&eval(b, c)), num(&eval(t, c)));
            R::Color(lerp_color(a, b, t))
        }
        V::ByOffset(list) => eval(
            &list[(c.line_offset.unsigned_abs() as usize).min(list.len() - 1)],
            c,
        ),
        V::Host(i) => R::Num(c.host[*i]),
        V::Word(i) => R::Num(c.word[*i]),
        V::Line(i) => R::Num(c.line[*i]),
        V::Op(op, args) => {
            let n: Vec<f64> = args.iter().map(|a| num(&eval(a, c))).collect();
            let fold = |f: fn(f64, f64) -> f64| n[1..].iter().fold(n[0], |acc, x| f(acc, *x));
            let r = match op {
                Op::Add => fold(|a, b| a + b),
                Op::Sub => fold(|a, b| a - b),
                Op::Mul => fold(|a, b| a * b),
                // A zero divisor gives 0: values stay finite, whatever an effect asks.
                Op::Div => fold(|a, b| if b == 0.0 { 0.0 } else { a / b }),
                Op::Min => fold(f64::min),
                Op::Max => fold(f64::max),
                Op::Round => n[0].round(),
                Op::Ceil => n[0].ceil(),
                Op::Floor => n[0].floor(),
                Op::Hash => hash1(n[0]),
            };
            R::Num(if r.is_finite() { r } else { 0.0 })
        }
        V::Choose(sel, cases) => {
            let key = match eval(sel, c) {
                R::Text(t) => t,
                R::Num(n) => format!("{n}"),
                _ => String::new(),
            };
            match cases
                .iter()
                .find(|(k, _)| *k == key)
                .or_else(|| cases.iter().find(|(k, _)| k == "default"))
            {
                Some((_, v)) => eval(v, c),
                None => eval(&cases[0].1, c),
            }
        }
        V::ByState(past, active, next) => eval(
            match c.line_state {
                PAST => past,
                NEXT => next,
                _ => active,
            },
            c,
        ),
        V::Pick(p, scope) => {
            let i = match scope.as_str() {
                "word_index" => c.word_index,
                "index_in_word" => c.index_in_word,
                "line_index" => c.line[0] as usize,
                _ => c.index,
            };
            match eval(p, c) {
                R::Palette(p) if !p.is_empty() => R::Color(p[i % p.len()]),
                other => other,
            }
        }
    }
}

/// Per-channel sRGB blend, `t` 0 = a, 1 = b (CSS `color-mix(in srgb, …)`).
fn lerp_color(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let t = t as f32;
    let ch = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    [ch(a[0], b[0]), ch(a[1], b[1]), ch(a[2], b[2])]
}

fn num(r: &R) -> f64 {
    if let R::Num(n) = r { *n } else { 0.0 }
}

fn col(r: &R) -> [u8; 3] {
    if let R::Color(c) = r {
        *c
    } else {
        [255, 255, 255]
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn premul(c: [u8; 3], a: f32) -> [f32; 4] {
    let l = |x: u8| srgb_to_linear(x as f32 / 255.0) * a;
    [l(c[0]), l(c[1]), l(c[2]), a]
}

// ---------------------------------------------------------------- schema

#[derive(Clone, Debug)]
pub struct Param {
    pub id: String,
    pub kind: String,
    pub default: J,
    pub level: u8,
    /// Bounds of a number, and the allowed texts of a choice (empty: any).
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    /// Display names by language code (`label` of the manifest); empty: show the id.
    pub label: HashMap<String, String>,
    /// Display names of each choice by language code (`choice_labels`), in the
    /// order of `choices`; an empty map shows the choice's own text.
    pub choice_labels: Vec<HashMap<String, String>>,
}

impl Param {
    /// Whether `v` is a valid value for this parameter: its type, a choice among
    /// `choices`, a number within `min..=max` (or a landscape/portrait pair of them).
    pub fn accepts(&self, v: &J) -> bool {
        // Only a font parameter may name an installed family: an export reads the
        // installed fonts for font parameters only (an open choice would take it).
        if self.kind != "font" && names_installed_font(v) {
            return false;
        }
        let one = |v: &J| match self.kind.as_str() {
            "color" => v.as_str().and_then(hex).is_some(),
            "number" => v.as_f64().is_some_and(|n| {
                n.is_finite() && self.min.is_none_or(|m| n >= m) && self.max.is_none_or(|m| n <= m)
            }),
            "bool" => v.is_boolean(),
            "palette" => v.as_array().is_some_and(|a| {
                !a.is_empty() && a.iter().all(|c| c.as_str().and_then(hex).is_some())
            }),
            "choice" => v
                .as_str()
                .is_some_and(|s| self.choices.is_empty() || self.choices.iter().any(|c| c == s)),
            // A bundled face, or an installed family (`system:<Family>`).
            "font" => v.as_str().is_some_and(|s| {
                Face::named(s).is_some()
                    || s.strip_prefix("system:")
                        .is_some_and(|f| !f.trim().is_empty())
            }),
            _ => v.is_string(),
        };
        match v {
            J::Object(m) => {
                m.len() == 2
                    && m.get("landscape").is_some_and(one)
                    && m.get("portrait").is_some_and(one)
            }
            v => one(v),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Line,
    Word,
    /// A syllable of a word cut into syllables; the whole word otherwise.
    Syllable,
    Letter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Enter,
    Exit,
    Loop,
    Active,
    Past,
    Future,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prop {
    Opacity,
    Scale,
    ScaleX,
    OffsetX,
    OffsetY,
    Rotation,
    Blur,
    Color,
    Color2,
    Wipe,
    Glow,
    /// Letter spacing in em, line unit only (as CSS `letter-spacing`).
    Tracking,
    /// Share of the line block shown from its bottom up (1 = all), line unit only:
    /// CSS `clip-path: inset(top)` on the whole block, every row and pass alike.
    RevealY,
    /// Plate passes on the track's unit: opacity factor, margin added on every
    /// side (em), horizontal scale about the plate's centre, and the share of the
    /// plate drawn from its left edge (1 = all).
    PlateOpacity,
    PlateMargin,
    PlateScaleX,
    PlateWipe,
    /// Width of the wipe's edge fade in em (0 = the engine's hard edge).
    WipeEdge,
}

impl Prop {
    fn is_plate(self) -> bool {
        matches!(
            self,
            Prop::PlateOpacity | Prop::PlateMargin | Prop::PlateScaleX | Prop::PlateWipe
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Back,
    /// The power curves x³ and 1 − (1 − x)³ ("easeInCubic" / "easeOutCubic" in the
    /// usual easing tables). Not the CSS ease-in / ease-out keywords (cubic-bezier).
    EaseInCubic,
    EaseOutCubic,
}

#[derive(Clone, Debug)]
enum Shape {
    Keys(Vec<(f64, V)>, Curve),
    Wave {
        amplitude: V,
        frequency: f64,
        phase_per_unit: f64,
        phase: f64,
    },
    Noise {
        amplitude: V,
        frequency: f64,
        seed: u64,
    },
    Const(V),
}

#[derive(Clone, Debug)]
struct Track {
    unit: Unit,
    slot: Slot,
    prop: Prop,
    duration: f64,
    stagger: f64,
    shape: Shape,
    /// A past track with its own `duration`: its keys run over that time from
    /// the moment the unit becomes past (else they read their first key).
    timed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Anchor {
    Unit,
    Word,
    WordXGlyphY,
    Baseline,
    Pivot,
}

#[derive(Clone, Debug)]
struct BoxSpec {
    x: V,
    y: V,
    width: V,
    anchor: V,
    space: Space,
}

/// Which rectangle fractional positions refer to: the safe surface the host
/// gives (the Short's safe area, else the whole canvas) or the whole canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Safe,
    Canvas,
}

fn parse_space(o: &serde_json::Map<String, J>, field: &str) -> Result<Space, LoadError> {
    match get_s(o, "space", "safe").as_str() {
        "safe" => Ok(Space::Safe),
        "canvas" => Ok(Space::Canvas),
        s => err(
            &format!("{field}.space"),
            format!("{s:?} unknown (safe, canvas)"),
        ),
    }
}

#[derive(Clone, Debug)]
struct Role {
    font: V,
    size: V,
    case: String,
    rotation: V,
    color: V,
    alpha: V,
    outline: V,
    align: String,
    require: Option<String>,
    pivot: Option<(V, V)>,
    /// The space of `pivot`.
    space: Space,
}

#[derive(Clone, Debug)]
struct SlotSpec {
    x: V,
    y: V,
    style: Option<Role>,
    space: Space,
}

#[derive(Clone, Debug)]
struct ShapeSpec {
    x0: V,
    y0: V,
    x1: V,
    y1: V,
    color: V,
    alpha: V,
    space: Space,
}

#[derive(Clone, Debug)]
struct Compose {
    rank: Vec<String>,
    stopwords: Vec<String>,
    carrier: Role,
    second: Role,
    others: Role,
    slots: V2<Vec<SlotSpec>>,
    shapes: Vec<ShapeSpec>,
    /// Validated but not used yet: the engine draws the roles in a fixed order.
    #[allow(dead_code)]
    order: Vec<String>,
}

/// A per-orientation pair of structured lists (slots differ by orientation).
#[derive(Clone, Debug)]
struct V2<T> {
    landscape: T,
    portrait: T,
}

#[derive(Clone, Debug)]
struct Layout {
    /// Validated but not read back: the layout fields present imply the mode.
    #[allow(dead_code)]
    mode: String,
    font: V,
    size: V,
    case: String,
    line_height: V,
    bx: BoxSpec,
    raster: V,
    visible: String,
    anchor: Anchor,
    compose: Option<Compose>,
    /// Lines shown around the active one; None shows the active line only.
    window: Option<Window>,
    /// `layout.line_breaks`: "keep" draws the lines typed in a cue as they are
    /// (each still wraps in the box); "rewrap" (the default) joins them.
    keep_breaks: bool,
    /// `layout.word_lead` (seconds): words and syllables read as starting and
    /// ending this much earlier, so a highlight meets the voice. Line times keep.
    word_lead: f64,
    /// `layout.preroll` (seconds): a line shows this much before its start, its
    /// line `enter` running from there, never cutting the previous line short.
    preroll: f64,
    /// `layout.hold` (seconds): after its end a line stays until the next one
    /// shows, at most this long; its line `exit` runs at the end of that span.
    hold: f64,
    /// `layout.chunk`: cues cut into groups of words that are each drawn as a line.
    chunk: Option<Chunk>,
    /// `layout.word_size`: a size factor per word (read with the word values,
    /// e.g. `@word_longest`), laid out so the row reflows. Line mode only.
    word_size: Option<V>,
    /// `layout.align`: rows centred in the box, or set against its left or right
    /// edge. Line mode only.
    align: RowAlign,
}

/// `layout.chunk`: each cue sliced in order into groups of `max` words (the last
/// one shorter); `rows_word` puts each word of a group on its own row.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Chunk {
    max: usize,
    rows_word: bool,
}

/// `layout.window`: past and upcoming lines around the active one. An upcoming
/// line shows `lead` seconds before it starts; lines stack `gap` (in the active
/// line's font size) apart, past lines above, upcoming lines below.
#[derive(Clone, Debug)]
struct Window {
    before: usize,
    after: usize,
    lead: f64,
    gap: f32,
    /// `glide`: when the active slot changes, the lines move to their new place
    /// over `duration` seconds along `curve` instead of jumping there.
    glide: Option<(f64, Curve)>,
}

#[derive(Clone, Debug)]
struct Pass {
    kind: String,
    color: V,
    color2: Option<V>,
    alpha: V,
    /// Opacity of the wipe's second colour (the sung part); `alpha` when absent.
    alpha2: Option<V>,
    offset_y: V,
    blur: V,
    outline: V,
    gain: V,
    /// A plate pass: the unit it is measured on (line rows, words or letters),
    /// its margins and corner radius in em.
    plate: Option<Plate>,
}

#[derive(Clone, Debug)]
struct Plate {
    on: Unit,
    margin_x: V,
    margin_y: V,
    radius: V,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub id: String,
    pub params: Vec<Param>,
    pub presets: Vec<(String, HashMap<String, J>)>,
    /// Display names by language code (`name` of the manifest), and one map per preset.
    pub names: HashMap<String, String>,
    pub preset_names: Vec<HashMap<String, String>>,
    layout: Layout,
    tracks: Vec<Track>,
    passes: Vec<Pass>,
    pub shader: Option<String>,
    pub load_ms: f64,
}

/// A `{"fr": "...", "en": "..."}` name; absent means no name (the id is shown).
fn names(j: Option<&J>, field: &str) -> Result<HashMap<String, String>, LoadError> {
    let Some(j) = j else {
        return Ok(HashMap::new());
    };
    let o = obj(j, field)?;
    o.iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|s| (k.clone(), s.to_string()))
                .ok_or_else(|| LoadError(format!("{field}.{k}: expected a string")))
        })
        .collect()
}

/// The name for `lang`, else English, else French, else any, else `fallback`.
pub fn localized<'a>(names: &'a HashMap<String, String>, lang: &str, fallback: &'a str) -> &'a str {
    [lang, "en", "fr"]
        .iter()
        .find_map(|k| names.get(*k))
        .or_else(|| names.values().min())
        .map_or(fallback, String::as_str)
}

fn obj<'a>(j: &'a J, field: &str) -> Result<&'a serde_json::Map<String, J>, LoadError> {
    j.as_object()
        .ok_or_else(|| LoadError(format!("{field}: expected an object")))
}

fn only(o: &serde_json::Map<String, J>, field: &str, allowed: &[&str]) -> Result<(), LoadError> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return err(field, format!("unknown field {k:?}"));
        }
    }
    Ok(())
}

fn get_v(o: &serde_json::Map<String, J>, field: &str, k: &str, default: J) -> Result<V, LoadError> {
    parse_v(o.get(k).unwrap_or(&default), &format!("{field}.{k}"))
}

fn get_s(o: &serde_json::Map<String, J>, k: &str, default: &str) -> String {
    o.get(k).and_then(J::as_str).unwrap_or(default).to_string()
}

fn parse_role(j: &J, field: &str) -> Result<Role, LoadError> {
    let o = obj(j, field)?;
    only(
        o,
        field,
        &[
            "font", "size", "case", "rotation", "color", "alpha", "outline", "align", "require",
            "pivot", "space",
        ],
    )?;
    let pivot = match o.get("pivot") {
        None => None,
        Some(J::Object(p)) if !p.contains_key("landscape") && !p.contains_key("portrait") => {
            only(p, &format!("{field}.pivot"), &["x", "y"])?;
            let (Some(x), Some(y)) = (p.get("x"), p.get("y")) else {
                return err(&format!("{field}.pivot"), "needs both x and y");
            };
            Some((
                parse_v(x, &format!("{field}.pivot.x"))?,
                parse_v(y, &format!("{field}.pivot.y"))?,
            ))
        }
        Some(J::Array(a)) if a.len() == 2 => Some((parse_v(&a[0], field)?, parse_v(&a[1], field)?)),
        Some(other) => {
            // orientation-dependent pivot: {"landscape": [x,y], "portrait": [x,y]}
            let p = obj(other, &format!("{field}.pivot"))?;
            only(p, &format!("{field}.pivot"), &["landscape", "portrait"])?;
            let pick = |k: &str| -> Result<(V, V), LoadError> {
                let a = p
                    .get(k)
                    .and_then(J::as_array)
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| LoadError(format!("{field}.pivot.{k}: expected [x, y]")))?;
                Ok((parse_v(&a[0], field)?, parse_v(&a[1], field)?))
            };
            let (l, pt) = (pick("landscape")?, pick("portrait")?);
            Some((
                V::Orient(Box::new(l.0), Box::new(pt.0)),
                V::Orient(Box::new(l.1), Box::new(pt.1)),
            ))
        }
    };
    let align = get_s(o, "align", "center");
    if !matches!(align.as_str(), "center" | "left") {
        return err(field, format!("align {align:?} unknown"));
    }
    let require = o.get("require").and_then(J::as_str).map(str::to_string);
    if let Some(r) = &require {
        if r != "content_word" {
            return err(field, format!("require {r:?} unknown"));
        }
    }
    Ok(Role {
        space: parse_space(o, field)?,
        font: {
            if o.get("font").is_some_and(names_installed_font) {
                return Err(LoadError(format!(
                    "{field}.font: an installed font is chosen through a font parameter, not named here"
                )));
            }
            get_v(o, field, "font", J::from("sans"))?
        },
        size: get_v(o, field, "size", J::from(0.05))?,
        case: get_s(o, "case", "none"),
        rotation: get_v(o, field, "rotation", J::from(0.0))?,
        color: get_v(o, field, "color", J::from("#FFFFFF"))?,
        alpha: get_v(o, field, "alpha", J::from(1.0))?,
        outline: get_v(o, field, "outline", J::from(0.0))?,
        align,
        require,
        pivot,
    })
}

fn parse_slots(j: &J, field: &str) -> Result<Vec<SlotSpec>, LoadError> {
    let arr = j
        .as_array()
        .ok_or_else(|| LoadError(format!("{field}: expected an array")))?;
    arr.iter()
        .enumerate()
        .map(|(i, s)| {
            let f = format!("{field}[{i}]");
            let o = obj(s, &f)?;
            only(o, &f, &["x", "y", "style", "space"])?;
            Ok(SlotSpec {
                space: parse_space(o, &f)?,
                x: get_v(o, &f, "x", J::from(0.0))?,
                y: get_v(o, &f, "y", J::from(0.0))?,
                style: o
                    .get("style")
                    .map(|s| parse_role(s, &format!("{f}.style")))
                    .transpose()?,
            })
        })
        .collect()
}

fn parse_keys(j: &J, field: &str) -> Result<Vec<(f64, V)>, LoadError> {
    let arr = j
        .as_array()
        .ok_or_else(|| LoadError(format!("{field}: expected [[t, v], ...]")))?;
    if arr.is_empty() {
        return err(field, "empty keyframes");
    }
    let mut out = Vec::new();
    for (i, k) in arr.iter().enumerate() {
        let pair = k
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(|| LoadError(format!("{field}[{i}]: expected [t, v]")))?;
        let t = pair[0]
            .as_f64()
            .ok_or_else(|| LoadError(format!("{field}[{i}]: t must be a number")))?;
        if !(0.0..=1.0).contains(&t) {
            return err(&format!("{field}[{i}]"), "t outside 0..1");
        }
        out.push((t, parse_v(&pair[1], &format!("{field}[{i}]"))?));
    }
    Ok(out)
}

impl Effect {
    /// Load and validate a package. Every message starts with the file it concerns.
    pub fn load(dir: &Path) -> Result<Self, LoadError> {
        Self::load_inner(dir).map_err(|e| {
            if e.0.starts_with("effect.json")
                || e.0.starts_with("shader ")
                || e.0.contains(":\\")
                || e.0.contains(":/")
            {
                e
            } else {
                LoadError(format!("effect.json › {}", e.0))
            }
        })
    }

    fn load_inner(dir: &Path) -> Result<Self, LoadError> {
        let started = Instant::now();
        let path = dir.join("effect.json");
        let size = std::fs::metadata(&path)
            .map_err(|e| LoadError(format!("{}: {e}", path.display())))?
            .len();
        if size > MAX_JSON as u64 {
            return err("effect.json", format!("{size} bytes exceeds {MAX_JSON}"));
        }
        let bytes =
            std::fs::read(&path).map_err(|e| LoadError(format!("{}: {e}", path.display())))?;
        Self::from_bytes(dir, &bytes, started)
    }

    /// An effect.json already read (or built into NOH: `catalog::builtin_classic`);
    /// a shader is read from `dir`.
    pub(crate) fn from_json(dir: &Path, bytes: &[u8]) -> Result<Self, LoadError> {
        Self::from_bytes(dir, bytes, Instant::now())
    }

    fn from_bytes(dir: &Path, bytes: &[u8], started: Instant) -> Result<Self, LoadError> {
        if bytes.len() > MAX_JSON {
            return err(
                "effect.json",
                format!("{} bytes exceeds {MAX_JSON}", bytes.len()),
            );
        }
        let root: J =
            serde_json::from_slice(bytes).map_err(|e| LoadError(format!("effect.json: {e}")))?;
        let o = obj(&root, "effect.json")?;
        only(
            o,
            "effect.json",
            &[
                "engine", "id", "name", "version", "license", "target", "params", "presets",
                "layout", "animate", "render", "shader",
            ],
        )?;
        match o.get("engine").and_then(J::as_u64) {
            Some(ENGINE_API) => {}
            other => {
                return err(
                    "engine",
                    format!(
                        "API version {} is not supported (this engine: {ENGINE_API})",
                        other.map_or("absent".to_string(), |v| v.to_string())
                    ),
                );
            }
        }
        if o.get("target").and_then(J::as_str) != Some("text") {
            return err("target", "only \"text\" is supported");
        }
        let id = get_s(o, "id", "");
        if id.is_empty() {
            return err("id", "missing");
        }
        // params
        let mut params = Vec::new();
        for (i, p) in o
            .get("params")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let f = format!("params[{i}]");
            let po = obj(p, &f)?;
            only(
                po,
                &f,
                &[
                    "id",
                    "type",
                    "default",
                    "level",
                    "label",
                    "min",
                    "max",
                    "unit",
                    "choices",
                    "choice_labels",
                ],
            )?;
            let kind = get_s(po, "type", "");
            if !matches!(
                kind.as_str(),
                "color" | "number" | "choice" | "bool" | "font" | "palette"
            ) {
                return err(&format!("{f}.type"), format!("unknown type {kind:?}"));
            }
            let level = po.get("level").and_then(J::as_u64).unwrap_or(2) as u8;
            if !(1..=2).contains(&level) {
                return err(&format!("{f}.level"), "must be 1 or 2");
            }
            let default = po
                .get("default")
                .cloned()
                .ok_or_else(|| LoadError(format!("{f}.default: missing")))?;
            // Only a font parameter may name an installed family (an export reads
            // the installed fonts for it before drawing): never another type's
            // default or choices.
            if kind != "font"
                && (names_installed_font(&default)
                    || po.get("choices").is_some_and(names_installed_font))
            {
                return err(
                    &format!("{f}"),
                    "an installed font is chosen through a font parameter only",
                );
            }
            // A font's default is drawn when an installed one is missing: it must
            // be a face NOH ships.
            if kind == "font" && default.as_str().and_then(Face::named).is_none() {
                return err(
                    &format!("{f}.default"),
                    "a bundled face (sans, jost_extrabold, …), not an installed font",
                );
            }
            let choices: Vec<String> = po
                .get("choices")
                .and_then(J::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let label = names(po.get("label"), &format!("{f}.label"))?;
            let mut choice_labels = vec![HashMap::new(); choices.len()];
            if let Some(labels) = po.get("choice_labels") {
                let field = format!("{f}.choice_labels");
                for (k, v) in obj(labels, &field)? {
                    let Some(i) = choices.iter().position(|c| c == k) else {
                        return err(&field, format!("{k:?} is not one of its choices"));
                    };
                    choice_labels[i] = names(Some(v), &format!("{field}.{k}"))?;
                }
            }
            let param = Param {
                id: get_s(po, "id", ""),
                kind,
                default,
                level,
                min: po.get("min").and_then(J::as_f64),
                max: po.get("max").and_then(J::as_f64),
                choices,
                label,
                choice_labels,
            };
            // type check the default (or both orientation defaults)
            let check = |v: &J| -> bool {
                match param.kind.as_str() {
                    "color" => v.as_str().and_then(hex).is_some(),
                    "number" => v.is_number(),
                    "bool" => v.is_boolean(),
                    "palette" => v.as_array().is_some_and(|a| {
                        !a.is_empty() && a.iter().all(|c| c.as_str().and_then(hex).is_some())
                    }),
                    _ => v.is_string(),
                }
            };
            let ok = match &param.default {
                J::Object(m) => {
                    m.len() == 2
                        && m.get("landscape").is_some_and(check)
                        && m.get("portrait").is_some_and(check)
                }
                v => check(v),
            };
            if !ok {
                return err(
                    &format!("{f}.default"),
                    format!("does not match type {}", param.kind),
                );
            }
            params.push(param);
        }
        if params.len() > MAX_PARAMS {
            return err(
                "params",
                format!("{} params exceed {MAX_PARAMS}", params.len()),
            );
        }
        let level1 = params.iter().filter(|p| p.level == 1).count();
        if level1 > MAX_LEVEL1 {
            return err(
                "params",
                format!("{level1} level-1 params exceed {MAX_LEVEL1}"),
            );
        }
        let ids: Vec<String> = params.iter().map(|p| p.id.clone()).collect();
        // presets
        let mut presets = Vec::new();
        let mut preset_names = Vec::new();
        for (i, p) in o
            .get("presets")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let f = format!("presets[{i}]");
            let po = obj(p, &f)?;
            only(po, &f, &["id", "name", "values"])?;
            let values: HashMap<String, J> = po
                .get("values")
                .and_then(J::as_object)
                .map(|m| m.clone().into_iter().collect())
                .unwrap_or_default();
            for (k, v) in &values {
                if !ids.contains(k) {
                    return err(&format!("{f}.values"), format!("unknown param {k:?}"));
                }
                let font = params.iter().any(|p| &p.id == k && p.kind == "font");
                if !font && names_installed_font(v) {
                    return err(
                        &format!("{f}.values.{k}"),
                        "an installed font is chosen through a font parameter only",
                    );
                }
            }
            preset_names.push(names(po.get("name"), &format!("{f}.name"))?);
            presets.push((get_s(po, "id", ""), values));
        }
        // layout
        let lj = o
            .get("layout")
            .ok_or_else(|| LoadError("layout: missing".into()))?;
        let lo = obj(lj, "layout")?;
        only(
            lo,
            "layout",
            &[
                "mode",
                "font",
                "size",
                "case",
                "line_height",
                "box",
                "raster",
                "visible",
                "anchor",
                "reserve",
                "compose",
                "window",
                "line_breaks",
                "word_lead",
                "preroll",
                "hold",
                "chunk",
                "word_size",
                "align",
            ],
        )?;
        let keep_breaks = match get_s(lo, "line_breaks", "rewrap").as_str() {
            "keep" => true,
            "rewrap" => false,
            b => {
                return err(
                    "layout.line_breaks",
                    format!("{b:?} unknown (keep, rewrap)"),
                );
            }
        };
        let mode = get_s(lo, "mode", "line");
        if !matches!(mode.as_str(), "line" | "compose") {
            return err("layout.mode", format!("{mode:?} unknown (line, compose)"));
        }
        let bo = lo.get("box").map(|b| obj(b, "layout.box")).transpose()?;
        let empty = serde_json::Map::new();
        let bo = bo.unwrap_or(&empty);
        only(bo, "layout.box", &["x", "y", "width", "anchor", "space"])?;
        let bx = BoxSpec {
            space: parse_space(bo, "layout.box")?,
            x: get_v(bo, "layout.box", "x", J::from(0.5))?,
            y: get_v(bo, "layout.box", "y", J::from(0.88))?,
            width: get_v(bo, "layout.box", "width", J::from(0.86))?,
            anchor: get_v(bo, "layout.box", "anchor", J::from("bottom"))?,
        };
        let anchor = match get_s(lo, "anchor", "unit").as_str() {
            "unit" => Anchor::Unit,
            "word" => Anchor::Word,
            "word_x_glyph_y" => Anchor::WordXGlyphY,
            "baseline" => Anchor::Baseline,
            "pivot" => Anchor::Pivot,
            a => return err("layout.anchor", format!("{a:?} unknown")),
        };
        let visible = get_s(lo, "visible", "line");
        if !matches!(visible.as_str(), "line" | "from_start") {
            return err("layout.visible", format!("{visible:?} unknown"));
        }
        let compose = match lo.get("compose") {
            None => None,
            Some(c) => {
                let co = obj(c, "layout.compose")?;
                only(
                    co,
                    "layout.compose",
                    &["rank", "stopwords", "roles", "slots", "shapes", "order"],
                )?;
                let rank: Vec<String> = co
                    .get("rank")
                    .and_then(J::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                for k in &rank {
                    if !matches!(k.as_str(), "content_word" | "duration" | "length") {
                        return err("layout.compose.rank", format!("key {k:?} unknown"));
                    }
                }
                let roles = obj(
                    co.get("roles")
                        .ok_or_else(|| LoadError("layout.compose.roles: missing".into()))?,
                    "layout.compose.roles",
                )?;
                only(
                    roles,
                    "layout.compose.roles",
                    &["carrier", "second", "others"],
                )?;
                let role = |k: &str| -> Result<Role, LoadError> {
                    parse_role(
                        roles.get(k).ok_or_else(|| {
                            LoadError(format!("layout.compose.roles.{k}: missing"))
                        })?,
                        &format!("layout.compose.roles.{k}"),
                    )
                };
                let so = obj(
                    co.get("slots")
                        .ok_or_else(|| LoadError("layout.compose.slots: missing".into()))?,
                    "layout.compose.slots",
                )?;
                only(so, "layout.compose.slots", &["landscape", "portrait"])?;
                let slots = V2 {
                    landscape: parse_slots(
                        so.get("landscape").unwrap_or(&J::Array(vec![])),
                        "layout.compose.slots.landscape",
                    )?,
                    portrait: parse_slots(
                        so.get("portrait").unwrap_or(&J::Array(vec![])),
                        "layout.compose.slots.portrait",
                    )?,
                };
                let mut shapes = Vec::new();
                for (i, s) in co
                    .get("shapes")
                    .and_then(J::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let f = format!("layout.compose.shapes[{i}]");
                    let so = obj(s, &f)?;
                    only(so, &f, &["x0", "y0", "x1", "y1", "color", "alpha", "space"])?;
                    shapes.push(ShapeSpec {
                        space: parse_space(so, &f)?,
                        x0: get_v(so, &f, "x0", J::from(0.0))?,
                        y0: get_v(so, &f, "y0", J::from(0.0))?,
                        x1: get_v(so, &f, "x1", J::from(1.0))?,
                        y1: get_v(so, &f, "y1", J::from(1.0))?,
                        color: get_v(so, &f, "color", J::from("#000000"))?,
                        alpha: get_v(so, &f, "alpha", J::from(1.0))?,
                    });
                }
                Some(Compose {
                    rank,
                    stopwords: co
                        .get("stopwords")
                        .and_then(J::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    carrier: role("carrier")?,
                    second: role("second")?,
                    others: role("others")?,
                    slots,
                    shapes,
                    order: co
                        .get("order")
                        .and_then(J::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_else(|| {
                            vec![
                                "shapes".into(),
                                "carrier".into(),
                                "second".into(),
                                "others".into(),
                            ]
                        }),
                })
            }
        };
        if mode == "compose" && compose.is_none() {
            return err("layout.compose", "missing for mode compose");
        }
        let window = match lo.get("window") {
            None => None,
            Some(w) => {
                if mode == "compose" {
                    return err("layout.window", "line mode only");
                }
                let wo = obj(w, "layout.window")?;
                only(
                    wo,
                    "layout.window",
                    &["before", "after", "lead", "gap", "glide"],
                )?;
                let count = |k: &str| -> Result<usize, LoadError> {
                    match wo.get(k) {
                        None => Ok(0),
                        Some(v) => v
                            .as_u64()
                            .filter(|n| *n <= 3)
                            .map(|n| n as usize)
                            .ok_or_else(|| {
                                LoadError(format!("layout.window.{k}: a whole number from 0 to 3"))
                            }),
                    }
                };
                let number = |k: &str, default: f64, max: f64| -> Result<f64, LoadError> {
                    match wo.get(k) {
                        None => Ok(default),
                        Some(v) => v
                            .as_f64()
                            .filter(|n| n.is_finite() && *n >= 0.0 && *n <= max)
                            .ok_or_else(|| {
                                LoadError(format!("layout.window.{k}: a number from 0 to {max}"))
                            }),
                    }
                };
                let glide = match wo.get("glide") {
                    None => None,
                    Some(g) => {
                        let f = "layout.window.glide";
                        let go = obj(g, f)?;
                        only(go, f, &["duration", "curve"])?;
                        let duration = go
                            .get("duration")
                            .and_then(J::as_f64)
                            .filter(|n| n.is_finite() && *n > 0.0 && *n <= 2.0)
                            .ok_or_else(|| {
                                LoadError(format!("{f}.duration: a number above 0, at most 2"))
                            })?;
                        Some((
                            duration,
                            parse_curve(&get_s(go, "curve", "linear"), &format!("{f}.curve"))?,
                        ))
                    }
                };
                Some(Window {
                    before: count("before")?,
                    after: count("after")?,
                    lead: number("lead", 0.0, 10.0)?,
                    gap: number("gap", 0.3, 4.0)? as f32,
                    glide,
                })
            }
        };
        let seconds = |k: &str, max: f64| -> Result<f64, LoadError> {
            match lo.get(k) {
                None => Ok(0.0),
                Some(v) => v
                    .as_f64()
                    .filter(|n| n.is_finite() && (0.0..=max).contains(n))
                    .ok_or_else(|| LoadError(format!("layout.{k}: a number from 0 to {max}"))),
            }
        };
        let word_lead = seconds("word_lead", 0.3)?;
        let chunk = match lo.get("chunk") {
            None => None,
            Some(c) => {
                if mode == "compose" {
                    return err("layout.chunk", "line mode only");
                }
                // A chunk's breaks are its own rows: the cue's typed lines would be lost.
                if keep_breaks {
                    return err("layout.chunk", "not with line_breaks keep");
                }
                let co = obj(c, "layout.chunk")?;
                only(co, "layout.chunk", &["max", "rows"])?;
                let max = co
                    .get("max")
                    .and_then(J::as_u64)
                    .filter(|n| (2..=4).contains(n))
                    .ok_or_else(|| {
                        LoadError("layout.chunk.max: a whole number from 2 to 4".into())
                    })? as usize;
                let rows_word = match get_s(co, "rows", "flow").as_str() {
                    "word" => true,
                    "flow" => false,
                    r => return err("layout.chunk.rows", format!("{r:?} unknown (word, flow)")),
                };
                Some(Chunk { max, rows_word })
            }
        };
        let preroll = seconds("preroll", 1.0)?;
        let hold = seconds("hold", 10.0)?;
        let word_size = match lo.get("word_size") {
            None => None,
            Some(_) if mode == "compose" => {
                return err("layout.word_size", "line mode only");
            }
            Some(j) => Some(parse_v(j, "layout.word_size")?),
        };
        if mode == "compose" && lo.contains_key("align") {
            return err("layout.align", "line mode only");
        }
        let align = match get_s(lo, "align", "center").as_str() {
            "center" => RowAlign::Center,
            "left" => RowAlign::Left,
            "right" => RowAlign::Right,
            a => {
                return err(
                    "layout.align",
                    format!("{a:?} unknown (center, left, right)"),
                );
            }
        };
        let layout = Layout {
            mode,
            font: {
                if lo.get("font").is_some_and(names_installed_font) {
                    return Err(LoadError(
                        "layout.font: an installed font is chosen through a font parameter, not named here"
                            .into(),
                    ));
                }
                get_v(lo, "layout", "font", J::from("sans"))?
            },
            size: get_v(lo, "layout", "size", J::from(64))?,
            case: get_s(lo, "case", "none"),
            line_height: get_v(lo, "layout", "line_height", J::from(1.18))?,
            bx,
            raster: get_v(lo, "layout", "raster", J::from(1.0))?,
            visible,
            anchor,
            compose,
            window,
            keep_breaks,
            word_lead,
            preroll,
            hold,
            chunk,
            word_size,
            align,
        };
        // tracks
        let tracks_j = o
            .get("animate")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default();
        if tracks_j.len() > MAX_TRACKS {
            return err(
                "animate",
                format!("{} tracks exceed {MAX_TRACKS}", tracks_j.len()),
            );
        }
        let mut tracks = Vec::new();
        for (i, t) in tracks_j.iter().enumerate() {
            let f = format!("animate[{i}]");
            let to = obj(t, &f)?;
            only(
                to,
                &f,
                &[
                    "unit", "slot", "property", "duration", "stagger", "keys", "curve", "wave",
                    "noise", "value",
                ],
            )?;
            let unit = match get_s(to, "unit", "word").as_str() {
                "line" => Unit::Line,
                "word" => Unit::Word,
                "syllable" => Unit::Syllable,
                "letter" => Unit::Letter,
                u => return err(&format!("{f}.unit"), format!("{u:?} unknown")),
            };
            let slot = match get_s(to, "slot", "").as_str() {
                "enter" => Slot::Enter,
                "exit" => Slot::Exit,
                "loop" => Slot::Loop,
                "active" => Slot::Active,
                "past" => Slot::Past,
                "future" => Slot::Future,
                s => return err(&format!("{f}.slot"), format!("{s:?} unknown")),
            };
            let prop = match get_s(to, "property", "").as_str() {
                "opacity" => Prop::Opacity,
                "scale" => Prop::Scale,
                "scale_x" => Prop::ScaleX,
                "offset_x" => Prop::OffsetX,
                "offset_y" => Prop::OffsetY,
                "rotation" => Prop::Rotation,
                "blur" => Prop::Blur,
                "color" => Prop::Color,
                "color2" => Prop::Color2,
                "wipe" => Prop::Wipe,
                "glow" => Prop::Glow,
                "tracking" => Prop::Tracking,
                "reveal_y" => Prop::RevealY,
                "plate_opacity" => Prop::PlateOpacity,
                "plate_margin" => Prop::PlateMargin,
                "plate_scale_x" => Prop::PlateScaleX,
                "plate_wipe" => Prop::PlateWipe,
                "wipe_edge" => Prop::WipeEdge,
                p => return err(&format!("{f}.property"), format!("{p:?} unknown")),
            };
            // What the line unit draws, and what only the line unit draws: any other
            // pairing would be folded and never reach a letter.
            let line_props = [
                Prop::Opacity,
                Prop::OffsetX,
                Prop::OffsetY,
                Prop::Blur,
                Prop::Tracking,
                Prop::RevealY,
                Prop::Scale,
                Prop::ScaleX,
                Prop::Rotation,
            ];
            if prop == Prop::WipeEdge && unit == Unit::Line {
                return err(
                    &format!("{f}.property"),
                    "wipe_edge is on the word, syllable or letter unit",
                );
            }
            if unit == Unit::Line && !line_props.contains(&prop) && !prop.is_plate() {
                return err(
                    &format!("{f}.property"),
                    "not drawn on the line unit (opacity, offset_x, offset_y, blur, tracking, reveal_y, scale, scale_x, rotation, plate_*)",
                );
            }
            // Plates are measured on line rows, words or letters (render.passes on).
            if prop.is_plate() && unit == Unit::Syllable {
                return err(
                    &format!("{f}.property"),
                    "plate properties are on the line, word or letter unit",
                );
            }
            // Compose lays each word out alone, with no line em or block: line offsets,
            // letter spacing and the reveal clip would draw nothing there.
            if unit == Unit::Line
                && layout.compose.is_some()
                && matches!(
                    prop,
                    Prop::OffsetX
                        | Prop::OffsetY
                        | Prop::Tracking
                        | Prop::RevealY
                        | Prop::Scale
                        | Prop::ScaleX
                        | Prop::Rotation
                )
            {
                return err(
                    &format!("{f}.property"),
                    "not drawn on the line unit in compose mode (opacity, blur)",
                );
            }
            if unit != Unit::Line && matches!(prop, Prop::Tracking | Prop::RevealY) {
                return err(
                    &format!("{f}.property"),
                    format!("{:?} is a line unit property", get_s(to, "property", "")),
                );
            }
            let curve = parse_curve(&get_s(to, "curve", "linear"), &format!("{f}.curve"))?;
            let shape = if let Some(k) = to.get("keys") {
                Shape::Keys(parse_keys(k, &format!("{f}.keys"))?, curve)
            } else if let Some(w) = to.get("wave") {
                let wo = obj(w, &format!("{f}.wave"))?;
                only(
                    wo,
                    &format!("{f}.wave"),
                    &["amplitude", "frequency", "phase_per_unit", "phase"],
                )?;
                Shape::Wave {
                    amplitude: get_v(wo, &f, "amplitude", J::from(0.0))?,
                    frequency: wo.get("frequency").and_then(J::as_f64).unwrap_or(1.0),
                    phase_per_unit: wo.get("phase_per_unit").and_then(J::as_f64).unwrap_or(0.0),
                    phase: wo.get("phase").and_then(J::as_f64).unwrap_or(0.0),
                }
            } else if let Some(n) = to.get("noise") {
                let no = obj(n, &format!("{f}.noise"))?;
                only(
                    no,
                    &format!("{f}.noise"),
                    &["amplitude", "frequency", "seed"],
                )?;
                Shape::Noise {
                    amplitude: get_v(no, &f, "amplitude", J::from(0.0))?,
                    frequency: no.get("frequency").and_then(J::as_f64).unwrap_or(8.0),
                    seed: no.get("seed").and_then(J::as_u64).unwrap_or(1),
                }
            } else if let Some(v) = to.get("value") {
                Shape::Const(parse_v(v, &format!("{f}.value"))?)
            } else {
                return err(&f, "needs keys, wave, noise or value");
            };
            // A past track given its own duration times its keys from the unit's end.
            let timed = slot == Slot::Past && to.contains_key("duration");
            let duration = to.get("duration").and_then(J::as_f64).unwrap_or(0.3);
            if timed && !(duration.is_finite() && duration > 0.0) {
                return err(
                    &format!("{f}.duration"),
                    "a past track's duration must be above 0",
                );
            }
            tracks.push(Track {
                unit,
                slot,
                prop,
                duration,
                stagger: to.get("stagger").and_then(J::as_f64).unwrap_or(0.0),
                shape,
                timed,
            });
        }
        // passes
        let ro = obj(
            o.get("render")
                .ok_or_else(|| LoadError("render: missing".into()))?,
            "render",
        )?;
        only(ro, "render", &["passes"])?;
        let mut passes = Vec::new();
        for (i, p) in ro
            .get("passes")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let f = format!("render.passes[{i}]");
            let po = obj(p, &f)?;
            only(
                po,
                &f,
                &[
                    "kind", "color", "color2", "alpha", "alpha2", "offset_y", "blur", "outline",
                    "gain", "on", "margin_x", "margin_y", "radius",
                ],
            )?;
            let kind = get_s(po, "kind", "");
            if !matches!(
                kind.as_str(),
                "fill" | "shadow" | "glow" | "outline_only" | "plate"
            ) {
                return err(&format!("{f}.kind"), format!("{kind:?} unknown"));
            }
            let plate = if kind == "plate" {
                if layout.compose.is_some() {
                    return err(&format!("{f}.kind"), "plate passes are line mode only");
                }
                for k in ["color2", "alpha2", "offset_y", "blur", "outline", "gain"] {
                    if po.contains_key(k) {
                        return err(&format!("{f}.{k}"), "not a plate field");
                    }
                }
                let on = match get_s(po, "on", "line").as_str() {
                    "line" => Unit::Line,
                    "word" => Unit::Word,
                    "letter" => Unit::Letter,
                    u => {
                        return err(
                            &format!("{f}.on"),
                            format!("{u:?} unknown (line, word, letter)"),
                        );
                    }
                };
                Some(Plate {
                    on,
                    margin_x: get_v(po, &f, "margin_x", J::from(0.0))?,
                    margin_y: get_v(po, &f, "margin_y", J::from(0.0))?,
                    radius: get_v(po, &f, "radius", J::from(0.0))?,
                })
            } else {
                if let Some(k) = ["on", "margin_x", "margin_y", "radius"]
                    .iter()
                    .find(|k| po.contains_key(**k))
                {
                    return err(&format!("{f}.{k}"), "plate passes only");
                }
                None
            };
            // The wipe's second colour is drawn by fill and outline passes only; its
            // own opacity also by the glow (a halo fainter over the unsung side).
            if matches!(kind.as_str(), "shadow" | "glow") && po.contains_key("color2") {
                return err(&format!("{f}.color2"), "fill and outline_only passes only");
            }
            if kind == "shadow" && po.contains_key("alpha2") {
                return err(
                    &format!("{f}.alpha2"),
                    "fill, outline_only and glow passes only",
                );
            }
            passes.push(Pass {
                kind,
                color: get_v(po, &f, "color", J::from("#FFFFFF"))?,
                color2: po
                    .get("color2")
                    .map(|c| parse_v(c, &format!("{f}.color2")))
                    .transpose()?,
                alpha: get_v(po, &f, "alpha", J::from(1.0))?,
                alpha2: po
                    .get("alpha2")
                    .map(|c| parse_v(c, &format!("{f}.alpha2")))
                    .transpose()?,
                offset_y: get_v(po, &f, "offset_y", J::from(0.0))?,
                blur: get_v(po, &f, "blur", J::from(0.0))?,
                outline: get_v(po, &f, "outline", J::from(0.0))?,
                gain: get_v(po, &f, "gain", J::from(1.0))?,
                plate,
            });
        }
        // A plate property must reach a plate: one drawn on its track's unit.
        for (i, t) in tracks.iter().enumerate() {
            if t.prop.is_plate()
                && !passes
                    .iter()
                    .any(|p| p.plate.as_ref().is_some_and(|pl| pl.on == t.unit))
            {
                return err(
                    &format!("animate[{i}].property"),
                    "no plate pass is drawn on this unit (render.passes on)",
                );
            }
        }
        // shader (black box)
        let shader = match o.get("shader").and_then(J::as_str) {
            None => None,
            Some(rel) => {
                // Only plain names below the package: no root, prefix (drive, UNC), "." or "..".
                // A package is the same on every system: '/' is its only separator, so
                // '\' and ':' are refused everywhere, not only where Windows reads them.
                let path = Path::new(rel);
                if rel.is_empty()
                    || rel.contains(['\\', ':'])
                    || path.has_root()
                    || !path
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    return err("shader", "path must stay inside the package");
                }
                let file = dir.join(path);
                let size = std::fs::metadata(&file)
                    .map_err(|e| LoadError(format!("shader {rel}: {e}")))?
                    .len();
                if size > MAX_WGSL as u64 {
                    return err("shader", format!("{size} bytes exceeds {MAX_WGSL}"));
                }
                let src = std::fs::read_to_string(&file)
                    .map_err(|e| LoadError(format!("shader {rel}: {e}")))?;
                if src.len() > MAX_WGSL {
                    return err("shader", format!("{} bytes exceeds {MAX_WGSL}", src.len()));
                }
                super::plugin_shader::validate(&src)
                    .map_err(|e| LoadError(format!("shader {rel}: {e}")))?;
                Some(src)
            }
        };
        let names = names(o.get("name"), "name")?;
        let effect = Effect {
            id,
            params,
            presets,
            names,
            preset_names,
            layout,
            tracks,
            passes,
            shader,
            load_ms: 0.0,
        };
        // every $reference must name a declared param
        let mut refs = Vec::new();
        effect.collect_refs(&mut refs);
        for (r, field) in refs {
            if !ids.contains(&r) {
                return err(&field, format!("${r} names no declared param"));
            }
        }
        Ok(Effect {
            load_ms: started.elapsed().as_secs_f64() * 1000.0,
            ..effect
        })
    }

    fn collect_refs(&self, out: &mut Vec<(String, String)>) {
        let l = &self.layout;
        if let Some(v) = &l.word_size {
            param_refs(v, out);
        }
        for v in [
            &l.font,
            &l.size,
            &l.line_height,
            &l.raster,
            &l.bx.x,
            &l.bx.y,
            &l.bx.width,
            &l.bx.anchor,
        ] {
            param_refs(v, out);
        }
        if let Some(c) = &l.compose {
            for r in [&c.carrier, &c.second, &c.others] {
                for v in [
                    &r.font,
                    &r.size,
                    &r.rotation,
                    &r.color,
                    &r.alpha,
                    &r.outline,
                ] {
                    param_refs(v, out);
                }
            }
            for s in &c.shapes {
                for v in [&s.x0, &s.y0, &s.x1, &s.y1, &s.color, &s.alpha] {
                    param_refs(v, out);
                }
            }
        }
        for t in &self.tracks {
            match &t.shape {
                Shape::Keys(k, _) => k.iter().for_each(|(_, v)| param_refs(v, out)),
                Shape::Wave { amplitude, .. } | Shape::Noise { amplitude, .. } => {
                    param_refs(amplitude, out)
                }
                Shape::Const(v) => param_refs(v, out),
            }
        }
        for p in &self.passes {
            for v in [
                &p.color,
                &p.alpha,
                &p.offset_y,
                &p.blur,
                &p.outline,
                &p.gain,
            ] {
                param_refs(v, out);
            }
            for c in [&p.color2, &p.alpha2].into_iter().flatten() {
                param_refs(c, out);
            }
            if let Some(pl) = &p.plate {
                for v in [&pl.margin_x, &pl.margin_y, &pl.radius] {
                    param_refs(v, out);
                }
            }
        }
    }

    /// Param values: defaults, then the preset, then explicit overrides.
    pub fn values(
        &self,
        preset: Option<&str>,
        overrides: &HashMap<String, J>,
        portrait: bool,
    ) -> HashMap<String, PVal> {
        let mut raw: HashMap<String, J> = self
            .params
            .iter()
            .map(|p| (p.id.clone(), p.default.clone()))
            .collect();
        if let Some((_, v)) = preset.and_then(|id| self.presets.iter().find(|(p, _)| p == id)) {
            raw.extend(v.clone());
        }
        raw.extend(overrides.clone());
        self.params
            .iter()
            .map(|p| {
                let mut j = raw[&p.id].clone();
                if let J::Object(m) = &j {
                    j = m
                        .get(if portrait { "portrait" } else { "landscape" })
                        .cloned()
                        .unwrap_or(J::Null);
                }
                let v = match p.kind.as_str() {
                    "color" => PVal::Color(j.as_str().and_then(hex).unwrap_or([255, 255, 255])),
                    "number" => PVal::Num(j.as_f64().unwrap_or(0.0)),
                    "bool" => PVal::Bool(j.as_bool().unwrap_or(false)),
                    "palette" => PVal::Palette(
                        j.as_array()
                            .map(|a| a.iter().filter_map(|c| c.as_str().and_then(hex)).collect())
                            .unwrap_or_default(),
                    ),
                    _ => PVal::Text(j.as_str().unwrap_or("").to_string()),
                };
                (p.id.clone(), v)
            })
            .collect()
    }
}

fn parse_curve(c: &str, field: &str) -> Result<Curve, LoadError> {
    Ok(match c {
        "linear" => Curve::Linear,
        "ease_in" => Curve::EaseIn,
        "ease_out" => Curve::EaseOut,
        "ease_in_out" => Curve::EaseInOut,
        "back" => Curve::Back,
        "ease_in_cubic" => Curve::EaseInCubic,
        "ease_out_cubic" => Curve::EaseOutCubic,
        c => return err(field, format!("{c:?} unknown")),
    })
}

// ---------------------------------------------------------------- runtime

/// `hash`: a value from -1 to 1 for `x` rounded to a whole number (splitmix64).
fn hash1(x: f64) -> f64 {
    let mut z = (x.round() as i64 as u64).wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z as f64 / u64::MAX as f64) * 2.0 - 1.0
}

fn ease(c: Curve, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    match c {
        Curve::Linear => x,
        Curve::EaseIn => x * x,
        Curve::EaseOut => 1.0 - (1.0 - x) * (1.0 - x),
        Curve::EaseInOut => {
            if x < 0.5 {
                2.0 * x * x
            } else {
                1.0 - (-2.0 * x + 2.0).powi(2) / 2.0
            }
        }
        Curve::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
        }
        Curve::EaseInCubic => x * x * x,
        Curve::EaseOutCubic => 1.0 - (1.0 - x).powi(3),
    }
}

fn noise1(x: f64, seed: u64) -> f64 {
    let h = |i: i64| -> f64 {
        let mut z = (i as u64).wrapping_add(seed.wrapping_mul(0x9E3779B97F4A7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        (z as f64 / u64::MAX as f64) * 2.0 - 1.0
    };
    let i = x.floor();
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f);
    h(i as i64) * (1.0 - u) + h(i as i64 + 1) * u
}

fn apply_case(s: &str, case: &str) -> String {
    match case {
        "upper" => s.to_uppercase(),
        "lower" => s.to_lowercase(),
        _ => s.to_string(),
    }
}

#[derive(Clone, Debug)]
struct Glyph {
    sprite: Sprite,
    cx: f32,
    cy: f32,
    word: usize,
    index: usize,
    index_in_word: usize,
    font_size: f32,
    /// Its characters' place in the word, and its pen span (syllable boxes).
    char_in_word: usize,
    pen: [f32; 2],
    /// Characters from its row's centre, for letter spacing (PGlyph::row_pos).
    row_pos: f32,
    /// Its visual row in the block (plates find their row's box).
    row: usize,
}

#[derive(Clone, Debug)]
struct WordBox {
    x0: f32,
    x1: f32,
    pivot: [f32; 2],
    rot: f32,
    color: [u8; 3],
    alpha: f32,
    outline: f32,
}

struct Prepared {
    line: usize,
    state: u8,
    offset: i32,
    /// Vertical extent of the laid-out block (line mode), for stacking a window.
    top: f32,
    bottom: f32,
    /// The font size of this line, the unit of the window gap.
    em: f32,
    glyphs: Vec<Glyph>,
    words: Vec<WordBox>,
    /// word draw order (compose); identity otherwise
    order: Vec<usize>,
    shapes: Vec<Inst>,
    /// A word wider than the box: kept whole, wider than the box (line mode).
    too_wide: bool,
    /// Each visual row's top and bottom in canvas px (line mode), for plates.
    rows: Vec<(f32, f32)>,
    /// The centre of the laid-out text (line mode): the line's scale and
    /// rotation turn about it.
    center: [f32; 2],
}

pub struct PluginScene {
    pub effect: Effect,
    pub w: f32,
    pub h: f32,
    params: HashMap<String, PVal>,
    lines: Vec<Line>,
    /// Each line's visible span [from, to): its cue widened by `preroll` and
    /// `hold`, never overlapping the previous line (`shown_spans`).
    shown: Vec<(f64, f64)>,
    /// The preset the parameters come from, for `with_values`.
    preset: Option<String>,
    /// The safe surface in canvas pixels: x0, y0, width, height.
    safe: [f32; 4],
    /// A safe area applies (not the whole canvas): letters are checked against it.
    checked: bool,
    /// Without a safe area, letters are still checked against the canvas (exports:
    /// text leaving the picture is said). Classique's layout does not change.
    frame_check: bool,
    /// The last frame drew a letter's ink outside the safe surface.
    overflowed: bool,
    /// Whether the last `frame` showed a line holding a word wider than its box.
    too_wide: bool,
    /// Why the installed family the font value asks for was not drawn (the
    /// plugin's own face was): missing on this machine, or not read yet.
    font_lack: Option<Lack>,
    prepared: Vec<Prepared>,
    /// State and slot of the line being prepared or drawn, seen by values
    /// (`by_state`, `by_offset`).
    state: u8,
    offset: i32,
    /// The line being prepared or drawn (`@line_index`).
    li: usize,
    pub atlas: Atlas,
    pub tracks_us: Vec<f64>,
}

#[derive(Default)]
struct State {
    opacity: f64,
    scale: f64,
    scale_x: f64,
    offset_x: f64,
    offset_y: f64,
    rotation: f64,
    blur: f64,
    color: Option<[u8; 3]>,
    color2: Option<[u8; 3]>,
    wipe: Option<f64>,
    glow: Option<f64>,
    tracking: f64,
    reveal: Option<f64>,
    plate_opacity: f64,
    plate_margin: f64,
    plate_scale_x: f64,
    plate_wipe: Option<f64>,
    wipe_edge: Option<f64>,
}

impl State {
    /// A unit before its tracks: drawn whole, at its place and size.
    fn unit() -> Self {
        State {
            opacity: 1.0,
            scale: 1.0,
            scale_x: 1.0,
            plate_opacity: 1.0,
            plate_scale_x: 1.0,
            ..Default::default()
        }
    }
}

impl PluginScene {
    pub fn new(effect: Effect, w: u32, h: u32, lines: Vec<Line>, preset: Option<&str>) -> Self {
        let params = effect.values(preset, &HashMap::new(), h > w);
        let lines = match effect.layout.chunk {
            Some(c) => chunked(&lines, c, effect.layout.preroll),
            None => lines,
        };
        let shown = shown_spans(&lines, effect.layout.preroll, effect.layout.hold);
        Self {
            effect,
            w: w as f32,
            h: h as f32,
            params,
            lines,
            shown,
            preset: preset.map(str::to_string),
            safe: [0.0, 0.0, w as f32, h as f32],
            checked: false,
            frame_check: false,
            overflowed: false,
            too_wide: false,
            font_lack: None,
            prepared: Vec::new(),
            state: ACTIVE,
            offset: 0,
            li: 0,
            atlas: Atlas::new(48.0),
            tracks_us: Vec::new(),
        }
    }

    /// Parameter values over the preset's (the app's settings for this effect).
    /// A request is a wire format: an unknown id, a wrong type, a choice outside
    /// its list or a number outside its bounds is refused, not ignored.
    pub fn with_values(
        mut self,
        values: &std::collections::BTreeMap<String, J>,
    ) -> Result<Self, String> {
        for (k, v) in values {
            let p = self
                .effect
                .params
                .iter()
                .find(|p| &p.id == k)
                .ok_or_else(|| format!("{}: unknown parameter {k:?}", self.effect.id))?;
            if !p.accepts(v) {
                return Err(format!(
                    "{}: {v} is not a valid {} for {k:?}",
                    self.effect.id, p.kind
                ));
            }
        }
        if !values.is_empty() {
            let overrides: HashMap<String, J> =
                values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            self.params = self
                .effect
                .values(self.preset.as_deref(), &overrides, self.h > self.w);
        }
        Ok(self)
    }

    /// The safe surface as insets (left, top, right, bottom) from the canvas
    /// edges, e.g. `SafeArea::insets`. None keeps the whole canvas.
    pub fn with_safe_insets(mut self, insets: Option<[u32; 4]>) -> Self {
        if let Some([l, t, r, b]) = insets {
            let (l, t, r, b) = (l as f32, t as f32, r as f32, b as f32);
            self.safe = [l, t, (self.w - l - r).max(1.0), (self.h - t - b).max(1.0)];
            self.checked = true;
        }
        self
    }

    /// Check letters against the whole canvas when no safe area applies (the
    /// `@safe` host value stays 0, so layouts are unchanged).
    pub fn with_frame_check(mut self) -> Self {
        self.frame_check = true;
        self
    }

    /// Whether the last `frame` drew some letter ink outside the safe surface
    /// (by more than OVERFLOW_TOLERANCE px). Always false without a safe area.
    /// NOH only reports it: nothing is moved or resized.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Whether the last `frame` showed a line holding a word wider than its box:
    /// the word is drawn whole and overflows it; NOH only reports it. Every
    /// effect, animated or not, with or without a safe area.
    pub fn too_wide(&self) -> bool {
        self.too_wide
    }

    /// The installed family the font setting names and this machine lacks, once
    /// a line was laid out with it; that text is drawn in the plugin's own face.
    pub fn font_missing(&self) -> Option<&str> {
        match &self.font_lack {
            Some(Lack::Missing(family)) => Some(family),
            _ => None,
        }
    }

    /// A line was laid out before the installed fonts were read: drawn in the
    /// plugin's own face for now (not a missing font); build the scene again
    /// once they are read.
    pub fn font_unread(&self) -> bool {
        matches!(self.font_lack, Some(Lack::NotRead(_)))
    }

    /// Whether a font parameter of this scene names an installed family: the
    /// installed fonts must then be read before it is drawn (an export reads them
    /// itself, so it can only meet a missing font, never an unread one).
    pub fn wants_installed_fonts(&self) -> bool {
        self.effect.params.iter().filter(|p| p.kind == "font").any(
            |p| matches!(self.params.get(&p.id), Some(PVal::Text(s)) if s.starts_with("system:")),
        )
    }

    /// The plugin's own face, drawn when an installed font is missing: its font
    /// parameter's default (always a bundled face), else Sans.
    fn font_fallback(&self) -> Face {
        self.effect
            .params
            .iter()
            .find(|p| p.kind == "font")
            .and_then(|p| p.default.as_str())
            .and_then(Face::named)
            .unwrap_or(Face::Sans)
    }

    /// The safe surface: x0, y0, width, height in canvas pixels.
    pub fn safe_rect(&self) -> [f32; 4] {
        self.safe
    }

    /// Parse a value as an effect would and evaluate it in this scene's context
    /// (tests and diagnostics; no side effect).
    pub fn eval_value(&self, j: &J) -> Result<R, String> {
        let v = parse_v(j, "test").map_err(|e| e.0)?;
        Ok(eval(&v, &self.ctx()))
    }

    /// HOST_VARS: w, h, short, inset_l, inset_t, inset_r, inset_b, safe.
    fn host(&self) -> [f64; 8] {
        let [x0, y0, sw, sh] = self.safe;
        let (w, h) = (f64::from(self.w), f64::from(self.h));
        [
            w,
            h,
            w.min(h),
            f64::from(x0),
            f64::from(y0),
            w - f64::from(x0 + sw),
            h - f64::from(y0 + sh),
            f64::from(u8::from(self.checked)),
        ]
    }

    /// The rectangle a space refers to.
    fn frame_of(&self, space: Space) -> [f32; 4] {
        match space {
            Space::Safe => self.safe,
            Space::Canvas => [0.0, 0.0, self.w, self.h],
        }
    }

    /// A fractional point in a space, in canvas pixels. With the whole canvas
    /// the arithmetic is the previous `x * w` exactly (no added 0.0).
    fn at(&self, space: Space, x: f32, y: f32) -> [f32; 2] {
        let [fx, fy, fw, fh] = self.frame_of(space);
        let shift = |o: f32, v: f32| if o != 0.0 { o + v } else { v };
        [shift(fx, x * fw), shift(fy, y * fh)]
    }

    pub fn with_px_scale(mut self, s: f32) -> Self {
        self.atlas.px_scale = s;
        self
    }

    pub fn line_at(&self, t: f64) -> Option<usize> {
        self.shown
            .iter()
            .position(|&(from, to)| t >= from && t < to)
    }

    /// The lines drawn at `t`: (line, state, offset from the active slot).
    /// Without a window, the active line alone. With one, up to `before` past
    /// lines above and `after` upcoming lines below, each upcoming line from
    /// `lead` seconds before its start. Between lines, the first upcoming line
    /// within its lead takes the active slot, still in the NEXT state.
    pub fn visible_lines(&self, t: f64) -> Vec<(usize, u8, i32)> {
        let Some(w) = &self.effect.layout.window else {
            return self
                .line_at(t)
                .map(|li| vec![(li, ACTIVE, 0)])
                .unwrap_or_default();
        };
        let soon = |i: usize| self.shown.get(i).is_some_and(|s| s.0 - w.lead <= t);
        let (anchor, active) = match self.line_at(t) {
            Some(li) => (li, true),
            None => match self.shown.iter().position(|s| s.0 > t) {
                Some(li) if soon(li) => (li, false),
                _ => return Vec::new(),
            },
        };
        let mut out = Vec::new();
        if active {
            for k in (1..=w.before.min(anchor)).rev() {
                out.push((anchor - k, PAST, -(k as i32)));
            }
        }
        out.push((anchor, if active { ACTIVE } else { NEXT }, 0));
        for k in 1..=w.after {
            if soon(anchor + k) {
                out.push((anchor + k, NEXT, k as i32));
            }
        }
        out
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            params: &self.params,
            portrait: self.h > self.w,
            word_index: 0,
            index: 0,
            index_in_word: 0,
            line_state: self.state,
            line_offset: self.offset,
            host: self.host(),
            word: [0.0; 2],
            line: [self.li as f64],
        }
    }

    fn n(&self, v: &V) -> f32 {
        num(&eval(v, &self.ctx())) as f32
    }

    fn s(&self, v: &V) -> String {
        match eval(v, &self.ctx()) {
            R::Text(s) => s,
            _ => String::new(),
        }
    }

    fn place(
        &mut self,
        fonts: &mut Fonts,
        gpu: &dyn Upload,
        glyphs: &[PGlyph],
        ox: f32,
        oy: f32,
        oversample: f32,
        word: Option<usize>,
    ) -> Vec<Glyph> {
        let mut out = Vec::new();
        let mut per_word: HashMap<usize, usize> = HashMap::new();
        for g in glyphs {
            if let Some(s) = self.atlas.cov_sprite(fonts, gpu, g, oversample) {
                let wi = word.unwrap_or(g.word);
                let iw = per_word.entry(wi).or_insert(0);
                out.push(Glyph {
                    sprite: s,
                    cx: ox + g.x + s.left + s.w * 0.5,
                    cy: oy + g.baseline - s.top + s.h * 0.5,
                    word: wi,
                    index: g.index,
                    index_in_word: *iw,
                    font_size: g.size,
                    char_in_word: g.char_in_word,
                    pen: [ox + g.x, ox + g.x + g.advance],
                    row_pos: g.row_pos,
                    row: g.line,
                });
                *iw += 1;
            }
        }
        out
    }

    fn prepare(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, li: usize) -> Prepared {
        let line = self.lines[li].clone();
        let (mut top, mut bottom, mut em) = (0.0f32, 0.0f32, 0.0f32);
        let mut too_wide = false;
        let mut rows = Vec::new();
        let mut center = [0.0f32; 2];
        let (w, h) = (self.w, self.h);
        let portrait = h > w;
        let lay = self.effect.layout.clone();
        let mut words = vec![
            WordBox {
                x0: f32::MAX,
                x1: f32::MIN,
                pivot: [0.0; 2],
                rot: 0.0,
                color: [255; 3],
                alpha: 1.0,
                outline: 0.0
            };
            line.words.len()
        ];
        let mut order: Vec<usize> = (0..line.words.len()).collect();
        let mut shapes = Vec::new();
        let raster = self.n(&lay.raster);
        let glyphs = if let Some(c) = &lay.compose {
            let keys: Vec<String> = line
                .words
                .iter()
                .map(|x| {
                    x.text
                        .chars()
                        .filter(|c| c.is_alphanumeric() || *c == '\'')
                        .collect::<String>()
                        .to_lowercase()
                })
                .collect();
            let content = |i: usize| !keys[i].is_empty() && !c.stopwords.contains(&keys[i]);
            let rank_key = |i: usize| -> Vec<f64> {
                c.rank
                    .iter()
                    .map(|k| match k.as_str() {
                        "content_word" => f64::from(u8::from(content(i))),
                        "duration" => line.words[i].end - line.words[i].start,
                        _ => keys[i].len() as f64,
                    })
                    .collect()
            };
            let mut ranked: Vec<usize> = (0..line.words.len()).collect();
            ranked.sort_by(|a, b| rank_key(*b).partial_cmp(&rank_key(*a)).unwrap());
            let carrier = ranked[0];
            let second = ranked
                .get(1)
                .copied()
                .filter(|i| c.second.require.as_deref() != Some("content_word") || content(*i));
            for s in &c.shapes {
                let [x0, y0] = self.at(s.space, self.n(&s.x0), self.n(&s.y0));
                let [x1, y1] = self.at(s.space, self.n(&s.x1), self.n(&s.y1));
                let color = col(&eval(&s.color, &self.ctx()));
                shapes.push(solid(
                    (x0 + x1) * 0.5,
                    (y0 + y1) * 0.5,
                    x1 - x0,
                    y1 - y0,
                    premul(color, self.n(&s.alpha)),
                ));
            }
            let slots = if portrait {
                &c.slots.portrait
            } else {
                &c.slots.landscape
            };
            let mut slot_k = 0usize;
            let mut all = Vec::new();
            for (i, word) in line.words.iter().enumerate() {
                let (role, pivot, centred) = if i == carrier {
                    let r = &c.carrier;
                    let p = r
                        .pivot
                        .as_ref()
                        .map(|(x, y)| self.at(r.space, self.n(x), self.n(y)))
                        .unwrap_or_else(|| self.at(r.space, 0.5, 0.5));
                    (r.clone(), p, true)
                } else if Some(i) == second {
                    let r = &c.second;
                    let p = r
                        .pivot
                        .as_ref()
                        .map(|(x, y)| self.at(r.space, self.n(x), self.n(y)))
                        .unwrap_or_else(|| self.at(r.space, 0.5, 0.2));
                    (r.clone(), p, true)
                } else {
                    let s = &slots[slot_k % slots.len().max(1)];
                    slot_k += 1;
                    let r = s.style.clone().unwrap_or_else(|| c.others.clone());
                    let centred = r.align == "center";
                    (self.at(s.space, self.n(&s.x), self.n(&s.y)), r, centred).into_role()
                };
                let (face, lack) = fonts.face(&self.s(&role.font), self.font_fallback());
                if lack.is_some() {
                    self.font_lack = lack;
                }
                let size = self.n(&role.size) * h;
                let text = apply_case(&word.text, &role.case);
                let laid = layout(fonts, &[text], "", &[], face, size, None, 1.0);
                let ox = if centred {
                    pivot[0] - laid.width * 0.5
                } else {
                    pivot[0]
                };
                let oy = pivot[1] - laid.height * 0.5;
                let placed = self.place(fonts, gpu, &laid.glyphs, ox, oy, raster, Some(i));
                let p = if centred {
                    pivot
                } else {
                    [ox + laid.width * 0.5, pivot[1]]
                };
                let ctx = Ctx {
                    word_index: i,
                    word: [
                        line.words[i].end - line.words[i].start,
                        if Some(i) == longest_word(&line) {
                            1.0
                        } else {
                            0.0
                        },
                    ],
                    ..self.ctx()
                };
                words[i] = WordBox {
                    x0: ox,
                    x1: ox + laid.width,
                    pivot: p,
                    rot: self.n(&role.rotation).to_radians(),
                    color: col(&eval(&role.color, &ctx)),
                    alpha: self.n(&role.alpha),
                    outline: self.n(&role.outline) * size,
                };
                all.extend(placed);
            }
            let mut draw = vec![carrier];
            draw.extend(second);
            draw.extend((0..line.words.len()).filter(|i| *i != carrier && Some(*i) != second));
            order = draw;
            all
        } else {
            let (face, lack) = fonts.face(&self.s(&lay.font), self.font_fallback());
            if lack.is_some() {
                self.font_lack = lack;
            }
            let size = self.n(&lay.size);
            let words_t: Vec<String> = line
                .words
                .iter()
                .map(|x| apply_case(&x.text, &lay.case))
                .collect();
            let [_, _, fw, _] = self.frame_of(lay.bx.space);
            let max_w = fw * self.n(&lay.bx.width);
            // A chunk's word rows are breaks of their own (layout.chunk.rows).
            let rows_word = lay.chunk.is_some_and(|c| c.rows_word);
            let breaks: &[usize] = if lay.keep_breaks || rows_word {
                &line.breaks
            } else {
                &[]
            };
            // Each word's size factor, read with its word values (0.25..4).
            let word_sizes: Vec<f32> = match &lay.word_size {
                None => Vec::new(),
                Some(v) => {
                    let longest = longest_word(&line);
                    (0..line.words.len())
                        .map(|i| {
                            let wd = &line.words[i];
                            let ctx = Ctx {
                                word_index: i,
                                word: [
                                    wd.end - wd.start,
                                    if Some(i) == longest { 1.0 } else { 0.0 },
                                ],
                                ..self.ctx()
                            };
                            (num(&eval(v, &ctx)) as f32).clamp(0.25, 4.0)
                        })
                        .collect()
                }
            };
            let laid = layout_aligned(
                fonts,
                &words_t,
                &line.sep,
                breaks,
                face,
                size,
                &word_sizes,
                Some(max_w),
                self.n(&lay.line_height),
                lay.align,
            );
            let [bx, ay] = self.at(lay.bx.space, self.n(&lay.bx.x), self.n(&lay.bx.y));
            let ox = bx - max_w * 0.5;
            let oy = match self.s(&lay.bx.anchor).as_str() {
                "top" => ay,
                "center" => ay - laid.height * 0.5,
                _ => ay - laid.height,
            };
            (top, bottom, em) = (oy, oy + laid.height, size);
            too_wide = laid.too_wide;
            rows = laid
                .rows
                .iter()
                .map(|&(t, h)| (oy + t, oy + t + h))
                .collect();
            let (x0, x1) = laid.glyphs.iter().fold((f32::MAX, f32::MIN), |(a, b), g| {
                (a.min(ox + g.x), b.max(ox + g.x + g.advance))
            });
            center = [
                if x0 <= x1 { (x0 + x1) * 0.5 } else { bx },
                oy + laid.height * 0.5,
            ];
            for g in &laid.glyphs {
                let e = &mut words[g.word];
                e.x0 = e.x0.min(ox + g.x);
                e.x1 = e.x1.max(ox + g.x + g.advance);
            }
            self.place(fonts, gpu, &laid.glyphs, ox, oy, raster, None)
        };
        Prepared {
            line: li,
            state: self.state,
            offset: self.offset,
            top,
            bottom,
            em,
            glyphs,
            words,
            order,
            shapes,
            too_wide,
            rows,
            center,
        }
    }

    /// Evaluate the tracks for one unit level and fold them into `st`.
    fn fold(
        &self,
        st: &mut State,
        unit: Unit,
        t: f64,
        start: f64,
        end: f64,
        line_end: f64,
        idx: usize,
        ctx: &Ctx,
    ) {
        // A line's own enter runs from its pre-roll (layout.preroll).
        let from = if unit == Unit::Line {
            start - self.effect.layout.preroll
        } else {
            start
        };
        for tr in self.effect.tracks.iter().filter(|tr| tr.unit == unit) {
            let s0 = from + tr.stagger * idx as f64;
            let active = match tr.slot {
                Slot::Enter => t >= s0 && t < s0 + tr.duration,
                Slot::Exit => t >= line_end - tr.duration && t < line_end,
                Slot::Loop => true,
                Slot::Active => t >= start && t < end,
                Slot::Past => t >= end,
                Slot::Future => t < start,
            };
            // Enter tracks hold their last key after the slot, as the proto's clamps do.
            let held = tr.slot == Slot::Enter && t >= s0 + tr.duration;
            if !active && !held {
                continue;
            }
            let x = match tr.slot {
                Slot::Enter => ((t - s0) / tr.duration).clamp(0.0, 1.0),
                Slot::Exit => ((t - (line_end - tr.duration)) / tr.duration).clamp(0.0, 1.0),
                Slot::Active => ((t - start) / (end - start)).clamp(0.0, 1.0),
                // A timed return: its keys run from the unit's end (track duration).
                Slot::Past if tr.timed => ((t - end) / tr.duration).clamp(0.0, 1.0),
                _ => 0.0,
            };
            let r = match &tr.shape {
                Shape::Keys(k, c) => {
                    if k.len() == 1 {
                        eval(&k[0].1, ctx)
                    } else {
                        let mut seg = 0;
                        while seg + 2 < k.len() && x > k[seg + 1].0 {
                            seg += 1;
                        }
                        let (t0, t1) = (k[seg].0, k[seg + 1].0);
                        let u = if t1 > t0 {
                            ((x - t0) / (t1 - t0)).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let (a, b) = (eval(&k[seg].1, ctx), eval(&k[seg + 1].1, ctx));
                        match (a, b) {
                            (R::Num(a), R::Num(b)) => R::Num(a + (b - a) * ease(*c, u)),
                            // Colours blend per sRGB channel, as `mix` does.
                            (R::Color(a), R::Color(b)) => R::Color(lerp_color(a, b, ease(*c, u))),
                            (a, b) => {
                                if ease(*c, u) < 0.5 {
                                    a
                                } else {
                                    b
                                }
                            }
                        }
                    }
                }
                Shape::Wave {
                    amplitude,
                    frequency,
                    phase_per_unit,
                    phase,
                } => {
                    let a = num(&eval(amplitude, ctx));
                    R::Num(
                        a * (std::f64::consts::TAU * frequency * t - phase_per_unit * idx as f64
                            + phase)
                            .sin(),
                    )
                }
                Shape::Noise {
                    amplitude,
                    frequency,
                    seed,
                } => R::Num(
                    num(&eval(amplitude, ctx)) * noise1(t * frequency + idx as f64 * 17.0, *seed),
                ),
                Shape::Const(v) => eval(v, ctx),
            };
            match tr.prop {
                Prop::Opacity => st.opacity *= num(&r),
                Prop::Scale => st.scale *= num(&r),
                Prop::ScaleX => st.scale_x *= num(&r),
                Prop::OffsetX => st.offset_x += num(&r),
                Prop::OffsetY => st.offset_y += num(&r),
                Prop::Rotation => st.rotation += num(&r).to_radians(),
                Prop::Blur => st.blur = num(&r),
                Prop::Color => st.color = Some(col(&r)),
                Prop::Color2 => st.color2 = Some(col(&r)),
                Prop::Wipe => st.wipe = Some(num(&r)),
                Prop::Glow => st.glow = Some(num(&r)),
                Prop::Tracking => st.tracking += num(&r),
                Prop::RevealY => st.reveal = Some(num(&r).clamp(0.0, 1.0)),
                Prop::PlateOpacity => st.plate_opacity *= num(&r),
                Prop::PlateMargin => st.plate_margin += num(&r),
                Prop::PlateScaleX => st.plate_scale_x *= num(&r),
                Prop::PlateWipe => st.plate_wipe = Some(num(&r).clamp(0.0, 1.0)),
                Prop::WipeEdge => st.wipe_edge = Some(num(&r).max(0.0)),
            }
        }
    }

    pub fn frame(
        &mut self,
        fonts: &mut Fonts,
        gpu: &dyn Upload,
        t: f64,
    ) -> (Vec<Inst>, f32, Vec<Inst>, bool) {
        let visible = self.visible_lines(t);
        if visible.is_empty() {
            self.prepared.clear();
            self.overflowed = false;
            self.too_wide = false;
            return (Vec::new(), 1.0, Vec::new(), false);
        }
        // A window gliding to its new place also needs the lines as they were
        // placed before the change (`layout.window.glide`).
        let glide = self.glide(t, &visible);
        let mut needed = visible.clone();
        if let Some((_, prev)) = &glide {
            for e in prev {
                if !needed.contains(e) {
                    needed.push(*e);
                }
            }
        }
        // Keep the prepared lines still on screen, in their current state; build the rest.
        let mut changed = false;
        let mut kept: Vec<Prepared> = Vec::with_capacity(needed.len());
        for &(li, state, offset) in &needed {
            match self
                .prepared
                .iter()
                .position(|p| p.line == li && p.state == state && p.offset == offset)
            {
                Some(i) => kept.push(self.prepared.swap_remove(i)),
                None => {
                    self.state = state;
                    self.offset = offset;
                    self.li = li;
                    let p = self.prepare(fonts, gpu, li);
                    kept.push(p);
                    changed = true;
                }
            }
        }
        changed |= !self.prepared.is_empty();
        self.prepared = kept;
        self.state = ACTIVE;
        self.offset = 0;
        self.li = 0;
        let started = Instant::now();
        // Vertical shift of each line: the slot-0 line stays where its box puts it;
        // upcoming lines stack below it, past lines above, `gap` font sizes apart.
        let here: Vec<usize> = (0..visible.len()).collect();
        let dys = self.stack(&visible, &here);
        // (prepared index, line, state, offset, shift, fade), drawn in order.
        let mut draws: Vec<(usize, usize, u8, i32, f32, f64)> = Vec::new();
        if let Some((u, prev)) = &glide {
            let at: Vec<usize> = prev
                .iter()
                .map(|e| needed.iter().position(|n| n == e).unwrap_or(0))
                .collect();
            let dys_prev = self.stack(prev, &at);
            let y_now = |l: usize| {
                visible
                    .iter()
                    .position(|v| v.0 == l)
                    .map(|k| self.prepared[k].top + dys[k])
            };
            let y_before = |l: usize| {
                prev.iter()
                    .position(|v| v.0 == l)
                    .map(|j| self.prepared[at[j]].top + dys_prev[j])
            };
            // How far the column moves: measured on the new slot-0 line, else on
            // any line shown both before and now.
            let anchor = visible
                .iter()
                .find(|v| v.2 == 0)
                .map_or(visible[0].0, |v| v.0);
            let d = std::iter::once(anchor)
                .chain(visible.iter().map(|v| v.0))
                .find_map(|l| Some(y_before(l)? - y_now(l)?));
            if let Some(d) = d {
                let (uf, u) = (*u, *u as f32);
                // Lines leaving the window keep their place and look, move with the
                // column and fade out, under the others.
                for (j, &(li, state, offset)) in prev.iter().enumerate() {
                    if !visible.iter().any(|v| v.0 == li) {
                        draws.push((at[j], li, state, offset, dys_prev[j] - u * d, 1.0 - uf));
                    }
                }
                for (k, &(li, state, offset)) in visible.iter().enumerate() {
                    draws.push((k, li, state, offset, dys[k] + (1.0 - u) * d, 1.0));
                }
            }
        }
        if draws.is_empty() {
            for (k, &(li, state, offset)) in visible.iter().enumerate() {
                draws.push((k, li, state, offset, dys[k], 1.0));
            }
        }
        let mut glow = Vec::new();
        let mut main = Vec::new();
        let mut overflow = false;
        for &(k, li, state, offset, shift, fade) in &draws {
            self.state = state;
            self.offset = offset;
            self.li = li;
            self.draw_line(
                &self.prepared[k],
                li,
                shift,
                fade,
                t,
                &mut glow,
                &mut main,
                &mut overflow,
            );
        }
        self.overflowed = overflow;
        self.too_wide = self.prepared.iter().any(|p| p.too_wide);
        self.state = ACTIVE;
        self.offset = 0;
        self.li = 0;
        self.tracks_us.push(started.elapsed().as_secs_f64() * 1e6);
        (glow, 1.0, main, changed)
    }

    /// The vertical shift of each line of `lines` (a window, top to bottom), whose
    /// prepared layouts are `self.prepared[at[k]]`: the slot-0 line stays where its
    /// box puts it, the others stack `gap` font sizes apart above and below it.
    fn stack(&self, lines: &[(usize, u8, i32)], at: &[usize]) -> Vec<f32> {
        let gap = self.effect.layout.window.as_ref().map_or(0.0, |w| w.gap);
        let slot0 = lines.iter().position(|v| v.2 == 0).unwrap_or(0);
        let mut dys = vec![0.0f32; lines.len()];
        for k in slot0 + 1..lines.len() {
            let (prev, this) = (&self.prepared[at[k - 1]], &self.prepared[at[k]]);
            dys[k] = prev.bottom + dys[k - 1] + gap * prev.em - this.top;
        }
        for k in (0..slot0).rev() {
            let (next, this) = (&self.prepared[at[k + 1]], &self.prepared[at[k]]);
            dys[k] = next.top + dys[k + 1] - gap * this.em - this.bottom;
        }
        dys
    }

    /// While the window glides (`layout.window.glide`): how far along it is (the
    /// curve applied, 0 to 1) and the lines shown just before the active slot
    /// changed. None outside a glide, or when nothing was shown before.
    fn glide(&self, t: f64, visible: &[(usize, u8, i32)]) -> Option<(f64, Vec<(usize, u8, i32)>)> {
        let w = self.effect.layout.window.as_ref()?;
        let (duration, curve) = w.glide?;
        let anchor = visible.iter().find(|v| v.2 == 0)?.0;
        // When this line took the active slot: its showing, or earlier, in the
        // gap after the previous line, from its lead (`visible_lines`).
        let from = self.shown[anchor].0;
        let prev_end = anchor
            .checked_sub(1)
            .map_or(f64::NEG_INFINITY, |p| self.shown[p].1);
        let since = if prev_end < from {
            (from - w.lead).max(prev_end)
        } else {
            from
        };
        let x = (t - since) / duration;
        if !(0.0..1.0).contains(&x) {
            return None;
        }
        let prev = self.visible_lines(since - 1e-6);
        let before = prev.iter().find(|v| v.2 == 0)?.0;
        if before == anchor {
            return None;
        }
        Some((ease(curve, x), prev))
    }

    /// One prepared line at time `t`, shifted vertically by `shift` (its window
    /// slot and glide) and faded by `fade` (a line gliding out of the window).
    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &self,
        p: &Prepared,
        li: usize,
        shift: f32,
        fade: f64,
        t: f64,
        glow: &mut Vec<Inst>,
        main: &mut Vec<Inst>,
        overflow: &mut bool,
    ) {
        let line = &self.lines[li];
        // Exits run at the end of the line's visible span (layout.hold).
        let line_end = self.shown[li].1;
        let base = self.ctx();
        let mut line_st = State::unit();
        self.fold(
            &mut line_st,
            Unit::Line,
            t,
            line.start,
            line.end,
            line_end,
            0,
            &base,
        );
        if fade != 1.0 {
            line_st.opacity *= fade;
        }
        // What every instance of the line tells shader hooks: the line's seconds
        // since it showed and until it goes.
        let (since, until) = ((t - self.shown[li].0) as f32, (line_end - t) as f32);
        for s in &p.shapes {
            let mut s = *s;
            for c in &mut s.color {
                *c *= line_st.opacity as f32;
            }
            s.aux = [0.0, PASS_PLATE, since, until];
            main.push(s);
        }
        // The reveal clip line in canvas y: the block's top moved down by the hidden
        // share, following the block (window shift and line offset).
        // Fully shown (1) clips nothing, as `inset(0)` leaves the text whole.
        let clip_y = line_st.reveal.filter(|r| *r < 1.0).map(|r| {
            let top = p.top + shift + line_st.offset_y as f32 * p.em;
            top + (1.0 - r as f32) * (p.bottom - p.top)
        });
        // That line in an instance's local y (0 top, 1 bottom of its quad as drawn).
        let clip_of = |inst: &Inst| -> f32 {
            match clip_y {
                Some(y) => {
                    let h = (inst.size[1] * inst.xform[2]).max(1e-6);
                    (y - (inst.center[1] - h * 0.5)) / h
                }
                None => 0.0,
            }
        };
        // The line's own scale and rotation, about its block's centre; None when
        // it has neither, so its letters are placed as they always were.
        let lx = LineXform::of(&line_st, p.center);
        // After the line's scale and rotation: its offsets (in its em) and slot shift.
        let line_moved = |q: [f32; 2]| -> [f32; 2] {
            let x = q[0] + line_st.offset_x as f32 * p.em;
            let y = q[1] + line_st.offset_y as f32 * p.em;
            [x, if shift != 0.0 { y + shift } else { y }]
        };
        // Plates sit under every letter of their line, whatever the pass order.
        let plates_at = main.len();
        let mut plates: Vec<Inst> = Vec::new();
        let plate_passes: Vec<(&Pass, &Plate)> = self
            .effect
            .passes
            .iter()
            .filter_map(|pa| pa.plate.as_ref().map(|pl| (pa, pl)))
            .collect();
        let plate_of = |pass: &Pass,
                        pl: &Plate,
                        ctx: &Ctx,
                        unit: &State,
                        alpha: f64,
                        em: f32,
                        bx: [f32; 4],
                        unit_xf: UnitXform|
         -> Option<Inst> {
            let mut inst =
                self.plate(pass, pl, ctx, unit, alpha, em, bx, unit_xf, lx, &line_moved)?;
            inst.extra = [-1.0, t as f32, 0.0, 0.0];
            inst.aux = [0.0, PASS_PLATE, since, until];
            inst.xform[3] = clip_of(&inst);
            Some(inst)
        };
        // Plates are measured on the letters as drawn: where letter spacing (the
        // line's em) moves each letter, and where a scaled letter's pen edges land.
        let spaced = |g: &Glyph| (line_st.tracking * g.row_pos as f64) as f32 * p.em;
        let anchor_kind = self.effect.layout.anchor;
        let anchor_of = |g: &Glyph, wb: &WordBox| -> [f32; 2] {
            match anchor_kind {
                Anchor::Unit => [g.cx, g.cy],
                Anchor::Word => [(wb.x0 + wb.x1) * 0.5, g.cy],
                Anchor::WordXGlyphY => [(wb.x0 + wb.x1) * 0.5, g.cy],
                Anchor::Baseline => [(wb.x0 + wb.x1) * 0.5, g.cy],
                Anchor::Pivot => wb.pivot,
            }
        };
        // A letter's pen edge `x` once the letter is scaled by `sx` about `ax` and spaced.
        let edge = |g: &Glyph, x: f32, ax: f32, sx: f32| ax + (x - ax) * sx + spaced(g);
        for &(pass, pl) in plate_passes.iter().filter(|(_, pl)| pl.on == Unit::Line) {
            for (r, &(top, bottom)) in p.rows.iter().enumerate() {
                let (x0, x1) = p
                    .glyphs
                    .iter()
                    .filter(|g| g.row == r)
                    .fold((f32::MAX, f32::MIN), |(a, b), g| {
                        (a.min(g.pen[0] + spaced(g)), b.max(g.pen[1] + spaced(g)))
                    });
                if x0 <= x1 {
                    plates.extend(plate_of(
                        pass,
                        pl,
                        &base,
                        &line_st,
                        line_st.opacity,
                        p.em,
                        [x0, top, x1, bottom],
                        UnitXform::NONE,
                    ));
                }
            }
        }
        let compose = self.effect.layout.compose.is_some();
        let from_start = self.effect.layout.visible == "from_start";
        let mut by_word: Vec<Vec<&Glyph>> = vec![Vec::new(); line.words.len()];
        for g in &p.glyphs {
            by_word[g.word].push(g);
        }
        // A syllable-level wipe runs across the syllable's own glyphs.
        let syllable_wipe = self
            .effect
            .tracks
            .iter()
            .any(|tr| tr.unit == Unit::Syllable && tr.prop == Prop::Wipe);
        let lead = self.effect.layout.word_lead;
        let longest = longest_word(line);
        for &wi in &p.order {
            let wd = &line.words[wi];
            // The word's times as the effect reads them (layout.word_lead).
            let (w_start, w_end) = (wd.start - lead, wd.end - lead);
            if from_start && t < w_start {
                continue;
            }
            let wb = &p.words[wi];
            // Each glyph's syllable: (index, start, end, pen span); the whole word without syllables.
            let syllable_of = |g: &Glyph| -> (usize, f64, f64, [f32; 2]) {
                match wd
                    .syllables
                    .iter()
                    .rposition(|y| y.first_char <= g.char_in_word)
                {
                    Some(k) => {
                        let y = &wd.syllables[k];
                        let next = wd.syllables.get(k + 1).map_or(usize::MAX, |n| n.first_char);
                        let span = by_word[wi]
                            .iter()
                            .filter(|o| o.char_in_word >= y.first_char && o.char_in_word < next)
                            .fold([f32::MAX, f32::MIN], |b, o| {
                                [b[0].min(o.pen[0]), b[1].max(o.pen[1])]
                            });
                        (k, y.start - lead, y.end - lead, span)
                    }
                    None => (0, w_start, w_end, [wb.x0, wb.x1]),
                }
            };
            let word_vars = [
                wd.end - wd.start,
                if Some(wi) == longest { 1.0 } else { 0.0 },
            ];
            let wctx = Ctx {
                word_index: wi,
                word: word_vars,
                ..base
            };
            let mut word_st = State::unit();
            self.fold(
                &mut word_st,
                Unit::Word,
                t,
                w_start,
                w_end,
                line_end,
                wi,
                &wctx,
            );
            // A word's plate: from its first letter's drawn pen edge to its last's
            // (spaced, and scaled by the word as its letters are), on its first
            // letter's row, moved by the word's offsets.
            if let Some(first) = by_word[wi].first() {
                if let Some(&(top, bottom)) = p.rows.get(first.row) {
                    let em = first.font_size;
                    let (sx, s) = (
                        (word_st.scale * word_st.scale_x) as f32,
                        word_st.scale as f32,
                    );
                    let (x0, x1) = by_word[wi].iter().fold((f32::MAX, f32::MIN), |(a, b), g| {
                        let ax = anchor_of(g, wb)[0];
                        (
                            a.min(edge(g, g.pen[0], ax, sx)),
                            b.max(edge(g, g.pen[1], ax, sx)),
                        )
                    });
                    let ay = anchor_of(first, wb)[1];
                    let bx = [x0, ay + (top - ay) * s, x1, ay + (bottom - ay) * s];
                    let xf = UnitXform {
                        scale: [sx, s],
                        rot: word_st.rotation as f32,
                        offset: [word_st.offset_x as f32 * em, word_st.offset_y as f32 * em],
                    };
                    for &(pass, pl) in plate_passes.iter().filter(|(_, pl)| pl.on == Unit::Word) {
                        plates.extend(plate_of(
                            pass,
                            pl,
                            &wctx,
                            &word_st,
                            line_st.opacity * word_st.opacity,
                            em,
                            bx,
                            xf,
                        ));
                    }
                }
            }
            for g in &by_word[wi] {
                let gctx = Ctx {
                    word_index: wi,
                    index: g.index,
                    index_in_word: g.index_in_word,
                    word: word_vars,
                    ..base
                };
                // Plate properties start afresh on letters: a word's are its own plate's.
                let mut st = State {
                    opacity: word_st.opacity,
                    scale: word_st.scale,
                    scale_x: word_st.scale_x,
                    offset_x: word_st.offset_x,
                    offset_y: word_st.offset_y,
                    rotation: word_st.rotation,
                    blur: word_st.blur,
                    color: word_st.color,
                    color2: word_st.color2,
                    wipe: word_st.wipe,
                    glow: word_st.glow,
                    wipe_edge: word_st.wipe_edge,
                    plate_opacity: 1.0,
                    plate_scale_x: 1.0,
                    ..Default::default()
                };
                let (sk, s_start, s_end, s_span) = syllable_of(g);
                self.fold(
                    &mut st,
                    Unit::Syllable,
                    t,
                    s_start,
                    s_end,
                    line_end,
                    sk,
                    &gctx,
                );
                let wipe_span = if syllable_wipe {
                    s_span
                } else {
                    [wb.x0, wb.x1]
                };
                self.fold(
                    &mut st,
                    Unit::Letter,
                    t,
                    w_start,
                    w_end,
                    line_end,
                    g.index,
                    &gctx,
                );
                let alpha = (line_st.opacity * st.opacity) as f32;
                let (s, sx) = (st.scale as f32, (st.scale * st.scale_x) as f32);
                // anchor, then scale -> rotate -> translate
                let anchor = anchor_of(g, wb);
                let rot = st.rotation as f32 + if compose { wb.rot } else { 0.0 };
                let (dx, dy) = ((g.cx - anchor[0]) * sx, (g.cy - anchor[1]) * s);
                let (c, sn) = (rot.cos(), rot.sin());
                let em = g.font_size;
                let (rx, ry) = if compose || self.effect.layout.anchor == Anchor::Unit {
                    (dx * c - dy * sn, dx * sn + dy * c)
                } else {
                    (dx, dy)
                };
                // A letter's plate: its drawn pen span (spaced, scaled about the
                // letter's anchor as the letter is) on its row, moved with it.
                if let Some(&(top, bottom)) = p.rows.get(g.row) {
                    let bx = [
                        edge(g, g.pen[0], anchor[0], sx),
                        anchor[1] + (top - anchor[1]) * s,
                        edge(g, g.pen[1], anchor[0], sx),
                        anchor[1] + (bottom - anchor[1]) * s,
                    ];
                    let xf = UnitXform {
                        scale: [sx, s],
                        rot,
                        offset: [st.offset_x as f32 * em, st.offset_y as f32 * em],
                    };
                    for &(pass, pl) in plate_passes.iter().filter(|(_, pl)| pl.on == Unit::Letter) {
                        plates.extend(plate_of(
                            pass,
                            pl,
                            &gctx,
                            &st,
                            line_st.opacity * st.opacity,
                            em,
                            bx,
                            xf,
                        ));
                    }
                }
                let (cx, cy, glyph_rot, sx, s) = match &lx {
                    None => {
                        // Line offsets and letter spacing are in the line's em, so every
                        // row of a line moves alike whatever its words' sizes.
                        let line_dx =
                            (line_st.offset_x + line_st.tracking * g.row_pos as f64) as f32 * p.em;
                        let cx = anchor[0] + rx + st.offset_x as f32 * em + line_dx;
                        let cy = anchor[1]
                            + ry
                            + st.offset_y as f32 * em
                            + line_st.offset_y as f32 * p.em;
                        // Only shift a stacked window line: adding 0.0 would turn -0.0 into +0.0.
                        let cy = if shift != 0.0 { cy + shift } else { cy };
                        (cx, cy, rot, sx, s)
                    }
                    Some(x) => {
                        // Letter spacing, then the line's scale and rotation, then its
                        // offsets and slot.
                        let spaced = (line_st.tracking * g.row_pos as f64) as f32 * p.em;
                        let q = x.apply([
                            anchor[0] + rx + st.offset_x as f32 * em + spaced,
                            anchor[1] + ry + st.offset_y as f32 * em,
                        ]);
                        let [cx, cy] = line_moved(q);
                        (cx, cy, rot + x.rot, sx * x.scale[0], s * x.scale[1])
                    }
                };
                let blur = (st.blur + line_st.blur) as f32;
                for pass in &self.effect.passes {
                    if pass.plate.is_some() {
                        continue;
                    }
                    let pa = num(&eval(&pass.alpha, &gctx)) as f32;
                    let base_color = st.color.unwrap_or_else(|| {
                        if compose {
                            wb.color
                        } else {
                            col(&eval(&pass.color, &gctx))
                        }
                    });
                    let mut inst = Inst {
                        center: [cx, cy],
                        size: [g.sprite.w, g.sprite.h],
                        xform: [glyph_rot, sx, s, 0.0],
                        uv: g.sprite.uv,
                        color: [0.0; 4],
                        color2: [0.0; 4],
                        params: [
                            if g.sprite.msdf { 1.0 } else { 0.0 },
                            0.0,
                            0.0,
                            g.sprite.ppt,
                        ],
                        // time, word progress and unit index for plugin shader hooks
                        extra: [
                            -1.0,
                            t as f32,
                            (((t - w_start) / (w_end - w_start).max(1e-6)).clamp(0.0, 1.0)) as f32,
                            g.index as f32,
                        ],
                        aux: [0.0, PASS_FILL, since, until],
                    };
                    // Where the wipe splits the glyph (local x) and its edge fade
                    // (local width, 0 for the hard edge), for a wipe under way.
                    let split = |wp: f64| -> (f32, f32) {
                        let left = g.cx - g.sprite.w * 0.5;
                        match st.wipe_edge.filter(|e| *e > 0.0) {
                            None => {
                                let split_x =
                                    wipe_span[0] + wp as f32 * (wipe_span[1] - wipe_span[0]);
                                ((split_x - left) / g.sprite.w, 0.0)
                            }
                            // A soft edge enters wholly left of the span and leaves
                            // wholly right of it, so it never pops in or out.
                            Some(e) => {
                                let soft = e as f32 * em;
                                let split_x = wipe_span[0] - soft * 0.5
                                    + wp as f32 * (wipe_span[1] - wipe_span[0] + soft);
                                ((split_x - left) / g.sprite.w, soft / g.sprite.w)
                            }
                        }
                    };
                    match pass.kind.as_str() {
                        "shadow" => {
                            inst.aux[1] = PASS_SHADOW;
                            inst.center[1] += num(&eval(&pass.offset_y, &gctx)) as f32 * em;
                            inst.params[1] = num(&eval(&pass.blur, &gctx)) as f32 + blur;
                            inst.color = premul(col(&eval(&pass.color, &gctx)), pa * alpha);
                            inst.xform[3] = clip_of(&inst);
                            main.push(inst);
                        }
                        "glow" => {
                            inst.aux[1] = PASS_GLOW;
                            let gain = num(&eval(&pass.gain, &gctx)) as f32
                                * st.glow.unwrap_or(1.0) as f32;
                            let glow_rgb = col(&eval(&pass.color, &gctx));
                            let scaled =
                                |c: [f32; 4]| [c[0] * gain, c[1] * gain, c[2] * gain, c[3] * gain];
                            inst.color = scaled(premul(glow_rgb, pa * alpha));
                            // The sung side's own opacity (alpha2), split where the fill's
                            // wipe splits; gain and the glow track scale both sides.
                            if let (Some(wp), Some(v)) = (st.wipe, &pass.alpha2) {
                                let c2 =
                                    scaled(premul(glow_rgb, num(&eval(v, &gctx)) as f32 * alpha));
                                if wp >= 1.0 {
                                    inst.color = c2;
                                } else if wp > 0.0 {
                                    (inst.extra[0], inst.aux[0]) = split(wp);
                                    inst.color2 = c2;
                                }
                            }
                            inst.params[1] = blur;
                            inst.xform[3] = clip_of(&inst);
                            glow.push(inst);
                        }
                        _ => {
                            if pass.kind == "outline_only" {
                                inst.aux[1] = PASS_OUTLINE;
                            }
                            let a = pa * alpha * if compose { wb.alpha } else { 1.0 };
                            inst.color = premul(base_color, a);
                            inst.params[1] = blur;
                            if pass.kind == "outline_only" || (compose && wb.outline > 0.0) {
                                inst.params[2] = if compose {
                                    wb.outline
                                } else {
                                    num(&eval(&pass.outline, &gctx)) as f32 * em
                                };
                            }
                            // The letter's ink (sprite minus its atlas padding, plus any
                            // outline), scaled and rotated as drawn; shadows and glows aside.
                            if (self.checked || self.frame_check) && !*overflow && a > 0.0 {
                                let pad = if g.sprite.msdf {
                                    super::shaders::MSDF_RANGE
                                } else {
                                    super::text::COV_PAD as f32
                                } * g.sprite.ppt;
                                let half = [
                                    ((g.sprite.w - 2.0 * pad).max(0.0) * 0.5 + inst.params[2]) * sx,
                                    ((g.sprite.h - 2.0 * pad).max(0.0) * 0.5 + inst.params[2]) * s,
                                ];
                                *overflow = ink_outside(inst.center, half, glyph_rot, self.safe);
                            }
                            if let Some(wp) = st.wipe {
                                let c2 = st
                                    .color2
                                    .or_else(|| pass.color2.as_ref().map(|v| col(&eval(v, &gctx))))
                                    .unwrap_or(base_color);
                                // The sung side's own opacity (pass alpha2), else the pass's.
                                let a2 = match &pass.alpha2 {
                                    Some(v) => a / pa.max(1e-6) * num(&eval(v, &gctx)) as f32,
                                    None => a,
                                };
                                let a = a2;
                                if wp >= 1.0 {
                                    inst.color = premul(c2, a);
                                } else if wp > 0.0 {
                                    (inst.extra[0], inst.aux[0]) = split(wp);
                                    inst.color2 = premul(c2, a);
                                }
                            }
                            inst.xform[3] = clip_of(&inst);
                            main.push(inst);
                        }
                    }
                }
            }
        }
        main.splice(plates_at..plates_at, plates);
    }

    /// A plate around `bx` (x0, y0, x1, y1 in canvas px: its unit's letters as
    /// drawn, before their offsets): grown by its margins (in the unit's em, which
    /// the unit's scale enlarges as it does its letters), scaled and wiped by its
    /// unit's plate properties, turned by the unit's rotation about its centre and
    /// moved by its offsets, then moved with its line (`lx`, `moved`).
    #[allow(clippy::too_many_arguments)]
    fn plate(
        &self,
        pass: &Pass,
        pl: &Plate,
        ctx: &Ctx,
        unit: &State,
        alpha: f64,
        em: f32,
        bx: [f32; 4],
        unit_xf: UnitXform,
        lx: Option<LineXform>,
        moved: impl Fn([f32; 2]) -> [f32; 2],
    ) -> Option<Inst> {
        let a = (num(&eval(&pass.alpha, ctx)) * alpha * unit.plate_opacity) as f32;
        let grow = unit.plate_margin as f32;
        let mx = (num(&eval(&pl.margin_x, ctx)) as f32 + grow) * em * unit_xf.scale[0].abs();
        let my = (num(&eval(&pl.margin_y, ctx)) as f32 + grow) * em * unit_xf.scale[1].abs();
        let (x0, x1) = (bx[0].min(bx[2]), bx[0].max(bx[2]));
        let (y0, y1) = (bx[1].min(bx[3]), bx[1].max(bx[3]));
        let full = (x1 - x0 + 2.0 * mx) * unit.plate_scale_x as f32;
        let h = y1 - y0 + 2.0 * my;
        let centre = [(x0 + x1) * 0.5, (y0 + y1) * 0.5];
        // A wiped plate keeps its left edge: its centre moves left along the unit.
        let (w, d) = match unit.plate_wipe {
            Some(wp) => (full * wp as f32, -full * (1.0 - wp as f32) * 0.5),
            None => (full, 0.0),
        };
        if a <= 0.0 || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (c, s) = (unit_xf.rot.cos(), unit_xf.rot.sin());
        let mut at = [
            centre[0] + d * c + unit_xf.offset[0],
            centre[1] + d * s + unit_xf.offset[1],
        ];
        let (mut w, mut h) = (w, h);
        let mut rot = unit_xf.rot;
        if let Some(x) = lx {
            at = x.apply(at);
            w *= x.scale[0].abs();
            h *= x.scale[1].abs();
            rot += x.rot;
        }
        let at = moved(at);
        let radius = (num(&eval(&pl.radius, ctx)) as f32 * em).max(0.0)
            * unit_xf.scale[1].abs()
            * lx.map_or(1.0, |x| x.scale[1].abs());
        // Anti-aliasing over one physical pixel; the quad grows by it on every side.
        let aa = 1.0 / self.atlas.px_scale.max(1e-3);
        let color = premul(col(&eval(&pass.color, ctx)), a);
        Some(Inst {
            center: at,
            size: [w + 2.0 * aa, h + 2.0 * aa],
            xform: [rot, 1.0, 1.0, 0.0],
            uv: [w, h, radius, aa],
            color,
            color2: color,
            params: [2.0, 0.0, 0.0, 1.0],
            extra: [-1.0, 0.0, 0.0, 0.0],
            aux: [0.0, PASS_PLATE, 0.0, 0.0],
        })
    }
}

/// A unit's own move for its plate: its scale (x, y; the box already holds it,
/// margins and radius take it), its rotation about the plate's centre, then an
/// offset, in canvas px.
#[derive(Clone, Copy)]
struct UnitXform {
    scale: [f32; 2],
    rot: f32,
    offset: [f32; 2],
}

impl UnitXform {
    const NONE: UnitXform = UnitXform {
        scale: [1.0, 1.0],
        rot: 0.0,
        offset: [0.0, 0.0],
    };
}

/// A line's own scale (x, y) and rotation about its block's centre.
#[derive(Clone, Copy)]
struct LineXform {
    centre: [f32; 2],
    scale: [f32; 2],
    rot: f32,
}

impl LineXform {
    /// None when the line is neither scaled nor rotated.
    fn of(st: &State, centre: [f32; 2]) -> Option<Self> {
        if st.scale == 1.0 && st.scale_x == 1.0 && st.rotation == 0.0 {
            return None;
        }
        Some(Self {
            centre,
            scale: [(st.scale * st.scale_x) as f32, st.scale as f32],
            rot: st.rotation as f32,
        })
    }

    fn apply(&self, q: [f32; 2]) -> [f32; 2] {
        let d = [
            (q[0] - self.centre[0]) * self.scale[0],
            (q[1] - self.centre[1]) * self.scale[1],
        ];
        let (c, s) = (self.rot.cos(), self.rot.sin());
        [
            self.centre[0] + d[0] * c - d[1] * s,
            self.centre[1] + d[0] * s + d[1] * c,
        ]
    }
}

/// Cues cut into chunks of `max` words, each a line of its own: it starts with
/// its first word and ends where the next chunk of the same cue starts showing
/// (its start minus the pre-roll, never before its own start); the last chunk
/// ends with the cue. Word rows become breaks; otherwise a chunk rewraps.
fn chunked(lines: &[Line], c: Chunk, preroll: f64) -> Vec<Line> {
    let mut out = Vec::new();
    for l in lines {
        let groups: Vec<&[super::line::Word]> = l.words.chunks(c.max).collect();
        for (k, words) in groups.iter().enumerate() {
            let start = words[0].start;
            let end = match groups.get(k + 1) {
                Some(next) => (next[0].start - preroll).max(start),
                None => l.end,
            };
            out.push(Line {
                start,
                end,
                words: words.to_vec(),
                sep: l.sep.clone(),
                breaks: if c.rows_word {
                    (1..words.len()).collect()
                } else {
                    Vec::new()
                },
            });
        }
    }
    out
}

/// The longest-sung word of a line (the first on a tie); None without words.
fn longest_word(line: &Line) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, w) in line.words.iter().enumerate() {
        let d = w.end - w.start;
        if best.is_none_or(|(_, b)| d > b) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Each line's visible span. A line shows from `start - preroll`, but never
/// before the previous line's span ends (the pre-roll does not cut it short),
/// and stays after its end until the next line's pre-roll, at most `hold`.
/// A line left with no time at all keeps an empty span.
pub(crate) fn shown_spans(lines: &[Line], preroll: f64, hold: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        let prev_end = out.last().map_or(f64::NEG_INFINITY, |s| s.1);
        let from = (l.start - preroll).max(prev_end);
        let held = match lines.get(i + 1) {
            Some(next) => (l.end + hold).min(next.start - preroll),
            None => l.end + hold,
        };
        let to = l.end.max(held).max(from);
        out.push((from, to));
    }
    out
}

/// Letters may touch the safe edge by this much (anti-aliasing) before a report.
pub const OVERFLOW_TOLERANCE: f32 = 2.0;

/// Whether a rotated ink box (centre, half extents, rotation in radians) leaves
/// the safe rectangle `[x0, y0, w, h]` by more than OVERFLOW_TOLERANCE.
pub fn ink_outside(center: [f32; 2], half: [f32; 2], rot: f32, safe: [f32; 4]) -> bool {
    let (c, s) = (rot.cos(), rot.sin());
    let [x0, y0, w, h] = safe;
    let t = OVERFLOW_TOLERANCE;
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .any(|(kx, ky)| {
            let (dx, dy) = (kx * half[0], ky * half[1]);
            let (x, y) = (center[0] + dx * c - dy * s, center[1] + dx * s + dy * c);
            x < x0 - t || x > x0 + w + t || y < y0 - t || y > y0 + h + t
        })
}

trait IntoRole {
    fn into_role(self) -> (Role, [f32; 2], bool);
}

impl IntoRole for ([f32; 2], Role, bool) {
    fn into_role(self) -> (Role, [f32; 2], bool) {
        (self.1, self.0, self.2)
    }
}

fn solid(cx: f32, cy: f32, w: f32, h: f32, color: [f32; 4]) -> Inst {
    Inst {
        center: [cx, cy],
        size: [w, h],
        xform: [0.0, 1.0, 1.0, 0.0],
        uv: [0.0; 4],
        color,
        color2: color,
        params: [2.0, 0.0, 0.0, 1.0],
        extra: [-1.0, 0.0, 0.0, 0.0],
        aux: [0.0, PASS_PLATE, 0.0, 0.0],
    }
}
