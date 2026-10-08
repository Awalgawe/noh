//! Playback navigation targets (Monitor README): start and end, ±5 s, one
//! export frame, previous and next picture. Pure arithmetic on the project
//! clock; the transport draws the buttons and applies the targets.
use super::*;

/// The export's frame rate as an exact rational: `num / den` frames per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: u64,
    pub den: u64,
}

impl Rate {
    /// A usable rate (1 to 240 frames per second), else `None`.
    pub fn new(num: i64, den: i64) -> Option<Self> {
        let (num, den) = (u64::try_from(num).ok()?, u64::try_from(den).ok()?);
        (num > 0 && den > 0 && num >= den && num <= 240 * den).then_some(Self { num, den })
    }
    /// Index of the frame on screen `ms` after the origin. Half a millisecond
    /// of tolerance absorbs the millisecond rounding of a frame start (the
    /// player reports 33 for the frame at 33.37 ms).
    pub fn frame_at(self, ms: u64) -> u64 {
        ((2 * u128::from(ms) + 1) * u128::from(self.num) / (2000 * u128::from(self.den))) as u64
    }
    /// The first whole millisecond of frame `k`: `k · den / num` seconds, rounded up.
    pub fn frame_ms(self, k: u64) -> u64 {
        (u128::from(k) * 1000 * u128::from(self.den)).div_ceil(u128::from(self.num)) as u64
    }
    /// Frames that start before `ms`.
    fn frames_before(self, ms: u64) -> u64 {
        (u128::from(ms) * u128::from(self.num)).div_ceil(1000 * u128::from(self.den)) as u64
    }
    /// Frames per second rounded for the tooltip (`1/30 s` at 30000/1001).
    pub fn rounded(self) -> u64 {
        ((self.num + self.den / 2) / self.den).max(1)
    }
}

/// A picture the playhead can jump to: its first frame and its letter.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub ms: u64,
    pub label: String,
    pub source_index: usize,
}

/// Where each navigation button leads; `None` disables it (at a bound).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub previous: Option<Picture>,
    pub start: Option<u64>,
    pub back_5: Option<u64>,
    pub back_frame: Option<u64>,
    pub forward_frame: Option<u64>,
    pub forward_5: Option<u64>,
    pub end: Option<u64>,
    pub next: Option<Picture>,
}

/// Navigation from `cursor` inside `[start, end)` (the song, or the short).
/// Frames count from `start`; pictures loop from `picture_origin` (0, or the
/// short's start when its pictures restart at A).
pub fn targets(
    cursor: u64,
    (start, end): (u64, u64),
    rate: Rate,
    visuals: &[app_timeline::Visual],
    picture_origin: u64,
) -> Targets {
    if end <= start {
        return Targets::default();
    }
    let last_index = rate.frames_before(end - start).saturating_sub(1);
    // "Go to end" is the start of the last frame, inside the half-open bounds.
    let last = (start + rate.frame_ms(last_index)).min(end - 1);
    let last_index = rate.frame_at(last - start);
    let cursor = cursor.clamp(start, end - 1);
    let k = rate.frame_at(cursor - start);
    let back = cursor > start;
    let forward = k < last_index;
    let (previous, next) = pictures(cursor, (start, last), rate, visuals, picture_origin);
    Targets {
        previous,
        start: back.then_some(start),
        back_5: back.then(|| cursor.saturating_sub(5000).max(start)),
        back_frame: back.then(|| start + rate.frame_ms(k.saturating_sub(1))),
        forward_frame: forward.then(|| (start + rate.frame_ms(k + 1)).min(last)),
        forward_5: forward.then(|| (cursor + 5000).min(last)),
        end: forward.then_some(last),
        next,
    }
}

/// The start of the current picture (or the one before, from its first
/// frame) and of the next one, on the lane's cell arithmetic, each moved to
/// the first frame that shows it.
fn pictures(
    cursor: u64,
    (start, last): (u64, u64),
    rate: Rate,
    visuals: &[app_timeline::Visual],
    origin: u64,
) -> (Option<Picture>, Option<Picture>) {
    let Some(sequence) = app_timeline::sequence_seconds(visuals) else {
        return (None, None);
    };
    let k = rate.frame_at(cursor - start);
    let last_index = rate.frame_at(last - start);
    // Two passes back, so the cell before the current one is included.
    let from = ((cursor - origin) as f64 / 1000.0 - 2.0 * sequence).max(0.0);
    // Several cells inside one frame: the frame shows the last of them.
    let mut previous: Option<Picture> = None;
    let mut next: Option<(u64, Picture)> = None;
    for (index, cell_start, cell_end) in app_timeline::cells(visuals, from).take(100_000) {
        let begin = origin as f64 + cell_start * 1000.0;
        if origin as f64 + cell_end * 1000.0 <= start as f64 {
            continue;
        }
        let frame = if begin <= start as f64 {
            0
        } else {
            let exact = (begin - start as f64) * rate.num as f64 / (1000.0 * rate.den as f64);
            (exact - 1e-6).ceil().max(0.0) as u64
        };
        if frame > last_index || next.as_ref().is_some_and(|(f, _)| frame > *f) {
            break;
        }
        let picture = Picture {
            ms: (start + rate.frame_ms(frame)).min(last),
            label: visuals[index].label.clone(),
            source_index: visuals[index].source_index,
        };
        if frame > k {
            next = Some((frame, picture));
        } else if frame < k {
            previous = Some(picture);
        }
    }
    (previous, next.map(|(_, p)| p))
}

impl NohApp {
    /// The export plan's target rate; before the plan knows it, the rate the
    /// preview uses (the first video's, or the pictures' 25).
    pub(super) fn navigation_rate(&self) -> Rate {
        let planned = self
            .diagnosis
            .as_ref()
            .filter(|_| self.diagnosis_current())
            .and_then(|d| d.plan.as_ref())
            .and_then(|p| Rate::new(p.target.rate_num, p.target.rate_den));
        let video = || {
            self.clips
                .iter()
                .filter(|c| c.error.is_none())
                .filter(|c| !matches!(c.item, noh::input::MediaItem::Image { .. }))
                .find_map(|c| c.info.as_ref())
                .and_then(|i| Rate::new((i.fps * 1000.0).round() as i64, 1000))
        };
        planned.or_else(video).unwrap_or(Rate { num: 25, den: 1 })
    }
    /// Targets for the view the monitor shows.
    pub(super) fn navigation_targets(&self) -> Targets {
        let bounds = self.mini_bounds();
        let origin = match self.project.range {
            Some(range) if self.mini_preview.short && self.project.restart_loops => range.start_ms,
            _ => 0,
        };
        targets(
            self.mini_preview.cursor_ms,
            bounds,
            self.navigation_rate(),
            &self.project_visuals(),
            origin,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual(label: &str, seconds: f64, source_index: usize) -> app_timeline::Visual {
        app_timeline::Visual {
            label: label.into(),
            seconds,
            source_index,
        }
    }

    #[test]
    fn frame_steps_use_the_exact_rational_and_survive_millisecond_rounding() {
        let ntsc = Rate::new(30000, 1001).unwrap();
        assert_eq!(ntsc.rounded(), 30);
        // Frame 1 starts at 33.367 ms: the first whole millisecond is 34.
        assert_eq!(ntsc.frame_ms(1), 34);
        assert_eq!(ntsc.frame_ms(3), 101);
        // The player reports either rounding of a frame start: same frame.
        for k in 0..2000 {
            let exact = k as f64 * 1001.0 / 30.0;
            assert_eq!(ntsc.frame_at(ntsc.frame_ms(k)), k);
            assert_eq!(ntsc.frame_at(exact.round() as u64), k, "{k}");
        }
        // Stepping 30 frames forward from 0 lands on 1001 ms, never 30 × 40.
        let mut ms = 0;
        for _ in 0..30 {
            ms = targets(ms, (0, 60_000), ntsc, &[], 0)
                .forward_frame
                .unwrap();
        }
        assert_eq!(ms, 1001);
        let pal = Rate::new(25, 1).unwrap();
        assert_eq!(pal.frame_ms(1), 40);
        // Floor ± 1: from inside frame 2 (95 ms), back goes to frame 1.
        let t = targets(95, (0, 60_000), pal, &[], 0);
        assert_eq!(t.back_frame, Some(40));
        assert_eq!(t.forward_frame, Some(120));
        assert_eq!(Rate::new(0, 1), None);
        assert_eq!(Rate::new(1000, 1), None);
    }

    #[test]
    fn bounds_disable_the_buttons_that_cannot_move() {
        let pal = Rate::new(25, 1).unwrap();
        let at_start = targets(13_000, (13_000, 18_000), pal, &[], 0);
        assert_eq!(
            (at_start.start, at_start.back_5, at_start.back_frame),
            (None, None, None)
        );
        // Go to end = the start of the last frame of the half-open short.
        assert_eq!(at_start.end, Some(17_960));
        assert_eq!(at_start.forward_5, Some(17_960));
        let at_end = targets(17_960, (13_000, 18_000), pal, &[], 0);
        assert_eq!(
            (at_end.forward_frame, at_end.forward_5, at_end.end),
            (None, None, None)
        );
        assert_eq!(at_end.back_5, Some(13_000));
        // Inside the last frame, but past its start: still at the end.
        assert_eq!(targets(17_999, (13_000, 18_000), pal, &[], 0).end, None);
        // A song that is not a whole number of frames: the last one is partial.
        let song = targets(0, (0, 10_010), pal, &[], 0);
        assert_eq!(song.end, Some(10_000));
    }

    #[test]
    fn pictures_follow_the_lane_cells_and_restart_in_the_short() {
        let pal = Rate::new(25, 1).unwrap();
        let visuals = [
            visual("A", 2.0, 0),
            visual("B", 3.0, 1),
            visual("C", 1.5, 2),
        ];
        let bounds = (0, 60_000);
        // Inside B: previous is B's start, next is C.
        let t = targets(3_000, bounds, pal, &visuals, 0);
        assert_eq!(
            t.previous.as_ref().map(|p| (p.ms, p.label.as_str())),
            Some((2_000, "B"))
        );
        assert_eq!(
            t.next.as_ref().map(|p| (p.ms, p.label.as_str())),
            Some((5_000, "C"))
        );
        // On B's first frame: previous goes back to A, looping passes wrap.
        let t = targets(2_000, bounds, pal, &visuals, 0);
        assert_eq!(t.previous.map(|p| p.label), Some("A".into()));
        // A starts again at 6.5 s, inside frame 162: its first frame is 163.
        let t = targets(6_600, bounds, pal, &visuals, 0);
        assert_eq!(
            t.previous.map(|p| (p.ms, p.label)),
            Some((6_520, "A".into()))
        );
        assert_eq!(t.next.map(|p| (p.ms, p.label)), Some((8_520, "B".into())));
        // At the start there is nothing before.
        assert_eq!(targets(0, bounds, pal, &visuals, 0).previous, None);
        // A cell start between frames moves to the first frame showing it.
        let odd = [visual("A", 1.01, 0), visual("B", 1.0, 1)];
        let t = targets(0, bounds, pal, &odd, 0);
        assert_eq!(t.next.map(|p| p.ms), Some(1_040));
        // Short 13–18 s without restart: the main timeline's cells (the third
        // pass starts with A at 13 s).
        let short = (13_000, 18_000);
        let t = targets(14_000, short, pal, &visuals, 0);
        assert_eq!(
            t.previous.as_ref().map(|p| (p.ms, p.label.as_str())),
            Some((13_000, "A"))
        );
        assert_eq!(
            t.next.as_ref().map(|p| (p.ms, p.label.as_str())),
            Some((15_000, "B"))
        );
        // A short starting inside C (11.5–13 s): its start shows C.
        let t = targets(12_500, (12_000, 18_000), pal, &visuals, 0);
        assert_eq!(
            t.previous.map(|p| (p.ms, p.label)),
            Some((12_000, "C".into()))
        );
        assert_eq!(t.next.map(|p| (p.ms, p.label)), Some((13_000, "A".into())));
        // With restart, the short's pictures start at A from its start.
        let t = targets(14_000, (13_500, 18_000), pal, &visuals, 13_500);
        assert_eq!(
            t.previous.map(|p| (p.ms, p.label)),
            Some((13_500, "A".into()))
        );
        assert_eq!(t.next.map(|p| (p.ms, p.label)), Some((15_500, "B".into())));
        // No next picture before the end of the short.
        let t = targets(17_000, short, pal, &visuals, 0);
        assert_eq!(t.next, None);
    }
}
