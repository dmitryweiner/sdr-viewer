//! Command line: which source to open and how.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use clap::Parser;
use sdr_core::file::{FileSource, SampleFormat};
use sdr_core::generator::{Generator, GeneratorConfig};
use sdr_core::pace::{Paced, SystemClock};
use sdr_core::units::parse_hz;
use sdr_core::{Source, SourceInfo};
use sdr_rtl::{RtlConfig, RtlSource};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// An IQ recording to play (.cu8, .cs8, .cs16, .cf32; .raw and .bin are
    /// read as cu8). Without it the built-in signal generator runs.
    pub file: Option<PathBuf>,

    /// Sample rate, e.g. 2.4M or 2048k.
    #[arg(long, value_parser = parse_hz_arg, default_value = "2.4M")]
    pub rate: f64,

    /// Centre frequency, e.g. 433.92M.
    #[arg(long, value_parser = parse_hz_arg, default_value = "433.92M")]
    pub center: f64,

    /// Sample format of the file when its extension does not say it.
    #[arg(long, value_parser = parse_format_arg)]
    pub format: Option<SampleFormat>,

    /// Start the recording over when it ends.
    #[arg(long = "loop")]
    pub looping: bool,

    /// Receive from the first RTL-SDR instead of a file or the generator.
    #[arg(long, conflicts_with_all = ["file", "format", "looping"])]
    pub rtlsdr: bool,

    /// Tuner gain in dB for --rtlsdr, set to the tuner's nearest step;
    /// automatic without it.
    #[arg(long, requires = "rtlsdr")]
    pub gain: Option<f32>,

    /// Do not open the interface: analyse the start of the stream as fast
    /// as it reads and print the spectrum screen as text, COLSxROWS.
    #[arg(long, value_name = "COLSxROWS", value_parser = parse_size_arg)]
    pub snapshot: Option<(u16, u16)>,

    /// Seconds of the stream a snapshot looks at.
    #[arg(long, default_value_t = 5.0)]
    pub secs: f64,

    /// Keep the detected events in this SQLite file. With --rtlsdr they
    /// are kept anyway, in sdr-viewer/events.db under $XDG_DATA_HOME or
    /// ~/.local/share; a recording or the generator is kept only when
    /// this names a file.
    #[arg(long, value_name = "FILE", conflicts_with = "snapshot")]
    pub db: Option<PathBuf>,

    /// Keep no events, not even with --rtlsdr.
    #[arg(long, conflicts_with = "db")]
    pub no_db: bool,

    /// Seconds of samples held in memory, so that an event's samples can
    /// be saved after it was seen: 2 bytes a sample, 144 MB for 30 s at
    /// 2.4 MS/s. 0 holds none.
    #[arg(long, value_name = "SECS", default_value_t = 30.0, value_parser = parse_buffer_arg)]
    pub iq_buffer: f64,

    /// Where the log's `r` saves an event's samples, as a SigMF pair of
    /// files. Without it: sdr-viewer/iq under $XDG_DATA_HOME or
    /// ~/.local/share.
    #[arg(long, value_name = "DIR")]
    pub iq_dir: Option<PathBuf>,
}

/// The longest --iq-buffer: 10 minutes are 2.9 GB at 2.4 MS/s.
const MAX_IQ_BUFFER_S: f64 = 600.0;

/// Where the events go, if anywhere; `env` looks up an environment
/// variable.
pub fn db_path(args: &Args, env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if args.no_db || args.snapshot.is_some() {
        return None;
    }
    if args.db.is_some() || !args.rtlsdr {
        return args.db.clone();
    }
    Some(data_dir(env)?.join("events.db"))
}

/// Where saved samples go, if anywhere.
pub fn iq_dir(args: &Args, env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    args.iq_dir
        .clone()
        .or_else(|| Some(data_dir(env)?.join("iq")))
}

/// The app's own directory among the user's data.
fn data_dir(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let data = set("XDG_DATA_HOME").or_else(|| Some(set("HOME")?.join(".local/share")))?;
    Some(data.join("sdr-viewer"))
}

fn parse_buffer_arg(text: &str) -> Result<f64, String> {
    let seconds = text.parse().ok();
    seconds
        .filter(|seconds| (0.0..=MAX_IQ_BUFFER_S).contains(seconds))
        .ok_or_else(|| format!("not a number of seconds from 0 to {MAX_IQ_BUFFER_S}: {text:?}"))
}

fn parse_size_arg(text: &str) -> Result<(u16, u16), String> {
    let size = text
        .split_once('x')
        .and_then(|(cols, rows)| Some((cols.parse().ok()?, rows.parse().ok()?)));
    size.ok_or_else(|| format!("not a size: {text:?} (try 160x45)"))
}

fn parse_hz_arg(text: &str) -> Result<f64, String> {
    parse_hz(text).ok_or_else(|| format!("not a frequency: {text:?} (try 433.92M or 2400k)"))
}

fn parse_format_arg(text: &str) -> Result<SampleFormat, String> {
    SampleFormat::from_name(text).ok_or_else(|| {
        let names: Vec<_> = SampleFormat::ALL.iter().map(|f| f.name()).collect();
        format!("unknown format {text:?}; one of {}", names.join(", "))
    })
}

/// Opens what the arguments ask for; `paced` holds it to real time.
pub fn open_source(args: &Args, paced: bool) -> Result<Box<dyn Source>, String> {
    let sample_rate = args.rate.round() as u32;
    if sample_rate == 0 {
        return Err("the sample rate must be above zero".into());
    }
    if args.rtlsdr {
        let config = RtlConfig {
            sample_rate,
            center_hz: args.center,
            gain_db: args.gain,
        };
        // The hardware paces itself.
        return match RtlSource::open(&config) {
            Ok(source) => Ok(Box::new(source)),
            Err(err) => Err(format!(
                "cannot open the RTL-SDR: {err} (is it powered and on the bus? `lsusb` lists it as 0bda:2838)"
            )),
        };
    }
    let source: Box<dyn Source> = match &args.file {
        None => {
            let mut generator = Generator::new(GeneratorConfig {
                sample_rate,
                ..GeneratorConfig::demo()
            });
            generator.tune(args.center).map_err(|err| err.to_string())?;
            Box::new(generator)
        }
        Some(path) => Box::new(open_file(path, args, sample_rate)?),
    };
    if paced {
        Ok(Box::new(Paced::new(source, SystemClock::new())))
    } else {
        Ok(source)
    }
}

fn open_file(
    path: &Path,
    args: &Args,
    sample_rate: u32,
) -> Result<FileSource<BufReader<File>>, String> {
    let format = args
        .format
        .or_else(|| SampleFormat::from_path(path))
        .ok_or_else(|| {
            format!(
                "{}: unknown sample format; name it with --format",
                path.display()
            )
        })?;
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let info = SourceInfo {
        sample_rate,
        center_hz: args.center,
        description: name,
    };
    Ok(FileSource::new(
        BufReader::new(file),
        format,
        info,
        args.looping,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_core::Iq;

    fn args(list: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("sdr-viewer").chain(list.iter().copied())).unwrap()
    }

    /// A scratch file of its own per test, removed when dropped.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, bytes: &[u8]) -> Self {
            let dir = std::env::temp_dir().join(format!("sdr-viewer-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            let found = vars.iter().find(|(key, _)| *key == name);
            found.map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn events_of_the_receiver_go_to_the_user_s_data_directory() {
        let home = [("HOME", "/home/dmw")];
        assert_eq!(
            db_path(&args(&["--rtlsdr"]), env(&home)),
            Some("/home/dmw/.local/share/sdr-viewer/events.db".into())
        );
        let xdg = [("HOME", "/home/dmw"), ("XDG_DATA_HOME", "/data")];
        assert_eq!(
            db_path(&args(&["--rtlsdr"]), env(&xdg)),
            Some("/data/sdr-viewer/events.db".into())
        );
        let empty = [("HOME", "/home/dmw"), ("XDG_DATA_HOME", "")];
        assert_eq!(
            db_path(&args(&["--rtlsdr"]), env(&empty)),
            Some("/home/dmw/.local/share/sdr-viewer/events.db".into())
        );
        assert_eq!(db_path(&args(&["--rtlsdr"]), env(&[])), None);
    }

    #[test]
    fn a_recording_or_the_generator_is_kept_only_when_asked() {
        let home = [("HOME", "/home/dmw")];
        assert_eq!(db_path(&args(&[]), env(&home)), None);
        assert_eq!(db_path(&args(&["capture.cu8"]), env(&home)), None);
        assert_eq!(
            db_path(&args(&["capture.cu8", "--db", "x.db"]), env(&home)),
            Some("x.db".into())
        );
        assert_eq!(
            db_path(&args(&["--rtlsdr", "--db", "x.db"]), env(&home)),
            Some("x.db".into())
        );
    }

    #[test]
    fn no_db_and_snapshots_keep_nothing() {
        let home = [("HOME", "/home/dmw")];
        assert_eq!(db_path(&args(&["--rtlsdr", "--no-db"]), env(&home)), None);
        let snapshot = args(&["--rtlsdr", "--snapshot", "160x45"]);
        assert_eq!(db_path(&snapshot, env(&home)), None);
        let parse = |list: &[&str]| {
            Args::try_parse_from(std::iter::once("sdr-viewer").chain(list.iter().copied()))
        };
        assert!(parse(&["--db", "x.db", "--no-db"]).is_err());
        assert!(parse(&["--db", "x.db", "--snapshot", "160x45"]).is_err());
    }

    #[test]
    fn samples_are_saved_beside_the_events_or_where_asked() {
        let home = [("HOME", "/home/dmw")];
        assert_eq!(
            iq_dir(&args(&[]), env(&home)),
            Some("/home/dmw/.local/share/sdr-viewer/iq".into())
        );
        let xdg = [("HOME", "/home/dmw"), ("XDG_DATA_HOME", "/data")];
        assert_eq!(
            iq_dir(&args(&["--rtlsdr"]), env(&xdg)),
            Some("/data/sdr-viewer/iq".into())
        );
        assert_eq!(
            iq_dir(&args(&["--iq-dir", "cuts"]), env(&home)),
            Some("cuts".into())
        );
        assert_eq!(iq_dir(&args(&[]), env(&[])), None);
    }

    #[test]
    fn the_iq_buffer_is_thirty_seconds_unless_told() {
        assert_eq!(args(&[]).iq_buffer, 30.0);
        assert_eq!(args(&["--iq-buffer", "0"]).iq_buffer, 0.0);
        assert_eq!(args(&["--iq-buffer", "2.5"]).iq_buffer, 2.5);
        let parse = |list: &[&str]| {
            Args::try_parse_from(std::iter::once("sdr-viewer").chain(list.iter().copied()))
        };
        assert!(parse(&["--iq-buffer", "-1"]).is_err());
        assert!(parse(&["--iq-buffer", "601"]).is_err());
        assert!(parse(&["--iq-buffer", "nan"]).is_err());
        assert!(parse(&["--iq-buffer", "long"]).is_err());
    }

    #[test]
    fn without_a_file_runs_the_generator_with_the_defaults() {
        let source = open_source(&args(&[]), true).unwrap();
        let info = source.info();
        assert_eq!(info.description, "generator");
        assert_eq!(info.sample_rate, 2_400_000);
        assert_eq!(info.center_hz, 433_920_000.0);
    }

    #[test]
    fn rate_and_centre_accept_suffixes() {
        let source = open_source(&args(&["--rate", "1M", "--center", "144.8M"]), true).unwrap();
        assert_eq!(source.info().sample_rate, 1_000_000);
        assert_eq!(source.info().center_hz, 144_800_000.0);
    }

    #[test]
    fn bad_values_are_refused_by_the_parser() {
        let parse = |list: &[&str]| {
            Args::try_parse_from(std::iter::once("sdr-viewer").chain(list.iter().copied()))
        };
        assert!(parse(&["--rate", "fast"]).is_err());
        assert!(parse(&["--format", "wav"]).is_err());
        assert!(parse(&["--snapshot", "big"]).is_err());
        assert!(parse(&["--rtlsdr", "x.cu8"]).is_err());
        assert!(parse(&["--gain", "30"]).is_err());
        assert_eq!(args(&["--rtlsdr", "--gain", "29.7"]).gain, Some(29.7));
        assert_eq!(args(&["--snapshot", "160x45"]).snapshot, Some((160, 45)));
        assert!(open_source(&args(&["--rate", "0"]), true).is_err());
    }

    #[test]
    fn a_file_is_read_in_the_format_of_its_extension() {
        let file = TempFile::new("capture.cs8", &[64, 0, 0, 64]);
        let path = file.0.to_str().unwrap();
        let mut source = open_source(&args(&[path, "--rate", "1k"]), true).unwrap();
        assert_eq!(source.info().description, "capture.cs8");
        let mut buf = [Iq::new(0.0, 0.0); 4];
        assert_eq!(source.read(&mut buf).unwrap(), 2);
        assert_eq!(buf[..2], [Iq::new(0.5, 0.0), Iq::new(0.0, 0.5)]);
    }

    #[test]
    fn format_overrides_the_extension_and_is_required_without_one() {
        let file = TempFile::new("capture.dat", &[64, 0]);
        let path = file.0.to_str().unwrap();
        let err = open_source(&args(&[path]), true).err().unwrap();
        assert!(err.contains("--format"), "{err}");
        assert!(open_source(&args(&[path, "--format", "cs8"]), true).is_ok());
    }

    #[test]
    fn a_missing_file_names_itself_in_the_error() {
        let err = open_source(&args(&["/nonexistent/x.cu8"]), true)
            .err()
            .unwrap();
        assert!(err.contains("/nonexistent/x.cu8"), "{err}");
    }
}
