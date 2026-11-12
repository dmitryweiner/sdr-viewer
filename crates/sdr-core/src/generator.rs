//! A synthetic source: tones and bursts over Gaussian noise, fully repeatable.

use num_complex::Complex64;

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
    /// Gaussian values already scaled to the noise level; empty for no noise.
    noise: Vec<f32>,
}

/// A tone is a unit vector turned by a fixed angle every sample: one complex
/// multiplication instead of a sine and a cosine (on the CM4 the
/// trigonometry made the generator 70% of a core, `docs/hardware.md`).
struct ToneState {
    amplitude: f32,
    phasor: Complex64,
    /// The turn per sample at the current centre frequency.
    rotation: Complex64,
    gate: Option<GateState>,
}

/// A gate counted in samples, so it is exact and needs no division.
struct GateState {
    period: u64,
    on: u64,
    position: u64,
}

/// Rounding makes a phasor's length drift; it is pulled back to 1 this often.
const RENORMALIZE_EVERY: u64 = 1024;

/// Size of the noise table, a power of two; 16 KiB fits the CM4's L1 cache.
const NOISE_TABLE: usize = 4096;

impl Generator {
    pub fn new(config: GeneratorConfig) -> Self {
        let rate = f64::from(config.sample_rate);
        let tones = config
            .tones
            .iter()
            .map(|tone| ToneState {
                amplitude: 10f32.powf(tone.level_dbfs / 20.0),
                phasor: Complex64::new(1.0, 0.0),
                rotation: Complex64::new(1.0, 0.0),
                gate: tone.gate.map(|gate| GateState {
                    period: ((gate.period_s * rate).round() as u64).max(1),
                    on: (gate.on_s * rate).round() as u64,
                    position: 0,
                }),
            })
            .collect();
        let mut rng = SplitMix64::new(config.seed);
        let noise = config
            .noise_dbfs
            .map_or_else(Vec::new, |db| noise_table(&mut rng, db));
        let mut generator = Generator {
            rng,
            index: 0,
            tones,
            noise,
            config,
        };
        generator.update_rotations();
        generator
    }

    fn update_rotations(&mut self) {
        let rate = f64::from(self.config.sample_rate);
        for (state, tone) in self.tones.iter_mut().zip(&self.config.tones) {
            let step = std::f64::consts::TAU * (tone.freq_hz - self.config.center_hz) / rate;
            state.rotation = Complex64::from_polar(1.0, step);
        }
    }

    fn next_sample(&mut self) -> Iq {
        let renormalize = self.index.is_multiple_of(RENORMALIZE_EVERY);
        let mut sample = Iq::new(0.0, 0.0);
        for tone in &mut self.tones {
            let on = match &mut tone.gate {
                None => true,
                Some(gate) => {
                    let on = gate.position < gate.on;
                    gate.position += 1;
                    if gate.position == gate.period {
                        gate.position = 0;
                    }
                    on
                }
            };
            if renormalize {
                tone.phasor /= tone.phasor.norm();
            }
            if on {
                sample += Iq::new(tone.phasor.re as f32, tone.phasor.im as f32) * tone.amplitude;
            }
            tone.phasor *= tone.rotation;
        }
        if !self.noise.is_empty() {
            // One random word picks both values; the table is far shorter
            // than any FFT that will look at the noise is sensitive to.
            let bits = self.rng.next_u64();
            let i = self.noise[bits as usize % NOISE_TABLE];
            let q = self.noise[(bits >> 32) as usize % NOISE_TABLE];
            sample += Iq::new(i, q);
        }
        self.index += 1;
        sample
    }
}

/// Gaussian values with exactly zero mean, scaled so that I and Q drawn from
/// the table carry `noise_dbfs` of power between them.
fn noise_table(rng: &mut SplitMix64, noise_dbfs: f32) -> Vec<f32> {
    let mut values: Vec<f64> = (0..NOISE_TABLE / 2)
        .flat_map(|_| {
            let (a, b) = rng.next_gaussian_pair();
            [a, b]
        })
        .collect();
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    values.iter_mut().for_each(|v| *v -= mean);
    let variance = values.iter().map(|v| v * v).sum::<f64>() / n;
    // Complex noise: the power splits evenly between I and Q.
    let sigma = (10f64.powf(f64::from(noise_dbfs) / 10.0) / 2.0).sqrt();
    values
        .iter()
        .map(|v| (v * sigma / variance.sqrt()) as f32)
        .collect()
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
        self.update_rotations();
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
    fn a_tone_keeps_its_amplitude_and_frequency_over_a_long_run() {
        let mut generator = Generator::new(config(None, vec![tone(CENTER + 123_456.7, 0.0)]));
        let mut samples = Vec::new();
        for _ in 0..50 {
            samples = take(&mut generator, 100_000);
        }
        assert!(samples.iter().all(|s| (s.norm() - 1.0).abs() < 1e-5));
        assert!((frequency_offset(&samples) - 123_456.7).abs() < 0.1);
    }

    #[test]
    fn noise_is_white_and_i_is_independent_of_q() {
        let mut generator = Generator::new(config(Some(0.0), vec![]));
        let samples = take(&mut generator, 400_000);
        let n = samples.len() as f64;
        // Each of these is ~N(0, 1/sqrt(n)) = 0.0016 for independent values.
        let i_q: f64 = samples.iter().map(|s| f64::from(s.re * s.im)).sum::<f64>() / n * 2.0;
        let lag1: f64 = samples
            .windows(2)
            .map(|w| f64::from(w[0].re * w[1].re))
            .sum::<f64>()
            / n
            * 2.0;
        let lag7: f64 = samples
            .windows(8)
            .map(|w| f64::from(w[0].im * w[7].im))
            .sum::<f64>()
            / n
            * 2.0;
        for (name, value) in [("I·Q", i_q), ("lag 1", lag1), ("lag 7", lag7)] {
            assert!(value.abs() < 0.01, "{name}: {value}");
        }
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
