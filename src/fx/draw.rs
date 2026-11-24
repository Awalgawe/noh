//! The draws a scene script hands over, as host data (never values of the
//! script engine, so that a frame can leave the thread that evaluated it), and
//! the checks the painter would make, done without drawing: the same answer on
//! every machine. Where text falls comes from the layout alone (no glyph image).
use super::shaders::Inst;
use super::text::{Face, Fonts, PGlyph, layout};

/// One frame as host data: what to draw at an instant, whatever described it.
#[derive(Clone, Debug)]
pub struct Frame {
    pub draws: Vec<Draw>,
    /// The runs of `draws` that are layers, in order.
    pub layers: Vec<Span>,
    /// The sound at this instant, for layer shaders: loudness (0 to 1) and
    /// seconds since the last sudden rise.
    pub energy: f32,
    pub onset_age: f32,
    /// How the frame frames the picture (`None`: the picture as it is).
    pub video: Option<super::reframe::VideoDraw>,
}

/// Format caps: a scene asking for more is refused, not slowed down.
pub const MAX_DRAWS: usize = 256;
pub const MAX_PARTICLES_PER_EMITTER: usize = 2000;
pub const MAX_PARTICLES: usize = 4000;
pub const MAX_TEXT_CHARS: usize = 256;
/// Letters one frame may draw, every pass counted (a shadow, an outline and a
/// glowing edge each draw the letter once more).
pub const MAX_GLYPHS: usize = 6_000;
/// Distinct text layouts a scene may use: each is shaped and rasterised before
/// the first frame.
pub const MAX_BLOCKS: usize = 512;
/// Surface one frame may cover, in frames: every quad drawn, every pass counted,
/// what falls outside the frame left out where its place is known. The GPU pays
/// per covered pixel, not per letter.
pub const MAX_AREA: f32 = 16.0;
/// Different shadow blurs in one frame: each is a blurred layer of its own.
pub const MAX_BLURS: usize = 8;
/// Passes of one frame: each layer, and each run of loose draws between two
/// layers (a run of shapes is a pass of its own).
pub const MAX_PASSES: usize = 8;
/// The side of the quad a letter is drawn in, in em: the cell of the tallest and
/// widest letters with the atlas margin. An upper estimate, the same everywhere.
pub const LETTER_QUAD_EM: f32 = 1.25;
/// Largest scale a text may reach (its glyphs are rasterised once, at that size).
pub const MAX_TEXT_SCALE: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    Text(TextDraw),
    Rect(RectDraw),
    Emitter(EmitterDraw),
    Path(super::shapes::PathDraw),
    /// Host-only: no script function makes one.
    Prepared(Prepared),
}

/// Glyph instances a host front end prepared itself (a caption style lays its
/// lines out and animates them): the glow pass with its gain and the main pass,
/// exactly what the renderer draws. The glyph shader's hooks are the caller's
/// to set before the frame is drawn.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub glow: Vec<Inst>,
    pub gain: f32,
    pub main: Vec<Inst>,
}

/// Equal when the renderer would be handed the same bytes.
impl PartialEq for Prepared {
    fn eq(&self, o: &Self) -> bool {
        self.gain.to_bits() == o.gain.to_bits()
            && bytemuck::cast_slice::<Inst, u8>(&self.glow)
                == bytemuck::cast_slice::<Inst, u8>(&o.glow)
            && bytemuck::cast_slice::<Inst, u8>(&self.main)
                == bytemuck::cast_slice::<Inst, u8>(&o.main)
    }
}

impl Prepared {
    /// Instances drawn, both passes counted.
    pub fn glyph_instances(&self) -> usize {
        self.glow.len() + self.main.len()
    }

    /// Surface the instances cover, in frames of `w` x `h` pixels, both passes
    /// counted: each quad at its scale, the share of an unrotated quad outside
    /// the frame left out.
    pub fn area(&self, w: f32, h: f32) -> f32 {
        let frame = w * h;
        if frame <= 0.0 {
            return 0.0;
        }
        self.glow
            .iter()
            .chain(&self.main)
            .map(|i| {
                let qw = (i.size[0] * i.xform[1]).abs();
                let qh = (i.size[1] * i.xform[2]).abs();
                if qw <= 0.0 || qh <= 0.0 {
                    return 0.0;
                }
                let seen = if i.xform[0] == 0.0 {
                    let iw = ((i.center[0] + qw / 2.0).min(w) - (i.center[0] - qw / 2.0).max(0.0))
                        .max(0.0);
                    let ih = ((i.center[1] + qh / 2.0).min(h) - (i.center[1] - qh / 2.0).max(0.0))
                        .max(0.0);
                    iw * ih
                } else {
                    qw * qh
                };
                seen / frame
            })
            .sum()
    }

    /// How many different blurs the main pass asks for (each is a blurred
    /// layer of the renderer): coverage instances with a blur, by their sigma.
    pub fn blurs(&self) -> usize {
        let mut seen: Vec<u32> = Vec::new();
        for i in &self.main {
            if i.params[0] < 0.5 && i.params[1] > 0.0 {
                let key = i.params[1].to_bits();
                if !seen.contains(&key) {
                    seen.push(key);
                }
            }
        }
        seen.len()
    }
}

/// What draws a run of a group's draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run {
    /// The engine's glyph pass, painted from text, rectangles and particles.
    Engine,
    /// The shapes' pass.
    Shapes,
    /// The engine's glyph pass, from instances already prepared.
    Prepared,
}

/// A group's draws cut into runs the same painter draws: the engine (text,
/// rectangles, particles), the shapes' pass, or prepared instances (each such
/// draw is a run of its own: it has its own gain). Each run is one pass.
pub fn runs(draws: &[Draw]) -> Vec<(std::ops::Range<usize>, Run)> {
    let mut out: Vec<(std::ops::Range<usize>, Run)> = Vec::new();
    for (i, d) in draws.iter().enumerate() {
        let kind = match d {
            Draw::Path(_) => Run::Shapes,
            Draw::Prepared(_) => Run::Prepared,
            Draw::Text(_) | Draw::Rect(_) | Draw::Emitter(_) => Run::Engine,
        };
        match out.last_mut() {
            Some((r, k)) if *k == kind && kind != Run::Prepared => r.end = i + 1,
            _ => out.push((i..i + 1, kind)),
        }
    }
    out
}

/// A run of the frame's draws that the script grouped as one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub first: usize,
    pub len: usize,
    pub opacity: f32,
    /// The author's shader run over the layer's content, if any.
    pub shader: Option<ShaderSpec>,
}

/// A layer's shader: a file beside the scene, its resolution, its eight numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderSpec {
    pub name: String,
    /// Full resolution, or half (a quarter of the pixels, read back smoothly).
    pub full: bool,
    pub k: [f32; 8],
}

/// The frame's draws cut into the passes that draw them, in order: each layer
/// is a pass, and so is each run of draws between two layers.
pub fn passes(n: usize, layers: &[Span]) -> Vec<(std::ops::Range<usize>, f32)> {
    groups(n, layers)
        .into_iter()
        .map(|(r, i)| (r, i.map_or(1.0, |i| layers[i].opacity)))
        .collect()
}

/// The same groups, each with the index of its layer (none for a run of loose
/// draws). An empty layer draws nothing and is no pass.
pub fn groups(n: usize, layers: &[Span]) -> Vec<(std::ops::Range<usize>, Option<usize>)> {
    let mut out = Vec::new();
    let mut at = 0;
    for (i, s) in layers.iter().enumerate() {
        if s.first > at {
            out.push((at..s.first, None));
        }
        // An empty layer draws nothing, unless its shader draws it all.
        if s.len > 0 || s.shader.is_some() {
            out.push((s.first..s.first + s.len, Some(i)));
        }
        at = s.first + s.len;
    }
    if at < n {
        out.push((at..n, None));
    }
    out
}

/// A block of text as it is laid out and placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub text: String,
    /// Where the block's anchor point sits, fractions of the canvas.
    pub x: f32,
    pub y: f32,
    /// Font size, fraction of the canvas' short side.
    pub size: f32,
    /// Wrap width, fraction of the canvas width.
    pub width: f32,
    pub face: String,
    pub line_height: f32,
    /// "center", "left" or "right": how the lines sit in the block.
    pub align: String,
    /// Which point of the block's box is at (x, y): "center", "left", "right",
    /// "top", "bottom", "top_left", "top_right", "bottom_left", "bottom_right",
    /// or on the first line's baseline: "baseline", "baseline_left",
    /// "baseline_right".
    pub anchor: String,
    /// Rotation about the anchor point, radians, clockwise.
    pub rot: f32,
    /// Extra space between letters, in em.
    pub tracking: f32,
}

impl Block {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            x: 0.5,
            y: 0.5,
            size: 0.08,
            width: 0.88,
            face: "jost_extrabold".into(),
            line_height: 1.15,
            align: "center".into(),
            anchor: "center".into(),
            rot: 0.0,
            tracking: 0.0,
        }
    }

    /// The anchor in the block's box, in canvas pixels from its top-left corner.
    pub(crate) fn anchor_px(&self, width: f32, height: f32, baseline: f32) -> (f32, f32) {
        let fx = match self.anchor.as_str() {
            "baseline" => 0.5,
            "baseline_left" => 0.0,
            "baseline_right" => 1.0,
            _ => {
                let (fx, fy) = self.anchor_point().unwrap_or((0.5, 0.5));
                return (fx * width, fy * height);
            }
        };
        (fx * width, baseline)
    }

    /// The anchor as fractions of the block's box (the baseline anchors aside).
    fn anchor_point(&self) -> Option<(f32, f32)> {
        Some(match self.anchor.as_str() {
            "baseline" | "baseline_left" | "baseline_right" => (0.0, 0.0),
            "center" => (0.5, 0.5),
            "left" => (0.0, 0.5),
            "right" => (1.0, 0.5),
            "top" => (0.5, 0.0),
            "bottom" => (0.5, 1.0),
            "top_left" => (0.0, 0.0),
            "top_right" => (1.0, 0.0),
            "bottom_left" => (0.0, 1.0),
            "bottom_right" => (1.0, 1.0),
            _ => return None,
        })
    }

    pub fn check(&self) -> Result<(), String> {
        if self.text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("a text of more than {MAX_TEXT_CHARS} characters"));
        }
        if Face::named(&self.face).is_none() {
            return Err(format!("face {:?} unknown", self.face));
        }
        if !matches!(self.align.as_str(), "center" | "left" | "right") {
            return Err(format!(
                "align {:?} unknown (center, left, right)",
                self.align
            ));
        }
        if self.anchor_point().is_none() {
            return Err(format!("anchor {:?} unknown", self.anchor));
        }
        finite(
            "text",
            &[
                self.x,
                self.y,
                self.size,
                self.width,
                self.line_height,
                self.rot,
                self.tracking,
            ],
        )?;
        if !(0.005..=1.0).contains(&self.size) || !(0.02..=4.0).contains(&self.width) {
            return Err("text size or width out of range".into());
        }
        if !(-0.2..=2.0).contains(&self.tracking) {
            return Err("tracking out of range (-0.2 to 2 em)".into());
        }
        if !(0.5..=4.0).contains(&self.line_height) {
            return Err("line height out of range (0.5 to 4)".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub color: String,
    pub opacity: f32,
    /// Gaussian sigma and vertical offset, in em.
    pub blur: f32,
    pub dy: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub color: String,
    pub opacity: f32,
    /// Width in em.
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextDraw {
    pub block: Block,
    pub color: String,
    pub opacity: f32,
    /// 0 = whole, 1 = gone (noise threshold with a lit edge).
    pub dissolve: f32,
    pub edge: String,
    /// Gain of the lit edge in the glow layer.
    pub glow: f32,
    /// About the anchor point.
    pub scale: f32,
    pub shadow: Option<Shadow>,
    pub outline: Option<Outline>,
    /// One entry per word (missing entries are neutral).
    pub words: Vec<WordMod>,
}

impl TextDraw {
    pub fn new(text: &str) -> Self {
        Self {
            block: Block::new(text),
            color: "#FFFFFF".into(),
            opacity: 1.0,
            dissolve: 0.0,
            edge: "#FFB060".into(),
            glow: 0.0,
            scale: 1.0,
            shadow: None,
            outline: None,
            words: Vec::new(),
        }
    }

    /// How many letter instances it draws: its letters, once per pass.
    pub fn glyph_instances(&self) -> usize {
        if self.opacity <= 0.0 {
            return 0;
        }
        let letters = self
            .block
            .text
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let passes = 1
            + usize::from(self.shadow.is_some())
            + usize::from(self.outline.is_some())
            + usize::from(self.glow > 0.0);
        letters * passes
    }

    /// The largest scale any of its glyphs is drawn at.
    pub fn peak_scale(&self) -> f32 {
        let w = self.words.iter().map(|m| m.scale).fold(1.0f32, f32::max);
        self.scale * w
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WordMod {
    pub opacity: f32,
    /// Vertical shift in em (positive = down).
    pub dy: f32,
    pub scale: f32,
    pub dissolve: Option<f32>,
    /// Parsed when set, so that a text value stays cheap to copy.
    pub color: Option<[u8; 3]>,
}

impl WordMod {
    pub fn neutral() -> Self {
        Self {
            opacity: 1.0,
            dy: 0.0,
            scale: 1.0,
            dissolve: None,
            color: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RectDraw {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub color: String,
    pub opacity: f32,
    pub glow: f32,
}

impl RectDraw {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            x: 0.5,
            y: 0.5,
            w,
            h,
            rot: 0.0,
            color: "#FFFFFF".into(),
            opacity: 1.0,
            glow: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The ink of a text block (its letters' boxes).
    Text(Block),
    /// x0, y0, x1, y1 in fractions of the canvas.
    Rect { rect: [f32; 4] },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmitterDraw {
    pub source: Source,
    /// Births happen in [start, end), `rate` per second; each lives `life` seconds.
    pub start: f64,
    pub end: f64,
    pub rate: f32,
    pub life: f32,
    /// Every particle is gone by this time, whatever its birth (None: no such rule).
    pub gone_by: Option<f64>,
    /// "dust" or "petal".
    pub shape: String,
    /// Fraction of the short side.
    pub size: f32,
    pub colors: Vec<String>,
    /// Drift per second, fractions of the short side (x right, y down).
    pub wind: [f32; 2],
    /// Upward speed, fraction of the short side per second.
    pub rise: f32,
    /// Wobble amplitude reached at the end of life, fraction of the short side.
    pub swirl: f32,
    /// Radians per second.
    pub spin: f32,
    pub opacity: f32,
    pub glow: f32,
    pub seed: u32,
}

impl EmitterDraw {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            start: 0.0,
            end: 0.0,
            rate: 20.0,
            life: 4.0,
            gone_by: None,
            shape: "dust".into(),
            size: 0.012,
            colors: Vec::new(),
            wind: [0.0, 0.0],
            rise: 0.0,
            swirl: 0.0,
            spin: 0.0,
            opacity: 1.0,
            glow: 0.0,
            seed: 0,
        }
    }

    /// Indices of the particles that may be alive at `t` (first..=last), if any.
    pub(crate) fn births(&self, t: f64) -> Option<(u32, u32)> {
        let (rate, life) = (self.rate.max(0.0) as f64, self.life.max(0.01) as f64);
        if rate <= 0.0 || t < self.start || t > self.end + life {
            return None;
        }
        if self.gone_by.is_some_and(|g| t >= g) {
            return None;
        }
        let last_birth = t.min(self.end - 1e-9);
        if last_birth < self.start {
            return None;
        }
        let first = ((t - life - self.start) * rate).ceil().max(0.0) as u32;
        let last = ((last_birth - self.start) * rate).floor() as u32;
        (first <= last).then_some((first, last))
    }

    /// An upper bound of the particles drawn at `t`, without drawing them.
    pub fn alive_bound(&self, t: f64) -> usize {
        self.births(t).map_or(0, |(a, b)| (b - a + 1) as usize)
    }
}

pub fn hex(s: &str) -> Result<[u8; 3], String> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6 && h.is_ascii());
    let h = h.ok_or_else(|| format!("colour {s:?} is not #RRGGBB"))?;
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("colour {s:?}"));
    Ok([p(0)?, p(2)?, p(4)?])
}

fn finite(name: &str, values: &[f32]) -> Result<(), String> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(format!("{name}: a number is not finite"))
    }
}

impl Draw {
    /// What the painter would refuse, checked without drawing: the same answer
    /// on every machine.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Draw::Rect(r) => {
                hex(&r.color)?;
                finite("rect", &[r.x, r.y, r.w, r.h, r.rot, r.opacity, r.glow])
            }
            Draw::Text(t) => {
                t.block.check()?;
                hex(&t.color)?;
                hex(&t.edge)?;
                finite("text", &[t.opacity, t.dissolve, t.glow, t.scale])?;
                if let Some(s) = &t.shadow {
                    hex(&s.color)?;
                    finite("shadow", &[s.opacity, s.blur, s.dy])?;
                    if !(0.0..=1.0).contains(&s.blur) || !(-2.0..=2.0).contains(&s.dy) {
                        return Err(
                            "shadow blur (0 to 1 em) or offset (-2 to 2 em) out of range".into(),
                        );
                    }
                }
                if let Some(o) = &t.outline {
                    hex(&o.color)?;
                    finite("outline", &[o.opacity, o.width])?;
                    if !(0.0..=0.25).contains(&o.width) {
                        return Err("outline width out of range (0 to 0.25 em)".into());
                    }
                }
                for m in &t.words {
                    finite(
                        "word",
                        &[m.opacity, m.dy, m.scale, m.dissolve.unwrap_or(0.0)],
                    )?;
                }
                let peak = t.peak_scale();
                if !(0.0..=MAX_TEXT_SCALE).contains(&peak) {
                    return Err(format!("a text scale of {peak} exceeds {MAX_TEXT_SCALE}"));
                }
                Ok(())
            }
            Draw::Path(p) => p.check(),
            // Made by the host, drawn as it is.
            Draw::Prepared(_) => Ok(()),
            Draw::Emitter(e) => {
                if let Source::Text(b) = &e.source {
                    b.check()?;
                }
                for c in &e.colors {
                    hex(c)?;
                }
                if !matches!(e.shape.as_str(), "dust" | "petal") {
                    return Err(format!("emitter shape {:?} unknown (dust, petal)", e.shape));
                }
                finite(
                    "emitter",
                    &[
                        e.rate, e.life, e.size, e.wind[0], e.wind[1], e.rise, e.swirl, e.spin,
                        e.opacity, e.glow,
                    ],
                )?;
                if !e.start.is_finite() || !e.end.is_finite() {
                    return Err("emitter: a time is not finite".into());
                }
                if !(0.0..=0.5).contains(&e.size) {
                    return Err("particle size out of range (0 to 0.5)".into());
                }
                let n = e.rate.max(0.0) as f64 * e.life.max(0.01) as f64;
                if n > MAX_PARTICLES_PER_EMITTER as f64 {
                    return Err(format!(
                        "an emitter of {} particles (rate x life) exceeds {MAX_PARTICLES_PER_EMITTER}",
                        n as usize
                    ));
                }
                Ok(())
            }
        }
    }
}

/// A layout's identity: what changes the shaping or the place of the letters
/// inside the block (not where the block sits).
pub fn layout_key(b: &Block, w: f32, h: f32) -> String {
    let px = (b.size * w.min(h) * 4.0).round() / 4.0;
    format!(
        "{}|{}|{px}|{}|{}|{}|{}",
        b.face,
        b.text,
        (b.width * w).round(),
        b.line_height,
        b.align,
        (b.tracking * 1000.0).round()
    )
}

/// A block shaped and arranged, before any glyph image: pen positions in the
/// block's own box (origin at its top-left corner), in canvas pixels.
pub struct Arranged {
    pub glyphs: Vec<(PGlyph, f32)>,
    /// The block's box: the widest line by the lines' height.
    pub width: f32,
    pub height: f32,
    /// x0, y0, x1, y1 of each word in the box.
    pub words: Vec<[f32; 4]>,
    /// The first line's baseline, from the top of the box.
    pub baseline: f32,
    pub too_wide: bool,
}

/// Shape, wrap, align and space the letters of a block. No glyph image is made.
pub fn arrange(fonts: &mut Fonts, b: &Block, w: f32, h: f32) -> Arranged {
    let px = ((b.size * w.min(h) * 4.0).round() / 4.0).max(1.0);
    let max_w = (b.width * w).round();
    let face = Face::named(&b.face).unwrap_or(Face::Jost);
    let words: Vec<String> = b.text.split_whitespace().map(str::to_string).collect();
    let laid = layout(
        fonts,
        &words,
        " ",
        &[],
        face,
        px,
        Some(max_w),
        b.line_height,
    );
    // Letters of each line before a glyph (for the tracking), and the lines' widths.
    let track = b.tracking * px;
    let n_lines = laid.lines.len().max(1);
    let mut count = vec![0usize; n_lines];
    let mut order = Vec::with_capacity(laid.glyphs.len());
    for g in &laid.glyphs {
        let l = g.line.min(n_lines - 1);
        order.push(count[l]);
        count[l] += 1;
    }
    let line_w: Vec<f32> = (0..n_lines)
        .map(|l| {
            let base = laid.lines.get(l).map_or(0.0, |x| x.1);
            base + track * count[l].saturating_sub(1) as f32
        })
        .collect();
    let width = line_w.iter().copied().fold(0.0f32, f32::max);
    let line_px = px * b.line_height;
    let mut out = Vec::with_capacity(laid.glyphs.len());
    let mut boxes = vec![[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; words.len()];
    for (g, k) in laid.glyphs.iter().zip(order) {
        let l = g.line.min(n_lines - 1);
        let base_w = laid.lines.get(l).map_or(0.0, |x| x.1);
        // The engine centres each line in the wrap width: back to the line's own
        // start, then to its place in the block.
        let start = (max_w - base_w) * 0.5;
        let shift = match b.align.as_str() {
            "left" => 0.0,
            "right" => width - line_w[l],
            _ => (width - line_w[l]) * 0.5,
        };
        let x = g.x - start + shift + track * k as f32;
        if let Some(bx) = boxes.get_mut(g.word) {
            let top = l as f32 * line_px;
            bx[0] = bx[0].min(x);
            bx[1] = bx[1].min(top);
            bx[2] = bx[2].max(x + g.advance);
            bx[3] = bx[3].max(top + line_px);
        }
        out.push((g.clone(), x));
    }
    for bx in &mut boxes {
        if bx[0] > bx[2] {
            *bx = [0.0; 4];
        }
    }
    Arranged {
        glyphs: out,
        width,
        height: laid.height.max(line_px * n_lines as f32),
        words: boxes,
        baseline: laid.lines.first().map_or(px, |l| l.0),
        too_wide: laid.too_wide,
    }
}

/// A rectangle on the canvas, fractions of it.
#[derive(Clone, Copy, Debug)]
pub struct BoxF {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Where a text block and its words fall, from the layout alone (no glyph image,
/// no atlas): the block's box and each word's box, before any rotation, at
/// `scale` about the anchor.
pub fn measure(fonts: &mut Fonts, b: &Block, scale: f32, w: f32, h: f32) -> (BoxF, Vec<BoxF>) {
    let a = arrange(fonts, b, w, h);
    let (ax, ay) = b.anchor_px(a.width, a.height, a.baseline);
    let (px, py) = (b.x * w, b.y * h);
    let (ox, oy) = (px - ax * scale, py - ay * scale);
    let to = |r: [f32; 4]| BoxF {
        x0: (ox + r[0] * scale) / w,
        y0: (oy + r[1] * scale) / h,
        x1: (ox + r[2] * scale) / w,
        y1: (oy + r[3] * scale) / h,
    };
    (
        to([0.0, 0.0, a.width, a.height]),
        a.words.iter().map(|r| to(*r)).collect(),
    )
}
