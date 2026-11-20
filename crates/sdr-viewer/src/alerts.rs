//! Alerts in the running app: the rules of the user's file, read at the
//! start, and the tone — two short beeps, played through the speaker by
//! `aplay` on a thread of its own, so that nothing waits for the sound.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::thread::JoinHandle;

use sdr_detect::Rule;
use sdr_store::alerts::AlertsFile;

use crate::reader::Update;
use crate::writer;

/// What the file at `path` says. Unless `make` is false, a file that is
/// not there is made, with an example in its comments.
pub fn load(path: &Path, make: bool) -> Result<AlertsFile, String> {
    let wrap = |err: String| {
        format!(
            "the alerts file {}: {err} (--no-alerts runs without it)",
            path.display()
        )
    };
    if make {
        sdr_store::alerts::make(path).map_err(wrap)?;
    }
    sdr_store::alerts::load(path).map_err(wrap)
}

/// Samples a second of the tone's WAV: plenty for a beep, and what any
/// sound card takes.
const RATE: u32 = 16_000;
/// The beeps' pitch, length, the pause between them and their loudness
/// (a share of full scale).
const PITCH_HZ: f64 = 1_500.0;
const BEEP_S: f64 = 0.12;
const PAUSE_S: f64 = 0.08;
const LOUDNESS: f64 = 0.5;
/// Each beep rises and falls over this long, or it clicks.
const RAMP_S: f64 = 0.005;

/// Starts the thread that adds the rules sent to it — those made with
/// `!` — to the file at `path`, until the sender is gone. A failure is
/// told to the interface once; after it nothing more is written.
pub fn save(path: PathBuf, updates: SyncSender<Update>) -> (Sender<Vec<Rule>>, JoinHandle<()>) {
    let (new, rx) = channel();
    let saver = std::thread::spawn(move || {
        let write = |rules: &[Rule]| sdr_store::alerts::add(&path, rules).map(|_| ());
        if let Err(message) = writer::run(rx, write) {
            let _ = updates.send(Update::Notice(format!(
                "the alerts file is not changed: {message}"
            )));
        }
    });
    (new, saver)
}

/// The tone as a WAV file: 16-bit mono PCM, two beeps.
pub fn wav() -> Vec<u8> {
    let beep = (BEEP_S * f64::from(RATE)) as usize;
    let pause = (PAUSE_S * f64::from(RATE)) as usize;
    let ramp = RAMP_S * f64::from(RATE);
    let samples = (0..beep * 2 + pause).map(|index| {
        let at = if index < beep {
            index
        } else if index < beep + pause {
            return 0;
        } else {
            index - beep - pause
        };
        let envelope = (at as f64 / ramp).min((beep - at) as f64 / ramp).min(1.0);
        let phase = std::f64::consts::TAU * PITCH_HZ * at as f64 / f64::from(RATE);
        (LOUDNESS * envelope * phase.sin() * f64::from(i16::MAX)).round() as i16
    });
    let data: Vec<u8> = samples.flat_map(i16::to_le_bytes).collect();
    let data_len = u32::try_from(data.len()).expect("a beep is short");
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    // PCM, one channel, the rate, bytes a second, bytes a frame, bits.
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

/// Plays `wav` with `aplay` and waits for it to end.
pub fn aplay(wav: &[u8]) -> Result<(), String> {
    let mut child = Command::new("aplay")
        .args(["-q", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("no alert tone: aplay does not start ({err})"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A write that fails is told by aplay's exit.
        let _ = stdin.write_all(wav);
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("no alert tone: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let why = said.lines().next().unwrap_or("it failed").trim();
    Err(format!("no alert tone: aplay: {why}"))
}

/// Starts the thread that plays the tone with `play` (such as [`aplay`])
/// each time it is asked to. What is asked while a tone plays is not played after it.
/// When `play` fails, the interface is told why, once, and no tone is
/// played again.
pub fn start_tone(
    mut play: impl FnMut(&[u8]) -> Result<(), String> + Send + 'static,
    updates: SyncSender<Update>,
) -> (Sender<()>, JoinHandle<()>) {
    let (ask, asked) = channel::<()>();
    let player = std::thread::spawn(move || {
        let wav = wav();
        let mut failed = false;
        while asked.recv().is_ok() {
            if !failed && let Err(why) = play(&wav) {
                failed = true;
                let _ = updates.send(Update::Notice(why));
            }
            // Asked while it played.
            asked.try_iter().for_each(drop);
        }
    });
    (ask, player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn the_file_is_made_only_when_asked_and_a_wrong_one_says_where() {
        let dir = std::env::temp_dir().join(format!("sdr-viewer-alerts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("alerts.toml");
        assert_eq!(load(&path, false), Ok(AlertsFile::default()));
        assert!(!path.exists());
        assert_eq!(load(&path, true), Ok(AlertsFile::default()));
        assert!(path.exists());
        std::fs::write(&path, "[[alert]]\n").unwrap();
        let told = load(&path, true).unwrap_err();
        assert!(
            told.starts_with(&format!("the alerts file {}: alert 1: ", path.display())),
            "{told}"
        );
        assert!(told.ends_with(" (--no-alerts runs without it)"), "{told}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rules_made_go_to_the_end_of_the_file_and_a_failure_is_told_once() {
        let dir =
            std::env::temp_dir().join(format!("sdr-viewer-alerts-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("alerts.toml");
        let rule = |low_hz: f64| Rule {
            name: "LPD 17".into(),
            low_hz,
            high_hz: low_hz + 12.5e3,
            min_duration: std::time::Duration::ZERO,
            min_above_db: 10.0,
            record: false,
        };
        let (updates, told) = sync_channel(4);
        let (new, saver) = save(path.clone(), updates);
        new.send(vec![rule(433.46875e6)]).unwrap();
        drop(new);
        saver.join().unwrap();
        assert_eq!(load(&path, false).unwrap().rules, [rule(433.46875e6)]);
        assert_eq!(told.try_iter().count(), 0);
        // A file that no longer reads is not touched.
        std::fs::write(&path, "[[alert]]\n").unwrap();
        let (updates, told) = sync_channel(4);
        let (new, saver) = save(path.clone(), updates);
        new.send(vec![rule(433.5e6)]).unwrap();
        drop(new);
        saver.join().unwrap();
        match told.try_iter().collect::<Vec<_>>().as_slice() {
            [Update::Notice(notice)] => {
                assert!(
                    notice.starts_with("the alerts file is not changed: "),
                    "{notice}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[alert]]\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_tone_is_a_wav_of_two_beeps_a_third_of_a_second_long() {
        let wav = wav();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let u32_at = |at: usize| u32::from_le_bytes(wav[at..at + 4].try_into().unwrap());
        let u16_at = |at: usize| u16::from_le_bytes(wav[at..at + 2].try_into().unwrap());
        assert_eq!(u32_at(4) as usize, wav.len() - 8);
        assert_eq!((u16_at(20), u16_at(22)), (1, 1));
        assert_eq!((u32_at(24), u32_at(28)), (16_000, 32_000));
        assert_eq!((u16_at(32), u16_at(34)), (2, 16));
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(40) as usize, wav.len() - 44);
        let samples: Vec<i16> = wav[44..]
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        // 120 ms, 80 ms of silence, 120 ms.
        assert_eq!(samples.len(), 5_120);
        let loudest = |part: &[i16]| part.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!((16_000..=16_384).contains(&loudest(&samples[..1_920])));
        assert_eq!(loudest(&samples[1_920..3_200]), 0);
        assert!((16_000..=16_384).contains(&loudest(&samples[3_200..])));
        // Each beep rises from nothing and falls to it: no click.
        for edge in [0, 1_919, 3_200, 5_119] {
            assert!(samples[edge].unsigned_abs() < 200, "{edge}");
        }
    }

    #[test]
    fn what_is_asked_while_a_tone_plays_is_not_played_after_it() {
        let (updates, _told) = sync_channel(4);
        let (started_tx, started) = channel();
        let (release, released) = channel::<()>();
        let (ask, player) = start_tone(
            move |_| {
                started_tx.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            },
            updates,
        );
        ask.send(()).unwrap();
        started.recv().unwrap();
        for _ in 0..3 {
            ask.send(()).unwrap();
        }
        release.send(()).unwrap();
        drop(ask);
        player.join().unwrap();
        assert_eq!(started.try_iter().count(), 0);
    }

    #[test]
    fn a_tone_that_cannot_play_is_told_once_and_not_tried_again() {
        let (updates, told) = sync_channel(4);
        let (tried_tx, tried) = channel();
        let (ask, player) = start_tone(
            move |_| {
                tried_tx.send(()).unwrap();
                Err("no alert tone: aplay does not start (not found)".into())
            },
            updates,
        );
        ask.send(()).unwrap();
        ask.send(()).unwrap();
        drop(ask);
        player.join().unwrap();
        assert_eq!(tried.try_iter().count(), 1);
        let told: Vec<Update> = told.try_iter().collect();
        assert_eq!(
            told,
            [Update::Notice(
                "no alert tone: aplay does not start (not found)".into()
            )]
        );
    }
}
