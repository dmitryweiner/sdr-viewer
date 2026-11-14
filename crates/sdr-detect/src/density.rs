//! How often each frequency was at each level: a histogram of the
//! detector's frames that forgets with time. A short rare signal is a
//! faint mark where an averaged spectrum shows nothing.

use std::time::Duration;

use sdr_core::SpectrumFrame;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DensityConfig {
    /// The lowest level told apart, in dBFS; what is below counts as this.
    pub bottom_db: f32,
    /// The height of one level in dB.
    pub step_db: f32,
    pub levels: usize,
    /// After this long a frame weighs half of a new one.
    pub half_life: Duration,
}

impl Default for DensityConfig {
    fn default() -> Self {
        DensityConfig {
            // From under the noise of any receiver to full scale.
            bottom_db: -120.0,
            step_db: 1.0,
            levels: 120,
            half_life: Duration::from_secs(30),
        }
    }
}

/// The histogram as it stands: plain numbers for whoever draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct DensityMap {
    pub config: DensityConfig,
    /// Bins of the spectrum, lowest frequency first.
    pub bins: usize,
    /// The weight of the frames that found a bin at a level:
    /// `bin * levels + level`.
    cells: Vec<f32>,
    /// The weight of all frames: what a cell is a share of.
    total: f32,
}

impl DensityMap {
    fn new(config: DensityConfig, bins: usize) -> Self {
        DensityMap {
            config,
            bins,
            cells: vec![0.0; bins * config.levels],
            total: 0.0,
        }
    }

    /// The level `db` falls into; what is off the scale goes to its ends.
    pub fn level_of(&self, db: f32) -> usize {
        let level = ((db - self.config.bottom_db) / self.config.step_db).floor();
        (level.max(0.0) as usize).min(self.config.levels.saturating_sub(1))
    }

    /// The share of the remembered time `bin` spent at each level, lowest
    /// first; they add up to 1.
    pub fn shares(&self, bin: usize) -> impl Iterator<Item = f32> + '_ {
        let levels = self.config.levels;
        let scale = if self.total > 0.0 {
            1.0 / self.total
        } else {
            0.0
        };
        self.cells[bin * levels..(bin + 1) * levels]
            .iter()
            .map(move |weight| weight * scale)
    }

    /// The share of the remembered time `bin` spent in levels that lie
    /// wholly above `db`.
    pub fn above(&self, bin: usize, db: f32) -> f32 {
        let first = ((db - self.config.bottom_db) / self.config.step_db).ceil();
        self.shares(bin).skip(first.max(0.0) as usize).sum()
    }
}

/// Gathers a [`DensityMap`] from frames.
pub struct Density {
    config: DensityConfig,
    map: DensityMap,
    /// What a new frame weighs. Instead of making every cell lighter with
    /// every frame, each new frame is heavier than the one before; now and
    /// then everything is scaled back.
    weight: f32,
    /// The latest frame: its time, and the band the map is of.
    last: Option<(Duration, f64, u32)>,
}

/// The weight of a frame at which everything is scaled back to 1: far
/// from where an `f32` stops telling one frame more from none.
const HEAVY: f32 = 1e3;

impl Density {
    pub fn new(config: DensityConfig) -> Self {
        Density {
            config,
            map: DensityMap::new(config, 0),
            weight: 1.0,
            last: None,
        }
    }

    pub fn push(&mut self, frame: &SpectrumFrame) {
        let bins = frame.power_db.len();
        let same_band = self.last.is_some_and(|(_, center_hz, sample_rate)| {
            center_hz == frame.center_hz && sample_rate == frame.sample_rate
        });
        if !same_band || self.map.bins != bins {
            // Another band: what was gathered says nothing about it.
            self.map = DensityMap::new(self.config, bins);
            self.weight = 1.0;
        } else if let Some((before, ..)) = self.last {
            let passed = frame.time.saturating_sub(before).as_secs_f64();
            let half_lives = passed / self.config.half_life.as_secs_f64();
            self.weight *= 2f64.powf(half_lives) as f32;
        }
        self.last = Some((frame.time, frame.center_hz, frame.sample_rate));
        if self.weight >= HEAVY {
            let scale = 1.0 / self.weight;
            for cell in &mut self.map.cells {
                *cell *= scale;
            }
            self.map.total *= scale;
            self.weight = 1.0;
        }
        let levels = self.config.levels;
        for (bin, &db) in frame.power_db.iter().enumerate() {
            let level = self.map.level_of(db);
            self.map.cells[bin * levels + level] += self.weight;
        }
        self.map.total += self.weight;
    }

    /// Forgets everything: the next frame starts afresh.
    pub fn reset(&mut self) {
        self.map = DensityMap::new(self.config, 0);
        self.last = None;
    }

    pub fn map(&self) -> &DensityMap {
        &self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DensityConfig {
        DensityConfig {
            bottom_db: -100.0,
            step_db: 10.0,
            levels: 10,
            half_life: Duration::from_secs(1),
        }
    }

    fn frame(ms: u64, power_db: &[f32]) -> SpectrumFrame {
        SpectrumFrame {
            time: Duration::from_millis(ms),
            center_hz: 100e6,
            sample_rate: 1_000_000,
            power_db: power_db.to_vec(),
        }
    }

    fn shares(density: &Density, bin: usize) -> Vec<f32> {
        density.map().shares(bin).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_bin_is_counted_at_the_level_it_was_found_at() {
        let mut density = Density::new(DensityConfig {
            half_life: Duration::from_secs(1_000_000),
            ..config()
        });
        for i in 0..3 {
            density.push(&frame(i, &[-95.0, -55.0]));
        }
        density.push(&frame(3, &[-95.0, -20.0]));
        assert_eq!(density.map().bins, 2);
        let quiet = shares(&density, 0);
        assert!(close(quiet[0], 1.0), "{quiet:?}");
        let busy = shares(&density, 1);
        assert!(close(busy[4], 0.75) && close(busy[8], 0.25), "{busy:?}");
        assert!(close(busy.iter().sum(), 1.0));
    }

    #[test]
    fn what_is_off_the_scale_goes_to_its_ends() {
        let mut density = Density::new(config());
        density.push(&frame(0, &[-150.0, 3.0, f32::NEG_INFINITY]));
        assert_eq!(shares(&density, 0)[0], 1.0);
        assert_eq!(shares(&density, 1)[9], 1.0);
        assert_eq!(shares(&density, 2)[0], 1.0);
    }

    #[test]
    fn a_frame_weighs_half_after_a_half_life() {
        let mut density = Density::new(config());
        density.push(&frame(0, &[-55.0]));
        density.push(&frame(1000, &[-25.0]));
        let now = shares(&density, 0);
        assert!(close(now[4], 1.0 / 3.0), "{now:?}");
        assert!(close(now[7], 2.0 / 3.0), "{now:?}");
    }

    #[test]
    fn hours_of_frames_keep_the_shares_right() {
        // 100 frames a second for three hours of stream time: the weight
        // is scaled back hundreds of times on the way.
        let mut density = Density::new(config());
        let mut ms = 0;
        for i in 0..1_080_000u32 {
            let db = if i % 4 == 0 { -25.0 } else { -55.0 };
            density.push(&frame(ms, &[db]));
            ms += 10;
        }
        let now = shares(&density, 0);
        assert!((now[7] - 0.25).abs() < 0.01, "{now:?}");
        assert!((now[4] - 0.75).abs() < 0.01, "{now:?}");
        assert!(close(now.iter().sum(), 1.0));
    }

    #[test]
    fn the_share_above_a_level_counts_only_levels_wholly_above_it() {
        let mut density = Density::new(DensityConfig {
            half_life: Duration::from_secs(1_000_000),
            ..config()
        });
        for (i, db) in [-95.0, -55.0, -45.0, -15.0].into_iter().enumerate() {
            density.push(&frame(i as u64, &[db]));
        }
        let map = density.map();
        assert!(close(map.above(0, -200.0), 1.0));
        assert!(close(map.above(0, -60.0), 0.75), "{}", map.above(0, -60.0));
        // -55 is in the level from -60 to -50, which is not all above -58.
        assert!(close(map.above(0, -58.0), 0.5), "{}", map.above(0, -58.0));
        assert!(close(map.above(0, 0.0), 0.0));
    }

    #[test]
    fn another_band_or_a_reset_starts_afresh() {
        let mut density = Density::new(config());
        density.push(&frame(0, &[-55.0]));
        let mut retuned = frame(10, &[-25.0]);
        retuned.center_hz = 200e6;
        density.push(&retuned);
        assert_eq!(shares(&density, 0)[7], 1.0);
        density.reset();
        assert_eq!(density.map().bins, 0);
        density.push(&SpectrumFrame {
            power_db: vec![-95.0],
            ..retuned
        });
        assert_eq!(shares(&density, 0)[0], 1.0);
    }

    #[test]
    fn nothing_gathered_is_all_zeros() {
        let density = Density::new(config());
        assert_eq!(density.map().bins, 0);
        let mut one = Density::new(config());
        one.push(&frame(0, &[-55.0]));
        one.reset();
        assert_eq!(one.map().bins, 0);
    }
}
