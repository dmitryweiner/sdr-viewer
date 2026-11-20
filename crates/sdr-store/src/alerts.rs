//! The alerts file: TOML the user writes, one `[[alert]]` rule each. The
//! app reads it at the start; one that does not exist yet is made with an
//! example in its comments. The app only adds to its end — the rules
//! made with `!` — and never rewrites what is there.

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use sdr_core::units::format_hz;
use sdr_detect::Rule;
use sdr_detect::alert::DEFAULT_MIN_ABOVE_DB;
use toml::{Table, Value};

/// What a new file is.
pub const HEADER: &str = "\
# SDR Viewer's alerts. A signal that meets a rule here is told on the
# status line, with a tone, as soon as it does — while it is still on.
#
# The app reads this file when it starts. Above the rules, how many
# megabytes the alerts' recordings may take; after that no new ones are
# made (nothing is deleted):
#
#   record_limit_mb = 2000
#
# A rule:
#
#   [[alert]]
#   name = \"gate\"          # optional; without it the alert shows the frequency
#   from_mhz = 433.9       # the band the signal's centre must be in;
#   to_mhz = 434.0         # or freq_mhz = 433.95 with width_khz = 100
#   min_ms = 1000          # optional: on for at least this long (0)
#   min_db = 20            # optional: its peak this far above the noise (10)
#   record = true          # optional: record the signal (false)
#
# A signal sounds the tone once; a rule, at most once in 10 s. A
# recording is the signal's own band, narrowed (some 50 kS/s for a
# narrow one), from 1 s before it to 1 s after it, as a SigMF pair in
# alerts/ beside the samples r saves in the log.
# ! on the channel strip or the log adds a rule for the selected channel
# or event to the end of this file.
";

/// The keys a rule may have.
const KEYS: [&str; 8] = [
    "name",
    "from_mhz",
    "to_mhz",
    "freq_mhz",
    "width_khz",
    "min_ms",
    "min_db",
    "record",
];

/// How much the alerts' recordings may take when the file does not say.
pub const DEFAULT_RECORD_LIMIT_MB: f64 = 2000.0;

/// What the file says.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertsFile {
    pub rules: Vec<Rule>,
    /// Bytes the alerts' recordings may take, all of them together.
    pub record_limit: u64,
}

impl Default for AlertsFile {
    fn default() -> Self {
        AlertsFile {
            rules: Vec::new(),
            record_limit: (DEFAULT_RECORD_LIMIT_MB * 1e6) as u64,
        }
    }
}

/// The rules of a file's text, in its order.
pub fn parse(text: &str) -> Result<Vec<Rule>, String> {
    parse_file(text).map(|file| file.rules)
}

/// Everything a file's text says.
pub fn parse_file(text: &str) -> Result<AlertsFile, String> {
    let table: Table = toml::from_str(text).map_err(|err| err.to_string())?;
    let top = ["alert", "record_limit_mb"];
    if let Some(key) = table.keys().find(|key| !top.contains(&key.as_str())) {
        return Err(format!(
            "unknown key `{key}`: the file has record_limit_mb and [[alert]] tables"
        ));
    }
    let record_limit = match table.get("record_limit_mb") {
        None => AlertsFile::default().record_limit,
        Some(Value::Integer(mb)) if *mb >= 0 => (*mb as f64 * 1e6) as u64,
        Some(Value::Float(mb)) if *mb >= 0.0 && mb.is_finite() => (mb * 1e6) as u64,
        Some(_) => return Err("record_limit_mb must be a number, 0 or more".into()),
    };
    let entries = match table.get("alert") {
        None => {
            return Ok(AlertsFile {
                rules: Vec::new(),
                record_limit,
            });
        }
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err("`alert` must be written as [[alert]] tables".into()),
    };
    let rules = entries.iter().enumerate().map(|(index, entry)| {
        let wrap = |what: String| format!("alert {}: {what}", index + 1);
        let Value::Table(entry) = entry else {
            return Err(wrap("not a table".into()));
        };
        if let Some(key) = entry.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(wrap(format!(
                "unknown key `{key}` (a rule has {})",
                KEYS.join(", ")
            )));
        }
        let number = |key: &str| match entry.get(key) {
            Some(Value::Float(value)) if value.is_finite() => Ok(Some(*value)),
            Some(Value::Integer(value)) => Ok(Some(*value as f64)),
            Some(_) => Err(wrap(format!("{key} must be a number"))),
            None => Ok(None),
        };
        let positive = |key: &str| match number(key)? {
            Some(value) if value <= 0.0 => Err(wrap(format!("{key} must be above 0"))),
            value => Ok(value),
        };
        let (low_hz, high_hz) = match (
            positive("from_mhz")?,
            positive("to_mhz")?,
            positive("freq_mhz")?,
            positive("width_khz")?,
        ) {
            (Some(from), Some(to), None, None) if to > from => (from * 1e6, to * 1e6),
            (Some(_), Some(_), None, None) => {
                return Err(wrap("to_mhz must be above from_mhz".into()));
            }
            (None, None, Some(freq), Some(width)) => {
                let (center, half) = (freq * 1e6, width * 1e3 / 2.0);
                (center - half, center + half)
            }
            _ => {
                return Err(wrap(
                    "the band is from_mhz and to_mhz, or freq_mhz and width_khz".into(),
                ));
            }
        };
        let name = match entry.get("name") {
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            None | Some(Value::String(_)) => format_hz((low_hz + high_hz) / 2.0),
            Some(_) => return Err(wrap("name must be a string".into())),
        };
        let min_duration = match number("min_ms")? {
            Some(ms) if ms < 0.0 => return Err(wrap("min_ms must not be below 0".into())),
            Some(ms) => Duration::from_secs_f64(ms / 1e3),
            None => Duration::ZERO,
        };
        let min_above_db = number("min_db")?.map_or(DEFAULT_MIN_ABOVE_DB, |db| db as f32);
        let record = match entry.get("record") {
            None => false,
            Some(Value::Boolean(record)) => *record,
            Some(_) => return Err(wrap("record must be true or false".into())),
        };
        Ok(Rule {
            name,
            low_hz,
            high_hz,
            min_duration,
            min_above_db,
            record,
        })
    });
    Ok(AlertsFile {
        rules: rules.collect::<Result<_, _>>()?,
        record_limit,
    })
}

/// `text` with those of `new` added to its end whose band no rule of the
/// text has, and how many that was. The text itself is left as it is,
/// byte for byte. Every key is written, so the user sees what to change.
pub fn append(text: &str, new: &[Rule]) -> Result<(String, usize), String> {
    let mut there = parse(text)?;
    let mut out = text.to_string();
    let mut added = 0;
    for rule in new {
        if there.iter().any(|old| old.same_band(rule)) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        // Hertz as whole numbers first, so 433.475 MHz is not written as
        // 433.47499999999997.
        let center = ((rule.low_hz + rule.high_hz) / 2.0).round();
        let width = (rule.high_hz - rule.low_hz).round();
        let _ = writeln!(out, "\n[[alert]]");
        let _ = writeln!(out, "name = {}", Value::String(rule.name.clone()));
        let _ = writeln!(out, "freq_mhz = {:?}", center / 1e6);
        let _ = writeln!(out, "width_khz = {:?}", width / 1e3);
        let _ = writeln!(out, "min_ms = {}", rule.min_duration.as_millis());
        let _ = writeln!(out, "min_db = {:?}", f64::from(rule.min_above_db));
        let _ = writeln!(out, "record = {}", rule.record);
        there.push(rule.clone());
        added += 1;
    }
    // What goes to the disk must read back.
    parse(&out)?;
    Ok((out, added))
}

/// Adds `new` to the file at `path` as it is on the disk now (see
/// [`append`]), making the file, with the example, if need be. The new
/// text replaces the old in one step, so the file is never half written.
pub fn add(path: &Path, new: &[Rule]) -> Result<usize, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HEADER.to_string(),
        Err(err) => return Err(err.to_string()),
    };
    let (text, added) = append(&text, new)?;
    if added > 0 {
        crate::channels::replace(path, &text)?;
    }
    Ok(added)
}

/// What the file at `path` says; no rules when there is no file.
pub fn load(path: &Path) -> Result<AlertsFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_file(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AlertsFile::default()),
        Err(err) => Err(err.to_string()),
    }
}

/// Makes the file at `path` with the example of [`HEADER`] and no rules,
/// if there is none, so the user finds where to write theirs; says
/// whether it did.
pub fn make(path: &Path) -> Result<bool, String> {
    if path.try_exists().map_err(|err| err.to_string())? {
        return Ok(false);
    }
    crate::channels::replace(path, HEADER)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_rule_is_a_band_a_duration_and_a_level_with_a_name() {
        let text = "\
[[alert]]
name = \"gate\"
from_mhz = 433.9
to_mhz = 434
min_ms = 1000
min_db = 20.5

[[alert]]   # a channel's band, everything else as it comes
freq_mhz = 433.475
width_khz = 12.5
";
        assert_eq!(
            parse(text),
            Ok(vec![
                Rule {
                    name: "gate".into(),
                    low_hz: 433.9e6,
                    high_hz: 434.0e6,
                    min_duration: ms(1000),
                    min_above_db: 20.5,
                    record: false,
                },
                Rule {
                    name: "433.475 MHz".into(),
                    low_hz: 433.475e6 - 6250.0,
                    high_hz: 433.475e6 + 6250.0,
                    min_duration: Duration::ZERO,
                    min_above_db: 10.0,
                    record: false,
                },
            ])
        );
    }

    #[test]
    fn a_rule_records_when_asked_and_the_file_says_how_much_may_be_kept() {
        let text =
            "record_limit_mb = 500\n[[alert]]\nfreq_mhz = 433.9\nwidth_khz = 10\nrecord = true\n";
        let file = parse_file(text).unwrap();
        assert_eq!(file.record_limit, 500_000_000);
        assert!(file.rules[0].record);
        assert_eq!(parse_file("").unwrap().record_limit, 2_000_000_000);
        assert_eq!(
            parse_file("record_limit_mb = 0.5").unwrap().record_limit,
            500_000
        );
        assert_eq!(
            parse_file("record_limit_mb = -1").unwrap_err(),
            "record_limit_mb must be a number, 0 or more"
        );
        assert_eq!(
            parse("[[alert]]\nfreq_mhz = 433.9\nwidth_khz = 10\nrecord = 1\n").unwrap_err(),
            "alert 1: record must be true or false"
        );
        // What the app adds says whether it records.
        let recording = Rule {
            record: true,
            ..made()
        };
        let (text, _) = append(MINE, &[recording]).unwrap();
        assert!(text.ends_with("min_db = 10.0\nrecord = true\n"), "{text}");
        assert!(parse(&text).unwrap()[1].record);
    }

    #[test]
    fn the_header_alone_is_no_rules_and_its_example_is_one() {
        assert_eq!(parse(HEADER), Ok(Vec::new()));
        assert_eq!(parse(""), Ok(Vec::new()));
        // The example, uncommented, reads as it says.
        let example: String = HEADER
            .lines()
            .filter_map(|line| line.strip_prefix("#   "))
            .map(|line| format!("{line}\n"))
            .collect();
        let rules = parse(&example).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "gate");
        assert_eq!((rules[0].low_hz, rules[0].high_hz), (433.9e6, 434.0e6));
        assert_eq!(rules[0].min_duration, ms(1000));
        assert_eq!(rules[0].min_above_db, 20.0);
    }

    #[test]
    fn a_rule_that_does_not_read_says_which_and_why() {
        let wrong = |rule: &str| parse(&format!("[[alert]]\n{rule}\n")).unwrap_err();
        assert_eq!(
            wrong("from_mhz = 434\nto_mhz = 433.9"),
            "alert 1: to_mhz must be above from_mhz"
        );
        let no_band = "alert 1: the band is from_mhz and to_mhz, or freq_mhz and width_khz";
        assert_eq!(wrong("from_mhz = 433.9"), no_band);
        assert_eq!(wrong("name = \"x\""), no_band);
        assert_eq!(
            wrong("from_mhz = 433.9\nto_mhz = 434\nfreq_mhz = 433.95\nwidth_khz = 10"),
            no_band
        );
        assert_eq!(
            wrong("freq_mhz = 433.9\nwidth_khz = 0"),
            "alert 1: width_khz must be above 0"
        );
        assert_eq!(
            wrong("freq_mhz = \"433.9\"\nwidth_khz = 10"),
            "alert 1: freq_mhz must be a number"
        );
        assert_eq!(
            wrong("freq_mhz = 433.9\nwidth_khz = 10\nmin_ms = -1"),
            "alert 1: min_ms must not be below 0"
        );
        assert_eq!(
            wrong("freq_mhz = 433.9\nwidth_khz = 10\nname = 5"),
            "alert 1: name must be a string"
        );
        // A key mistyped is not a rule quietly different from what was meant.
        assert!(
            wrong("freq_mhz = 433.9\nwidth_khz = 10\nmin_dB = 20")
                .starts_with("alert 1: unknown key `min_dB` (a rule has name, from_mhz, ")
        );
        assert_eq!(
            parse("[alert]\nfreq_mhz = 433.9").unwrap_err(),
            "`alert` must be written as [[alert]] tables"
        );
        assert_eq!(
            parse("[[alarm]]\nfreq_mhz = 433.9").unwrap_err(),
            "unknown key `alarm`: the file has record_limit_mb and [[alert]] tables"
        );
        assert!(parse("[[alert]\n").is_err());
    }

    /// What `!` makes for the channel "LPD 17", 433.475 MHz, 12.5 kHz wide.
    fn made() -> Rule {
        Rule {
            name: "LPD 17".into(),
            low_hz: 433.46875e6,
            high_hz: 433.48125e6,
            min_duration: Duration::ZERO,
            min_above_db: DEFAULT_MIN_ABOVE_DB,
            record: false,
        }
    }

    const MINE: &str =
        "# my alerts\n[[alert]]\nname = \"gate\"  # the gate\nfrom_mhz = 433.9\nto_mhz = 434\n";

    #[test]
    fn a_rule_made_in_the_app_goes_to_the_end_with_every_key_written() {
        let (text, added) = append(MINE, &[made()]).unwrap();
        assert_eq!(added, 1);
        assert_eq!(
            text,
            format!(
                "{MINE}\n[[alert]]\nname = \"LPD 17\"\nfreq_mhz = 433.475\nwidth_khz = 12.5\nmin_ms = 0\nmin_db = 10.0\nrecord = false\n"
            )
        );
        let rules = parse(&text).unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules[1].same_band(&made()));
        assert_eq!(rules[1].name, "LPD 17");
        // A text without its last line's end gets one first.
        let (text, _) = append(MINE.trim_end(), &[made()]).unwrap();
        assert!(text.contains("to_mhz = 434\n\n[[alert]]"), "{text}");
    }

    #[test]
    fn a_band_the_file_has_already_is_not_added_again() {
        let (once, _) = append(MINE, &[made()]).unwrap();
        assert_eq!(append(&once, &[made()]), Ok((once.clone(), 0)));
        // Nor twice in one go; the gate's band has a rule.
        let gate = Rule {
            name: "the gate again".into(),
            low_hz: 433.9e6,
            high_hz: 434.0e6,
            ..made()
        };
        assert_eq!(append(MINE, &[made(), made(), gate]).unwrap().1, 1);
        // A file that does not read is not added to.
        assert!(append("[[alert]]\n", &[made()]).is_err());
    }

    /// A directory of its own per test, removed when dropped.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("sdr-store-alerts-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn adding_to_a_file_not_there_makes_it_with_the_example_first() {
        let dir = TempDir::new("add");
        let path = dir.0.join("sdr-viewer/alerts.toml");
        assert_eq!(add(&path, &[made()]), Ok(1));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(HEADER), "{text}");
        assert_eq!(load(&path).unwrap().rules.len(), 1);
        // What the user wrote meanwhile stays.
        std::fs::write(&path, format!("{text}# mine\n")).unwrap();
        assert_eq!(add(&path, &[made()]), Ok(0));
        let other = Rule {
            low_hz: 433.5e6,
            high_hz: 433.51e6,
            ..made()
        };
        assert_eq!(add(&path, &[other]), Ok(1));
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(&format!("{text}# mine\n")), "{after}");
    }

    #[test]
    fn a_file_not_there_is_no_rules_until_made_with_the_example() {
        let dir = TempDir::new("load");
        let path = dir.0.join("sdr-viewer/alerts.toml");
        assert_eq!(load(&path), Ok(AlertsFile::default()));
        assert!(!path.exists());
        assert_eq!(make(&path), Ok(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), HEADER);
        assert_eq!(load(&path), Ok(AlertsFile::default()));
        // The user's file is never made again.
        let rule = "\n[[alert]]\nfreq_mhz = 433.9\nwidth_khz = 10\n";
        std::fs::write(&path, format!("{HEADER}{rule}")).unwrap();
        assert_eq!(make(&path), Ok(false));
        assert_eq!(load(&path).unwrap().rules.len(), 1);
        std::fs::write(&path, "[[alert]]\n").unwrap();
        assert!(load(&path).is_err());
    }
}
