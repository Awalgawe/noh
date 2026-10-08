//! Bounded plain SRT import. Text is literal; only CRLF becomes LF.
use crate::subtitle_track::{
    MAX_CUE_TEXT_BYTES, MAX_CUES, MAX_TEXT_BYTES, SubtitleCue, SubtitleTrack, ValidationError,
};

pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SrtError {
    #[error("SRT input exceeds {} UTF-8 bytes", MAX_INPUT_BYTES)]
    InputLimit,
    #[error("SRT block {block}, line {line}: {detail}")]
    Syntax {
        block: usize,
        line: usize,
        detail: &'static str,
    },
    #[error("SRT block {block}, line {line}: {source}")]
    Validation {
        block: usize,
        line: usize,
        #[source]
        source: ValidationError,
    },
}

/// Parse without changing timing, sorting cues, inferring language or styling text.
/// Identifiers are positive u32 values, independent of cue order and timing.
pub fn parse(text: &str, duration_ms: u64) -> Result<SubtitleTrack, SrtError> {
    // Bound the original input, including BOM/CRLF, before allocating anything.
    if text.len() > MAX_INPUT_BYTES {
        return Err(SrtError::InputLimit);
    }
    let mut track = SubtitleTrack {
        language: None,
        duration_ms,
        cues: Vec::new(),
    };
    track
        .validate()
        .map_err(|source| validation(1, 1, source))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n').enumerate();
    let mut locations = Vec::new();
    let mut total_text_bytes = 0usize;
    let mut block = 1;
    loop {
        let identifier = loop {
            let Some((index, raw)) = lines.next() else {
                break None;
            };
            let value = line(raw, block, index + 1)?;
            if !value.trim().is_empty() {
                break Some((index + 1, value.trim()));
            }
        };
        let Some((identifier_line, identifier)) = identifier else {
            break;
        };
        if !identifier.bytes().all(|byte| byte.is_ascii_digit())
            || !identifier.parse::<u32>().is_ok_and(|value| value > 0)
        {
            return Err(syntax(
                block,
                identifier_line,
                "Expected a positive numeric cue identifier within u32 range",
            ));
        }
        if track.cues.len() == MAX_CUES {
            return Err(validation(
                block,
                identifier_line,
                ValidationError::CueCount,
            ));
        }
        let Some((index, raw)) = lines.next() else {
            return Err(syntax(block, identifier_line + 1, "Missing timestamp line"));
        };
        let timing_line = index + 1;
        let timing = line(raw, block, timing_line)?;
        let timing_error = || {
            syntax(
                block,
                timing_line,
                "Expected hh:mm:ss,mmm --> hh:mm:ss,mmm without position metadata",
            )
        };
        let (start, end) = timing.split_once("-->").ok_or_else(timing_error)?;
        let start_ms = timestamp(start.trim()).ok_or_else(timing_error)?;
        let end_ms = timestamp(end.trim()).ok_or_else(timing_error)?;
        let text_line = timing_line + 1;
        let mut cue_text = String::new();
        for (index, raw) in lines.by_ref() {
            let value = line(raw, block, index + 1)?;
            if value.trim().is_empty() {
                break;
            }
            let separator = usize::from(!cue_text.is_empty());
            let added = value.len() + separator; // Original input is at most 1 MiB.
            if cue_text.len() + added > MAX_CUE_TEXT_BYTES {
                return Err(validation(
                    block,
                    index + 1,
                    ValidationError::CueTextLimit { cue: block },
                ));
            }
            if total_text_bytes + added > MAX_TEXT_BYTES {
                return Err(validation(
                    block,
                    index + 1,
                    ValidationError::TrackTextLimit,
                ));
            }
            total_text_bytes += added;
            if separator != 0 {
                cue_text.push('\n');
            }
            cue_text.push_str(value);
        }
        if cue_text.is_empty() {
            return Err(syntax(block, text_line, "Expected nonempty subtitle text"));
        }
        locations.push((timing_line, text_line));
        track.cues.push(SubtitleCue {
            start_ms,
            end_ms,
            text: cue_text,
        });
        block += 1;
    }
    track.validate().map_err(|source| {
        let cue = match &source {
            ValidationError::Timing { cue }
            | ValidationError::Order { cue }
            | ValidationError::CueTextLimit { cue }
            | ValidationError::EmptyLine { cue }
            | ValidationError::ControlCharacter { cue } => *cue,
            _ => 1,
        };
        let (timing_line, text_line) = locations.get(cue - 1).copied().unwrap_or((1, 1));
        let line = match &source {
            ValidationError::Timing { .. } | ValidationError::Order { .. } => timing_line,
            ValidationError::ControlCharacter { .. } => {
                let text = &track.cues[cue - 1].text;
                let offset = text
                    .char_indices()
                    .find(|(_, character)| {
                        character.is_control() && !matches!(*character, '\n' | '\t')
                    })
                    .map(|(offset, _)| offset)
                    .unwrap_or(0);
                text_line + text[..offset].bytes().filter(|byte| *byte == b'\n').count()
            }
            _ => text_line,
        };
        validation(cue, line, source)
    })?;
    Ok(track)
}

fn line(raw: &str, block: usize, number: usize) -> Result<&str, SrtError> {
    let value = if let Some(value) = raw.strip_suffix('\n') {
        value.strip_suffix('\r').unwrap_or(value)
    } else {
        raw
    };
    if value.contains('\r') {
        return Err(syntax(
            block,
            number,
            "Lone carriage return; use LF or CRLF line endings",
        ));
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\t')
    {
        return Err(validation(
            block,
            number,
            ValidationError::ControlCharacter { cue: block },
        ));
    }
    Ok(value)
}

fn timestamp(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 12
        || bytes[2] != b':'
        || bytes[5] != b':'
        || bytes[8] != b','
        || [0, 1, 3, 4, 6, 7, 9, 10, 11]
            .iter()
            .any(|&index| !bytes[index].is_ascii_digit())
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .fold(0u64, |value, byte| value * 10 + u64::from(byte - b'0'))
    };
    let (hours, minutes, seconds, millis) =
        (number(0..2), number(3..5), number(6..8), number(9..12));
    (minutes < 60 && seconds < 60)
        .then_some(((hours * 60 + minutes) * 60 + seconds) * 1000 + millis)
}

fn syntax(block: usize, line: usize, detail: &'static str) -> SrtError {
    SrtError::Syntax {
        block,
        line,
        detail,
    }
}
fn validation(block: usize, line: usize, source: ValidationError) -> SrtError {
    SrtError::Validation {
        block,
        line,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle_track::MAX_DURATION_MS;

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
    fn one(text: &str) -> String {
        format!("1\n00:00:00,000 --> 00:00:00,001\n{text}\n\n")
    }

    #[test]
    fn canonical_g_output_roundtrips_unicode_multiline_and_literal_markup() {
        let input = track([
            (0, 1, "  Café 👋\n字幕\t한국어  ".into()),
            (59_999, 60_000, "<i>Literal text</i> & 日本語".into()),
            (3_599_999, 3_600_000, "Hour".into()),
            (MAX_DURATION_MS - 1, MAX_DURATION_MS, "Last".into()),
        ]);
        assert_eq!(
            parse(&input.to_srt().unwrap(), input.duration_ms).unwrap(),
            input
        );
    }

    #[test]
    fn bom_crlf_and_optional_final_blank_line_preserve_text_and_ignore_identifiers() {
        let input = "\u{feff} 42 \r\n 00:00:00,001 --> 00:00:00,010 \r\n  Café\r\n字幕  \r\n\r\n4294967295\r\n00:00:00,010 --> 00:00:00,011\r\nSecond";
        let expected = track([(1, 10, "  Café\n字幕  ".into()), (10, 11, "Second".into())]);
        for suffix in ["", "\r\n", "\r\n\r\n"] {
            assert_eq!(
                parse(&format!("{input}{suffix}"), MAX_DURATION_MS).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn whitespace_only_is_empty_but_duration_and_lone_cr_still_validate() {
        for text in ["", "\u{feff}", " \t\n\r\n\u{2003}\n"] {
            assert!(parse(text, MAX_DURATION_MS).unwrap().cues.is_empty());
            for duration in [0, MAX_DURATION_MS + 1, u64::MAX] {
                assert!(matches!(
                    parse(text, duration),
                    Err(SrtError::Validation {
                        source: ValidationError::Duration,
                        ..
                    })
                ));
            }
        }
        assert!(matches!(
            parse(" \r", 1),
            Err(SrtError::Syntax {
                block: 1,
                line: 1,
                ..
            })
        ));
    }

    #[test]
    fn timestamps_reject_wrong_width_range_overflow_and_position_metadata() {
        for timestamp in [
            "0:00:00,000",
            "100:00:00,000",
            "00:0:00,000",
            "00:00:0,000",
            "00:00:00,00",
            "00:00:00.000",
            "00:60:00,000",
            "00:00:60,000",
            "00:00:00,000 X1:0",
            "18446744073709551615:00:00,000",
            "００:00:00,000",
        ] {
            let text = format!("1\n00:00:00,000 --> {timestamp}\nText");
            assert!(
                matches!(
                    parse(&text, MAX_DURATION_MS),
                    Err(SrtError::Syntax {
                        block: 1,
                        line: 2,
                        ..
                    })
                ),
                "{timestamp}"
            );
        }
        for arrow in ["->", "-- >", "--> 00:00:00,001 -->"] {
            assert!(parse(&format!("1\n00:00:00,000 {arrow} 00:00:00,001\nText"), 1).is_err());
        }
    }

    #[test]
    fn shared_validation_rejects_timing_and_overlap_without_shifting_or_sorting() {
        for (start, end, duration) in [(0, 0, 2), (2, 1, 2), (0, 2, 1)] {
            let text = format!("1\n00:00:00,{start:03} --> 00:00:00,{end:03}\nText");
            assert_eq!(
                parse(&text, duration).unwrap_err(),
                validation(1, 2, ValidationError::Timing { cue: 1 })
            );
        }
        let text =
            "8\n00:00:00,000 --> 00:00:01,000\nFirst\n\n2\n00:00:00,999 --> 00:00:02,000\nSecond";
        assert_eq!(
            parse(text, 2000).unwrap_err(),
            validation(2, 6, ValidationError::Order { cue: 2 })
        );
    }

    #[test]
    fn malformed_blocks_and_controls_report_the_actual_block_and_line() {
        for identifier in ["0", "-1", "+1", "1a", "4294967296", ""] {
            assert!(
                parse(
                    &format!("{identifier}\n00:00:00,000 --> 00:00:00,001\nText"),
                    1
                )
                .is_err()
            );
        }
        for text in [
            "1",
            "1\n",
            "1\n00:00:00,000 --> 00:00:00,001",
            "1\n00:00:00,000 --> 00:00:00,001\n \t\n",
        ] {
            assert!(parse(text, 1).is_err());
        }
        assert!(matches!(
            parse(&format!("{}Injected", one("Text")), 1),
            Err(SrtError::Syntax {
                block: 2,
                line: 5,
                ..
            })
        ));
        assert_eq!(
            parse(&one("First\nBad\0text"), 1).unwrap_err(),
            validation(1, 4, ValidationError::ControlCharacter { cue: 1 })
        );
        for text in ["Lone\rCR", "Control\u{7f}", "Control\u{85}", "\u{85}"] {
            assert!(parse(&one(text), 1).is_err());
        }
        assert!(parse("\u{85}", 1).is_err());
    }

    #[test]
    fn input_and_cue_count_limits_accept_exact_boundaries() {
        assert!(
            parse(&" ".repeat(MAX_INPUT_BYTES), 1)
                .unwrap()
                .cues
                .is_empty()
        );
        assert_eq!(
            parse(&" ".repeat(MAX_INPUT_BYTES + 1), 1).unwrap_err(),
            SrtError::InputLimit
        );
        let input = track((0..MAX_CUES as u64).map(|start| (start, start + 1, "x".into())));
        let text = input.to_srt().unwrap();
        assert_eq!(parse(&text, input.duration_ms).unwrap(), input);
        assert!(matches!(
            parse(
                &format!("{text}10001\n00:00:10,000 --> 00:00:10,001\nx"),
                MAX_DURATION_MS
            ),
            Err(SrtError::Validation {
                block: 10001,
                source: ValidationError::CueCount,
                ..
            })
        ));
    }

    #[test]
    fn utf8_cue_and_aggregate_text_limits_are_checked_before_copying_excess() {
        let boundary = "é".repeat(MAX_CUE_TEXT_BYTES / 2);
        assert_eq!(parse(&one(&boundary), 1).unwrap().cues[0].text, boundary);
        assert_eq!(
            parse(&one(&format!("{boundary}x")), 1).unwrap_err(),
            validation(1, 3, ValidationError::CueTextLimit { cue: 1 })
        );
        let input = track(
            (0..(MAX_TEXT_BYTES / MAX_CUE_TEXT_BYTES) as u64)
                .map(|start| (start, start + 1, boundary.clone())),
        );
        let text = input.to_srt().unwrap();
        assert_eq!(parse(&text, input.duration_ms).unwrap(), input);
        assert!(matches!(
            parse(
                &format!("{text}65\n00:00:00,064 --> 00:00:00,065\nx"),
                MAX_DURATION_MS
            ),
            Err(SrtError::Validation {
                block: 65,
                source: ValidationError::TrackTextLimit,
                ..
            })
        ));
    }
}
