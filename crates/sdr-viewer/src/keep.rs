//! The event database in the running app: opened at the start, written by
//! its own threads — one for the events, one for the mesh.

use std::path::Path;
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use sdr_core::SourceInfo;
use sdr_detect::Event as Detected;
use sdr_mesh::update::{MeshUpdate, Packet};
use sdr_store::mesh::{ListenId, MeshHistory};
use sdr_store::{Store, StoredEvent};

use crate::reader::{Kept as Told, Update};
use crate::writer;

/// Events older than this are deleted from the database when the app
/// starts (the plan's section 12), and so are the mesh's packets.
pub const KEEP_EVENTS: Duration = Duration::from_secs(7 * 24 * 3600);

/// The most packets of the last day the pulse is given at the start.
const MAX_HISTORY: usize = 100_000;

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
    let wrap = |err: &dyn std::fmt::Display| failed(path, err);
    let mut store = open(path)?;
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

/// What the interface hands the mesh's writer, in order.
#[derive(Debug, Clone)]
pub enum MeshKept {
    /// What the client told, and when.
    Told(u64, Box<MeshUpdate>),
    /// The user erased the nodes then: what was kept goes.
    EraseAll(u64),
}

impl MeshKept {
    pub fn told(at: u64, update: MeshUpdate) -> Self {
        MeshKept::Told(at, Box::new(update))
    }
}

/// The mesh in the database at work: where the interface sends what the
/// client told and when the nodes were erased, the thread that writes it,
/// what earlier runs kept of the last day, and the texts of the week before
/// it.
pub struct KeptMesh {
    pub told: Sender<Vec<MeshKept>>,
    pub writer: JoinHandle<()>,
    pub history: MeshHistory,
    pub texts: Vec<Packet>,
}

/// Opens the database at `path` for the mesh at `now` (Unix seconds), reads
/// the last day and the texts of the week before it, and starts the thread that deletes what is older than
/// `KEEP_EVENTS` and then writes what is told: when the app listened to
/// which node (from its `Ready` to its `Lost`, or to the last thing told
/// if the app was stopped short), the packets and the node's own queued.
/// A deletion or a write that fails is told to the interface once; after a
/// failed write, writing stops.
pub fn keep_mesh(path: &Path, now: u64, updates: SyncSender<Update>) -> Result<KeptMesh, String> {
    let mut store = open(path)?;
    let day_ago = now.saturating_sub(sdr_tui::mesh::HISTORY_S);
    let history = store
        .mesh_since(day_ago, MAX_HISTORY)
        .map_err(|err| failed(path, &err))?;
    let week_ago = now.saturating_sub(KEEP_EVENTS.as_secs());
    let texts = store
        .texts(week_ago, day_ago, sdr_tui::mesh::MAX_TEXTS)
        .map_err(|err| failed(path, &err))?;
    let (told, rx) = channel::<Vec<MeshKept>>();
    let writer = std::thread::spawn(move || {
        if let Err(err) = store.prune_mesh(week_ago) {
            let _ = updates.send(Update::Notice(format!(
                "old packets are not deleted: {err}"
            )));
        }
        let mut me = None;
        let mut listening: Option<ListenId> = None;
        let mut write = |told: &[MeshKept]| {
            let mut packets = Vec::new();
            let mut sent = Vec::new();
            let mut last = None;
            for kept in told {
                let (at, update) = match kept {
                    MeshKept::Told(at, update) => (at, update.as_ref()),
                    MeshKept::EraseAll(at) => {
                        // What came before it goes with what is kept; the
                        // app listens on from then.
                        packets.clear();
                        sent.clear();
                        store.erase_mesh(*at)?;
                        if let (Some(_), Some(me)) = (listening, me) {
                            listening = Some(store.begin_listening(me, *at)?);
                        }
                        last = Some(*at);
                        continue;
                    }
                };
                last = Some(*at);
                match update {
                    MeshUpdate::Ready(ready) => {
                        if let Some(before) = listening.take() {
                            store.listened_until(before, *at)?;
                        }
                        me = Some(ready.me);
                        listening = Some(store.begin_listening(ready.me, *at)?);
                    }
                    MeshUpdate::Lost(_) => {
                        if let Some(ended) = listening.take() {
                            store.listened_until(ended, *at)?;
                        }
                    }
                    MeshUpdate::Packet(packet) => packets.push(packet.clone()),
                    MeshUpdate::Sent(queued) => sent.extend(me.map(|me| (me, queued.clone()))),
                    MeshUpdate::Connecting(_) | MeshUpdate::Node(_) => {}
                }
            }
            store.add_packets(&packets)?;
            store.add_sent(&sent)?;
            match (listening, last) {
                (Some(going_on), Some(at)) => store.listened_until(going_on, at),
                _ => Ok(()),
            }
        };
        let write = |told: &[MeshKept]| write(told).map_err(|err| err.to_string());
        if let Err(message) = writer::run(rx, write) {
            let _ = updates.send(Update::Notice(format!("packets are not kept: {message}")));
        }
    });
    Ok(KeptMesh {
        told,
        writer,
        history,
        texts,
    })
}

/// Opens the database at `path`, making its directory.
fn open(path: &Path) -> Result<Store, String> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|err| failed(path, &err))?;
    }
    Store::open(path).map_err(|err| failed(path, &err))
}

fn failed(path: &Path, err: &dyn std::fmt::Display) -> String {
    format!(
        "the event database {}: {err} (--no-db runs without it)",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_mesh::update::{BROADCAST, Content, Packet, Payload, Ready, Sent};
    use sdr_store::mesh::Listened;
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

    const NOON_S: u64 = 1_794_484_800;
    const DAY_S: u64 = 86_400;
    const ME: u32 = 0x015b_8900;
    const GATE: u32 = 0xbfb5_4553;

    fn ready() -> MeshUpdate {
        MeshUpdate::Ready(Ready {
            me: ME,
            firmware: "2.7.26".into(),
            radio: None,
            channels: vec![(0, "MediumFast".into())],
        })
    }

    fn heard(received: u64) -> MeshUpdate {
        MeshUpdate::Packet(Packet {
            received,
            from: GATE,
            to: BROADCAST,
            channel: 0,
            id: 1,
            snr: -9.5,
            rssi: -115,
            hops: Some(2),
            hop_start: 5,
            relay: 0x6c,
            via_mqtt: false,
            app_only: false,
            content: Content::Encrypted,
            payload: None,
        })
    }

    /// A text heard at `received`.
    fn text(received: u64) -> MeshUpdate {
        let MeshUpdate::Packet(mut packet) = heard(received) else {
            unreachable!()
        };
        packet.payload = Some(Payload {
            port: 1,
            data: vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'],
        });
        packet.content = Content::Text("hi".into());
        MeshUpdate::Packet(packet)
    }

    /// One run of the app: `told` sent to its writer, which is then
    /// waited for; what it read at its start.
    fn run(path: &Path, now: u64, told: Vec<Vec<(u64, MeshUpdate)>>) -> MeshHistory {
        run_for_texts(path, now, told).0
    }

    /// The same, with the texts of the week before the day it read.
    fn run_for_texts(
        path: &Path,
        now: u64,
        told: Vec<Vec<(u64, MeshUpdate)>>,
    ) -> (MeshHistory, Vec<Packet>) {
        let (updates, notices) = sync_channel(8);
        let kept = keep_mesh(path, now, updates).unwrap();
        for batch in told {
            let batch = batch
                .into_iter()
                .map(|(at, update)| MeshKept::told(at, update));
            kept.told.send(batch.collect()).unwrap();
        }
        drop(kept.told);
        kept.writer.join().unwrap();
        assert!(notices.try_recv().is_err());
        (kept.history, kept.texts)
    }

    #[test]
    fn erasing_deletes_what_was_kept_and_the_next_run_knows_when() {
        let dir = TempDir::new("mesh-erase");
        let path = dir.0.join("events.db");
        let at = NOON_S - 3600;
        let (updates, notices) = sync_channel(8);
        let kept = keep_mesh(&path, at, updates).unwrap();
        // Erased amid a batch: what came before goes, what came after stays.
        let told = vec![
            MeshKept::told(at, ready()),
            MeshKept::told(at + 10, heard(at + 9)),
            MeshKept::told(at + 20, MeshUpdate::Sent(Sent { at: at + 20, id: 7 })),
            MeshKept::EraseAll(at + 30),
            MeshKept::told(at + 40, heard(at + 39)),
        ];
        kept.told.send(told).unwrap();
        drop(kept.told);
        kept.writer.join().unwrap();
        assert!(notices.try_recv().is_err());
        let history = run(&path, NOON_S, Vec::new());
        assert_eq!(history.erased, Some(at + 30));
        let received: Vec<u64> = history.packets.iter().map(|p| p.received).collect();
        assert_eq!(received, [at + 39]);
        assert!(history.sent.is_empty());
        // It goes on listening from then.
        assert_eq!(
            history.listened,
            [Listened {
                node: ME,
                from: at + 30,
                until: at + 40
            }]
        );
    }

    #[test]
    fn the_next_run_reads_the_texts_of_the_week_before_its_day_too() {
        let dir = TempDir::new("mesh-texts");
        let path = dir.0.join("events.db");
        for ago in [8 * DAY_S, 3 * DAY_S, 2 * 3600] {
            let at = NOON_S - ago;
            let told = vec![vec![
                (at, ready()),
                (at + 1, text(at)),
                (at + 2, heard(at + 2)),
            ]];
            run(&path, at, told);
        }
        let (history, texts) = run_for_texts(&path, NOON_S, Vec::new());
        let received: Vec<u64> = texts.iter().map(|text| text.received).collect();
        assert_eq!(received, [NOON_S - 3 * DAY_S]);
        assert_eq!(texts[0].content, Content::Text("hi".into()));
        // The day's are among the day's packets.
        assert_eq!(history.packets.len(), 2);
    }

    #[test]
    fn the_mesh_is_kept_and_the_next_run_reads_its_last_day() {
        let dir = TempDir::new("mesh");
        let path = dir.0.join("data/events.db");
        // Eight days ago, two days ago, an hour ago.
        for ago in [8 * DAY_S, 2 * DAY_S] {
            let at = NOON_S - ago;
            let history = run(
                &path,
                at,
                vec![vec![(at, ready()), (at + 5, heard(at + 4))]],
            );
            assert!(history.packets.is_empty());
        }
        let at = NOON_S - 3600;
        let quit = MeshUpdate::Lost("the app quit".into());
        let told = vec![
            vec![(at, ready()), (at + 10, heard(at + 9))],
            vec![(at + 20, MeshUpdate::Sent(Sent { at: at + 20, id: 7 }))],
            vec![(at + 600, quit)],
        ];
        assert!(run(&path, at, told).packets.is_empty());

        let history = run(&path, NOON_S, Vec::new());
        assert_eq!(
            history.listened,
            [Listened {
                node: ME,
                from: at,
                until: at + 600
            }]
        );
        let received: Vec<u64> = history.packets.iter().map(|p| p.received).collect();
        assert_eq!(received, [at + 9]);
        assert_eq!(history.sent, [(ME, Sent { at: at + 20, id: 7 })]);
        // What is older than a week is gone; two days ago is kept.
        let store = Store::open(&path).unwrap();
        let all = store.mesh_since(0, 10).unwrap();
        let received: Vec<u64> = all.packets.iter().map(|p| p.received).collect();
        assert_eq!(received, [NOON_S - 2 * DAY_S + 4, at + 9]);
        assert_eq!(all.listened.len(), 2);
    }

    #[test]
    fn listening_ends_with_the_last_told_when_the_end_is_not() {
        let dir = TempDir::new("mesh-cut");
        let path = dir.0.join("events.db");
        let at = NOON_S - 3600;
        // Connected, a packet, lost and connected again; then the app was
        // killed.
        let told = vec![
            vec![(at, ready()), (at + 50, heard(at + 49))],
            vec![
                (at + 70, MeshUpdate::Lost("closed".into())),
                (at + 80, ready()),
            ],
            vec![(at + 140, heard(at + 139))],
        ];
        run(&path, at, told);
        let history = run(&path, NOON_S, Vec::new());
        let spans: Vec<(u64, u64)> = history
            .listened
            .iter()
            .map(|listened| (listened.from - at, listened.until - at))
            .collect();
        assert_eq!(spans, [(0, 70), (80, 140)]);
    }
}
