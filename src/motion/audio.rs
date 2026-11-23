//! What a scene reads of the sound: a loudness envelope and its sudden rises
//! ("energy and onsets"). Not a beat tracker: no tempo, no beat grid.

/// Envelope samples per second.
const RATE: f64 = 100.0;

pub struct Audio {
    /// Loudness 0..1 (1 = the 95th percentile of the sound), RATE per second.
    energy: Vec<f32>,
    /// Onset times in seconds, increasing.
    onsets: Vec<f64>,
}

impl Audio {
    /// No sound: energy 0 everywhere, no onset.
    pub fn silent() -> Self {
        Self {
            energy: Vec::new(),
            onsets: Vec::new(),
        }
    }

    /// From mono samples in -1..1 at `hz` samples per second.
    pub fn analyse(samples: &[f32], hz: u32) -> Self {
        let mut loudness = Loudness::new(hz);
        loudness.push(samples);
        Self::from_loudness(loudness.finish())
    }

    /// From the loudness of each hundredth of a second (`Loudness`).
    pub fn from_loudness(raw: Vec<f32>) -> Self {
        // Onsets: rises of the log envelope over 30 ms, above a local threshold
        // (mean + 2 deviations over a second around), 150 ms apart at least.
        let log: Vec<f32> = raw.iter().map(|v| (v + 1e-4).ln()).collect();
        let flux: Vec<f32> = (0..log.len())
            .map(|i| {
                if i >= 3 {
                    (log[i] - log[i - 3]).max(0.0)
                } else {
                    0.0
                }
            })
            .collect();
        let mut onsets = Vec::new();
        let mut last = f64::NEG_INFINITY;
        for i in 1..flux.len().saturating_sub(1) {
            let win = &flux[i.saturating_sub(50)..(i + 50).min(flux.len())];
            let mean = win.iter().sum::<f32>() / win.len() as f32;
            let var = win.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / win.len() as f32;
            let t = i as f64 / RATE;
            if flux[i] > mean + 2.0 * var.sqrt()
                && flux[i] > 0.25
                && flux[i] >= flux[i - 1]
                && flux[i] >= flux[i + 1]
                && t - last > 0.15
            {
                onsets.push(t);
                last = t;
            }
        }
        // Envelope: fast attack, slow release, scaled by the 95th percentile.
        let mut sorted = raw.clone();
        sorted.sort_by(f32::total_cmp);
        let p95 = sorted
            .get(sorted.len() * 95 / 100)
            .copied()
            .unwrap_or(1.0)
            .max(1e-4);
        let mut env = 0.0f32;
        let energy = raw
            .iter()
            .map(|&v| {
                let x = (v / p95).min(1.0);
                env += (x - env) * if x > env { 0.6 } else { 0.08 };
                env
            })
            .collect();
        Self { energy, onsets }
    }

    /// Loudness at `t`, interpolated; 0 before the start and after the end.
    pub fn energy_at(&self, t: f64) -> f64 {
        if self.energy.is_empty() || t.is_nan() || t < 0.0 {
            return 0.0;
        }
        let x = t * RATE;
        let i = x.floor() as usize;
        let f = (x - i as f64) as f32;
        let a = self.energy.get(i).copied().unwrap_or(0.0);
        let b = self.energy.get(i + 1).copied().unwrap_or(a);
        f64::from(a + (b - a) * f)
    }

    /// Seconds since the last onset at or before `t` (99 when there is none).
    pub fn onset_age(&self, t: f64) -> f64 {
        let i = self.onsets.partition_point(|&o| o <= t);
        if i == 0 { 99.0 } else { t - self.onsets[i - 1] }
    }

    pub fn onsets(&self) -> &[f64] {
        &self.onsets
    }
}

/// The loudness (root mean square) of each hundredth of a second of mono
/// samples at their own rate, as they come: the sound is never held whole (two
/// hours at 48 kHz would be 1.4 GB). Each hop is `hz / 100` samples, truncated,
/// summed in f64: the prototype's analysis exactly, which the scenes seen so far
/// were made with.
pub struct Loudness {
    hop: usize,
    filled: usize,
    acc: f64,
    out: Vec<f32>,
}

impl Loudness {
    pub fn new(hz: u32) -> Self {
        Self {
            hop: (f64::from(hz) / RATE) as usize,
            filled: 0,
            acc: 0.0,
            out: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        if self.hop == 0 {
            return;
        }
        for &s in samples {
            self.acc += f64::from(s) * f64::from(s);
            self.filled += 1;
            if self.filled == self.hop {
                self.out.push((self.acc / self.hop as f64).sqrt() as f32);
                self.acc = 0.0;
                self.filled = 0;
            }
        }
    }

    /// One value per whole hop (a last partial hop is dropped).
    pub fn finish(self) -> Vec<f32> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::{Audio, Loudness};

    /// Pushed in pieces or at once, the same loudness; a hop is hz / 100
    /// samples, truncated, and a partial last hop is dropped.
    #[test]
    fn loudness_is_the_same_however_the_samples_come() {
        let s: Vec<f32> = (0..44_100 + 300)
            .map(|i| ((i % 97) as f32 / 97.0) - 0.5)
            .collect();
        let mut whole = Loudness::new(44_100);
        whole.push(&s);
        let mut pieces = Loudness::new(44_100);
        for c in s.chunks(1000) {
            pieces.push(c);
        }
        let (a, b) = (whole.finish(), pieces.finish());
        assert_eq!(a, b);
        assert_eq!(a.len(), (44_100 + 300) / 441);
        let first = (s[..441].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / 441.0).sqrt();
        assert_eq!(a[0], first as f32);
    }

    /// Silence then a loud burst every half second: one onset per burst, and an
    /// energy that rises with the bursts.
    #[test]
    fn bursts_are_onsets_and_raise_the_energy() {
        let hz = 8000u32;
        let mut s = vec![0.001f32; hz as usize * 3];
        for k in 0..4 {
            let at = (hz as f64 * (0.5 + 0.5 * k as f64)) as usize;
            for v in &mut s[at..at + 800] {
                *v = 0.8;
            }
        }
        let a = Audio::analyse(&s, hz);
        assert_eq!(a.onsets().len(), 4, "{:?}", a.onsets());
        for (k, o) in a.onsets().iter().enumerate() {
            assert!((o - (0.5 + 0.5 * k as f64)).abs() < 0.05, "{o}");
        }
        assert!(a.energy_at(0.55) > 0.5 && a.energy_at(0.2) < 0.05);
        assert!((a.onset_age(0.6) - 0.1).abs() < 0.05);
        assert_eq!(a.onset_age(0.1), 99.0);
        assert_eq!(Audio::silent().energy_at(1.0), 0.0);
        assert_eq!(a.energy_at(f64::NAN), 0.0);
    }
}
