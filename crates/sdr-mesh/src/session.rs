//! One connection's talk with the node, without the socket: what to send
//! first, and the bytes that come back turned into updates.

use prost::Message;

use crate::frame::{self, Deframer};
use crate::lora::{self, Radio};
use crate::proto::config::device_config::Role;
use crate::proto::{self, PortNum, from_radio, mesh_packet, to_radio};
use crate::update::{Content, MeshUpdate, Node, NodeUser, Packet, Ready};

/// What the node has told so far in this connection.
#[derive(Debug)]
pub struct Session {
    /// Our request's number, which the node gives back when it is done.
    config_id: u32,
    deframer: Deframer,
    me: Option<u32>,
    firmware: String,
    lora: Option<proto::config::LoRaConfig>,
    channels: Vec<(u32, String)>,
}

impl Session {
    pub fn new(config_id: u32) -> Self {
        Session {
            config_id,
            deframer: Deframer::default(),
            me: None,
            firmware: String::new(),
            lora: None,
            channels: Vec::new(),
        }
    }

    /// The first thing to send: a request for the node's configuration
    /// and database, then the packets it hears.
    pub fn hello(&self) -> Vec<u8> {
        to_radio(to_radio::PayloadVariant::WantConfigId(self.config_id))
    }

    /// Sent now and then, so that the node keeps the connection.
    pub fn heartbeat() -> Vec<u8> {
        to_radio(to_radio::PayloadVariant::Heartbeat(
            proto::Heartbeat::default(),
        ))
    }

    /// The updates in `bytes`, which reached the app at `now` (Unix
    /// seconds: the time of a packet the node does not date). A message
    /// that does not decode is skipped.
    pub fn on_bytes(&mut self, bytes: &[u8], now: u64) -> Vec<MeshUpdate> {
        let messages = self.deframer.push(bytes);
        let decoded = messages
            .iter()
            .filter_map(|message| proto::FromRadio::decode(&message[..]).ok());
        decoded
            .filter_map(|message| self.on_message(message.payload_variant?, now))
            .collect()
    }

    fn on_message(&mut self, payload: from_radio::PayloadVariant, now: u64) -> Option<MeshUpdate> {
        use from_radio::PayloadVariant::*;
        match payload {
            MyInfo(info) => self.me = Some(info.my_node_num),
            NodeInfo(info) => return Some(MeshUpdate::Node(node(info))),
            Metadata(metadata) => self.firmware = metadata.firmware_version,
            Config(config) => {
                if let Some(proto::config::PayloadVariant::Lora(lora)) = config.payload_variant {
                    self.lora = Some(lora);
                }
            }
            Channel(channel) => self.on_channel(channel),
            ConfigCompleteId(id) if id == self.config_id => {
                return Some(MeshUpdate::Ready(self.ready()?));
            }
            Packet(packet) => {
                return Some(MeshUpdate::Packet(self::packet(packet, now, self.me)));
            }
            _ => {}
        }
        None
    }

    fn on_channel(&mut self, channel: proto::Channel) {
        use proto::channel::Role as ChannelRole;
        let role = ChannelRole::try_from(channel.role).unwrap_or(ChannelRole::Disabled);
        let index = u32::try_from(channel.index).unwrap_or(0);
        self.channels.retain(|&(i, _)| i != index);
        if role == ChannelRole::Disabled {
            return;
        }
        let name = channel.settings.map(|s| s.name).unwrap_or_default();
        let entry = (index, name);
        if role == ChannelRole::Primary {
            self.channels.insert(0, entry);
        } else {
            self.channels.push(entry);
        }
    }

    /// Everything told, once the node has said it is done.
    fn ready(&self) -> Option<Ready> {
        let primary = self.channels.first().map_or("", |(_, name)| name.as_str());
        let radio: Option<Radio> = self.lora.as_ref().map(|lora| lora::radio(lora, primary));
        let preset = radio.as_ref().map_or("", |radio| radio.preset.as_str());
        let channels = self.channels.iter().map(|(index, name)| {
            let name = if name.is_empty() { preset } else { name };
            (*index, name.to_string())
        });
        Some(Ready {
            me: self.me?,
            firmware: self.firmware.clone(),
            channels: channels.collect(),
            radio,
        })
    }
}

fn node(info: proto::NodeInfo) -> Node {
    Node {
        num: info.num,
        user: info.user.map(user),
        last_heard: (info.last_heard != 0).then_some(u64::from(info.last_heard)),
        snr: (info.snr != 0.0).then_some(info.snr),
        hops_away: info.hops_away,
        via_mqtt: info.via_mqtt,
    }
}

fn user(user: proto::User) -> NodeUser {
    let hw_model = proto::HardwareModel::try_from(user.hw_model)
        .map_or_else(|_| user.hw_model.to_string(), |m| m.as_str_name().into());
    let role = Role::try_from(user.role)
        .map_or_else(|_| user.role.to_string(), |r| r.as_str_name().into());
    NodeUser {
        id: user.id,
        long_name: user.long_name,
        short_name: user.short_name,
        hw_model,
        role,
    }
}

/// `me`: the node's own number, once it has told it.
fn packet(packet: proto::MeshPacket, received: u64, me: Option<u32>) -> Packet {
    let content = match packet.payload_variant {
        Some(mesh_packet::PayloadVariant::Decoded(data)) => content(data),
        _ => Content::Encrypted,
    };
    let hops = (packet.hop_start > 0).then(|| packet.hop_start.saturating_sub(packet.hop_limit));
    // The node's time of hearing it: a packet kept for a client that was
    // not connected arrives long after.
    let received = match packet.rx_time {
        0 => received,
        heard => u64::from(heard),
    };
    Packet {
        received,
        from: packet.from,
        to: packet.to,
        channel: packet.channel,
        id: packet.id,
        snr: packet.rx_snr,
        rssi: packet.rx_rssi,
        hops,
        hop_start: packet.hop_start,
        relay: (packet.relay_node & 0xff) as u8,
        via_mqtt: packet.via_mqtt,
        app_only: Some(packet.from) == me && packet.hop_start == 0,
        content,
    }
}

fn content(data: proto::Data) -> Content {
    if data.portnum == PortNum::NodeinfoApp as i32
        && let Ok(who) = proto::User::decode(&data.payload[..])
    {
        return Content::User(user(who));
    }
    Content::Other {
        port: data.portnum,
        name: port_name(data.portnum),
    }
}

/// A port's short name: "text message", "telemetry".
pub fn port_name(port: i32) -> String {
    let Ok(port) = PortNum::try_from(port) else {
        return format!("port {port}");
    };
    let name = port.as_str_name();
    let name = name.strip_suffix("_APP").unwrap_or(name);
    name.to_lowercase().replace('_', " ")
}

fn to_radio(payload: to_radio::PayloadVariant) -> Vec<u8> {
    let message = proto::ToRadio {
        payload_variant: Some(payload),
    };
    frame::encode(&message.encode_to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::config::LoRaConfig;
    use crate::proto::config::lo_ra_config::{ModemPreset, RegionCode};
    use crate::update::BROADCAST;

    fn framed(payload: from_radio::PayloadVariant) -> Vec<u8> {
        let message = proto::FromRadio {
            id: 0,
            payload_variant: Some(payload),
        };
        frame::encode(&message.encode_to_vec())
    }

    fn user(id: &str, long: &str, short: &str) -> proto::User {
        proto::User {
            id: id.into(),
            long_name: long.into(),
            short_name: short.into(),
            hw_model: proto::HardwareModel::HeltecV3 as i32,
            role: Role::Client as i32,
            ..proto::User::default()
        }
    }

    /// What the node sends after `hello`, as the device's meshtasticd did.
    fn config_stream(config_id: u32) -> Vec<u8> {
        use from_radio::PayloadVariant::*;
        let mut stream = framed(MyInfo(proto::MyNodeInfo {
            my_node_num: 0x015b_8900,
            ..proto::MyNodeInfo::default()
        }));
        stream.extend(framed(NodeInfo(proto::NodeInfo {
            num: 0x015b_8900,
            user: Some(user("!015b8900", "Meshtastic 8900", "8900")),
            last_heard: 1_794_726_262,
            ..proto::NodeInfo::default()
        })));
        stream.extend(framed(NodeInfo(proto::NodeInfo {
            num: 0xbfb5_4553,
            last_heard: 1_794_726_000,
            snr: 6.25,
            hops_away: Some(2),
            ..proto::NodeInfo::default()
        })));
        stream.extend(framed(Metadata(proto::DeviceMetadata {
            firmware_version: "2.7.26".into(),
            ..proto::DeviceMetadata::default()
        })));
        stream.extend(framed(Config(proto::Config {
            payload_variant: Some(proto::config::PayloadVariant::Lora(LoRaConfig {
                use_preset: true,
                region: RegionCode::Us as i32,
                modem_preset: ModemPreset::MediumFast as i32,
                channel_num: 70,
                ..LoRaConfig::default()
            })),
        })));
        for (index, name, role) in [(0, "", 1), (1, "Haifa", 2), (2, "", 0)] {
            stream.extend(framed(Channel(proto::Channel {
                index,
                settings: Some(proto::ChannelSettings {
                    name: name.into(),
                    ..proto::ChannelSettings::default()
                }),
                role,
            })));
        }
        stream.extend(framed(ConfigCompleteId(config_id)));
        stream
    }

    #[test]
    fn hello_asks_for_the_configuration_by_its_number() {
        let hello = Session::new(77).hello();
        let message = proto::ToRadio::decode(&hello[4..]).unwrap();
        assert_eq!(
            message.payload_variant,
            Some(to_radio::PayloadVariant::WantConfigId(77))
        );
        let beat = proto::ToRadio::decode(&Session::heartbeat()[4..]).unwrap();
        assert!(matches!(
            beat.payload_variant,
            Some(to_radio::PayloadVariant::Heartbeat(_))
        ));
    }

    #[test]
    fn the_database_comes_first_and_ready_when_the_node_is_done() {
        let mut session = Session::new(77);
        let updates = session.on_bytes(&config_stream(77), 1000);
        let [
            MeshUpdate::Node(me),
            MeshUpdate::Node(other),
            MeshUpdate::Ready(ready),
        ] = &updates[..]
        else {
            panic!("{updates:#?}");
        };
        assert_eq!(me.num, 0x015b_8900);
        let me_user = me.user.as_ref().unwrap();
        assert_eq!(me_user.long_name, "Meshtastic 8900");
        assert_eq!(
            (me_user.hw_model.as_str(), me_user.role.as_str()),
            ("HELTEC_V3", "CLIENT")
        );
        assert_eq!(me.last_heard, Some(1_794_726_262));
        // A node heard of but never heard telling who it is.
        assert_eq!(other.user, None);
        assert_eq!((other.snr, other.hops_away), (Some(6.25), Some(2)));
        assert_eq!(ready.me, 0x015b_8900);
        assert_eq!(ready.firmware, "2.7.26");
        let radio = ready.radio.as_ref().unwrap();
        assert_eq!((radio.slot, radio.frequency_mhz), (Some(70), Some(919.375)));
        // The disabled channel is left out; the primary's empty name is
        // the preset's.
        assert_eq!(
            ready.channels,
            [(0, "MediumFast".to_string()), (1, "Haifa".to_string())]
        );
    }

    #[test]
    fn another_requests_end_is_not_ours() {
        let mut session = Session::new(77);
        let updates = session.on_bytes(&config_stream(78), 1000);
        assert!(!updates.iter().any(|u| matches!(u, MeshUpdate::Ready(_))));
    }

    fn packet(payload: mesh_packet::PayloadVariant) -> proto::MeshPacket {
        proto::MeshPacket {
            from: 0xbfb5_4553,
            to: BROADCAST,
            channel: 0,
            id: 0x1234,
            rx_snr: 6.25,
            rx_rssi: -98,
            hop_start: 7,
            hop_limit: 5,
            relay_node: 0x3a,
            payload_variant: Some(payload),
            ..proto::MeshPacket::default()
        }
    }

    fn data(port: PortNum, payload: Vec<u8>) -> mesh_packet::PayloadVariant {
        mesh_packet::PayloadVariant::Decoded(proto::Data {
            portnum: port as i32,
            payload,
            ..proto::Data::default()
        })
    }

    #[test]
    fn a_packet_tells_its_hops_its_last_hop_and_what_it_is() {
        let mut session = Session::new(77);
        let nodeinfo = user("!bfb54553", "Haifa Gate", "HG1").encode_to_vec();
        let mut stream = framed(from_radio::PayloadVariant::Packet(packet(data(
            PortNum::NodeinfoApp,
            nodeinfo,
        ))));
        stream.extend(framed(from_radio::PayloadVariant::Packet(packet(data(
            PortNum::TelemetryApp,
            vec![1, 2, 3],
        )))));
        // Heard while no client was connected: the node kept it, and its
        // time is the node's.
        let mut queued = packet(data(PortNum::TelemetryApp, vec![]));
        queued.rx_time = 1500;
        stream.extend(framed(from_radio::PayloadVariant::Packet(queued)));
        let mut encrypted = packet(mesh_packet::PayloadVariant::Encrypted(vec![9; 20]));
        encrypted.channel = 0x8b;
        // From firmware that does not tell how many hops it may take.
        encrypted.hop_start = 0;
        stream.extend(framed(from_radio::PayloadVariant::Packet(encrypted)));
        let updates = session.on_bytes(&stream, 2000);
        let packets: Vec<&Packet> = updates
            .iter()
            .map(|u| match u {
                MeshUpdate::Packet(p) => p,
                other => panic!("{other:?}"),
            })
            .collect();
        let first = packets[0];
        assert_eq!(
            (first.received, first.from, first.to),
            (2000, 0xbfb5_4553, BROADCAST)
        );
        assert_eq!((first.snr, first.rssi), (6.25, -98));
        assert_eq!(
            (first.hops, first.hop_start, first.relay),
            (Some(2), 7, 0x3a)
        );
        let Content::User(who) = &first.content else {
            panic!("{first:?}");
        };
        assert_eq!(
            (who.short_name.as_str(), who.long_name.as_str()),
            ("HG1", "Haifa Gate")
        );
        assert_eq!(
            packets[1].content,
            Content::Other {
                port: PortNum::TelemetryApp as i32,
                name: "telemetry".into()
            }
        );
        assert_eq!(packets[2].received, 1500);
        assert_eq!(packets[3].content, Content::Encrypted);
        assert_eq!((packets[3].channel, packets[3].hops), (0x8b, None));
    }

    #[test]
    fn what_the_node_made_for_its_clients_alone_is_told_apart() {
        let mut session = Session::new(77);
        let mut stream = config_stream(77);
        // Its device telemetry of every minute: never sent, so the hop
        // count it starts with was never set.
        let mut minute = packet(data(PortNum::TelemetryApp, vec![]));
        (minute.from, minute.hop_start, minute.hop_limit) = (0x015b_8900, 0, 3);
        (minute.rx_snr, minute.rx_rssi, minute.relay_node) = (0.0, 0, 0);
        // What it sent on the air, handed over too: the hops are set.
        let mut sent = minute.clone();
        sent.hop_start = 3;
        // Another node's, from firmware that does not tell its hops.
        let mut old = packet(data(PortNum::TelemetryApp, vec![]));
        old.hop_start = 0;
        for packet in [minute, sent, old] {
            stream.extend(framed(from_radio::PayloadVariant::Packet(packet)));
        }
        let app_only: Vec<bool> = session
            .on_bytes(&stream, 2000)
            .iter()
            .filter_map(|u| match u {
                MeshUpdate::Packet(p) => Some(p.app_only),
                _ => None,
            })
            .collect();
        assert_eq!(app_only, [true, false, false]);
    }

    #[test]
    fn ports_have_short_names_and_unknown_ones_their_number() {
        assert_eq!(port_name(PortNum::TextMessageApp as i32), "text message");
        assert_eq!(port_name(PortNum::NeighborinfoApp as i32), "neighborinfo");
        assert_eq!(port_name(PortNum::AtakPlugin as i32), "atak plugin");
        assert_eq!(port_name(12345), "port 12345");
    }

    #[test]
    fn what_does_not_decode_is_skipped() {
        let mut session = Session::new(77);
        let mut stream = frame::encode(&[0xff; 10]);
        stream.extend(config_stream(77));
        let updates = session.on_bytes(&stream, 1000);
        assert_eq!(updates.len(), 3);
    }
}
