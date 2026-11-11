//! A small seeded generator, so synthetic signals repeat exactly in tests.

/// SplitMix64: fast, good enough for noise, and the same on every platform.
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1]: never 0, so `ln` is safe.
    pub(crate) fn next_unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    /// Two independent standard normal values (Box–Muller).
    pub(crate) fn next_gaussian_pair(&mut self) -> (f64, f64) {
        let radius = (-2.0 * self.next_unit().ln()).sqrt();
        let angle = std::f64::consts::TAU * self.next_unit();
        (radius * angle.cos(), radius * angle.sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_has_zero_mean_and_unit_variance() {
        let mut rng = SplitMix64::new(7);
        let n = 100_000;
        let (mut sum, mut sum_sq) = (0.0, 0.0);
        for _ in 0..n / 2 {
            let (a, b) = rng.next_gaussian_pair();
            sum += a + b;
            sum_sq += a * a + b * b;
        }
        let mean = sum / n as f64;
        let variance = sum_sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((variance - 1.0).abs() < 0.02, "variance {variance}");
    }
}
