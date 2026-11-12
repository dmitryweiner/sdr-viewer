//! The noise floor under a spectrum, followed over time.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorConfig {
    /// Stretches of the spectrum that each get their own level; the floor
    /// of a bin is interpolated between them.
    pub segments: usize,
    /// How long the floor takes to follow the noise up. Slow, so that a
    /// signal filling a whole segment is not taken for noise at once.
    pub rise: Duration,
    /// How long it takes to follow the noise down.
    pub fall: Duration,
}

impl Default for FloorConfig {
    fn default() -> Self {
        FloorConfig {
            segments: 32,
            rise: Duration::from_secs(10),
            fall: Duration::from_millis(500),
        }
    }
}

/// The level of the noise per bin in dBFS. In every frame the median of
/// each segment is taken — a signal narrower than half a segment does not
/// move it — and the segment's level follows the medians over time.
pub struct NoiseFloor {
    config: FloorConfig,
    segment_db: Vec<f32>,
    bin_db: Vec<f32>,
    scratch: Vec<f32>,
}

impl NoiseFloor {
    pub fn new(config: FloorConfig) -> Self {
        NoiseFloor {
            config,
            segment_db: Vec::new(),
            bin_db: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Forgets everything: the next frame sets the floor anew.
    pub fn reset(&mut self) {
        self.segment_db.clear();
        self.bin_db.clear();
    }

    /// Takes in a frame that came `elapsed` after the one before.
    pub fn update(&mut self, power_db: &[f32], elapsed: Duration) {
        let bins = power_db.len();
        let segments = self.config.segments.clamp(1, bins.max(1));
        let fresh = self.bin_db.len() != bins;
        if fresh {
            self.segment_db.clear();
            self.segment_db.resize(segments, 0.0);
        }
        if bins == 0 {
            self.bin_db.clear();
            return;
        }
        let rise = follow(elapsed, self.config.rise);
        let fall = follow(elapsed, self.config.fall);
        for (index, level) in self.segment_db.iter_mut().enumerate() {
            let from = index * bins / segments;
            let to = (index + 1) * bins / segments;
            self.scratch.clear();
            self.scratch.extend_from_slice(&power_db[from..to]);
            let middle = self.scratch.len() / 2;
            let (_, median, _) = self.scratch.select_nth_unstable_by(middle, f32::total_cmp);
            let step = *median - *level;
            *level = if fresh {
                *median
            } else {
                *level + step * if step > 0.0 { rise } else { fall }
            };
        }
        self.bin_db.resize(bins, 0.0);
        let last = (segments - 1) as f32;
        for (bin, out) in self.bin_db.iter_mut().enumerate() {
            // Where the bin lies among the segments' middles.
            let place = ((bin as f32 + 0.5) * segments as f32 / bins as f32 - 0.5).clamp(0.0, last);
            let left = place.floor() as usize;
            let right = (left + 1).min(segments - 1);
            let share = place - left as f32;
            *out = self.segment_db[left] * (1.0 - share) + self.segment_db[right] * share;
        }
    }

    /// The floor per bin; empty before the first frame.
    pub fn bins(&self) -> &[f32] {
        &self.bin_db
    }
}

/// The share of the way to a new level covered in `elapsed`, for a time
/// constant of `time`.
fn follow(elapsed: Duration, time: Duration) -> f32 {
    if time.is_zero() {
        return 1.0;
    }
    1.0 - (-elapsed.as_secs_f32() / time.as_secs_f32()).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINS: usize = 1024;
    const FRAME: Duration = Duration::from_millis(10);

    /// A level with a repeatable ripple of ±1 dB around it.
    fn noise(level: f32) -> Vec<f32> {
        (0..BINS)
            .map(|bin| level + ((bin * 7 % 5) as f32 - 2.0) / 2.0)
            .collect()
    }

    fn floor() -> NoiseFloor {
        NoiseFloor::new(FloorConfig::default())
    }

    fn assert_near(got: f32, wanted: f32) {
        assert!((got - wanted).abs() < 0.05, "{got} is not {wanted}");
    }

    #[test]
    fn is_empty_before_the_first_frame() {
        assert!(floor().bins().is_empty());
    }

    #[test]
    fn the_first_frame_sets_the_floor_to_the_median_of_the_noise() {
        let mut floor = floor();
        floor.update(&noise(-90.0), Duration::ZERO);
        assert_eq!(floor.bins().len(), BINS);
        for &db in floor.bins() {
            assert_near(db, -90.0);
        }
    }

    #[test]
    fn a_narrow_signal_does_not_lift_it() {
        let mut power = noise(-90.0);
        power[500..510].fill(-20.0);
        let mut floor = floor();
        for _ in 0..1000 {
            floor.update(&power, FRAME);
        }
        // A third of its segment: the median moves within the ripple.
        let under = floor.bins()[505];
        assert!((under + 90.0).abs() < 1.0, "{under}");
    }

    #[test]
    fn follows_the_shape_of_the_band() {
        let mut power = noise(-90.0);
        for db in &mut power[BINS / 2..] {
            *db += 20.0;
        }
        let mut floor = floor();
        floor.update(&power, Duration::ZERO);
        assert_near(floor.bins()[100], -90.0);
        assert_near(floor.bins()[BINS - 100], -70.0);
        // Between the middles of the two segments at the step it slopes.
        let at_step = floor.bins()[BINS / 2];
        assert!(at_step > -85.0 && at_step < -75.0, "{at_step}");
    }

    #[test]
    fn rises_slowly_and_falls_fast() {
        let mut floor = floor();
        floor.update(&noise(-90.0), Duration::ZERO);
        // One second of noise 10 dB up: a tenth of the rise time.
        for _ in 0..100 {
            floor.update(&noise(-80.0), FRAME);
        }
        assert_near(floor.bins()[300], -90.0 + 10.0 * (1.0 - (-0.1f32).exp()));
        // The rise time: 63% of the way from where it was.
        let mut floor = self::floor();
        floor.update(&noise(-90.0), Duration::ZERO);
        for _ in 0..1000 {
            floor.update(&noise(-80.0), FRAME);
        }
        assert_near(floor.bins()[300], -90.0 + 10.0 * (1.0 - (-1.0f32).exp()));
        // Back down: after 2 s, four fall times, it is nearly there.
        for _ in 0..200 {
            floor.update(&noise(-90.0), FRAME);
        }
        assert!(floor.bins()[300] < -89.8, "{}", floor.bins()[300]);
    }

    #[test]
    fn the_frame_rate_does_not_change_how_fast_it_follows() {
        let mut fine = floor();
        let mut coarse = floor();
        fine.update(&noise(-90.0), Duration::ZERO);
        coarse.update(&noise(-90.0), Duration::ZERO);
        for _ in 0..100 {
            fine.update(&noise(-80.0), FRAME);
        }
        for _ in 0..10 {
            coarse.update(&noise(-80.0), FRAME * 10);
        }
        assert_near(fine.bins()[10], coarse.bins()[10]);
    }

    #[test]
    fn starts_anew_after_a_reset_or_another_size() {
        let mut floor = floor();
        floor.update(&noise(-90.0), Duration::ZERO);
        floor.reset();
        assert!(floor.bins().is_empty());
        floor.update(&noise(-50.0), FRAME);
        assert_near(floor.bins()[0], -50.0);
        floor.update(&vec![-30.0; 100], FRAME);
        assert_eq!(floor.bins().len(), 100);
        assert_near(floor.bins()[99], -30.0);
    }

    #[test]
    fn copes_with_fewer_bins_than_segments() {
        let mut floor = floor();
        floor.update(&[-40.0, -40.0, -60.0], Duration::ZERO);
        assert_eq!(floor.bins(), [-40.0, -40.0, -60.0]);
        floor.update(&[], FRAME);
        assert!(floor.bins().is_empty());
    }
}
