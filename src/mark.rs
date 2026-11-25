//! The « made with NOH » signature (« Promote NOH »): a small closed frame
//! holding MADE WITH over the NOH wordmark, in a corner of an exported picture,
//! inside its safe area.
//!
//! One description serves every renderer. Exports burn it with libass (the one
//! renderer present in every FFmpeg path of NOH: `subtitles=`) as vector
//! drawings, so no font is involved. The app's preview paints the same numbers
//! with egui over the picture. The caption layouts keep clear of it through
//! `caption_band`.
//!
//! The look (fixed by the maintainer, 2026-11-25): the frame 5.2 % of the
//! short side wide, its stroke on whole pixels, MADE WITH exactly as wide as
//! NOH, white letters at 40 % and the stroke at 36 %, no shadow. The words stay
//! English whatever the app's language: it is a signature.
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// The NOH wordmark's outlines (from `assets/Jost-ExtraBold.ttf`, as
/// `app_icons::wordmark` places its letters): an ASS drawing in font units at
/// size 1000, y down, baseline y = 735, the box from x = 0.
const NOH_DRAWING: &str = include_str!("../assets/mark/noh.ass-drawing.txt");
/// MADE WITH's outlines: the font's capitals at size 1000, 120 units added
/// after each of the first 8 characters, baseline y = 0, first pen at x = 0.
const MADE_DRAWING: &str = include_str!("../assets/mark/made-with.ass-drawing.txt");
/// MADE WITH's ink, left edge and width, in its drawing's units.
const MADE_INK: (f32, f32) = (32.0, 7023.0);

/// What a workspace gets for the mark, and the filter that draws it (FFmpeg
/// runs in the workspace).
pub const SCRIPT: &str = "mark.ass";
pub const FILTER: &str = "subtitles=filename=mark.ass";

/// The frame's width as a share of the picture's short side (56 px at 1080).
pub const WIDTH_SHARE: f32 = 56.0 / 1080.0;
/// Opacities: the letters, the stroke.
pub const LETTERS: f32 = 0.40;
pub const STROKE: f32 = 0.36;
/// Away from the safe area's corner, or from the picture's edges without one,
/// as shares of the short side.
const SAFE_INSET: f32 = 0.02;
const EDGE_INSET: f32 = 0.03;
/// What captions keep from the frame, as a share of the short side.
const CAPTION_GAP: f32 = 0.35 * 0.022;

/// The wordmark's design box width (font units at size 1000), and each
/// letter's pen position on its baseline, as `app_icons::wordmark` draws them.
pub const WORDMARK_WIDTH: f32 = 2886.1;
pub const NOH_PENS: [(&str, f32); 3] = [("N", -71.93), ("O", 1029.92), ("H", 2150.19)];
pub const NOH_BASELINE: f32 = 735.0;
/// The wordmark's box in NOH caps: 2886.1 / 700.
const BOX_IN_CAPS: f32 = WORDMARK_WIDTH / 700.0;
/// MADE WITH: the text, and its tracking in font units (0.12 em).
pub const MADE_TEXT: &str = "MADE WITH";
pub const MADE_TRACKING: f32 = 120.0;

/// The corner the mark goes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "rmcp::schemars"))]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    /// Bottom-right, or top-left on a vertical picture (a Short's captions sit
    /// at the bottom).
    #[default]
    Auto,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// The corner itself for a `width` × `height` picture: vertical means taller
    /// than wide (a square is landscape).
    pub fn resolve(self, width: u32, height: u32) -> Self {
        match self {
            Corner::Auto if height > width => Corner::TopLeft,
            Corner::Auto => Corner::BottomRight,
            corner => corner,
        }
    }
    fn left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }
    fn top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }
}

/// The mark's colour, RGB (its opacity and stroke stay as fixed): white by
/// default. Written `#RRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

impl Default for Color {
    fn default() -> Self {
        Color::WHITE
    }
}

impl Color {
    pub const WHITE: Color = Color(255, 255, 255);
    pub const BLACK: Color = Color(0, 0, 0);

    /// `#RRGGBB` (the `#` may be left out).
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Color(byte(0)?, byte(2)?, byte(4)?))
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// ASS's colour: blue, green, red.
    fn ass(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.2, self.1, self.0)
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Color::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not a #RRGGBB colour: {text:?}")))
    }
}

#[cfg(feature = "mcp")]
impl rmcp::schemars::JsonSchema for Color {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Color".into()
    }
    fn json_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
        rmcp::schemars::json_schema!({
            "type": "string",
            "pattern": "^#?[0-9A-Fa-f]{6}$",
            "description": "An RGB colour, #RRGGBB"
        })
    }
}

/// « Promote NOH » as an export asks it: the mark's corner and colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promote {
    #[serde(default)]
    pub corner: Corner,
    #[serde(default)]
    pub color: Color,
}

/// The mark placed in a picture, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The frame's outer edges on whole pixels, and its stroke width.
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub stroke: i32,
    /// NOH's cap height, the wordmark box's left edge and its baseline.
    pub cap: f32,
    pub left: f32,
    pub baseline: f32,
    /// MADE WITH's scale (pixels per drawing unit) and baseline.
    pub made_scale: f32,
    pub made_baseline: f32,
    /// The resolved corner.
    pub corner: Corner,
}

impl Layout {
    /// The mark for a `width` × `height` picture with its safe insets (left,
    /// top, right, bottom), if any, in `corner`.
    pub fn new(width: u32, height: u32, safe: Option<[u32; 4]>, corner: Corner) -> Self {
        let (w, h) = (width as f32, height as f32);
        let short = w.min(h);
        let frame_w = (WIDTH_SHARE * short).round().max(8.0) as i32;
        let inner = BOX_IN_CAPS + 2.0 * 0.61;
        // The stroke is 0.17 caps on whole pixels: settle the two together.
        let mut stroke = (0.17 * (frame_w - 2) as f32 / inner).round().max(1.0) as i32;
        let mut cap = (frame_w - 2 * stroke) as f32 / inner;
        stroke = (0.17 * cap).round().max(1.0) as i32;
        cap = (frame_w - 2 * stroke) as f32 / inner;
        let made_scale = BOX_IN_CAPS * cap / MADE_INK.1;
        let frame_h = (2.0 * stroke as f32 + 1.92 * cap + 700.0 * made_scale + 0.47 * cap + cap)
            .round() as i32;
        let (ux0, uy0, ux1, uy1) = match safe {
            Some([l, t, r, b]) => {
                let inset = SAFE_INSET * short;
                (
                    l as f32 + inset,
                    t as f32 + inset,
                    w - r as f32 - inset,
                    h - b as f32 - inset,
                )
            }
            None => {
                let inset = EDGE_INSET * short;
                (inset, inset, w - inset, h - inset)
            }
        };
        let corner = corner.resolve(width, height);
        let x0 = if corner.left() {
            ux0.round() as i32
        } else {
            ux1.round() as i32 - frame_w
        };
        let y0 = if corner.top() {
            uy0.round() as i32
        } else {
            uy1.round() as i32 - frame_h
        };
        let (x1, y1) = (x0 + frame_w, y0 + frame_h);
        let baseline = y1 as f32 - stroke as f32 - 0.96 * cap;
        Self {
            x0,
            y0,
            x1,
            y1,
            stroke,
            cap,
            left: x0 as f32 + stroke as f32 + 0.61 * cap,
            baseline,
            made_scale,
            made_baseline: baseline - cap - 0.47 * cap,
            corner,
        }
    }

    /// Pixels per font unit of the wordmark.
    pub fn unit(&self) -> f32 {
        self.cap / 700.0
    }

    /// Each NOH letter and its pen position on the baseline.
    pub fn letters(&self) -> impl Iterator<Item = (&'static str, f32)> + '_ {
        let unit = self.unit();
        NOH_PENS
            .iter()
            .map(move |&(letter, x)| (letter, self.left + x * unit))
    }

    /// Where MADE WITH starts (its first pen) and its tracking, in pixels.
    pub fn made_pen(&self) -> (f32, f32) {
        (
            self.left - MADE_INK.0 * self.made_scale,
            MADE_TRACKING * self.made_scale,
        )
    }

    /// The edge captions keep clear of: `Bottom(band)` for a mark in a bottom
    /// corner (captions end `band` pixels above the picture's bottom edge),
    /// `Top(band)` for a top corner (they start `band` pixels below the top).
    pub fn band(&self, height: u32, short: u32) -> Band {
        let gap = CAPTION_GAP * short as f32;
        if self.corner.top() {
            Band::Top((self.y1 as f32 + gap).ceil() as u32)
        } else {
            Band::Bottom((height as f32 - self.y0 as f32 + gap).ceil().max(0.0) as u32)
        }
    }

    /// The ASS script that draws the mark for the whole video in `color`: one
    /// layer, three drawings in picture pixels (the ring, MADE WITH, NOH).
    pub fn script(&self, width: u32, height: u32, color: Color) -> String {
        let rgb = color.ass();
        let alpha = |opacity: f32| ((1.0 - opacity) * 255.0).round() as u8;
        let unit = self.unit();
        let noh = drawing(NOH_DRAWING, |x, y| {
            (
                self.left + x * unit,
                self.baseline + (y - NOH_BASELINE) * unit,
            )
        });
        let k = self.made_scale;
        let made = drawing(MADE_DRAWING, |x, y| {
            (self.left + (x - MADE_INK.0) * k, self.made_baseline + y * k)
        });
        let (x0, y0, x1, y1, s) = (self.x0, self.y0, self.x1, self.y1, self.stroke);
        // Outer edge clockwise, inner edge the other way: a ring, each pixel
        // covered once.
        let ring = format!(
            "m {x0} {y0} l {x1} {y0} l {x1} {y1} l {x0} {y1} m {} {} l {} {} l {} {} l {} {}",
            x0 + s,
            y0 + s,
            x0 + s,
            y1 - s,
            x1 - s,
            y1 - s,
            x1 - s,
            y0 + s
        );
        let tags = "\\an7\\pos(0,0)\\bord0\\shad0\\p1";
        format!(
            "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nLayoutResX: {width}\nLayoutResY: {height}\nScaledBorderAndShadow: yes\nWrapStyle: 2\nYCbCr Matrix: None\n[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\nStyle: Mark,Arial,10,&H{:02X}{rgb},&H{:02X}{rgb},&HFF000000,&HFF000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\n[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\nDialogue: 0,0:00:00.00,99:59:59.99,Mark,,0,0,0,,{{{tags}\\1a&H{:02X}&}}{ring}\nDialogue: 0,0:00:00.00,99:59:59.99,Mark,,0,0,0,,{{{tags}}}{made}\nDialogue: 0,0:00:00.00,99:59:59.99,Mark,,0,0,0,,{{{tags}}}{noh}\n",
            alpha(LETTERS),
            alpha(LETTERS),
            alpha(STROKE),
        )
    }
}

/// Which edge captions keep clear of, and how far (`Layout::band`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Top(u32),
    Bottom(u32),
}

/// An ASS drawing (m/l/b commands and coordinate pairs) with every point moved
/// by `f`.
fn drawing(source: &str, f: impl Fn(f32, f32) -> (f32, f32)) -> String {
    let mut out = String::with_capacity(source.len() + source.len() / 2);
    let mut pending: Option<f32> = None;
    for token in source.split_whitespace() {
        match token {
            "m" | "l" | "b" => {
                out.push_str(token);
                out.push(' ');
            }
            number => {
                let v: f32 = number.parse().unwrap_or(0.0);
                match pending.take() {
                    None => pending = Some(v),
                    Some(x) => {
                        let (px, py) = f(x, v);
                        out.push_str(&format!("{px:.2} {py:.2} "));
                    }
                }
            }
        }
    }
    out.trim_end().to_owned()
}

/// The edge captions keep clear of for a picture with the mark in `corner`.
pub fn caption_band(width: u32, height: u32, safe: Option<[u32; 4]>, corner: Corner) -> Band {
    Layout::new(width, height, safe, corner).band(height, width.min(height))
}

/// The warning of an export finished without its mark (`failed`).
pub const SKIPPED: &str = "warning.mark_skipped";

/// Whether an FFmpeg log shows the mark itself failing: no `subtitles` filter
/// in this FFmpeg, or libass unable to open the mark's script. Only error
/// lines count: a filter's arguments, echoed at some log levels, never do.
pub fn failed(log: &str) -> bool {
    log.lines().any(|line| {
        line.contains("No such filter: 'subtitles'")
            || line.contains("No such filter: \"subtitles\"")
            || (line.contains(SCRIPT)
                && (line.contains("Unable to open") || line.contains("Error opening")))
    })
}

/// The safe insets the mark keeps inside: the captions' own safe area when
/// they have one, the most careful platform area for a vertical picture
/// without captions (the platforms' buttons are there all the same), none for
/// a landscape one.
pub fn safe_for(
    width: u32,
    height: u32,
    captions: Option<crate::safe_area::SafeArea>,
) -> Option<[u32; 4]> {
    captions
        .and_then(|area| area.insets(width, height))
        .or_else(|| {
            (height > width)
                .then(|| crate::safe_area::SafeArea::Universal.insets(width, height))
                .flatten()
        })
}

/// Stage the mark's script in an FFmpeg workspace; `FILTER` then draws it.
pub fn stage(
    workspace: &Path,
    width: u32,
    height: u32,
    safe: Option<[u32; 4]>,
    promote: Promote,
) -> std::io::Result<()> {
    let script =
        Layout::new(width, height, safe, promote.corner).script(width, height, promote.color);
    fs::write(workspace.join(SCRIPT), script)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIVERSAL: [u32; 4] = [120, 288, 300, 672];

    #[test]
    fn the_frame_is_the_maintainer_s_size_on_whole_pixels() {
        let m = Layout::new(1920, 1080, None, Corner::Auto);
        assert_eq!((m.x1 - m.x0, m.y1 - m.y0, m.stroke), (56, 41, 2));
        assert!((m.cap - 9.73).abs() < 0.01, "{}", m.cap);
        // 4K: the same share.
        let big = Layout::new(3840, 2160, None, Corner::Auto);
        assert_eq!(big.x1 - big.x0, 112);
    }

    #[test]
    fn auto_is_bottom_right_or_top_left_on_a_vertical_picture() {
        let wide = Layout::new(1920, 1080, None, Corner::Auto);
        assert_eq!(wide.corner, Corner::BottomRight);
        // 3 % of the short side from the edges.
        assert_eq!((wide.x1, wide.y1), (1920 - 32, 1080 - 32));
        let tall = Layout::new(1080, 1920, Some(UNIVERSAL), Corner::Auto);
        assert_eq!(tall.corner, Corner::TopLeft);
        // Inside the safe area, 2 % of the short side in.
        assert_eq!((tall.x0, tall.y0), (142, 310));
        // A square is landscape.
        assert_eq!(Corner::Auto.resolve(1080, 1080), Corner::BottomRight);
    }

    #[test]
    fn the_words_fit_inside_the_frame() {
        for corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            let m = Layout::new(1080, 1920, Some(UNIVERSAL), corner);
            let inner = (m.x0 + m.stroke) as f32;
            let right = (m.x1 - m.stroke) as f32;
            assert!(m.left > inner && m.left + BOX_IN_CAPS * m.cap < right);
            assert!(m.made_baseline - 700.0 * m.made_scale > (m.y0 + m.stroke) as f32);
            assert!(m.baseline < (m.y1 - m.stroke) as f32);
            // MADE WITH is exactly NOH's width.
            assert!((MADE_INK.1 * m.made_scale - BOX_IN_CAPS * m.cap).abs() < 0.01);
        }
    }

    #[test]
    fn captions_keep_clear_on_the_mark_s_side() {
        let wide = Layout::new(1920, 1080, None, Corner::Auto);
        match caption_band(1920, 1080, None, Corner::Auto) {
            Band::Bottom(b) => assert!(((1080 - b) as i32) < wide.y0),
            other => panic!("{other:?}"),
        }
        let tall = Layout::new(1080, 1920, Some(UNIVERSAL), Corner::Auto);
        match caption_band(1080, 1920, Some(UNIVERSAL), Corner::Auto) {
            Band::Top(b) => assert!(b as i32 > tall.y1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_script_draws_the_ring_and_both_words_as_drawings() {
        let m = Layout::new(1920, 1080, None, Corner::Auto);
        let script = m.script(1920, 1080, Color::WHITE);
        assert!(script.contains("PlayResX: 1920\nPlayResY: 1080"));
        assert_eq!(script.matches("\\p1").count(), 3);
        assert!(script.contains("\\bord0\\shad0"));
        // 40 % letters (alpha 99), 36 % stroke (alpha A3).
        assert!(script.contains("&H99FFFFFF") && script.contains("\\1a&HA3&"));
        // No font: every event is a drawing.
        assert!(!script.contains("}MADE") && !script.contains("}NOH"));
        assert!(!FILTER.contains("fontsdir"));
    }

    #[test]
    fn the_colour_is_any_rgb_in_hex_and_reaches_the_script() {
        assert_eq!(Color::parse("#1A2b3C"), Some(Color(0x1A, 0x2B, 0x3C)));
        assert_eq!(Color::parse("ffffff"), Some(Color::WHITE));
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("#GG0000"), None);
        assert_eq!(Color(0x1A, 0x2B, 0x3C).hex(), "#1A2B3C");
        let script =
            Layout::new(1920, 1080, None, Corner::Auto).script(1920, 1080, Color(0x11, 0x22, 0x33));
        // ASS writes blue, green, red; the opacities stay.
        assert!(script.contains("&H99332211"), "{script}");
        let promote: Promote = serde_json::from_str(r##"{"color":"#000000"}"##).unwrap();
        assert_eq!(
            promote,
            Promote {
                corner: Corner::Auto,
                color: Color::BLACK
            }
        );
        assert!(serde_json::from_str::<Promote>(r#"{"color":"black"}"#).is_err());
    }

    #[test]
    fn only_the_mark_s_own_failure_reads_as_the_mark_s() {
        assert!(failed("[AVFilterGraph @ 0x1] No such filter: 'subtitles'"));
        assert!(failed("[Parsed_subtitles_3 @ 0x1] Unable to open mark.ass"));
        assert!(!failed("Conversion failed! No space left on device"));
        assert!(!failed("Exiting normally, received signal 2."));
        // A verbose log echoes the filter's arguments: not a failure of the mark.
        let verbose = "[Parsed_subtitles_2 @ 0x1] Setting 'filename' to value 'mark.ass'\n\
                       [out#0/mp4 @ 0x2] Error opening output export.mp4: No such file or directory";
        assert!(!failed(verbose));
    }

    #[test]
    fn a_vertical_picture_keeps_the_mark_inside_the_platforms_area() {
        use crate::safe_area::SafeArea;
        assert_eq!(
            safe_for(1080, 1920, None),
            SafeArea::Universal.insets(1080, 1920)
        );
        assert_eq!(safe_for(1920, 1080, None), None);
        assert_eq!(
            safe_for(1080, 1920, Some(SafeArea::Tiktok)),
            SafeArea::Tiktok.insets(1080, 1920)
        );
    }

    /// The drawings are the font's own glyphs: their boxes match the font's
    /// glyph boxes at the pen positions the preview uses (within one unit).
    #[cfg(feature = "effects")]
    #[test]
    fn the_drawings_are_the_font_s_letters_where_the_preview_draws_them() {
        let face =
            ttf_parser::Face::parse(include_bytes!("../assets/Jost-ExtraBold.ttf"), 0).unwrap();
        let bounds = |source: &str| {
            let numbers: Vec<f32> = source
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            let (xs, ys): (Vec<f32>, Vec<f32>) = numbers.chunks(2).map(|p| (p[0], p[1])).unzip();
            let fold = |v: &[f32], f: fn(f32, f32) -> f32, s: f32| v.iter().copied().fold(s, f);
            (
                fold(&xs, f32::min, f32::MAX),
                fold(&ys, f32::min, f32::MAX),
                fold(&xs, f32::max, f32::MIN),
                fold(&ys, f32::max, f32::MIN),
            )
        };
        let glyphs = |text: &str, pens: &[f32], baseline: f32| {
            let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (c, pen) in text.chars().zip(pens) {
                let id = face.glyph_index(c).unwrap();
                if let Some(r) = face.glyph_bounding_box(id) {
                    b.0 = b.0.min(pen + r.x_min as f32);
                    b.2 = b.2.max(pen + r.x_max as f32);
                    b.1 = b.1.min(baseline - r.y_max as f32);
                    b.3 = b.3.max(baseline - r.y_min as f32);
                }
            }
            b
        };
        let close = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)]
                .iter()
                .all(|(x, y)| (x - y).abs() <= 1.0)
        };
        // NOH at the header's pens: the drawing's frame is the wordmark's box,
        // where the N's pen is at -71.93.
        let pens: Vec<f32> = NOH_PENS.iter().map(|p| p.1).collect();
        let font = glyphs("NOH", &pens, NOH_BASELINE);
        let drawn = bounds(NOH_DRAWING);
        assert!(close(font, drawn), "{font:?} {drawn:?}");
        // MADE WITH: each pen after the previous advance and the tracking.
        let mut x = 0.0;
        let mut pens = Vec::new();
        for c in MADE_TEXT.chars() {
            pens.push(x);
            let id = face.glyph_index(c).unwrap();
            x += face.glyph_hor_advance(id).unwrap() as f32 + MADE_TRACKING;
        }
        let font = glyphs(MADE_TEXT, &pens, 0.0);
        let drawn = bounds(MADE_DRAWING);
        assert!(close(font, drawn), "{font:?} {drawn:?}");
    }
}
