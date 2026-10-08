//! Validate a regular presentation grid without retaining every packet.
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct PacketTiming {
    origin: Option<i64>,
    pub step: i64,
    pub decode_delay: i64,
    count: i64,
    next: i64,
    pending: BTreeSet<i64>,
    invalid: bool,
}

impl PacketTiming {
    pub fn push(&mut self, pts: i64, duration: i64) {
        let origin = *self.origin.get_or_insert(pts);
        if self.count == 0 {
            self.step = duration;
        }
        if !self.invalid {
            let Some(relative) = pts.checked_sub(origin) else {
                self.invalid = true;
                self.count += 1;
                return;
            };
            if self.step <= 0 || duration != self.step || relative < 0 || relative % self.step != 0
            {
                self.invalid = true;
            } else {
                let index = relative / self.step;
                if index < self.next || !self.pending.insert(index) {
                    self.invalid = true;
                }
                match self
                    .count
                    .checked_mul(self.step)
                    .and_then(|time| time.checked_sub(relative))
                {
                    Some(delay) => self.decode_delay = self.decode_delay.max(delay),
                    None => self.invalid = true,
                }
                while self.pending.remove(&self.next) {
                    self.next += 1;
                }
                // H.264 frame reordering is small; malformed grids cannot grow memory unboundedly.
                if self.pending.len() > 64 {
                    self.invalid = true;
                    self.pending.clear();
                }
            }
        }
        self.count += 1;
    }

    pub fn regular(&self) -> bool {
        self.count > 0 && !self.invalid && self.pending.is_empty() && self.next == self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_frames_keep_a_regular_presentation_grid() {
        let mut timing = PacketTiming::default();
        for index in [0, 4, 2, 1, 3, 5] {
            timing.push(900 + index * 40, 40);
        }
        assert!(timing.regular());
        assert_eq!(timing.decode_delay, 80);
    }

    #[test]
    fn duplicates_holes_and_variable_durations_are_not_regular() {
        for samples in [
            vec![(0, 40), (0, 40)],
            vec![(0, 40), (80, 40)],
            vec![(0, 40), (40, 20)],
        ] {
            let mut timing = PacketTiming::default();
            for (pts, duration) in samples {
                timing.push(pts, duration);
            }
            assert!(!timing.regular());
        }
    }

    #[test]
    fn extreme_timestamps_cannot_wrap_into_a_copyable_grid_or_panic() {
        for samples in [
            vec![(i64::MAX, 1), (i64::MIN, 1)],
            vec![(i64::MIN, 1), (i64::MAX, 1)],
            vec![(0, i64::MAX), (i64::MAX, i64::MAX), (i64::MAX, i64::MAX)],
        ] {
            let mut timing = PacketTiming::default();
            for (pts, duration) in samples {
                timing.push(pts, duration);
            }
            assert!(!timing.regular());
        }
        // Large positive and negative origins are valid when relative times fit.
        for origin in [i64::MIN, i64::MAX - 200] {
            let mut timing = PacketTiming::default();
            for index in [0, 4, 2, 1, 3, 5] {
                timing.push(origin + index * 40, 40);
            }
            assert!(timing.regular());
            assert_eq!(timing.decode_delay, 80);
        }
    }

    #[test]
    fn unbounded_reordering_is_rejected_and_cannot_revive_after_the_gap_closes() {
        let mut timing = PacketTiming::default();
        timing.push(0, 1);
        for index in 2..1000 {
            timing.push(index, 1);
        }
        assert!(!timing.regular());
        assert!(timing.pending.len() <= 64);
        timing.push(1, 1);
        assert!(!timing.regular());
    }
}
