//! Fast frames averaged into slow ones.

use sdr_core::SpectrumFrame;

/// Emits the mean power of every `count` frames as one frame carrying the
/// time of the first: the detector gets the fast frames, the screen these,
/// and the FFTs are done once.
pub struct FrameAverager {
    count: usize,
    /// The first frame of the group, its power replaced by the linear sum.
    sum: Option<SpectrumFrame>,
    summed: usize,
}

impl FrameAverager {
    pub fn new(count: usize) -> Self {
        FrameAverager {
            count: count.max(1),
            sum: None,
            summed: 0,
        }
    }

    pub fn push(&mut self, frame: &SpectrumFrame) -> Option<SpectrumFrame> {
        let same_band = |sum: &SpectrumFrame| {
            sum.center_hz == frame.center_hz
                && sum.sample_rate == frame.sample_rate
                && sum.power_db.len() == frame.power_db.len()
        };
        let linear = frame.power_db.iter().map(|db| 10f32.powf(db / 10.0));
        match &mut self.sum {
            Some(sum) if same_band(sum) => {
                for (sum, power) in sum.power_db.iter_mut().zip(linear) {
                    *sum += power;
                }
            }
            // The first frame, or the first after retuning: what was
            // gathered at the old frequency is dropped.
            _ => {
                self.sum = Some(SpectrumFrame {
                    power_db: linear.collect(),
                    ..frame.clone()
                });
                self.summed = 0;
            }
        }
        self.summed += 1;
        if self.summed < self.count {
            return None;
        }
        let mut done = self.sum.take()?;
        let scale = 1.0 / self.summed as f32;
        for power in &mut done.power_db {
            *power = 10.0 * (*power * scale + 1e-20).log10();
        }
        self.summed = 0;
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn frame(ms: u64, center_hz: f64, power_db: &[f32]) -> SpectrumFrame {
        SpectrumFrame {
            time: Duration::from_millis(ms),
            center_hz,
            sample_rate: 1000,
            power_db: power_db.to_vec(),
        }
    }

    #[test]
    fn averages_power_not_decibels_and_keeps_the_first_time() {
        let mut averager = FrameAverager::new(4);
        // A burst in one frame of four: a quarter of the power, -6 dB.
        assert_eq!(averager.push(&frame(10, 1e6, &[-20.0, -50.0])), None);
        assert_eq!(averager.push(&frame(20, 1e6, &[-200.0, -50.0])), None);
        assert_eq!(averager.push(&frame(30, 1e6, &[-200.0, -50.0])), None);
        let out = averager.push(&frame(40, 1e6, &[-200.0, -50.0])).unwrap();
        assert_eq!(out.time, Duration::from_millis(10));
        assert_eq!(out.center_hz, 1e6);
        assert_eq!(out.sample_rate, 1000);
        assert!((out.power_db[0] + 26.02).abs() < 0.01, "{out:?}");
        assert!((out.power_db[1] + 50.0).abs() < 0.01, "{out:?}");
        // The next group starts clean.
        assert_eq!(averager.push(&frame(50, 1e6, &[-30.0, -30.0])), None);
    }

    #[test]
    fn one_to_one_passes_frames_through() {
        let mut averager = FrameAverager::new(1);
        let out = averager.push(&frame(10, 1e6, &[-20.0])).unwrap();
        assert!((out.power_db[0] + 20.0).abs() < 0.001);
        assert!(averager.push(&frame(20, 1e6, &[-20.0])).is_some());
    }

    #[test]
    fn retuning_starts_the_group_again() {
        let mut averager = FrameAverager::new(2);
        assert_eq!(averager.push(&frame(10, 1e6, &[0.0])), None);
        assert_eq!(averager.push(&frame(20, 2e6, &[-40.0])), None);
        let out = averager.push(&frame(30, 2e6, &[-40.0])).unwrap();
        assert_eq!(out.time, Duration::from_millis(20));
        assert_eq!(out.center_hz, 2e6);
        assert!((out.power_db[0] + 40.0).abs() < 0.01);
    }
}
