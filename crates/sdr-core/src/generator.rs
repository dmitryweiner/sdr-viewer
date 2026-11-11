//! A synthetic source: tones and bursts over Gaussian noise, fully repeatable.

use crate::rng::SplitMix64;
use crate::source::{Iq, Source, SourceError, SourceInfo};

/// A carrier at an absolute frequency, optionally switched on and off.
#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    pub freq_hz: f64,
    /// Power relative to full scale.
    pub level_dbfs: f32,
    pub gate: Option<Gate>,
}

/// On for `on_s` at the start of every `period_s`, silent for the rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gate {
    pub period_s: f64,
    pub on_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorConfig {
    pub sample_rate: u32,
    pub center_hz: f64,
    /// Total noise power relative to full scale; `None` for no noise.
    pub noise_dbfs: Option<f32>,
    pub tones: Vec<Tone>,
    pub seed: u64,
}

impl GeneratorConfig {
    /// The scene the app shows without hardware: noise, a steady carrier,
    /// and a burst repeating every 2.04 s (the plan's periodicity example).
    pub fn demo() -> Self {
        GeneratorConfig {
            sample_rate: 2_400_000,
            center_hz: 433_920_000.0,
            noise_dbfs: Some(-60.0),
            tones: vec![
                Tone {
                    freq_hz: 433_500_000.0,
                    level_dbfs: -30.0,
                    gate: None,
                },
                Tone {
                    freq_hz: 434_200_000.0,
                    level_dbfs: -25.0,
                    gate: Some(Gate {
                        period_s: 2.04,
                        on_s: 0.3,
                    }),
                },
            ],
            seed: 1,
        }
    }
}

pub struct Generator {
    config: GeneratorConfig,
    rng: SplitMix64,
    /// Index of the next sample: the generator's clock.
    index: u64,
    tones: Vec<ToneState>,
    noise_sigma: f32,
}

struct ToneState {
    amplitude: f32,
    phase: f64,
    /// Radians per sample at the current centre frequency.
    step: f64,
    /// (period, on) in samples, so gating is exact.
    gate: Option<(u64, u64)>,
}

impl Generator {
    pub fn new(config: GeneratorConfig) -> Self {
        let rate = f64::from(config.sample_rate);
        let tones = config
            .tones
            .iter()
            .map(|tone| ToneState {
                amplitude: 10f32.powf(tone.level_dbfs / 20.0),
                phase: 0.0,
                step: 0.0,
                gate: tone.gate.map(|gate| {
                    let period = ((gate.period_s * rate).round() as u64).max(1);
                    (period, (gate.on_s * rate).round() as u64)
                }),
            })
            .collect();
        // Complex noise: the power splits evenly between I and Q.
        let noise_sigma = config
            .noise_dbfs
            .map_or(0.0, |db| (10f32.powf(db / 10.0) / 2.0).sqrt());
        let mut generator = Generator {
            rng: SplitMix64::new(config.seed),
            index: 0,
            tones,
            noise_sigma,
            config,
        };
        generator.update_steps();
        generator
    }

    fn update_steps(&mut self) {
        let rate = f64::from(self.config.sample_rate);
        for (state, tone) in self.tones.iter_mut().zip(&self.config.tones) {
            state.step = std::f64::consts::TAU * (tone.freq_hz - self.config.center_hz) / rate;
        }
    }

    fn next_sample(&mut self) -> Iq {
        let mut sample = Iq::new(0.0, 0.0);
        for tone in &mut self.tones {
            let on = tone
                .gate
                .is_none_or(|(period, on)| self.index % period < on);
            if on {
                let (sin, cos) = tone.phase.sin_cos();
                sample += Iq::new(cos as f32, sin as f32) * tone.amplitude;
            }
            tone.phase = (tone.phase + tone.step).rem_euclid(std::f64::consts::TAU);
        }
        if self.noise_sigma > 0.0 {
            let (i, q) = self.rng.next_gaussian_pair();
            sample += Iq::new(i as f32, q as f32) * self.noise_sigma;
        }
        self.index += 1;
        sample
    }
}

impl Source for Generator {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            sample_rate: self.config.sample_rate,
            center_hz: self.config.center_hz,
            description: "generator".into(),
        }
    }

    fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
        for sample in buf.iter_mut() {
            *sample = self.next_sample();
        }
        Ok(buf.len())
    }

    fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
        self.config.center_hz = center_hz;
        self.update_steps();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1_000_000;
    const CENTER: f64 = 100_000_000.0;

    fn config(noise_dbfs: Option<f32>, tones: Vec<Tone>) -> GeneratorConfig {
        GeneratorConfig {
            sample_rate: RATE,
            center_hz: CENTER,
            noise_dbfs,
            tones,
            seed: 42,
        }
    }

    fn tone(freq_hz: f64, level_dbfs: f32) -> Tone {
        Tone {
            freq_hz,
            level_dbfs,
            gate: None,
        }
    }

    fn take(source: &mut impl Source, n: usize) -> Vec<Iq> {
        let mut buf = vec![Iq::new(0.0, 0.0); n];
        assert_eq!(source.read(&mut buf).unwrap(), n);
        buf
    }

    fn power(samples: &[Iq]) -> f64 {
        samples.iter().map(|s| s.norm_sqr() as f64).sum::<f64>() / samples.len() as f64
    }

    /// Mean phase step between neighbours, in hertz.
    fn frequency_offset(samples: &[Iq]) -> f64 {
        let sum: num_complex::Complex64 = samples
            .windows(2)
            .map(|w| {
                let d = w[1] * w[0].conj();
                num_complex::Complex64::new(d.re as f64, d.im as f64)
            })
            .sum();
        sum.arg() * RATE as f64 / std::f64::consts::TAU
    }

    #[test]
    fn reports_its_rate_and_centre() {
        let info = Generator::new(config(None, vec![])).info();
        assert_eq!(info.sample_rate, RATE);
        assert_eq!(info.center_hz, CENTER);
        assert_eq!(info.description, "generator");
    }

    #[test]
    fn a_tone_sits_at_its_offset_from_the_centre_with_its_power() {
        let mut generator = Generator::new(config(None, vec![tone(CENTER + 100_000.0, -20.0)]));
        let samples = take(&mut generator, 10_000);
        assert!((frequency_offset(&samples) - 100_000.0).abs() < 1.0);
        assert!((power(&samples) - 0.01).abs() < 1e-5);
    }

    #[test]
    fn a_tone_below_the_centre_has_a_negative_offset() {
        let mut generator = Generator::new(config(None, vec![tone(CENTER - 250_000.0, 0.0)]));
        let samples = take(&mut generator, 10_000);
        assert!((frequency_offset(&samples) + 250_000.0).abs() < 1.0);
    }

    #[test]
    fn noise_has_its_power_and_no_dc() {
        let mut generator = Generator::new(config(Some(-30.0), vec![]));
        let samples = take(&mut generator, 200_000);
        assert!((power(&samples) / 0.001 - 1.0).abs() < 0.02);
        let mean: Iq = samples.iter().sum::<Iq>() / samples.len() as f32;
        assert!(mean.norm() < 0.001);
    }

    #[test]
    fn the_same_seed_repeats_and_another_differs() {
        let a = take(&mut Generator::new(config(Some(-30.0), vec![])), 100);
        let b = take(&mut Generator::new(config(Some(-30.0), vec![])), 100);
        let mut other = config(Some(-30.0), vec![]);
        other.seed = 43;
        let c = take(&mut Generator::new(other), 100);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn reading_in_pieces_gives_the_same_stream() {
        let cfg = config(Some(-40.0), vec![tone(CENTER + 12_345.0, -10.0)]);
        let whole = take(&mut Generator::new(cfg.clone()), 1000);
        let mut generator = Generator::new(cfg);
        let mut pieces = take(&mut generator, 300);
        pieces.extend(take(&mut generator, 700));
        assert_eq!(whole, pieces);
    }

    #[test]
    fn a_gated_tone_is_on_only_at_the_start_of_each_period() {
        let gate = Gate {
            period_s: 0.01,
            on_s: 0.002,
        };
        let gated = Tone {
            freq_hz: CENTER,
            level_dbfs: 0.0,
            gate: Some(gate),
        };
        let mut generator = Generator::new(config(None, vec![gated]));
        // 10 ms periods at 1 MS/s: on for samples 0..2000, off for 2000..10000.
        let samples = take(&mut generator, 25_000);
        let on = |i: usize| samples[i].norm() > 0.5;
        assert!(on(0) && on(1999));
        assert!(!on(2000) && !on(9999));
        assert!(on(10_000) && on(11_999));
        assert!(!on(12_000));
        assert!(on(20_000));
    }

    #[test]
    fn tuning_moves_the_offset_of_a_tone() {
        let mut generator = Generator::new(config(None, vec![tone(CENTER + 100_000.0, 0.0)]));
        generator.tune(CENTER + 150_000.0).unwrap();
        assert_eq!(generator.info().center_hz, CENTER + 150_000.0);
        let samples = take(&mut generator, 10_000);
        assert!((frequency_offset(&samples) + 50_000.0).abs() < 1.0);
    }

    #[test]
    fn the_demo_scene_never_ends() {
        let mut generator = Generator::new(GeneratorConfig::demo());
        let mut buf = vec![Iq::new(0.0, 0.0); 4096];
        for _ in 0..10 {
            assert_eq!(generator.read(&mut buf).unwrap(), buf.len());
        }
    }
}

/// The generator's cost per sample, which decides whether the demo scene can
/// run in real time on the CM4:
/// `cargo test --release -p sdr-core -- --ignored --nocapture bench_`
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    fn ns_per_sample(config: GeneratorConfig) -> f64 {
        let mut generator = Generator::new(config);
        let mut buf = vec![Iq::new(0.0, 0.0); 1 << 16];
        let rounds = 40;
        let started = Instant::now();
        for _ in 0..rounds {
            generator.read(&mut buf).unwrap();
        }
        started.elapsed().as_nanos() as f64 / (rounds * buf.len()) as f64
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn bench_generator() {
        let demo = GeneratorConfig::demo();
        let rate = f64::from(demo.sample_rate);
        let report = |name: &str, ns: f64| {
            println!(
                "{name:>12}: {ns:5.1} ns/sample, {:4.1}% of a core at 2.4 MS/s",
                ns * rate / 1e7
            );
        };
        report("demo", ns_per_sample(demo.clone()));
        report(
            "tones only",
            ns_per_sample(GeneratorConfig {
                noise_dbfs: None,
                ..demo.clone()
            }),
        );
        report(
            "noise only",
            ns_per_sample(GeneratorConfig {
                tones: vec![],
                ..demo
            }),
        );
    }
}
