//! The thread that pulls samples from the source, so the interface never
//! waits for the radio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use sdr_core::rate::RateMeter;
use sdr_core::{Iq, Source, SpectrumFrame};
use sdr_dsp::{Analyzer, AnalyzerConfig};
use sdr_tui::{SourceState, SourceStatus};

/// Samples per read: ~7 ms at 2.4 MS/s.
const CHUNK: usize = 16 * 1024;

/// How often the measured rate is reported.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// Bins per spectrum frame: ~1.2 kHz each at 2.4 MS/s, and more than twice
/// the columns of the widest terminal we design for.
pub const FFT_SIZE: usize = 2048;

/// Spectrum frames per second: one waterfall row each, and as often as a
/// terminal is worth redrawing.
pub const FRAMES_PER_SECOND: f64 = 10.0;

/// What the reader tells the interface.
#[derive(Debug, PartialEq)]
pub enum Update {
    Status(SourceStatus),
    Frame(SpectrumFrame),
}

/// Reads until the source ends or fails, or `stop` is set, sending spectrum
/// frames and the source's status to `updates`. What the interface has no
/// room for is dropped: reception never waits for drawing.
pub fn run(mut source: impl Source, updates: SyncSender<Update>, stop: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut meter = RateMeter::new(REPORT_EVERY);
    let mut buf = vec![Iq::new(0.0, 0.0); CHUNK];
    let config = AnalyzerConfig::for_rate(source.info().sample_rate, FFT_SIZE, FRAMES_PER_SECOND);
    let mut analyzer = Analyzer::new(config);
    let report = |state, rate| {
        let _ = updates.try_send(Update::Status(SourceStatus { rate, state }));
    };
    while !stop.load(Ordering::Relaxed) {
        match source.read(&mut buf) {
            Ok(0) => {
                report(SourceState::Ended, 0.0);
                return;
            }
            Ok(n) => {
                analyzer.push(&buf[..n], &source.info(), |frame| {
                    let _ = updates.try_send(Update::Frame(frame));
                });
                if meter.record(started.elapsed(), n as u64) {
                    report(SourceState::Running, meter.rate());
                }
            }
            Err(err) => {
                report(SourceState::Failed(err.to_string()), 0.0);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::file::{FileSource, SampleFormat};
    use sdr_core::{SourceError, SourceInfo};
    use std::io::Cursor;
    use std::sync::mpsc::sync_channel;

    fn info() -> SourceInfo {
        SourceInfo {
            sample_rate: 1000,
            center_hz: 1e6,
            description: "test".into(),
        }
    }

    fn updates(source: impl Source) -> Vec<Update> {
        let (tx, rx) = sync_channel(64);
        run(source, tx, Arc::new(AtomicBool::new(false)));
        rx.try_iter().collect()
    }

    fn last_status(source: impl Source) -> SourceStatus {
        match updates(source).pop() {
            Some(Update::Status(status)) => status,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sends_spectrum_frames_at_about_ten_a_second() {
        // 3 s of silence at 100 kS/s: 49 FFTs a second, 5 to a frame.
        let info = SourceInfo {
            sample_rate: 100_000,
            ..info()
        };
        let bytes = vec![127u8; 600_000];
        let source = FileSource::new(Cursor::new(bytes), SampleFormat::Cu8, info, false);
        let frames: Vec<SpectrumFrame> = updates(source)
            .into_iter()
            .filter_map(|update| match update {
                Update::Frame(frame) => Some(frame),
                Update::Status(_) => None,
            })
            .collect();
        assert_eq!(frames.len(), 300_000 / (FFT_SIZE * 5));
        assert_eq!(frames[0].power_db.len(), FFT_SIZE);
        assert_eq!(frames[0].center_hz, 1e6);
        assert_eq!(frames[1].time, Duration::from_secs_f64(0.1024));
    }

    #[test]
    fn reports_the_end_of_a_recording() {
        let source = FileSource::new(
            Cursor::new(vec![0u8; 100]),
            SampleFormat::Cu8,
            info(),
            false,
        );
        assert_eq!(last_status(source).state, SourceState::Ended);
    }

    struct Broken;

    impl Source for Broken {
        fn info(&self) -> SourceInfo {
            info()
        }
        fn read(&mut self, _: &mut [Iq]) -> Result<usize, SourceError> {
            Err(SourceError::Unsupported("reading"))
        }
        fn tune(&mut self, _: f64) -> Result<(), SourceError> {
            Ok(())
        }
    }

    #[test]
    fn reports_a_failure_with_its_message() {
        assert_eq!(
            last_status(Broken).state,
            SourceState::Failed("not supported: reading".into())
        );
    }

    #[test]
    fn stops_when_asked() {
        let (tx, rx) = sync_channel(4);
        let source = FileSource::new(Cursor::new(vec![0u8; 8]), SampleFormat::Cu8, info(), true);
        run(source, tx, Arc::new(AtomicBool::new(true)));
        assert!(rx.try_recv().is_err());
    }
}
