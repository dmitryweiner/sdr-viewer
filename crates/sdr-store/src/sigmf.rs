//! Recordings in SigMF: the samples in `NAME.sigmf-data`, what they are in
//! `NAME.sigmf-meta` beside it — a pair other SDR tools open.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

/// A stretch of one band, 8 bits a part (`cu8`).
#[derive(Debug, Clone, PartialEq)]
pub struct Recording<'a> {
    pub samples: &'a [[u8; 2]],
    pub sample_rate: u32,
    pub center_hz: f64,
    /// When the first sample was received; `None` where the wall clock
    /// says nothing about the samples (a recording played back).
    pub start: Option<SystemTime>,
    /// What received it, in a few words.
    pub source: &'a str,
    pub marks: &'a [Mark],
}

/// A signal inside a recording: SigMF's annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// Counted from the recording's first sample.
    pub first_sample: u64,
    pub samples: u64,
    pub low_hz: f64,
    pub high_hz: f64,
    pub comment: String,
}

/// What the samples of a recording are, whatever writes them.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta<'a> {
    /// SigMF's name for the samples' type: `cu8`, `ci16_le`.
    pub datatype: &'a str,
    pub sample_rate: f64,
    pub center_hz: f64,
    /// When the first sample was received; `None` where the wall clock
    /// says nothing about the samples.
    pub start: Option<SystemTime>,
    /// What received them and how they were made, in a few words.
    pub source: &'a str,
    pub marks: &'a [Mark],
}

/// Writes `NAME.sigmf-data` and `NAME.sigmf-meta` into `dir`, making the
/// directory if need be, and returns the path of the data file.
pub fn write(dir: &Path, name: &str, recording: &Recording) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let data = dir.join(format!("{name}.sigmf-data"));
    std::fs::write(&data, recording.samples.as_flattened())?;
    std::fs::write(dir.join(format!("{name}.sigmf-meta")), meta(recording))?;
    Ok(data)
}

/// Writes `NAME.sigmf-meta` into `dir`: for samples written by another
/// hand, as they come, into `NAME.sigmf-data` beside it.
pub fn write_meta(dir: &Path, name: &str, meta: &Meta) -> io::Result<()> {
    std::fs::write(dir.join(format!("{name}.sigmf-meta")), meta_text(meta))
}

/// The `.sigmf-meta` file of `recording`.
pub fn meta(recording: &Recording) -> String {
    meta_text(&Meta {
        datatype: "cu8",
        sample_rate: f64::from(recording.sample_rate),
        center_hz: recording.center_hz,
        start: recording.start,
        source: recording.source,
        marks: recording.marks,
    })
}

/// The `.sigmf-meta` file of samples that `meta` tells.
pub fn meta_text(recording: &Meta) -> String {
    let mut capture = format!(
        "      \"core:sample_start\": 0,\n      \"core:frequency\": {}",
        recording.center_hz
    );
    if let Some(start) = recording.start {
        let _ = write!(
            capture,
            ",\n      \"core:datetime\": \"{}\"",
            datetime(start)
        );
    }
    let marks: Vec<String> = recording
        .marks
        .iter()
        .map(|mark| {
            format!(
                "    {{\n      \"core:sample_start\": {},\n      \"core:sample_count\": {},\n      \
                 \"core:freq_lower_edge\": {},\n      \"core:freq_upper_edge\": {},\n      \
                 \"core:comment\": {}\n    }}",
                mark.first_sample,
                mark.samples,
                mark.low_hz,
                mark.high_hz,
                json_string(&mark.comment)
            )
        })
        .collect();
    let annotations = if marks.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{}\n  ]", marks.join(",\n"))
    };
    format!(
        "{{\n  \"global\": {{\n    \"core:datatype\": {},\n    \"core:sample_rate\": {},\n    \
         \"core:version\": \"1.0.0\",\n    \"core:recorder\": \"sdr-viewer {}\",\n    \
         \"core:description\": {}\n  }},\n  \"captures\": [\n    {{\n{capture}\n    }}\n  ],\n  \
         \"annotations\": {annotations}\n}}\n",
        json_string(recording.datatype),
        recording.sample_rate,
        env!("CARGO_PKG_VERSION"),
        json_string(recording.source),
    )
}

/// The moment as SigMF wants it: UTC, ISO 8601, to the millisecond.
pub fn datetime(at: SystemTime) -> String {
    DateTime::<Utc>::from(at)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// 2026-11-12 12:00:00.250 UTC.
    fn start() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_794_484_800_250)
    }

    fn recording<'a>(samples: &'a [[u8; 2]], marks: &'a [Mark]) -> Recording<'a> {
        Recording {
            samples,
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            start: Some(start()),
            source: "rtl-sdr",
            marks,
        }
    }

    #[test]
    fn the_meta_file_says_what_the_samples_are_and_marks_the_signal() {
        let marks = [Mark {
            first_sample: 120_000,
            samples: 98_304,
            low_hz: 433.42e6,
            high_hz: 433_430_000.5,
            comment: "peak -57.5 dBFS".into(),
        }];
        assert_eq!(
            meta(&recording(&[], &marks)),
            r#"{
  "global": {
    "core:datatype": "cu8",
    "core:sample_rate": 2400000,
    "core:version": "1.0.0",
    "core:recorder": "sdr-viewer 0.1.0",
    "core:description": "rtl-sdr"
  },
  "captures": [
    {
      "core:sample_start": 0,
      "core:frequency": 433920000,
      "core:datetime": "2026-11-12T12:00:00.250Z"
    }
  ],
  "annotations": [
    {
      "core:sample_start": 120000,
      "core:sample_count": 98304,
      "core:freq_lower_edge": 433420000,
      "core:freq_upper_edge": 433430000.5,
      "core:comment": "peak -57.5 dBFS"
    }
  ]
}
"#
        );
    }

    #[test]
    fn without_a_wall_clock_or_marks_the_meta_file_has_neither() {
        let recording = Recording {
            start: None,
            source: "file \"a\\b\"\n.cu8",
            ..recording(&[], &[])
        };
        let meta = meta(&recording);
        assert!(!meta.contains("core:datetime"), "{meta}");
        assert!(meta.contains("\"annotations\": []\n"), "{meta}");
        assert!(
            meta.contains(r#""core:description": "file \"a\\b\"\u000a.cu8""#),
            "{meta}"
        );
    }

    #[test]
    fn writes_the_samples_and_the_meta_file_side_by_side() {
        let dir = std::env::temp_dir().join(format!("sdr-sigmf-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let samples = [[0, 255], [127, 128], [3, 4]];
        let recording = recording(&samples, &[]);
        // The directory is made on the way.
        let data = write(&dir.join("iq"), "burst", &recording).unwrap();
        assert_eq!(data, dir.join("iq/burst.sigmf-data"));
        assert_eq!(std::fs::read(&data).unwrap(), [0, 255, 127, 128, 3, 4]);
        assert_eq!(
            std::fs::read_to_string(dir.join("iq/burst.sigmf-meta")).unwrap(),
            meta(&recording)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_meta_of_samples_written_elsewhere_says_their_type_and_rate() {
        let dir = std::env::temp_dir().join(format!("sdr-sigmf-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let narrow = Meta {
            datatype: "ci16_le",
            sample_rate: 42_666.5,
            center_hz: 433.475e6,
            start: Some(start()),
            source: "rtl-sdr; 433.475 MHz decimated by 48",
            marks: &[],
        };
        write_meta(&dir, "narrow", &narrow).unwrap();
        let text = std::fs::read_to_string(dir.join("narrow.sigmf-meta")).unwrap();
        assert_eq!(text, meta_text(&narrow));
        for part in [
            r#""core:datatype": "ci16_le","#,
            r#""core:sample_rate": 42666.5,"#,
            r#""core:frequency": 433475000,"#,
        ] {
            assert!(text.contains(part), "{part} is not in {text}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_that_cannot_be_made_is_an_error() {
        let recording = recording(&[], &[]);
        assert!(write(Path::new("/dev/null/iq"), "burst", &recording).is_err());
    }
}
