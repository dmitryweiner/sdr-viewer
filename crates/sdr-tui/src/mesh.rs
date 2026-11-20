//! The Meshtastic pulse's state: the nodes `meshtasticd` knows and the
//! packets heard from each in this run (and what the node handed this app
//! alone), on a time axis of the wall clock
//! (Unix seconds) whose columns begin on whole minutes (or whatever a
//! column stands for), so that the whole pulse steps at once; when the app
//! was listening; the order, what is shown, the selection and the time
//! scale. No drawing here.

use std::collections::VecDeque;

use sdr_mesh::update::{Content, MeshUpdate, Node, NodeUser, Packet, Ready};

/// Seconds to a column of the pulse.
pub const SCALES: [u64; 6] = [10, 30, 60, 300, 900, 3600];

/// The scale the pulse opens with: a node speaks every few minutes.
const FIRST_SCALE: usize = 2;

/// A node is active when it was heard in the last so many seconds: a
/// node that is on tells its position every 15 minutes, by default.
pub const ACTIVE_S: u64 = 15 * 60;

/// How far back each node's packets are kept.
pub const HISTORY_S: u64 = 24 * 3600;

/// And at most so many of them.
const MAX_TICKS: usize = 10_000;

/// The clock's marks on the axis are this far apart, the first of these
/// that is a whole number of columns, and enough of them.
const MARK_STEPS: [u64; 12] = [
    60, 120, 300, 600, 900, 1800, 3600, 7200, 10_800, 21_600, 43_200, 86_400,
];
const MARK_COLUMNS: u64 = 12;

/// The connection to `meshtasticd`.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    /// Not asked for: the app was started without `--mesh`.
    Off,
    Connecting(String),
    Connected,
    /// Gone, or never there, and why; the client tries again, and the link
    /// stays lost until one of its tries succeeds.
    Lost(String),
}

/// The order of the nodes after the node itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// The last heard first.
    #[default]
    Heard,
    /// The fewest hops first, the unknown last.
    Hops,
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
    /// Its last packet in this run, heard or for this app only.
    pub last: Option<Packet>,
    /// When its packets of this run arrived, oldest first.
    times: VecDeque<u64>,
    /// How many packets this run has heard from it.
    pub packets: u64,
    /// When the node handed this app what it made for its clients alone
    /// (`Packet::app_only`): no packet heard, never on the air.
    app_only_times: VecDeque<u64>,
    /// How many of those.
    pub app_only: u64,
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
            app_only_times: VecDeque::new(),
            app_only: 0,
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
    /// The command that starts `meshtasticd`, when it is this machine's:
    /// told while it does not answer.
    pub starter: Option<String>,
    nodes: Vec<NodeRow>,
    /// When the app was connected: from each `Ready` to the next loss.
    listened: Vec<(u64, Option<u64>)>,
    order: Order,
    active_only: bool,
    /// The list was started afresh then: nothing before it is shown.
    since: Option<u64>,
    selected: Option<u32>,
    scale: usize,
    now: u64,
    utc_offset_s: i32,
}

impl MeshView {
    /// A view that will hear from `meshtasticd`, or not (`Link::Off`).
    pub fn new(link: Link) -> Self {
        MeshView {
            link,
            ready: None,
            starter: None,
            nodes: Vec::new(),
            listened: Vec::new(),
            order: Order::default(),
            active_only: false,
            since: None,
            selected: None,
            scale: FIRST_SCALE,
            now: 0,
            utc_offset_s: 0,
        }
    }

    pub fn on_update(&mut self, update: MeshUpdate) {
        match update {
            MeshUpdate::Connecting(_) if matches!(self.link, Link::Lost(_)) => {}
            MeshUpdate::Connecting(addr) => self.link = Link::Connecting(addr),
            MeshUpdate::Ready(ready) => {
                self.link = Link::Connected;
                self.ready = Some(ready);
                if !matches!(self.listened.last(), Some((_, None))) {
                    self.listened.push((self.now, None));
                }
            }
            MeshUpdate::Lost(why) => {
                self.link = Link::Lost(why);
                if let Some((_, end @ None)) = self.listened.last_mut() {
                    *end = Some(self.now);
                }
            }
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
        if packet.app_only {
            keep(&mut row.app_only_times, packet.received);
            row.app_only += 1;
            row.last = Some(packet);
            return;
        }
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
        keep(&mut row.times, packet.received);
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

    /// The offset of the local time from UTC, for the clock's marks.
    pub fn set_utc_offset(&mut self, seconds: i32) {
        self.utc_offset_s = seconds;
    }

    /// "14:30": the local time of day at `unix_s`.
    pub fn time_of_day(&self, unix_s: u64) -> String {
        let local = unix_s as i64 + i64::from(self.utc_offset_s);
        let of_day = local.rem_euclid(86_400);
        format!("{:02}:{:02}", of_day / 3600, of_day % 3600 / 60)
    }

    /// The node itself first, then the others that are shown, in the
    /// order chosen.
    pub fn rows(&self) -> Vec<&NodeRow> {
        let me = self.ready.as_ref().map(|ready| ready.me);
        let from = self.shown_from();
        let mut rows: Vec<&NodeRow> = self
            .nodes
            .iter()
            .filter(|row| {
                Some(row.num) == me
                    || from.is_none_or(|from| row.last_heard.is_some_and(|heard| heard >= from))
            })
            .collect();
        rows.sort_by_key(|row| {
            let own = Some(row.num) == me;
            let hops = match self.order {
                Order::Heard => None,
                Order::Hops => Some(row.hops.unwrap_or(u32::MAX)),
            };
            (!own, hops, std::cmp::Reverse(row.last_heard), row.num)
        });
        rows
    }

    /// Nodes heard before this are not shown, if any are not.
    fn shown_from(&self) -> Option<u64> {
        let active = self.active_only.then(|| self.now.saturating_sub(ACTIVE_S));
        active.max(self.since)
    }

    /// How many nodes there are, shown or not.
    pub fn known(&self) -> usize {
        self.nodes.len()
    }

    /// Packets heard from `row` per column, `columns` of them up to the
    /// one now is in, the oldest first; none from before the list was
    /// started afresh.
    pub fn cells(&self, row: &NodeRow, columns: usize) -> Vec<u32> {
        self.count(&row.times, columns)
    }

    /// The same of what the node handed this app only.
    pub fn app_only_cells(&self, row: &NodeRow, columns: usize) -> Vec<u32> {
        self.count(&row.app_only_times, columns)
    }

    fn count(&self, times: &VecDeque<u64>, columns: usize) -> Vec<u32> {
        let scale = self.seconds_per_column();
        let from = self.since.unwrap_or(0);
        let mut cells = vec![0; columns];
        for &time in times
            .iter()
            .filter(|&&time| (from..=self.now).contains(&time))
        {
            let back = usize::try_from(self.now / scale - time / scale).unwrap_or(usize::MAX);
            if let Some(cell) = columns
                .checked_sub(back.saturating_add(1))
                .and_then(|c| cells.get_mut(c))
            {
                *cell += 1;
            }
        }
        cells
    }

    /// Which of `columns` the app was listening in, for a while at least:
    /// the others hold only what `meshtasticd` kept for it meanwhile, which
    /// is not all it heard, or nothing.
    pub fn listened(&self, columns: usize) -> Vec<bool> {
        let from = self.since.unwrap_or(0);
        (0..columns)
            .map(|column| {
                let (start, end) = self.column_span(column, columns);
                end > from
                    && self
                        .listened
                        .iter()
                        .any(|&(on, off)| on < end && off.unwrap_or(self.now) >= start)
            })
            .collect()
    }

    /// The seconds `column` of `columns` stands for, the last one holding
    /// now: [start, end).
    fn column_span(&self, column: usize, columns: usize) -> (u64, u64) {
        let scale = self.seconds_per_column();
        let back = (columns - 1 - column) as u64;
        let start = (self.now / scale).saturating_sub(back) * scale;
        (start, start + scale)
    }

    /// The clock on the axis: the columns of `columns` that begin on a
    /// round time of day, and that time.
    pub fn marks(&self, columns: usize) -> Vec<(usize, String)> {
        let scale = self.seconds_per_column();
        let step = MARK_STEPS
            .into_iter()
            .find(|step| step % scale == 0 && step / scale >= MARK_COLUMNS);
        let Some(step) = step else {
            return Vec::new();
        };
        (0..columns)
            .filter_map(|column| {
                let (start, _) = self.column_span(column, columns);
                let local = start as i64 + i64::from(self.utc_offset_s);
                (local.rem_euclid(step as i64) == 0).then(|| (column, self.time_of_day(start)))
            })
            .collect()
    }

    /// How many nodes this run has heard a packet from.
    pub fn heard(&self) -> usize {
        self.nodes.iter().filter(|node| node.packets > 0).count()
    }

    pub fn seconds_per_column(&self) -> u64 {
        SCALES[self.scale]
    }

    /// "10 s", "5 m", "1 h": what a column stands for.
    pub fn scale_name(&self) -> String {
        match self.seconds_per_column() {
            s if s < 60 => format!("{s} s"),
            s if s < 3600 => format!("{} m", s / 60),
            s => format!("{} h", s / 3600),
        }
    }

    pub fn zoom_in(&mut self) {
        self.scale = self.scale.saturating_sub(1);
    }

    pub fn zoom_out(&mut self) {
        self.scale = (self.scale + 1).min(SCALES.len() - 1);
    }

    pub fn order(&self) -> Order {
        self.order
    }

    pub fn next_order(&mut self) {
        self.order = match self.order {
            Order::Heard => Order::Hops,
            Order::Hops => Order::Heard,
        };
    }

    /// Only the nodes heard in the last `ACTIVE_S`, or all.
    pub fn active_only(&self) -> bool {
        self.active_only
    }

    pub fn toggle_active(&mut self) {
        self.active_only = !self.active_only;
    }

    /// When the list was last started afresh.
    pub fn since(&self) -> Option<u64> {
        self.since
    }

    /// Shows only the nodes heard from now on, and their packets: as if the
    /// app had started now. Nothing is forgotten — names stay known.
    pub fn start_afresh(&mut self) {
        self.since = Some(self.now);
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

    /// The selected node, while it is shown.
    pub fn selected(&self) -> Option<&NodeRow> {
        let selected = self.selected?;
        self.rows().into_iter().find(|row| row.num == selected)
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

/// Adds `time` to a row's times, and lets go of what is older than the
/// history or beyond its count.
fn keep(times: &mut VecDeque<u64>, time: u64) {
    times.push_back(time);
    while times.len() > MAX_TICKS || times.front().is_some_and(|&t| t + HISTORY_S < time) {
        times.pop_front();
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
            app_only: false,
            content,
        }
    }

    /// The node's device telemetry of every minute, as `meshtasticd`
    /// hands it to the app: not received, never sent.
    pub(crate) fn app_only(received: u64) -> Packet {
        let mut minute = packet(ME, received, telemetry());
        (minute.snr, minute.rssi, minute.hops) = (0.0, 0, None);
        (minute.hop_start, minute.relay, minute.app_only) = (0, 0, true);
        minute
    }

    pub(crate) fn telemetry() -> Content {
        Content::Other {
            port: 67,
            name: "telemetry".into(),
        }
    }

    /// The device's node, a gateway heard twice and a far node once,
    /// at 10:00:00 on the wall clock (UTC); the app has listened since
    /// 9:51:40, what came before is what `meshtasticd` kept for it.
    pub(crate) fn view() -> MeshView {
        let mut view = MeshView::new(Link::Connecting("localhost:4403".into()));
        view.set_now(35_500);
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
    fn what_only_the_app_got_is_no_packet_heard_and_has_marks_of_its_own() {
        let mut view = view();
        // Its device telemetry of 9:58:20 and 9:59:20, and what it sent on
        // the air at 9:59:30.
        for at in [35_900, 35_960] {
            view.on_update(MeshUpdate::Packet(app_only(at)));
        }
        let me = view.rows()[0].clone();
        assert_eq!((me.packets, me.app_only), (0, 2));
        assert_eq!(view.heard(), 2);
        assert_eq!(view.cells(&me, 20), [0; 20]);
        assert_eq!(view.app_only_cells(&me, 20)[17..], [1, 1, 0]);
        // The card tells it as the last.
        assert!(me.last.as_ref().unwrap().app_only);
        let mut sent = packet(ME, 35_970, telemetry());
        (sent.snr, sent.rssi, sent.hops) = (0.0, 0, Some(0));
        view.on_update(MeshUpdate::Packet(sent));
        let me = view.rows()[0].clone();
        assert_eq!((me.packets, me.app_only), (1, 2));
        assert_eq!(view.heard(), 3);
        assert_eq!(view.cells(&me, 20)[18], 1);
        // Started afresh, the app's own go too.
        view.start_afresh();
        assert_eq!(view.app_only_cells(&me, 20), [0; 20]);
    }

    #[test]
    fn columns_begin_on_the_clocks_minutes_so_the_pulse_steps_at_once() {
        let mut view = view();
        let gate = view.rows()[1].clone();
        // 9:59:10 is in the minute before 10:00, 9:43:20 17 minutes back.
        let cells = view.cells(&gate, 20);
        assert_eq!(cells.len(), 20);
        assert_eq!(cells[18], 1);
        assert_eq!(cells[2], 1);
        assert_eq!(cells.iter().sum::<u32>(), 2);
        // A second before 10:00 the newer was in the minute of now; until
        // 10:01 nothing moves, then both move together.
        view.set_now(35_999);
        assert_eq!(view.cells(&gate, 20)[19], 1);
        view.set_now(36_059);
        assert_eq!(view.cells(&gate, 20), cells);
        view.set_now(36_060);
        let moved = view.cells(&gate, 20);
        assert_eq!((moved[17], moved[1]), (1, 1));
        view.set_now(36_000);
        view.zoom_out();
        assert_eq!(view.seconds_per_column(), 300);
        assert_eq!(view.scale_name(), "5 m");
        assert_eq!(view.cells(&gate, 20)[15..], [1, 0, 0, 1, 0]);
        for _ in 0..5 {
            view.zoom_out();
        }
        assert_eq!(view.scale_name(), "1 h");
        assert_eq!(view.cells(&gate, 3), [0, 2, 0]);
        for _ in 0..9 {
            view.zoom_in();
        }
        assert_eq!(view.scale_name(), "10 s");
        // 9:59:10 is five columns of ten seconds back.
        assert_eq!(view.cells(&gate, 20)[14], 1);
    }

    #[test]
    fn the_time_before_the_app_listened_is_told_apart() {
        let mut view = view();
        // It listens since 9:51:40: from the minute of 9:51 on.
        let listened = view.listened(20);
        assert_eq!(listened[..10], [false; 10]);
        assert_eq!(listened[10..], [true; 10]);
        // Lost at 9:56:40, back at 9:58:20: the minute of 9:57 was missed.
        view.set_now(35_800);
        view.on_update(MeshUpdate::Lost("closed".into()));
        view.set_now(35_900);
        view.on_update(MeshUpdate::Connecting("localhost:4403".into()));
        view.on_update(MeshUpdate::Ready(ready()));
        view.set_now(36_000);
        let listened = view.listened(20);
        assert_eq!(listened[15..], [true, false, true, true, true]);
    }

    #[test]
    fn a_lost_link_stays_lost_while_trying_again() {
        let mut view = view();
        view.on_update(MeshUpdate::Lost("Connection refused".into()));
        view.on_update(MeshUpdate::Connecting("localhost:4403".into()));
        assert_eq!(view.link, Link::Lost("Connection refused".into()));
        view.on_update(MeshUpdate::Ready(ready()));
        assert_eq!(view.link, Link::Connected);
    }

    #[test]
    fn the_axis_marks_round_times_of_day_far_enough_apart() {
        let mut view = view();
        // A minute a column, 40 of them: 9:21 to 10:00, a mark each 15 m.
        assert_eq!(
            view.marks(40),
            [
                (9, "09:30".to_string()),
                (24, "09:45".to_string()),
                (39, "10:00".to_string())
            ]
        );
        // In the local time.
        view.set_utc_offset(2 * 3600);
        assert_eq!(view.marks(40)[0], (9, "11:30".to_string()));
        assert_eq!(view.time_of_day(36_000), "12:00");
        // Ten seconds a column: a mark each 2 minutes.
        view.zoom_in();
        view.zoom_in();
        let marks: Vec<usize> = view.marks(40).into_iter().map(|(at, _)| at).collect();
        assert_eq!(marks, [3, 15, 27, 39]);
    }

    #[test]
    fn starting_afresh_shows_only_what_is_heard_from_then() {
        let mut view = view();
        view.move_selection(1);
        view.move_selection(1);
        view.start_afresh();
        assert_eq!(view.since(), Some(36_000));
        let nums: Vec<u32> = view.rows().iter().map(|row| row.num).collect();
        assert_eq!(nums, [ME]);
        assert!(view.selected().is_none());
        // The past is not shown, nor listened to.
        assert_eq!(view.listened(20)[18..], [false, true]);
        view.on_update(MeshUpdate::Packet(packet(GATE, 36_020, telemetry())));
        view.set_now(36_030);
        let gate = view.rows()[1].clone();
        assert_eq!(gate.user.as_ref().unwrap().short_name, "HG1");
        assert_eq!(view.cells(&gate, 20).iter().sum::<u32>(), 1);
        // A new connection's database brings back only those heard since.
        for (num, heard) in [(FAR, 30_000), (0x0bad_cafe, 36_025)] {
            view.on_update(MeshUpdate::Node(Node {
                num,
                user: None,
                last_heard: Some(heard),
                snr: None,
                hops_away: Some(1),
                via_mqtt: false,
            }));
        }
        let nums: Vec<u32> = view.rows().iter().map(|row| row.num).collect();
        assert_eq!(nums, [ME, 0x0bad_cafe, GATE]);
        assert_eq!(view.known(), 4);
    }

    #[test]
    fn sorted_by_hops_the_nearest_come_first() {
        const NEAR: u32 = 0x1234_5678;
        const QUIET: u32 = 0x0bad_cafe;
        let mut view = view();
        let mut direct = packet(NEAR, 35_100, telemetry());
        direct.hops = Some(0);
        view.on_update(MeshUpdate::Packet(direct));
        view.on_update(MeshUpdate::Node(Node {
            num: QUIET,
            user: None,
            last_heard: Some(35_990),
            snr: None,
            hops_away: None,
            via_mqtt: false,
        }));
        let nums =
            |view: &MeshView| -> Vec<u32> { view.rows().iter().map(|row| row.num).collect() };
        assert_eq!(view.order(), Order::Heard);
        assert_eq!(nums(&view), [ME, QUIET, GATE, NEAR, FAR]);
        view.next_order();
        assert_eq!(view.order(), Order::Hops);
        // The node itself first still; who knows how far is last.
        assert_eq!(nums(&view), [ME, NEAR, GATE, FAR, QUIET]);
        view.next_order();
        assert_eq!(view.order(), Order::Heard);
    }

    #[test]
    fn the_active_are_the_nodes_heard_in_the_last_15_minutes() {
        let mut view = view();
        view.move_selection(1);
        view.move_selection(isize::MAX);
        assert_eq!(view.selected().map(|row| row.num), Some(FAR));
        view.toggle_active();
        assert!(view.active_only());
        // FAR was heard 50 minutes ago: hidden, and so is its selection.
        let nums: Vec<u32> = view.rows().iter().map(|row| row.num).collect();
        assert_eq!(nums, [ME, GATE]);
        assert!(view.selected().is_none());
        // GATE goes quiet for 15 minutes and more.
        view.set_now(35_950 + ACTIVE_S + 1);
        assert_eq!(view.rows().len(), 1);
        view.toggle_active();
        assert_eq!(view.rows().len(), 3);
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
