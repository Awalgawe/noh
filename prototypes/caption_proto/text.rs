//! Fonts, shaping/layout (cosmic-text) and the glyph atlases (bitmap coverage
//! through swash, or MSDF through fdsm).
use crate::gpu::{COV_ATLAS, MSDF_ATLAS, MSDF_RANGE, Upload};
use cosmic_text::{Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight, Wrap, fontdb};
use fdsm::transform::Transform;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Cjk,
    Sans,
    SansSemi,
    Jost,
    SerifItalic,
}

pub struct Fonts {
    pub system: FontSystem,
    pub swash: SwashCache,
    families: HashMap<Face, (String, Weight, Style)>,
    pub bytes: HashMap<fontdb::ID, (Arc<Vec<u8>>, u32)>,
}

impl Fonts {
    pub fn load(assets: &std::path::Path, serif: &std::path::Path) -> Result<Self, String> {
        let mut db = fontdb::Database::new();
        let mut bytes = HashMap::new();
        let mut families = HashMap::new();
        let files = [
            (Face::Cjk, assets.join("NOHCJK.otf")),
            (Face::Sans, assets.join("NotoSans-Regular.ttf")),
            (Face::SansSemi, assets.join("NotoSans-SemiBold.ttf")),
            (Face::Jost, assets.join("Jost-ExtraBold.ttf")),
            (Face::SerifItalic, serif.to_path_buf()),
        ];
        for (face, path) in files {
            let data = Arc::new(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
            let ids = db.load_font_source(fontdb::Source::Binary(data.clone()));
            let id = *ids.first().ok_or_else(|| format!("no face in {}", path.display()))?;
            let info = db.face(id).ok_or("face info")?;
            let family = info.families.first().map(|f| f.0.clone()).ok_or("family")?;
            families.insert(face, (family, info.weight, info.style));
            for id in ids {
                let index = db.face(id).map(|f| f.index).unwrap_or(0);
                bytes.insert(id, (data.clone(), index));
            }
        }
        // Only these fonts: fallback (e.g. CJK) resolves inside this set.
        let system = FontSystem::new_with_locale_and_db("en-US".into(), db);
        Ok(Self { system, swash: SwashCache::new(), families, bytes })
    }

    pub fn face_info(&self, face: Face) -> (String, Weight, Style) {
        self.families[&face].clone()
    }
}

pub fn attrs_of(info: &(String, Weight, Style)) -> Attrs<'_> {
    Attrs::new().family(Family::Name(&info.0)).weight(info.1).style(info.2).cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
}

impl Fonts {
}

/// One positioned glyph, relative to the layout origin (top-left of the box).
#[derive(Clone, Debug)]
pub struct PGlyph {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub size: f32,
    pub weight: Weight,
    /// Pen position on the baseline.
    pub x: f32,
    pub baseline: f32,
    pub advance: f32,
    pub word: usize,
    /// Index of this glyph inside its word, and inside the whole line.
    pub letter: usize,
    pub index: usize,
    pub line: usize,
    pub cluster_start: usize,
}

pub struct Laid {
    pub glyphs: Vec<PGlyph>,
    /// Text of each visual line (from glyph cluster ranges).
    pub line_texts: Vec<String>,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<(f32, f32)>, // (line_y baseline, line width)
}

/// Shape and wrap `words` (joined with `sep`) inside `max_width`, centred.
pub fn layout(fonts: &mut Fonts, words: &[String], sep: &str, face: Face, size: f32, max_width: Option<f32>, line_height: f32) -> Laid {
    let info = fonts.face_info(face);
    let attrs = attrs_of(&info);
    let mut spans: Vec<(String, Attrs)> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 && !sep.is_empty() {
            spans.push((sep.to_string(), attrs.clone().metadata(usize::MAX)));
        }
        spans.push((w.clone(), attrs.clone().metadata(i)));
    }
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(size, size * line_height));
    buffer.set_wrap(Wrap::WordOrGlyph);
    buffer.set_size(max_width, None);
    buffer.set_rich_text(spans.iter().map(|(s, a)| (s.as_str(), a.clone())), &attrs, Shaping::Advanced, Some(Align::Center));
    buffer.shape_until_scroll(&mut fonts.system, false);
    let mut glyphs = Vec::new();
    let mut lines = Vec::new();
    let mut line_texts = Vec::new();
    let mut per_word: HashMap<usize, usize> = HashMap::new();
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    for (li, run) in buffer.layout_runs().enumerate() {
        lines.push((run.line_y, run.line_w));
        let lo = run.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
        let hi = run.glyphs.iter().map(|g| g.end).max().unwrap_or(0);
        line_texts.push(run.text.get(lo..hi).unwrap_or("").trim().to_string());
        width = width.max(run.line_w);
        height = height.max(run.line_top + run.line_height);
        for g in run.glyphs {
            if g.metadata == usize::MAX {
                continue;
            }
            let letter = per_word.entry(g.metadata).or_insert(0);
            glyphs.push(PGlyph {
                font_id: g.font_id,
                glyph_id: g.glyph_id,
                size: g.font_size,
                weight: g.font_weight,
                x: g.x + g.font_size * g.x_offset,
                baseline: run.line_y + g.y - g.font_size * g.y_offset,
                advance: g.w,
                word: g.metadata,
                letter: *letter,
                index: glyphs.len(),
                line: li,
                cluster_start: g.start,
            });
            *letter += 1;
        }
    }
    Laid { glyphs, line_texts, width: max_width.unwrap_or(width), height, lines }
}

#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    /// Top-left offset from the pen/baseline origin, in canvas px at the requested size.
    pub left: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
    pub uv: [f32; 4],
    /// Canvas px per atlas texel at the requested size.
    pub ppt: f32,
    pub msdf: bool,
}

struct Shelf {
    size: u32,
    x: u32,
    y: u32,
    row_h: u32,
}

impl Shelf {
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.x + w > self.size {
            self.x = 0;
            self.y += self.row_h;
            self.row_h = 0;
        }
        if self.y + h > self.size {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w;
        self.row_h = self.row_h.max(h);
        Some(at)
    }
}

pub const COV_PAD: u32 = 24;

#[derive(Default, Clone, Copy, Debug)]
pub struct RasterStats {
    pub glyphs: usize,
    pub ms: f64,
    pub texels: u64,
    pub atlas_resets: usize,
}

pub struct Atlas {
    cov_shelf: Shelf,
    msdf_shelf: Shelf,
    cov: HashMap<(fontdb::ID, u16, u32), Option<Sprite>>,
    msdf: HashMap<(fontdb::ID, u16), Option<(f32, f32, f32, f32, [f32; 4])>>,
    /// MSDF em size in texels.
    pub msdf_em: f32,
    /// Physical pixels per logical canvas pixel (bitmap glyphs rasterise at physical size).
    pub px_scale: f32,
    pub stats: RasterStats,
}

impl Atlas {
    pub fn new(msdf_em: f32) -> Self {
        Self {
            cov_shelf: Shelf { size: COV_ATLAS, x: 0, y: 0, row_h: 0 },
            msdf_shelf: Shelf { size: MSDF_ATLAS, x: 0, y: 0, row_h: 0 },
            cov: HashMap::new(),
            msdf: HashMap::new(),
            msdf_em,
            px_scale: 1.0,
            stats: RasterStats::default(),
        }
    }

    pub fn reset_cov(&mut self) {
        self.cov.clear();
        self.cov_shelf = Shelf { size: COV_ATLAS, x: 0, y: 0, row_h: 0 };
        self.stats.atlas_resets += 1;
    }

    /// Coverage sprite rasterised at `size * oversample` px (drawn at `size`).
    pub fn cov_sprite(&mut self, fonts: &mut Fonts, gpu: &dyn Upload, g: &PGlyph, oversample: f32) -> Option<Sprite> {
        let raster = (g.size * oversample * self.px_scale * 4.0).round() / 4.0;
        let key = (g.font_id, g.glyph_id, raster.to_bits());
        if let Some(s) = self.cov.get(&key) {
            return *s;
        }
        let started = Instant::now();
        let (ck, _, _) = CacheKey::new(g.font_id, g.glyph_id, raster, (0.0, 0.0), g.weight, CacheKeyFlags::DISABLE_HINTING);
        let image = fonts.swash.get_image_uncached(&mut fonts.system, ck);
        let sprite = image.and_then(|img| {
            let (iw, ih) = (img.placement.width, img.placement.height);
            if iw == 0 || ih == 0 || img.content != cosmic_text::SwashContent::Mask {
                return None;
            }
            let (pw, ph) = (iw + 2 * COV_PAD, ih + 2 * COV_PAD);
            let at = match self.cov_shelf.alloc(pw, ph) {
                Some(at) => at,
                None => {
                    // Atlas full: the caller's frame still needs this glyph; start over.
                    self.cov.clear();
                    self.cov_shelf = Shelf { size: COV_ATLAS, x: 0, y: 0, row_h: 0 };
                    self.stats.atlas_resets += 1;
                    self.cov_shelf.alloc(pw, ph)?
                }
            };
            let mut padded = vec![0u8; (pw * ph) as usize];
            for row in 0..ih {
                let src = &img.data[(row * iw) as usize..((row + 1) * iw) as usize];
                let dst = ((row + COV_PAD) * pw + COV_PAD) as usize;
                padded[dst..dst + iw as usize].copy_from_slice(src);
            }
            gpu.upload_cov(at.0, at.1, pw, ph, &padded);
            self.stats.texels += u64::from(pw * ph);
            let inv = 1.0 / COV_ATLAS as f32;
            let scale = g.size / raster;
            Some(Sprite {
                left: (img.placement.left as f32 - COV_PAD as f32) * scale,
                top: (img.placement.top as f32 + COV_PAD as f32) * scale,
                w: pw as f32 * scale,
                h: ph as f32 * scale,
                uv: [at.0 as f32 * inv, at.1 as f32 * inv, (at.0 + pw) as f32 * inv, (at.1 + ph) as f32 * inv],
                ppt: scale,
                msdf: false,
            })
        });
        self.stats.glyphs += 1;
        self.stats.ms += started.elapsed().as_secs_f64() * 1000.0;
        self.cov.insert(key, sprite);
        sprite
    }

    /// MSDF sprite: generated once per glyph at `msdf_em` texels per em, any draw size.
    pub fn msdf_sprite(&mut self, fonts: &Fonts, gpu: &dyn Upload, g: &PGlyph) -> Option<Sprite> {
        let key = (g.font_id, g.glyph_id);
        let entry = match self.msdf.get(&key) {
            Some(e) => *e,
            None => {
                let started = Instant::now();
                let e = self.make_msdf(fonts, gpu, g);
                self.stats.glyphs += 1;
                self.stats.ms += started.elapsed().as_secs_f64() * 1000.0;
                self.msdf.insert(key, e);
                e
            }
        };
        let (left_t, top_t, w_t, h_t, uv) = entry?;
        // texel units -> canvas px at this size
        let ppt = g.size / self.msdf_em;
        Some(Sprite { left: left_t * ppt, top: top_t * ppt, w: w_t * ppt, h: h_t * ppt, uv, ppt, msdf: true })
    }

    fn make_msdf(&mut self, fonts: &Fonts, gpu: &dyn Upload, g: &PGlyph) -> Option<(f32, f32, f32, f32, [f32; 4])> {
        use nalgebra::{Affine2, Similarity2, Vector2};
        let (data, index) = fonts.bytes.get(&g.font_id)?;
        let face = ttf_parser::Face::parse(data, *index).ok()?;
        let gid = ttf_parser::GlyphId(g.glyph_id);
        let bbox = face.glyph_bounding_box(gid)?;
        let mut shape = fdsm_ttf_parser::load_shape_from_face(&face, gid)?;
        let shrink = f64::from(face.units_per_em()) / f64::from(self.msdf_em);
        let range = f64::from(MSDF_RANGE);
        let t = nalgebra::convert::<_, Affine2<f64>>(Similarity2::new(
            Vector2::new(range - f64::from(bbox.x_min) / shrink, range - f64::from(bbox.y_min) / shrink),
            0.0,
            1.0 / shrink,
        ));
        let w = ((f64::from(bbox.x_max) - f64::from(bbox.x_min)) / shrink + 2.0 * range).ceil() as u32;
        let h = ((f64::from(bbox.y_max) - f64::from(bbox.y_min)) / shrink + 2.0 * range).ceil() as u32;
        shape.transform(&t);
        let colored = fdsm::shape::Shape::edge_coloring_simple(shape, 0.03, 69441337420);
        let prepared = colored.prepare();
        let mut img = image::RgbImage::new(w, h);
        fdsm::generate::generate_msdf(&prepared, range, &mut img);
        fdsm::render::correct_sign_msdf(&mut img, &prepared, fdsm::bezier::scanline::FillRule::Nonzero);
        let at = self.msdf_shelf.alloc(w, h)?;
        // Flip vertically (font y up -> texture y down) and expand to RGBA.
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = img.get_pixel(x, h - 1 - y).0;
                let o = ((y * w + x) * 4) as usize;
                rgba[o..o + 3].copy_from_slice(&p);
                rgba[o + 3] = 255;
            }
        }
        gpu.upload_msdf(at.0, at.1, w, h, &rgba);
        self.stats.texels += u64::from(w * h);
        let inv = 1.0 / MSDF_ATLAS as f32;
        let left = (f64::from(bbox.x_min) / shrink - range) as f32;
        let top = (f64::from(bbox.y_max) / shrink + range) as f32;
        Some((left, top, w as f32, h as f32, [at.0 as f32 * inv, at.1 as f32 * inv, (at.0 + w) as f32 * inv, (at.1 + h) as f32 * inv]))
    }
}
