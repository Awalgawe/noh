//! Generic effect engine driven by plugin files (archi-greffons-v2). The engine
//! knows layouts, tracks and passes; it knows no effect by name.
use crate::effects::Line;
use crate::gpu::{Inst, Upload};
use crate::text::{Atlas, Face, Fonts, PGlyph, Sprite, layout};
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
}

#[derive(Clone, Debug)]
pub enum PVal {
    Num(f64),
    Color([u8; 3]),
    Text(String),
    Palette(Vec<[u8; 3]>),
    Bool(bool),
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

fn parse_v(j: &J, field: &str) -> Result<V, LoadError> {
    Ok(match j {
        J::Number(n) => V::Num(n.as_f64().unwrap_or(0.0)),
        J::String(s) if s.starts_with('$') => V::Param(s[1..].to_string(), field.to_string()),
        J::String(s) if s.starts_with('#') => V::Color(hex(s).ok_or_else(|| LoadError(format!("{field}: bad colour {s}")))?),
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
        J::Object(o) if o.len() == 1 => {
            let (k, v) = o.iter().next().unwrap();
            let args = v.as_array().ok_or_else(|| LoadError(format!("{field}: operator {k} needs an array")))?;
            match (k.as_str(), args.len()) {
                ("mix", 3) => V::Mix(Box::new(parse_v(&args[0], field)?), Box::new(parse_v(&args[1], field)?), Box::new(parse_v(&args[2], field)?)),
                ("scale", 2) => V::Scale(Box::new(parse_v(&args[0], field)?), Box::new(parse_v(&args[1], field)?)),
                ("pick", 2) => {
                    let scope = args[1].as_str().unwrap_or("");
                    if !matches!(scope, "word_index" | "index" | "index_in_word") {
                        return err(field, format!("pick scope {scope:?} unknown"));
                    }
                    V::Pick(Box::new(parse_v(&args[0], field)?), scope.into())
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
}

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
            let (a, b, t) = (col(&eval(a, c)), col(&eval(b, c)), num(&eval(t, c)) as f32);
            let ch = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
            R::Color([ch(a[0], b[0]), ch(a[1], b[1]), ch(a[2], b[2])])
        }
        V::Pick(p, scope) => {
            let i = match scope.as_str() {
                "word_index" => c.word_index,
                "index_in_word" => c.index_in_word,
                _ => c.index,
            };
            match eval(p, c) {
                R::Palette(p) if !p.is_empty() => R::Color(p[i % p.len()]),
                other => other,
            }
        }
    }
}

fn num(r: &R) -> f64 {
    if let R::Num(n) = r { *n } else { 0.0 }
}

fn col(r: &R) -> [u8; 3] {
    if let R::Color(c) = r { *c } else { [255, 255, 255] }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Line,
    Word,
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Back,
}

#[derive(Clone, Debug)]
enum Shape {
    Keys(Vec<(f64, V)>, Curve),
    Wave { amplitude: V, frequency: f64, phase_per_unit: f64, phase: f64 },
    Noise { amplitude: V, frequency: f64, seed: u64 },
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
}

#[derive(Clone, Debug)]
struct SlotSpec {
    x: V,
    y: V,
    style: Option<Role>,
}

#[derive(Clone, Debug)]
struct ShapeSpec {
    x0: V,
    y0: V,
    x1: V,
    y1: V,
    color: V,
    alpha: V,
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
}

#[derive(Clone, Debug)]
struct Pass {
    kind: String,
    color: V,
    color2: Option<V>,
    alpha: V,
    offset_y: V,
    blur: V,
    outline: V,
    gain: V,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub id: String,
    pub params: Vec<Param>,
    pub presets: Vec<(String, HashMap<String, J>)>,
    layout: Layout,
    tracks: Vec<Track>,
    passes: Vec<Pass>,
    pub shader: Option<String>,
    pub load_ms: f64,
}

fn obj<'a>(j: &'a J, field: &str) -> Result<&'a serde_json::Map<String, J>, LoadError> {
    j.as_object().ok_or_else(|| LoadError(format!("{field}: expected an object")))
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
    only(o, field, &["font", "size", "case", "rotation", "color", "alpha", "outline", "align", "require", "pivot"])?;
    let pivot = match o.get("pivot") {
        None => None,
        Some(J::Object(p)) if !p.contains_key("landscape") && !p.contains_key("portrait") => {
            only(p, &format!("{field}.pivot"), &["x", "y"])?;
            let (Some(x), Some(y)) = (p.get("x"), p.get("y")) else {
                return err(&format!("{field}.pivot"), "needs both x and y");
            };
            Some((parse_v(x, &format!("{field}.pivot.x"))?, parse_v(y, &format!("{field}.pivot.y"))?))
        }
        Some(J::Array(a)) if a.len() == 2 => Some((parse_v(&a[0], field)?, parse_v(&a[1], field)?)),
        Some(other) => {
            // orientation-dependent pivot: {"landscape": [x,y], "portrait": [x,y]}
            let p = obj(other, &format!("{field}.pivot"))?;
            only(p, &format!("{field}.pivot"), &["landscape", "portrait"])?;
            let pick = |k: &str| -> Result<(V, V), LoadError> {
                let a = p.get(k).and_then(J::as_array).filter(|a| a.len() == 2).ok_or_else(|| LoadError(format!("{field}.pivot.{k}: expected [x, y]")))?;
                Ok((parse_v(&a[0], field)?, parse_v(&a[1], field)?))
            };
            let (l, pt) = (pick("landscape")?, pick("portrait")?);
            Some((V::Orient(Box::new(l.0), Box::new(pt.0)), V::Orient(Box::new(l.1), Box::new(pt.1))))
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
        font: get_v(o, field, "font", J::from("sans"))?,
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
    let arr = j.as_array().ok_or_else(|| LoadError(format!("{field}: expected an array")))?;
    arr.iter()
        .enumerate()
        .map(|(i, s)| {
            let f = format!("{field}[{i}]");
            let o = obj(s, &f)?;
            only(o, &f, &["x", "y", "style"])?;
            Ok(SlotSpec {
                x: get_v(o, &f, "x", J::from(0.0))?,
                y: get_v(o, &f, "y", J::from(0.0))?,
                style: o.get("style").map(|s| parse_role(s, &format!("{f}.style"))).transpose()?,
            })
        })
        .collect()
}

fn parse_keys(j: &J, field: &str) -> Result<Vec<(f64, V)>, LoadError> {
    let arr = j.as_array().ok_or_else(|| LoadError(format!("{field}: expected [[t, v], ...]")))?;
    if arr.is_empty() {
        return err(field, "empty keyframes");
    }
    let mut out = Vec::new();
    for (i, k) in arr.iter().enumerate() {
        let pair = k.as_array().filter(|p| p.len() == 2).ok_or_else(|| LoadError(format!("{field}[{i}]: expected [t, v]")))?;
        let t = pair[0].as_f64().ok_or_else(|| LoadError(format!("{field}[{i}]: t must be a number")))?;
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
            if e.0.starts_with("effect.json") || e.0.starts_with("shader ") || e.0.contains(":\\") || e.0.contains(":/") {
                e
            } else {
                LoadError(format!("effect.json › {}", e.0))
            }
        })
    }

    fn load_inner(dir: &Path) -> Result<Self, LoadError> {
        let started = Instant::now();
        let path = dir.join("effect.json");
        let bytes = std::fs::read(&path).map_err(|e| LoadError(format!("{}: {e}", path.display())))?;
        if bytes.len() > MAX_JSON {
            return err("effect.json", format!("{} bytes exceeds {MAX_JSON}", bytes.len()));
        }
        let root: J = serde_json::from_slice(&bytes).map_err(|e| LoadError(format!("effect.json: {e}")))?;
        let o = obj(&root, "effect.json")?;
        only(o, "effect.json", &["engine", "id", "name", "version", "license", "target", "params", "presets", "layout", "animate", "render", "shader"])?;
        match o.get("engine").and_then(J::as_u64) {
            Some(ENGINE_API) => {}
            other => return err("engine", format!("API version {} is not supported (this engine: {ENGINE_API})", other.map_or("absent".to_string(), |v| v.to_string()))),
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
        for (i, p) in o.get("params").and_then(J::as_array).cloned().unwrap_or_default().iter().enumerate() {
            let f = format!("params[{i}]");
            let po = obj(p, &f)?;
            only(po, &f, &["id", "type", "default", "level", "label", "min", "max", "unit", "choices"])?;
            let kind = get_s(po, "type", "");
            if !matches!(kind.as_str(), "color" | "number" | "choice" | "bool" | "font" | "palette") {
                return err(&format!("{f}.type"), format!("unknown type {kind:?}"));
            }
            let level = po.get("level").and_then(J::as_u64).unwrap_or(2) as u8;
            if !(1..=2).contains(&level) {
                return err(&format!("{f}.level"), "must be 1 or 2");
            }
            let default = po.get("default").cloned().ok_or_else(|| LoadError(format!("{f}.default: missing")))?;
            let param = Param { id: get_s(po, "id", ""), kind, default, level };
            // type check the default (or both orientation defaults)
            let check = |v: &J| -> bool {
                match param.kind.as_str() {
                    "color" => v.as_str().and_then(hex).is_some(),
                    "number" => v.is_number(),
                    "bool" => v.is_boolean(),
                    "palette" => v.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|c| c.as_str().and_then(hex).is_some())),
                    _ => v.is_string(),
                }
            };
            let ok = match &param.default {
                J::Object(m) => m.len() == 2 && m.get("landscape").is_some_and(check) && m.get("portrait").is_some_and(check),
                v => check(v),
            };
            if !ok {
                return err(&format!("{f}.default"), format!("does not match type {}", param.kind));
            }
            params.push(param);
        }
        if params.len() > MAX_PARAMS {
            return err("params", format!("{} params exceed {MAX_PARAMS}", params.len()));
        }
        let level1 = params.iter().filter(|p| p.level == 1).count();
        if level1 > MAX_LEVEL1 {
            return err("params", format!("{level1} level-1 params exceed {MAX_LEVEL1}"));
        }
        let ids: Vec<String> = params.iter().map(|p| p.id.clone()).collect();
        // presets
        let mut presets = Vec::new();
        for (i, p) in o.get("presets").and_then(J::as_array).cloned().unwrap_or_default().iter().enumerate() {
            let f = format!("presets[{i}]");
            let po = obj(p, &f)?;
            only(po, &f, &["id", "name", "values"])?;
            let values: HashMap<String, J> = po.get("values").and_then(J::as_object).map(|m| m.clone().into_iter().collect()).unwrap_or_default();
            for k in values.keys() {
                if !ids.contains(k) {
                    return err(&format!("{f}.values"), format!("unknown param {k:?}"));
                }
            }
            presets.push((get_s(po, "id", ""), values));
        }
        // layout
        let lj = o.get("layout").ok_or_else(|| LoadError("layout: missing".into()))?;
        let lo = obj(lj, "layout")?;
        only(lo, "layout", &["mode", "font", "size", "case", "line_height", "box", "raster", "visible", "anchor", "reserve", "compose"])?;
        let mode = get_s(lo, "mode", "line");
        if !matches!(mode.as_str(), "line" | "compose") {
            return err("layout.mode", format!("{mode:?} unknown (line, compose)"));
        }
        let bo = lo.get("box").map(|b| obj(b, "layout.box")).transpose()?;
        let empty = serde_json::Map::new();
        let bo = bo.unwrap_or(&empty);
        only(bo, "layout.box", &["x", "y", "width", "anchor"])?;
        let bx = BoxSpec {
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
                only(co, "layout.compose", &["rank", "stopwords", "roles", "slots", "shapes", "order"])?;
                let rank: Vec<String> = co.get("rank").and_then(J::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                for k in &rank {
                    if !matches!(k.as_str(), "content_word" | "duration" | "length") {
                        return err("layout.compose.rank", format!("key {k:?} unknown"));
                    }
                }
                let roles = obj(co.get("roles").ok_or_else(|| LoadError("layout.compose.roles: missing".into()))?, "layout.compose.roles")?;
                only(roles, "layout.compose.roles", &["carrier", "second", "others"])?;
                let role = |k: &str| -> Result<Role, LoadError> { parse_role(roles.get(k).ok_or_else(|| LoadError(format!("layout.compose.roles.{k}: missing")))?, &format!("layout.compose.roles.{k}")) };
                let so = obj(co.get("slots").ok_or_else(|| LoadError("layout.compose.slots: missing".into()))?, "layout.compose.slots")?;
                only(so, "layout.compose.slots", &["landscape", "portrait"])?;
                let slots = V2 {
                    landscape: parse_slots(so.get("landscape").unwrap_or(&J::Array(vec![])), "layout.compose.slots.landscape")?,
                    portrait: parse_slots(so.get("portrait").unwrap_or(&J::Array(vec![])), "layout.compose.slots.portrait")?,
                };
                let mut shapes = Vec::new();
                for (i, s) in co.get("shapes").and_then(J::as_array).cloned().unwrap_or_default().iter().enumerate() {
                    let f = format!("layout.compose.shapes[{i}]");
                    let so = obj(s, &f)?;
                    only(so, &f, &["x0", "y0", "x1", "y1", "color", "alpha"])?;
                    shapes.push(ShapeSpec {
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
                    stopwords: co.get("stopwords").and_then(J::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                    carrier: role("carrier")?,
                    second: role("second")?,
                    others: role("others")?,
                    slots,
                    shapes,
                    order: co.get("order").and_then(J::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_else(|| vec!["shapes".into(), "carrier".into(), "second".into(), "others".into()]),
                })
            }
        };
        if mode == "compose" && compose.is_none() {
            return err("layout.compose", "missing for mode compose");
        }
        let layout = Layout {
            mode,
            font: get_v(lo, "layout", "font", J::from("sans"))?,
            size: get_v(lo, "layout", "size", J::from(64))?,
            case: get_s(lo, "case", "none"),
            line_height: get_v(lo, "layout", "line_height", J::from(1.18))?,
            bx,
            raster: get_v(lo, "layout", "raster", J::from(1.0))?,
            visible,
            anchor,
            compose,
        };
        // tracks
        let tracks_j = o.get("animate").and_then(J::as_array).cloned().unwrap_or_default();
        if tracks_j.len() > MAX_TRACKS {
            return err("animate", format!("{} tracks exceed {MAX_TRACKS}", tracks_j.len()));
        }
        let mut tracks = Vec::new();
        for (i, t) in tracks_j.iter().enumerate() {
            let f = format!("animate[{i}]");
            let to = obj(t, &f)?;
            only(to, &f, &["unit", "slot", "property", "duration", "stagger", "keys", "curve", "wave", "noise", "value"])?;
            let unit = match get_s(to, "unit", "word").as_str() {
                "line" => Unit::Line,
                "word" => Unit::Word,
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
                p => return err(&format!("{f}.property"), format!("{p:?} unknown")),
            };
            let curve = match get_s(to, "curve", "linear").as_str() {
                "linear" => Curve::Linear,
                "ease_in" => Curve::EaseIn,
                "ease_out" => Curve::EaseOut,
                "ease_in_out" => Curve::EaseInOut,
                "back" => Curve::Back,
                c => return err(&format!("{f}.curve"), format!("{c:?} unknown")),
            };
            let shape = if let Some(k) = to.get("keys") {
                Shape::Keys(parse_keys(k, &format!("{f}.keys"))?, curve)
            } else if let Some(w) = to.get("wave") {
                let wo = obj(w, &format!("{f}.wave"))?;
                only(wo, &format!("{f}.wave"), &["amplitude", "frequency", "phase_per_unit", "phase"])?;
                Shape::Wave {
                    amplitude: get_v(wo, &f, "amplitude", J::from(0.0))?,
                    frequency: wo.get("frequency").and_then(J::as_f64).unwrap_or(1.0),
                    phase_per_unit: wo.get("phase_per_unit").and_then(J::as_f64).unwrap_or(0.0),
                    phase: wo.get("phase").and_then(J::as_f64).unwrap_or(0.0),
                }
            } else if let Some(n) = to.get("noise") {
                let no = obj(n, &format!("{f}.noise"))?;
                only(no, &format!("{f}.noise"), &["amplitude", "frequency", "seed"])?;
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
            tracks.push(Track { unit, slot, prop, duration: to.get("duration").and_then(J::as_f64).unwrap_or(0.3), stagger: to.get("stagger").and_then(J::as_f64).unwrap_or(0.0), shape });
        }
        // passes
        let ro = obj(o.get("render").ok_or_else(|| LoadError("render: missing".into()))?, "render")?;
        only(ro, "render", &["passes"])?;
        let mut passes = Vec::new();
        for (i, p) in ro.get("passes").and_then(J::as_array).cloned().unwrap_or_default().iter().enumerate() {
            let f = format!("render.passes[{i}]");
            let po = obj(p, &f)?;
            only(po, &f, &["kind", "color", "color2", "alpha", "offset_y", "blur", "outline", "gain"])?;
            let kind = get_s(po, "kind", "");
            if !matches!(kind.as_str(), "fill" | "shadow" | "glow" | "outline_only") {
                return err(&format!("{f}.kind"), format!("{kind:?} unknown"));
            }
            passes.push(Pass {
                kind,
                color: get_v(po, &f, "color", J::from("#FFFFFF"))?,
                color2: po.get("color2").map(|c| parse_v(c, &format!("{f}.color2"))).transpose()?,
                alpha: get_v(po, &f, "alpha", J::from(1.0))?,
                offset_y: get_v(po, &f, "offset_y", J::from(0.0))?,
                blur: get_v(po, &f, "blur", J::from(0.0))?,
                outline: get_v(po, &f, "outline", J::from(0.0))?,
                gain: get_v(po, &f, "gain", J::from(1.0))?,
            });
        }
        // shader (black box)
        let shader = match o.get("shader").and_then(J::as_str) {
            None => None,
            Some(rel) => {
                if rel.contains("..") || rel.starts_with('/') || rel.contains(':') {
                    return err("shader", "path must stay inside the package");
                }
                let src = std::fs::read_to_string(dir.join(rel)).map_err(|e| LoadError(format!("shader {rel}: {e}")))?;
                if src.len() > MAX_WGSL {
                    return err("shader", format!("{} bytes exceeds {MAX_WGSL}", src.len()));
                }
                crate::plugin_shader::validate(&src).map_err(|e| LoadError(format!("shader {rel}: {e}")))?;
                Some(src)
            }
        };
        let effect = Effect { id, params, presets, layout, tracks, passes, shader, load_ms: 0.0 };
        // every $reference must name a declared param
        let mut refs = Vec::new();
        effect.collect_refs(&mut refs);
        for (r, field) in refs {
            if !ids.contains(&r) {
                return err(&field, format!("${r} names no declared param"));
            }
        }
        Ok(Effect { load_ms: started.elapsed().as_secs_f64() * 1000.0, ..effect })
    }

    fn collect_refs(&self, out: &mut Vec<(String, String)>) {
        let l = &self.layout;
        for v in [&l.font, &l.size, &l.line_height, &l.raster, &l.bx.x, &l.bx.y, &l.bx.width, &l.bx.anchor] {
            param_refs(v, out);
        }
        if let Some(c) = &l.compose {
            for r in [&c.carrier, &c.second, &c.others] {
                for v in [&r.font, &r.size, &r.rotation, &r.color, &r.alpha, &r.outline] {
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
                Shape::Wave { amplitude, .. } | Shape::Noise { amplitude, .. } => param_refs(amplitude, out),
                Shape::Const(v) => param_refs(v, out),
            }
        }
        for p in &self.passes {
            for v in [&p.color, &p.alpha, &p.offset_y, &p.blur, &p.outline, &p.gain] {
                param_refs(v, out);
            }
            if let Some(c) = &p.color2 {
                param_refs(c, out);
            }
        }
    }

    /// Param values: defaults, then the preset, then explicit overrides.
    pub fn values(&self, preset: Option<&str>, overrides: &HashMap<String, J>, portrait: bool) -> HashMap<String, PVal> {
        let mut raw: HashMap<String, J> = self.params.iter().map(|p| (p.id.clone(), p.default.clone())).collect();
        if let Some((_, v)) = preset.and_then(|id| self.presets.iter().find(|(p, _)| p == id)) {
            raw.extend(v.clone());
        }
        raw.extend(overrides.clone());
        self.params
            .iter()
            .map(|p| {
                let mut j = raw[&p.id].clone();
                if let J::Object(m) = &j {
                    j = m.get(if portrait { "portrait" } else { "landscape" }).cloned().unwrap_or(J::Null);
                }
                let v = match p.kind.as_str() {
                    "color" => PVal::Color(j.as_str().and_then(hex).unwrap_or([255, 255, 255])),
                    "number" => PVal::Num(j.as_f64().unwrap_or(0.0)),
                    "bool" => PVal::Bool(j.as_bool().unwrap_or(false)),
                    "palette" => PVal::Palette(j.as_array().map(|a| a.iter().filter_map(|c| c.as_str().and_then(hex)).collect()).unwrap_or_default()),
                    _ => PVal::Text(j.as_str().unwrap_or("").to_string()),
                };
                (p.id.clone(), v)
            })
            .collect()
    }
}

// ---------------------------------------------------------------- runtime

fn ease(c: Curve, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    match c {
        Curve::Linear => x,
        Curve::EaseIn => x * x,
        Curve::EaseOut => 1.0 - (1.0 - x) * (1.0 - x),
        Curve::EaseInOut => {
            if x < 0.5 { 2.0 * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(2) / 2.0 }
        }
        Curve::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
        }
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

fn face_of(name: &str) -> Face {
    match name {
        "cjk" => Face::Cjk,
        "sans_semibold" => Face::SansSemi,
        "jost_extrabold" => Face::Jost,
        "serif_italic" => Face::SerifItalic,
        _ => Face::Sans,
    }
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
    glyphs: Vec<Glyph>,
    words: Vec<WordBox>,
    /// word draw order (compose); identity otherwise
    order: Vec<usize>,
    shapes: Vec<Inst>,
}

pub struct PluginScene {
    pub effect: Effect,
    pub w: f32,
    pub h: f32,
    params: HashMap<String, PVal>,
    lines: Vec<Line>,
    prepared: Option<Prepared>,
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
}

impl PluginScene {
    pub fn new(effect: Effect, w: u32, h: u32, lines: Vec<Line>, preset: Option<&str>) -> Self {
        let params = effect.values(preset, &HashMap::new(), h > w);
        Self { effect, w: w as f32, h: h as f32, params, lines, prepared: None, atlas: Atlas::new(48.0), tracks_us: Vec::new() }
    }

    pub fn with_px_scale(mut self, s: f32) -> Self {
        self.atlas.px_scale = s;
        self
    }

    pub fn line_at(&self, t: f64) -> Option<usize> {
        self.lines.iter().position(|l| t >= l.start && t < l.end)
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { params: &self.params, portrait: self.h > self.w, word_index: 0, index: 0, index_in_word: 0 }
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

    fn place(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, glyphs: &[PGlyph], ox: f32, oy: f32, oversample: f32, word: Option<usize>) -> Vec<Glyph> {
        let mut out = Vec::new();
        let mut per_word: HashMap<usize, usize> = HashMap::new();
        for g in glyphs {
            if let Some(s) = self.atlas.cov_sprite(fonts, gpu, g, oversample) {
                let wi = word.unwrap_or(g.word);
                let iw = per_word.entry(wi).or_insert(0);
                out.push(Glyph { sprite: s, cx: ox + g.x + s.left + s.w * 0.5, cy: oy + g.baseline - s.top + s.h * 0.5, word: wi, index: g.index, index_in_word: *iw, font_size: g.size });
                *iw += 1;
            }
        }
        out
    }

    fn prepare(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, li: usize) {
        let line = self.lines[li].clone();
        let (w, h) = (self.w, self.h);
        let portrait = h > w;
        let lay = self.effect.layout.clone();
        let mut words = vec![WordBox { x0: f32::MAX, x1: f32::MIN, pivot: [0.0; 2], rot: 0.0, color: [255; 3], alpha: 1.0, outline: 0.0 }; line.words.len()];
        let mut order: Vec<usize> = (0..line.words.len()).collect();
        let mut shapes = Vec::new();
        let raster = self.n(&lay.raster);
        let glyphs = if let Some(c) = &lay.compose {
            let keys: Vec<String> = line.words.iter().map(|x| x.text.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase()).collect();
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
            let second = ranked.get(1).copied().filter(|i| c.second.require.as_deref() != Some("content_word") || content(*i));
            for s in &c.shapes {
                let (x0, y0, x1, y1) = (self.n(&s.x0) * w, self.n(&s.y0) * h, self.n(&s.x1) * w, self.n(&s.y1) * h);
                let color = col(&eval(&s.color, &self.ctx()));
                shapes.push(solid((x0 + x1) * 0.5, (y0 + y1) * 0.5, x1 - x0, y1 - y0, premul(color, self.n(&s.alpha))));
            }
            let slots = if portrait { &c.slots.portrait } else { &c.slots.landscape };
            let mut slot_k = 0usize;
            let mut all = Vec::new();
            for (i, word) in line.words.iter().enumerate() {
                let (role, pivot, centred) = if i == carrier {
                    let r = &c.carrier;
                    let p = r.pivot.as_ref().map(|(x, y)| [self.n(x) * w, self.n(y) * h]).unwrap_or([w * 0.5, h * 0.5]);
                    (r.clone(), p, true)
                } else if Some(i) == second {
                    let r = &c.second;
                    let p = r.pivot.as_ref().map(|(x, y)| [self.n(x) * w, self.n(y) * h]).unwrap_or([w * 0.5, h * 0.2]);
                    (r.clone(), p, true)
                } else {
                    let s = &slots[slot_k % slots.len().max(1)];
                    slot_k += 1;
                    let r = s.style.clone().unwrap_or_else(|| c.others.clone());
                    let centred = r.align == "center";
                    ([self.n(&s.x) * w, self.n(&s.y) * h], r, centred).into_role()
                };
                let face = face_of(&self.s(&role.font));
                let size = self.n(&role.size) * h;
                let text = apply_case(&word.text, &role.case);
                let laid = layout(fonts, &[text], "", face, size, None, 1.0);
                let ox = if centred { pivot[0] - laid.width * 0.5 } else { pivot[0] };
                let oy = pivot[1] - laid.height * 0.5;
                let placed = self.place(fonts, gpu, &laid.glyphs, ox, oy, raster, Some(i));
                let p = if centred { pivot } else { [ox + laid.width * 0.5, pivot[1]] };
                let ctx = Ctx { word_index: i, ..self.ctx() };
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
            let face = face_of(&self.s(&lay.font));
            let size = self.n(&lay.size);
            let words_t: Vec<String> = line.words.iter().map(|x| apply_case(&x.text, &lay.case)).collect();
            let max_w = w * self.n(&lay.bx.width);
            let laid = layout(fonts, &words_t, &line.sep, face, size, Some(max_w), self.n(&lay.line_height));
            let ox = self.n(&lay.bx.x) * w - max_w * 0.5;
            let ay = self.n(&lay.bx.y) * h;
            let oy = match self.s(&lay.bx.anchor).as_str() {
                "top" => ay,
                "center" => ay - laid.height * 0.5,
                _ => ay - laid.height,
            };
            for g in &laid.glyphs {
                let e = &mut words[g.word];
                e.x0 = e.x0.min(ox + g.x);
                e.x1 = e.x1.max(ox + g.x + g.advance);
            }
            self.place(fonts, gpu, &laid.glyphs, ox, oy, raster, None)
        };
        self.prepared = Some(Prepared { line: li, glyphs, words, order, shapes });
    }

    /// Evaluate the tracks for one unit level and fold them into `st`.
    fn fold(&self, st: &mut State, unit: Unit, t: f64, start: f64, end: f64, line_end: f64, idx: usize, ctx: &Ctx) {
        for tr in self.effect.tracks.iter().filter(|tr| tr.unit == unit) {
            let s0 = start + tr.stagger * idx as f64;
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
                        let u = if t1 > t0 { ((x - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 1.0 };
                        let (a, b) = (eval(&k[seg].1, ctx), eval(&k[seg + 1].1, ctx));
                        match (a, b) {
                            (R::Num(a), R::Num(b)) => R::Num(a + (b - a) * ease(*c, u)),
                            (a, b) => {
                                if ease(*c, u) < 0.5 { a } else { b }
                            }
                        }
                    }
                }
                Shape::Wave { amplitude, frequency, phase_per_unit, phase } => {
                    let a = num(&eval(amplitude, ctx));
                    R::Num(a * (std::f64::consts::TAU * frequency * t - phase_per_unit * idx as f64 + phase).sin())
                }
                Shape::Noise { amplitude, frequency, seed } => R::Num(num(&eval(amplitude, ctx)) * noise1(t * frequency + idx as f64 * 17.0, *seed)),
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
            }
        }
    }

    pub fn frame(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, t: f64) -> (Vec<Inst>, f32, Vec<Inst>, bool) {
        let Some(li) = self.line_at(t) else {
            self.prepared = None;
            return (Vec::new(), 1.0, Vec::new(), false);
        };
        let changed = self.prepared.as_ref().map(|p| p.line) != Some(li);
        if changed {
            self.prepare(fonts, gpu, li);
        }
        let started = Instant::now();
        let line = &self.lines[li];
        let p = self.prepared.as_ref().unwrap();
        let base = self.ctx();
        let mut line_st = State { opacity: 1.0, scale: 1.0, scale_x: 1.0, ..Default::default() };
        self.fold(&mut line_st, Unit::Line, t, line.start, line.end, line.end, 0, &base);
        let mut glow = Vec::new();
        let mut main = Vec::new();
        for s in &p.shapes {
            let mut s = *s;
            for c in &mut s.color {
                *c *= line_st.opacity as f32;
            }
            main.push(s);
        }
        let compose = self.effect.layout.compose.is_some();
        let from_start = self.effect.layout.visible == "from_start";
        let mut by_word: Vec<Vec<&Glyph>> = vec![Vec::new(); line.words.len()];
        for g in &p.glyphs {
            by_word[g.word].push(g);
        }
        for &wi in &p.order {
            let wd = &line.words[wi];
            if from_start && t < wd.start {
                continue;
            }
            let wb = &p.words[wi];
            let wctx = Ctx { word_index: wi, ..base };
            let mut word_st = State { opacity: 1.0, scale: 1.0, scale_x: 1.0, ..Default::default() };
            self.fold(&mut word_st, Unit::Word, t, wd.start, wd.end, line.end, wi, &wctx);
            for g in &by_word[wi] {
                let gctx = Ctx { word_index: wi, index: g.index, index_in_word: g.index_in_word, ..base };
                let mut st = State { opacity: word_st.opacity, scale: word_st.scale, scale_x: word_st.scale_x, offset_x: word_st.offset_x, offset_y: word_st.offset_y, rotation: word_st.rotation, blur: word_st.blur, color: word_st.color, color2: word_st.color2, wipe: word_st.wipe, glow: word_st.glow };
                self.fold(&mut st, Unit::Letter, t, wd.start, wd.end, line.end, g.index, &gctx);
                let alpha = (line_st.opacity * st.opacity) as f32;
                let (s, sx) = (st.scale as f32, (st.scale * st.scale_x) as f32);
                // anchor, then scale -> rotate -> translate
                let anchor = match self.effect.layout.anchor {
                    Anchor::Unit => [g.cx, g.cy],
                    Anchor::Word => [(wb.x0 + wb.x1) * 0.5, g.cy],
                    Anchor::WordXGlyphY => [(wb.x0 + wb.x1) * 0.5, g.cy],
                    Anchor::Baseline => [(wb.x0 + wb.x1) * 0.5, g.cy],
                    Anchor::Pivot => wb.pivot,
                };
                let rot = st.rotation as f32 + if compose { wb.rot } else { 0.0 };
                let (dx, dy) = ((g.cx - anchor[0]) * sx, (g.cy - anchor[1]) * s);
                let (c, sn) = (rot.cos(), rot.sin());
                let em = g.font_size;
                let (rx, ry) = if compose || self.effect.layout.anchor == Anchor::Unit { (dx * c - dy * sn, dx * sn + dy * c) } else { (dx, dy) };
                let cx = anchor[0] + rx + st.offset_x as f32 * em;
                let cy = anchor[1] + ry + st.offset_y as f32 * em;
                let glyph_rot = rot;
                for pass in &self.effect.passes {
                    let pa = num(&eval(&pass.alpha, &gctx)) as f32;
                    let base_color = st.color.unwrap_or_else(|| if compose { wb.color } else { col(&eval(&pass.color, &gctx)) });
                    let mut inst = Inst {
                        center: [cx, cy],
                        size: [g.sprite.w, g.sprite.h],
                        xform: [glyph_rot, sx, s, 0.0],
                        uv: g.sprite.uv,
                        color: [0.0; 4],
                        color2: [0.0; 4],
                        params: [if g.sprite.msdf { 1.0 } else { 0.0 }, 0.0, 0.0, g.sprite.ppt],
                        // time, word progress and unit index for plugin shader hooks
                        extra: [-1.0, t as f32, (((t - wd.start) / (wd.end - wd.start).max(1e-6)).clamp(0.0, 1.0)) as f32, g.index as f32],
                    };
                    match pass.kind.as_str() {
                        "shadow" => {
                            inst.center[1] += num(&eval(&pass.offset_y, &gctx)) as f32 * em;
                            inst.params[1] = num(&eval(&pass.blur, &gctx)) as f32 + st.blur as f32;
                            inst.color = premul(col(&eval(&pass.color, &gctx)), pa * alpha);
                            main.push(inst);
                        }
                        "glow" => {
                            let gain = num(&eval(&pass.gain, &gctx)) as f32 * st.glow.unwrap_or(1.0) as f32;
                            let c = premul(col(&eval(&pass.color, &gctx)), pa * alpha);
                            inst.color = [c[0] * gain, c[1] * gain, c[2] * gain, c[3] * gain];
                            inst.params[1] = st.blur as f32;
                            glow.push(inst);
                        }
                        _ => {
                            let a = pa * alpha * if compose { wb.alpha } else { 1.0 };
                            inst.color = premul(base_color, a);
                            inst.params[1] = st.blur as f32;
                            if pass.kind == "outline_only" || (compose && wb.outline > 0.0) {
                                inst.params[2] = if compose { wb.outline } else { num(&eval(&pass.outline, &gctx)) as f32 * em };
                            }
                            if let Some(wp) = st.wipe {
                                let c2 = st.color2.or_else(|| pass.color2.as_ref().map(|v| col(&eval(v, &gctx)))).unwrap_or(base_color);
                                if wp >= 1.0 {
                                    inst.color = premul(c2, a);
                                } else if wp > 0.0 {
                                    let split_x = wb.x0 + wp as f32 * (wb.x1 - wb.x0);
                                    let left = g.cx - g.sprite.w * 0.5;
                                    inst.extra[0] = (split_x - left) / g.sprite.w;
                                    inst.color2 = premul(c2, a);
                                }
                            }
                            main.push(inst);
                        }
                    }
                }
            }
        }
        self.tracks_us.push(started.elapsed().as_secs_f64() * 1e6);
        (glow, 1.0, main, changed)
    }
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
    }
}
