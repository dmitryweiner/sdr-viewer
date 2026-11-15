//! The Meshtastic pulse's state: the nodes `meshtasticd` knows and the
//! packets heard from each in this run, on a time axis of the wall clock
//! (Unix seconds), the selection and the time scale. No drawing here.

use std::collections::VecDeque;

use sdr_mesh::update::{Content, MeshUpdate, Node, NodeUser, Packet, Ready};

/// Seconds to a column of the pulse: a node speaks every few minutes.
pub const SCALES: [u64; 4] = [60, 300, 900, 3600];

/// How far back each node's packets are kept.
pub const HISTORY_S: u64 = 24 * 3600;

/// And at most so many of them.
const MAX_TICKS: usize = 10_000;

/// The connection to `meshtasticd`.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    /// Not asked for: the app was started without `--mesh`.
    Off,
    Connecting(String),
    Connected,
    /// Gone, and why; the client tries again.
    Lost(String),
}

/// One node of the pulse.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    pub num: u32,
    pub user: Option<NodeUser>,
    /// When it was last heard, by the node's database or a packet since.
    pub last_heard: Option<u64>,
    /// Of the last packet heard from it.
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hops: Option<u32>,
    /// The last packet heard from it in this run.
    pub last: Option<Packet>,
    /// When its packets of this run arrived, oldest first.
    times: VecDeque<u64>,
    /// How many packets this run has heard from it.
    pub packets: u64,
}

impl NodeRow {
    fn new(num: u32) -> Self {
        NodeRow {
            num,
            user: None,
            last_heard: None,
            snr: None,
            rssi: None,
            hops: None,
            last: None,
            times: VecDeque::new(),
            packets: 0,
        }
    }

    /// "!bfb54553" when it has not told its own id.
    pub fn id(&self) -> String {
        self.user
            .as_ref()
            .map_or_else(|| format!("!{:08x}", self.num), |user| user.id.clone())
    }
}

#[derive(Debug, Clone)]
pub struct MeshView {
    pub link: Link,
    /// What the node told when it was last connected.
    pub ready: Option<Ready>,
    nodes: Vec<NodeRow>,
    selected: Option<u32>,
    scale: usize,
    now: u64,
}

impl MeshView {
    /// A view that will hear from `meshtasticd`, or not (`Link::Off`).
    pub fn new(link: Link) -> Self {
        MeshView {
            link,
            ready: None,
            nodes: Vec::new(),
            selected: None,
            scale: 0,
            now: 0,
        }
    }

    pub fn on_update(&mut self, update: MeshUpdate) {
        match update {
            MeshUpdate::Connecting(addr) => self.link = Link::Connecting(addr),
            MeshUpdate::Ready(ready) => {
                self.link = Link::Connected;
                self.ready = Some(ready);
            }
            MeshUpdate::Lost(why) => self.link = Link::Lost(why),
            MeshUpdate::Node(node) => self.on_node(node),
            MeshUpdate::Packet(packet) => self.on_packet(packet),
        }
    }

    /// A node of the database; what this run has heard of it stays.
    fn on_node(&mut self, node: Node) {
        let row = self.row_mut(node.num);
        if node.user.is_some() {
            row.user = node.user;
        }
        row.last_heard = row.last_heard.max(node.last_heard);
        if row.packets == 0 {
            row.snr = node.snr;
            row.hops = node.hops_away;
        }
    }

    fn on_packet(&mut self, packet: Packet) {
        let row = self.row_mut(packet.from);
        if let Content::User(user) = &packet.content {
            row.user = Some(user.clone());
        }
        row.last_heard = row.last_heard.max(Some(packet.received));
        // The node's own packets are not received: nothing is measured.
        if packet.snr != 0.0 || packet.rssi != 0 {
            row.snr = Some(packet.snr);
            row.rssi = Some(packet.rssi);
        }
        if packet.hops.is_some() {
            row.hops = packet.hops;
        }
        row.times.push_back(packet.received);
        while row.times.len() > MAX_TICKS
            || row
                .times
                .front()
                .is_some_and(|&t| t + HISTORY_S < packet.received)
        {
            row.times.pop_front();
        }
        row.packets += 1;
        row.last = Some(packet);
    }

    fn row_mut(&mut self, num: u32) -> &mut NodeRow {
        let at = match self.nodes.iter().position(|node| node.num == num) {
            Some(at) => at,
            None => {
                self.nodes.push(NodeRow::new(num));
                self.nodes.len() - 1
            }
        };
        &mut self.nodes[at]
    }

    /// The wall clock, Unix seconds: where the pulse's right edge is.
    pub fn set_now(&mut self, now: u64) {
        self.now = now;
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    /// The node itself first, then the others, the last heard first.
    pub fn rows(&self) -> Vec<&NodeRow> {
        let me = self.ready.as_ref().map(|ready| ready.me);
        let mut rows: Vec<&NodeRow> = self.nodes.iter().collect();
        rows.sort_by_key(|row| {
            let own = Some(row.num) == me;
            (!own, std::cmp::Reverse(row.last_heard), row.num)
        });
        rows
    }

    /// Packets of `row` per column, `columns` of them up to now, the
    /// oldest first.
    pub fn cells(&self, row: &NodeRow, columns: usize) -> Vec<u32> {
        let scale = self.seconds_per_column();
        let mut cells = vec![0; columns];
        for &time in row.times.iter().filter(|&&time| time <= self.now) {
            // The last column holds what is less than `scale` old.
            let back = usize::try_from((self.now - time) / scale).unwrap_or(usize::MAX);
            if let Some(cell) = columns
                .checked_sub(back.saturating_add(1))
                .and_then(|c| cells.get_mut(c))
            {
                *cell += 1;
            }
        }
        cells
    }

    /// How many nodes this run has heard a packet from.
    pub fn heard(&self) -> usize {
        self.nodes.iter().filter(|node| node.packets > 0).count()
    }

    pub fn seconds_per_column(&self) -> u64 {
        SCALES[self.scale]
    }

    pub fn zoom_in(&mut self) {
        self.scale = self.scale.saturating_sub(1);
    }

    pub fn zoom_out(&mut self) {
        self.scale = (self.scale + 1).min(SCALES.len() - 1);
    }

    /// The node that relayed a packet, told by the last byte of its
    /// number: only when one known node alone ends in it.
    pub fn relayed_by(&self, relay: u8) -> Option<&NodeRow> {
        if relay == 0 {
            return None;
        }
        let mut ending = self
            .nodes
            .iter()
            .filter(|node| node.num & 0xff == u32::from(relay));
        let first = ending.next()?;
        ending.next().is_none().then_some(first)
    }

    /// The name of the channel a packet came on: the node's own, or its
    /// hash when the node has no key for it.
    pub fn channel_name(&self, packet: &Packet) -> String {
        if packet.content == Content::Encrypted {
            return format!("hash 0x{:02x}, no key", packet.channel);
        }
        let channels = self
            .ready
            .as_ref()
            .map_or(&[][..], |ready| &ready.channels[..]);
        channels
            .iter()
            .find(|(index, _)| *index == packet.channel)
            .map_or_else(
                || format!("channel {}", packet.channel),
                |(_, name)| name.clone(),
            )
    }

    pub fn selected(&self) -> Option<&NodeRow> {
        let selected = self.selected?;
        self.nodes.iter().find(|node| node.num == selected)
    }

    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected?;
        self.rows().iter().position(|row| row.num == selected)
    }

    /// Moves the selection by rows; it appears on the first row.
    pub fn move_selection(&mut self, by: isize) {
        let rows = self.rows();
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let row = match self.selected_row() {
            Some(row) => row.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.selected = Some(rows[row].num);
    }

    pub fn release(&mut self) {
        self.selected = None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sdr_mesh::lora::Radio;
    use sdr_mesh::update::BROADCAST;

    pub(crate) const ME: u32 = 0x015b_8900;
    pub(crate) const GATE: u32 = 0xbfb5_4553;
    pub(crate) const FAR: u32 = 0x4339_6f6c;

    pub(crate) fn who(num: u32, short: &str, long: &str) -> NodeUser {
        NodeUser {
            id: format!("!{num:08x}"),
            long_name: long.into(),
            short_name: short.into(),
            hw_model: "HELTEC_V3".into(),
            role: "CLIENT".into(),
        }
    }

    pub(crate) fn ready() -> Ready {
        Ready {
            me: ME,
            firmware: "2.7.26".into(),
            radio: Some(Radio {
                region: "US".into(),
                preset: "MediumFast".into(),
                slot: Some(70),
                frequency_mhz: Some(919.375),
            }),
            channels: vec![(0, "MediumFast".into()), (1, "Haifa".into())],
        }
    }

    pub(crate) fn packet(from: u32, received: u64, content: Content) -> Packet {
        Packet {
            received,
            from,
            to: BROADCAST,
            channel: 0,
            id: 1,
            snr: -9.5,
            rssi: -115,
            hops: Some(2),
            hop_start: 5,
            relay: 0x6c,
            via_mqtt: false,
            content,
        }
    }

    pub(crate) fn telemetry() -> Content {
        Content::Other {
            port: 67,
            name: "telemetry".into(),
        }
    }

    /// The device's node, a gateway heard twice and a far node once,
    /// at 10:00:00 on the wall clock.
    pub(crate) fn view() -> MeshView {
        let mut view = MeshView::new(Link::Connecting("localhost:4403".into()));
        for update in [
            MeshUpdate::Node(Node {
                num: ME,
                user: Some(who(ME, "8900", "Meshtastic 8900")),
                last_heard: Some(36_000),
                snr: None,
                hops_away: None,
                via_mqtt: false,
            }),
            // Heard of before the run, without a name.
            MeshUpdate::Node(Node {
                num: GATE,
                user: None,
                last_heard: Some(30_000),
                snr: Some(-13.25),
                hops_away: Some(3),
                via_mqtt: false,
            }),
            MeshUpdate::Ready(ready()),
            MeshUpdate::Packet(packet(
                GATE,
                35_000,
                Content::User(who(GATE, "HG1", "Haifa Gate")),
            )),
            MeshUpdate::Packet(packet(GATE, 35_950, telemetry())),
            MeshUpdate::Packet(packet(FAR, 33_000, Content::Encrypted)),
        ] {
            view.on_update(update);
        }
        view.set_now(36_000);
        view
    }

    #[test]
    fn the_link_follows_the_client() {
        let mut view = MeshView::new(Link::Off);
        view.on_update(MeshUpdate::Connecting("node:4403".into()));
        assert_eq!(view.link, Link::Connecting("node:4403".into()));
        view.on_update(MeshUpdate::Ready(ready()));
        assert_eq!(view.link, Link::Connected);
        view.on_update(MeshUpdate::Lost("refused".into()));
        assert_eq!(view.link, Link::Lost("refused".into()));
        // What the node told stays while it is away.
        assert_eq!(view.ready, Some(ready()));
    }

    #[test]
    fn nodes_are_the_databases_and_those_heard_since() {
        let view = view();
        let rows = view.rows();
        let nums: Vec<u32> = rows.iter().map(|row| row.num).collect();
        assert_eq!(nums, [ME, GATE, FAR]);
        let gate = rows[1];
        // Its node info named it; its last packet tells the rest.
        assert_eq!(gate.user.as_ref().unwrap().short_name, "HG1");
        assert_eq!((gate.last_heard, gate.packets), (Some(35_950), 2));
        assert_eq!(
            (gate.snr, gate.rssi, gate.hops),
            (Some(-9.5), Some(-115), Some(2))
        );
        assert_eq!(gate.last.as_ref().unwrap().content, telemetry());
        assert_eq!(rows[2].id(), "!43396f6c");
        assert_eq!(view.heard(), 2);
    }

    #[test]
    fn a_new_connections_database_does_not_undo_what_was_heard() {
        let mut view = view();
        view.on_update(MeshUpdate::Node(Node {
            num: GATE,
            user: None,
            last_heard: Some(30_000),
            snr: Some(-13.25),
            hops_away: Some(3),
            via_mqtt: false,
        }));
        let gate = view.rows()[1];
        assert_eq!(gate.user.as_ref().unwrap().short_name, "HG1");
        assert_eq!(
            (gate.last_heard, gate.snr, gate.hops),
            (Some(35_950), Some(-9.5), Some(2))
        );
    }

    #[test]
    fn a_nodes_own_packets_measure_nothing() {
        let mut view = view();
        let mut own = packet(ME, 35_990, telemetry());
        (own.snr, own.rssi, own.hops) = (0.0, 0, None);
        view.on_update(MeshUpdate::Packet(own));
        let me = view.rows()[0];
        assert_eq!((me.snr, me.rssi, me.hops), (None, None, None));
        assert_eq!(me.packets, 1);
    }

    #[test]
    fn cells_count_the_packets_of_each_column_up_to_now() {
        let mut view = view();
        let gate = view.rows()[1].clone();
        // A minute a column: 35 000 is 16 minutes and 40 s ago.
        let cells = view.cells(&gate, 20);
        assert_eq!(cells.len(), 20);
        assert_eq!(cells[19], 1);
        assert_eq!(cells[3], 1);
        assert_eq!(cells.iter().sum::<u32>(), 2);
        view.zoom_out();
        assert_eq!(view.seconds_per_column(), 300);
        assert_eq!(view.cells(&gate, 20)[16..], [1, 0, 0, 1]);
        for _ in 0..5 {
            view.zoom_out();
        }
        assert_eq!(view.seconds_per_column(), 3600);
        assert_eq!(view.cells(&gate, 3), [0, 0, 2]);
        for _ in 0..5 {
            view.zoom_in();
        }
        assert_eq!(view.seconds_per_column(), 60);
        // A packet a whole column old is in the column before the last.
        view.on_update(MeshUpdate::Packet(packet(GATE, 35_940, telemetry())));
        let gate = view.rows()[1].clone();
        assert_eq!(view.cells(&gate, 20)[18..], [1, 1]);
    }

    #[test]
    fn a_day_of_packets_is_kept() {
        let mut view = view();
        // A day after the first packet, which goes.
        view.on_update(MeshUpdate::Packet(packet(
            GATE,
            35_001 + HISTORY_S,
            telemetry(),
        )));
        let gate = view.rows()[1].clone();
        assert_eq!(gate.times, [35_950, 35_001 + HISTORY_S]);
        assert_eq!(gate.packets, 3);
    }

    #[test]
    fn a_relay_is_named_only_when_one_node_ends_in_its_byte() {
        let mut view = view();
        assert_eq!(view.relayed_by(0x53).map(|row| row.num), Some(GATE));
        assert!(view.relayed_by(0x6c).is_some());
        assert!(view.relayed_by(0x00).is_none());
        view.on_update(MeshUpdate::Packet(packet(0x1111_1153, 35_999, telemetry())));
        assert!(view.relayed_by(0x53).is_none());
    }

    #[test]
    fn a_channel_is_named_by_its_index_or_shown_by_its_hash() {
        let view = view();
        let mut on = packet(GATE, 1, telemetry());
        assert_eq!(view.channel_name(&on), "MediumFast");
        on.channel = 1;
        assert_eq!(view.channel_name(&on), "Haifa");
        on.channel = 5;
        assert_eq!(view.channel_name(&on), "channel 5");
        let mut sealed = packet(FAR, 1, Content::Encrypted);
        sealed.channel = 0x18;
        assert_eq!(view.channel_name(&sealed), "hash 0x18, no key");
    }

    #[test]
    fn the_selection_is_a_node_and_follows_it() {
        let mut view = view();
        view.move_selection(1);
        assert_eq!(view.selected().map(|row| row.num), Some(ME));
        view.move_selection(1);
        assert_eq!(view.selected_row(), Some(1));
        view.move_selection(isize::MAX);
        assert_eq!(view.selected().map(|row| row.num), Some(FAR));
        // FAR is heard now: it moves up, the selection with it.
        view.on_update(MeshUpdate::Packet(packet(FAR, 35_999, telemetry())));
        assert_eq!(view.selected_row(), Some(1));
        view.move_selection(isize::MIN);
        assert_eq!(view.selected_row(), Some(0));
        view.release();
        assert_eq!(view.selected(), None);
    }
}
