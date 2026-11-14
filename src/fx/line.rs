//! Timed caption lines and words. Word times come from the SRT cue split evenly,
//! weighted by letter count, until word-level alignment exists.
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// A word cut into syllables (Task 41 S6), in order; empty otherwise.
    pub syllables: Vec<Syllable>,
}

/// A syllable of a word: its first character in the word, and its time.
#[derive(Clone, Debug, PartialEq)]
pub struct Syllable {
    pub first_char: usize,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
    /// Joiner between words when laying out (" " for Latin, "" for Japanese).
    pub sep: String,
    /// Indices of the words that start a line typed in the cue (never 0), for
    /// effects that keep the cue's own line breaks (`layout.line_breaks`).
    pub breaks: Vec<usize>,
}

/// Where a cue's own lines start, as indices of its whitespace-separated words:
/// cumulative word counts at each typed line that has words, never 0 ("a b\nc d"
/// gives [2]). Lines with no word add no break.
pub fn typed_breaks(text: &str) -> Vec<usize> {
    let mut breaks = Vec::new();
    let mut count = 0;
    for line in text.lines() {
        let n = line.split_whitespace().count();
        if n > 0 && count > 0 {
            breaks.push(count);
        }
        count += n;
    }
    breaks
}

/// Even split of the line duration across words, weighted by letter count.
pub fn split_words(start: f64, end: f64, words: &[String], sep: &str) -> Line {
    let weight = |w: &String| w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64;
    let total: f64 = words.iter().map(weight).sum::<f64>().max(1.0);
    let mut t = start;
    let words = words
        .iter()
        .map(|w| {
            let d = (end - start) * weight(w) / total;
            let word = Word {
                text: w.clone(),
                start: t,
                end: t + d,
                syllables: Vec::new(),
            };
            t += d;
            word
        })
        .collect();
    Line {
        start,
        end,
        words,
        sep: sep.into(),
        breaks: Vec::new(),
    }
}

/// A cue's line with its known word times: each word ends where the next starts.
fn timed_line(
    c: &crate::subtitle_track::SubtitleCue,
    timed: &[crate::subtitle_track::WordTime],
) -> Line {
    let s = |ms: u64| ms as f64 / 1000.0;
    let words = timed
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let end = s(timed.get(i + 1).map_or(c.end_ms, |n| n.start_ms));
            let mut first_char = 0;
            let syl = w.syllables.iter().flatten().collect::<Vec<_>>();
            let syllables = syl
                .iter()
                .enumerate()
                .map(|(k, y)| {
                    let one = Syllable {
                        first_char,
                        start: s(y.start_ms),
                        end: syl.get(k + 1).map_or(end, |n| s(n.start_ms)),
                    };
                    first_char += y.text.chars().count();
                    one
                })
                .collect();
            Word {
                text: w.text.clone(),
                start: s(w.start_ms),
                end,
                syllables,
            }
        })
        .collect();
    Line {
        start: s(c.start_ms),
        end: s(c.end_ms),
        words,
        sep: " ".into(),
        breaks: Vec::new(),
    }
}

/// Lines from subtitle cues on the caption clock (seconds). Words are separated by
/// white space (a cue without spaces, Chinese or Japanese, is one word). A cue
/// with word times uses them; without, its duration is split by letters.
pub fn from_track(track: &crate::subtitle_track::SubtitleTrack) -> Vec<Line> {
    track
        .cues
        .iter()
        .filter_map(|c| {
            let words: Vec<String> = c.text.split_whitespace().map(str::to_string).collect();
            (!words.is_empty()).then(|| {
                let mut line = match &c.words {
                    Some(timed) if c.words_fit(timed) => timed_line(c, timed),
                    _ => split_words(
                        c.start_ms as f64 / 1000.0,
                        c.end_ms as f64 / 1000.0,
                        &words,
                        " ",
                    ),
                };
                // Both branches carry the text's own words (words_fit requires the
                // same words), so the text's breaks always index them.
                line.breaks = typed_breaks(&c.text);
                line
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::subtitle_track::{SubtitleCue, SubtitleTrack, WordSource, WordTime};

    /// Task 43 step C: a cue's typed lines, as word indices.
    #[test]
    fn typed_breaks_count_the_words_of_each_line() {
        assert_eq!(super::typed_breaks("a b\nc d"), [2]);
        assert_eq!(super::typed_breaks("a b\r\nc d"), [2]);
        assert_eq!(super::typed_breaks("a b\n   \nc d"), [2]);
        assert_eq!(super::typed_breaks("a b\nc\nd e f"), [2, 3]);
        assert!(super::typed_breaks("a b").is_empty());
        let line = |cue: SubtitleCue| {
            super::from_track(&SubtitleTrack {
                language: None,
                duration_ms: 5000,
                cues: vec![cue],
            })
            .remove(0)
        };
        assert_eq!(
            line(SubtitleCue::new(1000, 3000, "Something had\nto give")).breaks,
            [2]
        );
        // Timed words matching the text keep the breaks; mismatched ones rewrap.
        let mut timed = SubtitleCue::new(1000, 3000, "Something had\nto give");
        let w = |text: &str, start_ms| WordTime {
            text: text.into(),
            start_ms,
            source: WordSource::Hand,
            syllables: None,
        };
        timed.words = Some(vec![
            w("Something", 1000),
            w("had", 1500),
            w("to", 2000),
            w("give", 2500),
        ]);
        assert_eq!(line(timed.clone()).breaks, [2]);
        timed.text = "Something had\nto give it".into();
        timed.words = Some(vec![
            w("Something", 1000),
            w("had", 1500),
            w("to", 2000),
            w("give", 2500),
        ]);
        // Words that no longer fit the text: the letter split of the text's words,
        // whose breaks are the text's.
        let refit = line(timed);
        assert_eq!((refit.words.len(), refit.breaks.clone()), (5, vec![2]));
    }

    #[test]
    fn a_word_cut_into_syllables_carries_their_times() {
        use crate::subtitle_track::SyllableTime;
        let mut cue = SubtitleCue::new(1000, 3000, "Something had");
        let syl = |text: &str, start_ms| SyllableTime {
            text: text.into(),
            start_ms,
            source: WordSource::Hand,
        };
        cue.words = Some(vec![
            WordTime {
                text: "Something".into(),
                start_ms: 1000,
                source: WordSource::Hand,
                syllables: Some(vec![syl("Some", 1000), syl("thing", 1700)]),
            },
            WordTime {
                text: "had".into(),
                start_ms: 2600,
                source: WordSource::Whisper,
                syllables: None,
            },
        ]);
        let track = SubtitleTrack {
            language: None,
            duration_ms: 5000,
            cues: vec![cue],
        };
        let lines = super::from_track(&track);
        assert_eq!(
            lines[0].words[0].syllables,
            [
                super::Syllable {
                    first_char: 0,
                    start: 1.0,
                    end: 1.7
                },
                super::Syllable {
                    first_char: 4,
                    start: 1.7,
                    end: 2.6
                }
            ]
        );
        assert!(lines[0].words[1].syllables.is_empty());
    }

    #[test]
    fn known_word_times_replace_the_letter_split() {
        let mut cue = SubtitleCue::new(1000, 3000, "Something had");
        cue.words = Some(vec![
            WordTime {
                text: "Something".into(),
                start_ms: 1000,
                source: WordSource::Hand,
                syllables: None,
            },
            WordTime {
                text: "had".into(),
                start_ms: 2600,
                source: WordSource::Whisper,
                syllables: None,
            },
        ]);
        let track = SubtitleTrack {
            language: None,
            duration_ms: 5000,
            cues: vec![cue, SubtitleCue::new(3000, 4000, "a bb")],
        };
        let lines = super::from_track(&track);
        assert_eq!(
            lines[0]
                .words
                .iter()
                .map(|w| (w.start, w.end))
                .collect::<Vec<_>>(),
            [(1.0, 2.6), (2.6, 3.0)]
        );
        // No times: the letter split, as before (1 and 2 letters).
        assert!((lines[1].words[1].start - (3.0 + 1.0 / 3.0)).abs() < 1e-9);
    }
}
