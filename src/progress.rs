//! Map each sequential stage into its allocated share of one export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProgressRange {
    start: u32,
    end: u32,
}

impl ProgressRange {
    pub const fn new(start: u32, end: u32) -> Self {
        assert!(start <= end && end <= 100);
        Self { start, end }
    }

    pub fn percent(self, fraction: f64) -> u32 {
        self.start + ((self.end - self.start) as f64 * fraction.clamp(0.0, 1.0)) as u32
    }

    pub fn subrange(self, start: u32, end: u32) -> Self {
        assert!(start <= end && end <= 100);
        let span = self.end - self.start;
        Self::new(
            self.start + span * start / 100,
            self.start + span * end / 100,
        )
    }

    pub fn partition(self, index: usize, count: usize) -> Self {
        assert!(index < count);
        let span = u128::from(self.end - self.start);
        Self::new(
            self.start + (span * index as u128 / count as u128) as u32,
            self.start + (span * (index + 1) as u128 / count as u128) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_short_stages_have_no_gaps_or_backtracking() {
        let range = ProgressRange::new(0, 40).subrange(0, 75);
        for count in [1, 3, 31, 100, 1000] {
            let mut previous = range.percent(0.0);
            for index in 0..count {
                let part = range.partition(index, count);
                let video = part.subrange(0, 80);
                let audio = part.subrange(80, 100);
                assert_eq!(video.percent(0.0), previous);
                assert_eq!(video.percent(1.0), audio.percent(0.0));
                previous = audio.percent(1.0);
            }
            assert_eq!(previous, range.percent(1.0));
        }
    }

    #[test]
    fn rendering_cannot_finish_publication_or_leave_its_range() {
        let range = ProgressRange::new(40, 99);
        assert_eq!(range.percent(-1.0), 40);
        assert_eq!(range.percent(f64::NAN), 40);
        assert_eq!(range.percent(2.0), 99);
        assert_eq!(range.subrange(65, 100).percent(1.0), 99);
    }
}
