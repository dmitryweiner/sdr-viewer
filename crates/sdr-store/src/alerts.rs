//! The alerts file: TOML the user writes, one `[[alert]]` rule each. The
//! app reads it at the start; one that does not exist yet is made with an
//! example in its comments.

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
# The app reads this file when it starts. A rule:
#
#   [[alert]]
#   name = \"gate\"          # optional; without it the alert shows the frequency
#   from_mhz = 433.9       # the band the signal's centre must be in;
#   to_mhz = 434.0         # or freq_mhz = 433.95 with width_khz = 100
#   min_ms = 1000          # optional: on for at least this long (0)
#   min_db = 20            # optional: its peak this far above the noise (10)
#
# A signal sounds the tone once; a rule, at most once in 10 s.
";

/// The keys a rule may have.
const KEYS: [&str; 7] = [
    "name",
    "from_mhz",
    "to_mhz",
    "freq_mhz",
    "width_khz",
    "min_ms",
    "min_db",
];

/// The rules of a file's text, in its order.
pub fn parse(text: &str) -> Result<Vec<Rule>, String> {
    let table: Table = toml::from_str(text).map_err(|err| err.to_string())?;
    if let Some(key) = table.keys().find(|key| *key != "alert") {
        return Err(format!("unknown key `{key}`: rules are [[alert]] tables"));
    }
    let entries = match table.get("alert") {
        None => return Ok(Vec::new()),
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
        Ok(Rule {
            name,
            low_hz,
            high_hz,
            min_duration,
            min_above_db,
        })
    });
    rules.collect()
}

/// The rules of the file at `path`; none when there is no file.
pub fn load(path: &Path) -> Result<Vec<Rule>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
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
                },
                Rule {
                    name: "433.475 MHz".into(),
                    low_hz: 433.475e6 - 6250.0,
                    high_hz: 433.475e6 + 6250.0,
                    min_duration: Duration::ZERO,
                    min_above_db: 10.0,
                },
            ])
        );
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
            "unknown key `alarm`: rules are [[alert]] tables"
        );
        assert!(parse("[[alert]\n").is_err());
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
    fn a_file_not_there_is_no_rules_until_made_with_the_example() {
        let dir = TempDir::new("load");
        let path = dir.0.join("sdr-viewer/alerts.toml");
        assert_eq!(load(&path), Ok(Vec::new()));
        assert!(!path.exists());
        assert_eq!(make(&path), Ok(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), HEADER);
        assert_eq!(load(&path), Ok(Vec::new()));
        // The user's file is never made again.
        let rule = "\n[[alert]]\nfreq_mhz = 433.9\nwidth_khz = 10\n";
        std::fs::write(&path, format!("{HEADER}{rule}")).unwrap();
        assert_eq!(make(&path), Ok(false));
        assert_eq!(load(&path).unwrap().len(), 1);
        std::fs::write(&path, "[[alert]]\n").unwrap();
        assert!(load(&path).is_err());
    }
}
