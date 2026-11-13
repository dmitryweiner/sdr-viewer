//! From samples to everything made of them: one pass of FFTs feeds the
//! event detector with fast frames and the screen with their averages.

use sdr_core::{Iq, SourceInfo, SpectrumFrame};
use sdr_detect::{Detector, DetectorConfig, Event, Levels};
use sdr_dsp::{Analyzer, AnalyzerConfig, FrameAverager};

/// Bins per spectrum frame: ~1.2 kHz each at 2.4 MS/s, and more than twice
/// the columns of the widest terminal we design for.
pub const FFT_SIZE: usize = 2048;

/// Spectrum frames per second for the screen: one waterfall row each, and
/// as often as a terminal is worth redrawing.
pub const FRAMES_PER_SECOND: f64 = 10.0;

/// Frames per second for the detector: a 10 ms burst fills a frame instead
/// of reading 10 dB low in a screen frame.
pub const DETECTOR_FRAMES_PER_SECOND: f64 = 100.0;

#[derive(Debug, PartialEq)]
pub enum Output {
    /// A frame for the screen, and what the detector held its last part
    /// against.
    Frame(SpectrumFrame, Levels),
    /// An event that has ended.
    Event(Event),
}

pub struct Pipeline {
    analyzer: Analyzer,
    averager: FrameAverager,
    detector: Detector,
}

impl Pipeline {
    pub fn new(sample_rate: u32) -> Self {
        let fast = AnalyzerConfig::for_rate(sample_rate, FFT_SIZE, DETECTOR_FRAMES_PER_SECOND);
        let slow = AnalyzerConfig::for_rate(sample_rate, FFT_SIZE, FRAMES_PER_SECOND);
        // A whole number of detector frames to a screen frame.
        let group = (slow.average as f64 / fast.average as f64).round().max(1.0) as usize;
        Pipeline {
            analyzer: Analyzer::new(fast),
            averager: FrameAverager::new(group),
            detector: Detector::new(DetectorConfig::default()),
        }
    }

    pub fn push(&mut self, samples: &[Iq], info: &SourceInfo, mut out: impl FnMut(Output)) {
        let Pipeline {
            analyzer,
            averager,
            detector,
        } = self;
        analyzer.push(samples, info, |frame| {
            detector.push(&frame, |event| out(Output::Event(event)));
            if let Some(frame) = averager.push(&frame) {
                out(Output::Frame(frame, detector.levels()));
            }
        });
    }

    /// Ends the events still going on and returns them: reading is over.
    pub fn finish(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        self.detector.finish(|event| events.push(event));
        events
    }

    /// The events still going on, as far as they have got.
    pub fn open_events(&self) -> Vec<Event> {
        self.detector.open()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::Source;
    use sdr_core::generator::{Generator, GeneratorConfig};
    use std::time::Duration;

    #[test]
    fn the_demo_scene_gives_screen_frames_ended_events_and_an_open_one() {
        let mut generator = Generator::new(GeneratorConfig::demo());
        let info = generator.info();
        let mut pipeline = Pipeline::new(info.sample_rate);
        let mut frames = Vec::new();
        let mut floors = Vec::new();
        let mut events = Vec::new();
        let mut buf = vec![Iq::new(0.0, 0.0); 2_400_000];
        generator.read(&mut buf).unwrap();
        pipeline.push(&buf, &info, |output| match output {
            Output::Frame(frame, levels) => {
                frames.push(frame);
                floors.push(levels);
            }
            Output::Event(event) => events.push(event),
        });
        // Every screen frame comes with the detector's floor under it:
        // where the band is quiet, the frame's own level.
        assert_eq!(floors.len(), frames.len());
        for (frame, levels) in frames.iter().zip(&floors) {
            assert_eq!(levels.floor_db.len(), FFT_SIZE);
            assert_eq!(levels.threshold_db, 8.0);
            let quiet = &frame.power_db[80..120];
            let mean = quiet.iter().sum::<f32>() / quiet.len() as f32;
            assert!((levels.floor_db[100] - mean).abs() < 1.0, "{mean}");
        }
        // 12 FFTs to a detector frame, 10 of those to a screen frame:
        // 102.4 ms.
        assert_eq!(frames.len(), 9);
        assert_eq!(frames[1].time, Duration::from_secs_f64(0.1024));
        assert_eq!(frames[0].power_db.len(), FFT_SIZE);
        // The burst of the first 0.3 s has ended, the carrier goes on.
        assert_eq!(events.len(), 1, "{events:?}");
        assert!((events[0].peak_hz - 434.2e6).abs() < 1_200.0);
        let open = pipeline.open_events();
        assert_eq!(open.len(), 1, "{open:?}");
        assert!((open[0].peak_hz - 433.5e6).abs() < 1_200.0);
        assert_eq!(pipeline.finish(), open);
        assert!(pipeline.open_events().is_empty());
    }

    #[test]
    fn a_slow_source_still_gets_about_ten_screen_frames_a_second() {
        // 49 FFTs a second: each is a detector frame, 5 make a screen frame.
        let info = SourceInfo {
            sample_rate: 100_000,
            center_hz: 1e6,
            description: "test".into(),
        };
        let mut pipeline = Pipeline::new(info.sample_rate);
        let mut frames = 0;
        let silence = vec![Iq::new(0.0, 0.0); 300_000];
        pipeline.push(&silence, &info, |output| {
            if matches!(output, Output::Frame(..)) {
                frames += 1;
            }
        });
        assert_eq!(frames, 300_000 / (FFT_SIZE * 5));
    }
}
