//! The last seconds of the stream, kept so that a signal can be saved
//! after it was noticed — from before its start.

use std::ops::Range;

use crate::Iq;

/// The newest `capacity` samples of a stream, 8 bits a part: what an
/// RTL-SDR gives, unchanged, in a quarter of the memory of `Iq`. Samples
/// are numbered from the first one pushed, whatever has been forgotten
/// since.
pub struct IqRing {
    /// Sample number `n` lives at `n % capacity`.
    samples: Vec<[u8; 2]>,
    /// The number of the next sample.
    next: u64,
    held: usize,
}

impl Default for IqRing {
    /// A ring that keeps nothing.
    fn default() -> Self {
        IqRing::new(0)
    }
}

impl IqRing {
    /// A ring for `capacity` samples; with 0 it keeps nothing.
    pub fn new(capacity: usize) -> Self {
        IqRing {
            // Zeroed memory costs nothing until it is written to.
            samples: vec![[0; 2]; capacity],
            next: 0,
            held: 0,
        }
    }

    /// A ring for `seconds` of a stream at `sample_rate`.
    pub fn for_seconds(seconds: f64, sample_rate: u32) -> Self {
        IqRing::new((seconds.max(0.0) * f64::from(sample_rate)).round() as usize)
    }

    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    pub fn push(&mut self, samples: &[Iq]) {
        let capacity = self.samples.len();
        let count = samples.len() as u64;
        if capacity > 0 {
            // Of more than fits only the end is worth writing.
            let skip = samples.len().saturating_sub(capacity);
            let at = ((self.next + skip as u64) % capacity as u64) as usize;
            // Up to the end of the ring, then on from its start: two plain
            // runs, which the compiler turns into vector code.
            let (to_end, from_start) =
                samples[skip..].split_at((capacity - at).min(count as usize - skip));
            let (wrapped, tail) = self.samples.split_at_mut(at);
            for (slot, sample) in tail.iter_mut().zip(to_end) {
                *slot = [quantize(sample.re), quantize(sample.im)];
            }
            for (slot, sample) in wrapped.iter_mut().zip(from_start) {
                *slot = [quantize(sample.re), quantize(sample.im)];
            }
            self.held = (self.held + samples.len()).min(capacity);
        }
        self.next += count;
    }

    /// Forgets what is held — it was another band — and goes on counting.
    pub fn clear(&mut self) {
        self.held = 0;
    }

    /// The numbers of the samples held.
    pub fn held(&self) -> Range<u64> {
        self.next - self.held as u64..self.next
    }

    /// The samples of `range` that are held, oldest first, as `cu8` pairs.
    pub fn copy(&self, range: Range<u64>) -> Vec<[u8; 2]> {
        let held = self.held();
        let (start, end) = (range.start.max(held.start), range.end.min(held.end));
        if start >= end {
            return Vec::new();
        }
        let capacity = self.samples.len();
        let from = (start % capacity as u64) as usize;
        let count = (end - start) as usize;
        let first = count.min(capacity - from);
        let mut out = Vec::with_capacity(count);
        out.extend_from_slice(&self.samples[from..from + first]);
        out.extend_from_slice(&self.samples[..count - first]);
        out
    }
}

/// A part of a sample as `rtl_sdr` writes it: 127.5 is zero. Rounded to
/// the nearest by the half added before the cast, which cuts the fraction
/// off and stops at 0 and 255.
fn quantize(part: f32) -> u8 {
    (part * 127.5 + 128.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SampleFormat;

    /// Samples whose bytes are `first`, `first + 1`, … for I, and 255 less
    /// that for Q.
    fn ramp(first: u8, count: u8) -> Vec<Iq> {
        let bytes: Vec<u8> = (first..first + count).flat_map(|b| [b, 255 - b]).collect();
        let mut out = vec![Iq::new(0.0, 0.0); usize::from(count)];
        SampleFormat::Cu8.decode(&bytes, &mut out);
        out
    }

    #[test]
    fn holds_what_was_pushed_as_the_bytes_it_came_from() {
        let mut ring = IqRing::new(8);
        assert_eq!(ring.held(), 0..0);
        ring.push(&ramp(0, 3));
        ring.push(&ramp(3, 2));
        assert_eq!(ring.held(), 0..5);
        assert_eq!(
            ring.copy(0..5),
            [[0, 255], [1, 254], [2, 253], [3, 252], [4, 251]]
        );
        assert_eq!(ring.copy(1..3), [[1, 254], [2, 253]]);
    }

    #[test]
    fn every_byte_of_a_cu8_stream_survives() {
        let mut ring = IqRing::new(256);
        ring.push(&ramp(0, 255));
        let copy = ring.copy(0..255);
        assert!(copy.iter().enumerate().all(|(i, pair)| pair[0] == i as u8));
    }

    #[test]
    fn forgets_the_oldest_and_keeps_their_numbers() {
        let mut ring = IqRing::new(4);
        ring.push(&ramp(0, 3));
        ring.push(&ramp(3, 3));
        assert_eq!(ring.held(), 2..6);
        assert_eq!(ring.copy(0..100), [[2, 253], [3, 252], [4, 251], [5, 250]]);
        assert_eq!(ring.copy(4..5), [[4, 251]]);
        assert!(ring.copy(0..2).is_empty());
        assert!(ring.copy(6..9).is_empty());
    }

    #[test]
    fn of_a_push_longer_than_the_ring_the_end_is_kept() {
        let mut ring = IqRing::new(4);
        ring.push(&ramp(0, 1));
        ring.push(&ramp(1, 10));
        assert_eq!(ring.held(), 7..11);
        assert_eq!(ring.copy(7..11), [[7, 248], [8, 247], [9, 246], [10, 245]]);
    }

    #[test]
    fn clearing_forgets_the_samples_but_not_the_count() {
        let mut ring = IqRing::new(4);
        ring.push(&ramp(0, 3));
        ring.clear();
        assert_eq!(ring.held(), 3..3);
        assert!(ring.copy(0..3).is_empty());
        ring.push(&ramp(3, 2));
        assert_eq!(ring.held(), 3..5);
        assert_eq!(ring.copy(0..9), [[3, 252], [4, 251]]);
    }

    #[test]
    fn a_ring_of_nothing_only_counts() {
        let mut ring = IqRing::new(0);
        ring.push(&ramp(0, 3));
        assert_eq!(ring.held(), 3..3);
        assert!(ring.copy(0..3).is_empty());
    }

    #[test]
    fn its_size_is_seconds_of_the_stream() {
        assert_eq!(IqRing::for_seconds(30.0, 2_400_000).capacity(), 72_000_000);
        assert_eq!(IqRing::for_seconds(0.0, 2_400_000).capacity(), 0);
        assert_eq!(IqRing::for_seconds(-1.0, 2_400_000).capacity(), 0);
    }

    #[test]
    fn a_sample_goes_to_the_nearest_step() {
        let mut ring = IqRing::new(2);
        // Steps are 1/127.5 apart; 0.3 and 0.7 of a step above byte 130.
        let at = |steps: f32| (130.0 + steps - 127.5) / 127.5;
        ring.push(&[Iq::new(at(0.3), at(0.7)), Iq::new(at(-0.3), at(-0.7))]);
        assert_eq!(ring.copy(0..2), [[130, 131], [130, 129]]);
    }

    /// `cargo test --release -p sdr-core -- --ignored --nocapture bench_ring`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn bench_ring() {
        let samples = vec![Iq::new(0.25, -0.5); 16 * 1024];
        let mut ring = IqRing::new(72_000_000);
        // Once around, so that no page is new.
        for _ in 0..72_000_000 / samples.len() + 1 {
            ring.push(&samples);
        }
        let started = std::time::Instant::now();
        let rounds = 2_000;
        for _ in 0..rounds {
            ring.push(std::hint::black_box(&samples));
        }
        let took = started.elapsed();
        std::hint::black_box(ring.copy(0..u64::MAX));
        let per_sample = took.as_secs_f64() / (rounds * samples.len()) as f64;
        println!(
            "ring: {:.2} ns/sample = {:.2}% of a core at 2.4 MS/s",
            per_sample * 1e9,
            per_sample * 2.4e6 * 100.0
        );
    }

    #[test]
    fn samples_beyond_full_scale_are_clipped() {
        let mut ring = IqRing::new(1);
        ring.push(&[Iq::new(3.0, -3.0)]);
        assert_eq!(ring.copy(0..1), [[255, 0]]);
    }
}
