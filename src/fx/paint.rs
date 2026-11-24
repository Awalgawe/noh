//! A frame's draws painted by the engine of `crate::fx`: text through the
//! layout and the coverage glyph atlas (the path the shipped styles use),
//! rectangles, and stateless particles (a particle's place is a function of its
//! age, never integrated). Each pass of the frame (a layer, or a run of loose
//! draws) is one frame of the engine, stacked by `super::compose`; a run of
//! vector shapes is a pass of the compositor's own, and a layer holding several
//! runs is stacked as one group so that it still fades as one picture.
//!
//! The engine's glyph shader runs `MOTION_HOOKS`, written by NOH (never by an
//! author). The hook tells elements apart by the « progress » channel, a
//! contract between this painter and that hook only: 0..1 is a text or a
//! rectangle and its dissolve, 2 a dust particle, 3 a petal.
use super::compose::{Compositor, Fill, Shape};
use super::draw::{
    Block, Draw, EmitterDraw, Frame, MAX_DRAWS, MAX_PARTICLES, MAX_TEXT_SCALE, Run, Source, Span,
    arrange, groups, hex, layout_key, runs,
};
use super::gl::GlRenderer;
use super::shaders::{COV_ATLAS, Inst, Upload};
use super::shapes::{Paint, PathDraw, ShapeCache, Tess};
use super::text::{Atlas, Fonts};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Share of the coverage atlas a scene's letters may fill: the shelf packer
/// wastes room, and a full atlas would drop letters.
pub const ATLAS_BUDGET: f64 = 0.6;

/// The glyph shader's hooks for motion scenes, inside the plugin shader profile
/// (no loop, no helper call, no texture of its own; validated as plugins are).
/// The dissolve's grain lives on the letter: its noise is computed from where
/// the pixel is in the glyph, from its centre, in canvas pixels of the text at scale 1 (`time`
/// carries the glyph's size and its place in the line, `grain_key`). So the
/// grain is 7 pixels on a text at scale 1 and grows with the text's scale, as
/// in the scenes made with the prototype, and it is the same at every pixel
/// scale (the canvas is the export's size in the preview too). The packed
/// `time` also reaches `displace`, which these hooks leave as the default (no
/// movement): a motion `displace` reading it would need another channel.
pub const MOTION_HOOKS: &str = r#"
fn shade(i: GlyphIn) -> vec4<f32> {
  let q = i.local * 2.0 - vec2<f32>(1.0);
  if (i.progress >= 2.5) {
    let w = 0.6;
    let r = (1.0 + w * w) / (2.0 * w);
    let d = max(length(q - vec2<f32>(r - w, 0.0)), length(q + vec2<f32>(r - w, 0.0)));
    let a = clamp((r - d) * 9.0, 0.0, 1.0);
    let vein = 0.82 + 0.18 * smoothstep(0.0, 0.25, abs(q.x));
    let light = 0.7 + 0.3 * (0.5 - 0.5 * q.y);
    return vec4<f32>(i.color.rgb * vein * light, i.color.a) * a;
  }
  if (i.progress >= 1.5) {
    let d = clamp(1.0 - dot(q, q), 0.0, 1.0);
    return i.color * d * d;
  }
  if (i.progress <= 0.0) {
    return i.color * i.coverage;
  }
  let m = floor(i.time / 16.0);
  let k = i.time - 16.0 * m;
  let gw = floor(m / 1024.0);
  let gh = m - 1024.0 * gw;
  // From the glyph's centre: its sprite's margin depends on the raster, evenly
  // on both sides.
  let glyph = (i.local - vec2<f32>(0.5)) * vec2<f32>(gw, gh) * 4.0 + vec2<f32>(k * 97.0, k * 41.0);
  let px = select(i.frame * g.canvas, glyph, i.time > 0.5);
  let c1 = px / 7.0;
  let b1 = floor(c1);
  let f1 = fract(c1);
  let u1 = f1 * f1 * (vec2<f32>(3.0) - 2.0 * f1);
  let n1 = mix(
    mix(fract(sin(dot(b1, vec2<f32>(127.1, 311.7))) * 43758.5453),
        fract(sin(dot(b1 + vec2<f32>(1.0, 0.0), vec2<f32>(127.1, 311.7))) * 43758.5453), u1.x),
    mix(fract(sin(dot(b1 + vec2<f32>(0.0, 1.0), vec2<f32>(127.1, 311.7))) * 43758.5453),
        fract(sin(dot(b1 + vec2<f32>(1.0, 1.0), vec2<f32>(127.1, 311.7))) * 43758.5453), u1.x),
    u1.y);
  let c2 = px / 2.7 + vec2<f32>(17.0, 5.0);
  let b2 = floor(c2);
  let f2 = fract(c2);
  let u2 = f2 * f2 * (vec2<f32>(3.0) - 2.0 * f2);
  let n2 = mix(
    mix(fract(sin(dot(b2, vec2<f32>(269.5, 183.3))) * 43758.5453),
        fract(sin(dot(b2 + vec2<f32>(1.0, 0.0), vec2<f32>(269.5, 183.3))) * 43758.5453), u2.x),
    mix(fract(sin(dot(b2 + vec2<f32>(0.0, 1.0), vec2<f32>(269.5, 183.3))) * 43758.5453),
        fract(sin(dot(b2 + vec2<f32>(1.0, 1.0), vec2<f32>(269.5, 183.3))) * 43758.5453), u2.x),
    u2.y);
  let n = n1 * 0.65 + n2 * 0.35;
  let th = i.progress * 1.04 - 0.04;
  let alive = smoothstep(th, th + 0.03, n);
  let edge = alive * (1.0 - smoothstep(th + 0.03, th + 0.2, n));
  return (i.color * (1.0 - edge) * alive + i.color2 * edge) * i.coverage;
}
"#;

/// What a glyph's dissolve grain is laid by, for the hooks' `time`: the glyph's
/// size in canvas pixels at scale 1, in 4-pixel steps (up to 4092), and its
/// index in the line modulo 16 (the next glyphs' grains are offset), packed in
/// a whole number under 2^24 so that a 32-bit float holds it exactly.
pub(crate) fn grain_key(w: f32, h: f32, index: usize) -> f32 {
    let q = |v: f32| (v / 4.0).round().clamp(0.0, 1023.0) as u32;
    ((q(w) * 1024 + q(h)) * 16 + (index % 16) as u32) as f32
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

fn scaled(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A number in 0..1 from three integers (stateless randomness).
fn rnd(seed: u32, i: u32, k: u32) -> f32 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(i.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(k.wrapping_mul(0xC2B2_AE35));
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / 16_777_216.0
}

/// The turn the particles' random phases are scaled by: the prototype's value,
/// a little short of 2 pi, kept so that a scene's scatter stays where it was
/// drawn (the full constant moves every particle a little).
#[allow(clippy::approx_constant)]
const TURN: f32 = 6.283;

/// Rasters are made at quarter steps of scale, never below 1.
fn raster_step(scale: f32) -> f32 {
    ((scale.max(1.0) * 4.0).ceil() / 4.0).min(MAX_TEXT_SCALE)
}

struct LaidGlyph {
    /// Centre relative to the block's top-left corner, at scale 1.
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    uv: [f32; 4],
    ppt: f32,
    word: usize,
    index: usize,
}

struct LaidBlock {
    glyphs: Vec<LaidGlyph>,
    /// Centre of each word (same origin).
    words: Vec<[f32; 2]>,
    width: f32,
    height: f32,
    baseline: f32,
    /// Running sum of the glyph areas, to pick a birth place weighted by ink.
    areas: Vec<f32>,
    /// Letters the atlas could not hold or the font does not draw.
    missing: usize,
}

/// A point of a block's box (canvas px from its top-left corner) on the canvas:
/// scaled and turned about the anchor point.
struct Placement {
    px: f32,
    py: f32,
    ax: f32,
    ay: f32,
    scale: f32,
    cos: f32,
    sin: f32,
}

impl Placement {
    fn new(b: &Block, l: &LaidBlock, scale: f32, w: f32, h: f32) -> Self {
        let (ax, ay) = b.anchor_px(l.width, l.height, l.baseline);
        Self {
            px: b.x * w,
            py: b.y * h,
            ax,
            ay,
            scale,
            cos: b.rot.cos(),
            sin: b.rot.sin(),
        }
    }

    fn at(&self, x: f32, y: f32) -> [f32; 2] {
        let (dx, dy) = ((x - self.ax) * self.scale, (y - self.ay) * self.scale);
        [
            self.px + dx * self.cos - dy * self.sin,
            self.py + dx * self.sin + dy * self.cos,
        ]
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct PaintStats {
    pub glyphs: usize,
    pub rects: usize,
    pub particles: usize,
    /// Layouts made during this call (zero once the scene is prepared).
    pub layouts: usize,
    /// Vector shapes drawn.
    pub paths: usize,
    /// Time spent cutting shapes into triangles (or finding them cut), in ms.
    pub tess_ms: f64,
    /// Layer shaders run, and shader layers left out (`Shaders::Hide`).
    pub shader_layers: usize,
    pub hidden_shader_layers: usize,
}

/// What becomes of the frame's shader layers.
pub enum Shaders<'a> {
    /// Left out whole, and counted (the preview's default: a driver reset in
    /// the window cannot be seen there; stills and the export draw them).
    Hide,
    /// Drawn with these programs and the sound at that instant.
    Draw {
        set: &'a super::layer_shader::ShaderSet,
        energy: f32,
        onset_age: f32,
    },
}

/// What preparing a scene made: its layouts, the letters rasterised, and the
/// share of the glyph atlas they use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prepared {
    pub layouts: usize,
    pub glyphs: usize,
    pub atlas_share: f64,
}

pub struct Painter {
    fonts: Rc<RefCell<Fonts>>,
    atlas: Atlas,
    w: f32,
    h: f32,
    cache: HashMap<String, Rc<LaidBlock>>,
    /// Layout key -> the scale its letters are rasterised for.
    raster: HashMap<String, f32>,
    /// Shapes already cut into triangles, by content.
    shapes: ShapeCache,
}

impl Painter {
    /// A painter for frames of `w` x `h` canvas pixels (the export's canvas, in
    /// the preview too: the engine scales its own pixels).
    pub fn new(w: u32, h: u32, fonts: Rc<RefCell<Fonts>>) -> Self {
        Self {
            fonts,
            atlas: Atlas::new(48.0),
            w: w as f32,
            h: h as f32,
            cache: HashMap::new(),
            raster: HashMap::new(),
            shapes: ShapeCache::default(),
        }
    }

    /// Lay out and rasterise every block a validated scene uses
    /// (`check::Checker::blocks`), each at the largest scale it reaches, so that
    /// no frame pays for a first appearance. Refuses a scene whose letters would
    /// not fit the atlas.
    pub fn prepare(
        &mut self,
        gpu: &dyn Upload,
        needs: &[(Block, f32)],
    ) -> Result<Prepared, String> {
        for (b, peak) in needs {
            let r = raster_step(*peak);
            let e = self
                .raster
                .entry(layout_key(b, self.w, self.h))
                .or_insert(r);
            *e = e.max(r);
        }
        let mut layouts = 0;
        for (b, _) in needs {
            let l = self.block(gpu, b, 1.0, &mut layouts);
            if l.missing > 0 {
                return Err(format!(
                    "{} letter(s) of {:?} could not be drawn (glyph atlas full, or absent from the font)",
                    l.missing, b.text
                ));
            }
        }
        let share = self.atlas_share();
        if share > ATLAS_BUDGET || self.atlas.stats.atlas_resets > 0 {
            return Err(format!(
                "the scene's letters need {:.0} % of the glyph atlas (limit {:.0} %): fewer sizes, smaller text or less scale",
                share * 100.0,
                ATLAS_BUDGET * 100.0
            ));
        }
        Ok(Prepared {
            layouts,
            glyphs: self.atlas.stats.glyphs,
            atlas_share: share,
        })
    }

    /// Share of the atlas filled since it last started over (`texels` counts
    /// from the beginning: only `prepare` reads this, before any frame).
    fn atlas_share(&self) -> f64 {
        self.atlas.stats.texels as f64 / (COV_ATLAS as f64 * COV_ATLAS as f64)
    }

    fn block(
        &mut self,
        gpu: &dyn Upload,
        b: &Block,
        scale: f32,
        layouts: &mut usize,
    ) -> Rc<LaidBlock> {
        let key = layout_key(b, self.w, self.h);
        // The scale prepared for this block, else the scale asked now (a layout
        // first seen after the validation: rasterised here, on its first frame).
        let raster = self
            .raster
            .get(&key)
            .copied()
            .unwrap_or_else(|| raster_step(scale))
            .max(raster_step(scale));
        let cache_key = format!("{key}|{raster}");
        if let Some(l) = self.cache.get(&cache_key) {
            return l.clone();
        }
        *layouts += 1;
        let mut fonts = self.fonts.borrow_mut();
        let arranged = arrange(&mut fonts, b, self.w, self.h);
        let mut glyphs = Vec::new();
        let mut sums = vec![[0.0f32; 3]; arranged.words.len()];
        let mut areas = Vec::new();
        let mut total = 0.0;
        let mut missing = 0;
        for (g, x) in &arranged.glyphs {
            let Some(sp) = self.atlas.cov_sprite(&mut fonts, gpu, g, raster) else {
                missing += 1;
                continue;
            };
            let cx = x + sp.left + sp.w * 0.5;
            // `top` is the sprite top above the baseline (as plugin.rs places glyphs).
            let cy = g.baseline - sp.top + sp.h * 0.5;
            if let Some(s) = sums.get_mut(g.word) {
                s[0] += cx;
                s[1] += cy;
                s[2] += 1.0;
            }
            total += sp.w * sp.h;
            areas.push(total);
            glyphs.push(LaidGlyph {
                cx,
                cy,
                w: sp.w,
                h: sp.h,
                uv: sp.uv,
                ppt: sp.ppt,
                word: g.word,
                index: g.index,
            });
        }
        drop(fonts);
        let l = Rc::new(LaidBlock {
            glyphs,
            words: sums
                .iter()
                .map(|s| [s[0] / s[2].max(1.0), s[1] / s[2].max(1.0)])
                .collect(),
            width: arranged.width,
            height: arranged.height,
            baseline: arranged.baseline,
            areas,
            missing,
        });
        if self.cache.len() > 2048 {
            self.cache.clear();
        }
        self.cache.insert(cache_key, l.clone());
        l
    }

    /// Draws -> glow instances and main instances for one pass of the engine.
    /// A letter the atlas cannot hold is an error: the caller leaves the frame's
    /// overlay out rather than draw it with letters missing.
    pub fn paint(
        &mut self,
        gpu: &dyn Upload,
        draws: &[Draw],
        t: f64,
    ) -> Result<(Vec<Inst>, Vec<Inst>, PaintStats), String> {
        if draws.len() > MAX_DRAWS {
            return Err(format!("{} draws exceed {MAX_DRAWS}", draws.len()));
        }
        let mut stats = PaintStats::default();
        // When the glyph atlas is full, a new letter makes it start over
        // (`Atlas::cov_sprite`): every letter laid before, in this frame or in the
        // cache, then points at texels the new uploads overwrite.
        let resets = self.atlas.stats.atlas_resets;
        let (mut glow, mut main) = (Vec::new(), Vec::new());
        let short = self.w.min(self.h);
        for d in draws {
            match d {
                Draw::Rect(r) => {
                    let c = premul(hex(&r.color)?, r.opacity.clamp(0.0, 1.0));
                    let inst = Inst {
                        center: [r.x * self.w, r.y * self.h],
                        size: [r.w * self.w, r.h * self.h],
                        xform: [r.rot, 1.0, 1.0, 0.0],
                        color: c,
                        params: [2.0, 0.0, 0.0, 1.0],
                        extra: [-1.0, 0.0, 0.0, 0.0],
                        ..Default::default()
                    };
                    if r.glow > 0.0 {
                        glow.push(Inst {
                            color: scaled(c, r.glow),
                            ..inst
                        });
                    }
                    main.push(inst);
                    stats.rects += 1;
                }
                Draw::Text(td) => {
                    let b = &td.block;
                    let laid = self.block(gpu, b, td.peak_scale(), &mut stats.layouts);
                    if laid.missing > 0 {
                        return Err(format!(
                            "at t={t:.3}: {} letter(s) of {:?} could not be drawn (glyph atlas full, or absent from the font)",
                            laid.missing, b.text
                        ));
                    }
                    let place = Placement::new(b, &laid, td.scale, self.w, self.h);
                    let base = hex(&td.color)?;
                    let edge = hex(&td.edge)?;
                    let em = b.size * short;
                    let shadow = match &td.shadow {
                        Some(s) => Some((hex(&s.color)?, s.opacity, s.blur * em, s.dy * em)),
                        None => None,
                    };
                    let outline = match &td.outline {
                        Some(o) => Some((hex(&o.color)?, o.opacity, o.width * em)),
                        None => None,
                    };
                    // Shadows, then outlines, then fills: a shadow never covers a letter.
                    let mut fills = Vec::with_capacity(laid.glyphs.len());
                    let mut outlines = Vec::new();
                    for g in &laid.glyphs {
                        let m = td.words.get(g.word);
                        let (wo, wdy, ws) =
                            m.map_or((1.0, 0.0, 1.0), |m| (m.opacity, m.dy, m.scale));
                        let a = (td.opacity * wo).clamp(0.0, 1.0);
                        if a <= 0.0 {
                            continue;
                        }
                        let dis = m
                            .and_then(|m| m.dissolve)
                            .unwrap_or(td.dissolve)
                            .clamp(0.0, 1.0);
                        if dis >= 1.0 {
                            continue;
                        }
                        let col = m.and_then(|m| m.color).unwrap_or(base);
                        let wc = laid.words.get(g.word).copied().unwrap_or([g.cx, g.cy]);
                        // The word's scale about the word, then the block's own placement.
                        let bx = wc[0] + (g.cx - wc[0]) * ws;
                        let by = wc[1] + (g.cy - wc[1]) * ws + wdy * em;
                        let s = ws * td.scale;
                        let inst = Inst {
                            center: place.at(bx, by),
                            size: [g.w, g.h],
                            xform: [b.rot, s, s, 0.0],
                            uv: g.uv,
                            color: premul(col, a),
                            color2: premul(edge, a),
                            params: [0.0, 0.0, 0.0, g.ppt],
                            extra: [-1.0, grain_key(g.w, g.h, g.index), dis, g.index as f32],
                            aux: [0.0; 4],
                            lane: [0.0; 4],
                        };
                        if let Some((c, o, blur, dy)) = shadow {
                            main.push(Inst {
                                center: place.at(bx, by + dy),
                                color: premul(c, a * o),
                                color2: [0.0; 4],
                                params: [0.0, (blur * s).min(96.0), 0.0, g.ppt],
                                ..inst
                            });
                        }
                        if let Some((c, o, width)) = outline {
                            outlines.push(Inst {
                                color: premul(c, a * o),
                                color2: [0.0; 4],
                                params: [0.0, 0.0, width, g.ppt],
                                ..inst
                            });
                        }
                        // The glow pass draws the lit edge only: no fill colour.
                        if td.glow > 0.0 && dis > 0.0 {
                            glow.push(Inst {
                                color: [0.0; 4],
                                color2: scaled(premul(edge, a), td.glow),
                                ..inst
                            });
                        }
                        fills.push(inst);
                        stats.glyphs += 1;
                    }
                    main.append(&mut outlines);
                    main.append(&mut fills);
                }
                Draw::Emitter(e) => self.emit(gpu, e, t, &mut glow, &mut main, &mut stats)?,
                // Shapes are drawn by their own pass, not by the engine; prepared
                // instances are handed to the engine as they are.
                Draw::Path(_) | Draw::Prepared(_) => {}
            }
        }
        if self.atlas.stats.atlas_resets != resets {
            // Nothing laid before the reset may be drawn: this frame is refused
            // (its overlay left out) and later frames lay their letters again.
            self.cache.clear();
            return Err(format!(
                "at t={t:.3}: the glyph atlas filled up with letters first seen after the scene was prepared; this frame is left out"
            ));
        }
        if stats.particles > MAX_PARTICLES {
            return Err(format!(
                "{} particles exceed {MAX_PARTICLES}",
                stats.particles
            ));
        }
        Ok((glow, main, stats))
    }

    fn emit(
        &mut self,
        gpu: &dyn Upload,
        e: &EmitterDraw,
        t: f64,
        glow: &mut Vec<Inst>,
        main: &mut Vec<Inst>,
        stats: &mut PaintStats,
    ) -> Result<(), String> {
        let Some((first, last)) = e.births(t) else {
            return Ok(());
        };
        let rate = e.rate as f64;
        let petal = e.shape == "petal";
        let mut colors = Vec::new();
        for c in &e.colors {
            colors.push(hex(c)?);
        }
        if colors.is_empty() {
            colors.push([255, 255, 255]);
        }
        let short = self.w.min(self.h);
        let laid = match &e.source {
            Source::Text(b) => {
                let l = self.block(gpu, b, 1.0, &mut stats.layouts);
                let place = Placement::new(b, &l, 1.0, self.w, self.h);
                Some((l, place))
            }
            Source::Rect { .. } => None,
        };
        for i in first..=last {
            let birth = e.start + i as f64 / rate;
            let age = (t - birth) as f32;
            // A particle born late still leaves on time when `gone_by` is set.
            let life = match e.gone_by {
                Some(g) => e.life.min((g - birth) as f32),
                None => e.life,
            };
            if age < 0.0 || life <= 0.0 || age > life {
                continue;
            }
            let r = |k: u32| rnd(e.seed, i, k);
            let (x0, y0) = match (&e.source, &laid) {
                (Source::Rect { rect }, _) => (
                    (rect[0] + (rect[2] - rect[0]) * r(0)) * self.w,
                    (rect[1] + (rect[3] - rect[1]) * r(1)) * self.h,
                ),
                (_, Some((l, place))) => {
                    let total = l.areas.last().copied().unwrap_or(0.0);
                    if total <= 0.0 {
                        continue;
                    }
                    let pick = l.areas.partition_point(|&a| a < r(0) * total);
                    let g = &l.glyphs[pick.min(l.glyphs.len() - 1)];
                    let p = place.at(
                        g.cx + (r(1) - 0.5) * g.w * 0.7,
                        g.cy + (r(2) - 0.5) * g.h * 0.7,
                    );
                    (p[0], p[1])
                }
                _ => continue,
            };
            let lf = age / life;
            let wob = e.swirl * lf * short;
            let x = x0
                + e.wind[0] * (0.6 + 0.8 * r(3)) * age * short
                + wob * (age * 1.7 + r(4) * TURN).sin();
            let y = y0
                + (e.wind[1] - e.rise * (0.5 + r(5))) * age * short
                + wob * 0.6 * (age * 1.3 + r(6) * TURN).cos();
            let grow = smooth(0.0, 0.05, lf);
            let fade = 1.0 - smooth(0.75, 1.0, lf);
            let size = e.size * short * (0.6 + 0.8 * r(7)) * grow;
            let a = (e.opacity * fade).clamp(0.0, 1.0);
            if a <= 0.0 || size <= 0.0 {
                continue;
            }
            let c = premul(colors[i as usize % colors.len()], a);
            let rot = r(8) * TURN + e.spin * age * (0.5 + r(9));
            let inst = if petal {
                // A flat leaf turning about its long axis: its width follows a cosine.
                let turn = (age * e.spin * 1.3 + r(10) * TURN).cos();
                let flip = turn.signum() * turn.abs().max(0.18);
                Inst {
                    center: [x, y],
                    size: [size, size * 1.7],
                    xform: [rot, flip, 1.0, 0.0],
                    color: scaled(c, 0.75 + 0.25 * turn.abs()),
                    params: [2.0, 0.0, 0.0, 1.0],
                    extra: [-1.0, 0.0, 3.0, i as f32],
                    ..Default::default()
                }
            } else {
                Inst {
                    center: [x, y],
                    size: [size, size],
                    xform: [rot, 1.0, 1.0, 0.0],
                    color: c,
                    params: [2.0, 0.0, 0.0, 1.0],
                    extra: [-1.0, 0.0, 2.0, i as f32],
                    ..Default::default()
                }
            };
            if e.glow > 0.0 {
                glow.push(Inst {
                    color: scaled(inst.color, e.glow),
                    ..inst
                });
            }
            main.push(inst);
            stats.particles += 1;
        }
        Ok(())
    }
}

/// What a frame that needs more than one call of the renderer is drawn with.
pub struct Stage<'a> {
    pub painter: &'a mut Painter,
    pub comp: &'a mut Compositor,
    pub shaders: Shaders<'a>,
}

/// Where a drawn frame is.
pub enum Drawn {
    /// In the renderer's own output (`GlRenderer::output_texture`); the rows
    /// read back when asked for, and the renderer's times.
    Direct(Vec<u8>, super::shaders::FrameTimes),
    /// In the stage's compositor (`Compositor::texture`).
    Stacked(PaintStats),
}

/// The frame's only draw, when the frame is one draw of prepared instances and
/// nothing else: what takes the direct path.
pub fn direct(f: &Frame) -> Option<&super::draw::Prepared> {
    match (f.draws.as_slice(), f.layers.is_empty(), &f.video) {
        ([Draw::Prepared(p)], true, None) => Some(p),
        _ => None,
    }
}

/// The one way a frame description is drawn, whatever described it.
///
/// A frame that is one draw of prepared instances (every caption style's
/// frame) takes the direct path: one call of the renderer, no compositor, no
/// pass more than before styles came through here, so the same pixels by
/// construction. `read` asks for its rows back. Any other frame is painted and
/// stacked on the `stage` (the glyph shader's hooks must then be
/// `MOTION_HOOKS` for its text, rectangles and particles); without a stage it
/// is refused.
pub fn draw(
    gpu: &mut GlRenderer,
    f: &Frame,
    t: f64,
    read: bool,
    stage: Option<Stage<'_>>,
) -> Result<Drawn, String> {
    if let Some(p) = direct(f) {
        let (rows, times) = gpu.frame(&p.glow, p.gain, &p.main, read)?;
        return Ok(Drawn::Direct(rows, times));
    }
    let Some(s) = stage else {
        return Err("this frame needs the compositor, and none was given".into());
    };
    draw_frame_with(gpu, s.painter, s.comp, &f.draws, &f.layers, t, s.shaders).map(Drawn::Stacked)
}

/// One frame of a scene drawn into `comp`, its shader layers left out (see
/// `draw_frame_with`).
pub fn draw_frame(
    gpu: &mut GlRenderer,
    painter: &mut Painter,
    comp: &mut Compositor,
    draws: &[Draw],
    layers: &[Span],
    t: f64,
) -> Result<PaintStats, String> {
    draw_frame_with(gpu, painter, comp, draws, layers, t, Shaders::Hide)
}

/// One frame of a scene drawn into `comp`: each pass painted, drawn by the
/// engine (whose hooks must be `MOTION_HOOKS`), and stacked; a shader layer's
/// content is stacked on its own, then its shader runs over it. On an error
/// the compositor's output is not finished: the caller leaves the overlay out.
pub fn draw_frame_with(
    gpu: &mut GlRenderer,
    painter: &mut Painter,
    comp: &mut Compositor,
    draws: &[Draw],
    layers: &[Span],
    t: f64,
    shaders: Shaders<'_>,
) -> Result<PaintStats, String> {
    let mut total = PaintStats::default();
    comp.begin();
    let canvas = (painter.w, painter.h);
    for (range, layer) in groups(draws.len(), layers) {
        let group = &draws[range];
        let runs = runs(group);
        let opacity = layer.map_or(1.0, |i| layers[i].opacity);
        let spec = layer.and_then(|i| layers[i].shader.as_ref());
        let program = match (spec, &shaders) {
            (None, _) => None,
            (Some(_), Shaders::Hide) => {
                total.hidden_shader_layers += 1;
                continue;
            }
            (Some(spec), Shaders::Draw { set, .. }) => Some(
                set.get(&spec.name)
                    .ok_or_else(|| format!("shader {:?} is not compiled", spec.name))?,
            ),
        };
        // A layer made of several passes is stacked on its own first, so that
        // it fades as one picture; a layer with a shader always is (the shader
        // reads it).
        let grouped = layer.is_some() && (runs.len() > 1 || spec.is_some());
        if grouped {
            comp.begin_group()?;
        }
        let o = if grouped { 1.0 } else { opacity };
        for (r, kind) in runs {
            let run = &group[r];
            if kind == Run::Prepared {
                for d in run {
                    let Draw::Prepared(p) = d else { continue };
                    gpu.frame(&p.glow, p.gain, &p.main, false)?;
                    comp.stack(gpu.layer_texture(), o);
                    total.glyphs += p.glyph_instances();
                }
                continue;
            }
            if kind == Run::Shapes {
                let paths: Vec<&PathDraw> = run
                    .iter()
                    .filter_map(|d| match d {
                        Draw::Path(p) => Some(p),
                        _ => None,
                    })
                    .collect();
                let cut = std::time::Instant::now();
                let tess = paths
                    .iter()
                    .map(|p| painter.shapes.get(p, canvas.0, canvas.1))
                    .collect::<Result<Vec<Arc<Tess>>, String>>()?;
                total.tess_ms += cut.elapsed().as_secs_f64() * 1000.0;
                let shapes = paths
                    .iter()
                    .zip(&tess)
                    .map(|(p, t)| shape(p, t, canvas.0, canvas.1))
                    .collect::<Result<Vec<Shape>, String>>()?;
                comp.draw_shapes(&shapes, canvas, o)?;
                total.paths += shapes.len();
                continue;
            }
            let (glow, main, s) = painter.paint(&*gpu, run, t)?;
            gpu.frame(&glow, 1.0, &main, false)?;
            comp.stack(gpu.layer_texture(), o);
            total.glyphs += s.glyphs;
            total.rects += s.rects;
            total.particles += s.particles;
            total.layouts += s.layouts;
        }
        if let (
            Some(spec),
            Some(c),
            Shaders::Draw {
                energy, onset_age, ..
            },
        ) = (spec, program, &shaders)
        {
            let k = spec.k;
            let u = [
                0.0, 0.0, t as f32, *energy, *onset_age, 0.0, 0.0, 0.0, k[0], k[1], k[2], k[3],
                k[4], k[5], k[6], k[7],
            ];
            comp.end_group_shader(c.prog, c.ubo_binding, spec.full, u, opacity)?;
            total.shader_layers += 1;
        } else if grouped {
            comp.end_group(opacity);
        }
    }
    comp.finish();
    Ok(total)
}

/// A path's paint and triangles as the compositor draws them. Colours stay
/// display-encoded (the compositor mixes gradient stops that way); a solid fill
/// and a stroke are opaque, the path's opacity applies to both.
fn shape<'a>(p: &PathDraw, t: &'a Tess, w: f32, h: f32) -> Result<Shape<'a>, String> {
    let rgb = |c: &str| {
        hex(c).map(|c| {
            [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ]
        })
    };
    let paint = match (&p.fill, p.gradient_px(w, h)) {
        (_, Some((mode, a, b, r, stops))) => {
            let stops = stops
                .iter()
                .map(|(o, c, alpha)| rgb(c).map(|c| (*o, [c[0], c[1], c[2], *alpha])))
                .collect::<Result<Vec<_>, String>>()?;
            if mode == 1 {
                Fill::Linear {
                    from: a,
                    to: b,
                    stops,
                }
            } else {
                Fill::Radial {
                    center: a,
                    radius: r,
                    stops,
                }
            }
        }
        (Paint::Solid(c), None) => {
            let c = rgb(c)?;
            Fill::Solid([c[0], c[1], c[2], 1.0])
        }
        // No fill: the path has no fill triangles.
        _ => Fill::Solid([1.0; 4]),
    };
    let stroke_color = match &p.stroke {
        Some((c, _)) => {
            let c = rgb(c)?;
            [c[0], c[1], c[2], 1.0]
        }
        None => [1.0; 4],
    };
    Ok(Shape {
        fill: &t.fill,
        paint,
        stroke: &t.stroke,
        stroke_color,
        opacity: p.opacity,
    })
}
