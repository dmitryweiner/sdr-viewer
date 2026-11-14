//! The channels file: TOML the user may edit, one `[[channel]]` each.
//! The app only ever adds to its end; what is there — the user's names,
//! order and comments — is never rewritten.

use std::fmt::Write;
use std::path::Path;

use sdr_detect::Channel;
use toml::{Table, Value};

/// What a new file begins with.
pub const HEADER: &str = "\
# SDR Viewer's channels: the rows of the channel strip (screen 2).
#
# A channel is made here by itself wherever a signal is found that is in
# none. The file is yours to edit while the app is not running — it only
# adds new channels to the end, and reads the file when it starts:
#
#   [[channel]]
#   name = \"LPD 17\"      # optional; without it the row shows the frequency
#   freq_mhz = 433.475   # the centre
#   width_khz = 12.5
#   hide = true          # optional: no row, and no new channel made there
#
# Removing a channel only lets it be made again; hide it instead.
# X pressed twice on the channel strip removes from this file the channels
# that have neither a name nor `hide` and no signal on at that moment:
# give a name to what is to stay.
";

/// The channels of a file's text, in its order.
pub fn parse(text: &str) -> Result<Vec<Channel>, String> {
    let table: Table = toml::from_str(text).map_err(|err| err.to_string())?;
    let entries = match table.get("channel") {
        None => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err("`channel` must be written as [[channel]] tables".into()),
    };
    let channels = entries.iter().enumerate().map(|(index, entry)| {
        let wrap = |what: String| format!("channel {}: {what}", index + 1);
        let Value::Table(entry) = entry else {
            return Err(wrap("not a table".into()));
        };
        let number = |key: &str| match entry.get(key) {
            Some(Value::Float(value)) if *value > 0.0 && value.is_finite() => Ok(*value),
            Some(Value::Integer(value)) if *value > 0 => Ok(*value as f64),
            Some(_) => Err(wrap(format!("{key} must be a number above 0"))),
            None => Err(wrap(format!("{key} is missing"))),
        };
        let name = match entry.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.clone()).filter(|name| !name.is_empty()),
            Some(_) => return Err(wrap("name must be a string".into())),
        };
        let hidden = match entry.get("hide") {
            None => false,
            Some(Value::Boolean(hide)) => *hide,
            Some(_) => return Err(wrap("hide must be true or false".into())),
        };
        Ok(Channel {
            name,
            center_hz: number("freq_mhz")? * 1e6,
            width_hz: number("width_khz")? * 1e3,
            hidden,
        })
    });
    channels.collect()
}

/// `text` with those of `new` added to its end whose centre is in no
/// channel of the text, and how many that was. The text itself is left
/// as it is, byte for byte.
pub fn append(text: &str, new: &[Channel]) -> Result<(String, usize), String> {
    let there = parse(text)?;
    let mut out = text.to_string();
    let mut added = 0;
    for channel in new {
        if there.iter().any(|old| old.contains(channel.center_hz)) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        let _ = writeln!(out, "\n[[channel]]");
        if let Some(name) = &channel.name {
            let _ = writeln!(out, "name = {}", Value::String(name.clone()));
        }
        // Hertz as whole numbers first, so 433.475 MHz is not written as
        // 433.47499999999997.
        let _ = writeln!(out, "freq_mhz = {:?}", channel.center_hz.round() / 1e6);
        let _ = writeln!(out, "width_khz = {:?}", channel.width_hz.round() / 1e3);
        if channel.hidden {
            let _ = writeln!(out, "hide = true");
        }
        added += 1;
    }
    // What goes to the disk must read back.
    parse(&out)?;
    Ok((out, added))
}

/// `text` without its channels that have neither a name nor `hide` —
/// those the app made and the user did not take for their own — but for
/// those at the centres of `keep`; and how many went. Everything else stays byte for byte; the comment lines
/// that end a removed channel stay too, as they are most likely about
/// the next one.
pub fn forget_made(text: &str, keep: &[Channel]) -> Result<(String, usize), String> {
    parse(text)?;
    let is_header = |line: &str| line.trim_start().starts_with('[');
    let is_channel = |line: &str| {
        let code = line.split('#').next().unwrap_or_default();
        code.split_whitespace().collect::<String>() == "[[channel]]"
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut removed = 0;
    let mut start = 0;
    while start < lines.len() {
        // A table: its header and what follows up to the next header.
        let rest = &lines[start + 1..];
        let end = start
            + 1
            + rest
                .iter()
                .position(|line| is_header(line))
                .unwrap_or(rest.len());
        let table = &lines[start..end];
        let made = is_channel(table[0])
            && parse(&table.concat())?.first().is_some_and(|channel| {
                // A centre comes back from the file a hair off.
                let stays = |kept: &Channel| (kept.center_hz - channel.center_hz).abs() < 1.0;
                channel.name.is_none() && !channel.hidden && !keep.iter().any(stays)
            });
        if made {
            removed += 1;
            let blank_or_comment = |line: &str| {
                let line = line.trim();
                line.is_empty() || line.starts_with('#')
            };
            let tail = table
                .iter()
                .rev()
                .take_while(|line| blank_or_comment(line))
                .count();
            let comments = table[table.len() - tail..].iter();
            out.extend(comments.filter(|line| !line.trim().is_empty()).copied());
        } else {
            out.extend(table.iter().copied());
        }
        start = end;
    }
    // Blank lines that stood between the removed channels.
    let out = match removed {
        0 => out,
        _ => format!("{}\n", out.trim_end()),
    };
    parse(&out)?;
    Ok((out, removed))
}

/// Replaces the file at `path` in one step, so it is never half written.
fn replace(path: &Path, text: &str) -> Result<(), String> {
    let io = |err: std::io::Error| err.to_string();
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".part");
    std::fs::write(&partial, text).map_err(io)?;
    std::fs::rename(&partial, path).map_err(io)
}

/// Removes from the file at `path` the channels the app made (see
/// [`forget_made`]); no file is nothing to do.
pub fn forget(path: &Path, keep: &[Channel]) -> Result<usize, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.to_string()),
    };
    let (text, removed) = forget_made(&text, keep)?;
    if removed > 0 {
        replace(path, &text)?;
    }
    Ok(removed)
}

/// The channels of the file at `path`; none when there is no file yet.
pub fn load(path: &Path) -> Result<Vec<Channel>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err.to_string()),
    }
}

/// Adds `new` to the file at `path` as it is on the disk now (see
/// [`append`]), making the file and its directory if need be. The new
/// text replaces the old in one step, so the file is never half written.
pub fn add(path: &Path, new: &[Channel]) -> Result<usize, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HEADER.to_string(),
        Err(err) => return Err(err.to_string()),
    };
    let (text, added) = append(&text, new)?;
    if added == 0 {
        return Ok(0);
    }
    replace(path, &text)?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(center_hz: f64, width_hz: f64) -> Channel {
        Channel {
            name: None,
            center_hz,
            width_hz,
            hidden: false,
        }
    }

    const MINE: &str = "\
# my channels
[[channel]]
name = \"LPD 17\"   # the gate
freq_mhz = 433.475
width_khz = 12.5

[[channel]]
freq_mhz = 433.92
width_khz = 10
hide = true
";

    #[test]
    fn reads_names_frequencies_widths_and_what_is_hidden() {
        assert_eq!(
            parse(MINE).unwrap(),
            [
                Channel {
                    name: Some("LPD 17".into()),
                    center_hz: 433.475e6,
                    width_hz: 12_500.0,
                    hidden: false,
                },
                Channel {
                    name: None,
                    center_hz: 433.92e6,
                    width_hz: 10_000.0,
                    hidden: true,
                },
            ]
        );
        assert_eq!(parse("").unwrap(), []);
        assert_eq!(parse(HEADER).unwrap(), []);
        assert_eq!(parse("other = 1\n").unwrap(), []);
    }

    #[test]
    fn what_is_wrong_in_the_file_is_named() {
        let err = |text: &str| parse(text).unwrap_err();
        assert!(
            err("[[channel]\n").contains("line 1"),
            "{}",
            err("[[channel]\n")
        );
        assert_eq!(
            err("[[channel]]\nfreq_mhz = 433\nwidth_khz = 10\n[[channel]]\nwidth_khz = 10\n"),
            "channel 2: freq_mhz is missing"
        );
        assert_eq!(
            err("[[channel]]\nfreq_mhz = \"433M\"\nwidth_khz = 10\n"),
            "channel 1: freq_mhz must be a number above 0"
        );
        assert_eq!(
            err("[[channel]]\nfreq_mhz = 433\nwidth_khz = 0\n"),
            "channel 1: width_khz must be a number above 0"
        );
        assert_eq!(
            err("[[channel]]\nfreq_mhz = 433\nwidth_khz = 1\nname = 7\n"),
            "channel 1: name must be a string"
        );
        assert_eq!(
            err("[[channel]]\nfreq_mhz = 433\nwidth_khz = 1\nhide = 1\n"),
            "channel 1: hide must be true or false"
        );
        assert_eq!(
            err("channel = 5\n"),
            "`channel` must be written as [[channel]] tables"
        );
    }

    #[test]
    fn new_channels_go_to_the_end_and_the_rest_stays_as_it_was() {
        let new = [
            made(433.5e6, 10_000.0),
            // In the user's "LPD 17", and in the hidden one.
            made(433.478e6, 10_000.0),
            made(433.921e6, 10_000.0),
            Channel {
                name: Some("a \"quoted\" name".into()),
                center_hz: 144.8e6,
                width_hz: 180_500.0,
                hidden: true,
            },
        ];
        let (text, added) = append(MINE, &new).unwrap();
        assert_eq!(added, 2);
        assert!(text.starts_with(MINE));
        assert_eq!(
            &text[MINE.len()..],
            "\n[[channel]]\nfreq_mhz = 433.5\nwidth_khz = 10.0\n\
             \n[[channel]]\nname = 'a \"quoted\" name'\nfreq_mhz = 144.8\n\
             width_khz = 180.5\nhide = true\n"
        );
        let channels = parse(&text).unwrap();
        assert_eq!(channels.len(), 4);
        assert_eq!(channels[2], new[0]);
        assert_eq!(channels[3], new[3]);
        // Nothing new: nothing changes.
        assert_eq!(append(&text, &new).unwrap(), (text.clone(), 0));
        // A file that does not end its last line.
        let (text, _) = append("# mine", &new[..1]).unwrap();
        assert!(text.starts_with("# mine\n\n[[channel]]\n"), "{text}");
    }

    #[test]
    fn a_frequency_is_written_as_a_person_would() {
        for khz in [433_475.0, 433_075.0, 868_300.0, 1_090_000.0, 27_185.0] {
            let (text, _) = append("", &[made(khz * 1e3, 12_500.0)]).unwrap();
            let expected = format!("freq_mhz = {:?}\n", khz / 1e3);
            assert!(text.contains(&expected), "{text}");
            assert!(
                expected.len() <= "freq_mhz = 1090.000\n".len(),
                "{expected}"
            );
            assert_eq!(parse(&text).unwrap()[0].center_hz, khz * 1e3);
        }
    }

    #[test]
    fn a_file_that_does_not_read_is_not_added_to() {
        assert!(append("[[channel]]\nfreq_mhz = 1\n", &[made(433.5e6, 1e4)]).is_err());
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("sdr-viewer-channels-{name}-{}", std::process::id()));
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
    fn the_file_is_made_with_its_explanation_and_added_to_as_it_is_on_the_disk() {
        let dir = TempDir::new("add");
        let path = dir.0.join("deep/channels.toml");
        assert_eq!(load(&path).unwrap(), []);
        // Nothing to add: no file is made.
        assert_eq!(add(&path, &[]).unwrap(), 0);
        assert!(!path.exists());
        assert_eq!(add(&path, &[made(433.5e6, 1e4)]).unwrap(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(HEADER), "{text}");
        assert_eq!(load(&path).unwrap(), [made(433.5e6, 1e4)]);
        // The user edits it meanwhile: their text stays, ours follows.
        let edited = text.replace("[[channel]]", "[[channel]]\nname = \"gate\"");
        std::fs::write(&path, &edited).unwrap();
        let new = [made(433.5e6, 1e4), made(434.0e6, 1e4)];
        assert_eq!(add(&path, &new).unwrap(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(&edited), "{text}");
        let channels = load(&path).unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].name.as_deref(), Some("gate"));
        // Nothing is left beside it.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn the_made_channels_are_removed_and_the_users_text_stays() {
        let text = "\
# my channels

[[channel]]
freq_mhz = 433.2
width_khz = 10.0

[[channel]]
name = \"LPD 17\"   # the gate
freq_mhz = 433.475
width_khz = 12.5

[[channel]]   # made
freq_mhz = 433.5
width_khz = 10.0   # wide enough

# the receiver's own
[[channel]]
freq_mhz = 433.92
width_khz = 10
hide = true

[other]
key = 1

[[channel]]
freq_mhz = 434.0
width_khz = 10.0
";
        let (left, removed) = forget_made(text, &[]).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(
            left,
            "\
# my channels

[[channel]]
name = \"LPD 17\"   # the gate
freq_mhz = 433.475
width_khz = 12.5

# the receiver's own
[[channel]]
freq_mhz = 433.92
width_khz = 10
hide = true

[other]
key = 1
"
        );
        // Nothing made: not a byte changes.
        assert_eq!(forget_made(&left, &[]).unwrap(), (left.clone(), 0));
        assert_eq!(forget_made(MINE, &[]).unwrap(), (MINE.to_string(), 0));
        assert_eq!(forget_made("", &[]).unwrap(), (String::new(), 0));
        // Only made ones: the explanation stays.
        let (made, _) = append(HEADER, &[made(433.5e6, 1e4), made(434.5e6, 1e4)]).unwrap();
        assert_eq!(forget_made(&made, &[]).unwrap(), (HEADER.to_string(), 2));
        // One of them has a signal on: it stays.
        let (left, removed) = forget_made(&made, &[self::made(434.5e6, 1e4)]).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(parse(&left).unwrap(), [self::made(434.5e6, 1e4)]);
        assert!(forget_made("[[channel]\n", &[]).is_err());
    }

    #[test]
    fn the_file_forgets_what_was_made_and_can_be_added_to_again() {
        let dir = TempDir::new("forget");
        let path = dir.0.join("channels.toml");
        assert_eq!(forget(&path, &[]).unwrap(), 0);
        assert!(!path.exists());
        add(&path, &[made(433.5e6, 1e4), made(434.0e6, 1e4)]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let named = text.replacen("\n[[channel]]", "\n[[channel]]\nname = \"gate\"", 1);
        std::fs::write(&path, named).unwrap();
        assert_eq!(forget(&path, &[]).unwrap(), 1);
        assert_eq!(load(&path).unwrap().len(), 1);
        assert_eq!(forget(&path, &[]).unwrap(), 0);
        assert_eq!(add(&path, &[made(434.0e6, 1e4)]).unwrap(), 1);
        assert_eq!(load(&path).unwrap().len(), 2);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn a_file_the_user_broke_is_left_alone() {
        let dir = TempDir::new("broken");
        let path = dir.0.join("channels.toml");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(&path, "[[channel]\n").unwrap();
        assert!(load(&path).is_err());
        assert!(add(&path, &[made(433.5e6, 1e4)]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[channel]\n");
    }
}
