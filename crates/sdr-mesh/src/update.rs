//! What the client tells the interface, in plain types: nothing of the
//! protobuf messages leaves this crate.

use crate::lora::Radio;

/// The node number of "everyone".
pub const BROADCAST: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq)]
pub enum MeshUpdate {
    /// Connecting to this address, at the start or again.
    Connecting(String),
    /// The node has told all about itself: its number, firmware, radio and
    /// channels, after the nodes of its database.
    Ready(Ready),
    /// A node of the node's database, as it was when we connected.
    Node(Node),
    /// A packet the node heard.
    Packet(Packet),
    /// The node has queued a packet of its own for the air: what it tells
    /// of itself, its replies. Its acknowledgements go without a word.
    Sent(Sent),
    /// The connection is gone, and why; it is tried again.
    Lost(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    /// The node's own number.
    pub me: u32,
    pub firmware: String,
    pub radio: Option<Radio>,
    /// The channels in use, by index, the primary first; an empty name is
    /// the preset's.
    pub channels: Vec<(u32, String)>,
}

/// Who a node is, as it tells it in its node info.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeUser {
    /// "!015b8900".
    pub id: String,
    pub long_name: String,
    pub short_name: String,
    /// "PORTDUINO", "HELTEC_V3"; "UNSET" when not told.
    pub hw_model: String,
    /// "CLIENT", "ROUTER".
    pub role: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub num: u32,
    pub user: Option<NodeUser>,
    /// When the node last heard it, Unix seconds by the node's clock.
    pub last_heard: Option<u64>,
    /// Of the last packet heard from it.
    pub snr: Option<f32>,
    pub hops_away: Option<u32>,
    pub via_mqtt: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    /// When the node heard it, Unix seconds by the node's clock; when it
    /// reached the app, by the app's, if the node does not tell.
    pub received: u64,
    pub from: u32,
    /// A node, or `BROADCAST`.
    pub to: u32,
    /// The channel's index if the node could read it, its hash if not.
    pub channel: u32,
    pub id: u32,
    /// Of the last hop; 0 when not measured.
    pub snr: f32,
    pub rssi: i32,
    /// Hops it took, when the sender said how many it may take.
    pub hops: Option<u32>,
    /// The hops it was sent with (`hop_start`); 0 from older firmware.
    pub hop_start: u32,
    /// The last byte of the number of the node that relayed it to us; 0
    /// when not known.
    pub relay: u8,
    pub via_mqtt: bool,
    /// The node made it for its clients alone and never sent it on the
    /// air: its device telemetry every minute, its local stats every 15
    /// (firmware 2.7.26, `DeviceTelemetry.cpp`). Told by the hops it
    /// starts with, which the node sets on what it transmits only.
    pub app_only: bool,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// When the app was told, Unix seconds by the app's clock: the node
    /// does not date it.
    pub at: u64,
    /// The packet's number: a copy of the packet may follow.
    pub id: u32,
}

/// What the node counts of itself, for its clients alone, every 15
/// minutes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalStats {
    /// Packets it has sent on the air since it started, acknowledgements
    /// and relays too.
    pub sent: u32,
    /// Seconds since it started.
    pub uptime_s: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// On a channel the node has no key for.
    Encrypted,
    /// A node telling who it is.
    User(NodeUser),
    /// The node's own counts.
    LocalStats(LocalStats),
    /// Anything else, by its port: the number and a short name ("text
    /// message", "telemetry", "position").
    Other { port: i32, name: String },
}
