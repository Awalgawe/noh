//! Edits of a subtitle track from the lyrics lane (fused mockup v9, Task 41 S3):
//! pure functions, no IO. Every edit keeps the track valid and follows the word
//! rules of the mockup's note: a text edit keeps unchanged words' times; moving a
//! cue moves its words; an edge stops at its neighbour (and at hand-placed
//! words), and words it leaves outside are spread by letters ("estimated").
//! The lyrics screen adds cuts at a word, joins with a space, syllable
//! proposals for whole lines, and the sync game's holds and « passed » rules.
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
/// without word times (style/line.rs split_words), on whole milliseconds.
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
    let mut cue = SubtitleCue::new(start, start + len, text.trim());
    // A line added after another is sung by the same voices.
    if let Some(before) = at.checked_sub(1).and_then(|p| track.cues.get(p)) {
        cue.voices = before.voices.clone();
    }
    track.cues.insert(at, cue);
    if track.validate().is_err() {
        track.cues.remove(at);
        return None;
    }
    Some(at)
}

/// Give cues `lo..=hi` the set `voices` (sorted and made unique here). Refused
/// (nothing changes) when a number is not declared or nothing would change.
pub fn set_voices(track: &mut SubtitleTrack, lo: usize, hi: usize, voices: &[u8]) -> bool {
    let mut set = voices.to_vec();
    set.sort_unstable();
    set.dedup();
    if hi >= track.cues.len()
        || lo > hi
        || set
            .iter()
            .any(|&v| v == 0 || v as usize > track.voices.len())
        || track.cues[lo..=hi].iter().all(|c| c.voices == set)
    {
        return false;
    }
    for cue in &mut track.cues[lo..=hi] {
        cue.voices = set.clone();
    }
    true
}

/// Declare `voices` (at most `MAX_VOICES`). Lines lose the numbers that are no
/// longer declared (only the last voices can go, so numbers never shift).
pub fn declare_voices(
    track: &mut SubtitleTrack,
    voices: Vec<crate::subtitle_track::Voice>,
) -> bool {
    if voices.len() > crate::subtitle_track::MAX_VOICES || voices == track.voices {
        return false;
    }
    let before = track.clone();
    let count = voices.len() as u8;
    track.voices = voices;
    for cue in &mut track.cues {
        cue.voices.retain(|&v| v <= count);
    }
    if track.validate().is_err() {
        *track = before;
        return false;
    }
    true
}

/// How many lines sing voice `n`.
pub fn lines_with_voice(track: &SubtitleTrack, n: u8) -> usize {
    track.cues.iter().filter(|c| c.voices.contains(&n)).count()
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
    // Both halves are sung by the voices of the line they come from.
    first.voices = cue.voices.clone();
    second.voices = cue.voices.clone();
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
/// end, texts joined by `sep` (a space: one line of text, as the lyrics screen
/// joins, answer Q3); word times are kept when both have them.
pub fn join_next_with(track: &mut SubtitleTrack, i: usize, sep: &str) -> bool {
    if i + 1 >= track.cues.len() {
        return false;
    }
    let (a, b) = (track.cues[i].clone(), track.cues[i + 1].clone());
    let mut joined = SubtitleCue::new(a.start_ms, b.end_ms, format!("{}{sep}{}", a.text, b.text));
    // The joined line keeps the first line's voices (the screen says so).
    joined.voices = a.voices.clone();
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

/// Cut cue `i` before its word `k` (1 ≤ k < word count), at that word's start:
/// its known time, or the letter estimate when the cue has no word times (the
/// cut `split_at` makes). The lyrics screen cuts at the text cursor with it.
pub fn split_at_word(track: &mut SubtitleTrack, i: usize, k: usize) -> bool {
    let Some(cue) = track.cues.get(i) else {
        return false;
    };
    let texts: Vec<&str> = cue.text.split_whitespace().collect();
    if k == 0 || k >= texts.len() {
        return false;
    }
    let starts: Vec<u64> = match &cue.words {
        Some(w) => w.iter().map(|w| w.start_ms).collect(),
        None => spread(&texts, cue.start_ms, cue.end_ms),
    };
    // Equal estimated starts (a very short cue) would move the cut to another word.
    if starts.iter().filter(|t| **t < starts[k]).count() != k {
        return false;
    }
    split_at(track, i, starts[k])
}

/// Give each word of the cues in `range` that is not cut yet the split
/// `propose` returns for its text, when it has two parts or more and fits (a
/// cue without word times gets its letter-estimated words first). The
/// Syllables level of the sync game asks this for the lines it will play.
/// Returns how many words were cut.
pub fn propose_syllables(
    track: &mut SubtitleTrack,
    range: std::ops::Range<usize>,
    propose: impl Fn(&str) -> Vec<String>,
) -> usize {
    let mut cut = 0;
    let count = track.cues.len();
    for i in range.filter(|i| *i < count) {
        let words = words_or_estimate(&track.cues[i]);
        for (w, word) in words.iter().enumerate() {
            if word.syllables.is_some() {
                continue;
            }
            let parts = propose(&word.text);
            if parts.len() >= 2 && set_syllables(track, i, w, &parts) {
                cut += 1;
            }
        }
    }
    cut
}

/// What a sync game takes: a whole cue, a word, a syllable, or a backing
/// vocal (a word between parentheses). The Words and Syllables levels take the
/// cue's main words only: one key cannot hold a word and tap a backing vocal
/// that overlaps it, so backing vocals have a pass of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Line,
    Word,
    Syllable,
    Backing,
}

/// The kinds of a cue's words for the game: true for a backing vocal. Words
/// that do not match the text (they cannot here) are all main words.
fn kinds(cue: &SubtitleCue, words: usize) -> Vec<bool> {
    let flags = crate::subtitle_track::backing_flags(&cue.text);
    if flags.len() == words {
        flags
    } else {
        vec![false; words]
    }
}

/// Backing vocals nobody took that now sit out of text order (a main word
/// taken before them, or after them) are spread again between their
/// neighbours, by letters (Estimated, no end): a main word taken by hand never
/// moves because of them.
fn respread_untaken_backing(
    cue: &SubtitleCue,
    words: &mut [WordTime],
    kind: &[bool],
    taken: &[(usize, usize)],
) {
    let n = words.len();
    let mut i = 0;
    while i < n {
        if !kind[i] || taken.iter().any(|t| t.0 == i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < n && kind[j + 1] && !taken.iter().any(|t| t.0 == j + 1) {
            j += 1;
        }
        let low = i
            .checked_sub(1)
            .map_or(cue.start_ms, |p| last_start(&words[p]) + MIN_WORD_GAP_MS);
        let high = words
            .get(j + 1)
            .map_or(cue.end_ms, |w| w.start_ms)
            .saturating_sub(MIN_WORD_GAP_MS);
        let in_order = words[i..=j]
            .iter()
            .all(|w| w.start_ms >= low && last_start(w) <= high)
            && words[i..=j]
                .windows(2)
                .all(|p| p[1].start_ms >= last_start(&p[0]) + MIN_WORD_GAP_MS);
        if !in_order && high > low {
            let texts: Vec<&str> = words[i..=j].iter().map(|w| w.text.as_str()).collect();
            let starts = spread(&texts, low, high);
            for (w, start_ms) in words[i..=j].iter_mut().zip(starts) {
                w.start_ms = start_ms;
                w.end_ms = None;
                w.source = WordSource::Estimated;
                w.syllables = None;
            }
        }
        i = j + 1;
    }
}

/// The start of a word's last unit (its last syllable, or the word itself).
fn last_start(w: &WordTime) -> u64 {
    w.syllables
        .as_ref()
        .and_then(|s| s.last())
        .map_or(w.start_ms, |s| s.start_ms.max(w.start_ms))
}

/// One held key in the sync game: pressed at `start_ms`, released at `end_ms`
/// (song times), for cue `cue`, its word `word` and that word's syllable
/// `syllable` (the indices below the game's level are ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    pub cue: usize,
    pub word: usize,
    pub syllable: usize,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Two holds closer than this are one sung stretch (legato): the first ends
/// where the second starts.
pub const LEGATO_MS: u64 = 120;

/// The track after a sync game's holds, as one new track (one undo step), or
/// None when they cannot all apply.
///
/// `holds` are in the order they were held: song time rising, cue index never
/// going back (a Refaire drops the holds it rewinds over before new ones come).
/// The screen applies the holds after each release; on None it drops only the
/// last hold, says so, and keeps the others.
///
/// - Lines: the cue takes the hold's start and end (at least MIN_CUE_MS long);
///   its words keep their places in proportion.
/// - Words and syllables: each taken unit gets its start, its end and the Hand
///   source. A word's syllables are spread again inside a taken word; a taken
///   syllable 0 moves its word's start, a taken last syllable sets its word's
///   end. At the Syllables level a word that is not cut is one unit (syllable
///   0); a later syllable of an uncut word cannot be taken. Untaken units that
///   a taken end now covers move to that end (Estimated, no end), so a held end
///   is never cut short by a neighbour nobody took. The cue grows to hold them.
/// - A take that starts before the previous cue's end pulls that end back, as
///   far as the previous cue's own last unit allows: a stale end (a recognizer
///   segment that ran on) never pushes the take later.
/// - A cue that now ends after the next cue's start pushes that start, never
///   past the cue after it (the next hold usually retakes it).
pub fn apply_holds(track: &SubtitleTrack, level: Level, holds: &[Hold]) -> Option<SubtitleTrack> {
    debug_assert!(
        holds
            .windows(2)
            .all(|p| p[0].start_ms <= p[1].start_ms && p[0].cue <= p[1].cue),
        "holds must come in the order they were held"
    );
    let mut holds = holds.to_vec();
    for n in 1..holds.len() {
        let (prev_start, prev_end, start) = (
            holds[n - 1].start_ms,
            holds[n - 1].end_ms,
            holds[n].start_ms,
        );
        if start > prev_start && (prev_end > start || start - prev_end < LEGATO_MS) {
            holds[n - 1].end_ms = start;
        }
    }
    let mut t = track.clone();
    let mut n = 0;
    while n < holds.len() {
        let c = holds[n].cue;
        let group: Vec<Hold> = holds[n..]
            .iter()
            .take_while(|h| h.cue == c)
            .copied()
            .collect();
        n += group.len();
        if c >= t.cues.len() {
            return None;
        }
        pull_previous_end(&mut t, c, group[0].start_ms);
        let prev_end = c.checked_sub(1).map_or(0, |p| t.cues[p].end_ms);
        let cue = t.cues[c].clone();
        let mut next = cue.clone();
        match level {
            Level::Line => {
                let h = group.last()?;
                next.start_ms = h.start_ms.max(prev_end);
                next.end_ms = h.end_ms.max(next.start_ms + MIN_CUE_MS);
                next.words = cue.words.as_ref().and_then(|w| {
                    let (a, b) = (cue.start_ms, cue.end_ms.max(cue.start_ms + 1));
                    let (na, nb) = (next.start_ms, next.end_ms);
                    let f = |ms: u64| na + (ms.saturating_sub(a)) * (nb - na) / (b - a);
                    let moved = w
                        .iter()
                        .map(|w| WordTime {
                            start_ms: f(w.start_ms),
                            end_ms: w.end_ms.map(f),
                            syllables: w.syllables.as_ref().map(|s| {
                                s.iter()
                                    .map(|s| SyllableTime {
                                        start_ms: f(s.start_ms),
                                        end_ms: s.end_ms.map(f),
                                        ..s.clone()
                                    })
                                    .collect()
                            }),
                            ..w.clone()
                        })
                        .collect();
                    settle(&next, moved)
                });
            }
            Level::Word | Level::Syllable | Level::Backing => {
                let mut words = words_or_estimate(&cue);
                let kind = kinds(&cue, words.len());
                let backing = level == Level::Backing;
                // (word, syllable) of every taken unit; a whole word is syllable 0.
                let mut taken: Vec<(usize, usize)> = Vec::new();
                for h in &group {
                    // Each pass takes its own kind of words only.
                    if *kind.get(h.word)? != backing {
                        return None;
                    }
                    let mut h = *h;
                    if backing {
                        // A backing vocal keeps text order: its start stays
                        // between its neighbours' starts, which never move.
                        let low = h
                            .word
                            .checked_sub(1)
                            .map_or(cue.start_ms, |p| last_start(&words[p]) + MIN_WORD_GAP_MS);
                        let high = words
                            .get(h.word + 1)
                            .map_or(cue.end_ms, |w| w.start_ms)
                            .saturating_sub(MIN_WORD_GAP_MS);
                        if low > high {
                            return None;
                        }
                        h.start_ms = h.start_ms.clamp(low, high);
                    }
                    let w = words.get_mut(h.word)?;
                    let end = h.end_ms.max(h.start_ms + 1);
                    let whole = level != Level::Syllable || w.syllables.is_none();
                    if whole {
                        if level == Level::Syllable && h.syllable != 0 {
                            return None;
                        }
                        w.start_ms = h.start_ms;
                        w.end_ms = Some(end);
                        w.source = WordSource::Hand;
                        if let Some(syl) = &w.syllables {
                            let texts: Vec<&str> = syl.iter().map(|s| s.text.as_str()).collect();
                            // Spread inside the held word; a hold too short for them
                            // keeps the word whole rather than refusing the game.
                            w.syllables = spread_before(&texts, h.start_ms, end).map(|starts| {
                                syl.iter()
                                    .zip(starts)
                                    .enumerate()
                                    .map(|(k, (s, start_ms))| SyllableTime {
                                        start_ms,
                                        end_ms: None,
                                        source: if k == 0 {
                                            WordSource::Hand
                                        } else {
                                            WordSource::Estimated
                                        },
                                        ..s.clone()
                                    })
                                    .collect()
                            });
                        }
                        taken.push((h.word, 0));
                    } else {
                        let count = w.syllables.as_ref()?.len();
                        let s = w.syllables.as_mut()?.get_mut(h.syllable)?;
                        s.start_ms = h.start_ms;
                        s.end_ms = Some(end);
                        s.source = WordSource::Hand;
                        if h.syllable == 0 {
                            w.start_ms = h.start_ms;
                            w.source = WordSource::Hand;
                        }
                        if h.syllable + 1 == count {
                            w.end_ms = Some(end);
                        }
                        taken.push((h.word, h.syllable));
                    }
                }
                push_covered(&mut words, &taken, level, &kind);
                if !backing {
                    respread_untaken_backing(&cue, &mut words, &kind, &taken);
                }
                let first = words.first()?.start_ms;
                let last_end = words
                    .iter()
                    .map(|w| {
                        let last = w
                            .syllables
                            .as_ref()
                            .and_then(|s| s.last())
                            .map_or(w.start_ms, |s| s.start_ms);
                        w.end_ms.unwrap_or(0).max(last + MIN_WORD_GAP_MS)
                    })
                    .max()?;
                next.start_ms = cue.start_ms.min(first).max(prev_end);
                next.end_ms = cue.end_ms.max(last_end);
                next.words = Some(settle(&next, words)?);
            }
        }
        let cap = t.cues.get(c + 2).map_or(t.duration_ms, |x| x.start_ms);
        if let Some(following) = t.cues.get_mut(c + 1)
            && following.start_ms < next.end_ms
        {
            let len = following.end_ms - following.start_ms;
            following.start_ms = next.end_ms;
            following.end_ms = following
                .end_ms
                .max(next.end_ms + MIN_CUE_MS.min(len))
                .min(cap);
            if let Some(mut w) = following.words.take() {
                let probe = following.clone();
                following.words = settle(&probe, std::mem::take(&mut w));
            }
        }
        t.cues[c] = next;
    }
    t.validate().is_ok().then_some(t)
}

/// A take for cue `c` starting at `start` before the previous cue's end pulls
/// that end back to the take, but not before the previous cue's last unit (its
/// known end, or 50 ms after its last start) nor under MIN_CUE_MS.
fn pull_previous_end(t: &mut SubtitleTrack, c: usize, start: u64) {
    let Some(p) = c.checked_sub(1) else {
        return;
    };
    let prev = &mut t.cues[p];
    if prev.end_ms <= start {
        return;
    }
    let last_unit = prev.words.as_ref().map_or(0, |w| {
        w.iter()
            .map(|w| {
                let last = w
                    .syllables
                    .as_ref()
                    .and_then(|s| s.last())
                    .map_or(w.start_ms, |s| s.start_ms);
                w.end_ms.unwrap_or(0).max(last + MIN_WORD_GAP_MS)
            })
            .max()
            .unwrap_or(0)
    });
    let end = start
        .max(last_unit)
        .max(prev.start_ms + MIN_CUE_MS)
        .min(prev.end_ms);
    prev.end_ms = end;
    if let Some(mut w) = prev.words.take() {
        prev.fit_ends(&mut w);
        let fits = prev.words_fit(&w);
        prev.words = fits.then_some(w);
    }
}

/// Untaken units that a taken unit's end now covers move to that end
/// (Estimated, no end), each 50 ms after the one before: a word whole at the
/// Words level (its syllables shifted with it), a syllable at the Syllables
/// level (its word's start following its syllable 0).
fn push_covered(words: &mut [WordTime], taken: &[(usize, usize)], level: Level, kind: &[bool]) {
    let mut floor = 0u64;
    let backing = level == Level::Backing;
    for (wi, w) in words.iter_mut().enumerate() {
        // Only the words of the pass's kind: a held main word never pushes a
        // backing vocal it overlaps, nor the other way round.
        if kind.get(wi).copied().unwrap_or(false) != backing {
            continue;
        }
        match (level, w.syllables.as_mut()) {
            (Level::Syllable, Some(syl)) => {
                for (si, s) in syl.iter_mut().enumerate() {
                    if taken.contains(&(wi, si)) {
                        floor = floor.max(s.end_ms.unwrap_or(s.start_ms + MIN_WORD_GAP_MS));
                    } else if s.start_ms < floor {
                        s.start_ms = floor;
                        s.end_ms = None;
                        s.source = WordSource::Estimated;
                        floor += MIN_WORD_GAP_MS;
                    }
                }
                if w.start_ms != syl[0].start_ms {
                    w.start_ms = syl[0].start_ms;
                    w.source = syl[0].source;
                }
            }
            _ => {
                if taken.contains(&(wi, 0)) {
                    floor = floor.max(w.end_ms.unwrap_or(w.start_ms + MIN_WORD_GAP_MS));
                } else if w.start_ms < floor {
                    let d = floor - w.start_ms;
                    w.start_ms = floor;
                    w.end_ms = None;
                    w.source = WordSource::Estimated;
                    for s in w.syllables.iter_mut().flatten() {
                        s.start_ms += d;
                        s.end_ms = None;
                        s.source = WordSource::Estimated;
                    }
                    floor = w
                        .syllables
                        .as_ref()
                        .and_then(|s| s.last())
                        .map_or(w.start_ms, |s| s.start_ms)
                        + MIN_WORD_GAP_MS;
                }
            }
        }
    }
}

/// A unit the sync game expects next, with its guide: the known time (Whisper,
/// by hand) or the letter estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub cue: usize,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Playback this far past the expected unit's guide end with no press: the
/// game waits (the unit was passed).
pub const MISS_AFTER_MS: u64 = 1500;
/// A press this far after the next line's guide start, while units of the
/// current line are left, means they were passed.
pub const NEXT_LINE_MARGIN_MS: u64 = 300;

/// The game's two « passed » rules for the expected unit `idx` at song time
/// `now`. With `pressing` false: no key held and playback MISS_AFTER_MS past its
/// guide end. With `pressing` true (a key just went down): at Words or
/// Syllables level, the next line's first unit started more than
/// NEXT_LINE_MARGIN_MS ago, so this press is not used and the game waits; a
/// slip inside a line is not detected, and never spreads past it.
pub fn passed(now: u64, guides: &[Guide], idx: usize, level: Level, pressing: bool) -> bool {
    let Some(g) = guides.get(idx) else {
        return false;
    };
    if !pressing {
        return now > g.end_ms + MISS_AFTER_MS;
    }
    if level == Level::Line {
        return false;
    }
    guides[idx..]
        .iter()
        .find(|x| x.cue != g.cue)
        .is_some_and(|n| now > n.start_ms + NEXT_LINE_MARGIN_MS)
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
            voices: Vec::new(),
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

    #[test]
    fn a_cut_at_a_word_uses_its_start_or_the_letter_estimate() {
        let mut t = track();
        assert!(split_at_word(&mut t, 0, 2));
        assert_eq!(t.cues[0].text, "Something had");
        assert_eq!((t.cues[0].end_ms, t.cues[1].start_ms), (2200, 2200));
        // No word times: the cut falls where the letters put « disappears ».
        let mut t = track();
        assert!(split_at_word(&mut t, 1, 2));
        assert_eq!(
            (t.cues[1].text.as_str(), t.cues[2].text.as_str()),
            ("and nothing", "disappears")
        );
        let letters = spread(&["and", "nothing", "disappears"], 4000, 6000);
        assert_eq!(t.cues[2].start_ms, letters[2]);
        // Not before the first word, not past the last.
        let mut t = track();
        assert!(!split_at_word(&mut t, 0, 0));
        assert!(!split_at_word(&mut t, 0, 4));
    }

    #[test]
    fn a_join_with_a_space_gives_one_line_of_text() {
        let mut t = track();
        assert!(split_at_word(&mut t, 0, 2));
        assert!(join_next_with(&mut t, 0, " "));
        assert_eq!(t.cues[0].text, "Something had to give");
        assert_eq!(t.cues[0].words.as_ref().map(Vec::len), Some(4));
    }

    #[test]
    fn syllables_are_proposed_for_every_whole_word_of_the_lines() {
        let mut t = track();
        let cut = propose_syllables(&mut t, 0..2, |w| match w {
            "Something" => vec!["Some".into(), "thing".into()],
            "nothing" => vec!["no".into(), "thing".into()],
            _ => vec![w.into()],
        });
        assert_eq!(cut, 2);
        let syl = |t: &SubtitleTrack, i: usize, w: usize| {
            t.cues[i].words.as_ref().unwrap()[w]
                .syllables
                .as_ref()
                .map(|s| s.iter().map(|s| s.text.clone()).collect::<Vec<_>>())
        };
        assert_eq!(syl(&t, 0, 0), Some(vec!["Some".into(), "thing".into()]));
        // A cue without word times got its estimated words first.
        assert_eq!(syl(&t, 1, 1), Some(vec!["no".into(), "thing".into()]));
        assert_eq!(syl(&t, 1, 0), None);
        // Already cut: left alone.
        assert_eq!(
            propose_syllables(&mut t, 0..2, |w| vec![w.into(), "x".into()]),
            0
        );
    }

    #[test]
    fn line_holds_set_start_and_end_and_push_the_next_line() {
        let t = track();
        let hold = |cue, start_ms, end_ms| Hold {
            cue,
            word: 0,
            syllable: 0,
            start_ms,
            end_ms,
        };
        let out = apply_holds(&t, Level::Line, &[hold(0, 1200, 4300)]).unwrap();
        assert_eq!((out.cues[0].start_ms, out.cues[0].end_ms), (1200, 4300));
        // Words keep their places in proportion: « had » was 40 % in.
        assert_eq!(
            out.cues[0].words.as_ref().unwrap()[1].start_ms,
            1200 + 3100 * 800 / 2000
        );
        // The next line started at 4000: pushed to 4300.
        assert_eq!(out.cues[1].start_ms, 4300);
        assert!(out.validate().is_ok());
        // Legato: a second hold starting 80 ms after the first ended closes the gap.
        let out =
            apply_holds(&t, Level::Line, &[hold(0, 1000, 3000), hold(1, 3080, 5900)]).unwrap();
        assert_eq!((out.cues[0].end_ms, out.cues[1].start_ms), (3080, 3080));
    }

    #[test]
    fn word_holds_keep_their_end_and_the_line_grows_to_hold_them() {
        let t = track();
        let hold = |word, start_ms, end_ms| Hold {
            cue: 0,
            word,
            syllable: 0,
            start_ms,
            end_ms,
        };
        let out = apply_holds(
            &t,
            Level::Word,
            &[
                hold(0, 900, 1500),
                hold(1, 1800, 2000),
                hold(2, 2300, 2400),
                hold(3, 2600, 3400),
            ],
        )
        .unwrap();
        let words = out.cues[0].words.as_ref().unwrap();
        let got: Vec<(u64, Option<u64>, WordSource)> = words
            .iter()
            .map(|w| (w.start_ms, w.end_ms, w.source))
            .collect();
        assert_eq!(
            got,
            [
                (900, Some(1500), WordSource::Hand),
                (1800, Some(2000), WordSource::Hand),
                (2300, Some(2400), WordSource::Hand),
                (2600, Some(3400), WordSource::Hand),
            ]
        );
        assert_eq!((out.cues[0].start_ms, out.cues[0].end_ms), (900, 3400));
        assert!(out.validate().is_ok());
        // Syllables level: an uncut word is one unit (syllable 0), nothing more.
        let whole = apply_holds(&t, Level::Syllable, &[hold(0, 900, 1500)]).unwrap();
        assert_eq!(whole.cues[0].words.as_ref().unwrap()[0].end_ms, Some(1500));
        let later = Hold {
            syllable: 1,
            ..hold(0, 900, 1500)
        };
        assert!(apply_holds(&t, Level::Syllable, &[later]).is_none());
    }

    /// A game stopped mid-line: a held end is never cut short by a neighbour
    /// nobody took; that neighbour moves to the end, Estimated.
    #[test]
    fn a_held_end_pushes_the_untaken_units_it_covers() {
        let t = track();
        let hold = |word, syllable, start_ms, end_ms| Hold {
            cue: 0,
            word,
            syllable,
            start_ms,
            end_ms,
        };
        let out = apply_holds(&t, Level::Word, &[hold(0, 0, 1000, 1900)]).unwrap();
        let w = out.cues[0].words.as_ref().unwrap();
        assert_eq!((w[0].end_ms, w[0].source), (Some(1900), WordSource::Hand));
        assert_eq!(
            (w[1].start_ms, w[1].end_ms, w[1].source),
            (1900, None, WordSource::Estimated)
        );
        assert_eq!(w[2].start_ms, 2200, "a word the end does not reach stays");
        assert!(out.validate().is_ok());

        let mut t = track();
        propose_syllables(&mut t, 0..1, |w| match w {
            "Something" => vec!["Some".into(), "thing".into()],
            _ => vec![w.into()],
        });
        let out = apply_holds(&t, Level::Syllable, &[hold(0, 0, 1000, 1500)]).unwrap();
        let s = out.cues[0].words.as_ref().unwrap()[0]
            .syllables
            .clone()
            .unwrap();
        assert_eq!((s[0].start_ms, s[0].end_ms), (1000, Some(1500)));
        assert_eq!(
            (s[1].start_ms, s[1].end_ms, s[1].source),
            (1500, None, WordSource::Estimated)
        );
        assert!(out.validate().is_ok());
    }

    /// A previous line whose end ran on (a recognizer segment) never pushes the
    /// next line's first take later: its end is pulled back to the take.
    #[test]
    fn a_stale_previous_end_is_pulled_back_to_the_take() {
        let t = track();
        // Line 1 ends at 3000; its last word « give » starts at 2500.
        let hold = |start_ms, end_ms| Hold {
            cue: 1,
            word: 0,
            syllable: 0,
            start_ms,
            end_ms,
        };
        let out = apply_holds(&t, Level::Word, &[hold(2800, 3300)]).unwrap();
        assert_eq!(out.cues[0].end_ms, 2800);
        let first = &out.cues[1].words.as_ref().unwrap()[0];
        assert_eq!(
            (out.cues[1].start_ms, first.start_ms, first.source),
            (2800, 2800, WordSource::Hand)
        );
        assert_eq!(first.end_ms, Some(3300));
        // Not before the previous line's own last word (50 ms after « give »).
        let out = apply_holds(&t, Level::Word, &[hold(2400, 3300)]).unwrap();
        assert_eq!((out.cues[0].end_ms, out.cues[1].start_ms), (2550, 2550));
        // The Lines level pulls the same way.
        let out = apply_holds(&t, Level::Line, &[hold(2800, 4500)]).unwrap();
        assert_eq!((out.cues[0].end_ms, out.cues[1].start_ms), (2800, 2800));
        assert!(out.validate().is_ok());
    }

    #[test]
    fn syllable_holds_set_their_word_start_and_end() {
        let mut t = track();
        propose_syllables(&mut t, 0..1, |w| match w {
            "Something" => vec!["Some".into(), "thing".into()],
            _ => vec![w.into()],
        });
        let hold = |syllable, start_ms, end_ms| Hold {
            cue: 0,
            word: 0,
            syllable,
            start_ms,
            end_ms,
        };
        let out = apply_holds(
            &t,
            Level::Syllable,
            &[hold(0, 1000, 1300), hold(1, 1350, 1700)],
        )
        .unwrap();
        let w = &out.cues[0].words.as_ref().unwrap()[0];
        let s = w.syllables.as_ref().unwrap();
        // 50 ms apart: legato closes « Some » onto « thing ».
        assert_eq!((s[0].start_ms, s[0].end_ms), (1000, Some(1350)));
        assert_eq!((s[1].start_ms, s[1].end_ms), (1350, Some(1700)));
        assert_eq!((w.start_ms, w.end_ms), (1000, Some(1700)));
    }

    #[test]
    fn the_game_waits_on_a_passed_unit_and_never_past_one_line() {
        let g = |cue, start_ms, end_ms| Guide {
            cue,
            start_ms,
            end_ms,
        };
        let guides = [g(0, 1000, 1400), g(0, 1400, 2000), g(1, 3000, 3500)];
        // No press: 1.5 s after the expected unit's guide end.
        assert!(!passed(2900, &guides, 0, Level::Word, false));
        assert!(passed(2901, &guides, 0, Level::Word, false));
        // A press after the next line started (+0.3 s) while this line has units left.
        assert!(!passed(3300, &guides, 1, Level::Word, true));
        assert!(passed(3301, &guides, 1, Level::Word, true));
        // Line level: every unit is a line, the press rule never applies.
        assert!(!passed(9000, &guides, 1, Level::Line, true));
        // A slip inside a line is not detected.
        assert!(!passed(1900, &guides, 0, Level::Word, true));
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
        assert!(join_next_with(&mut t, 0, " "));
        assert_eq!(ends(&t, 0), [Some(1600), Some(2150), None, None]);
        assert!(t.validate().is_ok());

        // A text edit keeps unchanged words' ends; a new word has none.
        let mut t = base.clone();
        assert!(set_text(&mut t, 0, "Something had to live"));
        assert_eq!(ends(&t, 0), [Some(1600), Some(2100), None, None]);

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
            voices: Vec::new(),
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
            voices: Vec::new(),
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
        assert!(join_next_with(&mut t, 0, " "));
        assert_eq!(t.cues[0].text, "Something had to give");
        assert_eq!(starts(&t, 0).len(), 4);
        assert!(t.validate().is_ok());
        // Without word times the letters decide where the text is cut.
        assert!(split_at(&mut t, 1, 5000));
        assert_eq!(t.cues[1].text, "and nothing");
        assert!(!split_at(&mut t, 1, 4100), "each part keeps 200 ms");
    }

    #[test]
    fn cut_lines_keep_their_voices_and_joined_lines_keep_the_first_ones() {
        let mut t = SubtitleTrack {
            voices: vec![crate::subtitle_track::Voice::default(); 2],
            language: None,
            duration_ms: 10_000,
            cues: vec![
                SubtitleCue::new(1000, 3000, "a b c"),
                SubtitleCue::new(3000, 4000, "d"),
            ],
        };
        t.cues[0].voices = vec![1, 2];
        t.cues[1].voices = vec![2];
        assert!(split_at_word(&mut t, 0, 1));
        assert_eq!(t.cues[0].voices, [1, 2]);
        assert_eq!(t.cues[1].voices, [1, 2]);
        assert!(join_next_with(&mut t, 1, " "));
        assert_eq!(t.cues[1].text, "b c d");
        assert_eq!(t.cues[1].voices, [1, 2], "the first line's voices");
    }

    #[test]
    fn voices_are_set_on_lines_and_declared_voices_can_shrink() {
        let mut t = SubtitleTrack {
            voices: vec![crate::subtitle_track::Voice::default(); 2],
            language: None,
            duration_ms: 20_000,
            cues: vec![
                SubtitleCue::new(1000, 2000, "a"),
                SubtitleCue::new(3000, 4000, "b"),
                SubtitleCue::new(9000, 10_000, "c"),
            ],
        };
        assert!(set_voices(&mut t, 0, 1, &[2, 1, 2]));
        assert_eq!(t.cues[0].voices, [1, 2]);
        assert_eq!(t.cues[1].voices, [1, 2]);
        assert!(!set_voices(&mut t, 0, 1, &[1, 2]), "nothing to change");
        assert!(!set_voices(&mut t, 2, 2, &[3]), "voice 3 is not declared");
        // A line added after another takes its voices.
        let i = add_cue(&mut t, 6000, "new").unwrap();
        assert_eq!(t.cues[i].voices, [1, 2]);
        assert_eq!(lines_with_voice(&t, 2), 3);
        // Declaring one voice takes voice 2 from every line.
        assert!(declare_voices(
            &mut t,
            vec![crate::subtitle_track::Voice::default()]
        ));
        assert!(t.cues.iter().all(|c| !c.voices.contains(&2)));
        assert!(!declare_voices(
            &mut t,
            vec![crate::subtitle_track::Voice::default(); 5]
        ));
    }

    fn duet() -> SubtitleTrack {
        SubtitleTrack {
            voices: Vec::new(),
            language: None,
            duration_ms: 20_000,
            cues: vec![SubtitleCue::new(1000, 5000, "jusqu'au matin (au matin)")],
        }
    }

    fn take(word: usize, start_ms: u64, end_ms: u64) -> Hold {
        Hold {
            cue: 0,
            word,
            syllable: 0,
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn a_main_word_held_over_the_backing_vocals_keeps_its_end_and_does_not_push_them() {
        let t = duet();
        let out =
            apply_holds(&t, Level::Word, &[take(0, 1000, 1800), take(1, 1900, 4600)]).unwrap();
        let w = out.cues[0].words.as_ref().unwrap();
        assert_eq!(
            (w[1].start_ms, w[1].end_ms),
            (1900, Some(4600)),
            "matin held to 4600"
        );
        // The backing vocals were not taken and not pushed to 4600: still estimated,
        // after « matin » starts, in text order.
        assert_eq!(w[2].source, WordSource::Estimated);
        assert!(w[2].start_ms > 1900 && w[2].start_ms < 4600, "{:?}", w[2]);
        assert!(w[3].start_ms > w[2].start_ms);
        // The main pass never takes a backing vocal.
        assert!(apply_holds(&t, Level::Word, &[take(2, 2000, 2500)]).is_none());
    }

    #[test]
    fn the_backing_pass_takes_backing_vocals_only_and_never_moves_a_main_word() {
        let t = duet();
        let main =
            apply_holds(&t, Level::Word, &[take(0, 1000, 1800), take(1, 1900, 4600)]).unwrap();
        let out = apply_holds(
            &main,
            Level::Backing,
            &[take(2, 3000, 3500), take(3, 3600, 4900)],
        )
        .unwrap();
        let w = out.cues[0].words.as_ref().unwrap();
        assert_eq!(
            (w[0].start_ms, w[1].start_ms, w[1].end_ms),
            (1000, 1900, Some(4600))
        );
        assert_eq!((w[2].start_ms, w[2].source), (3000, WordSource::Hand));
        assert_eq!((w[3].start_ms, w[3].end_ms), (3600, Some(4900)));
        assert!(
            out.cues[0].has_backing_overlap(w),
            "the held word runs past them"
        );
        assert!(apply_holds(&main, Level::Backing, &[take(1, 3000, 3500)]).is_none());
        // A backing vocal tapped before the main word it follows stays after it.
        let early = apply_holds(&main, Level::Backing, &[take(2, 1200, 1500)]).unwrap();
        let w = early.cues[0].words.as_ref().unwrap();
        assert_eq!(w[2].start_ms, 1900 + MIN_WORD_GAP_MS);
        assert_eq!(w[1].start_ms, 1900, "the main word did not move");
    }

    #[test]
    fn untaken_backing_vocals_out_of_order_are_spread_again_between_their_neighbours() {
        let mut t = duet();
        t.cues[0] = SubtitleCue::new(1000, 5000, "a (b c) d");
        // « d » is taken early: « (b c) » would sit after it.
        let out =
            apply_holds(&t, Level::Word, &[take(0, 1000, 1200), take(3, 1600, 4000)]).unwrap();
        let w = out.cues[0].words.as_ref().unwrap();
        assert_eq!((w[3].start_ms, w[3].source), (1600, WordSource::Hand));
        assert!(
            w[1].start_ms >= 1000 + MIN_WORD_GAP_MS && w[2].start_ms + MIN_WORD_GAP_MS <= 1600,
            "{w:?}"
        );
    }
}
