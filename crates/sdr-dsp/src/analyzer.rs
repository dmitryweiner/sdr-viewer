//! Windowed FFTs averaged into spectrum frames.

use std::sync::Arc;
use std::time::Duration;

use rustfft::{Fft, FftPlanner};
use sdr_core::{Iq, SourceInfo, SpectrumFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalyzerConfig {
    /// Bins per frame.
    pub fft_size: usize,
    /// FFTs whose power is averaged into one frame.
    pub average: usize,
}

impl AnalyzerConfig {
    /// A config giving about `frames_per_second` at `sample_rate`.
    pub fn for_rate(sample_rate: u32, fft_size: usize, frames_per_second: f64) -> Self {
        let ffts_per_second = f64::from(sample_rate) / fft_size as f64;
        let average = (ffts_per_second / frames_per_second).round().max(1.0) as usize;
        AnalyzerConfig { fft_size, average }
    }
}

/// Cuts the stream into blocks of `fft_size` samples without overlap,
/// applies a Hann window, and emits the mean power of every `average`
/// blocks as one frame. Power is averaged, not decibels, so a burst shorter
/// than a frame keeps its energy.
pub struct Analyzer {
    config: AnalyzerConfig,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Turns |X|² into power relative to a full-scale tone.
    scale: f32,
    /// Samples waiting for a full block.
    pending: Vec<Iq>,
    block: Vec<Iq>,
    scratch: Vec<Iq>,
    sum: Vec<f32>,
    summed: usize,
    /// Samples consumed into finished frames.
    position: u64,
}

impl Analyzer {
    pub fn new(config: AnalyzerConfig) -> Self {
        let n = config.fft_size;
        // The periodic Hann window: its sum is exactly n / 2.
        let window: Vec<f32> = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()) as f32)
            .collect();
        let window_sum: f32 = window.iter().sum();
        let fft = FftPlanner::new().plan_fft_forward(n);
        let zero = Iq::new(0.0, 0.0);
        Analyzer {
            config,
            scale: 1.0 / (window_sum * window_sum),
            scratch: vec![zero; fft.get_inplace_scratch_len()],
            fft,
            window,
            pending: Vec::with_capacity(n),
            block: vec![zero; n],
            sum: vec![0.0; n],
            summed: 0,
            position: 0,
        }
    }

    /// Feeds samples; `emit` is called for every frame they complete.
    pub fn push(
        &mut self,
        mut samples: &[Iq],
        info: &SourceInfo,
        mut emit: impl FnMut(SpectrumFrame),
    ) {
        let n = self.config.fft_size;
        while !samples.is_empty() {
            let take = (n - self.pending.len()).min(samples.len());
            self.pending.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.pending.len() < n {
                return;
            }
            self.add_block();
            if self.summed == self.config.average {
                emit(self.finish_frame(info));
            }
        }
    }

    fn add_block(&mut self) {
        let n = self.config.fft_size;
        for ((out, sample), weight) in self.block.iter_mut().zip(&self.pending).zip(&self.window) {
            *out = sample * weight;
        }
        self.pending.clear();
        self.fft
            .process_with_scratch(&mut self.block, &mut self.scratch);
        // The FFT puts 0 Hz first; frames go from the lowest frequency up.
        let (positive, negative) = self.block.split_at(n / 2);
        for (sum, bin) in self.sum.iter_mut().zip(negative.iter().chain(positive)) {
            *sum += bin.norm_sqr();
        }
        self.summed += 1;
    }

    fn finish_frame(&mut self, info: &SourceInfo) -> SpectrumFrame {
        let scale = self.scale / self.summed as f32;
        let power_db = self
            .sum
            .iter()
            .map(|sum| 10.0 * (sum * scale + 1e-20).log10())
            .collect();
        let frame = SpectrumFrame {
            time: Duration::from_secs_f64(self.position as f64 / f64::from(info.sample_rate)),
            center_hz: info.center_hz,
            sample_rate: info.sample_rate,
            power_db,
        };
        self.position += (self.summed * self.config.fft_size) as u64;
        self.sum.fill(0.0);
        self.summed = 0;
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::Source;
    use sdr_core::generator::{Generator, GeneratorConfig, Tone};

    const N: usize = 1024;
    /// With this rate a bin is exactly 1 kHz wide.
    const RATE: u32 = 1_024_000;
    const CENTER: f64 = 100e6;

    fn frames(
        noise_dbfs: Option<f32>,
        tones: Vec<Tone>,
        average: usize,
        samples: usize,
    ) -> Vec<SpectrumFrame> {
        let mut generator = Generator::new(GeneratorConfig {
            sample_rate: RATE,
            center_hz: CENTER,
            noise_dbfs,
            tones,
            seed: 3,
        });
        let mut buf = vec![Iq::new(0.0, 0.0); samples];
        generator.read(&mut buf).unwrap();
        let mut analyzer = Analyzer::new(AnalyzerConfig {
            fft_size: N,
            average,
        });
        let mut out = Vec::new();
        analyzer.push(&buf, &generator.info(), |frame| out.push(frame));
        out
    }

    fn tone(offset_hz: f64, level_dbfs: f32) -> Tone {
        Tone {
            freq_hz: CENTER + offset_hz,
            level_dbfs,
            gate: None,
        }
    }

    fn peak(frame: &SpectrumFrame) -> (usize, f32) {
        let (index, level) = frame
            .power_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        (index, *level)
    }

    #[test]
    fn a_full_scale_tone_reads_zero_db_in_its_bin() {
        let out = frames(None, vec![tone(100_000.0, 0.0)], 1, N);
        assert_eq!(out.len(), 1);
        let (index, level) = peak(&out[0]);
        assert_eq!(index, N / 2 + 100);
        assert!(level.abs() < 0.01, "{level}");
        assert_eq!(out[0].bin_hz(index), CENTER + 100_000.0);
        // Far from the tone there is nothing.
        assert!(out[0].power_db[N / 2 - 200] < -100.0);
    }

    #[test]
    fn a_tone_below_the_centre_is_left_of_the_middle_at_its_level() {
        let out = frames(None, vec![tone(-250_000.0, -20.0)], 1, N);
        let (index, level) = peak(&out[0]);
        assert_eq!(index, N / 2 - 250);
        assert!((level + 20.0).abs() < 0.01, "{level}");
    }

    #[test]
    fn the_centre_frequency_is_the_middle_bin() {
        let out = frames(None, vec![tone(0.0, -6.0)], 1, N);
        assert_eq!(peak(&out[0]).0, N / 2);
    }

    #[test]
    fn noise_spreads_over_the_bins_by_the_window() {
        let out = frames(Some(-30.0), vec![], 64, N * 64);
        assert_eq!(out.len(), 1);
        let mean_power: f32 = out[0]
            .power_db
            .iter()
            .map(|db| 10f32.powf(db / 10.0))
            .sum::<f32>()
            / N as f32;
        // A Hann window passes 1.5/N of white noise power into each bin.
        let expected = -30.0 + 10.0 * (1.5 / N as f32).log10();
        assert!((10.0 * mean_power.log10() - expected).abs() < 0.3);
    }

    #[test]
    fn frames_carry_their_time_and_what_was_received() {
        let out = frames(None, vec![], 4, N * 4 * 3 + 100);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].time, Duration::ZERO);
        assert_eq!(
            out[2].time,
            Duration::from_secs_f64((N * 8) as f64 / f64::from(RATE))
        );
        assert_eq!(out[1].center_hz, CENTER);
        assert_eq!(out[1].sample_rate, RATE);
        assert_eq!(out[1].power_db.len(), N);
    }

    #[test]
    fn chunking_does_not_change_the_frames() {
        let whole = frames(Some(-40.0), vec![tone(50_000.0, -10.0)], 2, N * 6);
        let mut generator = Generator::new(GeneratorConfig {
            sample_rate: RATE,
            center_hz: CENTER,
            noise_dbfs: Some(-40.0),
            tones: vec![tone(50_000.0, -10.0)],
            seed: 3,
        });
        let mut analyzer = Analyzer::new(AnalyzerConfig {
            fft_size: N,
            average: 2,
        });
        let mut pieces = Vec::new();
        let mut left = N * 6;
        for size in [1usize, 777, 1024, 5000].into_iter().cycle() {
            let size = size.min(left);
            let mut buf = vec![Iq::new(0.0, 0.0); size];
            generator.read(&mut buf).unwrap();
            analyzer.push(&buf, &generator.info(), |frame| pieces.push(frame));
            left -= size;
            if left == 0 {
                break;
            }
        }
        assert_eq!(whole, pieces);
    }

    #[test]
    fn averaging_keeps_the_energy_of_a_short_burst() {
        // On for the first of four blocks: a quarter of the power, -6 dB.
        let gate = sdr_core::generator::Gate {
            period_s: (N * 4) as f64 / f64::from(RATE),
            on_s: N as f64 / f64::from(RATE),
        };
        let burst = Tone {
            freq_hz: CENTER + 10_000.0,
            level_dbfs: 0.0,
            gate: Some(gate),
        };
        let out = frames(None, vec![burst], 4, N * 4);
        let (_, level) = peak(&out[0]);
        assert!((level + 6.02).abs() < 0.05, "{level}");
    }

    #[test]
    fn the_config_for_a_rate_gives_the_asked_frame_rate() {
        assert_eq!(
            AnalyzerConfig::for_rate(2_400_000, 2048, 10.0),
            AnalyzerConfig {
                fft_size: 2048,
                average: 117
            }
        );
        assert_eq!(AnalyzerConfig::for_rate(1000, 2048, 10.0).average, 1);
    }
}
