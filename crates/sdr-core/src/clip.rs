//! Overload: samples the receiver could not hold.

use crate::Iq;

/// How many of `samples` have a part at full scale or beyond: the
/// converter ran out of range there, and what it gave is not the signal.
/// An 8-bit receiver's bytes 0 and 255 are exactly -1 and 1.
pub fn clipped(samples: &[Iq]) -> usize {
    samples
        .iter()
        .filter(|sample| sample.re.abs() >= 1.0 || sample.im.abs() >= 1.0)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SampleFormat;

    #[test]
    fn counts_samples_with_a_part_at_full_scale() {
        let samples = [
            Iq::new(0.5, -0.5),
            Iq::new(1.0, 0.0),
            Iq::new(0.0, -1.0),
            Iq::new(0.999, -0.999),
            Iq::new(-1.5, 2.0),
        ];
        assert_eq!(clipped(&samples), 3);
        assert_eq!(clipped(&[]), 0);
    }

    #[test]
    fn the_ends_of_a_byte_are_full_scale_and_its_neighbours_are_not() {
        let mut samples = [Iq::new(0.0, 0.0); 4];
        SampleFormat::Cu8.decode(&[0, 127, 128, 255, 1, 254, 127, 128], &mut samples);
        assert_eq!(clipped(&samples), 2);
    }
}
