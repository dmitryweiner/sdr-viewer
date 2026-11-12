//! The thread that pulls samples from the source, so the interface never
//! waits for the radio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

use sdr_core::rate::RateMeter;
use sdr_core::{Iq, Source, SourceInfo, SpectrumFrame};
use sdr_tui::{Command, SourceState, SourceStatus};

use crate::pipeline::{Output, Pipeline};

/// Samples per read: ~7 ms at 2.4 MS/s.
const CHUNK: usize = 16 * 1024;

/// How often the measured rate is reported.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// What the reader tells the interface.
#[derive(Debug, PartialEq)]
pub enum Update {
    Status(SourceStatus),
    Frame(SpectrumFrame),
    /// The source after a command changed it.
    Source(SourceInfo),
    /// A command the source refused, in its words.
    Notice(String),
}

/// Reads until the source ends or fails, or `stop` is set, sending spectrum
/// frames and the source's status to `updates`, and carrying out `commands`
/// between reads. Frames and status the interface has no room for are
/// dropped: reception never waits for drawing.
pub fn run(
    mut source: impl Source,
    updates: SyncSender<Update>,
    commands: Receiver<Command>,
    stop: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let mut meter = RateMeter::new(REPORT_EVERY);
    let mut buf = vec![Iq::new(0.0, 0.0); CHUNK];
    let mut pipeline = Pipeline::new(source.info().sample_rate);
    let report = |state, rate| {
        let _ = updates.try_send(Update::Status(SourceStatus { rate, state }));
    };
    while !stop.load(Ordering::Relaxed) {
        for command in commands.try_iter() {
            let done = match command {
                Command::Tune(center_hz) => source.tune(center_hz),
                Command::Gain(change) => source.set_gain(change),
            };
            // The answer to a key press must not get lost: wait for room.
            let _ = updates.send(match done {
                Ok(()) => Update::Source(source.info()),
                Err(err) => Update::Notice(err.to_string()),
            });
        }
        match source.read(&mut buf) {
            Ok(0) => {
                report(SourceState::Ended, 0.0);
                return;
            }
            Ok(n) => {
                pipeline.push(&buf[..n], &source.info(), |output| match output {
                    Output::Frame(frame) => {
                        let _ = updates.try_send(Update::Frame(frame));
                    }
                    // The detector already runs; the signal log that shows
                    // its events is the next step of phase 3.
                    Output::Event(_) => {}
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
    use crate::pipeline::FFT_SIZE;
    use sdr_core::GainChange;
    use sdr_core::file::{FileSource, SampleFormat};
    use sdr_core::generator::{Generator, GeneratorConfig};
    use sdr_core::{SourceError, SourceInfo};
    use std::io::Cursor;
    use std::sync::mpsc::{channel, sync_channel};

    fn info() -> SourceInfo {
        SourceInfo {
            sample_rate: 1000,
            center_hz: 1e6,
            description: "test".into(),
        }
    }

    fn updates(source: impl Source) -> Vec<Update> {
        updates_after(source, &[])
    }

    /// Everything the reader sends when `commands` are waiting at its start.
    fn updates_after(source: impl Source, commands: &[Command]) -> Vec<Update> {
        let (tx, rx) = sync_channel(64);
        let (command_tx, command_rx) = channel();
        for command in commands {
            command_tx.send(*command).unwrap();
        }
        run(source, tx, command_rx, Arc::new(AtomicBool::new(false)));
        rx.try_iter().collect()
    }

    /// A generator that ends after a number of reads.
    struct Brief(Generator, usize);

    impl Source for Brief {
        fn info(&self) -> SourceInfo {
            self.0.info()
        }
        fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
            if self.1 == 0 {
                return Ok(0);
            }
            self.1 -= 1;
            self.0.read(buf)
        }
        fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
            self.0.tune(center_hz)
        }
    }

    #[test]
    fn a_tune_command_moves_the_source_and_the_frames_that_follow() {
        let source = Brief(Generator::new(GeneratorConfig::demo()), 40);
        let got = updates_after(source, &[Command::Tune(100e6)]);
        match &got[0] {
            Update::Source(info) => assert_eq!(info.center_hz, 100e6),
            other => panic!("{other:?}"),
        }
        let centers: Vec<f64> = got
            .iter()
            .filter_map(|update| match update {
                Update::Frame(frame) => Some(frame.center_hz),
                _ => None,
            })
            .collect();
        assert!(!centers.is_empty());
        assert!(centers.iter().all(|&center| center == 100e6));
    }

    #[test]
    fn a_refused_command_comes_back_as_a_notice_and_reading_goes_on() {
        let source = FileSource::new(
            Cursor::new(vec![0u8; 100]),
            SampleFormat::Cu8,
            info(),
            false,
        );
        let commands = [Command::Tune(100e6), Command::Gain(GainChange::Step(1))];
        let got = updates_after(source, &commands);
        assert_eq!(
            got,
            [
                Update::Notice("not supported: a recording cannot be retuned".into()),
                Update::Notice("not supported: this source has no gain".into()),
                Update::Status(SourceStatus {
                    rate: 0.0,
                    state: SourceState::Ended
                }),
            ]
        );
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
                _ => None,
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
        let (_command_tx, command_rx) = channel();
        let source = FileSource::new(Cursor::new(vec![0u8; 8]), SampleFormat::Cu8, info(), true);
        run(source, tx, command_rx, Arc::new(AtomicBool::new(true)));
        assert!(rx.try_recv().is_err());
    }
}
