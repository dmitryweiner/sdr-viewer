//! The picture of one event: a small spectrogram cut around it, time
//! across and frequency up. The state and the arithmetic; no drawing.

use std::time::Duration;

/// How far below the picture's own noise its scale starts, and its height
/// in dB: as on the spectrum screen.
const BELOW_NOISE_DB: f32 = 2.0;
const RANGE_DB: f32 = 50.0;

/// Spectra of the samples around an event, a little wider than its band.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Stream time of the first column.
    pub start: Duration,
    /// From one column to the next.
    pub step: Duration,
    pub low_hz: f64,
    pub high_hz: f64,
    /// One spectrum per moment, in dBFS, the lowest frequency first; all
    /// of one length.
    pub columns: Vec<Vec<f32>>,
}

/// What the log has of the selected event's picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Picture {
    /// Asked for.
    Waiting,
    /// The samples are no longer held, or never were.
    Gone,
    Ready(Fragment),
}

impl Fragment {
    /// How long the picture is.
    pub fn duration(&self) -> Duration {
        self.step * u32::try_from(self.columns.len()).unwrap_or(u32::MAX)
    }

    /// Where `time` of the stream falls across the picture, 0 = its left
    /// edge, 1 = its right; outside it — the nearer edge.
    pub fn across(&self, time: Duration) -> f64 {
        let length = self.duration().as_secs_f64();
        if length == 0.0 {
            return 0.0;
        }
        let from_start = time.saturating_sub(self.start).as_secs_f64();
        (from_start / length).min(1.0)
    }

    /// The picture as `height` rows of `width` pixels, the highest
    /// frequency first: each pixel the strongest of what falls into it,
    /// so a short narrow burst never falls between pixels.
    pub fn pixels(&self, width: usize, height: usize) -> Vec<Vec<f32>> {
        let moments = self.columns.len();
        let bins = self.columns.first().map_or(0, Vec::len);
        if moments == 0 || bins == 0 {
            return vec![vec![f32::MIN; width]; height];
        }
        let span = |index: usize, of: usize, from: usize| {
            let start = (index * from / of).min(from - 1);
            let end = ((index + 1) * from / of).max(start + 1).min(from);
            start..end
        };
        (0..height)
            .map(|row| {
                let bins = span(height - 1 - row, height, bins);
                (0..width)
                    .map(|x| {
                        self.columns[span(x, width, moments)]
                            .iter()
                            .flat_map(|column| &column[bins.clone()])
                            .copied()
                            .fold(f32::MIN, f32::max)
                    })
                    .collect()
            })
            .collect()
    }

    /// The dB at the bottom and the top of the picture's colours: from
    /// just under its own noise, the median of all it holds.
    pub fn range(&self) -> (f32, f32) {
        let mut all: Vec<f32> = self.columns.iter().flatten().copied().collect();
        if all.is_empty() {
            return (-RANGE_DB, 0.0);
        }
        let middle = all.len() / 2;
        let (_, median, _) = all.select_nth_unstable_by(middle, f32::total_cmp);
        let bottom = *median - BELOW_NOISE_DB;
        (bottom, bottom + RANGE_DB)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 8 moments 10 ms apart from 1 s, 4 bins from 100 to 104 kHz: noise
    /// at -80 and a signal at -30 in bin 1 of moments 2 to 5.
    pub(crate) fn fragment() -> Fragment {
        let columns = (0..8)
            .map(|moment| {
                let mut column = vec![-80.0; 4];
                if (2..6).contains(&moment) {
                    column[1] = -30.0;
                }
                column
            })
            .collect();
        Fragment {
            start: Duration::from_secs(1),
            step: Duration::from_millis(10),
            low_hz: 100e3,
            high_hz: 104e3,
            columns,
        }
    }

    #[test]
    fn pixels_are_rows_from_the_highest_frequency_down() {
        let pixels = fragment().pixels(8, 4);
        assert_eq!(pixels.len(), 4);
        assert_eq!(pixels[0], [-80.0; 8]);
        // Bin 1 is the third row from the top.
        assert_eq!(
            pixels[2],
            [-80.0, -80.0, -30.0, -30.0, -30.0, -30.0, -80.0, -80.0]
        );
    }

    #[test]
    fn a_smaller_picture_keeps_the_strongest_of_each_pixel() {
        let pixels = fragment().pixels(2, 2);
        // The signal is in the lower half of the band, in both halves of
        // the time.
        assert_eq!(pixels, [[-80.0, -80.0], [-30.0, -30.0]]);
    }

    #[test]
    fn a_larger_picture_repeats_what_it_has() {
        let pixels = fragment().pixels(16, 8);
        assert_eq!(pixels.len(), 8);
        assert_eq!(pixels[4][3], -80.0);
        assert_eq!(pixels[4][4], -30.0);
        assert_eq!(pixels[5][11], -30.0);
        assert_eq!(pixels[5][12], -80.0);
        assert_eq!(pixels[6][4], -80.0);
    }

    #[test]
    fn an_empty_fragment_is_a_dark_picture() {
        let empty = Fragment {
            columns: Vec::new(),
            ..fragment()
        };
        assert_eq!(empty.pixels(2, 1), [[f32::MIN, f32::MIN]]);
        assert_eq!(empty.range(), (-50.0, 0.0));
        assert_eq!(empty.across(Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn the_colours_start_just_under_the_pictures_noise() {
        assert_eq!(fragment().range(), (-82.0, -32.0));
    }

    #[test]
    fn a_moment_of_the_stream_has_its_place_across_the_picture() {
        let fragment = fragment();
        assert_eq!(fragment.duration(), Duration::from_millis(80));
        assert_eq!(fragment.across(Duration::from_millis(1_020)), 0.25);
        assert_eq!(fragment.across(Duration::from_millis(1_080)), 1.0);
        assert_eq!(fragment.across(Duration::from_millis(500)), 0.0);
        assert_eq!(fragment.across(Duration::from_secs(9)), 1.0);
    }
}
