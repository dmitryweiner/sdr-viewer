//! Events: what rose above the noise floor, where and for how long.

use std::time::Duration;

use sdr_core::SpectrumFrame;

use crate::floor::{FloorConfig, NoiseFloor};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectorConfig {
    /// A bin this far above the floor is part of a signal.
    pub threshold_db: f32,
    /// Parts of the spectrum closer than this are one signal.
    pub merge_hz: f64,
    /// A signal silent for no longer than this goes on as the same event:
    /// the pauses inside a keyed transmission and between its repeats.
    pub hold: Duration,
    /// A carrier switched on or off inside an FFT block splashes over the
    /// whole spectrum, falling 20 dB per tenfold distance in bins. What is
    /// weaker than a signal of the same frame by more than this many dB
    /// plus that fall is taken for its splash.
    pub splash_db: f32,
    pub floor: FloorConfig,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        DetectorConfig {
            threshold_db: 8.0,
            merge_hz: 5_000.0,
            hold: Duration::from_millis(100),
            splash_db: 10.0,
            floor: FloorConfig::default(),
        }
    }
}

/// What the detector holds a spectrum against, for whoever draws it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Levels {
    /// The noise floor per bin in dBFS; empty before the first frame.
    pub floor_db: Vec<f32>,
    /// A bin this far above its floor is part of a signal.
    pub threshold_db: f32,
}

/// One signal from its first frame to its last.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Stream time of the first frame it was in.
    pub start: Duration,
    /// Up to the end of the last frame it was in.
    pub duration: Duration,
    /// The band it filled in its strongest frame. The switching on and off
    /// splashes much wider for a frame and is left out.
    pub low_hz: f64,
    pub high_hz: f64,
    /// The strongest bin of the strongest frame.
    pub peak_hz: f64,
    pub peak_dbfs: f32,
    /// The peak over the noise floor at its frequency.
    pub above_floor_db: f32,
}

impl Event {
    pub fn center_hz(&self) -> f64 {
        (self.low_hz + self.high_hz) / 2.0
    }

    pub fn width_hz(&self) -> f64 {
        self.high_hz - self.low_hz
    }
}

/// Bins `low..=high` above the threshold in one frame.
#[derive(Debug, Clone, Copy)]
struct Blob {
    low: usize,
    high: usize,
    peak_bin: usize,
    peak_db: f32,
    above_db: f32,
}

/// An event that has not ended yet.
#[derive(Debug, Clone, Copy)]
struct Open {
    start: Duration,
    /// The latest frame it was in.
    seen: Duration,
    /// Every bin it has touched: what a new blob is matched against.
    low: usize,
    high: usize,
    strongest: Blob,
}

/// What turns bins into hertz; events end when it changes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    center_hz: f64,
    sample_rate: u32,
    bins: usize,
}

impl Band {
    fn bin_width(&self) -> f64 {
        f64::from(self.sample_rate) / self.bins as f64
    }

    /// The frequency at the middle of a bin.
    fn hz(&self, bin: usize) -> f64 {
        self.center_hz + (bin as f64 - self.bins as f64 / 2.0) * self.bin_width()
    }
}

/// Follows the noise floor and reports every signal above it as an
/// [`Event`] once it has ended. Works at any frame rate; the frames set the
/// resolution of the start and the duration.
pub struct Detector {
    config: DetectorConfig,
    floor: NoiseFloor,
    open: Vec<Open>,
    blobs: Vec<Blob>,
    band: Option<Band>,
    /// Time of the latest frame and the distance between the last two.
    time: Duration,
    frame_length: Duration,
}

impl Detector {
    pub fn new(config: DetectorConfig) -> Self {
        Detector {
            floor: NoiseFloor::new(config.floor),
            config,
            open: Vec::new(),
            blobs: Vec::new(),
            band: None,
            time: Duration::ZERO,
            frame_length: Duration::ZERO,
        }
    }

    /// Takes in the next frame; `emit` gets the events it ended, earliest
    /// first.
    pub fn push(&mut self, frame: &SpectrumFrame, mut emit: impl FnMut(Event)) {
        let band = Band {
            center_hz: frame.center_hz,
            sample_rate: frame.sample_rate,
            bins: frame.power_db.len(),
        };
        if self.band.is_some_and(|before| before != band) {
            // Retuned: what was on the air there cannot be followed here.
            self.finish(&mut emit);
            self.floor.reset();
        }
        let elapsed = match self.band {
            Some(_) => frame.time.saturating_sub(self.time),
            None => Duration::ZERO,
        };
        if !elapsed.is_zero() {
            self.frame_length = elapsed;
        }
        self.band = Some(band);
        self.time = frame.time;
        self.floor.update(&frame.power_db, elapsed);

        let reach = (self.config.merge_hz / band.bin_width()).round() as usize;
        self.find_blobs(&frame.power_db, reach);
        for index in 0..self.blobs.len() {
            let blob = self.blobs[index];
            self.take(blob, frame.time, reach);
        }
        let hold = self.config.hold;
        let ended = |open: &Open| frame.time.saturating_sub(open.seen) > hold;
        self.emit_where(ended, &mut emit);
    }

    /// Ends every open event: the stream is over.
    pub fn finish(&mut self, mut emit: impl FnMut(Event)) {
        self.emit_where(|_| true, &mut emit);
    }

    /// The noise floor per bin of the latest frame, in dBFS.
    pub fn floor_db(&self) -> &[f32] {
        self.floor.bins()
    }

    /// The floor of the latest frame and the threshold over it.
    pub fn levels(&self) -> Levels {
        Levels {
            floor_db: self.floor.bins().to_vec(),
            threshold_db: self.config.threshold_db,
        }
    }

    /// The events still going on, as far as they have got, earliest first.
    pub fn open(&self) -> Vec<Event> {
        let mut events: Vec<Event> = self.open.iter().map(|open| self.event(open)).collect();
        events.sort_by_key(|event| event.start);
        events
    }

    /// Fills `blobs` with the runs of bins above the threshold, runs no
    /// more than `reach` bins apart joined, without the splashes of the
    /// stronger ones.
    fn find_blobs(&mut self, power_db: &[f32], reach: usize) {
        self.blobs.clear();
        for (bin, (&db, &floor)) in power_db.iter().zip(self.floor.bins()).enumerate() {
            let above_db = db - floor;
            if above_db <= self.config.threshold_db {
                continue;
            }
            match self.blobs.last_mut() {
                Some(blob) if bin - blob.high - 1 <= reach => {
                    blob.high = bin;
                    if db > blob.peak_db {
                        blob.peak_bin = bin;
                        blob.peak_db = db;
                        blob.above_db = above_db;
                    }
                }
                _ => self.blobs.push(Blob {
                    low: bin,
                    high: bin,
                    peak_bin: bin,
                    peak_db: db,
                    above_db,
                }),
            }
        }
        let splash_db = self.config.splash_db;
        let is_splash = |blob: &Blob, blobs: &[Blob]| {
            blobs.iter().any(|strong| {
                let apart = blob.peak_bin.abs_diff(strong.peak_bin) as f32;
                apart > 0.0 && blob.peak_db < strong.peak_db - splash_db - 20.0 * apart.log10()
            })
        };
        let all = self.blobs.clone();
        self.blobs.retain(|blob| !is_splash(blob, &all));
    }

    /// Adds a blob to the open events it touches — joining them if there
    /// are several — or opens a new one.
    fn take(&mut self, blob: Blob, time: Duration, reach: usize) {
        let touches =
            |open: &Open| blob.low <= open.high + reach + 1 && open.low <= blob.high + reach + 1;
        let Some(first) = self.open.iter().position(touches) else {
            self.open.push(Open {
                start: time,
                seen: time,
                low: blob.low,
                high: blob.high,
                strongest: blob,
            });
            return;
        };
        let mut joined = self.open[first];
        let mut index = first + 1;
        while index < self.open.len() {
            if touches(&self.open[index]) {
                let other = self.open.swap_remove(index);
                joined.start = joined.start.min(other.start);
                joined.low = joined.low.min(other.low);
                joined.high = joined.high.max(other.high);
                if other.strongest.peak_db > joined.strongest.peak_db {
                    joined.strongest = other.strongest;
                }
            } else {
                index += 1;
            }
        }
        joined.seen = time;
        joined.low = joined.low.min(blob.low);
        joined.high = joined.high.max(blob.high);
        if blob.peak_db > joined.strongest.peak_db {
            joined.strongest = blob;
        }
        self.open[first] = joined;
    }

    fn emit_where(&mut self, ended: impl Fn(&Open) -> bool, emit: &mut impl FnMut(Event)) {
        let mut events: Vec<Event> = self
            .open
            .iter()
            .filter(|open| ended(open))
            .map(|open| self.event(open))
            .collect();
        if events.is_empty() {
            return;
        }
        self.open.retain(|open| !ended(open));
        events.sort_by_key(|event| event.start);
        events.into_iter().for_each(emit);
    }

    fn event(&self, open: &Open) -> Event {
        let band = self.band.expect("an open event came from a frame");
        let half_bin = band.bin_width() / 2.0;
        Event {
            start: open.start,
            duration: open.seen + self.frame_length - open.start,
            low_hz: band.hz(open.strongest.low) - half_bin,
            high_hz: band.hz(open.strongest.high) + half_bin,
            peak_hz: band.hz(open.strongest.peak_bin),
            peak_dbfs: open.strongest.peak_db,
            above_floor_db: open.strongest.above_db,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::generator::{Generator, GeneratorConfig};
    use sdr_core::{Iq, Source};
    use sdr_dsp::{Analyzer, AnalyzerConfig};

    const BINS: usize = 1000;
    /// A bin is 1 kHz wide and bin 500 is 100 MHz.
    const RATE: u32 = 1_000_000;
    const CENTER: f64 = 100e6;
    const FLOOR: f32 = -90.0;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// Frames 10 ms apart; every signal is (first frame, frames, first bin,
    /// bins, dBFS).
    struct Air {
        signals: Vec<(usize, usize, usize, usize, f32)>,
        center_hz: f64,
    }

    impl Air {
        fn new(signals: &[(usize, usize, usize, usize, f32)]) -> Self {
            Air {
                signals: signals.to_vec(),
                center_hz: CENTER,
            }
        }

        fn frame(&self, index: usize) -> SpectrumFrame {
            let mut power_db: Vec<f32> = (0..BINS)
                .map(|bin| FLOOR + ((bin * 7 + index * 3) % 5) as f32 - 2.0)
                .collect();
            for &(from, frames, bin, bins, db) in &self.signals {
                if (from..from + frames).contains(&index) {
                    power_db[bin..bin + bins].fill(db);
                }
            }
            SpectrumFrame {
                time: ms(10 * index as u64),
                center_hz: self.center_hz,
                sample_rate: RATE,
                power_db,
            }
        }
    }

    /// The events `frames` frames of `air` end, and the detector after them.
    fn watch(air: &Air, frames: usize) -> (Vec<Event>, Detector) {
        let mut detector = Detector::new(DetectorConfig::default());
        let mut events = Vec::new();
        for index in 0..frames {
            detector.push(&air.frame(index), |event| events.push(event));
        }
        (events, detector)
    }

    #[test]
    fn noise_alone_is_no_event() {
        let (events, detector) = watch(&Air::new(&[]), 200);
        assert!(events.is_empty());
        assert!(detector.open().is_empty());
        assert_eq!(detector.floor_db().len(), BINS);
        assert!((detector.floor_db()[400] - FLOOR).abs() < 0.5);
    }

    #[test]
    fn levels_are_the_floor_and_the_threshold_over_it() {
        let detector = Detector::new(DetectorConfig::default());
        assert_eq!(
            detector.levels(),
            Levels {
                floor_db: Vec::new(),
                threshold_db: 8.0
            }
        );
        let (_, detector) = watch(&Air::new(&[]), 200);
        let levels = detector.levels();
        assert_eq!(levels.floor_db, detector.floor_db());
        assert_eq!(levels.threshold_db, 8.0);
    }

    #[test]
    fn a_burst_becomes_one_event_with_its_time_place_and_level() {
        // 50 ms from 0.2 s, 3 kHz wide around 100.101 MHz.
        let (events, _) = watch(&Air::new(&[(20, 5, 600, 3, -40.0)]), 100);
        assert_eq!(events.len(), 1, "{events:?}");
        let event = &events[0];
        assert_eq!(event.start, ms(200));
        assert_eq!(event.duration, ms(50));
        assert_eq!(event.low_hz, CENTER + 99_500.0);
        assert_eq!(event.high_hz, CENTER + 102_500.0);
        assert_eq!(event.center_hz(), CENTER + 101_000.0);
        assert_eq!(event.width_hz(), 3_000.0);
        assert_eq!(event.peak_dbfs, -40.0);
        assert!((event.above_floor_db - 50.0).abs() < 0.5, "{event:?}");
    }

    #[test]
    fn an_event_is_reported_once_it_has_been_silent_for_the_hold_time() {
        let air = Air::new(&[(20, 5, 600, 3, -40.0)]);
        let mut detector = Detector::new(DetectorConfig::default());
        let mut ended_at = None;
        for index in 0..100 {
            detector.push(&air.frame(index), |_| ended_at = Some(index));
            if (20..35).contains(&index) {
                let open = detector.open();
                assert_eq!(open.len(), 1, "frame {index}");
                assert_eq!(open[0].start, ms(200));
                assert_eq!(open[0].duration, ms(10 * (index.min(24) as u64 - 19)));
            }
        }
        // Last seen in frame 24; 100 ms of hold have passed after frame 35.
        assert_eq!(ended_at, Some(35));
        assert!(detector.open().is_empty());
    }

    #[test]
    fn the_peak_is_the_strongest_bin_of_the_strongest_frame() {
        let air = Air::new(&[
            (10, 10, 300, 5, -50.0),
            (14, 1, 302, 1, -30.0),
            // Switching on splashes wide for one frame.
            (10, 1, 250, 100, -70.0),
        ]);
        let (events, _) = watch(&air, 60);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].peak_hz, CENTER - 198_000.0);
        assert_eq!(events[0].peak_dbfs, -30.0);
        assert_eq!(events[0].width_hz(), 5_000.0);
        assert_eq!(events[0].duration, ms(100));
    }

    #[test]
    fn pauses_within_the_hold_time_do_not_split_an_event() {
        // 30 ms on, 60 ms off, three times.
        let air = Air::new(&[
            (10, 3, 700, 2, -45.0),
            (19, 3, 700, 2, -45.0),
            (28, 3, 700, 2, -45.0),
        ]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].start, ms(100));
        assert_eq!(events[0].duration, ms(210));
    }

    #[test]
    fn a_longer_pause_makes_two_events() {
        let air = Air::new(&[(10, 3, 700, 2, -45.0), (30, 4, 700, 2, -45.0)]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!((events[0].start, events[0].duration), (ms(100), ms(30)));
        assert_eq!((events[1].start, events[1].duration), (ms(300), ms(40)));
    }

    #[test]
    fn signals_apart_in_frequency_are_separate_events() {
        let air = Air::new(&[(10, 5, 200, 3, -45.0), (12, 20, 800, 3, -55.0)]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].center_hz(), CENTER - 299_000.0);
        assert_eq!(events[0].duration, ms(50));
        assert_eq!(events[1].center_hz(), CENTER + 301_000.0);
        assert_eq!(events[1].duration, ms(200));
    }

    #[test]
    fn parts_closer_than_the_merge_distance_are_one_signal() {
        // Two tones of one transmitter 4 kHz apart, and another 10 kHz off.
        let air = Air::new(&[
            (10, 5, 400, 1, -45.0),
            (10, 5, 405, 1, -45.0),
            (10, 5, 416, 1, -45.0),
        ]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].width_hz(), 6_000.0);
        assert_eq!(events[1].width_hz(), 1_000.0);
    }

    #[test]
    fn the_splash_of_a_strong_signal_is_not_an_event_but_a_weak_signal_is() {
        // 100 bins from a -20 dBFS signal its splash is under -70 dBFS.
        let air = Air::new(&[
            (10, 5, 500, 1, -20.0),
            (10, 1, 600, 2, -75.0),
            (10, 5, 400, 2, -65.0),
        ]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].peak_dbfs, -65.0);
        assert_eq!(events[0].peak_hz, CENTER - 100_000.0);
        assert_eq!(events[1].peak_dbfs, -20.0);
    }

    #[test]
    fn a_signal_that_bridges_two_events_joins_them() {
        let air = Air::new(&[
            (10, 10, 400, 2, -45.0),
            (12, 10, 420, 2, -50.0),
            (15, 1, 400, 22, -60.0),
        ]);
        let (events, _) = watch(&air, 80);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].start, ms(100));
        assert_eq!(events[0].duration, ms(120));
        assert_eq!(events[0].peak_dbfs, -45.0);
    }

    #[test]
    fn a_steady_carrier_stays_open_until_the_stream_is_finished() {
        let air = Air::new(&[(0, 1000, 500, 1, -30.0)]);
        let (mut events, mut detector) = watch(&air, 300);
        assert!(events.is_empty());
        assert_eq!(detector.open().len(), 1);
        detector.finish(|event| events.push(event));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, Duration::ZERO);
        assert_eq!(events[0].duration, ms(3000));
        assert_eq!(events[0].peak_hz, CENTER);
        assert!(detector.open().is_empty());
    }

    #[test]
    fn retuning_ends_the_open_events_where_they_were() {
        let mut air = Air::new(&[(0, 1000, 600, 1, -30.0)]);
        let mut detector = Detector::new(DetectorConfig::default());
        let mut events = Vec::new();
        for index in 0..20 {
            detector.push(&air.frame(index), |event| events.push(event));
        }
        air.center_hz = 200e6;
        detector.push(&air.frame(20), |event| events.push(event));
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].peak_hz, CENTER + 100_000.0);
        assert_eq!(events[0].duration, ms(200));
        // The same bin at the new frequency is a new event.
        let open = detector.open();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].start, ms(200));
        assert_eq!(open[0].peak_hz, 200e6 + 100_000.0);
    }

    #[test]
    fn finds_the_demo_scene_s_burst_and_carrier_in_real_frames() {
        let mut generator = Generator::new(GeneratorConfig::demo());
        let info = generator.info();
        // 12 FFTs to a frame: 10.24 ms.
        let config = AnalyzerConfig::for_rate(info.sample_rate, 2048, 100.0);
        let frame_s = (config.average * config.fft_size) as f64 / f64::from(info.sample_rate);
        let mut analyzer = Analyzer::new(config);
        let mut detector = Detector::new(DetectorConfig::default());
        let mut events = Vec::new();
        let mut buf = vec![Iq::new(0.0, 0.0); 16 * 1024];
        for _ in 0..5 * info.sample_rate as usize / buf.len() {
            generator.read(&mut buf).unwrap();
            analyzer.push(&buf, &info, |frame| {
                detector.push(&frame, |event| events.push(event));
            });
        }
        // The burst: 0.3 s every 2.04 s at 434.2 MHz, -25 dBFS.
        assert_eq!(events.len(), 3, "{events:?}");
        for (event, start_s) in events.iter().zip([0.0, 2.04, 4.08]) {
            assert!((event.start.as_secs_f64() - start_s).abs() <= frame_s);
            assert!((event.duration.as_secs_f64() - 0.3).abs() <= 2.0 * frame_s);
            assert!((event.peak_hz - 434.2e6).abs() < 1_200.0, "{event:?}");
            assert!((event.peak_dbfs + 25.5).abs() < 1.0, "{event:?}");
            assert!((event.above_floor_db - 66.0).abs() < 2.0, "{event:?}");
            assert!(event.width_hz() < 20_000.0, "{event:?}");
        }
        // The carrier never ends.
        detector.finish(|event| events.push(event));
        assert_eq!(events.len(), 4, "{events:?}");
        assert!((events[3].peak_hz - 433.5e6).abs() < 1_200.0);
        assert_eq!(events[3].start, Duration::ZERO);
        assert!((events[3].duration.as_secs_f64() - 5.0).abs() < 0.05);
    }
}
