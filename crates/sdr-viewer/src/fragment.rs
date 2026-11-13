//! The picture of an event from its samples: spectra through the cut,
//! cropped to a band a little wider than the event.

use std::time::Duration;

use sdr_core::Iq;
use sdr_core::file::SampleFormat;
use sdr_dsp::{Analyzer, AnalyzerConfig};
use sdr_tui::fragment::Fragment;

use crate::pipeline::FFT_SIZE;
use crate::reader::Cut;

/// The most moments a picture has: more than a terminal has columns, and
/// few enough FFTs (a tenth of a second of a CM4 core) for any event.
const MAX_COLUMNS: usize = 512;

/// The picture's band: so many times the event's width, and at least so
/// many hertz, around its middle.
const WIDTHS: f64 = 4.0;
const MIN_SPAN_HZ: f64 = 40_000.0;

/// One spectrum of `FFT_SIZE` samples (0.85 ms at 2.4 MS/s) per moment;
/// of a long cut every n-th block, so that there are at most
/// `MAX_COLUMNS`.
pub fn of(cut: &Cut) -> Fragment {
    let info = &cut.source;
    let rate = f64::from(info.sample_rate);
    let event = &cut.event;
    let span = (WIDTHS * event.width_hz()).max(MIN_SPAN_HZ);
    let bin = |hz: f64| {
        let place = ((hz - info.center_hz) / rate + 0.5) * FFT_SIZE as f64;
        place.round().clamp(0.0, FFT_SIZE as f64) as usize
    };
    let (low, high) = (
        bin(event.center_hz() - span / 2.0),
        bin(event.center_hz() + span / 2.0),
    );
    let edge = |bin: usize| info.center_hz + (bin as f64 / FFT_SIZE as f64 - 0.5) * rate;

    let blocks = cut.samples.len() / FFT_SIZE;
    let every = blocks.div_ceil(MAX_COLUMNS).max(1);
    let mut analyzer = Analyzer::new(AnalyzerConfig {
        fft_size: FFT_SIZE,
        average: 1,
    });
    let mut block = vec![Iq::new(0.0, 0.0); FFT_SIZE];
    let mut columns = Vec::with_capacity(blocks.div_ceil(every));
    for bytes in cut.samples.as_chunks::<FFT_SIZE>().0.iter().step_by(every) {
        SampleFormat::Cu8.decode(bytes.as_flattened(), &mut block);
        analyzer.push(&block, info, |frame| {
            columns.push(frame.power_db[low..high].to_vec());
        });
    }
    Fragment {
        start: Duration::from_secs_f64(cut.first_sample as f64 / rate),
        step: Duration::from_secs_f64((every * FFT_SIZE) as f64 / rate),
        low_hz: edge(low),
        high_hz: edge(high),
        columns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::generator::{Generator, GeneratorConfig};
    use sdr_core::{IqRing, Source};
    use sdr_detect::Event;

    /// `seconds` of the demo scene from its start, as the ring holds
    /// them, cut for its burst: 434.2 MHz, on for the first 0.3 s.
    fn cut(seconds: f64) -> Cut {
        let mut generator = Generator::new(GeneratorConfig::demo());
        let mut buf = vec![Iq::new(0.0, 0.0); (seconds * 2.4e6) as usize];
        generator.read(&mut buf).unwrap();
        let mut ring = IqRing::new(buf.len());
        ring.push(&buf);
        Cut {
            samples: ring.copy(0..buf.len() as u64),
            first_sample: 0,
            source: generator.info(),
            event: Event {
                start: Duration::ZERO,
                duration: Duration::from_millis(300),
                low_hz: 434.195e6,
                high_hz: 434.205e6,
                peak_hz: 434.2e6,
                peak_dbfs: -25.0,
                above_floor_db: 66.0,
            },
        }
    }

    fn strongest(column: &[f32]) -> (usize, f32) {
        let best = column.iter().copied().enumerate();
        best.max_by(|a, b| a.1.total_cmp(&b.1)).unwrap()
    }

    #[test]
    fn the_picture_shows_the_burst_while_it_is_on_in_the_middle_of_its_band() {
        let fragment = of(&cut(0.4));
        // A spectrum every 2048 samples, 0.853 ms.
        assert_eq!(fragment.columns.len(), 960_000 / 2048);
        assert_eq!(fragment.start, Duration::ZERO);
        assert_eq!(fragment.step, Duration::from_secs_f64(2048.0 / 2.4e6));
        // 40 kHz around 434.2 MHz, to the bin: 34 bins of 1.17 kHz.
        assert_eq!(fragment.columns[0].len(), 34);
        assert!((fragment.low_hz - 434.18e6).abs() < 1_200.0, "{fragment:?}");
        assert!((fragment.high_hz - 434.22e6).abs() < 1_200.0);
        let (bin, db) = strongest(&fragment.columns[100]);
        assert_eq!(bin, 17);
        assert!((db + 25.0).abs() < 1.0, "{db}");
        // 0.34 s in, the burst is over.
        let (_, db) = strongest(&fragment.columns[400]);
        assert!(db < -60.0, "{db}");
    }

    #[test]
    fn a_long_cut_is_looked_at_only_so_often() {
        let fragment = of(&cut(3.0));
        // 3515 blocks: every 7th.
        assert_eq!(fragment.columns.len(), 503);
        assert_eq!(fragment.step, Duration::from_secs_f64(7.0 * 2048.0 / 2.4e6));
        assert!((fragment.duration().as_secs_f64() - 3.0).abs() < 0.01);
    }

    #[test]
    fn the_band_stops_at_the_edge_of_what_was_received() {
        let mut cut = cut(0.1);
        cut.event.low_hz = 435.11e6;
        cut.event.high_hz = 435.12e6;
        let fragment = of(&cut);
        assert_eq!(fragment.high_hz, 433.92e6 + 1.2e6);
        assert!((fragment.low_hz - 435.095e6).abs() < 1_200.0);
        assert_eq!(fragment.columns[0].len(), 21);
    }

    #[test]
    fn a_cut_shorter_than_one_spectrum_is_an_empty_picture() {
        let mut cut = cut(0.1);
        cut.samples.truncate(1000);
        assert!(of(&cut).columns.is_empty());
    }
}
