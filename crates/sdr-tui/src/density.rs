//! The arithmetic of the density screen: a [`DensityMap`] reduced to the
//! pixels of a plot, and a share of time turned into a brightness.

use std::ops::Range;

use sdr_detect::DensityMap;

/// The share of time drawn as the darkest colour: one detector frame of
/// ten thousand. From here to "always" the brightness grows with the
/// logarithm, so the rare is seen beside the constant.
pub const FAINTEST: f32 = 1e-4;

/// How bright a share of time is, 0 to 1; nothing for never.
pub fn brightness(share: f32) -> Option<f32> {
    (share > 0.0).then(|| (1.0 - share.log10() / FAINTEST.log10()).clamp(0.0, 1.0))
}

/// The map's `bins` as `width` columns and `rows` pixel rows over the dB
/// `range`, the lowest row first: `row * width + column`. A pixel holds
/// the largest share of time among the bins and levels that fall into it,
/// so neither a narrow signal nor a thin level is lost, and the picture
/// is as bright on a small plot as on a big one. What is off the scale
/// is in its end rows, as a bar of the spectrum stops at the top.
pub fn pixels(
    map: &DensityMap,
    bins: Range<usize>,
    width: usize,
    rows: usize,
    (bottom, top): (f32, f32),
) -> Vec<f32> {
    let mut pixels = vec![0.0f32; width * rows];
    let count = bins.len();
    if count == 0 || rows == 0 {
        return pixels;
    }
    let step = (top - bottom) / rows as f32;
    // The levels of each row: from where its bottom edge falls to where
    // the next row's does.
    let levels: Vec<Range<usize>> = (0..rows)
        .map(|row| {
            let from = match row {
                0 => 0,
                _ => map.level_of(bottom + step * row as f32),
            };
            let to = if row + 1 == rows {
                map.config.levels
            } else {
                map.level_of(bottom + step * (row + 1) as f32)
            };
            from..to.max(from + 1)
        })
        .collect();
    let mut strongest = vec![0.0f32; map.config.levels];
    for column in 0..width {
        let start = bins.start + column * count / width;
        let end = (bins.start + (column + 1) * count / width)
            .max(start + 1)
            .min(bins.end);
        strongest.fill(0.0);
        for bin in start.min(bins.end - 1)..end {
            for (most, share) in strongest.iter_mut().zip(map.shares(bin)) {
                *most = most.max(share);
            }
        }
        for (row, levels) in levels.iter().enumerate() {
            pixels[row * width + column] = strongest[levels.clone()]
                .iter()
                .copied()
                .fold(0.0, f32::max);
        }
    }
    pixels
}

/// The largest share of time one of `bins` spent above its `threshold`
/// (which is given for those bins, in their order).
pub fn busy(map: &DensityMap, bins: Range<usize>, threshold: &[f32]) -> f32 {
    bins.zip(threshold)
        .map(|(bin, &db)| map.above(bin, db))
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::SpectrumFrame;
    use sdr_detect::{Density, DensityConfig};
    use std::time::Duration;

    /// Four frames of eight bins: bin 2 always at -45, bin 5 once at -15,
    /// everything else at -95.
    fn map() -> DensityMap {
        let mut density = Density::new(DensityConfig {
            bottom_db: -100.0,
            step_db: 10.0,
            levels: 10,
            half_life: Duration::from_secs(1_000_000),
        });
        for i in 0..4 {
            let mut power_db = vec![-95.0; 8];
            power_db[2] = -45.0;
            if i == 3 {
                power_db[5] = -15.0;
            }
            density.push(&SpectrumFrame {
                time: Duration::from_millis(i * 10),
                center_hz: 100e6,
                sample_rate: 1_000_000,
                power_db,
            });
        }
        density.map().clone()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn brightness_grows_with_the_logarithm_of_the_share() {
        assert_eq!(brightness(0.0), None);
        assert_eq!(brightness(1.0), Some(1.0));
        assert!(close(brightness(0.01).unwrap(), 0.5));
        assert_eq!(brightness(FAINTEST), Some(0.0));
        assert_eq!(brightness(1e-9), Some(0.0));
        let shares = [1e-4, 1e-3, 0.02, 0.3, 1.0];
        assert!(shares.is_sorted_by(|a, b| brightness(*a) < brightness(*b)));
    }

    #[test]
    fn a_pixel_per_bin_and_level_is_the_map_itself() {
        let pixels = pixels(&map(), 0..8, 8, 10, (-100.0, 0.0));
        let at = |column: usize, row: usize| pixels[row * 8 + column];
        assert!(close(at(0, 0), 1.0));
        assert!(close(at(2, 5), 1.0) && at(2, 0) == 0.0);
        assert!(close(at(5, 0), 0.75) && close(at(5, 8), 0.25));
        assert_eq!(at(5, 5), 0.0);
    }

    #[test]
    fn a_pixel_of_many_bins_and_levels_keeps_the_largest_share() {
        // Two columns of four bins, two rows of 50 dB.
        let pixels = pixels(&map(), 0..8, 2, 2, (-100.0, 0.0));
        assert!(close(pixels[0], 1.0) && close(pixels[1], 1.0));
        assert!(close(pixels[2], 1.0), "{pixels:?}");
        assert!(close(pixels[3], 0.25), "{pixels:?}");
    }

    #[test]
    fn rows_thinner_than_a_level_repeat_it_and_leave_no_gaps() {
        // 5 dB a row over 10 dB levels, on the part of the band in view.
        let pixels = pixels(&map(), 2..4, 2, 8, (-60.0, -20.0));
        let column: Vec<f32> = (0..8).map(|row| pixels[row * 2]).collect();
        assert!(close(column[2], 1.0) && close(column[3], 1.0), "{column:?}");
        assert_eq!(column[1], 0.0);
        assert_eq!(column[4], 0.0);
        // The quiet bin beside it is below the plot: in its lowest row.
        assert!(close(pixels[1], 1.0));
        assert!((1..8).all(|row| pixels[row * 2 + 1] == 0.0));
    }

    #[test]
    fn what_is_above_the_scale_is_in_the_top_row() {
        let pixels = pixels(&map(), 0..8, 8, 4, (-100.0, -60.0));
        let at = |column: usize, row: usize| pixels[row * 8 + column];
        assert!(close(at(2, 3), 1.0) && close(at(5, 3), 0.25));
        assert!(close(at(5, 0), 0.75) && at(5, 1) == 0.0);
    }

    #[test]
    fn more_columns_than_bins_repeat_the_bins() {
        let pixels = pixels(&map(), 2..3, 4, 10, (-100.0, 0.0));
        assert!((0..4).all(|column| close(pixels[5 * 4 + column], 1.0)));
    }

    #[test]
    fn busy_is_the_largest_share_above_the_threshold() {
        let map = map();
        assert!(close(busy(&map, 4..7, &[-80.0; 3]), 0.25));
        assert!(close(busy(&map, 0..8, &[-80.0; 8]), 1.0));
        assert_eq!(busy(&map, 0..2, &[-80.0; 2]), 0.0);
    }
}
