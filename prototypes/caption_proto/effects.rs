//! The five catalogue tests as declarative per-word / per-letter animation over a
//! laid-out line. Word times are synthetic (even split weighted by letters).
use crate::gpu::{Inst, Upload};
use crate::text::{Atlas, Face, Fonts, PGlyph, Sprite, layout};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
    /// Joiner between words when laying out (" " for Latin, "" for Japanese).
    pub sep: String,
}

/// Even split of the line duration across words, weighted by letter count.
pub fn split_words(start: f64, end: f64, words: &[String], sep: &str) -> Line {
    let weight = |w: &String| w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64;
    let total: f64 = words.iter().map(weight).sum::<f64>().max(1.0);
    let mut t = start;
    let words = words
        .iter()
        .map(|w| {
            let d = (end - start) * weight(w) / total;
            let word = Word { text: w.clone(), start: t, end: t + d };
            t += d;
            word
        })
        .collect();
    Line { start, end, words, sep: sep.into() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    Karaoke = 1,
    PopBlur = 2,
    Neon = 3,
    Wave = 4,
    Compo = 5,
}

impl Test {
    pub fn from_num(n: u32) -> Option<Self> {
        Some(match n {
            1 => Self::Karaoke,
            2 => Self::PopBlur,
            3 => Self::Neon,
            4 => Self::Wave,
            5 => Self::Compo,
            _ => return None,
        })
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Premultiplied linear colour from sRGB hex and alpha.
fn rgba(hex: u32, a: f32) -> [f32; 4] {
    let r = srgb_to_linear(((hex >> 16) & 255) as f32 / 255.0);
    let g = srgb_to_linear(((hex >> 8) & 255) as f32 / 255.0);
    let b = srgb_to_linear((hex & 255) as f32 / 255.0);
    [r * a, g * a, b * a, a]
}

fn scale4(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

fn ease_out_back(x: f64) -> f64 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// A glyph ready to draw: sprite plus its resting quad centre on the canvas.
#[derive(Clone, Debug)]
struct Placed {
    sprite: Sprite,
    cx: f32,
    cy: f32,
    word: usize,
    index: usize,
    font_size: f32,
}

/// Per-word placement for the composition test.
#[derive(Clone, Debug)]
struct CompoWord {
    glyphs: Vec<Placed>,
    pivot: [f32; 2],
    rot: f32,
    color: [f32; 4],
    outline: f32,
}

struct Prepared {
    line: usize,
    glyphs: Vec<Placed>,
    /// Word extents in canvas x (for the karaoke wipe), left..right.
    word_x: Vec<(f32, f32)>,
    compo: Vec<CompoWord>,
    /// Draw order for the composition: carrier, outline word, then the rest.
    order: Vec<usize>,
    shapes: Vec<Inst>,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct LineCost {
    pub line: usize,
    pub layout_ms: f64,
    pub raster_ms: f64,
    pub raster_glyphs: usize,
}

pub struct Scene {
    pub test: Test,
    pub w: f32,
    pub h: f32,
    pub msdf: bool,
    lines: Vec<Line>,
    prepared: Option<Prepared>,
    pub atlas: Atlas,
}

pub const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "at", "for", "from", "is", "are", "was", "be", "i", "you", "he", "she", "it", "we",
    "they", "me", "my", "your", "not", "but", "as", "that", "what", "how", "with", "by", "no", "so", "like", "too", "all", "us", "can", "would",
    "there", "this", "its", "it's", "still", "who", "him", "her", "them",
];

fn word_key(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase()
}

impl Scene {
    pub fn new(test: Test, w: u32, h: u32, msdf: bool, lines: Vec<Line>) -> Self {
        Self { test, w: w as f32, h: h as f32, msdf, lines, prepared: None, atlas: Atlas::new(48.0) }
    }

    /// Same logical canvas, rendered at `phys_h / h` physical pixels per canvas pixel.
    pub fn with_px_scale(mut self, px_scale: f32) -> Self {
        self.atlas.px_scale = px_scale;
        self
    }

    pub fn line_at(&self, t: f64) -> Option<usize> {
        self.lines.iter().position(|l| t >= l.start && t < l.end)
    }

    fn portrait(&self) -> bool {
        self.h > self.w
    }

    fn sprite(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, g: &PGlyph, oversample: f32) -> Option<Sprite> {
        if self.msdf { self.atlas.msdf_sprite(fonts, gpu, g) } else { self.atlas.cov_sprite(fonts, gpu, g, oversample) }
    }

    fn place(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, glyphs: &[PGlyph], ox: f32, oy: f32, oversample: f32) -> Vec<Placed> {
        let mut out = Vec::with_capacity(glyphs.len());
        for g in glyphs {
            if let Some(s) = self.sprite(fonts, gpu, g, oversample) {
                out.push(Placed {
                    sprite: s,
                    cx: ox + g.x + s.left + s.w * 0.5,
                    cy: oy + g.baseline - s.top + s.h * 0.5,
                    word: g.word,
                    index: g.index,
                    font_size: g.size,
                });
            }
        }
        out
    }

    fn prepare(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, li: usize) -> LineCost {
        let started = Instant::now();
        let raster_before = self.atlas.stats;
        let line = self.lines[li].clone();
        let (w, h, portrait) = (self.w, self.h, self.portrait());
        let mut word_x = Vec::new();
        let mut compo = Vec::new();
        let mut shapes = Vec::new();
        let mut order: Vec<usize> = Vec::new();
        let glyphs = match self.test {
            Test::Compo => {
                let keys: Vec<String> = line.words.iter().map(|x| word_key(&x.text)).collect();
                let rank = |i: usize| -> (bool, f64, usize) {
                    (!STOP.contains(&keys[i].as_str()) && !keys[i].is_empty(), line.words[i].end - line.words[i].start, keys[i].len())
                };
                let mut ranked: Vec<usize> = (0..line.words.len()).collect();
                ranked.sort_by(|a, b| rank(*b).partial_cmp(&rank(*a)).unwrap());
                let carrier = ranked[0];
                let outline_word = ranked.get(1).copied().filter(|i| rank(*i).0);
                let mut draw: Vec<usize> = vec![carrier];
                draw.extend(outline_word);
                draw.extend((0..line.words.len()).filter(|i| *i != carrier && Some(*i) != outline_word));
                // Colour blocks first (behind).
                let block = rgba(0xFF4F8B, 0.88);
                let by = if portrait { 0.64 * h } else { 0.70 * h };
                shapes.push(solid(w * 0.5, (by + h) * 0.5, w, h - by, 0.0, block));
                let mut small_slot = 0;
                let slots: &[(f32, f32)] = if portrait {
                    &[(0.08, 0.70), (0.52, 0.76), (0.10, 0.83), (0.48, 0.90), (0.12, 0.96), (0.56, 0.68)]
                } else {
                    &[(0.05, 0.78), (0.40, 0.84), (0.66, 0.78), (0.10, 0.92), (0.48, 0.94), (0.75, 0.90)]
                };
                for (i, word) in line.words.iter().enumerate() {
                    let (face, size, text, pivot, rot, color, outline) = if i == carrier {
                        let size = 0.92 * h / 0.70;
                        let pivot = if portrait { [w * 0.40, h * 0.47] } else { [w * 0.48, h * 0.50] };
                        (Face::Jost, size, word.text.to_uppercase(), pivot, -0.10f32, rgba(0xFFFFFF, 0.93), 0.0)
                    } else if Some(i) == outline_word {
                        let size = if portrait { 0.15 * h } else { 0.22 * h };
                        (Face::SansSemi, size, word.text.clone(), [w * 0.58, h * 0.20], 0.07, rgba(0xFFE14D, 1.0), size * 0.035)
                    } else {
                        let (fx, fy) = slots[small_slot % slots.len()];
                        small_slot += 1;
                        let serif = small_slot % 2 == 1;
                        let size = if serif { 0.075 * h } else { 0.05 * h };
                        let face = if serif { Face::SerifItalic } else { Face::Sans };
                        let color = if serif { rgba(0x111111, 1.0) } else { rgba(0xFFFFFF, 1.0) };
                        (face, size, word.text.clone(), [w * fx, h * fy], if small_slot % 2 == 0 { 0.05 } else { -0.04 }, color, 0.0)
                    };
                    let laid = layout(fonts, &[text], "", face, size, None, 1.0);
                    // Left-align small words at their slot; centre the two big ones.
                    let centred = i == carrier || Some(i) == outline_word;
                    let ox = if centred { pivot[0] - laid.width * 0.5 } else { pivot[0] };
                    let oy = pivot[1] - laid.height * 0.5;
                    let mut gs = laid.glyphs;
                    for g in &mut gs {
                        g.word = i;
                    }
                    let placed = self.place(fonts, gpu, &gs, ox, oy, 1.0);
                    let pivot = if centred { pivot } else { [ox + laid.width * 0.5, pivot[1]] };
                    compo.push(CompoWord { glyphs: placed, pivot, rot, color, outline });
                }
                order = draw;
                Vec::new()
            }
            _ => {
                let (face, size, upper) = match self.test {
                    Test::Karaoke => (Face::SansSemi, if portrait { 76.0 } else { 64.0 }, false),
                    Test::PopBlur => (Face::Jost, if portrait { 104.0 } else { 92.0 }, true),
                    Test::Neon => (Face::SansSemi, if portrait { 92.0 } else { 80.0 }, false),
                    Test::Wave => (Face::Cjk, if portrait { 72.0 } else { 64.0 }, false),
                    Test::Compo => unreachable!(),
                };
                let words: Vec<String> = line.words.iter().map(|x| if upper { x.text.to_uppercase() } else { x.text.clone() }).collect();
                let max_w = w * 0.86;
                let laid = layout(fonts, &words, &line.sep, face, size, Some(max_w), 1.18);
                let ox = (w - max_w) * 0.5;
                let oy = if portrait { h * 0.62 - laid.height * 0.5 } else { h * 0.88 - laid.height };
                let oversample = if self.test == Test::PopBlur { 1.25 } else { 1.0 };
                let placed = self.place(fonts, gpu, &laid.glyphs, ox, oy, oversample);
                word_x = vec![(f32::MAX, f32::MIN); line.words.len()];
                for g in &laid.glyphs {
                    let e = &mut word_x[g.word];
                    e.0 = e.0.min(ox + g.x);
                    e.1 = e.1.max(ox + g.x + g.advance);
                }
                placed
            }
        };
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        let raster_ms = self.atlas.stats.ms - raster_before.ms;
        self.prepared = Some(Prepared { line: li, glyphs, word_x, compo, order, shapes });
        LineCost { line: li, layout_ms: total_ms - raster_ms, raster_ms, raster_glyphs: self.atlas.stats.glyphs - raster_before.glyphs }
    }

    /// Instances for time `t`: (glow, glow_gain, main), plus the line cost when
    /// this frame entered a new line.
    pub fn frame(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, t: f64) -> (Vec<Inst>, f32, Vec<Inst>, Option<LineCost>) {
        let Some(li) = self.line_at(t) else {
            self.prepared = None;
            return (Vec::new(), 1.0, Vec::new(), None);
        };
        let mut cost = None;
        if self.prepared.as_ref().map(|p| p.line) != Some(li) {
            cost = Some(self.prepare(fonts, gpu, li));
        }
        let line = &self.lines[li];
        let p = self.prepared.as_ref().unwrap();
        let fade_in = clamp01((t - line.start) / 0.15) as f32;
        let fade_out = clamp01((line.end - t) / 0.12) as f32;
        // Aligned with archi-greffons-v3 §3.5: opacities multiply across levels.
        let line_a = fade_in * fade_out;
        let mut glow = Vec::new();
        let mut main = Vec::new();
        let shadow_of = |g: &Placed, alpha: f32, dy: f32, xform: [f32; 4], cx: f32, cy: f32, msdf: bool| -> Inst {
            let mut i = glyph_inst(g, cx, cy + dy, xform, rgba(0x000000, 0.55 * alpha), msdf);
            i.params[1] = 3.0;
            i
        };
        match self.test {
            Test::Karaoke => {
                let sung = rgba(0xFFD400, line_a);
                let rest = rgba(0xFFFFFF, line_a);
                for g in &p.glyphs {
                    let wd = &line.words[g.word];
                    let xf = [0.0, 1.0, 1.0, 0.0];
                    main.push(shadow_of(g, line_a, g.font_size * 0.05, xf, g.cx, g.cy, self.msdf));
                    let mut i = glyph_inst(g, g.cx, g.cy, xf, rest, self.msdf);
                    if t >= wd.end {
                        i.color = sung;
                    } else if t > wd.start {
                        let prog = ((t - wd.start) / (wd.end - wd.start)) as f32;
                        let (x0, x1) = p.word_x[g.word];
                        let split_x = x0 + prog * (x1 - x0);
                        let left = g.cx - g.sprite.w * 0.5;
                        i.extra[0] = (split_x - left) / g.sprite.w;
                        i.color2 = sung;
                    }
                    main.push(i);
                }
            }
            Test::PopBlur => {
                for g in &p.glyphs {
                    let wd = &line.words[g.word];
                    if t < wd.start {
                        continue;
                    }
                    let k = clamp01((t - wd.start) / 0.30);
                    let s = (0.55 + 0.45 * ease_out_back(k)) as f32;
                    let stretch = (1.35 - 0.35 * clamp01(k / 0.6)) as f32;
                    let a = clamp01((t - wd.start) / 0.12) as f32 * line_a;
                    let blur = (14.0 * (1.0 - clamp01((t - wd.start) / 0.22))) as f32;
                    let (x0, x1) = p.word_x[g.word];
                    let pivot = [(x0 + x1) * 0.5, g.cy];
                    let (cx, cy) = (pivot[0] + (g.cx - pivot[0]) * s * stretch, pivot[1] + (g.cy - pivot[1]) * s);
                    let xf = [0.0, s * stretch, s, 0.0];
                    let mut sh = shadow_of(g, a, g.font_size * 0.06, xf, cx, cy, self.msdf);
                    sh.params[1] = 3.0 + blur;
                    main.push(sh);
                    let mut i = glyph_inst(g, cx, cy, xf, rgba(0xFFFFFF, a), self.msdf);
                    i.params[1] = blur;
                    main.push(i);
                }
            }
            Test::Neon => {
                const PALETTE: [u32; 5] = [0xFF2D55, 0x19E6FF, 0xFFE600, 0xB84DFF, 0x3DFF8A];
                for g in &p.glyphs {
                    let wd = &line.words[g.word];
                    let base = PALETTE[g.word % PALETTE.len()];
                    let active = t >= wd.start && t < wd.end;
                    let intensity = if active { 2.4 } else if t >= wd.end { 1.0 } else { 0.45 };
                    let xf = [0.0, 1.0, 1.0, 0.0];
                    glow.push(glyph_inst(g, g.cx, g.cy, xf, scale4(rgba(base, line_a), intensity), self.msdf));
                    // Core: tinted near-white.
                    let core = mix_hex(base, 0xFFFFFF, if active { 0.75 } else { 0.55 });
                    main.push(glyph_inst(g, g.cx, g.cy, xf, rgba(core, line_a), self.msdf));
                }
            }
            Test::Wave => {
                let tt = t as f32;
                for g in &p.glyphs {
                    let ph = g.index as f32 * 0.45;
                    let dy = (g.font_size * 0.14) * (std::f32::consts::TAU * 0.9 * tt - ph).sin();
                    let rot = 0.10 * (std::f32::consts::TAU * 0.9 * tt - ph + 0.8).sin();
                    let xf = [rot, 1.0, 1.0, 0.0];
                    main.push(shadow_of(g, line_a, g.font_size * 0.05, xf, g.cx, g.cy + dy, self.msdf));
                    main.push(glyph_inst(g, g.cx, g.cy + dy, xf, rgba(0xFFFFFF, line_a), self.msdf));
                }
            }
            Test::Compo => {
                for s in &p.shapes {
                    let mut s = *s;
                    s.color = scale4(s.color, line_a);
                    main.push(s);
                }
                for &wi in &p.order {
                    let cw = &p.compo[wi];
                    let wd = &line.words[wi];
                    if t < wd.start {
                        continue;
                    }
                    let k = clamp01((t - wd.start) / 0.14);
                    let s = (0.86 + 0.14 * ease_out_back(k)) as f32;
                    let a = clamp01((t - wd.start) / 0.08) as f32 * line_a;
                    let (c, sn) = (cw.rot.cos(), cw.rot.sin());
                    for g in &cw.glyphs {
                        let dx = (g.cx - cw.pivot[0]) * s;
                        let dy = (g.cy - cw.pivot[1]) * s;
                        let cx = cw.pivot[0] + dx * c - dy * sn;
                        let cy = cw.pivot[1] + dx * sn + dy * c;
                        let mut i = glyph_inst(g, cx, cy, [cw.rot, s, s, 0.0], scale4(cw.color, a), self.msdf);
                        i.params[2] = cw.outline;
                        main.push(i);
                    }
                }
            }
        }
        (glow, 1.0, main, cost)
    }
}

fn mix_hex(a: u32, b: u32, k: f32) -> u32 {
    let ch = |s: u32| -> u32 {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        ((x + (y - x) * k).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn glyph_inst(g: &Placed, cx: f32, cy: f32, xform: [f32; 4], color: [f32; 4], msdf: bool) -> Inst {
    let _ = msdf;
    Inst {
        center: [cx, cy],
        size: [g.sprite.w, g.sprite.h],
        xform,
        uv: g.sprite.uv,
        color,
        color2: color,
        params: [if g.sprite.msdf { 1.0 } else { 0.0 }, 0.0, 0.0, g.sprite.ppt],
        extra: [-1.0, 0.0, 0.0, 0.0],
    }
}

fn solid(cx: f32, cy: f32, w: f32, h: f32, rot: f32, color: [f32; 4]) -> Inst {
    Inst {
        center: [cx, cy],
        size: [w, h],
        xform: [rot, 1.0, 1.0, 0.0],
        uv: [0.0; 4],
        color,
        color2: color,
        params: [2.0, 0.0, 0.0, 1.0],
        extra: [-1.0, 0.0, 0.0, 0.0],
    }
}
