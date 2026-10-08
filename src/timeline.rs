//! Project-clock selection and viewport arithmetic. No UI, file IO or media work.
use serde::{Deserialize, Serialize};

/// A nonempty half-open interval on the soundtrack clock, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPart {
    Body,
    Start,
    End,
}

/// Preserve the pointer's original grab offset. Absolute drag updates avoid
/// accumulating rounding errors or jumping when a handle's hit area is grabbed.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    original: Range,
    anchor_ms: u64,
    part: DragPart,
}
impl Drag {
    pub fn new(range: Range, part: DragPart, pointer_ms: u64) -> Self {
        Self {
            original: range,
            anchor_ms: pointer_ms,
            part,
        }
    }

    pub fn at(self, pointer_ms: u64, duration_ms: u64) -> Option<Range> {
        let delta = (i128::from(pointer_ms) - i128::from(self.anchor_ms))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        let range = self.original.clamped(duration_ms)?;
        match self.part {
            DragPart::Body => range.moved(delta, duration_ms),
            DragPart::Start => {
                Some(range.resize_start(range.start_ms.saturating_add_signed(delta)))
            }
            DragPart::End => {
                range.resize_end(range.end_ms.saturating_add_signed(delta), duration_ms)
            }
        }
    }
}

impl Range {
    pub fn new(start_ms: u64, end_ms: u64, duration_ms: u64) -> Option<Self> {
        (start_ms < end_ms && end_ms <= duration_ms).then_some(Self { start_ms, end_ms })
    }

    pub fn duration_ms(self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    /// Place at a time, preserving the requested length when either edge is hit.
    pub fn place(start_ms: u64, length_ms: u64, duration_ms: u64) -> Option<Self> {
        if duration_ms == 0 || length_ms == 0 {
            return None;
        }
        let length = length_ms.min(duration_ms);
        let start = start_ms.min(duration_ms - length);
        Self::new(start, start + length, duration_ms)
    }

    /// Clamp translation as a unit: never shrink a range at the project edges.
    pub fn moved(self, delta_ms: i64, duration_ms: u64) -> Option<Self> {
        let start = self.start_ms.saturating_add_signed(delta_ms);
        Self::place(start, self.duration_ms(), duration_ms)
    }

    pub fn resize_start(self, start_ms: u64) -> Self {
        Self {
            start_ms: start_ms.min(self.end_ms.saturating_sub(1)),
            ..self
        }
    }

    pub fn resize_end(self, end_ms: u64, duration_ms: u64) -> Option<Self> {
        let range = self.clamped(duration_ms)?;
        Some(Self {
            end_ms: end_ms.clamp(range.start_ms + 1, duration_ms),
            ..range
        })
    }

    pub fn clamped(self, duration_ms: u64) -> Option<Self> {
        Self::place(self.start_ms, self.duration_ms(), duration_ms)
    }

    /// Project/audio to short-local time; the excluded end has no local sample.
    pub fn local_ms(self, project_ms: u64) -> Option<u64> {
        (self.start_ms..self.end_ms)
            .contains(&project_ms)
            .then(|| project_ms - self.start_ms)
    }

    pub fn project_ms(self, local_ms: u64) -> Option<u64> {
        (local_ms < self.duration_ms()).then(|| self.start_ms + local_ms)
    }
}

/// The visible region of the one project timeline. A zero-length project has
/// no viewport; callers show their empty timeline instead of dividing by zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport(pub Range);

impl Viewport {
    pub fn full(duration_ms: u64) -> Option<Self> {
        Range::new(0, duration_ms, duration_ms).map(Self)
    }

    pub fn fraction(self, time_ms: u64) -> f64 {
        (time_ms as f64 - self.0.start_ms as f64) / self.0.duration_ms() as f64
    }

    pub fn time(self, fraction: f64) -> u64 {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.0.start_ms + (fraction * self.0.duration_ms() as f64).round() as u64
    }

    /// Zoom about a pointer/focus anchor, with a one-second precision floor
    /// (or the complete duration for a project shorter than one second).
    pub fn zoom(self, factor: f64, anchor: f64, duration_ms: u64) -> Option<Self> {
        if !factor.is_finite() || factor <= 0.0 || duration_ms == 0 {
            return self.0.clamped(duration_ms).map(Self);
        }
        let anchor = if anchor.is_finite() {
            anchor.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let time = self.time(anchor);
        let length = ((self.0.duration_ms() as f64 / factor).round() as u64)
            .clamp(duration_ms.min(1_000), duration_ms);
        Range::place(
            time.saturating_sub((anchor * length as f64).round() as u64),
            length,
            duration_ms,
        )
        .map(Self)
    }

    pub fn pan(self, delta_ms: i64, duration_ms: u64) -> Option<Self> {
        self.0.moved(delta_ms, duration_ms).map(Self)
    }

    pub fn focus(range: Range, duration_ms: u64) -> Option<Self> {
        let margin = (range.duration_ms() / 4).max(500);
        Range::place(
            range.start_ms.saturating_sub(margin),
            range.duration_ms().saturating_add(2 * margin),
            duration_ms,
        )
        .map(Self)
    }
}

pub fn format_time(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds >= 3_600 {
        format!(
            "{}:{:02}:{:02}.{:03}",
            seconds / 3_600,
            seconds / 60 % 60,
            seconds % 60,
            ms % 1_000
        )
    } else {
        format!("{}:{:02}.{:03}", seconds / 60, seconds % 60, ms % 1_000)
    }
}

/// Parse seconds, m:ss or h:mm:ss; accept localized comma decimal separators.
/// Reject signs, exponents, empty components and sub-millisecond ambiguity.
pub fn parse_time(value: &str) -> Option<u64> {
    let value = value.trim().replace(',', ".");
    let parts: Vec<_> = value.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut total = 0u64;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let (whole, fraction) = if last {
            part.split_once('.').unwrap_or((part, ""))
        } else {
            (*part, "")
        };
        if whole.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
            || fraction.len() > 3
            || (last && part.ends_with('.'))
        {
            return None;
        }
        let seconds: u64 = whole.parse().ok()?;
        if i > 0 && seconds >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(seconds)?;
        if last {
            let millis = if fraction.is_empty() {
                0
            } else {
                fraction.parse::<u64>().ok()? * 10u64.pow(3 - fraction.len() as u32)
            };
            return total.checked_mul(1_000)?.checked_add(millis);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_preserves_length_at_both_edges() {
        let range = Range::new(13_000, 18_000, 20_000).unwrap();
        assert_eq!(range.moved(-15_000, 20_000), Range::new(0, 5_000, 20_000));
        assert_eq!(
            range.moved(i64::MAX, 20_000),
            Range::new(15_000, 20_000, 20_000)
        );
        assert_eq!(range.moved(i64::MIN, 20_000), Range::new(0, 5_000, 20_000));
    }

    #[test]
    fn pointer_grab_offset_is_stable_when_body_and_handles_hit_limits() {
        let range = Range::new(13_000, 18_000, 20_000).unwrap();
        let body = Drag::new(range, DragPart::Body, 14_300);
        assert_eq!(body.at(14_300, 20_000), Some(range));
        assert_eq!(body.at(20_000, 20_000), Range::new(15_000, 20_000, 20_000));
        assert_eq!(
            body.at(14_300, 20_000),
            Some(range),
            "Returning from a clamp must restore the original grab offset"
        );
        let left = Drag::new(range, DragPart::Start, 12_950);
        assert_eq!(left.at(12_950, 20_000), Some(range));
        assert_eq!(left.at(20_000, 20_000).unwrap().start_ms, 17_999);
        let right = Drag::new(range, DragPart::End, 18_050);
        assert_eq!(right.at(0, 20_000).unwrap().end_ms, 13_001);
        assert_eq!(right.at(20_000, 20_000).unwrap().end_ms, 19_950);
    }

    #[test]
    fn crossing_handles_and_replacement_duration_remain_nonempty() {
        let range = Range::new(13_000, 18_000, 20_000).unwrap();
        assert_eq!(range.resize_start(19_000).start_ms, 17_999);
        assert_eq!(range.resize_end(1, 20_000).unwrap().end_ms, 13_001);
        assert_eq!(range.clamped(2_000), Range::new(0, 2_000, 2_000));
        assert_eq!(range.clamped(0), None);
        assert_eq!(
            Range::place(u64::MAX, u64::MAX, 20_000),
            Range::new(0, 20_000, 20_000)
        );
    }

    #[test]
    fn local_clock_excludes_end_and_retains_selected_audio() {
        let range = Range::new(13_000, 18_000, 20_000).unwrap();
        assert_eq!(range.local_ms(14_000), Some(1_000));
        assert_eq!(range.project_ms(3_000), Some(16_000));
        assert_eq!(range.local_ms(18_000), None);
        assert_eq!(range.local_ms(12_999), None);
        assert_eq!(range.project_ms(5_000), None);
    }

    #[test]
    fn focused_long_timeline_keeps_anchor_and_clamps_pan() {
        let duration = 7_200_000;
        let full = Viewport::full(duration).unwrap();
        let zoom = full.zoom(1_000.0, 0.6, duration).unwrap();
        assert_eq!(zoom.time(0.6), full.time(0.6));
        assert!((zoom.fraction(zoom.time(0.3)) - 0.3).abs() < 0.001);
        assert_eq!(zoom.pan(i64::MAX, duration).unwrap().0.end_ms, duration);
        assert_eq!(zoom.pan(i64::MIN, duration).unwrap().0.start_ms, 0);
        assert_eq!(zoom.time(f64::NAN), zoom.0.start_ms);
        assert_eq!(zoom.zoom(f64::INFINITY, 0.0, duration), Some(zoom));
        assert_eq!(
            Viewport::full(500).unwrap().zoom(10.0, 0.5, 500),
            Viewport::full(500)
        );
    }

    #[test]
    fn timecodes_round_trip_and_reject_ambiguous_input() {
        for ms in [0, 1, 999, 13_000, 59_999, 60_000, 3_600_001, 7_200_000] {
            assert_eq!(parse_time(&format_time(ms)), Some(ms));
        }
        assert_eq!(parse_time("1:02,125"), Some(62_125));
        assert_eq!(parse_time("13.5"), Some(13_500));
        for invalid in [
            "-1",
            "NaN",
            "1e2",
            "1:60",
            "1::2",
            ":2",
            "1.0001",
            "1.",
            "18446744073709551615",
        ] {
            assert_eq!(parse_time(invalid), None, "{invalid}");
        }
    }
}
