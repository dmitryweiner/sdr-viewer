//! The mesh in the database: when the app listened to `meshtasticd`'s
//! node, the packets it was given and what the node queued of its own.

use rusqlite::{Row, params};
use sdr_mesh::session;
use sdr_mesh::update::{Content, Packet, Payload, Sent};

use crate::{Store, StoreError};

/// The tables layout 2 adds. Times are Unix seconds.
pub(crate) const TABLES: &str = "
CREATE TABLE mesh_listened (
    id INTEGER PRIMARY KEY,
    -- The node's own number.
    node INTEGER NOT NULL,
    from_s INTEGER NOT NULL,
    -- Moved on while the app listens.
    until_s INTEGER NOT NULL
);
CREATE TABLE mesh_packets (
    id INTEGER PRIMARY KEY,
    -- When the node heard it, by its clock.
    received_s INTEGER NOT NULL,
    from_node INTEGER NOT NULL,
    to_node INTEGER NOT NULL,
    -- The channel's index, or its hash when the node has no key.
    channel INTEGER NOT NULL,
    packet_id INTEGER NOT NULL,
    snr REAL NOT NULL,
    rssi INTEGER NOT NULL,
    hops INTEGER,
    hop_start INTEGER NOT NULL,
    relay INTEGER NOT NULL,
    via_mqtt INTEGER NOT NULL,
    -- Made by the node for its clients alone, never on the air.
    app_only INTEGER NOT NULL,
    -- Its kind (Meshtastic's PortNum) and its Data message as the node
    -- decrypted it; both NULL when the node had no key.
    port INTEGER,
    data BLOB
);
CREATE INDEX mesh_packets_by_time ON mesh_packets(received_s);
CREATE TABLE mesh_sent (
    id INTEGER PRIMARY KEY,
    -- When the node told it had queued it, by the app's clock.
    at_s INTEGER NOT NULL,
    node INTEGER NOT NULL,
    packet_id INTEGER NOT NULL
);
CREATE INDEX mesh_sent_by_time ON mesh_sent(at_s);
";

/// One time the app listened to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenId(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listened {
    /// The node's own number.
    pub node: u32,
    pub from: u64,
    pub until: u64,
}

/// What the mesh kept of a time, the oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshHistory {
    pub listened: Vec<Listened>,
    pub packets: Vec<Packet>,
    /// What a node queued of its own, and which node it was.
    pub sent: Vec<(u32, Sent)>,
}

impl Store {
    /// Notes that the app began listening to the node `node` at `at`.
    pub fn begin_listening(&mut self, node: u32, at: u64) -> Result<ListenId, StoreError> {
        self.connection.execute(
            "INSERT INTO mesh_listened (node, from_s, until_s) VALUES (?1, ?2, ?2)",
            params![node, signed(at)],
        )?;
        Ok(ListenId(self.connection.last_insert_rowid()))
    }

    /// The app listened until `at`, or still does.
    pub fn listened_until(&mut self, listening: ListenId, at: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE mesh_listened SET until_s = MAX(until_s, ?2) WHERE id = ?1",
            params![listening.0, signed(at)],
        )?;
        Ok(())
    }

    /// Adds packets, all or none.
    pub fn add_packets(&mut self, packets: &[Packet]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO mesh_packets (received_s, from_node, to_node, channel, packet_id,
                     snr, rssi, hops, hop_start, relay, via_mqtt, app_only, port, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?;
            for packet in packets {
                let payload = packet.payload.as_ref();
                insert.execute(params![
                    signed(packet.received),
                    packet.from,
                    packet.to,
                    packet.channel,
                    packet.id,
                    f64::from(packet.snr),
                    packet.rssi,
                    packet.hops,
                    packet.hop_start,
                    packet.relay,
                    packet.via_mqtt,
                    packet.app_only,
                    payload.map(|payload| payload.port),
                    payload.map(|payload| &payload.data),
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Adds what nodes queued of their own, each with its node, all or
    /// none.
    pub fn add_sent(&mut self, sent: &[(u32, Sent)]) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO mesh_sent (at_s, node, packet_id) VALUES (?1, ?2, ?3)",
            )?;
            for (node, sent) in sent {
                insert.execute(params![signed(sent.at), node, sent.id])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// What was kept from `since` on: the times the app listened that
    /// ended then or later, the last `limit` packets and the last `limit`
    /// of the node's own, each the oldest first.
    pub fn mesh_since(&self, since: u64, limit: usize) -> Result<MeshHistory, StoreError> {
        let since = signed(since);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut select = self.connection.prepare_cached(
            "SELECT node, from_s, until_s FROM mesh_listened
             WHERE until_s >= ?1 ORDER BY from_s, id",
        )?;
        let listened = select
            .query_map([since], |row| {
                Ok(Listened {
                    node: row.get(0)?,
                    from: unsigned(row.get(1)?),
                    until: unsigned(row.get(2)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut select = self.connection.prepare_cached(
            "SELECT received_s, from_node, to_node, channel, packet_id, snr, rssi, hops,
                    hop_start, relay, via_mqtt, app_only, port, data
             FROM mesh_packets WHERE received_s >= ?1
             ORDER BY received_s DESC, id DESC LIMIT ?2",
        )?;
        let mut packets: Vec<Packet> = select
            .query_map(params![since, limit], packet)?
            .collect::<Result<_, _>>()?;
        packets.reverse();
        let mut select = self.connection.prepare_cached(
            "SELECT at_s, node, packet_id FROM mesh_sent WHERE at_s >= ?1
             ORDER BY at_s DESC, id DESC LIMIT ?2",
        )?;
        let mut sent: Vec<(u32, Sent)> = select
            .query_map(params![since, limit], |row| {
                let at = unsigned(row.get(0)?);
                Ok((
                    row.get(1)?,
                    Sent {
                        at,
                        id: row.get(2)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        sent.reverse();
        Ok(MeshHistory {
            listened,
            packets,
            sent,
        })
    }

    /// Deletes what the mesh told before `before`; returns how many
    /// packets went.
    pub fn prune_mesh(&mut self, before: u64) -> Result<u64, StoreError> {
        let before = signed(before);
        let transaction = self.connection.transaction()?;
        let removed =
            transaction.execute("DELETE FROM mesh_packets WHERE received_s < ?1", [before])?;
        transaction.execute("DELETE FROM mesh_sent WHERE at_s < ?1", [before])?;
        transaction.execute("DELETE FROM mesh_listened WHERE until_s < ?1", [before])?;
        transaction.commit()?;
        Ok(removed as u64)
    }
}

fn packet(row: &Row) -> rusqlite::Result<Packet> {
    let port: Option<i32> = row.get(12)?;
    let data: Option<Vec<u8>> = row.get(13)?;
    let payload = port.zip(data).map(|(port, data)| Payload { port, data });
    Ok(Packet {
        received: unsigned(row.get(0)?),
        from: row.get(1)?,
        to: row.get(2)?,
        channel: row.get(3)?,
        id: row.get(4)?,
        snr: row.get::<_, f64>(5)? as f32,
        rssi: row.get(6)?,
        hops: row.get(7)?,
        hop_start: row.get(8)?,
        relay: row.get(9)?,
        via_mqtt: row.get(10)?,
        app_only: row.get(11)?,
        content: payload.as_ref().map_or(Content::Encrypted, session::read),
        payload,
    })
}

fn signed(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

fn unsigned(seconds: i64) -> u64 {
    u64::try_from(seconds).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_mesh::update::BROADCAST;

    const ME: u32 = 0x015b_8900;
    const GATE: u32 = 0xbfb5_4553;

    /// A packet of `from` heard at `received`, carrying `data` on `port`
    /// as it came (any bytes: the content is what they read as).
    fn heard(from: u32, received: u64, payload: Option<Payload>) -> Packet {
        Packet {
            received,
            from,
            to: BROADCAST,
            channel: 1,
            id: 0x1234_5678,
            snr: -9.25,
            rssi: -115,
            hops: Some(2),
            hop_start: 7,
            relay: 0x6c,
            via_mqtt: false,
            app_only: false,
            content: payload.as_ref().map_or(Content::Encrypted, session::read),
            payload,
        }
    }

    /// A text: `Data { portnum: 1, payload: "hi" }` as protobuf.
    fn text() -> Payload {
        Payload {
            port: 1,
            data: vec![0x08, 0x01, 0x12, 0x02, b'h', b'i'],
        }
    }

    #[test]
    fn packets_come_back_as_they_went_in_with_what_they_carried() {
        let mut store = Store::in_memory().unwrap();
        let listening = store.begin_listening(ME, 36_000).unwrap();
        let mut sealed = heard(0x4339_6f6c, 36_010, None);
        (sealed.channel, sealed.hops, sealed.via_mqtt) = (0x18, None, true);
        let mut minute = heard(ME, 36_030, None);
        (minute.snr, minute.rssi, minute.relay, minute.app_only) = (0.0, 0, 0, true);
        let packets = [heard(GATE, 36_005, Some(text())), sealed, minute];
        assert_eq!(
            packets[0].content,
            Content::Other {
                port: 1,
                name: "text message".into()
            }
        );
        store.add_packets(&packets).unwrap();
        store
            .add_sent(&[(
                ME,
                Sent {
                    at: 36_020,
                    id: 0x77,
                },
            )])
            .unwrap();
        store.listened_until(listening, 36_060).unwrap();
        let history = store.mesh_since(0, 100).unwrap();
        assert_eq!(
            history.listened,
            [Listened {
                node: ME,
                from: 36_000,
                until: 36_060
            }]
        );
        assert_eq!(history.packets, packets);
        assert_eq!(
            history.sent,
            [(
                ME,
                Sent {
                    at: 36_020,
                    id: 0x77
                }
            )]
        );
    }

    #[test]
    fn what_was_kept_since_a_time_comes_the_oldest_first_the_newest_kept() {
        let mut store = Store::in_memory().unwrap();
        let morning = store.begin_listening(ME, 30_000).unwrap();
        store.listened_until(morning, 31_000).unwrap();
        // An end told late does not move it back.
        store.listened_until(morning, 30_500).unwrap();
        let noon = store.begin_listening(ME, 36_000).unwrap();
        store.listened_until(noon, 36_100).unwrap();
        let times = [30_100, 36_050, 30_900, 36_010];
        let packets: Vec<Packet> = times.iter().map(|&at| heard(GATE, at, None)).collect();
        store.add_packets(&packets).unwrap();
        let sent: Vec<(u32, Sent)> = [30_200, 36_020, 36_030]
            .map(|at| (ME, Sent { at, id: at as u32 }))
            .into();
        store.add_sent(&sent).unwrap();

        let history = store.mesh_since(31_000, 100).unwrap();
        let from: Vec<u64> = history.listened.iter().map(|l| l.from).collect();
        assert_eq!(from, [30_000, 36_000]);
        let received: Vec<u64> = history.packets.iter().map(|p| p.received).collect();
        assert_eq!(received, [36_010, 36_050]);
        let at: Vec<u64> = history.sent.iter().map(|(_, s)| s.at).collect();
        assert_eq!(at, [36_020, 36_030]);

        let history = store.mesh_since(0, 2).unwrap();
        let received: Vec<u64> = history.packets.iter().map(|p| p.received).collect();
        assert_eq!(received, [36_010, 36_050]);
        assert_eq!(history.sent.len(), 2);
    }

    #[test]
    fn pruning_the_mesh_deletes_what_is_older() {
        let mut store = Store::in_memory().unwrap();
        let old = store.begin_listening(ME, 1_000).unwrap();
        store.listened_until(old, 2_000).unwrap();
        let going_on = store.begin_listening(ME, 2_500).unwrap();
        store.listened_until(going_on, 4_000).unwrap();
        store
            .add_packets(&[heard(GATE, 1_500, None), heard(GATE, 3_500, None)])
            .unwrap();
        store
            .add_sent(&[
                (ME, Sent { at: 1_600, id: 1 }),
                (ME, Sent { at: 3_600, id: 2 }),
            ])
            .unwrap();
        assert_eq!(store.prune_mesh(3_000).unwrap(), 1);
        let history = store.mesh_since(0, 100).unwrap();
        assert_eq!(history.listened.len(), 1);
        assert_eq!(history.listened[0].from, 2_500);
        assert_eq!(history.packets.len(), 1);
        assert_eq!(history.sent, [(ME, Sent { at: 3_600, id: 2 })]);
        // The events are another matter.
        assert_eq!(store.count().unwrap(), 0);
    }
}
