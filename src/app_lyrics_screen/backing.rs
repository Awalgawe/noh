//! Backing vocals in the lyrics screen (voices mockup, « Chœurs »): the words
//! between parentheses are drawn dimmer and underlined in the text, sit in a
//! band of their own under the line's words in the track, and get a sync pass
//! of their own. Typing « ( » over a selection puts it between parentheses.
use super::*;
use noh::subtitle_track::{backing_flags, unbalanced_parentheses};

/// The tag a line's row shows: « chœurs » when it has backing vocals,
/// « parenthèse non fermée » when its parentheses do not balance.
pub(super) fn tag_key(text: &str) -> Option<&'static str> {
    if unbalanced_parentheses(text) {
        Some("lyrics_screen.backing_unclosed")
    } else if backing_flags(text).contains(&true) {
        Some("lyrics_screen.backing_tag")
    } else {
        None
    }
}

/// `text` laid out with its backing vocals (each group, from its first word to
/// its last) in `dim`, underlined; the rest in `color`.
pub(super) fn job(
    text: &str,
    font: FontId,
    color: Color32,
    dim: Color32,
    wrap: f32,
) -> egui::text::LayoutJob {
    let flags = backing_flags(text);
    // Byte ranges of the whitespace-separated words, in order.
    let mut spans = Vec::new();
    let mut start = None;
    for (i, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                spans.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        spans.push((s, text.len()));
    }
    let plain = egui::TextFormat::simple(font.clone(), color);
    let backing = egui::TextFormat {
        underline: Stroke::new(1.0, dim),
        ..egui::TextFormat::simple(font, dim)
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut at = 0;
    let mut k = 0;
    while k < spans.len() {
        if !flags.get(k).copied().unwrap_or(false) {
            k += 1;
            continue;
        }
        let first = k;
        while k + 1 < spans.len() && flags.get(k + 1).copied().unwrap_or(false) {
            k += 1;
        }
        let (a, b) = (spans[first].0, spans[k].1);
        if a > at {
            job.append(&text[at..a], 0.0, plain.clone());
        }
        job.append(&text[a..b], 0.0, backing.clone());
        at = b;
        k += 1;
    }
    if at < text.len() || text.is_empty() {
        job.append(&text[at..], 0.0, plain);
    }
    job
}

/// Whether the lines from `from` on hold a backing vocal (the sync bar offers
/// « Chœurs » then).
pub(super) fn has_backing(track: &SubtitleTrack, from: usize) -> bool {
    track
        .cues
        .iter()
        .skip(from)
        .any(|c| backing_flags(&c.text).contains(&true))
}

impl NohApp {
    /// « ( » typed over a selection in line `i`'s field: the selection is put
    /// between parentheses (backing vocals) instead of replaced. Returns whether
    /// the key was taken.
    pub(super) fn ls_wrap_in_parentheses(&mut self, ctx: &egui::Context, i: usize) -> bool {
        let typed = ctx.input(|input| {
            input
                .events
                .iter()
                .any(|e| matches!(e, egui::Event::Text(t) if t == "("))
        });
        if !typed {
            return false;
        }
        let id = row_id(i);
        let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
            return false;
        };
        let Some(range) = state.cursor.char_range() else {
            return false;
        };
        let (p, q): (usize, usize) = (range.primary.index.into(), range.secondary.index.into());
        let (a, b) = (p.min(q), p.max(q));
        if a == b {
            return false;
        }
        let Some(d) = self.lyrics_screen.draft.as_mut().filter(|d| d.cue == i) else {
            return false;
        };
        let chars: Vec<char> = d.text.chars().collect();
        if b > chars.len() {
            return false;
        }
        let mut text: String = chars[..a].iter().collect();
        text.push('(');
        text.extend(&chars[a..b]);
        text.push(')');
        text.extend(&chars[b..]);
        d.text = text;
        d.typed_at = Some(Instant::now());
        ctx.input_mut(|input| {
            input
                .events
                .retain(|e| !matches!(e, egui::Event::Text(t) if t == "("));
        });
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(b + 2),
            )));
        egui::TextEdit::store_state(ctx, id, state);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backing_vocals_are_a_dim_underlined_section() {
        let t = style::tokens_for(true);
        let font = FontId::proportional(14.0);
        let j = job(
            "jusqu'au matin (au matin)",
            font.clone(),
            t.text,
            t.text_3,
            300.0,
        );
        let parts: Vec<(&str, bool)> = j
            .sections
            .iter()
            .map(|s| {
                let (a, b): (usize, usize) = (s.byte_range.start.into(), s.byte_range.end.into());
                (&j.text[a..b], s.format.underline.width > 0.0)
            })
            .collect();
        assert_eq!(parts, [("jusqu'au matin ", false), ("(au matin)", true)]);
        let j = job("a (b c", font, t.text, t.text_3, 300.0);
        assert!(j.sections.iter().all(|s| s.format.underline.width == 0.0));
        assert_eq!(tag_key("a (b c"), Some("lyrics_screen.backing_unclosed"));
        assert_eq!(tag_key("a (b) c"), Some("lyrics_screen.backing_tag"));
        assert_eq!(tag_key("a b"), None);
    }
}
