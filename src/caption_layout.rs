//! Plain caption layout for the bundled NOH CJK font, without shaping tables.
//!
//! Nominal advances and line metrics are rounded upward to whole pixels. Callers
//! must reserve outline, rasterization and renderer-specific margins themselves;
//! these metrics are not a guarantee of identical libass shaping or ink bounds.
//! Wrapping preserves text and whitespace, explicit blank lines and graphemes.
//! CRLF becomes LF; each tab is rendered as four spaces, not a variable tab stop.
use crate::subtitle_track::MAX_CUE_TEXT_BYTES;
use skrifa::{
    FontRef, MetadataProvider,
    charmap::Charmap,
    instance::{LocationRef, Size},
    metrics::GlyphMetrics,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LayoutError {
    #[error("Caption {field} must be finite and positive")]
    InvalidDimensions { field: &'static str },
    #[error("Caption text exceeds {} UTF-8 bytes", MAX_CUE_TEXT_BYTES)]
    TextLimit,
    #[error("Caption contains unsupported character U+{codepoint:04X}")]
    UnsupportedCharacter { codepoint: u32 },
    #[error("Caption contains disallowed control U+{codepoint:04X}")]
    ControlCharacter { codepoint: u32 },
    #[error("The bundled caption font has invalid or unavailable metrics")]
    FontMetrics,
    #[error("Caption grapheme needs {required_width} pixels but only {max_width} are available")]
    GraphemeTooWide { required_width: f32, max_width: f32 },
    #[error(
        "Caption block has {lines} lines and needs {required_height} pixels but only {max_height} are available"
    )]
    BlockTooTall {
        lines: usize,
        required_height: f32,
        max_height: f32,
    },
}

/// Wrap without silently shrinking, truncating, reordering or dropping text.
/// Font size is pixels per em; width/height are the caller's usable text region.
pub fn layout_text(
    text: &str,
    font_size: f32,
    max_width: f32,
    max_height: f32,
) -> Result<Vec<String>, LayoutError> {
    positive(max_width, "width")?;
    positive(max_height, "height")?;
    let metrics = FontMetrics::new(font_size)?;
    metrics.validate_text(text, true)?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let normalized = text.replace("\r\n", "\n").replace('\t', "    ");
    let mut lines = Vec::new();
    for paragraph in normalized.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let word_ends: Vec<_> = paragraph
            .split_word_bound_indices()
            .map(|(index, word)| index + word.len())
            .collect();
        let graphemes: Vec<_> = paragraph
            .grapheme_indices(true)
            .map(|(index, text)| {
                Ok((
                    index + text.len(),
                    metrics.width(text)?,
                    text.chars().all(char::is_whitespace),
                ))
            })
            .collect::<Result<_, LayoutError>>()?;
        let mut start = 0;
        while start < graphemes.len() {
            let mut end = start;
            let mut width = 0.0;
            let (mut whitespace_break, mut word_break) = (None, None);
            while end < graphemes.len() {
                let next_width = width + graphemes[end].1;
                if next_width.ceil() > f64::from(max_width) {
                    break;
                }
                width = next_width;
                end += 1;
                if graphemes[end - 1].2 {
                    whitespace_break = Some(end);
                }
                if word_ends.binary_search(&graphemes[end - 1].0).is_ok() {
                    word_break = Some(end);
                }
            }
            if end == start {
                return Err(LayoutError::GraphemeTooWide {
                    required_width: graphemes[start].1.ceil() as f32,
                    max_width,
                });
            }
            if end < graphemes.len() {
                end = whitespace_break.or(word_break).unwrap_or(end);
            }
            let byte_start = if start == 0 {
                0
            } else {
                graphemes[start - 1].0
            };
            lines.push(paragraph[byte_start..graphemes[end - 1].0].to_owned());
            start = end;
        }
    }
    let required_height = f64::from(metrics.line_height) * lines.len() as f64;
    if required_height > f64::from(max_height) {
        return Err(LayoutError::BlockTooTall {
            lines: lines.len(),
            required_height: required_height as f32,
            max_height,
        });
    }
    Ok(lines)
}

/// Conservative whole-pixel baseline spacing from ascent, descent and leading.
#[cfg(test)]
pub fn line_height(font_size: f32) -> Result<f32, LayoutError> {
    Ok(FontMetrics::new(font_size)?.line_height)
}

/// Whole-pixel nominal advance width for one line, including four-space tabs.
#[cfg(test)]
pub fn measure_line(text: &str, font_size: f32) -> Result<f32, LayoutError> {
    let metrics = FontMetrics::new(font_size)?;
    metrics.validate_text(text, false)?;
    Ok(metrics.width(&text.replace('\t', "    "))?.ceil() as f32)
}

struct FontMetrics {
    charmap: Charmap<'static>,
    glyphs: GlyphMetrics<'static>,
    scale: f64,
    line_height: f32,
}
impl FontMetrics {
    fn new(font_size: f32) -> Result<Self, LayoutError> {
        positive(font_size, "font size")?;
        let font = FontRef::new(include_bytes!("../assets/NOHCJK.otf"))
            .map_err(|_| LayoutError::FontMetrics)?;
        // Unscaled advances avoid the fixed-point size saturation of glyph_metrics.
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        if metrics.units_per_em == 0 {
            return Err(LayoutError::FontMetrics);
        }
        let scale = f64::from(font_size) / f64::from(metrics.units_per_em);
        let height = (f64::from(metrics.ascent) - f64::from(metrics.descent)
            + f64::from(metrics.leading.max(0.0)))
            * scale;
        let line_height = height.ceil() as f32;
        if !line_height.is_finite() || line_height <= 0.0 {
            return Err(LayoutError::FontMetrics);
        }
        Ok(Self {
            charmap: font.charmap(),
            glyphs: font.glyph_metrics(Size::unscaled(), LocationRef::default()),
            scale,
            line_height,
        })
    }
    fn validate_text(&self, text: &str, multiline: bool) -> Result<(), LayoutError> {
        if text.len() > MAX_CUE_TEXT_BYTES {
            return Err(LayoutError::TextLimit);
        }
        let mut characters = text.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\t' || (multiline && character == '\n') {
                continue;
            }
            if multiline && character == '\r' && characters.peek() == Some(&'\n') {
                continue;
            }
            if character.is_control() {
                return Err(LayoutError::ControlCharacter {
                    codepoint: u32::from(character),
                });
            }
            self.advance(character)?;
        }
        Ok(())
    }
    fn advance(&self, character: char) -> Result<f64, LayoutError> {
        let glyph = self
            .charmap
            .map(character)
            .filter(|glyph| glyph.to_u32() != 0)
            .ok_or(LayoutError::UnsupportedCharacter {
                codepoint: u32::from(character),
            })?;
        let advance = self
            .glyphs
            .advance_width(glyph)
            .ok_or(LayoutError::FontMetrics)?;
        if !advance.is_finite() || advance < 0.0 {
            return Err(LayoutError::FontMetrics);
        }
        Ok(f64::from(advance) * self.scale)
    }
    fn width(&self, text: &str) -> Result<f64, LayoutError> {
        text.chars().try_fold(0.0, |width, character| {
            let width = width + self.advance(character)?;
            if !width.is_finite() || width > f64::from(f32::MAX) {
                return Err(LayoutError::FontMetrics);
            }
            Ok(width)
        })
    }
}
fn positive(value: f32, field: &'static str) -> Result<(), LayoutError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(LayoutError::InvalidDimensions { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_german_japanese_korean_and_chinese_fit_without_losing_text() {
        for text in [
            "Écoutez cette phrase française.",
            "Grüße aus Deutschland: Straße und Größe.",
            "字幕の文章を読みやすく表示します。",
            "한국어 자막을 읽기 쉽게 표시합니다.",
            "中文字幕应该完整并且易于阅读。",
        ] {
            let lines = layout_text(text, 32.0, 260.0, 1000.0).unwrap();
            assert_eq!(lines.concat(), text);
            assert!(
                lines
                    .iter()
                    .all(|line| measure_line(line, 32.0).unwrap() <= 260.0)
            );
        }
    }

    #[test]
    fn long_latin_tokens_wrap_only_between_complete_graphemes() {
        let text = "e\u{301}".repeat(50);
        let lines = layout_text(&text, 32.0, 100.0, 2000.0).unwrap();
        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), text);
        for line in lines {
            assert!(!line.starts_with('\u{301}'));
            assert!(line.ends_with('\u{301}'));
            assert!(measure_line(&line, 32.0).unwrap() <= 100.0);
        }
        let token = "Supercalifragilisticexpialidocious";
        let lines = layout_text(token, 32.0, 90.0, 2000.0).unwrap();
        assert_eq!(lines.concat(), token);
        assert!(
            lines
                .iter()
                .all(|line| measure_line(line, 32.0).unwrap() <= 90.0)
        );
    }

    #[test]
    fn whitespace_word_boundaries_explicit_newlines_and_tabs_are_preserved() {
        let text = "Alpha beta gamma";
        let width = measure_line("Alpha beta ", 32.0).unwrap();
        assert_eq!(
            layout_text(text, 32.0, width, 1000.0).unwrap(),
            ["Alpha beta ", "gamma"]
        );
        assert_eq!(
            layout_text("First\tline\r\n\nLast\n", 24.0, 1000.0, 1000.0).unwrap(),
            ["First    line", "", "Last", ""]
        );
        assert!(layout_text("", 24.0, 100.0, 100.0).unwrap().is_empty());
    }

    #[test]
    fn unsupported_glyphs_and_controls_are_actionable_errors() {
        assert_eq!(
            layout_text("Unsupported \u{10ffff}", 32.0, 500.0, 500.0).unwrap_err(),
            LayoutError::UnsupportedCharacter {
                codepoint: 0x10ffff
            }
        );
        for text in ["Lone\rCR", "NUL\0", "DEL\u{7f}"] {
            assert!(matches!(
                layout_text(text, 32.0, 500.0, 500.0),
                Err(LayoutError::ControlCharacter { .. })
            ));
        }
        let width = measure_line("字", 32.0).unwrap();
        assert!(matches!(
            layout_text("字", 32.0, width - 1.0, 500.0),
            Err(LayoutError::GraphemeTooWide { .. })
        ));
    }

    #[test]
    fn dimensions_and_original_utf8_byte_limit_validate_before_layout() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                layout_text("Text", invalid, 500.0, 500.0),
                Err(LayoutError::InvalidDimensions { .. })
            ));
            assert!(matches!(
                layout_text("Text", 32.0, invalid, 500.0),
                Err(LayoutError::InvalidDimensions { .. })
            ));
            assert!(matches!(
                layout_text("Text", 32.0, 500.0, invalid),
                Err(LayoutError::InvalidDimensions { .. })
            ));
        }
        let text = "é".repeat(MAX_CUE_TEXT_BYTES / 2);
        assert!(layout_text(&text, 1.0, 100000.0, 1000.0).is_ok());
        assert_eq!(
            layout_text(&format!("{text}x"), 1.0, 100000.0, 1000.0).unwrap_err(),
            LayoutError::TextLimit
        );
    }

    #[test]
    fn exact_height_fits_but_excessive_block_is_rejected_without_shrinking() {
        let height = line_height(32.0).unwrap();
        assert_eq!(
            layout_text("First\nSecond", 32.0, 500.0, height * 2.0).unwrap(),
            ["First", "Second"]
        );
        assert_eq!(
            layout_text("First\nSecond", 32.0, 500.0, height * 2.0 - 1.0).unwrap_err(),
            LayoutError::BlockTooTall {
                lines: 2,
                required_height: height * 2.0,
                max_height: height * 2.0 - 1.0
            }
        );
    }
}
