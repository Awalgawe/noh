//! Validated, backend-independent subtitle timing and plain UTF-8 SRT output.
//!
//! Times are integer milliseconds. The two-digit SRT hour field bounds source
//! duration to 99:59:59,999. Text is preserved, except that CRLF becomes LF when
//! serialized; lone CR and other controls besides LF/tab are rejected. Every
//! text line must contain a non-whitespace character, including the last line.
use serde::{Deserialize, Serialize};
use std::fmt::Write;

pub const MAX_DURATION_MS: u64 = 359_999_999;
pub const MAX_CUES: usize = 10_000;
pub const MAX_CUE_TEXT_BYTES: usize = 4096;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleTrack {
    /// Optional caller/backend language tag; no language is inferred here.
    /// Tags contain 2–63 ASCII letters/hyphens with nonempty letter segments.
    pub language: Option<String>,
    pub duration_ms: u64,
    pub cues: Vec<SubtitleCue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// When each word starts, in order (None: effects split the cue by letters).
    /// The words' texts are the cue text split on whitespace; see `words_fit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<WordTime>>,
}

/// Where a word or syllable time comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordSource {
    /// Rebuilt from the recognizer's per-token times.
    Whisper,
    /// Placed by hand.
    Hand,
    /// Spread by letter count between known neighbours.
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WordTime {
    pub text: String,
    pub start_ms: u64,
    /// When the word stops being sung, if known (a held word, or a breath before
    /// the next one). None: it lasts until the next word starts (or the cue ends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    pub source: WordSource,
    /// A word cut into syllables (two or more), each with its own start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syllables: Option<Vec<SyllableTime>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyllableTime {
    pub text: String,
    pub start_ms: u64,
    /// As `WordTime::end_ms`, within its word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    pub source: WordSource,
}

/// The smallest gap between two word (or syllable) starts.
pub const MIN_WORD_GAP_MS: u64 = 50;

impl SubtitleCue {
    /// A cue without word times.
    pub fn new(start_ms: u64, end_ms: u64, text: impl Into<String>) -> Self {
        Self {
            start_ms,
            end_ms,
            text: text.into(),
            words: None,
        }
    }

    /// Clamp the known ends of `words` to where each unit may end: a word before
    /// the next word's start (or the cue end), a syllable before the next
    /// syllable (or its word's end). An end that no longer follows its start is
    /// dropped, so the unit lasts until the next start again.
    pub fn fit_ends(&self, words: &mut [WordTime]) {
        let starts: Vec<u64> = words.iter().map(|w| w.start_ms).collect();
        for (i, w) in words.iter_mut().enumerate() {
            let bound = starts.get(i + 1).copied().unwrap_or(self.end_ms);
            w.end_ms = w.end_ms.map(|e| e.min(bound)).filter(|e| *e > w.start_ms);
            if let Some(syllables) = &mut w.syllables {
                // A word end at or before its last syllable's start would hide it.
                let last_start = syllables.last().map_or(w.start_ms, |s| s.start_ms);
                w.end_ms = w.end_ms.filter(|e| *e > last_start);
                let word_end = w.end_ms.unwrap_or(bound);
                let syl_starts: Vec<u64> = syllables.iter().map(|s| s.start_ms).collect();
                for (k, s) in syllables.iter_mut().enumerate() {
                    let b = syl_starts.get(k + 1).copied().unwrap_or(word_end);
                    s.end_ms = s.end_ms.map(|e| e.min(b)).filter(|e| *e > s.start_ms);
                }
            }
        }
    }

    /// Whether `words` belong to this cue: the same texts as the cue's
    /// whitespace split, starts inside [start, end - 50 ms] and at least 50 ms
    /// apart; syllables (two or more) spell their word, start with it and stay
    /// before the next word, 50 ms apart. A known end follows its start and
    /// stays before the next unit's start (or the cue end, or its word's end).
    pub fn words_fit(&self, words: &[WordTime]) -> bool {
        let mut fitted = words.to_vec();
        self.fit_ends(&mut fitted);
        if fitted != words {
            return false;
        }
        let split: Vec<&str> = self.text.split_whitespace().collect();
        if words.is_empty()
            || words.len() != split.len()
            || words.iter().zip(&split).any(|(w, s)| w.text != *s)
        {
            return false;
        }
        let last_allowed = self.end_ms.saturating_sub(MIN_WORD_GAP_MS);
        let mut previous: Option<u64> = None;
        for (i, w) in words.iter().enumerate() {
            if w.start_ms < self.start_ms
                || w.start_ms > last_allowed
                || previous.is_some_and(|p| w.start_ms < p + MIN_WORD_GAP_MS)
            {
                return false;
            }
            previous = Some(w.start_ms);
            if let Some(syllables) = &w.syllables {
                let next = words.get(i + 1).map_or(self.end_ms, |n| n.start_ms);
                if syllables.len() < 2
                    || syllables[0].start_ms != w.start_ms
                    || syllables
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<String>()
                        != w.text
                    || syllables
                        .windows(2)
                        .any(|p| p[1].start_ms < p[0].start_ms + MIN_WORD_GAP_MS)
                    || syllables
                        .last()
                        .is_some_and(|s| s.start_ms + MIN_WORD_GAP_MS > next)
                {
                    return false;
                }
                previous = syllables.last().map(|s| s.start_ms);
            }
        }
        true
    }
}

/// Cue indices in validation errors are one-based, matching SRT numbering.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error(
        "Source duration must be between 1 and {} milliseconds",
        MAX_DURATION_MS
    )]
    Duration,
    #[error("Language must be a 2–63 byte ASCII alphabetic/hyphen tag with nonempty segments")]
    Language,
    #[error("Subtitle track exceeds {} cues", MAX_CUES)]
    CueCount,
    #[error("Cue {cue} must end after its start and within the source duration")]
    Timing { cue: usize },
    #[error("Cue {cue} is out of order or overlaps the preceding cue")]
    Order { cue: usize },
    #[error("Cue {cue} exceeds {} UTF-8 text bytes", MAX_CUE_TEXT_BYTES)]
    CueTextLimit { cue: usize },
    #[error("Subtitle track exceeds {} UTF-8 text bytes", MAX_TEXT_BYTES)]
    TrackTextLimit,
    #[error("Cue {cue} contains an empty or whitespace-only text line")]
    EmptyLine { cue: usize },
    #[error("Cue {cue} contains a disallowed control character or lone carriage return")]
    ControlCharacter { cue: usize },
    #[error("Cue {cue} has word times that do not match its text or timing")]
    Words { cue: usize },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrimError {
    #[error("Trim range must satisfy 0 <= start < end <= source duration")]
    Range,
    #[error("Invalid source subtitle track: {0}")]
    Track(#[from] ValidationError),
}

impl SubtitleTrack {
    /// Validate without allocating, trimming, shifting, sorting or guessing.
    /// Adjacent cues and an empty track are valid. Limits count original UTF-8
    /// bytes, including CRLF, before any serialization normalization.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validated_text_bytes().map(|_| ())
    }

    /// Return the cues intersecting the half-open source range `[start_ms, end_ms)`.
    /// The source is validated before range checks; touching-only cues are omitted.
    pub fn trim(&self, start_ms: u64, end_ms: u64) -> Result<Self, TrimError> {
        self.validate().map_err(TrimError::Track)?;
        if start_ms >= end_ms || end_ms > self.duration_ms {
            return Err(TrimError::Range);
        }

        let cues = self
            .cues
            .iter()
            .filter_map(|cue| {
                let start = cue.start_ms.max(start_ms);
                let end = cue.end_ms.min(end_ms);
                (start < end).then(|| {
                    let mut out =
                        SubtitleCue::new(start - start_ms, end - start_ms, cue.text.clone());
                    out.words = cue.words.as_ref().and_then(|words| {
                        let shifted: Option<Vec<WordTime>> = words
                            .iter()
                            .map(|w| {
                                let mut w = w.clone();
                                w.start_ms = w.start_ms.checked_sub(start_ms)?;
                                w.end_ms = w.end_ms.map(|e| e.saturating_sub(start_ms));
                                if let Some(syllables) = &mut w.syllables {
                                    for s in syllables.iter_mut() {
                                        s.start_ms = s.start_ms.checked_sub(start_ms)?;
                                        s.end_ms = s.end_ms.map(|e| e.saturating_sub(start_ms));
                                    }
                                }
                                Some(w)
                            })
                            .collect();
                        // A cut end clamps the words' known ends to the new cue end.
                        shifted
                            .map(|mut w| {
                                out.fit_ends(&mut w);
                                w
                            })
                            .filter(|w| out.words_fit(w))
                    });
                    out
                })
            })
            .collect();
        Ok(Self {
            language: self.language.clone(),
            duration_ms: end_ms - start_ms,
            cues,
        })
    }

    fn validated_text_bytes(&self) -> Result<usize, ValidationError> {
        if self.duration_ms == 0 || self.duration_ms > MAX_DURATION_MS {
            return Err(ValidationError::Duration);
        }
        if let Some(language) = &self.language
            && (!(2..=63).contains(&language.len())
                || language.split('-').any(|part| {
                    part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphabetic())
                }))
        {
            return Err(ValidationError::Language);
        }
        if self.cues.len() > MAX_CUES {
            return Err(ValidationError::CueCount);
        }
        let (mut previous_end, mut text_bytes) = (0, 0usize);
        for (index, cue) in self.cues.iter().enumerate() {
            let number = index + 1;
            if cue.end_ms <= cue.start_ms || cue.end_ms > self.duration_ms {
                return Err(ValidationError::Timing { cue: number });
            }
            if cue.start_ms < previous_end {
                return Err(ValidationError::Order { cue: number });
            }
            previous_end = cue.end_ms;
            if cue.text.len() > MAX_CUE_TEXT_BYTES {
                return Err(ValidationError::CueTextLimit { cue: number });
            }
            text_bytes = text_bytes
                .checked_add(cue.text.len())
                .filter(|&bytes| bytes <= MAX_TEXT_BYTES)
                .ok_or(ValidationError::TrackTextLimit)?;
            let mut characters = cue.text.chars().peekable();
            while let Some(character) = characters.next() {
                if (character == '\r' && characters.peek() != Some(&'\n'))
                    || (character.is_control() && !matches!(character, '\r' | '\n' | '\t'))
                {
                    return Err(ValidationError::ControlCharacter { cue: number });
                }
            }
            // split, unlike lines(), retains a trailing empty line. CR is legal
            // only immediately before LF and is whitespace for this check.
            if cue.text.split('\n').any(|line| line.trim().is_empty()) {
                return Err(ValidationError::EmptyLine { cue: number });
            }
            if cue
                .words
                .as_ref()
                .is_some_and(|words| !cue.words_fit(words))
            {
                return Err(ValidationError::Words { cue: number });
            }
        }
        Ok(text_bytes)
    }

    /// Serialize validated cues as numbered, unstyled SRT with LF line endings
    /// and one blank line after each cue. Silence serializes to an empty string.
    /// This never synthesizes timing or changes cue order/text whitespace.
    pub fn to_srt(&self) -> Result<String, ValidationError> {
        let text_bytes = self.validated_text_bytes()?;
        // Validation bounds this arithmetic to less than 650 KiB, including
        // five-digit numbering and timestamps, before allocating the result.
        let mut output = String::with_capacity(text_bytes + self.cues.len() * 38);
        for (index, cue) in self.cues.iter().enumerate() {
            writeln!(output, "{}", index + 1).expect("Writing to a String is infallible");
            timestamp(&mut output, cue.start_ms);
            output.push_str(" --> ");
            timestamp(&mut output, cue.end_ms);
            output.push('\n');
            // Validation established that every CR belongs to a CRLF pair.
            output.extend(cue.text.chars().filter(|&character| character != '\r'));
            output.push_str("\n\n");
        }
        Ok(output)
    }
}

fn timestamp(output: &mut String, milliseconds: u64) {
    write!(
        output,
        "{:02}:{:02}:{:02},{:03}",
        milliseconds / 3_600_000,
        milliseconds / 60_000 % 60,
        milliseconds / 1000 % 60,
        milliseconds % 1000
    )
    .expect("Writing to a String is infallible");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worded(start: u64, end: u64, text: &str, starts: &[u64]) -> SubtitleCue {
        let mut cue = SubtitleCue::new(start, end, text);
        cue.words = Some(
            text.split_whitespace()
                .zip(starts)
                .map(|(w, t)| WordTime {
                    text: w.into(),
                    start_ms: *t,
                    end_ms: None,
                    source: WordSource::Whisper,
                    syllables: None,
                })
                .collect(),
        );
        cue
    }

    #[test]
    fn word_times_must_spell_the_cue_and_stay_inside_it_50_ms_apart() {
        let ok = worded(1000, 2000, "la la", &[1000, 1500]);
        assert!(ok.words_fit(ok.words.as_ref().unwrap()));
        for bad in [
            worded(1000, 2000, "la la", &[1000, 1040]),
            worded(1000, 2000, "la la", &[900, 1500]),
            worded(1000, 2000, "la la", &[1000, 1960]),
            worded(1000, 2000, "la la", &[1000]),
        ] {
            assert!(!bad.words_fit(bad.words.as_ref().unwrap()), "{bad:?}");
            let track = SubtitleTrack {
                language: None,
                duration_ms: 5000,
                cues: vec![bad],
            };
            assert_eq!(track.validate(), Err(ValidationError::Words { cue: 1 }));
        }
        let mut syl = worded(1000, 3000, "something had", &[1000, 2000]);
        syl.words.as_mut().unwrap()[0].syllables = Some(vec![
            SyllableTime {
                text: "some".into(),
                start_ms: 1000,
                end_ms: None,
                source: WordSource::Hand,
            },
            SyllableTime {
                text: "thing".into(),
                start_ms: 1500,
                end_ms: None,
                source: WordSource::Hand,
            },
        ]);
        assert!(syl.words_fit(syl.words.as_ref().unwrap()));
        syl.words.as_mut().unwrap()[0].syllables.as_mut().unwrap()[1].text = "thin".into();
        assert!(!syl.words_fit(syl.words.as_ref().unwrap()));
    }

    #[test]
    fn known_ends_follow_their_start_and_stay_before_the_next_unit() {
        let mut cue = worded(1000, 3000, "a b", &[1000, 2000]);
        let mut w = cue.words.clone().unwrap();
        w[0].end_ms = Some(1800);
        w[1].end_ms = Some(3000);
        assert!(cue.words_fit(&w));
        // Past the next start, at or before its own start: refused...
        for bad in [2001, 1000, 900] {
            let mut x = w.clone();
            x[0].end_ms = Some(bad);
            assert!(!cue.words_fit(&x), "end {bad}");
        }
        // ...and fit_ends clamps the first and drops the others.
        let mut x = w.clone();
        x[0].end_ms = Some(2500);
        x[1].end_ms = Some(3500);
        cue.fit_ends(&mut x);
        assert_eq!((x[0].end_ms, x[1].end_ms), (Some(2000), Some(3000)));
        x[0].end_ms = Some(1000);
        cue.fit_ends(&mut x);
        assert_eq!(x[0].end_ms, None);
        // A word's end must leave its last syllable a moment; a syllable ends
        // before the next one, or at its word's end.
        cue.text = "ab c".into();
        let mut s = vec![
            WordTime {
                text: "ab".into(),
                start_ms: 1000,
                end_ms: Some(1700),
                source: WordSource::Hand,
                syllables: Some(vec![
                    SyllableTime {
                        text: "a".into(),
                        start_ms: 1000,
                        end_ms: Some(1300),
                        source: WordSource::Hand,
                    },
                    SyllableTime {
                        text: "b".into(),
                        start_ms: 1500,
                        end_ms: Some(1900),
                        source: WordSource::Hand,
                    },
                ]),
            },
            w[1].clone(),
        ];
        s[1].text = "c".into();
        assert!(!cue.words_fit(&s), "syllable past its word's end");
        cue.fit_ends(&mut s);
        assert_eq!(s[0].syllables.as_ref().unwrap()[1].end_ms, Some(1700));
        assert!(cue.words_fit(&s));
        // Serialized only when known.
        let json = serde_json::to_string(&w[0]).unwrap();
        assert!(json.contains("\"end_ms\":1800"));
        let mut none = w[0].clone();
        none.end_ms = None;
        assert!(!serde_json::to_string(&none).unwrap().contains("end_ms"));
        // Trimming shifts ends and clamps them to the cut.
        cue = worded(1000, 3000, "a b", &[1000, 2000]);
        cue.words = Some(w.clone());
        let track = SubtitleTrack {
            language: None,
            duration_ms: 5000,
            cues: vec![cue],
        };
        let trimmed = track.trim(500, 2600).unwrap();
        let ends: Vec<Option<u64>> = trimmed.cues[0]
            .words
            .iter()
            .flatten()
            .map(|w| w.end_ms)
            .collect();
        assert_eq!(ends, [Some(1300), Some(2100)]);
    }

    #[test]
    fn trimming_shifts_word_times_and_drops_those_a_cut_leaves_outside() {
        let track = SubtitleTrack {
            language: None,
            duration_ms: 10_000,
            cues: vec![
                worded(1000, 3000, "a b", &[1000, 2000]),
                worded(4000, 6000, "c d", &[4000, 5000]),
            ],
        };
        let short = track.trim(500, 4500).unwrap();
        assert_eq!(
            short.cues[0]
                .words
                .as_ref()
                .unwrap()
                .iter()
                .map(|w| w.start_ms)
                .collect::<Vec<_>>(),
            [500, 1500]
        );
        assert!(short.cues[1].words.is_none(), "the cut leaves 'd' outside");
        // Without words, the JSON is unchanged (the field is skipped).
        let bare = SubtitleCue::new(0, 10, "x");
        assert!(!serde_json::to_string(&bare).unwrap().contains("words"));
    }

    fn track(cues: impl IntoIterator<Item = (u64, u64, String)>) -> SubtitleTrack {
        SubtitleTrack {
            language: None,
            duration_ms: MAX_DURATION_MS,
            cues: cues
                .into_iter()
                .map(|(start_ms, end_ms, text)| SubtitleCue {
                    start_ms,
                    end_ms,
                    text,
                    words: None,
                })
                .collect(),
        }
    }

    #[test]
    fn srt_uses_exact_integer_milliseconds_at_clock_boundaries() {
        let input = track([
            (0, 1, "First".into()),
            (999, 1000, "Second".into()),
            (59_999, 60_000, "Minute".into()),
            (3_599_999, 3_600_000, "Hour".into()),
            (MAX_DURATION_MS - 1, MAX_DURATION_MS, "Last".into()),
        ]);
        assert_eq!(
            input.to_srt().unwrap(),
            "1\n00:00:00,000 --> 00:00:00,001\nFirst\n\n\
             2\n00:00:00,999 --> 00:00:01,000\nSecond\n\n\
             3\n00:00:59,999 --> 00:01:00,000\nMinute\n\n\
             4\n00:59:59,999 --> 01:00:00,000\nHour\n\n\
             5\n99:59:59,998 --> 99:59:59,999\nLast\n\n"
        );
    }

    #[test]
    fn silence_and_adjacent_cues_are_valid_without_synthetic_content() {
        let silence = track([]);
        assert!(silence.validate().is_ok());
        assert_eq!(silence.to_srt().unwrap(), "");
        let adjacent = track([(0, 1000, "One".into()), (1000, 2000, "Two".into())]);
        assert!(adjacent.validate().is_ok());
    }

    #[test]
    fn trim_intersects_boundary_and_adjacent_cues_without_changing_order_or_text() {
        let mut input = track([
            (0, 300, "左端を越える".into()),
            (300, 400, "相接する一".into()),
            (400, 700, "Café 👋".into()),
            (700, 900, "右端を越える".into()),
        ]);
        input.duration_ms = 1000;
        input.language = Some("ja-JP".into());
        let trimmed = input.trim(200, 800).unwrap();
        assert_eq!(trimmed.duration_ms, 600);
        assert_eq!(trimmed.language.as_deref(), Some("ja-JP"));
        assert_eq!(
            trimmed.cues,
            [
                SubtitleCue {
                    start_ms: 0,
                    end_ms: 100,
                    text: "左端を越える".into(),
                    words: None,
                },
                SubtitleCue {
                    start_ms: 100,
                    end_ms: 200,
                    text: "相接する一".into(),
                    words: None,
                },
                SubtitleCue {
                    start_ms: 200,
                    end_ms: 500,
                    text: "Café 👋".into(),
                    words: None,
                },
                SubtitleCue {
                    start_ms: 500,
                    end_ms: 600,
                    text: "右端を越える".into(),
                    words: None,
                },
            ]
        );
        assert!(trimmed.validate().is_ok());
    }

    #[test]
    fn trim_uses_half_open_boundaries_and_preserves_empty_and_full_tracks() {
        let mut input = track([
            (0, 100, "Before".into()),
            (100, 200, "At start".into()),
            (300, 400, "At end".into()),
            (400, 500, "After".into()),
        ]);
        input.duration_ms = 500;
        input.language = Some("en-US".into());
        let trimmed = input.trim(100, 300).unwrap();
        assert_eq!(trimmed.duration_ms, 200);
        assert_eq!(trimmed.cues.len(), 1);
        assert_eq!(trimmed.cues[0].start_ms, 0);
        assert_eq!(trimmed.cues[0].end_ms, 100);
        assert_eq!(trimmed.cues[0].text, "At start");

        let no_overlap = input.trim(200, 300).unwrap();
        assert_eq!(no_overlap.duration_ms, 100);
        assert!(no_overlap.cues.is_empty());
        assert_eq!(no_overlap.language.as_deref(), Some("en-US"));

        let empty = track([]);
        let empty_trimmed = empty.trim(2, 10).unwrap();
        assert_eq!(empty_trimmed.duration_ms, 8);
        assert!(empty_trimmed.cues.is_empty());
        assert!(empty_trimmed.validate().is_ok());

        let full = input.trim(0, input.duration_ms).unwrap();
        assert_eq!(full, input);
    }

    #[test]
    fn trim_rejects_invalid_ranges_and_invalid_original_tracks() {
        let mut input = track([(100, 200, "Cue".into())]);
        input.duration_ms = 500;
        for (start, end) in [
            (0, 0),
            (200, 100),
            (0, 501),
            (500, 501),
            (u64::MAX, u64::MAX),
        ] {
            assert_eq!(input.trim(start, end).unwrap_err(), TrimError::Range);
        }

        let mut invalid = input.clone();
        invalid.cues[0].end_ms = 99;
        assert_eq!(
            invalid.trim(0, 100).unwrap_err(),
            TrimError::Track(ValidationError::Timing { cue: 1 })
        );
        // The source track is validated before even an invalid requested range.
        assert_eq!(
            invalid.trim(10, 10).unwrap_err(),
            TrimError::Track(ValidationError::Timing { cue: 1 })
        );
    }

    #[test]
    fn unicode_and_whitespace_survive_with_crlf_normalized_only_on_output() {
        let input = track([(0, 1000, "  Café 👋\r\n字幕\t中文  ".into())]);
        let original = input.clone();
        assert_eq!(
            input.to_srt().unwrap(),
            "1\n00:00:00,000 --> 00:00:01,000\n  Café 👋\n字幕\t中文  \n\n"
        );
        assert_eq!(input, original);
        let json = serde_json::to_vec(&input).unwrap();
        assert_eq!(
            serde_json::from_slice::<SubtitleTrack>(&json).unwrap(),
            input
        );
    }

    #[test]
    fn source_and_cue_ranges_are_checked_without_overflow_or_reordering() {
        for duration in [0, MAX_DURATION_MS + 1, u64::MAX] {
            let mut input = track([]);
            input.duration_ms = duration;
            assert_eq!(input.to_srt().unwrap_err(), ValidationError::Duration);
        }
        for (start, end) in [
            (0, 0),
            (2, 1),
            (MAX_DURATION_MS, MAX_DURATION_MS + 1),
            (0, u64::MAX),
        ] {
            assert_eq!(
                track([(start, end, "Text".into())]).validate().unwrap_err(),
                ValidationError::Timing { cue: 1 }
            );
        }
        for second in [(999, 2000), (0, 500)] {
            let input = track([
                (0, 1000, "First".into()),
                (second.0, second.1, "Second".into()),
            ]);
            assert_eq!(
                input.validate().unwrap_err(),
                ValidationError::Order { cue: 2 }
            );
        }
        let reversed = track([(2000, 3000, "Later".into()), (0, 1000, "Earlier".into())]);
        let original = reversed.clone();
        assert_eq!(
            reversed.to_srt().unwrap_err(),
            ValidationError::Order { cue: 2 }
        );
        assert_eq!(reversed, original);
    }

    #[test]
    fn empty_lines_and_controls_cannot_inject_srt_separators() {
        for text in [
            "",
            " \t",
            "\u{2003}",
            "\nText",
            "Text\n",
            "Text\n\nMore",
            "Text\n \t\nMore",
            "Text\r\n",
        ] {
            assert_eq!(
                track([(0, 1, text.into())]).validate().unwrap_err(),
                ValidationError::EmptyLine { cue: 1 },
                "{text:?}"
            );
        }
        for text in ["Text\0", "Text\rMore", "Text\r", "Text\u{7f}", "Text\u{85}"] {
            assert_eq!(
                track([(0, 1, text.into())]).to_srt().unwrap_err(),
                ValidationError::ControlCharacter { cue: 1 },
                "{text:?}"
            );
        }
    }

    #[test]
    fn language_tags_have_explicit_minimal_grammar() {
        for language in [
            "en".into(),
            "en-US".into(),
            "zh-Hant".into(),
            "a-b".into(),
            "a".repeat(63),
        ] {
            let mut input = track([]);
            input.language = Some(language);
            assert!(input.validate().is_ok());
        }
        for language in [
            "".into(),
            "e".into(),
            " en".into(),
            "en_Us".into(),
            "en-US1".into(),
            "-en".into(),
            "en-".into(),
            "en--US".into(),
            "éé".into(),
            "a".repeat(64),
        ] {
            let mut input = track([]);
            input.language = Some(language);
            assert_eq!(input.validate().unwrap_err(), ValidationError::Language);
        }
    }

    #[test]
    fn cue_and_utf8_byte_limits_accept_the_boundary_and_reject_the_next_byte() {
        let boundary = track((0..MAX_CUES as u64).map(|start| (start, start + 1, "x".into())));
        assert!(boundary.to_srt().is_ok());
        let mut excess = boundary;
        excess.cues.push(SubtitleCue {
            start_ms: MAX_CUES as u64,
            end_ms: MAX_CUES as u64 + 1,
            text: "x".into(),
            words: None,
        });
        assert_eq!(excess.to_srt().unwrap_err(), ValidationError::CueCount);

        let mut boundary = track([(0, 1, "é".repeat(MAX_CUE_TEXT_BYTES / 2))]);
        assert!(boundary.validate().is_ok());
        boundary.cues[0].text.push('x');
        assert_eq!(
            boundary.to_srt().unwrap_err(),
            ValidationError::CueTextLimit { cue: 1 }
        );

        let boundary = track(
            (0..(MAX_TEXT_BYTES / MAX_CUE_TEXT_BYTES) as u64)
                .map(|start| (start, start + 1, "x".repeat(MAX_CUE_TEXT_BYTES))),
        );
        assert!(boundary.to_srt().is_ok());
        let mut excess = boundary;
        excess.cues.push(SubtitleCue {
            start_ms: 64,
            end_ms: 65,
            text: "x".into(),
            words: None,
        });
        assert_eq!(
            excess.to_srt().unwrap_err(),
            ValidationError::TrackTextLimit
        );
    }

    #[test]
    fn serde_rejects_negative_times_and_unknown_fields() {
        for json in [
            r#"{"duration_ms":-1,"cues":[]}"#,
            r#"{"duration_ms":1,"cues":[{"start_ms":-1,"end_ms":1,"text":"x"}]}"#,
            r#"{"duration_ms":1,"cues":[{"start_ms":0,"end_ms":-1,"text":"x"}]}"#,
            r#"{"duration_ms":1,"cues":[],"guess":true}"#,
            r#"{"duration_ms":1,"cues":[{"start_ms":0,"end_ms":1,"text":"x","guess":true}]}"#,
        ] {
            assert!(
                serde_json::from_str::<SubtitleTrack>(json).is_err(),
                "{json}"
            );
        }
        let input: SubtitleTrack = serde_json::from_str(r#"{"duration_ms":1,"cues":[]}"#).unwrap();
        assert_eq!(input.language, None);
        assert_eq!(input.to_srt().unwrap(), "");
    }
}
