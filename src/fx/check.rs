//! The counted checks of one evaluated frame, the same for the validation pass
//! and for every frame the preview or the export evaluates afterwards: a scene
//! is refused on quantities that are the same on every machine, never on a
//! measured time.
use super::draw::{
    Block, Draw, Frame, LETTER_QUAD_EM, MAX_AREA, MAX_BLOCKS, MAX_BLURS, MAX_DRAWS, MAX_GLYPHS,
    MAX_PARTICLES, MAX_PASSES, Source, layout_key, measure,
};
use super::shapes::{PathDraw, ShapeCache, check_paths};
use super::text::Fonts;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What one frame counted, for the report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    /// Upper bound of the particles drawn.
    pub particles: usize,
    /// Letter instances, every pass counted.
    pub glyphs: usize,
    /// Surface covered, in frames, every pass counted.
    pub area: f32,
    pub blurs: usize,
    pub passes: usize,
    pub paths: usize,
    /// Segments of the paths once flattened, and the surface of their boxes.
    pub segments: usize,
    pub shape_area: f32,
    /// Triangles of fills taken although they were not ears (said, not refused).
    pub forced: usize,
}

/// Keeps what the checks of successive frames share: the layouts' places, the
/// cut shapes, and the distinct texts the scene has used so far (capped over the
/// whole scene, not per frame).
pub struct Checker {
    fonts: Rc<RefCell<Fonts>>,
    w: f32,
    h: f32,
    shapes: ShapeCache,
    /// Each distinct layout with the largest scale it reaches.
    blocks: HashMap<String, (Block, f32)>,
}

/// Passes a frame costs: each run of engine draws or of shapes, within each
/// layer and between layers.
pub fn pass_count(f: &Frame) -> usize {
    super::draw::groups(f.draws.len(), &f.layers)
        .iter()
        .map(|(r, layer)| {
            // A layer's shader is one pass more, over its content (or alone).
            let shader = layer.is_some_and(|i| f.layers[i].shader.is_some());
            super::draw::runs(&f.draws[r.clone()]).len() + usize::from(shader)
        })
        .sum()
}

/// The shader layers of a frame and the pixels they compute, in frames (a
/// half-resolution layer computes a quarter of the frame).
pub fn shader_load(f: &Frame) -> (usize, f32) {
    let specs = f.layers.iter().filter_map(|l| l.shader.as_ref());
    specs.fold((0, 0.0), |(n, a), s| {
        (n + 1, a + if s.full { 1.0 } else { 0.25 })
    })
}

/// What a frame of prepared instances counts against the engine's caps, for a
/// front end whose frames always draw (a caption style): the first cap it is
/// over, as a sentence. Said to the user as a warning, never a refusal: a style
/// that draws today must not start failing on long lyrics.
pub fn over_caps(p: &super::draw::Prepared, w: f32, h: f32) -> Option<String> {
    let glyphs = p.glyph_instances();
    if glyphs > MAX_GLYPHS {
        return Some(format!(
            "{glyphs} letters drawn (every pass counted) exceed {MAX_GLYPHS}"
        ));
    }
    let area = p.area(w, h);
    if area > MAX_AREA {
        return Some(format!(
            "the draws cover {area:.2} frames (every pass counted), more than {MAX_AREA}"
        ));
    }
    let blurs = p.blurs();
    if blurs > MAX_BLURS {
        return Some(format!(
            "{blurs} different blurs in one frame exceed {MAX_BLURS}"
        ));
    }
    None
}

/// The share of the box `[x0, y0, x1, y1]` (fractions of the frame) that falls
/// inside the frame.
fn inside(b: [f32; 4]) -> f32 {
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return 0.0;
    }
    let iw = (b[2].min(1.0) - b[0].max(0.0)).max(0.0);
    let ih = (b[3].min(1.0) - b[1].max(0.0)).max(0.0);
    iw * ih / (bw * bh)
}

impl Checker {
    pub fn new(fonts: Rc<RefCell<Fonts>>, w: u32, h: u32) -> Self {
        Self {
            fonts,
            w: w as f32,
            h: h as f32,
            shapes: ShapeCache::default(),
            blocks: HashMap::new(),
        }
    }

    /// Every distinct layout the checked frames used, with the largest scale it
    /// reaches: what the painter prepares before the first frame. A frame
    /// evaluated later (an instant off the validated grid) may bring a layout or
    /// a scale not in this list: the painter must then prepare it or leave the
    /// frame's overlay out, never draw it with missing letters.
    pub fn blocks(&self) -> Vec<(Block, f32)> {
        let mut v: Vec<(Block, f32)> = self.blocks.values().cloned().collect();
        v.sort_by(|a, b| a.0.text.cmp(&b.0.text).then(a.0.size.total_cmp(&b.0.size)));
        v
    }

    /// What one frame covers and how many blurred layers it asks for, counted
    /// from the draws alone (and the fonts, for where words fall).
    ///
    /// - A letter counts as a square of `LETTER_QUAD_EM` em at the scale it is
    ///   drawn at, once per pass (fill, outline, shadow, lit edge). The share of
    ///   its word outside the frame is left out when the text is not rotated.
    /// - A rectangle counts its surface inside the frame (all of it when
    ///   rotated), twice with a glow. A particle counts its largest quad, twice
    ///   with a glow: where it is on screen is not known here.
    /// - Paths have their own cap on surface (`shapes::MAX_SHAPE_AREA`).
    fn cover(&self, draws: &[Draw], t: f64) -> (f32, usize) {
        let (w, h) = (self.w, self.h);
        let short = w.min(h);
        let frame = w * h;
        let mut area = 0.0f32;
        let mut blurs: Vec<u32> = Vec::new();
        let mut prepared_blurs = 0;
        for d in draws {
            match d {
                Draw::Rect(r) => {
                    let a = (r.w * r.h).abs();
                    let seen = if r.rot == 0.0 {
                        let (hw, hh) = (r.w.abs() / 2.0, r.h.abs() / 2.0);
                        a * inside([r.x - hw, r.y - hh, r.x + hw, r.y + hh])
                    } else {
                        a
                    };
                    if r.opacity > 0.0 {
                        area += seen * if r.glow > 0.0 { 2.0 } else { 1.0 };
                    }
                }
                Draw::Path(_) => {}
                Draw::Prepared(p) => {
                    area += p.area(w, h);
                    prepared_blurs += p.blurs();
                }
                Draw::Emitter(e) => {
                    let side = e.size * short * 1.4;
                    let quad = side * side * if e.shape == "petal" { 1.7 } else { 1.0 };
                    let passes = if e.glow > 0.0 { 2.0 } else { 1.0 };
                    area += e.alive_bound(t) as f32 * (quad / frame).min(1.0) * passes;
                }
                Draw::Text(td) => {
                    if td.opacity <= 0.0 {
                        continue;
                    }
                    let b = &td.block;
                    let em = b.size * short;
                    let passes = 1.0
                        + f32::from(u8::from(td.shadow.is_some()))
                        + f32::from(u8::from(td.outline.is_some()))
                        + f32::from(u8::from(td.glow > 0.0));
                    let boxes = if b.rot == 0.0 {
                        measure(&mut self.fonts.borrow_mut(), b, td.scale, w, h).1
                    } else {
                        Vec::new()
                    };
                    let pad = 0.25 * em * td.scale;
                    for (k, word) in b.text.split_whitespace().enumerate() {
                        let ws = td.words.get(k).map_or(1.0, |m| m.scale.abs());
                        let s = td.scale * ws;
                        let side = LETTER_QUAD_EM * em * s;
                        let letters = word.chars().count() as f32;
                        let seen = boxes.get(k).map_or(1.0, |x| {
                            inside([
                                x.x0 - pad / w,
                                x.y0 - pad / h,
                                x.x1 + pad / w,
                                x.y1 + pad / h,
                            ])
                        });
                        area += letters * (side * side / frame) * seen * passes;
                        if let Some(sh) = &td.shadow {
                            let key = ((sh.blur * em * s).min(96.0) * 4.0).round() as u32;
                            if key > 0 && !blurs.contains(&key) {
                                blurs.push(key);
                            }
                        }
                    }
                }
            }
        }
        (area, blurs.len() + prepared_blurs)
    }

    /// The first of the counted checks, on its own: a front end with caps of its
    /// own (a script's costly calls) says them after this one and before the rest.
    pub fn draws_cap(&self, f: &Frame, t: f64) -> Result<(), String> {
        if f.draws.len() > MAX_DRAWS {
            return Err(format!(
                "at t={t:.3}: {} draws exceed {MAX_DRAWS}",
                f.draws.len()
            ));
        }
        Ok(())
    }

    /// The counted checks of one frame at `t`. An error names the instant and the
    /// cap; the caller refuses the frame (export) or leaves its overlay out
    /// (preview).
    pub fn check(&mut self, f: &Frame, t: f64) -> Result<Counts, String> {
        let at = |e: String| format!("at t={t:.3}: {e}");
        self.draws_cap(f, t)?;
        let passes = pass_count(f);
        if passes > MAX_PASSES {
            return Err(at(format!(
                "{passes} passes exceed {MAX_PASSES} (each layer is one, and so is each run of draws between two layers)"
            )));
        }
        if f.layers.iter().any(|l| !l.opacity.is_finite()) {
            return Err(at("a layer's opacity is not finite".into()));
        }
        if let Some(v) = &f.video {
            v.check().map_err(at)?;
        }
        let (shaders, shader_area) = shader_load(f);
        if shaders > super::layer_shader::MAX_SHADER_LAYERS {
            return Err(at(format!(
                "{shaders} shader layers exceed {}",
                super::layer_shader::MAX_SHADER_LAYERS
            )));
        }
        if shader_area > super::layer_shader::MAX_SHADER_AREA {
            return Err(at(format!(
                "shader layers computing {shader_area:.2} frames of pixels exceed {} (full resolution counts 1, half a quarter)",
                super::layer_shader::MAX_SHADER_AREA
            )));
        }
        let mut c = Counts {
            passes,
            ..Counts::default()
        };
        for d in &f.draws {
            d.check().map_err(at)?;
            match d {
                Draw::Emitter(e) => c.particles += e.alive_bound(t),
                Draw::Text(td) => c.glyphs += td.glyph_instances(),
                Draw::Prepared(p) => c.glyphs += p.glyph_instances(),
                Draw::Rect(_) | Draw::Path(_) => {}
            }
        }
        if c.particles > MAX_PARTICLES {
            return Err(at(format!(
                "{} particles exceed {MAX_PARTICLES}",
                c.particles
            )));
        }
        if c.glyphs > MAX_GLYPHS {
            return Err(at(format!(
                "{} letters drawn (every pass counted) exceed {MAX_GLYPHS}",
                c.glyphs
            )));
        }
        (c.area, c.blurs) = self.cover(&f.draws, t);
        if c.area > MAX_AREA {
            return Err(at(format!(
                "the draws cover {:.2} frames (every pass counted), more than {MAX_AREA}",
                c.area
            )));
        }
        if c.blurs > MAX_BLURS {
            return Err(at(format!(
                "{} different shadow blurs in one frame exceed {MAX_BLURS}",
                c.blurs
            )));
        }
        let paths: Vec<&PathDraw> = f
            .draws
            .iter()
            .filter_map(|d| if let Draw::Path(p) = d { Some(p) } else { None })
            .collect();
        if !paths.is_empty() {
            (c.segments, c.shape_area, c.forced) =
                check_paths(&paths, &mut self.shapes, self.w, self.h).map_err(at)?;
            c.paths = paths.len();
        }
        for d in &f.draws {
            let (block, peak) = match d {
                Draw::Text(td) => (Some(&td.block), td.peak_scale()),
                Draw::Emitter(e) => match &e.source {
                    Source::Text(b) => (Some(b), 1.0),
                    Source::Rect { .. } => (None, 1.0),
                },
                Draw::Rect(_) | Draw::Path(_) | Draw::Prepared(_) => (None, 1.0),
            };
            if let Some(b) = block {
                let e = self
                    .blocks
                    .entry(layout_key(b, self.w, self.h))
                    .or_insert_with(|| (b.clone(), peak));
                e.1 = e.1.max(peak);
                if self.blocks.len() > MAX_BLOCKS {
                    return Err(at(format!(
                        "more than {MAX_BLOCKS} different texts or text settings in the scene"
                    )));
                }
            }
        }
        Ok(c)
    }
}
