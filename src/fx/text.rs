//! Fonts, shaping/layout (cosmic-text) and the glyph atlases (bitmap coverage
//! through swash, or MSDF through fdsm).
use super::shaders::{COV_ATLAS, MSDF_ATLAS, MSDF_RANGE, Upload};
use cosmic_text::{
    Align, Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SwashCache, Weight, Wrap, fontdb,
};
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
    /// Inter at weight 600 (variable file).
    InterSemi,
    /// Barlow Condensed ExtraBold (800).
    CondensedExtraBold,
    /// Jost at weight 500 (variable file; `Jost` stays the static ExtraBold).
    JostMedium,
    /// An installed font, the n-th one a plugin asked for (`system:<Family>`).
    System(u16),
}

/// Every name `Face::named` takes, as a font setting may give it.
pub const FACE_NAMES: [&str; 8] = [
    "cjk",
    "sans",
    "sans_semibold",
    "jost_extrabold",
    "serif_italic",
    "inter_semibold",
    "condensed_extrabold",
    "jost_medium",
];

impl Face {
    /// The bundled face a plugin names (layout.font, a font param), if any.
    pub fn named(name: &str) -> Option<Face> {
        Some(match name {
            "cjk" => Face::Cjk,
            "sans" => Face::Sans,
            "sans_semibold" => Face::SansSemi,
            "jost_extrabold" => Face::Jost,
            "serif_italic" => Face::SerifItalic,
            "inter_semibold" => Face::InterSemi,
            "condensed_extrabold" => Face::CondensedExtraBold,
            "jost_medium" => Face::JostMedium,
            _ => return None,
        })
    }
}

/// Why a `system:<Family>` value is not drawn in that family (the plugin's own
/// face is drawn instead).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lack {
    /// Not installed on this machine: NOH says so (the warning the maintainer asked for).
    Missing(String),
    /// The installed fonts are not read yet: not a warning; whoever lays the
    /// text out again once they are read (the app rebuilds its scene) draws it.
    NotRead(String),
}

/// The bytes of a font the MSDF path outlines: built into the executable, or an
/// installed file mapped in memory (shared with fontdb, not copied).
#[derive(Clone)]
pub enum FontData {
    Static(&'static [u8]),
    Shared(Arc<dyn AsRef<[u8]> + Send + Sync>),
}

impl FontData {
    pub fn bytes(&self) -> &[u8] {
        match self {
            FontData::Static(b) => b,
            FontData::Shared(b) => (**b).as_ref(),
        }
    }
}

/// The installed fonts, read once and shared (the app keeps them for its scenes).
pub type InstalledFonts = Arc<fontdb::Database>;

/// The installed fonts (fontdb's system folders, or the given folders in tests
/// and capture mode). Reading them takes a while: build it once, off the UI thread.
pub fn system_fonts(dirs: Option<&[std::path::PathBuf]>) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    match dirs {
        None => db.load_system_fonts(),
        Some(dirs) => {
            for d in dirs {
                db.load_fonts_dir(d);
            }
        }
    }
    db
}

/// The family names of a font database, sorted, without duplicates (the picker's
/// « installed fonts »).
pub fn families_of(db: &fontdb::Database) -> Vec<String> {
    let mut names: Vec<String> = db
        .faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

pub struct Fonts {
    pub system: FontSystem,
    pub swash: SwashCache,
    families: HashMap<Face, (String, Weight, Style)>,
    pub bytes: HashMap<fontdb::ID, (FontData, u32)>,
    /// The installed fonts `system:<Family>` is looked up in (None: not read yet).
    installed: Option<Arc<fontdb::Database>>,
    /// `system:<Family>` at a weight and style already added: its face.
    system_faces: HashMap<(String, u16, bool), Face>,
    /// The `wght` a variable file is drawn at, for the faces that set one
    /// (the MSDF path outlines at it; cosmic-text applies it itself).
    pub weights: HashMap<fontdb::ID, f32>,
}

/// The engine's fonts, built into the executable (OFL; see assets/*OFL*.txt and
/// assets/NOTICE.md), with the weight a variable file is drawn at (None: the
/// file's own).
const FACES: [(Face, &[u8], Option<u16>); 8] = [
    (Face::Cjk, crate::captions::FONT, None),
    (
        Face::Sans,
        include_bytes!("../../assets/NotoSans-Regular.ttf"),
        None,
    ),
    (
        Face::SansSemi,
        include_bytes!("../../assets/NotoSans-SemiBold.ttf"),
        None,
    ),
    (
        Face::Jost,
        include_bytes!("../../assets/Jost-ExtraBold.ttf"),
        None,
    ),
    (
        Face::SerifItalic,
        include_bytes!("../../assets/NotoSerif-Italic.ttf"),
        None,
    ),
    (
        Face::InterSemi,
        include_bytes!("../../assets/Inter-Variable.ttf"),
        Some(600),
    ),
    (
        Face::CondensedExtraBold,
        include_bytes!("../../assets/BarlowCondensed-ExtraBold.ttf"),
        None,
    ),
    (
        Face::JostMedium,
        include_bytes!("../../assets/Jost-Variable.ttf"),
        Some(500),
    ),
];

impl Fonts {
    pub fn load() -> Result<Self, String> {
        let mut db = fontdb::Database::new();
        let mut bytes = HashMap::new();
        let mut families = HashMap::new();
        let mut weights = HashMap::new();
        for (face, data, weight) in FACES {
            let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
            let id = *ids
                .first()
                .ok_or_else(|| format!("no face in font {face:?}"))?;
            let info = db.face(id).ok_or("face info")?;
            let family = info.families.first().map(|f| f.0.clone()).ok_or("family")?;
            // A variable file is asked for at its set weight, not its default one.
            let asked = weight.map_or(info.weight, Weight);
            families.insert(face, (family, asked, info.style));
            if let Some(w) = weight {
                weights.insert(id, f32::from(w));
            }
            for id in ids {
                let index = db.face(id).map(|f| f.index).unwrap_or(0);
                bytes.insert(id, (FontData::Static(data), index));
            }
        }
        // Only these fonts: fallback (e.g. CJK) resolves inside this set.
        let system = FontSystem::new_with_locale_and_db("en-US".into(), db);
        Ok(Self {
            system,
            swash: SwashCache::new(),
            families,
            bytes,
            weights,
            installed: None,
            system_faces: HashMap::new(),
        })
    }

    /// The installed fonts `system:<Family>` resolves in (see `system_fonts`).
    pub fn with_installed(mut self, db: Arc<fontdb::Database>) -> Self {
        self.installed = Some(db);
        self
    }

    pub fn set_installed(&mut self, db: Arc<fontdb::Database>) {
        self.installed = Some(db);
    }

    pub fn face_info(&self, face: Face) -> (String, Weight, Style) {
        self.families[&face].clone()
    }

    /// The face a plugin's font value names: a bundled face name, or
    /// `system:<Family>` looked up in the installed fonts at the weight and style
    /// of `fallback` (the plugin's own bundled face), else the nearest one; a
    /// variable file is drawn at that weight, a family with one weight as it is
    /// (no synthetic bold or italic). Only the matching face joins the engine's
    /// fonts, never the installed set. A family that is not installed gives
    /// `fallback` and `Lack::Missing`: NOH reports it, it does not choose another
    /// font; before the installed fonts are read, `fallback` and `Lack::NotRead`.
    /// An unknown bundled name is the old Sans.
    pub fn face(&mut self, name: &str, fallback: Face) -> (Face, Option<Lack>) {
        let Some(family) = name.strip_prefix("system:") else {
            return (Face::named(name).unwrap_or(Face::Sans), None);
        };
        let (_, weight, style) = self.face_info(fallback);
        let italic = style != Style::Normal;
        let key = (family.to_string(), weight.0, italic);
        if let Some(face) = self.system_faces.get(&key) {
            return (*face, None);
        }
        if self.installed.is_none() {
            return (fallback, Some(Lack::NotRead(family.to_string())));
        }
        match self.add_installed(family, weight, style) {
            Some(face) => {
                self.system_faces.insert(key, face);
                (face, None)
            }
            None => (fallback, Some(Lack::Missing(family.to_string()))),
        }
    }

    fn add_installed(&mut self, family: &str, weight: Weight, style: Style) -> Option<Face> {
        let installed = self.installed.clone()?;
        let id = installed.query(&fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight,
            stretch: fontdb::Stretch::Normal,
            style,
        })?;
        // The matching face joins the engine's fonts alone, under a name of its
        // own: a family that NOH also ships (e.g. Barlow Condensed) is then drawn
        // from the installed file, the rest of the installed set stays out, and
        // the bundled faces keep their own fallback (tested with a wide-coverage
        // file installed).
        let n = self.system_faces.len() as u16;
        let alias = format!("NOH installed {n}");
        let mut info = installed.face(id)?.clone();
        info.families = vec![(alias.clone(), fontdb::Language::English_UnitedStates)];
        info.id = fontdb::ID::dummy();
        let db = self.system.db_mut();
        let mine = db.push_face_info(info);
        // SAFETY: fontdb maps the installed file in memory (shared with the MSDF
        // path, not copied); as for any mapped font, the file must not be
        // truncated while NOH draws with it.
        let Some((data, index)) = (unsafe { db.make_shared_face_data(mine) }) else {
            // The file went away after the installed fonts were read: missing,
            // and nothing is left behind in the engine's fonts.
            db.remove_face(mine);
            return None;
        };
        let face_info = db.face(mine)?;
        let face_style = face_info.style;
        let own_weight = face_info.weight;
        // A variable file is asked for, and outlined, at the plugin's weight; a
        // file with one weight is drawn as it is.
        let variable = ttf_parser::Face::parse((*data).as_ref(), index).is_ok_and(|f| {
            f.variation_axes()
                .into_iter()
                .any(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))
        });
        let asked = if variable { weight } else { own_weight };
        let face = Face::System(n);
        self.families.insert(face, (alias, asked, face_style));
        if variable {
            self.weights.insert(mine, f32::from(weight.0));
        }
        self.bytes.insert(mine, (FontData::Shared(data), index));
        Some(face)
    }
}

pub fn attrs_of(info: &(String, Weight, Style)) -> Attrs<'_> {
    Attrs::new()
        .family(Family::Name(&info.0))
        .weight(info.1)
        .style(info.2)
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
}

/// How the font picker shows a face's own name (`Fonts::label`).
#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    /// The name drawn in the face: a white alpha mask, `w` × `h` px, its
    /// baseline at the same height in every label of one size.
    Drawn { w: u32, h: u32, alpha: Vec<u8> },
    /// A symbol font (a Windows Symbol cmap, e.g. Wingdings): its name drawn in
    /// it would be pictures; the picker writes it in the interface font.
    Symbol,
    /// The face lacks glyphs for its own name, or could not be read.
    Unreadable,
}

impl Fonts {
    /// `text` drawn in `face` at `px` pixels per em, with no glyph borrowed from
    /// another font: a face that cannot draw its whole name gives `Unreadable`.
    pub fn label(&mut self, face: Face, text: &str, px: f32) -> Label {
        let info = self.face_info(face);
        let query = fontdb::Query {
            families: &[Family::Name(&info.0)],
            weight: info.1,
            stretch: fontdb::Stretch::Normal,
            style: info.2,
        };
        let Some(id) = self.system.db().query(&query) else {
            return Label::Unreadable;
        };
        let symbol = self
            .system
            .db()
            .with_face_data(id, |data, index| {
                ttf_parser::Face::parse(data, index)
                    .ok()
                    .and_then(|f| f.tables().cmap)
                    .is_some_and(|cmap| {
                        cmap.subtables.into_iter().any(|s| {
                            s.platform_id == ttf_parser::PlatformId::Windows && s.encoding_id == 0
                        })
                    })
            })
            .unwrap_or(false);
        if symbol {
            return Label::Symbol;
        }
        let attrs = attrs_of(&info);
        let line = (px * 1.3).ceil();
        let mut buffer = Buffer::new(&mut self.system, Metrics::new(px, line));
        buffer.set_size(None, None);
        buffer.set_rich_text([(text, attrs.clone())], &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.system, false);
        let mut glyphs = Vec::new();
        let mut width = 0.0f32;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            for g in run.glyphs {
                if g.font_id != id || g.glyph_id == 0 {
                    return Label::Unreadable;
                }
                glyphs.push(g.physical((0.0, run.line_y), 1.0));
            }
        }
        if glyphs.is_empty() {
            return Label::Unreadable;
        }
        let (w, h) = (width.ceil() as u32 + 2, line as u32);
        let mut alpha = vec![0u8; (w * h) as usize];
        for g in glyphs {
            let Some(img) = self.swash.get_image_uncached(&mut self.system, g.cache_key) else {
                continue;
            };
            if img.content != cosmic_text::SwashContent::Mask {
                return Label::Unreadable;
            }
            let (iw, ih) = (img.placement.width as i32, img.placement.height as i32);
            let (x0, y0) = (g.x + img.placement.left, g.y - img.placement.top);
            for row in 0..ih {
                let y = y0 + row;
                if y < 0 || y >= h as i32 {
                    continue;
                }
                for col in 0..iw {
                    let x = x0 + col;
                    if x < 0 || x >= w as i32 {
                        continue;
                    }
                    let a = img.data[(row * iw + col) as usize];
                    let at = (y as u32 * w + x as u32) as usize;
                    alpha[at] = alpha[at].max(a);
                }
            }
        }
        Label::Drawn { w, h, alpha }
    }
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
    /// Byte offset of the glyph's cluster in its paragraph (cosmic-text restarts
    /// at 0 after each typed line break).
    pub cluster_start: usize,
    /// Characters of its word before this glyph's cluster (syllables find their glyphs).
    pub char_in_word: usize,
    /// Its place in its visual row for letter spacing: characters of the row
    /// (spaces included) before its cluster, minus half the row's characters.
    pub row_pos: f32,
}

pub struct Laid {
    pub glyphs: Vec<PGlyph>,
    /// Text of each visual line (from glyph cluster ranges).
    pub line_texts: Vec<String>,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<(f32, f32)>, // (line_y baseline, line width)
    /// Each visual line's box: its top and height (the font size times the line
    /// height, the tallest word's on a row of mixed sizes).
    pub rows: Vec<(f32, f32)>,
    /// A line holds a word wider than `max_width`: it is kept whole, wider than
    /// the box and centred on it (NOH never cuts a word between letters).
    pub too_wide: bool,
}

/// Shape and wrap `words` (joined with `sep`) inside `max_width`, centred. At the
/// word indices in `breaks` a new line starts (a cue's typed line, Task 43 step
/// C); each typed line still wraps within `max_width`.
pub fn layout(
    fonts: &mut Fonts,
    words: &[String],
    sep: &str,
    breaks: &[usize],
    face: Face,
    size: f32,
    max_width: Option<f32>,
    line_height: f32,
) -> Laid {
    layout_sized(
        fonts,
        words,
        sep,
        breaks,
        face,
        size,
        &[],
        max_width,
        line_height,
    )
}

/// How the rows of a laid-out block sit in its box (`layout.align`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAlign {
    Center,
    Left,
    Right,
}

/// `layout` with a size factor per word (`word_sizes[i]`, missing = 1): a word
/// drawn larger takes more room and its row grows, as a CSS `font-size` on a
/// span does; separators keep `size`.
#[allow(clippy::too_many_arguments)]
pub fn layout_sized(
    fonts: &mut Fonts,
    words: &[String],
    sep: &str,
    breaks: &[usize],
    face: Face,
    size: f32,
    word_sizes: &[f32],
    max_width: Option<f32>,
    line_height: f32,
) -> Laid {
    layout_aligned(
        fonts,
        words,
        sep,
        breaks,
        face,
        size,
        word_sizes,
        max_width,
        line_height,
        RowAlign::Center,
    )
}

/// `layout_sized` with its rows centred, or set against the left or right edge
/// of the box. A row wider than the box stays whole: centred on it, or from the
/// edge it is set against. Letter spacing (`row_pos`) grows from that edge too.
#[allow(clippy::too_many_arguments)]
pub fn layout_aligned(
    fonts: &mut Fonts,
    words: &[String],
    sep: &str,
    breaks: &[usize],
    face: Face,
    size: f32,
    word_sizes: &[f32],
    max_width: Option<f32>,
    line_height: f32,
    align: RowAlign,
) -> Laid {
    let info = fonts.face_info(face);
    let attrs = attrs_of(&info);
    let mut spans: Vec<(String, Attrs)> = Vec::new();
    // Byte offset of each word in its paragraph: cosmic-text's glyph offsets
    // (start, end) restart at 0 on each line of text after a "\n".
    let mut word_at = Vec::with_capacity(words.len());
    let mut at = 0;
    for (i, w) in words.iter().enumerate() {
        if i > 0 && breaks.contains(&i) {
            spans.push(("\n".to_string(), attrs.clone().metadata(usize::MAX)));
            at = 0;
        } else if i > 0 && !sep.is_empty() {
            spans.push((sep.to_string(), attrs.clone().metadata(usize::MAX)));
            at += sep.len();
        }
        word_at.push(at);
        at += w.len();
        let word_attrs = match word_sizes.get(i) {
            Some(&f) if f != 1.0 => attrs
                .clone()
                .metrics(Metrics::new(size * f, size * f * line_height)),
            _ => attrs.clone(),
        };
        spans.push((w.clone(), word_attrs.metadata(i)));
    }
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(size, size * line_height));
    // Between words only (UAX #14, so CJK still breaks between ideographs): a word
    // wider than the box stays whole and is said (`too_wide`), never cut.
    buffer.set_wrap(Wrap::Word);
    buffer.set_size(max_width, None);
    buffer.set_rich_text(
        spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
        &attrs,
        Shaping::Advanced,
        Some(match align {
            RowAlign::Center => Align::Center,
            RowAlign::Left => Align::Left,
            RowAlign::Right => Align::Right,
        }),
    );
    buffer.shape_until_scroll(&mut fonts.system, false);
    let mut glyphs = Vec::new();
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    let mut line_texts = Vec::new();
    let mut per_word: HashMap<usize, usize> = HashMap::new();
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    let mut too_wide = false;
    for (li, run) in buffer.layout_runs().enumerate() {
        // cosmic-text centres a line by (box - line) / 2, floored at 0: a line wider
        // than the box would start at its left edge (right edge for a right-to-left
        // line, which spans [box - line, box]). Centre it on the box instead.
        let shift = match max_width {
            Some(box_w) if run.line_w > box_w + 0.5 => {
                too_wide = true;
                match align {
                    RowAlign::Center => {
                        let half = (run.line_w - box_w) * 0.5;
                        if run.rtl { half } else { -half }
                    }
                    // From the edge the row is set against, wherever the shaper put it.
                    RowAlign::Left | RowAlign::Right => {
                        let start = run.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
                        let want = if align == RowAlign::Left {
                            0.0
                        } else {
                            box_w - run.line_w
                        };
                        if start.is_finite() { want - start } else { 0.0 }
                    }
                }
            }
            _ => 0.0,
        };
        lines.push((run.line_y, run.line_w));
        rows.push((run.line_top, run.line_height));
        let lo = run.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
        let hi = run.glyphs.iter().map(|g| g.end).max().unwrap_or(0);
        let row_chars = run.text.get(lo..hi).map_or(0, |s| s.chars().count()) as f32;
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
                x: g.x + g.font_size * g.x_offset + shift,
                baseline: run.line_y + g.y - g.font_size * g.y_offset,
                advance: g.w,
                word: g.metadata,
                letter: *letter,
                index: glyphs.len(),
                line: li,
                cluster_start: g.start,
                char_in_word: words[g.metadata]
                    .get(..g.start.saturating_sub(word_at[g.metadata]))
                    .map_or(0, |s| s.chars().count()),
                row_pos: run.text.get(lo..g.start).map_or(0, |s| s.chars().count()) as f32
                    - match align {
                        RowAlign::Center => row_chars * 0.5,
                        RowAlign::Left => 0.0,
                        RowAlign::Right => (row_chars - 1.0).max(0.0),
                    },
            });
            *letter += 1;
        }
    }
    Laid {
        glyphs,
        line_texts,
        width: max_width.unwrap_or(width),
        height,
        lines,
        rows,
        too_wide,
    }
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
            cov_shelf: Shelf {
                size: COV_ATLAS,
                x: 0,
                y: 0,
                row_h: 0,
            },
            msdf_shelf: Shelf {
                size: MSDF_ATLAS,
                x: 0,
                y: 0,
                row_h: 0,
            },
            cov: HashMap::new(),
            msdf: HashMap::new(),
            msdf_em,
            px_scale: 1.0,
            stats: RasterStats::default(),
        }
    }

    pub fn reset_cov(&mut self) {
        self.cov.clear();
        self.cov_shelf = Shelf {
            size: COV_ATLAS,
            x: 0,
            y: 0,
            row_h: 0,
        };
        self.stats.atlas_resets += 1;
    }

    /// Coverage sprite rasterised at `size * oversample` px (drawn at `size`).
    pub fn cov_sprite(
        &mut self,
        fonts: &mut Fonts,
        gpu: &dyn Upload,
        g: &PGlyph,
        oversample: f32,
    ) -> Option<Sprite> {
        let raster = (g.size * oversample * self.px_scale * 4.0).round() / 4.0;
        let key = (g.font_id, g.glyph_id, raster.to_bits());
        if let Some(s) = self.cov.get(&key) {
            return *s;
        }
        let started = Instant::now();
        let (ck, _, _) = CacheKey::new(
            g.font_id,
            g.glyph_id,
            raster,
            (0.0, 0.0),
            g.weight,
            CacheKeyFlags::DISABLE_HINTING,
        );
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
                    self.cov_shelf = Shelf {
                        size: COV_ATLAS,
                        x: 0,
                        y: 0,
                        row_h: 0,
                    };
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
                uv: [
                    at.0 as f32 * inv,
                    at.1 as f32 * inv,
                    (at.0 + pw) as f32 * inv,
                    (at.1 + ph) as f32 * inv,
                ],
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
        Some(Sprite {
            left: left_t * ppt,
            top: top_t * ppt,
            w: w_t * ppt,
            h: h_t * ppt,
            uv,
            ppt,
            msdf: true,
        })
    }

    fn make_msdf(
        &mut self,
        fonts: &Fonts,
        gpu: &dyn Upload,
        g: &PGlyph,
    ) -> Option<(f32, f32, f32, f32, [f32; 4])> {
        use nalgebra::{Affine2, Similarity2, Vector2};
        let (data, index) = fonts.bytes.get(&g.font_id)?;
        let mut face = ttf_parser::Face::parse(data.bytes(), *index).ok()?;
        // A variable file with a set weight is outlined at it (bbox included).
        if let Some(w) = fonts.weights.get(&g.font_id) {
            face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), *w)?;
        }
        let gid = ttf_parser::GlyphId(g.glyph_id);
        let bbox = face.glyph_bounding_box(gid)?;
        let mut shape = fdsm_ttf_parser::load_shape_from_face(&face, gid)?;
        let shrink = f64::from(face.units_per_em()) / f64::from(self.msdf_em);
        let range = f64::from(MSDF_RANGE);
        let t = nalgebra::convert::<_, Affine2<f64>>(Similarity2::new(
            Vector2::new(
                range - f64::from(bbox.x_min) / shrink,
                range - f64::from(bbox.y_min) / shrink,
            ),
            0.0,
            1.0 / shrink,
        ));
        let w =
            ((f64::from(bbox.x_max) - f64::from(bbox.x_min)) / shrink + 2.0 * range).ceil() as u32;
        let h =
            ((f64::from(bbox.y_max) - f64::from(bbox.y_min)) / shrink + 2.0 * range).ceil() as u32;
        shape.transform(&t);
        let colored = fdsm::shape::Shape::edge_coloring_simple(shape, 0.03, 69441337420);
        let prepared = colored.prepare();
        let mut img = image::RgbImage::new(w, h);
        fdsm::generate::generate_msdf(&prepared, range, &mut img);
        fdsm::render::correct_sign_msdf(
            &mut img,
            &prepared,
            fdsm::bezier::scanline::FillRule::Nonzero,
        );
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
        Some((
            left,
            top,
            w as f32,
            h as f32,
            [
                at.0 as f32 * inv,
                at.1 as f32 * inv,
                (at.0 + w) as f32 * inv,
                (at.1 + h) as f32 * inv,
            ],
        ))
    }
}

#[cfg(test)]
mod line_break_tests {
    /// Task 43 step C: after a typed line break, a word's letters are counted
    /// from its own start (cosmic-text restarts offsets on each paragraph), so
    /// syllables find the same letters on line 2 as on line 1.
    #[test]
    fn letters_after_a_typed_break_map_like_the_first_line() {
        let mut fonts = super::Fonts::load().unwrap();
        let words: Vec<String> = ["Something", "had", "something", "more"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let laid = super::layout(
            &mut fonts,
            &words,
            " ",
            &[2],
            super::Face::Sans,
            48.0,
            Some(4000.0),
            1.2,
        );
        let letters = |w: usize| {
            laid.glyphs
                .iter()
                .filter(|g| g.word == w)
                .map(|g| (g.char_in_word, g.line))
                .collect::<Vec<_>>()
        };
        let (first, second) = (letters(0), letters(2));
        assert_eq!(
            first.iter().map(|l| l.0).collect::<Vec<_>>(),
            second.iter().map(|l| l.0).collect::<Vec<_>>()
        );
        assert_eq!(
            first.iter().map(|l| l.0).collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert!(
            second.iter().all(|l| l.1 > first[0].1),
            "on a later line: {first:?} {second:?}"
        );
        // Without the break the four words share one line.
        let one = super::layout(
            &mut fonts,
            &words,
            " ",
            &[],
            super::Face::Sans,
            48.0,
            Some(4000.0),
            1.2,
        );
        assert!(one.glyphs.iter().all(|g| g.line == 0));
    }
}
