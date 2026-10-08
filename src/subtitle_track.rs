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
                (start < end).then(|| SubtitleCue {
                    start_ms: start - start_ms,
                    end_ms: end - start_ms,
                    text: cue.text.clone(),
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
                },
                SubtitleCue {
                    start_ms: 100,
                    end_ms: 200,
                    text: "相接する一".into(),
                },
                SubtitleCue {
                    start_ms: 200,
                    end_ms: 500,
                    text: "Café 👋".into(),
                },
                SubtitleCue {
                    start_ms: 500,
                    end_ms: 600,
                    text: "右端を越える".into(),
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
