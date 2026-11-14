//! The event database in the running app: opened at the start, written by
//! its own thread.

use std::path::Path;
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use sdr_core::SourceInfo;
use sdr_detect::Event as Detected;
use sdr_store::{Store, StoredEvent};

use crate::reader::{Kept as Told, Update};
use crate::writer;

/// Events older than this are deleted from the database when the app
/// starts (the plan's section 12).
pub const KEEP_EVENTS: Duration = Duration::from_secs(7 * 24 * 3600);

/// The event database at work: where the reader sends events, the thread
/// that writes them, and what earlier runs left for the log.
pub struct Kept {
    pub events: Sender<Vec<Told>>,
    pub writer: JoinHandle<()>,
    pub history: Vec<StoredEvent>,
}

/// Opens the event database at `path`, notes a session for `source`
/// that began at `started`, and starts the thread that deletes what is
/// older than `KEEP_EVENTS` and then writes what is sent to it. A deletion
/// or a write that fails is told to the interface once; after a failed
/// write, writing stops.
pub fn keep_events(
    path: &Path,
    source: &SourceInfo,
    started: SystemTime,
    updates: SyncSender<Update>,
) -> Result<Kept, String> {
    let wrap = |err: &dyn std::fmt::Display| {
        format!(
            "the event database {}: {err} (--no-db runs without it)",
            path.display()
        )
    };
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|err| wrap(&err))?;
    }
    let mut store = Store::open(path).map_err(|err| wrap(&err))?;
    let oldest = started - KEEP_EVENTS;
    let history = store
        .recent(oldest, sdr_tui::log::MAX_EVENTS)
        .map_err(|err| wrap(&err))?;
    let session = store
        .begin_session(started, source)
        .map_err(|err| wrap(&err))?;
    let (events, rx) = channel();
    let writer = std::thread::spawn(move || {
        // A week of a busy band is millions of rows: not before the
        // interface is up.
        if let Err(err) = store.prune(oldest) {
            let _ = updates.send(Update::Notice(format!("old events are not deleted: {err}")));
        }
        // In the order told: what ended before the log was erased goes
        // with it, what ended after stays.
        let write = |told: &[Told]| {
            let mut events: Vec<Detected> = Vec::new();
            for told in told {
                match told {
                    Told::Event(event) => events.push(event.clone()),
                    Told::EraseAll => {
                        events.clear();
                        store.erase(session).map_err(|err| err.to_string())?;
                    }
                }
            }
            let added = store.add_events(session, &events);
            added.map_err(|err| err.to_string())
        };
        if let Err(message) = writer::run(rx, write) {
            let _ = updates.send(Update::Notice(format!("events are not kept: {message}")));
        }
    });
    Ok(Kept {
        events,
        writer,
        history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::time::UNIX_EPOCH;

    /// 2026-11-12 12:00:00 UTC.
    fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_794_484_800)
    }

    const DAY: Duration = Duration::from_secs(86_400);

    fn source() -> SourceInfo {
        SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "test".into(),
        }
    }

    fn event(start_ms: u64, above: f32) -> Detected {
        Detected {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_millis(40),
            low_hz: 433.42e6,
            high_hz: 433.43e6,
            peak_hz: 433.425e6,
            peak_dbfs: -57.5,
            above_floor_db: above,
        }
    }

    /// A directory of its own in the system's temporary one, removed with
    /// what is in it when the test is over.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("sdr-viewer-keep-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_run_reads_the_week_s_history_deletes_what_is_older_and_adds_its_own() {
        let dir = TempDir::new("week");
        // The directory is made on the way.
        let path = dir.0.join("data/events.db");
        let (updates, notices) = sync_channel(8);

        // Ten days ago and yesterday.
        for (ago, above) in [(10, 11.0), (1, 12.0)] {
            let kept = keep_events(&path, &source(), noon() - ago * DAY, updates.clone()).unwrap();
            kept.events
                .send(vec![Told::Event(event(1_000, above))])
                .unwrap();
            drop(kept.events);
            kept.writer.join().unwrap();
        }

        let kept = keep_events(&path, &source(), noon(), updates.clone()).unwrap();
        let history: Vec<f32> = kept
            .history
            .iter()
            .map(|stored| stored.event.above_floor_db)
            .collect();
        assert_eq!(history, [12.0]);
        assert_eq!(kept.history[0].start, noon() - DAY + Duration::from_secs(1));
        kept.events
            .send(vec![Told::Event(event(2_000, 13.0))])
            .unwrap();
        drop(kept.events);
        kept.writer.join().unwrap();

        let store = Store::open(&path).unwrap();
        let all: Vec<f32> = store
            .recent(UNIX_EPOCH, 10)
            .unwrap()
            .iter()
            .map(|stored| stored.event.above_floor_db)
            .collect();
        assert_eq!(all, [13.0, 12.0]);
        assert!(notices.try_recv().is_err());
    }

    #[test]
    fn erasing_deletes_what_was_kept_before_it_and_keeps_what_comes_after() {
        let dir = TempDir::new("erase");
        let path = dir.0.join("events.db");
        let (updates, notices) = sync_channel(8);
        let kept = keep_events(&path, &source(), noon() - DAY, updates.clone()).unwrap();
        kept.events
            .send(vec![Told::Event(event(1_000, 11.0))])
            .unwrap();
        drop(kept.events);
        kept.writer.join().unwrap();

        let kept = keep_events(&path, &source(), noon(), updates.clone()).unwrap();
        assert_eq!(kept.history.len(), 1);
        let told = vec![
            Told::Event(event(1_000, 12.0)),
            Told::EraseAll,
            Told::Event(event(2_000, 13.0)),
        ];
        kept.events.send(told).unwrap();
        drop(kept.events);
        kept.writer.join().unwrap();

        let store = Store::open(&path).unwrap();
        let all = store.recent(UNIX_EPOCH, 10).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].event.above_floor_db, 13.0);
        assert_eq!(all[0].start, noon() + Duration::from_secs(2));
        assert!(notices.try_recv().is_err());
    }

    #[test]
    fn a_database_that_cannot_be_opened_says_where_and_how_to_go_on() {
        let dir = TempDir::new("blocked");
        std::fs::create_dir_all(&dir.0).unwrap();
        // A file where the directory should be.
        std::fs::write(dir.0.join("data"), b"").unwrap();
        let path = dir.0.join("data/events.db");
        let (updates, _notices) = sync_channel(8);
        let message = keep_events(&path, &source(), noon(), updates)
            .err()
            .expect("must fail");
        assert!(message.starts_with("the event database "), "{message}");
        assert!(message.contains("data/events.db"), "{message}");
        assert!(message.ends_with("(--no-db runs without it)"), "{message}");
    }
}
