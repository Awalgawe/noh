//! The « made with NOH » signature (« Promote NOH »): a small lockup at the
//! bottom-right of an exported picture, inside its safe area.
//!
//! One description serves every renderer: exports burn it with libass (the one
//! renderer present in every FFmpeg path of NOH: `subtitles=`), the app's
//! preview paints the same numbers with egui over the picture, and the caption
//! layouts keep clear of it through `caption_band`.
//!
//! The lockup: the NOH wordmark (Jost ExtraBold, its letters at the header's
//! fixed spacing), and above it « made with », letter-spaced to the wordmark's
//! width. White at 70 %, with a soft dark halo so it reads on light and busy
//! pictures alike. The words stay English whatever the app's language: it is a
//! signature.
use std::{fs, path::Path};

/// The wordmark's face, the one the header draws with.
pub const FONT: &[u8] = include_bytes!("../assets/Jost-ExtraBold.ttf");
/// The font family name libass finds in `FONT`.
const FAMILY: &str = "Jost";
/// What a workspace gets for the mark, and the filter that draws it (FFmpeg
/// runs in the workspace). Its own folder: the captions' `fonts` folder is
/// created by them.
pub const SCRIPT: &str = "mark.ass";
pub const FONTS: &str = "markfonts";
pub const FILTER: &str = "subtitles=filename=mark.ass:fontsdir=markfonts";

/// NOH's letters (cap height) as a share of the picture's short side.
pub const CAP_SHARE: f32 = 0.022;
/// « made with » as a share of the wordmark's size.
pub const SMALL_SHARE: f32 = 0.45;
/// White at 70 %.
pub const OPACITY: f32 = 0.70;
/// Away from the safe area's corner, or from the picture's edges without one,
/// as shares of the short side.
const SAFE_INSET: f32 = 0.02;
const EDGE_INSET: f32 = 0.03;

/// The wordmark's design box (font units at size 1000), and each letter's pen
/// position on its baseline, as `app_icons::wordmark` draws it.
pub const WORDMARK: (f32, f32) = (2886.1, 770.0);
pub const LETTERS: [(&str, f32); 3] = [("N", -71.93), ("O", 1029.92), ("H", 2150.19)];
const BASELINE: f32 = 735.0;
/// Jost: cap height and descent in em.
const CAP_EM: f32 = 0.700;
const DESCENT_EM: f32 = 0.25;
/// « made with »: its natural advance in em (Jost ExtraBold), 9 characters.
const SMALL_TEXT: &str = "made with";
const SMALL_ADVANCE_EM: f32 = 4.898;

/// The lockup placed in a picture, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The wordmark's font size and box (left, top, width, height).
    pub size: f32,
    pub left: f32,
    pub width: f32,
    /// NOH's baseline: the lockup's bottom edge.
    pub baseline: f32,
    /// « made with »: font size, baseline and extra spacing between letters.
    pub small: f32,
    pub small_baseline: f32,
    pub small_spacing: f32,
    /// The top of the lockup's ink (« made with »'s cap line).
    pub top: f32,
    /// The halo: width and blur radius.
    pub halo: f32,
    pub blur: f32,
}

impl Layout {
    /// The lockup for a `width` × `height` picture with its safe insets
    /// (left, top, right, bottom), if any.
    pub fn new(width: u32, height: u32, safe: Option<[u32; 4]>) -> Self {
        let (w, h) = (width as f32, height as f32);
        let short = w.min(h);
        let cap = CAP_SHARE * short;
        let size = cap / CAP_EM;
        let unit = size / 1000.0;
        let (right, bottom) = match safe {
            Some([_, _, r, b]) => (
                w - r as f32 - SAFE_INSET * short,
                h - b as f32 - SAFE_INSET * short,
            ),
            None => (w - EDGE_INSET * short, h - EDGE_INSET * short),
        };
        let box_width = WORDMARK.0 * unit;
        let small = SMALL_SHARE * size;
        // 9 letters, 8 gaps share what the wordmark is wider by.
        let small_spacing = ((box_width - SMALL_ADVANCE_EM * small) / 8.0).max(0.0);
        let gap = 0.35 * cap;
        let small_baseline = bottom - (BASELINE / 1000.0) * size - gap;
        Self {
            size,
            left: right - box_width,
            width: box_width,
            baseline: bottom,
            small,
            small_baseline,
            small_spacing,
            top: small_baseline - CAP_EM * small,
            halo: 0.12 * cap,
            blur: 0.15 * cap,
        }
    }

    /// How far above the picture's bottom edge the lockup reaches, with one
    /// gap more: what captions keep clear of.
    pub fn band(&self, height: u32) -> u32 {
        let gap = 0.35 * CAP_EM * self.size;
        (height as f32 - self.top + gap).ceil().max(0.0) as u32
    }

    /// Each NOH letter and its pen position on the baseline.
    pub fn letters(&self) -> impl Iterator<Item = (&'static str, f32)> + '_ {
        let unit = self.size / 1000.0;
        LETTERS
            .iter()
            .map(move |&(letter, x)| (letter, self.left + x * unit))
    }

    /// The ASS script that draws the lockup for the whole video.
    pub fn script(&self, width: u32, height: u32) -> String {
        let alpha = ((1.0 - OPACITY) * 255.0).round() as u8;
        let mut out = format!(
            "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nLayoutResX: {width}\nLayoutResY: {height}\nScaledBorderAndShadow: yes\nWrapStyle: 2\nYCbCr Matrix: None\n[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\nStyle: Mark,{FAMILY},{:.2},&H{alpha:02X}FFFFFF,&H{alpha:02X}FFFFFF,&HB4000000,&HFF000000,0,0,0,0,100,100,0,0,1,{:.2},0,1,0,0,0,1\n[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n",
            self.size, self.halo
        );
        // \an1 places a line's bottom-left (its descent line) at \pos.
        let blur = format!("\\blur{:.2}", self.blur);
        for (letter, x) in self.letters() {
            out.push_str(&format!(
                "Dialogue: 0,0:00:00.00,99:59:59.99,Mark,,0,0,0,,{{\\an1\\pos({x:.1},{:.1}){blur}}}{letter}\n",
                self.baseline + DESCENT_EM * self.size
            ));
        }
        out.push_str(&format!(
            "Dialogue: 0,0:00:00.00,99:59:59.99,Mark,,0,0,0,,{{\\an1\\pos({:.1},{:.1})\\fs{:.2}\\fsp{:.2}{blur}}}{SMALL_TEXT}\n",
            self.left,
            self.small_baseline + DESCENT_EM * self.small,
            self.small,
            self.small_spacing
        ));
        out
    }
}

/// The text above the wordmark, as drawn.
pub fn small_text() -> &'static str {
    SMALL_TEXT
}

/// How far above the picture's bottom edge captions keep, so they never touch
/// the mark: the lockup's band. Styles get a safe surface that ends there (as
/// the safe area reaches them); the libass captions a bottom margin at least
/// that tall.
pub fn caption_band(width: u32, height: u32, safe: Option<[u32; 4]>) -> u32 {
    Layout::new(width, height, safe).band(height)
}

/// The warning of an export finished without its mark (`failed`).
pub const SKIPPED: &str = "warning.mark_skipped";

/// Whether an FFmpeg log shows the mark itself failing: no `subtitles` filter
/// in this FFmpeg, or libass refusing the mark's script or font. A cancel, a
/// full disk or the video's own failure never reads so.
pub fn failed(log: &str) -> bool {
    log.contains("No such filter: 'subtitles'")
        || log.contains("No such filter: \"subtitles\"")
        || log.contains(SCRIPT)
        || log.contains(FONTS)
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

/// Stage the mark's script and font in an FFmpeg workspace; `FILTER` then
/// draws it.
pub fn stage(
    workspace: &Path,
    width: u32,
    height: u32,
    safe: Option<[u32; 4]>,
) -> std::io::Result<()> {
    let script = Layout::new(width, height, safe).script(width, height);
    fs::write(workspace.join(SCRIPT), script)?;
    let fonts = workspace.join(FONTS);
    fs::create_dir_all(&fonts)?;
    fs::write(fonts.join("Jost-ExtraBold.ttf"), FONT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIKTOK: [u32; 4] = [120, 240, 300, 660];

    #[test]
    fn the_lockup_sits_in_the_safe_corner_at_its_share_of_the_short_side() {
        for (w, h, safe) in [
            (1920, 1080, None),
            (1080, 1920, Some(TIKTOK)),
            (1280, 720, None),
        ] {
            let m = Layout::new(w, h, safe);
            let short = w.min(h) as f32;
            assert!((m.size * CAP_EM - CAP_SHARE * short).abs() < 0.01);
            let [_, t, r, b] = safe.unwrap_or([0; 4]);
            // Inside the picture and its safe area, clear of the corner.
            assert!(m.left > 0.0 && m.left + m.width <= w as f32 - r as f32);
            assert!(m.top > t as f32 && m.baseline <= h as f32 - b as f32);
            // « made with » spans the wordmark's width.
            let small_width = SMALL_ADVANCE_EM * m.small + 8.0 * m.small_spacing;
            assert!(
                (small_width - m.width).abs() < 0.5,
                "{small_width} {}",
                m.width
            );
            assert!(m.small_baseline < m.baseline);
        }
    }

    #[test]
    fn captions_keep_clear_of_the_lockup() {
        for (w, h, safe) in [(1920, 1080, None), (1080, 1920, Some(TIKTOK))] {
            let m = Layout::new(w, h, safe);
            let band = caption_band(w, h, safe);
            // The captions' bottom edge is above the lockup's top, and below
            // the safe area's own bottom edge it changes nothing.
            assert!(((h - band) as f32) < m.top, "{} {}", h - band, m.top);
            assert!(band > safe.map_or(0, |s| s[3]));
        }
    }

    #[test]
    fn the_script_draws_the_three_letters_and_the_words_in_english() {
        let m = Layout::new(1920, 1080, None);
        let script = m.script(1920, 1080);
        assert!(script.contains("PlayResX: 1920\nPlayResY: 1080"));
        assert!(script.contains("Style: Mark,Jost,"));
        // 70 % opaque white: alpha 4D.
        assert!(script.contains("&H4DFFFFFF"));
        for letter in ["}N\n", "}O\n", "}H\n", "}made with\n"] {
            assert!(script.contains(letter), "{letter}");
        }
    }

    #[test]
    fn only_the_mark_s_own_failure_reads_as_the_mark_s() {
        assert!(failed("[AVFilterGraph] No such filter: 'subtitles'"));
        assert!(failed("[Parsed_subtitles_3] Unable to open mark.ass"));
        assert!(!failed("Conversion failed! No space left on device"));
        assert!(!failed("Exiting normally, received signal 2."));
        assert!(!failed("[Parsed_subtitles_1] Unable to open captions.ass"));
    }

    #[test]
    fn a_vertical_picture_keeps_the_mark_above_the_platforms_buttons() {
        use crate::safe_area::SafeArea;
        assert_eq!(
            safe_for(1080, 1920, None),
            SafeArea::Universal.insets(1080, 1920)
        );
        assert_eq!(safe_for(1920, 1080, None), None);
        assert_eq!(safe_for(1080, 1920, Some(SafeArea::Tiktok)), Some(TIKTOK));
    }

    #[test]
    fn the_font_answers_to_the_family_the_script_names() {
        // The name table's family (name ID 1, Windows platform), read without
        // a font library: libass picks the font by this name.
        let be16 = |at: usize| u16::from_be_bytes([FONT[at], FONT[at + 1]]) as usize;
        let be32 = |at: usize| {
            u32::from_be_bytes([FONT[at], FONT[at + 1], FONT[at + 2], FONT[at + 3]]) as usize
        };
        let tables = be16(4);
        let name = (0..tables)
            .map(|i| 12 + 16 * i)
            .find(|&r| &FONT[r..r + 4] == b"name")
            .map(|r| be32(r + 8))
            .expect("a name table");
        let (count, strings) = (be16(name + 2), name + be16(name + 4));
        let families: Vec<String> = (0..count)
            .map(|j| name + 6 + 12 * j)
            .filter(|&r| be16(r) == 3 && be16(r + 6) == 1)
            .map(|r| {
                let (len, off) = (be16(r + 8), strings + be16(r + 10));
                let units: Vec<u16> = FONT[off..off + len]
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            })
            .collect();
        assert!(
            families.iter().all(|f| f == FAMILY) && !families.is_empty(),
            "{families:?}"
        );
    }

    #[test]
    fn staging_writes_the_script_and_the_font_beside_the_captions() {
        let dir = tempfile::tempdir().unwrap();
        // The captions' own folder may already be there.
        fs::create_dir(dir.path().join("fonts")).unwrap();
        stage(dir.path(), 1080, 1920, Some(TIKTOK)).unwrap();
        assert!(dir.path().join(SCRIPT).is_file());
        assert_eq!(
            fs::read(dir.path().join(FONTS).join("Jost-ExtraBold.ttf")).unwrap(),
            FONT
        );
        assert!(FILTER.contains(SCRIPT) && FILTER.contains(FONTS));
    }
}
