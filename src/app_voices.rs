//! Who sings a lyrics line, as the editor shows it: the badge before a line in
//! the lyrics screen, the colour band on a line's block in both tracks, and
//! the names. The colours are the editor's own (`voice` tokens), always with the
//! voice's number: the video's colours belong to the caption style.
use crate::app_style::{self as style, Tokens};
use eframe::egui::{self, Align2, Color32, FontId, Painter, Rect, Stroke, pos2};
use noh::{i18n::Language, subtitle_track::SubtitleTrack};

/// A badge's size in the lyrics screen's text list.
pub const BADGE_W: f32 = 34.0;
pub const BADGE_H: f32 = 22.0;

/// The editor colour of voice `n` (1-based).
pub fn color(t: &Tokens, n: u8) -> Color32 {
    t.voice[(n.max(1) as usize - 1) % t.voice.len()]
}

/// « Voix 2 », or the name given to voice `n`.
pub fn name(track: &SubtitleTrack, n: u8, l: Language) -> String {
    track
        .voices
        .get(n as usize - 1)
        .and_then(|v| v.name.clone())
        .unwrap_or_else(|| l.format("voices.numbered", &[n.to_string()]))
}

/// What a set of voices is called: « Aucune », a name, « Ensemble » for voices
/// 1 and 2 together, or the names joined.
pub fn set_name(track: &SubtitleTrack, set: &[u8], l: Language) -> String {
    match set {
        [] => l.text("voices.none").to_string(),
        [n] => name(track, *n, l),
        [1, 2] => l.text("voices.together").to_string(),
        many => many
            .iter()
            .map(|&n| name(track, n, l))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The badge of a set of voices in `rect`: an outline with a dash when no voice
/// is known, otherwise one coloured part per voice with its number.
pub fn paint_badge(painter: &Painter, rect: Rect, set: &[u8], t: &Tokens, hovered: bool) {
    let radius = (rect.height() / 2.0) as u8;
    if set.is_empty() {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, if hovered { t.text_2 } else { t.border_strong }),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "–",
            FontId::proportional(style::text::SMALL),
            if hovered { t.text } else { t.text_3 },
        );
        return;
    }
    let inside = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let part = rect.width() / set.len() as f32;
    // One rounded pill, cut into parts: each part is a plain rectangle clipped
    // by the pill's shape (the first and last keep the round ends).
    for (k, &n) in set.iter().enumerate() {
        let x0 = rect.left() + part * k as f32;
        let cell = Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + part, rect.bottom()));
        let r = egui::CornerRadius {
            nw: if k == 0 { radius } else { 0 },
            sw: if k == 0 { radius } else { 0 },
            ne: if k + 1 == set.len() { radius } else { 0 },
            se: if k + 1 == set.len() { radius } else { 0 },
        };
        inside.rect_filled(cell, r, color(t, n));
        inside.text(
            cell.center(),
            Align2::CENTER_CENTER,
            n.to_string(),
            style::semibold(style::text::SMALL),
            t.on_voice,
        );
    }
    if hovered {
        painter.rect_stroke(
            rect.expand(1.0),
            radius + 1,
            Stroke::new(1.0, t.text_2),
            egui::StrokeKind::Outside,
        );
    }
}

/// The colour band along the top of a line's block: one part per voice.
pub fn paint_band(painter: &Painter, block: Rect, set: &[u8], t: &Tokens, height: f32) {
    if set.is_empty() {
        return;
    }
    let band = Rect::from_min_max(block.min, pos2(block.right(), block.top() + height));
    let part = band.width() / set.len() as f32;
    let clip = painter.with_clip_rect(band.intersect(painter.clip_rect()));
    for (k, &n) in set.iter().enumerate() {
        let x0 = band.left() + part * k as f32;
        clip.rect_filled(
            Rect::from_min_max(pos2(x0, band.top()), pos2(x0 + part, band.bottom())),
            egui::CornerRadius {
                nw: if k == 0 { 4 } else { 0 },
                ne: if k + 1 == set.len() { 4 } else { 0 },
                sw: 0,
                se: 0,
            },
            color(t, n),
        );
    }
}

/// « 1+2 »: the numbers of a set, for the small label on a block.
pub fn numbers(set: &[u8]) -> String {
    set.iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::subtitle_track::{SubtitleCue, Voice};

    fn track() -> SubtitleTrack {
        SubtitleTrack {
            voices: vec![
                Voice {
                    name: Some("Léa".into()),
                },
                Voice::default(),
                Voice::default(),
            ],
            language: None,
            duration_ms: 1000,
            cues: vec![SubtitleCue::new(0, 1000, "a")],
        }
    }

    #[test]
    fn a_set_is_named_by_its_voices() {
        let l = Language::Fr;
        let t = track();
        assert_eq!(set_name(&t, &[], l), "Aucune");
        assert_eq!(set_name(&t, &[1], l), "Léa");
        assert_eq!(set_name(&t, &[2], l), "Voix 2");
        assert_eq!(set_name(&t, &[1, 2], l), "Ensemble");
        assert_eq!(set_name(&t, &[1, 3], l), "Léa, Voix 3");
        assert_eq!(numbers(&[1, 2]), "1+2");
    }

    #[test]
    fn every_voice_number_reads_on_its_colour() {
        for t in [style::tokens_for(true), style::tokens_for(false)] {
            for n in 1..=4u8 {
                let ratio = contrast(color(t, n), t.on_voice);
                assert!(ratio >= 4.5, "voice {n}: {ratio:.2}");
            }
        }
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let f = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
}
