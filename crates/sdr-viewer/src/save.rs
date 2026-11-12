//! Saving an event's samples: a thread of its own writes the files, so
//! reception never waits for the disk.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use sdr_core::units::format_hz;
use sdr_store::sigmf::{self, Mark, Recording};

use crate::reader::Cut;

/// Writes every cut that comes as a SigMF recording in `dir` and tells
/// how it went: `Ok` with what was saved, `Err` with what went wrong.
/// `started` is when the stream began, for a source that receives as the
/// clock runs; without it the files carry stream time.
pub fn run(
    cuts: Receiver<Cut>,
    dir: &Path,
    started: Option<SystemTime>,
    mut tell: impl FnMut(Result<String, String>),
) {
    for cut in cuts {
        tell(save(&cut, dir, started));
    }
}

fn save(cut: &Cut, dir: &Path, started: Option<SystemTime>) -> Result<String, String> {
    let rate = f64::from(cut.source.sample_rate);
    let sample = |time: Duration| (time.as_secs_f64() * rate).round() as u64;
    let event = &cut.event;
    let first = sample(event.start).saturating_sub(cut.first_sample);
    let marks = [Mark {
        first_sample: first,
        samples: sample(event.duration).min((cut.samples.len() as u64).saturating_sub(first)),
        low_hz: event.low_hz,
        high_hz: event.high_hz,
        comment: format!(
            "peak {:.1} dBFS at {}, {:+.1} dB above the noise floor",
            event.peak_dbfs,
            format_hz(event.peak_hz),
            event.above_floor_db
        ),
    }];
    let recording = Recording {
        samples: &cut.samples,
        sample_rate: cut.source.sample_rate,
        center_hz: cut.source.center_hz,
        start: started
            .map(|started| started + Duration::from_secs_f64(cut.first_sample as f64 / rate)),
        source: &cut.source.description,
        marks: &marks,
    };
    let name = name(cut, started);
    match sigmf::write(dir, &name, &recording) {
        Ok(_) => Ok(format!(
            "IQ saved: {name} ({:.2} s)",
            cut.samples.len() as f64 / rate
        )),
        Err(err) => Err(format!("IQ not saved in {}: {err}", dir.display())),
    }
}

/// When the event began — in UTC, or in the stream — and where:
/// "20261112T120000.500Z_434.200MHz", "at-0.500s_434.200MHz".
fn name(cut: &Cut, started: Option<SystemTime>) -> String {
    let event = &cut.event;
    let when = match started {
        Some(started) => DateTime::<Utc>::from(started + event.start)
            .format("%Y%m%dT%H%M%S%.3fZ")
            .to_string(),
        None => format!("at-{:.3}s", event.start.as_secs_f64()),
    };
    format!("{when}_{:.3}MHz", event.center_hz() / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::SourceInfo;
    use sdr_detect::Event;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;
    use std::time::UNIX_EPOCH;

    /// 2026-11-12 12:00:00 UTC.
    fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_794_484_800)
    }

    /// 0.2 s of a 1 kS/s stream from 0.45 s, around an event of 0.5 to
    /// 0.6 s.
    fn cut() -> Cut {
        Cut {
            samples: (0..200).map(|i| [i as u8, 255 - i as u8]).collect(),
            first_sample: 450,
            source: SourceInfo {
                sample_rate: 1000,
                center_hz: 433.92e6,
                description: "rtl-sdr".into(),
            },
            event: Event {
                start: Duration::from_millis(500),
                duration: Duration::from_millis(100),
                low_hz: 434.19e6,
                high_hz: 434.21e6,
                peak_hz: 434.2e6,
                peak_dbfs: -25.04,
                above_floor_db: 66.3,
            },
        }
    }

    /// A directory of its own per test, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sdr-viewer-save-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn told(
        dir: &Path,
        started: Option<SystemTime>,
        cuts: Vec<Cut>,
    ) -> Vec<Result<String, String>> {
        let (tx, rx) = channel();
        for cut in cuts {
            tx.send(cut).unwrap();
        }
        drop(tx);
        let mut told = Vec::new();
        run(rx, dir, started, |result| told.push(result));
        told
    }

    #[test]
    fn a_cut_becomes_a_sigmf_pair_named_by_the_events_time_and_place() {
        let dir = TempDir::new("pair");
        let told = told(&dir.0, Some(noon()), vec![cut()]);
        let name = "20261112T120000.500Z_434.200MHz";
        assert_eq!(told, [Ok(format!("IQ saved: {name} (0.20 s)"))]);
        let data = std::fs::read(dir.0.join(format!("{name}.sigmf-data"))).unwrap();
        assert_eq!(data.len(), 400);
        assert_eq!(data[..4], [0, 255, 1, 254]);
        let meta = std::fs::read_to_string(dir.0.join(format!("{name}.sigmf-meta"))).unwrap();
        // The file begins 50 ms before the event, and says where it is.
        for part in [
            r#""core:sample_rate": 1000,"#,
            r#""core:frequency": 433920000,"#,
            r#""core:datetime": "2026-11-12T12:00:00.450Z""#,
            r#""core:description": "rtl-sdr""#,
            "\"core:sample_start\": 50,\n      \"core:sample_count\": 100,",
            r#""core:freq_lower_edge": 434190000,"#,
            r#""core:freq_upper_edge": 434210000,"#,
            r#""core:comment": "peak -25.0 dBFS at 434.200 MHz, +66.3 dB above the noise floor""#,
        ] {
            assert!(meta.contains(part), "{part} is not in {meta}");
        }
    }

    #[test]
    fn without_a_wall_clock_the_files_carry_stream_time() {
        let dir = TempDir::new("stream");
        let told = told(&dir.0, None, vec![cut()]);
        let name = "at-0.500s_434.200MHz";
        assert_eq!(told, [Ok(format!("IQ saved: {name} (0.20 s)"))]);
        let meta = std::fs::read_to_string(dir.0.join(format!("{name}.sigmf-meta"))).unwrap();
        assert!(!meta.contains("core:datetime"), "{meta}");
    }

    #[test]
    fn an_event_cut_short_is_marked_as_far_as_the_file_goes() {
        let dir = TempDir::new("short");
        // The ring had lost the event's start; it is still on at the end.
        let mut cut = cut();
        cut.first_sample = 550;
        cut.event.duration = Duration::from_secs(10);
        told(&dir.0, None, vec![cut]);
        let meta = std::fs::read_to_string(dir.0.join("at-0.500s_434.200MHz.sigmf-meta")).unwrap();
        assert!(
            meta.contains("\"core:sample_start\": 0,\n      \"core:sample_count\": 200,"),
            "{meta}"
        );
    }

    #[test]
    fn a_failure_is_told_and_the_next_cut_is_still_tried() {
        let told = told(Path::new("/dev/null/iq"), None, vec![cut(), cut()]);
        assert_eq!(told.len(), 2);
        let message = told[0].as_ref().unwrap_err();
        assert!(
            message.starts_with("IQ not saved in /dev/null/iq: "),
            "{message}"
        );
    }
}
