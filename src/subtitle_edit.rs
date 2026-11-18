//! Edits of a subtitle track from the lyrics lane (fused mockup v9, Task 41 S3):
//! pure functions, no IO. Every edit keeps the track valid and follows the word
//! rules of the mockup's note: a text edit keeps unchanged words' times; moving a
//! cue moves its words; an edge stops at its neighbour (and at hand-placed
//! words), and words it leaves outside are spread by letters ("estimated").
use crate::subtitle_track::{
    MIN_WORD_GAP_MS, SubtitleCue, SubtitleTrack, SyllableTime, WordSource, WordTime,
};

/// The shortest cue an edge drag leaves.
pub const MIN_CUE_MS: u64 = 200;
/// The smallest gap where "+" offers a new cue.
pub const MIN_ADD_GAP_MS: u64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

fn letters(w: &str) -> u64 {
    w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as u64
}

/// Starts for `words` spread by letter count over [a, b), as the engine does
/// without word times (fx/line.rs split_words), on whole milliseconds.
fn spread(words: &[&str], a: u64, b: u64) -> Vec<u64> {
    let total: u64 = words.iter().map(|w| letters(w)).sum::<u64>().max(1);
    let mut acc = 0;
    words
        .iter()
        .map(|w| {
            let t = a + (b.saturating_sub(a)) * acc / total;
            acc += letters(w);
            t
        })
        .collect()
}

/// Starts for `words` between `a` and `next` (exclusive): spread by letters,
/// then kept 50 ms apart and at least 50 ms before `next` (the following word,
/// or the cue end). None when they cannot fit.
fn spread_before(words: &[&str], a: u64, next: u64) -> Option<Vec<u64>> {
    let n = words.len() as u64;
    if n == 0 {
        return Some(Vec::new());
    }
    let last = next.checked_sub(MIN_WORD_GAP_MS * n)?;
    if last < a {
        return None;
    }
    let mut t = spread(words, a, next);
    for i in 1..t.len() {
        t[i] = t[i].max(t[i - 1] + MIN_WORD_GAP_MS);
    }
    for (i, v) in t.iter_mut().enumerate().rev() {
        *v = (*v).min(next - MIN_WORD_GAP_MS * (n - i as u64));
    }
    (t[0] >= a).then_some(t)
}

/// Make a word list fit its cue: starts at least 50 ms apart and inside the cue
/// (a moved start becomes Estimated and loses its known end); known ends are
/// clamped to the next start. None when the cue cannot hold them.
fn settle(cue: &SubtitleCue, mut words: Vec<WordTime>) -> Option<Vec<WordTime>> {
    let mut previous: Option<u64> = None;
    for w in &mut words {
        let low = previous.map_or(cue.start_ms, |p| p + MIN_WORD_GAP_MS);
        if w.start_ms < low {
            w.start_ms = low;
            w.end_ms = None;
            w.source = WordSource::Estimated;
            w.syllables = None;
        }
        previous = Some(
            w.syllables
                .as_ref()
                .and_then(|s| s.last())
                .map_or(w.start_ms, |s| s.start_ms.max(w.start_ms)),
        );
    }
    cue.fit_ends(&mut words);
    cue.words_fit(&words).then_some(words)
}

/// Words for `new_text` after a text edit: words kept in order (longest common
/// subsequence of the texts) keep their time, source and syllables; new or
/// changed words are spread by letters between their kept neighbours.
pub fn remap_words(cue: &SubtitleCue, old: &[WordTime], new_text: &str) -> Option<Vec<WordTime>> {
    let new: Vec<&str> = new_text.split_whitespace().collect();
    if new.is_empty() {
        return None;
    }
    let (n, m) = (old.len(), new.len());
    let mut l = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if old[i].text == new[j] {
                l[i + 1][j + 1] + 1
            } else {
                l[i + 1][j].max(l[i][j + 1])
            };
        }
    }
    let mut keep = vec![None; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i].text == new[j] {
            keep[j] = Some(i);
            i += 1;
            j += 1;
        } else if l[i + 1][j] >= l[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut out: Vec<WordTime> = Vec::with_capacity(m);
    let mut a = 0;
    while a < m {
        if let Some(k) = keep[a] {
            out.push(old[k].clone());
            a += 1;
            continue;
        }
        let mut e = a;
        while e < m && keep[e].is_none() {
            e += 1;
        }
        // Between the previous word (50 ms after it) and the next kept word, or
        // the last start the cue allows.
        let hi = if e < m {
            old[keep[e].unwrap()].start_ms
        } else {
            cue.end_ms.saturating_sub(MIN_WORD_GAP_MS)
        };
        let lo = out
            .last()
            .map_or(cue.start_ms, |w| w.start_ms + MIN_WORD_GAP_MS);
        for (x, t) in spread(&new[a..e], lo, hi).into_iter().enumerate() {
            out.push(WordTime {
                text: new[a + x].into(),
                start_ms: t,
                end_ms: None,
                source: WordSource::Estimated,
                syllables: None,
            });
        }
        a = e;
    }
    let mut probe = cue.clone();
    probe.text = new_text.into();
    settle(&probe, out)
}

/// Set a cue's text (a trimmed, non-empty text; the caller checks it parses as
/// subtitle text). Word times follow `remap_words`. Returns false when refused.
pub fn set_text(track: &mut SubtitleTrack, i: usize, text: &str) -> bool {
    let Some(cue) = track.cues.get(i) else {
        return false;
    };
    let mut next = cue.clone();
    next.text = text.trim().replace("\r\n", "\n");
    next.words = cue
        .words
        .as_ref()
        .and_then(|w| remap_words(&next, w, &next.text));
    let mut probe = track.clone();
    probe.cues[i] = next;
    if probe.validate().is_err() {
        return false;
    }
    *track = probe;
    true
}

/// Where an edge may go: between the neighbours, at least MIN_CUE_MS long, and
/// never past a hand-placed word (the end stays 50 ms after the last one).
pub fn edge_bounds(track: &SubtitleTrack, i: usize, edge: Edge) -> (u64, u64) {
    let cue = &track.cues[i];
    let words = cue.words.as_deref().unwrap_or(&[]);
    // A hand-placed word never moves because of an edge: the edge keeps room
    // for the words before (or after) it at 50 ms each.
    let first_hand = words.iter().position(|w| w.source == WordSource::Hand);
    let last_hand = words.iter().rposition(|w| w.source == WordSource::Hand);
    match edge {
        Edge::Start => {
            let low = i.checked_sub(1).map_or(0, |p| track.cues[p].end_ms);
            let hand = first_hand.map_or(u64::MAX, |f| {
                words[f].start_ms.saturating_sub(MIN_WORD_GAP_MS * f as u64)
            });
            let high = cue.end_ms.saturating_sub(MIN_CUE_MS).min(hand);
            (low, high.max(low))
        }
        Edge::End => {
            let high = track
                .cues
                .get(i + 1)
                .map_or(track.duration_ms, |n| n.start_ms);
            let hand = last_hand.map_or(0, |l| {
                words[l].start_ms + MIN_WORD_GAP_MS * (words.len() - l) as u64
            });
            let low = (cue.start_ms + MIN_CUE_MS).max(hand);
            (low.min(high), high)
        }
    }
}

/// Move an edge to `ms` (clamped by `edge_bounds`). Words still inside keep their
/// time; words left outside are spread by letters between the edge and the
/// nearest kept word. Returns the edge's new position.
pub fn set_edge(track: &mut SubtitleTrack, i: usize, edge: Edge, ms: u64) -> u64 {
    let (lo, hi) = edge_bounds(track, i, edge);
    let ms = ms.clamp(lo, hi);
    let cue = &mut track.cues[i];
    match edge {
        Edge::Start => cue.start_ms = ms,
        Edge::End => cue.end_ms = ms,
    }
    if let Some(words) = cue.words.take() {
        let last_ok = cue.end_ms.saturating_sub(MIN_WORD_GAP_MS);
        let inside = |w: &WordTime| w.start_ms >= cue.start_ms && w.start_ms <= last_ok;
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let mut out = words.clone();
        // Leading words before the new start: spread between it and the first kept word.
        let first_in = words.iter().position(inside);
        let last_in = words.iter().rposition(inside);
        let estimated = |out: &mut Vec<WordTime>, from: usize, starts: Option<Vec<u64>>| {
            starts.map(|starts| {
                for (k, t) in starts.into_iter().enumerate() {
                    out[from + k] = WordTime {
                        start_ms: t,
                        end_ms: None,
                        source: WordSource::Estimated,
                        syllables: None,
                        ..words[from + k].clone()
                    };
                }
            })
        };
        let placed = match (first_in, last_in) {
            (Some(f), Some(l)) => {
                let lead = estimated(
                    &mut out,
                    0,
                    spread_before(&texts[..f], cue.start_ms, words[f].start_ms),
                );
                let trail = estimated(
                    &mut out,
                    l + 1,
                    spread_before(
                        &texts[l + 1..],
                        words[l].start_ms + MIN_WORD_GAP_MS,
                        cue.end_ms,
                    ),
                );
                lead.and(trail)
            }
            _ => estimated(&mut out, 0, spread_before(&texts, cue.start_ms, cue.end_ms)),
        };
        cue.words = placed.and_then(|_| settle(cue, out));
    }
    ms
}

/// Move a whole cue by `delta_ms` (clamped between its neighbours and the
/// track); its words move with it. Returns the applied delta.
pub fn move_cue(track: &mut SubtitleTrack, i: usize, delta_ms: i64) -> i64 {
    let low = i.checked_sub(1).map_or(0, |p| track.cues[p].end_ms) as i64;
    let high = track
        .cues
        .get(i + 1)
        .map_or(track.duration_ms, |n| n.start_ms) as i64;
    let cue = &mut track.cues[i];
    let d = delta_ms.clamp(low - cue.start_ms as i64, high - cue.end_ms as i64);
    let shift = |t: u64| (t as i64 + d) as u64;
    cue.start_ms = shift(cue.start_ms);
    cue.end_ms = shift(cue.end_ms);
    for w in cue.words.iter_mut().flatten() {
        w.start_ms = shift(w.start_ms);
        w.end_ms = w.end_ms.map(shift);
        for s in w.syllables.iter_mut().flatten() {
            s.start_ms = shift(s.start_ms);
            s.end_ms = s.end_ms.map(shift);
        }
    }
    d
}

/// The gap around `ms` where a cue can be added (at least MIN_ADD_GAP_MS).
pub fn gap_at(track: &SubtitleTrack, ms: u64) -> Option<(u64, u64)> {
    let at = track.cues.partition_point(|c| c.end_ms <= ms);
    if track.cues.get(at).is_some_and(|c| c.start_ms <= ms) {
        return None;
    }
    let a = at.checked_sub(1).map_or(0, |p| track.cues[p].end_ms);
    let b = track.cues.get(at).map_or(track.duration_ms, |c| c.start_ms);
    (b - a >= MIN_ADD_GAP_MS).then_some((a, b))
}

/// Add a cue with `text` in the gap around `ms`: up to 2 s, centred on `ms`
/// within the gap. Returns its index.
pub fn add_cue(track: &mut SubtitleTrack, ms: u64, text: &str) -> Option<usize> {
    let (a, b) = gap_at(track, ms)?;
    let len = (b - a).min(2000);
    let start = ms.saturating_sub(len / 2).clamp(a, b - len);
    let at = track.cues.partition_point(|c| c.start_ms < start);
    track
        .cues
        .insert(at, SubtitleCue::new(start, start + len, text.trim()));
    if track.validate().is_err() {
        track.cues.remove(at);
        return None;
    }
    Some(at)
}

pub fn delete(track: &mut SubtitleTrack, i: usize) -> Option<SubtitleCue> {
    (i < track.cues.len()).then(|| track.cues.remove(i))
}

/// Cut a cue at `ms` (strictly inside, each part at least MIN_CUE_MS): the
/// words starting before `ms` go to the first part (with word times), else the
/// text is cut where the letters say. Returns false when it cannot.
pub fn split_at(track: &mut SubtitleTrack, i: usize, ms: u64) -> bool {
    let Some(cue) = track.cues.get(i).cloned() else {
        return false;
    };
    if ms < cue.start_ms + MIN_CUE_MS || ms + MIN_CUE_MS > cue.end_ms {
        return false;
    }
    let texts: Vec<&str> = cue.text.split_whitespace().collect();
    if texts.len() < 2 {
        return false;
    }
    let starts = match &cue.words {
        Some(w) => w.iter().map(|w| w.start_ms).collect(),
        None => spread(&texts, cue.start_ms, cue.end_ms),
    };
    let k = starts
        .iter()
        .filter(|t| **t < ms)
        .count()
        .clamp(1, texts.len() - 1);
    let mut first = SubtitleCue::new(cue.start_ms, ms, texts[..k].join(" "));
    let mut second = SubtitleCue::new(ms, cue.end_ms, texts[k..].join(" "));
    if let Some(w) = &cue.words {
        // The first part now ends at `ms`: its last word's known end is clamped there.
        let mut head = w[..k].to_vec();
        first.fit_ends(&mut head);
        first.words = Some(head).filter(|x| first.words_fit(x));
        second.words = settle(&second, w[k..].to_vec());
    }
    track.cues[i] = first;
    track.cues.insert(i + 1, second);
    if track.validate().is_err() {
        track.cues.remove(i + 1);
        track.cues[i] = cue;
        return false;
    }
    true
}

/// Join a cue with the next one: one cue from the first start to the second
/// end, texts joined by a line break; word times are kept when both have them.
pub fn join_next(track: &mut SubtitleTrack, i: usize) -> bool {
    if i + 1 >= track.cues.len() {
        return false;
    }
    let (a, b) = (track.cues[i].clone(), track.cues[i + 1].clone());
    let mut joined = SubtitleCue::new(a.start_ms, b.end_ms, format!("{}\n{}", a.text, b.text));
    if let (Some(x), Some(y)) = (&a.words, &b.words) {
        let mut all: Vec<WordTime> = x.iter().chain(y).cloned().collect();
        joined.fit_ends(&mut all);
        joined.words = Some(all).filter(|w| joined.words_fit(w));
    }
    track.cues[i] = joined;
    track.cues.remove(i + 1);
    if track.validate().is_err() {
        track.cues[i] = a;
        track.cues.insert(i + 1, b);
        return false;
    }
    true
}

/// One placeable unit of a cue: a word, or a syllable of a word cut into
/// syllables, in order. (word index, syllable index or None, start.)
pub fn units(cue: &SubtitleCue) -> Vec<(usize, Option<usize>, u64)> {
    let mut out = Vec::new();
    for (wi, w) in cue.words.iter().flatten().enumerate() {
        match &w.syllables {
            Some(syl) => out.extend(
                syl.iter()
                    .enumerate()
                    .map(|(si, s)| (wi, Some(si), s.start_ms)),
            ),
            None => out.push((wi, None, w.start_ms)),
        }
    }
    out
}

/// The cue's words, or (without word times) the letter split as Estimated words,
/// so a cue imported without word times can be placed by hand too.
pub fn words_or_estimate(cue: &SubtitleCue) -> Vec<WordTime> {
    if let Some(w) = &cue.words {
        return w.clone();
    }
    let texts: Vec<&str> = cue.text.split_whitespace().collect();
    let starts = spread_before(&texts, cue.start_ms, cue.end_ms)
        .unwrap_or_else(|| spread(&texts, cue.start_ms, cue.end_ms));
    texts
        .iter()
        .zip(starts)
        .map(|(t, s)| WordTime {
            text: (*t).into(),
            start_ms: s,
            end_ms: None,
            source: WordSource::Estimated,
            syllables: None,
        })
        .collect()
}

fn set_unit(words: &mut [WordTime], unit: (usize, Option<usize>), ms: u64) {
    let (wi, si) = unit;
    match (si, words[wi].syllables.as_mut()) {
        (Some(si), Some(syl)) => {
            syl[si].start_ms = ms;
            syl[si].source = WordSource::Hand;
            if si == 0 {
                words[wi].start_ms = ms;
                words[wi].source = WordSource::Hand;
            }
        }
        _ => {
            words[wi].start_ms = ms;
            words[wi].source = WordSource::Hand;
        }
    }
}

/// Place one unit (word or syllable) of cue `i` at `ms`, by hand: kept 50 ms
/// after the previous unit and before the next one (the first unit not before
/// the cue start, the last 50 ms before its end). Returns where it went.
pub fn set_unit_start(track: &mut SubtitleTrack, i: usize, unit: usize, ms: u64) -> Option<u64> {
    let cue = track.cues.get_mut(i)?;
    let mut words = words_or_estimate(cue);
    cue.words = Some(words.clone());
    let list = units(cue);
    let &(wi, si, _) = list.get(unit)?;
    let low = unit
        .checked_sub(1)
        .map_or(cue.start_ms, |p| list[p].2 + MIN_WORD_GAP_MS);
    let high = list
        .get(unit + 1)
        .map_or(cue.end_ms.saturating_sub(MIN_WORD_GAP_MS), |n| {
            n.2.saturating_sub(MIN_WORD_GAP_MS)
        });
    if high < low {
        return None;
    }
    let ms = ms.clamp(low, high);
    set_unit(&mut words, (wi, si), ms);
    // The previous unit's known end stays before this start; this unit's own end
    // is dropped if the start passed it.
    cue.fit_ends(&mut words);
    if !cue.words_fit(&words) {
        return None;
    }
    cue.words = Some(words);
    Some(ms)
}

/// A split typed with hyphens ("Some-thing"), if it keeps every character of
/// `word`, in order. The typed text is read along the word: a hyphen the word
/// has there stays in it ("peut-être" is whole), any other hyphen cuts
/// ("peut-ê-tre", and "dis--moi" for "dis-" | "moi"). One part means "not cut".
pub fn parse_syllables(typed: &str, word: &str) -> Option<Vec<String>> {
    let letters: Vec<char> = word.chars().collect();
    let mut parts = vec![String::new()];
    let mut k = 0;
    for c in typed.trim().chars() {
        if letters.get(k) == Some(&c) {
            parts.last_mut().unwrap().push(c);
            k += 1;
        } else if c == '-' {
            if !parts.last().unwrap().is_empty() {
                parts.push(String::new());
            }
        } else {
            return None;
        }
    }
    parts.retain(|p| !p.is_empty());
    (k == letters.len()).then_some(parts)
}

/// Cut word `w` of cue `i` into syllables (`parts` make the word), or join it
/// back with fewer than two parts. The first syllable keeps the word's start
/// and source; the others are spread by letters before the next unit, 50 ms
/// apart, and marked Estimated. False when nothing changes or they don't fit.
pub fn set_syllables(track: &mut SubtitleTrack, i: usize, w: usize, parts: &[String]) -> bool {
    let Some(cue) = track.cues.get_mut(i) else {
        return false;
    };
    let mut words = words_or_estimate(cue);
    let Some(word) = words.get(w).cloned() else {
        return false;
    };
    if parts.len() < 2 {
        if word.syllables.is_none() {
            return false;
        }
        words[w].syllables = None;
    } else {
        if parts.concat() != word.text {
            return false;
        }
        let next = words.get(w + 1).map_or(cue.end_ms, |n| n.start_ms);
        let texts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let Some(starts) = spread_before(&texts, word.start_ms, next) else {
            return false;
        };
        let syllables = parts
            .iter()
            .zip(starts)
            .enumerate()
            .map(|(k, (text, start_ms))| SyllableTime {
                text: text.clone(),
                start_ms,
                end_ms: None,
                source: if k == 0 {
                    word.source
                } else {
                    WordSource::Estimated
                },
            })
            .collect();
        words[w].syllables = Some(syllables);
    }
    cue.fit_ends(&mut words);
    if !cue.words_fit(&words) {
        return false;
    }
    cue.words = Some(words);
    true
}

/// Rhythm taps for cue `i`: one start per unit, in order. A first tap before
/// the cue start moves the start there (not before the previous cue's end); a
/// last tap after the end moves the end to 0.3 s after it (not past the next
/// cue). Taps closer than 50 ms are pushed apart. All or nothing.
pub fn apply_taps(track: &mut SubtitleTrack, i: usize, taps: &[u64]) -> bool {
    let Some(cue) = track.cues.get(i) else {
        return false;
    };
    let mut words = words_or_estimate(cue);
    let mut probe = cue.clone();
    probe.words = Some(words.clone());
    let list = units(&probe);
    if taps.len() != list.len() || taps.is_empty() {
        return false;
    }
    let prev_end = i.checked_sub(1).map_or(0, |p| track.cues[p].end_ms);
    let next_start = track
        .cues
        .get(i + 1)
        .map_or(track.duration_ms, |n| n.start_ms);
    let mut t: Vec<u64> = taps.to_vec();
    t[0] = t[0].max(prev_end);
    for k in 1..t.len() {
        t[k] = t[k].max(t[k - 1] + MIN_WORD_GAP_MS);
    }
    let mut cue = cue.clone();
    cue.start_ms = cue.start_ms.min(t[0]);
    let last = *t.last().unwrap();
    if last + MIN_WORD_GAP_MS > cue.end_ms {
        cue.end_ms = (last + 300).min(next_start);
    }
    for (&(wi, si, _), &ms) in list.iter().zip(&t) {
        set_unit(&mut words, (wi, si), ms);
    }
    // Taps give starts only: every unit moved, so no old end is kept.
    for w in &mut words {
        w.end_ms = None;
        for s in w.syllables.iter_mut().flatten() {
            s.end_ms = None;
        }
    }
    if !cue.words_fit(&words) {
        return false;
    }
    cue.words = Some(words);
    let mut next = track.clone();
    next.cues[i] = cue;
    if next.validate().is_err() {
        return false;
    }
    *track = next;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: u64, source: WordSource) -> WordTime {
        WordTime {
            text: text.into(),
            start_ms,
            end_ms: None,
            source,
            syllables: None,
        }
    }

    fn track() -> SubtitleTrack {
        let mut a = SubtitleCue::new(1000, 3000, "Something had to give");
        a.words = Some(vec![
            word("Something", 1000, WordSource::Hand),
            word("had", 1800, WordSource::Whisper),
            word("to", 2200, WordSource::Whisper),
            word("give", 2500, WordSource::Whisper),
        ]);
        SubtitleTrack {
            language: None,
            duration_ms: 20_000,
            cues: vec![a, SubtitleCue::new(4000, 6000, "and nothing disappears")],
        }
    }

    fn starts(t: &SubtitleTrack, i: usize) -> Vec<(String, u64, WordSource)> {
        t.cues[i]
            .words
            .iter()
            .flatten()
            .map(|w| (w.text.clone(), w.start_ms, w.source))
            .collect()
    }

    /// Known ends (lyrics screen, held words) stay valid through every edit:
    /// shifted with the cue, clamped to the next start, dropped when a start
    /// moves past them or is tapped again.
    #[test]
    fn known_word_ends_stay_valid_through_every_edit() {
        let ends = |t: &SubtitleTrack, i: usize| -> Vec<Option<u64>> {
            t.cues[i].words.iter().flatten().map(|w| w.end_ms).collect()
        };
        let mut base = track();
        {
            let w = base.cues[0].words.as_mut().unwrap();
            w[0].end_ms = Some(1600);
            w[1].end_ms = Some(2100);
        }
        assert!(base.validate().is_ok());

        let mut t = base.clone();
        assert_eq!(move_cue(&mut t, 0, 500), 500);
        assert_eq!(ends(&t, 0), [Some(2100), Some(2600), None, None]);

        // A hand-placed start before the previous word's end clamps that end.
        let mut t = base.clone();
        assert_eq!(set_unit_start(&mut t, 0, 1, 1500), Some(1500));
        assert_eq!(ends(&t, 0)[0], Some(1500));
        // A start moved past its own end drops it.
        let mut t = base.clone();
        assert_eq!(set_unit_start(&mut t, 0, 1, 2150), Some(2150));
        assert_eq!(ends(&t, 0)[1], None);

        // A split clamps the first part's last end to the cut; a join keeps it.
        let mut t = base.clone();
        t.cues[0].words.as_mut().unwrap()[1].end_ms = Some(2190);
        assert!(split_at(&mut t, 0, 2150));
        assert_eq!(ends(&t, 0), [Some(1600), Some(2150)]);
        assert!(join_next(&mut t, 0));
        assert_eq!(ends(&t, 0), [Some(1600), Some(2150), None, None]);
        assert!(t.validate().is_ok());

        // A text edit keeps unchanged words' ends; a new word has none.
        let mut t = base.clone();
        assert!(set_text(&mut t, 0, "Something had to live"));
        assert_eq!(ends(&t, 0), [Some(1600), Some(2100), None, None]);

        // Rhythm taps give starts only: every old end goes.
        let mut t = base.clone();
        assert!(apply_taps(&mut t, 0, &[1000, 1700, 2200, 2500]));
        assert_eq!(ends(&t, 0), [None, None, None, None]);

        // settle: a start pushed later loses its end.
        let cue = base.cues[0].clone();
        let mut words = cue.words.clone().unwrap();
        words[1].start_ms = 1020;
        words[1].end_ms = Some(1700);
        let settled = settle(&cue, words).unwrap();
        assert_eq!(settled[1].start_ms, 1050);
        assert_eq!(settled[1].end_ms, None);

        // The End edge dragged left past the last word's known end clamps it to
        // the new cue end; dragged before the word's start, the word moves and
        // its end goes.
        let mut t = base.clone();
        t.cues[0].words.as_mut().unwrap()[3].end_ms = Some(2900);
        assert_eq!(set_edge(&mut t, 0, Edge::End, 2700), 2700);
        assert_eq!(ends(&t, 0), [Some(1600), Some(2100), None, Some(2700)]);
        assert_eq!(set_edge(&mut t, 0, Edge::End, 2400), 2400);
        assert_eq!(ends(&t, 0)[3], None);
        assert!(t.validate().is_ok());

        // A re-split word: its syllables lose their ends, its own end stays.
        let mut t = base.clone();
        let parts = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        assert!(set_syllables(&mut t, 0, 0, &parts("Some", "thing")));
        t.cues[0].words.as_mut().unwrap()[0]
            .syllables
            .as_mut()
            .unwrap()[0]
            .end_ms = Some(1300);
        assert!(t.validate().is_ok());
        assert!(set_syllables(&mut t, 0, 0, &parts("So", "mething")));
        let w = &t.cues[0].words.as_ref().unwrap()[0];
        assert_eq!(w.end_ms, Some(1600));
        assert!(w.syllables.iter().flatten().all(|s| s.end_ms.is_none()));
    }

    #[test]
    fn a_text_edit_keeps_unchanged_words_and_spreads_the_new_ones() {
        let mut t = track();
        assert!(set_text(&mut t, 0, "Something has to give now"));
        let w = starts(&t, 0);
        assert_eq!(w[0], ("Something".into(), 1000, WordSource::Hand));
        assert_eq!(w[1].0, "has");
        assert_eq!(w[1].2, WordSource::Estimated);
        assert_eq!((w[2].1, w[3].1), (2200, 2500), "kept words keep their time");
        assert_eq!(w[4].2, WordSource::Estimated);
        assert!(t.validate().is_ok());
        assert!(!set_text(&mut t, 0, "   "), "an empty text is refused");
        // A cue without word times stays without them.
        assert!(set_text(&mut t, 1, "and all disappears"));
        assert!(t.cues[1].words.is_none());
    }

    #[test]
    fn edges_stop_at_neighbours_the_minimum_length_and_hand_placed_words() {
        let mut t = track();
        assert_eq!(
            set_edge(&mut t, 0, Edge::End, 9000),
            4000,
            "stops at the next cue"
        );
        assert_eq!(
            set_edge(&mut t, 0, Edge::Start, 1500),
            1000,
            "a hand-placed word holds the start"
        );
        assert_eq!(
            set_edge(&mut t, 1, Edge::Start, 0),
            4000,
            "stops at the previous cue"
        );
        assert_eq!(
            set_edge(&mut t, 1, Edge::End, 4000),
            4200,
            "200 ms at least"
        );
        assert!(t.validate().is_ok());
    }

    #[test]
    fn words_an_edge_leaves_outside_are_spread_and_marked_estimated() {
        let mut t = track();
        set_edge(&mut t, 0, Edge::End, 2300);
        let w = starts(&t, 0);
        assert_eq!((w[0].1, w[1].1, w[2].1), (1000, 1800, 2200));
        assert_eq!(w[3].2, WordSource::Estimated, "'give' was left outside");
        assert!(w[3].1 > 2200 && w[3].1 <= 2250);
        assert!(t.validate().is_ok());
    }

    #[test]
    fn words_left_outside_fit_before_the_end_and_hand_words_never_move() {
        // End 200 ms after the last kept word, two words outside: they fit 50 ms apart.
        let mut cue = SubtitleCue::new(1000, 4000, "a b c d");
        cue.words = Some(vec![
            word("a", 1000, WordSource::Whisper),
            word("b", 1500, WordSource::Whisper),
            word("c", 3000, WordSource::Whisper),
            word("d", 3500, WordSource::Whisper),
        ]);
        let mut t = SubtitleTrack {
            language: None,
            duration_ms: 10_000,
            cues: vec![cue],
        };
        set_edge(&mut t, 0, Edge::End, 1700);
        let w = starts(&t, 0);
        assert_eq!(
            w.iter().map(|w| w.1).collect::<Vec<_>>(),
            [1000, 1500, 1550, 1625]
        );
        assert_eq!(w[3].2, WordSource::Estimated);
        assert!(t.validate().is_ok());
        // With no room left for them at 50 ms, the cue falls back to the letter split.
        set_edge(&mut t, 0, Edge::End, 1610);
        assert!(t.cues[0].words.is_none());
        // A hand word after two others: the start stops 100 ms before it.
        let mut cue = SubtitleCue::new(1000, 4000, "x y z");
        cue.words = Some(vec![
            word("x", 1000, WordSource::Whisper),
            word("y", 1100, WordSource::Whisper),
            word("z", 2000, WordSource::Hand),
        ]);
        let mut t = SubtitleTrack {
            language: None,
            duration_ms: 10_000,
            cues: vec![cue],
        };
        // The end keeps 50 ms per word after the last hand word.
        assert_eq!(edge_bounds(&t, 0, Edge::End).0, 2050);
        assert_eq!(set_edge(&mut t, 0, Edge::Start, 2000), 1900);
        let w = starts(&t, 0);
        assert_eq!(
            w[2],
            ("z".into(), 2000, WordSource::Hand),
            "the hand word stays put"
        );
        assert_eq!((w[0].1, w[1].1), (1900, 1950));
    }

    #[test]
    fn a_word_placed_by_hand_stays_between_its_neighbours() {
        let mut t = track();
        assert_eq!(
            set_unit_start(&mut t, 0, 2, 1000),
            Some(1850),
            "50 ms after 'had'"
        );
        assert_eq!(starts(&t, 0)[2], ("to".into(), 1850, WordSource::Hand));
        assert_eq!(
            set_unit_start(&mut t, 0, 3, 9000),
            Some(2950),
            "50 ms before the end"
        );
        // A cue without word times gets them from the letter split, then by hand.
        assert_eq!(set_unit_start(&mut t, 1, 1, 4400), Some(4400));
        let w = starts(&t, 1);
        assert_eq!((w[0].2, w[1].2), (WordSource::Estimated, WordSource::Hand));
        assert!(t.validate().is_ok());
    }

    #[test]
    fn a_word_cut_into_syllables_keeps_its_start_and_spreads_the_rest() {
        let mut t = track();
        let parts = parse_syllables("Some-thing", "Something").unwrap();
        assert!(set_syllables(&mut t, 0, 0, &parts));
        let syl: Vec<_> = t.cues[0].words.as_ref().unwrap()[0]
            .syllables
            .iter()
            .flatten()
            .map(|s| (s.text.as_str(), s.start_ms, s.source))
            .collect();
        assert_eq!(
            syl[0],
            ("Some", 1000, WordSource::Hand),
            "the word's start and source"
        );
        assert_eq!((syl[1].0, syl[1].2), ("thing", WordSource::Estimated));
        assert!(
            syl[1].1 > 1000 && syl[1].1 + 50 <= 1800,
            "between the word and the next one: {}",
            syl[1].1
        );
        assert_eq!(
            units(&t.cues[0]).len(),
            5,
            "each syllable is placed, and tapped, on its own"
        );
        // Each syllable is then placed by hand like a word.
        assert_eq!(set_unit_start(&mut t, 0, 1, 1500), Some(1500));
        assert_eq!(
            t.cues[0].words.as_ref().unwrap()[0]
                .syllables
                .as_ref()
                .unwrap()[1]
                .source,
            WordSource::Hand
        );
        assert!(t.validate().is_ok());
        // Letters must all stay, in order; no room means no change; one part joins back.
        assert_eq!(parse_syllables("Some-thin", "Something"), None);
        assert_eq!(parse_syllables("so-me", "Something"), None);
        // A word's own hyphen stays in it; a doubled one cuts there.
        assert_eq!(
            parse_syllables("peut-être", "peut-être"),
            Some(vec!["peut-être".into()])
        );
        assert_eq!(
            parse_syllables("peut-ê-tre", "peut-être"),
            Some(vec!["peut-ê".into(), "tre".into()])
        );
        assert_eq!(
            parse_syllables("dis--moi", "dis-moi"),
            Some(vec!["dis-".into(), "moi".into()])
        );
        assert_eq!(parse_syllables("dis-mo", "dis-moi"), None);
        // "to" keeps 50 ms before "give": no room for two syllables.
        let mut tight = track();
        tight.cues[0].words.as_mut().unwrap()[3].start_ms = 2250;
        assert!(!set_syllables(
            &mut tight,
            0,
            2,
            &parse_syllables("t-o", "to").unwrap()
        ));
        assert!(set_syllables(&mut t, 0, 0, &["Something".into()]));
        assert!(t.cues[0].words.as_ref().unwrap()[0].syllables.is_none());
        assert!(
            !set_syllables(&mut t, 0, 0, &["Something".into()]),
            "already whole"
        );
    }

    #[test]
    fn rhythm_taps_place_every_unit_and_may_move_the_line_edges() {
        let mut t = track();
        assert!(!apply_taps(&mut t, 0, &[900, 1500]), "one tap per unit");
        assert!(apply_taps(&mut t, 0, &[900, 1500, 1520, 3100]));
        assert_eq!(
            t.cues[0].start_ms, 900,
            "an early first tap moves the start"
        );
        assert_eq!(
            t.cues[0].end_ms, 3400,
            "a late last tap moves the end 0.3 s after it"
        );
        assert_eq!(
            starts(&t, 0).iter().map(|w| (w.1, w.2)).collect::<Vec<_>>(),
            [
                (900, WordSource::Hand),
                (1500, WordSource::Hand),
                (1550, WordSource::Hand),
                (3100, WordSource::Hand)
            ]
        );
        assert!(t.validate().is_ok());
        // Never past the neighbours.
        assert!(apply_taps(&mut t, 1, &[3000, 4500, 7000]));
        assert_eq!(t.cues[1].start_ms, 3400);
        assert!(t.validate().is_ok());
    }

    #[test]
    fn moving_a_cue_moves_its_words_within_its_neighbours() {
        let mut t = track();
        assert_eq!(
            move_cue(&mut t, 0, 5000),
            1000,
            "stops against the next cue"
        );
        assert_eq!(t.cues[0].start_ms, 2000);
        assert_eq!(starts(&t, 0)[1].1, 2800);
        assert_eq!(move_cue(&mut t, 0, -10_000), -2000);
        assert!(t.validate().is_ok());
    }

    #[test]
    fn plus_adds_a_cue_in_a_gap_of_half_a_second_or_more() {
        let mut t = track();
        assert_eq!(gap_at(&t, 3500), Some((3000, 4000)));
        assert_eq!(gap_at(&t, 2000), None, "inside a cue");
        let at = add_cue(&mut t, 3500, "new").unwrap();
        assert_eq!((t.cues[at].start_ms, t.cues[at].end_ms), (3000, 4000));
        assert!(gap_at(&t, 3500).is_none());
        assert!(t.validate().is_ok());
    }

    #[test]
    fn cut_and_join_keep_the_words_on_their_side() {
        let mut t = track();
        assert!(split_at(&mut t, 0, 2100));
        assert_eq!(t.cues[0].text, "Something had");
        assert_eq!(t.cues[1].text, "to give");
        assert_eq!(starts(&t, 1)[0].1, 2200);
        assert!(t.validate().is_ok());
        assert!(join_next(&mut t, 0));
        assert_eq!(t.cues[0].text, "Something had\nto give");
        assert_eq!(starts(&t, 0).len(), 4);
        assert!(t.validate().is_ok());
        // Without word times the letters decide where the text is cut.
        assert!(split_at(&mut t, 1, 5000));
        assert_eq!(t.cues[1].text, "and nothing");
        assert!(!split_at(&mut t, 1, 4100), "each part keeps 200 ms");
    }
}
