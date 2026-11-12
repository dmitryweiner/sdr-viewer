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
            let mut at = ((self.next + skip as u64) % capacity as u64) as usize;
            for sample in &samples[skip..] {
                self.samples[at] = [quantize(sample.re), quantize(sample.im)];
                at = if at + 1 == capacity { 0 } else { at + 1 };
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

/// A part of a sample as `rtl_sdr` writes it: 127.5 is zero.
fn quantize(part: f32) -> u8 {
    (part * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8
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
    fn samples_beyond_full_scale_are_clipped() {
        let mut ring = IqRing::new(1);
        ring.push(&[Iq::new(3.0, -3.0)]);
        assert_eq!(ring.copy(0..1), [[255, 0]]);
    }
}
