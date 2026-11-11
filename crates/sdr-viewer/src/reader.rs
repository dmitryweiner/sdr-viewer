//! The thread that pulls samples from the source, so the interface never
//! waits for the radio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use sdr_core::rate::RateMeter;
use sdr_core::{Iq, Source};
use sdr_tui::{SourceState, SourceStatus};

/// Samples per read: ~7 ms at 2.4 MS/s.
const CHUNK: usize = 16 * 1024;

/// How often the measured rate is reported.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// Reads until the source ends or fails, or `stop` is set, reporting the
/// status to `status`. A report the interface has no room for is dropped:
/// the next one replaces it anyway.
pub fn run(mut source: impl Source, status: SyncSender<SourceStatus>, stop: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut meter = RateMeter::new(REPORT_EVERY);
    let mut buf = vec![Iq::new(0.0, 0.0); CHUNK];
    let report = |state, rate| {
        let _ = status.try_send(SourceStatus { rate, state });
    };
    while !stop.load(Ordering::Relaxed) {
        match source.read(&mut buf) {
            Ok(0) => {
                report(SourceState::Ended, 0.0);
                return;
            }
            Ok(n) => {
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

    fn last_status(source: impl Source) -> SourceStatus {
        let (tx, rx) = sync_channel(4);
        run(source, tx, Arc::new(AtomicBool::new(false)));
        rx.try_iter().last().unwrap()
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
