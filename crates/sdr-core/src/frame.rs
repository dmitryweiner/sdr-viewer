//! The unit every analyser and screen works on.

use std::time::Duration;

/// One power spectrum: what the receiver saw around `center_hz` at `time`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumFrame {
    /// Time of the frame's first sample, counted in samples from the start
    /// of the stream.
    pub time: Duration,
    pub center_hz: f64,
    /// The width of the spectrum in hertz.
    pub sample_rate: u32,
    /// Power per bin in dBFS (a full-scale tone reads 0), from the lowest
    /// frequency to the highest; the centre frequency is bin `len / 2`.
    pub power_db: Vec<f32>,
}

impl SpectrumFrame {
    /// The frequency at the middle of bin `index`.
    pub fn bin_hz(&self, index: usize) -> f64 {
        let bins = self.power_db.len() as f64;
        self.center_hz + (index as f64 - bins / 2.0) * f64::from(self.sample_rate) / bins
    }

    /// The frequency at `fraction` of the width, 0 = the left edge.
    pub fn hz_at(&self, fraction: f64) -> f64 {
        self.center_hz + (fraction - 0.5) * f64::from(self.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_map_to_frequencies_around_the_centre() {
        let frame = SpectrumFrame {
            time: Duration::ZERO,
            center_hz: 100e6,
            sample_rate: 1_000_000,
            power_db: vec![0.0; 1000],
        };
        assert_eq!(frame.bin_hz(500), 100e6);
        assert_eq!(frame.bin_hz(0), 99.5e6);
        assert_eq!(frame.bin_hz(999), 100.499e6);
        assert_eq!(frame.hz_at(0.0), 99.5e6);
        assert_eq!(frame.hz_at(0.75), 100.25e6);
    }
}
