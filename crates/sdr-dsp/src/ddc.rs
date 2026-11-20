//! Down-conversion: one signal of the received band moved to zero and the
//! band narrowed around it, so that it takes a fiftieth of the samples.
//!
//! Two stages, each a low-pass filter worked out only for the samples it
//! keeps: first a cheap one (three moving sums, the response of a
//! third-order CIC filter) that takes the rate down to about four times
//! the output's, then a windowed sinc for the last factor of up to four,
//! which sets the edge of the band.

use sdr_core::Iq;

/// The narrowest output rate: a narrow signal keeps room around it for
/// its drift and its neighbours' edges.
pub const MIN_OUTPUT_RATE: f64 = 50_000.0;

/// The output rate is at least this many times the signal's width.
const RATE_PER_WIDTH: f64 = 2.5;

/// The second stage's share of the decimation: the first stage's
/// filter is good only far from the band, so it leaves this much room.
const LAST_FACTOR: usize = 4;

/// Of the output rate, the band kept flat on either side of zero, and
/// where the stopband begins.
const PASS: f64 = 0.4;
const STOP: f64 = 0.6;

/// The phasor is set back to unit length this often, against rounding:
/// in 32 bits it drifts by less than 0.1 Hz.
const RENORMALISE_EVERY: usize = 1024;

/// A low-pass filter worked out once every `factor` samples.
#[derive(Debug, Clone)]
struct Decimator {
    taps: Vec<f32>,
    factor: usize,
    /// The samples not yet used up, oldest first: the filter's history
    /// and what came since.
    held: Vec<Iq>,
}

impl Decimator {
    fn new(taps: Vec<f32>, factor: usize) -> Self {
        // History of zeros: the first outputs rise from nothing.
        let held = vec![Iq::new(0.0, 0.0); taps.len() - 1];
        Decimator { taps, factor, held }
    }

    fn push(&mut self, input: &[Iq], out: &mut Vec<Iq>) {
        self.held.extend_from_slice(input);
        let taps = self.taps.len();
        let mut at = 0;
        while at + taps <= self.held.len() {
            let window = &self.held[at..at + taps];
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (sample, tap) in window.iter().zip(&self.taps) {
                re += sample.re * tap;
                im += sample.im * tap;
            }
            out.push(Iq::new(re, im));
            at += self.factor;
        }
        self.held.drain(..at);
    }
}

/// Taps of a third-order CIC filter decimating by `factor`: three moving
/// sums of `factor` in a row, gain 1 at zero. Its nulls sit on the
/// frequencies that fold onto zero.
fn cic_taps(factor: usize) -> Vec<f32> {
    let boxcar = vec![1.0f64; factor];
    let twice = convolve(&boxcar, &boxcar);
    let thrice = convolve(&twice, &boxcar);
    let sum: f64 = thrice.iter().sum();
    thrice.iter().map(|tap| (tap / sum) as f32).collect()
}

fn convolve(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; a.len() + b.len() - 1];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            out[i + j] += x * y;
        }
    }
    out
}

/// A Blackman-windowed sinc decimating by `factor`: flat to `PASS` of
/// the output rate, down from `STOP` on, gain 1 at zero.
fn sinc_taps(factor: usize) -> Vec<f32> {
    // The transition, as a share of the input rate; a Blackman window
    // needs about 5.5 over it taps.
    let transition = (STOP - PASS) / factor as f64;
    let count = ((5.5 / transition).ceil() as usize) | 1;
    let cutoff = (PASS + STOP) / 2.0 / factor as f64;
    let middle = (count - 1) as f64 / 2.0;
    let taps: Vec<f64> = (0..count)
        .map(|index| {
            let t = index as f64 - middle;
            let sinc = if t == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * t).sin() / (std::f64::consts::PI * t)
            };
            let phase = std::f64::consts::TAU * index as f64 / (count - 1) as f64;
            let blackman = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            sinc * blackman
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter().map(|tap| (tap / sum) as f32).collect()
}

/// The decimation for a signal `width_hz` wide out of `input_rate`: as
/// much as keeps the output at [`MIN_OUTPUT_RATE`] and at
/// `RATE_PER_WIDTH` widths; above `LAST_FACTOR` × 2 a multiple of
/// `LAST_FACTOR`, so that the second stage has its share.
pub fn decimation_for(input_rate: u32, width_hz: f64) -> usize {
    let wanted = (RATE_PER_WIDTH * width_hz).max(MIN_OUTPUT_RATE);
    let most = (f64::from(input_rate) / wanted).floor().max(1.0) as usize;
    if most >= 2 * LAST_FACTOR {
        most / LAST_FACTOR * LAST_FACTOR
    } else {
        most
    }
}

/// Moves the signal at `offset_hz` from the band's centre to zero and
/// narrows the band to it.
#[derive(Debug, Clone)]
pub struct Downconverter {
    input_rate: u32,
    decimation: usize,
    phasor: Iq,
    rotation: Iq,
    since_renormalised: usize,
    first: Decimator,
    last: Decimator,
    mixed: Vec<Iq>,
    between: Vec<Iq>,
}

impl Downconverter {
    /// For a signal `width_hz` wide at `offset_hz` from the centre of a
    /// band received at `input_rate`.
    pub fn new(input_rate: u32, offset_hz: f64, width_hz: f64) -> Self {
        let decimation = decimation_for(input_rate, width_hz);
        let last = decimation.min(LAST_FACTOR);
        let first = decimation / last;
        let turn = -std::f64::consts::TAU * offset_hz / f64::from(input_rate);
        Downconverter {
            input_rate,
            decimation,
            phasor: Iq::new(1.0, 0.0),
            rotation: Iq::new(turn.cos() as f32, turn.sin() as f32),
            since_renormalised: 0,
            first: Decimator::new(cic_taps(first), first),
            last: Decimator::new(sinc_taps(last), last),
            mixed: Vec::new(),
            between: Vec::new(),
        }
    }

    /// How many input samples make one output sample.
    pub fn decimation(&self) -> usize {
        self.decimation
    }

    pub fn output_rate(&self) -> f64 {
        f64::from(self.input_rate) / self.decimation as f64
    }

    /// Takes the next input samples; `out` gets the output samples they
    /// complete.
    pub fn push(&mut self, input: &[Iq], out: &mut Vec<Iq>) {
        self.mixed.clear();
        for sample in input {
            self.mixed.push(sample * self.phasor);
            self.phasor *= self.rotation;
            self.since_renormalised += 1;
            if self.since_renormalised == RENORMALISE_EVERY {
                self.phasor /= self.phasor.norm();
                self.since_renormalised = 0;
            }
        }
        self.between.clear();
        self.first.push(&self.mixed, &mut self.between);
        self.last.push(&self.between, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 2_400_000;

    /// `seconds` of a tone at `hz` from the centre, amplitude `level`.
    fn tone(hz: f64, level: f32, seconds: f64) -> Vec<Iq> {
        let count = (seconds * f64::from(RATE)) as usize;
        (0..count)
            .map(|index| {
                let phase = std::f64::consts::TAU * hz * index as f64 / f64::from(RATE);
                Iq::new(phase.cos() as f32, phase.sin() as f32) * level
            })
            .collect()
    }

    /// The output of `input` given in pieces of `piece` samples.
    fn narrowed(ddc: &mut Downconverter, input: &[Iq], piece: usize) -> Vec<Iq> {
        let mut out = Vec::new();
        for part in input.chunks(piece) {
            ddc.push(part, &mut out);
        }
        out
    }

    /// The mean power of the output after its filters have filled.
    fn power(out: &[Iq]) -> f32 {
        let settled = &out[out.len() / 4..];
        settled.iter().map(|s| s.norm_sqr()).sum::<f32>() / settled.len() as f32
    }

    #[test]
    fn a_narrow_signal_goes_to_fifty_thousand_samples_a_second() {
        assert_eq!(decimation_for(RATE, 10e3), 48);
        assert_eq!(Downconverter::new(RATE, 0.0, 10e3).output_rate(), 50_000.0);
        // A wider one keeps two and a half widths; the widest, everything.
        assert_eq!(decimation_for(RATE, 100e3), 8);
        assert_eq!(decimation_for(RATE, 200e3), 4);
        assert_eq!(decimation_for(RATE, 2e6), 1);
        // A slow source keeps what it has.
        assert_eq!(decimation_for(250_000, 10e3), 5);
    }

    #[test]
    fn the_signal_comes_out_at_zero_at_its_own_level() {
        // A tone 1 kHz above the signal's place at +300 kHz.
        let mut ddc = Downconverter::new(RATE, 300e3, 10e3);
        let out = narrowed(&mut ddc, &tone(301e3, 0.5, 0.1), 16_384);
        assert_eq!(out.len(), 5_000);
        assert!((power(&out) - 0.25).abs() < 0.01, "{}", power(&out));
        // At 1 kHz: a turn every 50 samples.
        let settled = &out[2_000..];
        let turn: f64 = settled
            .windows(2)
            .map(|pair| f64::from((pair[1] * pair[0].conj()).arg()))
            .sum::<f64>()
            / (settled.len() - 1) as f64;
        let hz = turn / std::f64::consts::TAU * 50_000.0;
        assert!((hz - 1_000.0).abs() < 1.0, "{hz}");
    }

    #[test]
    fn what_is_outside_the_band_is_left_out() {
        let mut ddc = Downconverter::new(RATE, 300e3, 10e3);
        let inside = power(&narrowed(&mut ddc, &tone(310e3, 1.0, 0.1), 16_384));
        // Next to the band, and where the output rate would fold onto
        // the signal (+50 kHz from it).
        for (hz, at_most_db) in [(335e3, -60.0), (350e3, -60.0), (500e3, -80.0)] {
            let mut ddc = Downconverter::new(RATE, 300e3, 10e3);
            let outside = power(&narrowed(&mut ddc, &tone(hz, 1.0, 0.1), 16_384));
            let db = 10.0 * (outside / inside).log10();
            assert!(db < at_most_db, "{hz} Hz: {db:.1} dB");
        }
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn bench_downconverter() {
        for width_hz in [10e3, 200e3] {
            let input = tone(305e3, 0.3, 0.5);
            let mut ddc = Downconverter::new(RATE, 300e3, width_hz);
            let mut out = Vec::new();
            let started = std::time::Instant::now();
            for part in input.chunks(16 * 1024) {
                out.clear();
                ddc.push(std::hint::black_box(part), &mut out);
            }
            let per_sample = started.elapsed().as_secs_f64() / input.len() as f64;
            println!(
                "downconverter, {:.0} kHz wide, ÷{}: {:.2} ns/sample = {:.2}% of a core at 2.4 MS/s",
                width_hz / 1e3,
                ddc.decimation(),
                per_sample * 1e9,
                per_sample * 2.4e6 * 100.0
            );
        }
    }

    #[test]
    fn pieces_of_any_size_give_the_same_output() {
        let input = tone(305e3, 0.3, 0.02);
        let whole = narrowed(
            &mut Downconverter::new(RATE, 300e3, 10e3),
            &input,
            input.len(),
        );
        let pieces = narrowed(&mut Downconverter::new(RATE, 300e3, 10e3), &input, 7_777);
        assert_eq!(whole, pieces);
    }
}
