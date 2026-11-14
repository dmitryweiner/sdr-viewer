//! The channels file in the running app: read at the start, added to by
//! a thread of its own.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::thread::JoinHandle;

use sdr_detect::Channel;

use crate::reader::Update;
use crate::writer;

/// The channels of the file at `path`, none if there is no file yet.
pub fn load(path: &Path) -> Result<Vec<Channel>, String> {
    sdr_store::channels::load(path).map_err(|err| {
        format!(
            "the channels file {}: {err} (--no-channels runs without it)",
            path.display()
        )
    })
}

/// Starts the thread that adds the channels sent to it to the file at
/// `path`, until the sender is gone. A failure is told to the interface
/// once; after it nothing more is written.
pub fn save(path: PathBuf, updates: SyncSender<Update>) -> (Sender<Vec<Channel>>, JoinHandle<()>) {
    let (new, rx) = channel();
    let saver = std::thread::spawn(move || {
        let write = |new: &[Channel]| sdr_store::channels::add(&path, new).map(|_| ());
        if let Err(message) = writer::run(rx, write) {
            let _ = updates.send(Update::Notice(format!(
                "new channels are not saved: {message}"
            )));
        }
    });
    (new, saver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn made(center_hz: f64) -> Channel {
        Channel {
            name: None,
            center_hz,
            width_hz: 10e3,
            hidden: false,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdr-viewer-channels-app-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("channels.toml")
    }

    #[test]
    fn channels_sent_before_the_end_are_in_the_file_after_it() {
        let path = temp("saved");
        let (updates, notices) = sync_channel(4);
        let (new, saver) = save(path.clone(), updates);
        new.send(vec![made(433.5e6)]).unwrap();
        new.send(vec![made(433.7e6)]).unwrap();
        drop(new);
        saver.join().unwrap();
        assert_eq!(load(&path).unwrap(), [made(433.5e6), made(433.7e6)]);
        assert_eq!(notices.try_iter().count(), 0);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_added_to_is_told_once() {
        let path = temp("broken");
        std::fs::write(&path, "[[channel]\n").unwrap();
        let err = load(&path).unwrap_err();
        assert!(
            err.contains("channels.toml") && err.contains("--no-channels"),
            "{err}"
        );
        let (updates, notices) = sync_channel(4);
        let (new, saver) = save(path.clone(), updates);
        new.send(vec![made(433.5e6)]).unwrap();
        drop(new);
        saver.join().unwrap();
        let notices: Vec<Update> = notices.try_iter().collect();
        assert!(
            matches!(&notices[..], [Update::Notice(text)] if text.starts_with("new channels are not saved: ")),
            "{notices:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[channel]\n");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
