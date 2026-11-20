//! Recording on an alert, on the worker's side: the samples the reader
//! hands over, narrowed to the event's band and written as they come —
//! `ci16_le` SigMF in a directory of their own — until the alerts'
//! recordings together reach their limit. Nothing is ever deleted.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sdr_core::units::{format_hz, format_rate};
use sdr_core::{Iq, SourceInfo};
use sdr_detect::Alert;
use sdr_dsp::Downconverter;
use sdr_store::sigmf::{self, Mark, Meta};

use crate::save::name_of;

/// The extension of the samples' file.
const DATA: &str = "sigmf-data";

/// What came of a recording, for the interface.
#[derive(Debug, Clone, PartialEq)]
pub enum Told {
    Done(String),
    Notice(String),
    /// The recordings have reached their limit: no more are made.
    Full,
}

/// A recording being written.
struct Writing {
    id: u64,
    name: String,
    file: BufWriter<File>,
    ddc: Downconverter,
    /// The stream's number of its first input sample.
    first: u64,
    source: SourceInfo,
    center_hz: f64,
    width_hz: f64,
    marks: Vec<Mark>,
    /// Output samples written.
    written: u64,
    /// Scratch for the samples as numbers, and the narrowed ones.
    input: Vec<Iq>,
    output: Vec<Iq>,
}

/// The recordings being written into `dir`, and how much all of them in
/// it take.
pub struct Recordings {
    dir: PathBuf,
    started: Option<SystemTime>,
    limit: u64,
    used: u64,
    full: bool,
    writing: Vec<Writing>,
}

/// What the `.sigmf-data` files in `dir` take; nothing when there is no
/// such directory.
fn used_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == DATA))
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

impl Recordings {
    /// Recordings into `dir`, which with those there already may take
    /// `limit` bytes. `started` is when the stream began, for a source
    /// that receives as the clock runs.
    pub fn new(dir: PathBuf, started: Option<SystemTime>, limit: u64) -> Self {
        let used = used_in(&dir);
        Recordings {
            dir,
            started,
            limit,
            used,
            full: used >= limit,
            writing: Vec::new(),
        }
    }

    /// Begins recording `id`: the stream from sample `first` on, of a
    /// band `width_hz` wide around `center_hz`, for `alert`.
    pub fn start(
        &mut self,
        (id, first): (u64, u64),
        (center_hz, width_hz): (f64, f64),
        alert: &Alert,
        source: &SourceInfo,
    ) -> Option<Told> {
        if self.full {
            return None;
        }
        let rate = f64::from(source.sample_rate);
        let first_time = Duration::from_secs_f64(first as f64 / rate);
        let base = name_of(first_time, center_hz, self.started);
        let opened = std::fs::create_dir_all(&self.dir).and_then(|()| self.create(&base));
        let (name, file) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                return Some(Told::Notice(format!(
                    "alert not recorded in {}: {err}",
                    self.dir.display()
                )));
            }
        };
        let offset = center_hz - source.center_hz;
        let mut writing = Writing {
            id,
            name,
            file: BufWriter::new(file),
            ddc: Downconverter::new(source.sample_rate, offset, width_hz),
            first,
            source: source.clone(),
            center_hz,
            width_hz,
            marks: Vec::new(),
            written: 0,
            input: Vec::new(),
            output: Vec::new(),
        };
        writing.mark(alert);
        // A meta file from the start: what was written is readable even
        // if the app never gets to the end.
        if let Err(err) = self.write_meta(&writing) {
            return Some(Told::Notice(format!("alert not recorded: {err}")));
        }
        self.writing.push(writing);
        None
    }

    /// A file of its own: `base`, or `base-2`… when that is taken.
    fn create(&self, base: &str) -> std::io::Result<(String, File)> {
        let mut turn = 1;
        loop {
            let name = match turn {
                1 => base.to_string(),
                turn => format!("{base}-{turn}"),
            };
            let path = self.dir.join(format!("{name}.{DATA}"));
            match File::create_new(&path) {
                Ok(file) => return Ok((name, file)),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && turn < 100 => {
                    turn += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Another alert in recording `id`'s band.
    pub fn mark(&mut self, id: u64, alert: &Alert) {
        if let Some(writing) = self.writing.iter_mut().find(|writing| writing.id == id) {
            writing.mark(alert);
        }
    }

    /// The next samples of recording `id`, 8 bits a part.
    pub fn samples(&mut self, id: u64, samples: &[[u8; 2]]) -> Option<Told> {
        let index = self.writing.iter().position(|writing| writing.id == id)?;
        let writing = &mut self.writing[index];
        writing.input.resize(samples.len(), Iq::new(0.0, 0.0));
        sdr_core::file::SampleFormat::Cu8.decode(samples.as_flattened(), &mut writing.input);
        writing.output.clear();
        writing.ddc.push(&writing.input, &mut writing.output);
        let bytes: Vec<u8> = (writing.output.iter())
            .flat_map(|sample| [sample.re, sample.im])
            .flat_map(|part| {
                ((part * 32767.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()
            })
            .collect();
        if self.used + bytes.len() as u64 > self.limit {
            self.full = true;
            let told = self.end(id);
            return Some(match told {
                Some(Told::Notice(why)) => Told::Notice(why),
                _ => Told::Full,
            });
        }
        let writing = &mut self.writing[index];
        if let Err(err) = writing.file.write_all(&bytes) {
            let name = writing.name.clone();
            self.writing.remove(index);
            return Some(Told::Notice(format!(
                "alert recording {name} cut short: {err}"
            )));
        }
        writing.written += writing.output.len() as u64;
        self.used += bytes.len() as u64;
        None
    }

    /// Ends recording `id`: its samples flushed, its meta file written
    /// with every alert it had.
    pub fn end(&mut self, id: u64) -> Option<Told> {
        let index = self.writing.iter().position(|writing| writing.id == id)?;
        let mut writing = self.writing.remove(index);
        let flushed = writing.file.flush();
        if let Err(err) = flushed.and_then(|()| self.write_meta(&writing)) {
            return Some(Told::Notice(format!(
                "alert recording {} not finished: {err}",
                writing.name
            )));
        }
        let rate = writing.ddc.output_rate();
        Some(Told::Done(format!(
            "alert recorded: {} ({:.1} s at {})",
            writing.name,
            writing.written as f64 / rate,
            format_rate(rate)
        )))
    }

    fn write_meta(&self, writing: &Writing) -> std::io::Result<()> {
        let input_rate = f64::from(writing.source.sample_rate);
        let start = (self.started)
            .map(|started| started + Duration::from_secs_f64(writing.first as f64 / input_rate));
        let source = format!(
            "{}; {} around {}, decimated by {}",
            writing.source.description,
            format_hz(writing.width_hz),
            format_hz(writing.center_hz),
            writing.ddc.decimation()
        );
        let meta = Meta {
            datatype: "ci16_le",
            sample_rate: writing.ddc.output_rate(),
            center_hz: writing.center_hz,
            start,
            source: &source,
            marks: &writing.marks,
        };
        sigmf::write_meta(&self.dir, &writing.name, &meta)
    }
}

impl Writing {
    /// Marks `alert`'s event, as it was when its rule was met.
    fn mark(&mut self, alert: &Alert) {
        let input_rate = f64::from(self.source.sample_rate);
        let decimation = self.ddc.decimation() as f64;
        let sample = |time: Duration| (time.as_secs_f64() * input_rate / decimation).round() as u64;
        let event = &alert.event;
        let first = (event.start.as_secs_f64() * input_rate).round() as u64;
        self.marks.push(Mark {
            first_sample: (first.saturating_sub(self.first) as f64 / decimation).round() as u64,
            samples: sample(event.duration),
            low_hz: event.low_hz,
            high_hz: event.high_hz,
            comment: format!(
                "alert {}: peak {:.1} dBFS, {:+.1} dB above the noise floor, when the rule was met",
                alert.rule, event.peak_dbfs, event.above_floor_db
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_detect::Event;
    use std::time::UNIX_EPOCH;

    /// A directory of its own per test, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sdr-viewer-recordings-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn source() -> SourceInfo {
        SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "rtl-sdr".into(),
        }
    }

    /// An alert on a 10 kHz event at 434.2 MHz from 1 s.
    fn alert() -> Alert {
        Alert {
            rule: "gate".into(),
            record: true,
            number: 1,
            event: Event {
                start: Duration::from_secs(1),
                duration: Duration::from_millis(100),
                low_hz: 434.195e6,
                high_hz: 434.205e6,
                peak_hz: 434.2e6,
                peak_dbfs: -30.0,
                above_floor_db: 40.0,
            },
        }
    }

    /// `count` samples of a tone 1 kHz above 434.2 MHz, 8 bits a part.
    fn tone(count: usize) -> Vec<[u8; 2]> {
        (0..count)
            .map(|index| {
                let phase = std::f64::consts::TAU * 281e3 * index as f64 / 2.4e6;
                let part = |value: f64| (127.5 + 100.0 * value).round() as u8;
                [part(phase.cos()), part(phase.sin())]
            })
            .collect()
    }

    /// 2026-11-12 12:00:00 UTC.
    fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_794_484_800)
    }

    #[test]
    fn a_recording_is_the_events_band_narrowed_with_its_alert_marked() {
        let dir = TempDir::new("one");
        let mut recordings = Recordings::new(dir.0.clone(), Some(noon()), 1_000_000);
        // From half a second before the event.
        let first = 1_200_000;
        let started = recordings.start((0, first), (434.2e6, 10e3), &alert(), &source());
        assert_eq!(started, None);
        for _ in 0..10 {
            assert_eq!(recordings.samples(0, &tone(24_000)), None);
        }
        let told = recordings.end(0);
        let name = "20261112T120000.500Z_434.200MHz";
        assert_eq!(
            told,
            Some(Told::Done(format!(
                "alert recorded: {name} (0.1 s at 50.000 kS/s)"
            )))
        );
        // 240 000 samples in, 5 000 out, 4 bytes each.
        let data = std::fs::read(dir.0.join(format!("{name}.sigmf-data"))).unwrap();
        assert_eq!(data.len(), 20_000);
        let meta = std::fs::read_to_string(dir.0.join(format!("{name}.sigmf-meta"))).unwrap();
        for part in [
            r#""core:datatype": "ci16_le","#,
            r#""core:sample_rate": 50000,"#,
            r#""core:frequency": 434200000,"#,
            r#""core:datetime": "2026-11-12T12:00:00.500Z""#,
            r#""core:description": "rtl-sdr; 10.000 kHz around 434.200 MHz, decimated by 48""#,
            "\"core:sample_start\": 25000,\n      \"core:sample_count\": 5000,",
            r#""core:comment": "alert gate: peak -30.0 dBFS, +40.0 dB above the noise floor, when the rule was met""#,
        ] {
            assert!(meta.contains(part), "{part} is not in {meta}");
        }
    }

    #[test]
    fn the_tone_comes_out_a_kilohertz_from_zero() {
        let dir = TempDir::new("tone");
        let mut recordings = Recordings::new(dir.0.clone(), None, 1_000_000);
        recordings.start((0, 0), (434.2e6, 10e3), &alert(), &source());
        recordings.samples(0, &tone(240_000));
        recordings.end(0);
        let data = std::fs::read(dir.0.join("at-0.000s_434.200MHz.sigmf-data")).unwrap();
        let samples: Vec<Iq> = data
            .chunks(4)
            .map(|four| {
                let part = |at: usize| f32::from(i16::from_le_bytes([four[at], four[at + 1]]));
                Iq::new(part(0), part(2)) / 32767.0
            })
            .collect();
        let settled = &samples[1_000..];
        let level = settled.iter().map(|s| s.norm()).sum::<f32>() / settled.len() as f32;
        assert!((level - 100.0 / 127.5).abs() < 0.02, "{level}");
        let turn: f32 = settled
            .windows(2)
            .map(|pair| (pair[1] * pair[0].conj()).arg())
            .sum::<f32>()
            / (settled.len() - 1) as f32;
        let hz = turn / std::f32::consts::TAU * 50_000.0;
        assert!((hz - 1_000.0).abs() < 2.0, "{hz}");
    }

    #[test]
    fn at_the_limit_the_recording_stops_and_no_new_one_is_made() {
        let dir = TempDir::new("limit");
        // What is there counts: 15 000 bytes of an earlier run.
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("earlier.sigmf-data"), vec![0u8; 15_000]).unwrap();
        std::fs::write(dir.0.join("earlier.sigmf-meta"), vec![0u8; 50_000]).unwrap();
        let mut recordings = Recordings::new(dir.0.clone(), None, 30_000);
        recordings.start((0, 0), (434.2e6, 10e3), &alert(), &source());
        // 2 000 bytes a 24 000 samples: the seventh would pass 15 000 more.
        let told: Vec<Option<Told>> = (0..8)
            .map(|_| recordings.samples(0, &tone(24_000)))
            .collect();
        assert_eq!(told[..7], [None, None, None, None, None, None, None]);
        assert_eq!(told[7], Some(Told::Full));
        let data = std::fs::read(dir.0.join("at-0.000s_434.200MHz.sigmf-data")).unwrap();
        assert_eq!(data.len(), 14_000);
        assert!(dir.0.join("at-0.000s_434.200MHz.sigmf-meta").exists());
        // Nothing more is made, and nothing is deleted.
        assert_eq!(recordings.samples(0, &tone(24_000)), None);
        assert_eq!(
            recordings.start((1, 0), (434.2e6, 10e3), &alert(), &source()),
            None
        );
        assert_eq!(recordings.end(1), None);
        assert!(dir.0.join("earlier.sigmf-data").exists());
        // A new run with the directory as full as that makes none either.
        let mut again = Recordings::new(dir.0.clone(), None, 29_000);
        again.start((0, 0), (434.2e6, 10e3), &alert(), &source());
        assert_eq!(again.samples(0, &tone(24_000)), None);
        assert_eq!(again.end(0), None);
    }

    #[test]
    fn two_recordings_of_one_moment_and_place_get_names_of_their_own() {
        let dir = TempDir::new("twice");
        let mut recordings = Recordings::new(dir.0.clone(), None, 1_000_000);
        recordings.start((0, 0), (434.2e6, 10e3), &alert(), &source());
        recordings.start((1, 0), (434.2e6, 10e3), &alert(), &source());
        recordings.end(0);
        match recordings.end(1) {
            Some(Told::Done(done)) => assert!(done.contains("434.200MHz-2 "), "{done}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_directory_that_cannot_be_made_is_told() {
        let mut recordings = Recordings::new("/dev/null/alerts".into(), None, 1_000_000);
        match recordings.start((0, 0), (434.2e6, 10e3), &alert(), &source()) {
            Some(Told::Notice(why)) => {
                assert!(
                    why.starts_with("alert not recorded in /dev/null/alerts: "),
                    "{why}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(recordings.samples(0, &tone(10)), None);
    }
}
