//! What is kept on disk. Here the event database: a SQLite file with the
//! sessions of reception and the events the detector found in them.
//! Recordings of samples are in [`sigmf`].

pub mod sigmf;

use std::fmt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use sdr_core::SourceInfo;
use sdr_detect::Event;

/// The layout this build reads and writes, kept in SQLite's `user_version`.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    -- When reception began, milliseconds since 1970 UTC.
    started_unix_ms INTEGER NOT NULL,
    source TEXT NOT NULL,
    center_hz REAL NOT NULL,
    sample_rate INTEGER NOT NULL
);
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    -- The session's start plus the event's time in the stream.
    start_unix_ms INTEGER NOT NULL,
    stream_start_us INTEGER NOT NULL,
    duration_us INTEGER NOT NULL,
    low_hz REAL NOT NULL,
    high_hz REAL NOT NULL,
    peak_hz REAL NOT NULL,
    peak_dbfs REAL NOT NULL,
    above_floor_db REAL NOT NULL
);
CREATE INDEX events_by_start ON events(start_unix_ms);
";

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    /// The file was written by a build that knows a later layout.
    Newer {
        found: i64,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(err) => write!(f, "{err}"),
            StoreError::Newer { found } => write!(
                f,
                "made by a newer sdr-viewer (layout {found}, this build knows {SCHEMA_VERSION})"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        StoreError::Sqlite(err)
    }
}

/// One run of reception: the events of a session share its clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionId(i64);

/// An event as it lies in the database.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    pub session: SessionId,
    /// When it began by the wall clock.
    pub start: SystemTime,
    /// The event; its `start` is the time in the session's stream.
    pub event: Event,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it if it is not there.
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        let connection = Connection::open(path)?;
        // A write-ahead log with relaxed syncing: far fewer flushes to an
        // SD card, and a power cut loses the last moments, not the file.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Store::prepare(connection)
    }

    /// A database that lives only as long as the value: for tests.
    pub fn in_memory() -> Result<Store, StoreError> {
        Store::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Store, StoreError> {
        connection.pragma_update(None, "foreign_keys", true)?;
        let found: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match found {
            0 => {
                connection.execute_batch(&format!(
                    "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))?;
            }
            SCHEMA_VERSION => {}
            found => return Err(StoreError::Newer { found }),
        }
        Ok(Store { connection })
    }

    /// Notes that reception from `source` began at `started`.
    pub fn begin_session(
        &mut self,
        started: SystemTime,
        source: &SourceInfo,
    ) -> Result<SessionId, StoreError> {
        self.connection.execute(
            "INSERT INTO sessions (started_unix_ms, source, center_hz, sample_rate)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                unix_ms(started),
                source.description,
                source.center_hz,
                source.sample_rate
            ],
        )?;
        Ok(SessionId(self.connection.last_insert_rowid()))
    }

    /// Adds events of a session, all or none.
    pub fn add_events(&mut self, session: SessionId, events: &[Event]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO events (session_id, start_unix_ms, stream_start_us, duration_us,
                                     low_hz, high_hz, peak_hz, peak_dbfs, above_floor_db)
                 SELECT ?1, started_unix_ms + ?2 / 1000, ?2, ?3, ?4, ?5, ?6, ?7, ?8
                 FROM sessions WHERE id = ?1",
            )?;
            for event in events {
                insert.execute(params![
                    session.0,
                    micros(event.start),
                    micros(event.duration),
                    event.low_hz,
                    event.high_hz,
                    event.peak_hz,
                    f64::from(event.peak_dbfs),
                    f64::from(event.above_floor_db),
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// How many events there are in all.
    pub fn count(&self) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Deletes the events that began before `before`, and the sessions
    /// left without any; returns how many events went. The file does not
    /// shrink: SQLite fills the freed pages with what comes next.
    pub fn prune(&mut self, before: SystemTime) -> Result<u64, StoreError> {
        let transaction = self.connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM events WHERE start_unix_ms < ?1",
            [unix_ms(before)],
        )?;
        transaction.execute(
            "DELETE FROM sessions WHERE started_unix_ms < ?1
             AND NOT EXISTS (SELECT 1 FROM events WHERE session_id = sessions.id)",
            [unix_ms(before)],
        )?;
        transaction.commit()?;
        Ok(removed as u64)
    }

    /// The latest `limit` events of all sessions that began at `since` or
    /// later, the newest first.
    pub fn recent(&self, since: SystemTime, limit: usize) -> Result<Vec<StoredEvent>, StoreError> {
        let mut select = self.connection.prepare_cached(
            "SELECT session_id, start_unix_ms, stream_start_us, duration_us,
                    low_hz, high_hz, peak_hz, peak_dbfs, above_floor_db
             FROM events WHERE start_unix_ms >= ?1
             ORDER BY start_unix_ms DESC, id DESC LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = select.query_map(params![unix_ms(since), limit], |row| {
            Ok(StoredEvent {
                session: SessionId(row.get(0)?),
                start: UNIX_EPOCH + Duration::from_millis(unsigned(row.get(1)?)),
                event: Event {
                    start: Duration::from_micros(unsigned(row.get(2)?)),
                    duration: Duration::from_micros(unsigned(row.get(3)?)),
                    low_hz: row.get(4)?,
                    high_hz: row.get(5)?,
                    peak_hz: row.get(6)?,
                    peak_dbfs: row.get::<_, f64>(7)? as f32,
                    above_floor_db: row.get::<_, f64>(8)? as f32,
                },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn unix_ms(time: SystemTime) -> i64 {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}

/// SQLite's integers are signed; nothing we store is negative.
fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-12 12:00:00 UTC.
    const NOON_MS: u64 = 1_794_484_800_000;

    fn noon() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(NOON_MS)
    }

    fn source() -> SourceInfo {
        SourceInfo {
            sample_rate: 2_400_000,
            center_hz: 433.92e6,
            description: "rtl-sdr".into(),
        }
    }

    fn event(start_ms: u64, above: f32) -> Event {
        Event {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_micros(40_960),
            low_hz: 433.42e6,
            high_hz: 433.43e6,
            peak_hz: 433.425e6,
            peak_dbfs: -57.5,
            above_floor_db: above,
        }
    }

    /// A database file of its own in the system's temporary directory,
    /// removed with its side files when the test is over.
    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("sdr-store-test-{}-{name}.db", std::process::id()));
            let db = TempDb(path);
            db.remove();
            db
        }

        fn remove(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut path = self.0.clone().into_os_string();
                path.push(suffix);
                let _ = std::fs::remove_file(path);
            }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            self.remove();
        }
    }

    #[test]
    fn a_new_database_is_empty() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.recent(UNIX_EPOCH, 10).unwrap().is_empty());
    }

    #[test]
    fn events_come_back_as_they_went_in_with_their_wall_clock_time() {
        let mut store = Store::in_memory().unwrap();
        let session = store.begin_session(noon(), &source()).unwrap();
        let events = [event(1_500, 12.5), event(63_000, 30.0)];
        store.add_events(session, &events).unwrap();
        assert_eq!(store.count().unwrap(), 2);
        let got = store.recent(UNIX_EPOCH, 10).unwrap();
        assert_eq!(got.len(), 2);
        // The newest first.
        assert_eq!(got[0].event, events[1]);
        assert_eq!(got[0].start, noon() + Duration::from_secs(63));
        assert_eq!(got[0].session, session);
        assert_eq!(got[1].event, events[0]);
        assert_eq!(got[1].start, noon() + Duration::from_millis(1_500));
    }

    #[test]
    fn recent_spans_sessions_by_wall_clock_and_keeps_to_its_limit() {
        let mut store = Store::in_memory().unwrap();
        let morning = store
            .begin_session(noon() - Duration::from_secs(3600), &source())
            .unwrap();
        let midday = store.begin_session(noon(), &source()).unwrap();
        assert_ne!(morning, midday);
        // Late in the morning's stream is still before noon.
        store.add_events(midday, &[event(1_000, 20.0)]).unwrap();
        store
            .add_events(morning, &[event(5_000, 10.0), event(600_000, 11.0)])
            .unwrap();
        let got = store.recent(UNIX_EPOCH, 2).unwrap();
        let above: Vec<f32> = got
            .iter()
            .map(|stored| stored.event.above_floor_db)
            .collect();
        assert_eq!(above, [20.0, 11.0]);
        assert_eq!(got[1].session, morning);
    }

    #[test]
    fn recent_leaves_out_what_began_before_since() {
        let mut store = Store::in_memory().unwrap();
        let session = store.begin_session(noon(), &source()).unwrap();
        store
            .add_events(session, &[event(1_000, 10.0), event(2_000, 11.0)])
            .unwrap();
        let got = store.recent(noon() + Duration::from_secs(2), 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event.above_floor_db, 11.0);
    }

    #[test]
    fn pruning_deletes_old_events_and_the_sessions_left_empty() {
        let mut store = Store::in_memory().unwrap();
        let hour = Duration::from_secs(3600);
        let old = store.begin_session(noon() - 3 * hour, &source()).unwrap();
        let straddling = store.begin_session(noon() - hour, &source()).unwrap();
        store
            .add_events(old, &[event(0, 10.0), event(1_000, 11.0)])
            .unwrap();
        // One before noon, one after: 90 minutes into the session.
        store
            .add_events(straddling, &[event(60_000, 12.0), event(5_400_000, 13.0)])
            .unwrap();
        assert_eq!(store.prune(noon()).unwrap(), 3);
        let got = store.recent(UNIX_EPOCH, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event.above_floor_db, 13.0);
        assert_eq!(got[0].session, straddling);
        let sessions: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        // Nothing more to delete, and a session just begun is not empty
        // rubbish.
        let fresh = store.begin_session(noon(), &source()).unwrap();
        assert_eq!(store.prune(noon()).unwrap(), 0);
        store.add_events(fresh, &[event(0, 14.0)]).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn the_file_keeps_events_between_openings() {
        let db = TempDb::new("reopen");
        {
            let mut store = Store::open(&db.0).unwrap();
            let session = store.begin_session(noon(), &source()).unwrap();
            store.add_events(session, &[event(100, 15.0)]).unwrap();
        }
        let mut store = Store::open(&db.0).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let session = store.begin_session(noon(), &source()).unwrap();
        store.add_events(session, &[event(200, 16.0)]).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused_and_left_alone() {
        let db = TempDb::new("newer");
        {
            let connection = Connection::open(&db.0).unwrap();
            connection.pragma_update(None, "user_version", 7).unwrap();
        }
        let err = Store::open(&db.0).err().expect("must be refused");
        assert_eq!(
            err.to_string(),
            "made by a newer sdr-viewer (layout 7, this build knows 1)"
        );
        let connection = Connection::open(&db.0).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
    }

    #[test]
    fn a_path_that_cannot_be_a_database_is_an_error() {
        let missing = std::env::temp_dir().join("sdr-store-no-such-dir/events.db");
        assert!(Store::open(&missing).is_err());
    }

    #[test]
    fn events_of_an_unknown_session_are_not_stored() {
        let mut store = Store::in_memory().unwrap();
        store
            .add_events(SessionId(42), &[event(100, 15.0)])
            .unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }
}
