//! The mesh's thread: `sdr_mesh`'s client, its updates handed to the
//! interface; what earlier runs kept, for the pulse; and the pulse as
//! text, for `--snapshot --screen 6`.

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use sdr_core::SourceInfo;
use sdr_mesh::client::{self, Timing};
use sdr_mesh::update::{BROADCAST, Content, MeshUpdate, Packet, Sent};
use sdr_store::mesh::MeshHistory;
use sdr_tui::mesh::{History, Link, MeshView};
use sdr_tui::{App, Screen, SourceState, ui};

use crate::reader::Update;

const MAX_PACKETS: usize = 40;

/// The wall clock in Unix seconds: the mesh's time axis.
pub fn unix_now() -> u64 {
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since.map_or(0, |since| since.as_secs())
}

/// The interface's view of the node at `addr` before it has answered, its
/// clock in the local time (`utc_offset_s`).
pub fn view(addr: &str, utc_offset_s: i32) -> MeshView {
    let addr = client::with_port(addr);
    let exe = std::env::current_exe().ok();
    let mut view = MeshView::new(Link::Connecting(addr.clone()));
    view.starter = starter(&addr, exe.as_deref(), Path::is_file);
    view.set_utc_offset(utc_offset_s);
    view.set_now(unix_now());
    view
}

/// The command that starts the `meshtasticd` at `addr` when it is this
/// machine's: the `lora.sh` beside the app `exe` if there is one (it powers
/// the LoRa radio first), or systemd's.
pub fn starter(addr: &str, exe: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<String> {
    let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);
    if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    let script = exe
        .and_then(Path::parent)
        .map(|dir| dir.join("lora.sh"))
        .filter(|script| exists(script));
    Some(match script {
        Some(script) => format!("sudo {} on", script.display()),
        None => "sudo systemctl start meshtasticd".into(),
    })
}

/// What earlier runs kept, as the pulse takes it (`MeshView::load_history`):
/// the node they listened to last, the times they listened, the `texts` of
/// the days before, and the packets and that node's own queued, in the
/// order they came.
pub fn for_the_pulse(history: MeshHistory, texts: Vec<Packet>) -> History {
    let me = history.listened.last().map(|listened| listened.node);
    let listened = history.listened.iter();
    let listened = listened.map(|listened| (listened.from, listened.until));
    let packets = history
        .packets
        .into_iter()
        .map(|packet| (packet.received, MeshUpdate::Packet(packet)));
    let sent = history
        .sent
        .into_iter()
        .filter(|&(node, _)| Some(node) == me);
    let sent = sent.map(|(_, sent)| (sent.at, MeshUpdate::Sent(sent)));
    let mut told: Vec<(u64, MeshUpdate)> = packets.chain(sent).collect();
    told.sort_by_key(|&(at, _)| at);
    History {
        me,
        listened: listened.collect(),
        texts,
        told: told.into_iter().map(|(_, update)| update).collect(),
    }
}

/// Talks to `meshtasticd` at `addr` until `stop`, or until the interface
/// is gone. Nothing of it is kept, so the thread need not be waited for.
pub fn start(addr: &str, updates: SyncSender<Update>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    let addr = client::with_port(addr);
    std::thread::spawn(move || {
        let sink = |update| updates.send(Update::Mesh(update)).is_ok();
        client::run(
            &addr,
            client::tcp,
            sink,
            &stop,
            unix_now,
            &Timing::default(),
        );
    })
}

/// `secs` of the node at `addr`: its pulse as text and the packets heard.
/// Fails if the node never answered.
pub fn snapshot(addr: &str, (width, height): (u16, u16), secs: f64) -> Result<String, String> {
    let addr = client::with_port(addr);
    let stop = AtomicBool::new(false);
    let start = unix_now();
    let mut updates = Vec::new();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_secs_f64(secs.max(0.0)));
            stop.store(true, Ordering::Relaxed);
        });
        let timing = Timing {
            retry: Duration::from_secs(1),
            ..Timing::default()
        };
        let sink = |update| {
            updates.push(update);
            true
        };
        client::run(&addr, client::tcp, sink, &stop, unix_now, &timing);
    });
    let mut app = App::new(SourceInfo {
        sample_rate: 0,
        center_hz: 0.0,
        description: "mesh only".into(),
    });
    app.status.state = SourceState::Off;
    app.screen = Screen::Mesh;
    app.mesh = MeshView::new(Link::Connecting(addr.clone()));
    app.mesh
        .set_utc_offset(chrono::Local::now().offset().local_minus_utc());
    // The updates are told as of the start: it listened from then.
    app.set_wall_clock(start);
    let mut told = Vec::new();
    for update in updates {
        if matches!(update, MeshUpdate::Packet(_) | MeshUpdate::Sent(_)) {
            told.push(update.clone());
        }
        app.on_mesh(update);
    }
    if app.mesh.ready.is_none() {
        let why = match &app.mesh.link {
            Link::Lost(why) => format!(": {why}"),
            _ => String::new(),
        };
        let exe = std::env::current_exe().ok();
        let start = starter(&addr, exe.as_deref(), Path::is_file)
            .map_or(String::new(), |command| {
                format!("; is it running? Start it: {command}")
            });
        return Err(format!(
            "no answer from meshtasticd at {addr} in {secs} s{why}{start}"
        ));
    }
    app.set_wall_clock(unix_now());
    let mut text = ui::render_text(&app, width, height);
    let _ = writeln!(text, "\n{} packets in {secs} s.", told.len());
    let me = app.mesh.ready.as_ref().map(|ready| ready.me);
    for update in told.iter().rev().take(MAX_PACKETS).rev() {
        let line = match update {
            MeshUpdate::Packet(packet) => describe(&app.mesh, packet),
            MeshUpdate::Sent(sent) => describe_sent(sent, me),
            _ => continue,
        };
        let _ = writeln!(text, "{line}");
    }
    Ok(text)
}

/// "14:30:05": the local time of day at `unix_s`.
fn local_time(unix_s: u64) -> String {
    chrono::DateTime::from_timestamp(unix_s as i64, 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// One line: when the node queued a packet of its own for the air.
fn describe_sent(sent: &Sent, me: Option<u32>) -> String {
    let me = me.map_or("the node".to_string(), |me| format!("!{me:08x}"));
    format!(
        "{}  {me} queued a packet of its own for the air, 0x{:08x}",
        local_time(sent.at),
        sent.id
    )
}

/// One line: when, from whom to whom, what, and how it arrived.
fn describe(view: &MeshView, packet: &Packet) -> String {
    let when = local_time(packet.received);
    let to = if packet.app_only {
        "this app".to_string()
    } else if packet.to == BROADCAST {
        "all".to_string()
    } else {
        format!("!{:08x}", packet.to)
    };
    let what = match &packet.content {
        Content::Encrypted => "encrypted".to_string(),
        Content::User(user) => format!("node info {} {:?}", user.short_name, user.long_name),
        Content::LocalStats(stats) => format!("local stats: {} sent", stats.sent),
        Content::Text(text) => format!("text {text:?}"),
        Content::Position(position) => format!(
            "position {:.5}, {:.5}",
            position.latitude, position.longitude
        ),
        Content::Device(device) => {
            let battery = device
                .battery
                .map_or("battery ?".into(), |percent| format!("battery {percent}%"));
            let busy = device
                .channel_utilization
                .map_or(String::new(), |busy| format!(", channel {busy:.1}%"));
            format!("{battery}{busy}")
        }
        Content::Other { name, .. } => name.clone(),
    };
    let hops = packet.hops.map_or("hops ?".to_string(), |hops| {
        format!("{hops}/{} hops", packet.hop_start)
    });
    format!(
        "{when}  !{:08x} → {to:<9}  {what:<24}  {:<22}  SNR {:>6.2}  RSSI {:>4}  {hops}{}",
        packet.from,
        view.channel_name(packet),
        packet.snr,
        packet.rssi,
        if packet.via_mqtt { "  MQTT" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdr_mesh::update::{DeviceMetrics, LocalStats, Position};
    use sdr_store::mesh::Listened;
    use sdr_tui::mesh::History;

    fn minute_of(from: u32, received: u64) -> Packet {
        Packet {
            received,
            from,
            to: BROADCAST,
            channel: 0,
            id: 1,
            snr: 0.0,
            rssi: 0,
            hops: None,
            hop_start: 0,
            relay: 0,
            via_mqtt: false,
            app_only: true,
            content: Content::Other {
                port: 67,
                name: "telemetry".into(),
            },
            payload: None,
        }
    }

    #[test]
    fn texts_positions_and_metrics_are_told_by_what_they_say() {
        let view = MeshView::new(Link::Off);
        let mut packet = minute_of(0x015b_8900, 36_000);
        let mut said = |content| {
            packet.content = content;
            describe(&view, &packet)
        };
        let text = said(Content::Text("shalom\nall".into()));
        assert!(text.contains(" text \"shalom\\nall\" "), "{text}");
        let position = said(Content::Position(Position {
            latitude: 32.79412,
            longitude: 34.9892,
            altitude_m: Some(240),
            within_m: Some(2918.0),
        }));
        assert!(
            position.contains(" position 32.79412, 34.98920 "),
            "{position}"
        );
        let metrics = said(Content::Device(DeviceMetrics {
            battery: Some(87),
            voltage: Some(4.05),
            channel_utilization: Some(12.5),
            air_util_tx: Some(1.5),
        }));
        assert!(
            metrics.contains(" battery 87%, channel 12.5% "),
            "{metrics}"
        );
    }

    #[test]
    fn what_the_node_hands_the_app_alone_is_not_to_all() {
        let mut minute = minute_of(0x015b_8900, 36_000);
        let view = MeshView::new(Link::Off);
        assert!(describe(&view, &minute).contains("!015b8900 → this app "));
        minute.app_only = false;
        assert!(describe(&view, &minute).contains("!015b8900 → all "));
        minute.content = Content::LocalStats(LocalStats {
            sent: 3,
            uptime_s: 975,
        });
        assert!(describe(&view, &minute).contains(" local stats: 3 sent "));
        let sent = describe_sent(
            &Sent {
                at: 36_000,
                id: 0x77,
            },
            Some(0x015b_8900),
        );
        assert!(
            sent.ends_with("  !015b8900 queued a packet of its own for the air, 0x00000077"),
            "{sent}"
        );
    }

    #[test]
    fn what_earlier_runs_kept_goes_to_the_pulse_in_the_order_it_came() {
        const ME: u32 = 0x015b_8900;
        let packet = |received| {
            let mut packet = minute_of(ME, received);
            packet.app_only = false;
            packet
        };
        let history = MeshHistory {
            listened: vec![
                Listened {
                    node: 0x0bad_cafe,
                    from: 30_000,
                    until: 31_000,
                },
                Listened {
                    node: ME,
                    from: 35_000,
                    until: 35_600,
                },
            ],
            packets: vec![packet(35_100), packet(35_300)],
            sent: vec![
                (0x0bad_cafe, Sent { at: 30_500, id: 1 }),
                (ME, Sent { at: 35_200, id: 2 }),
            ],
        };
        let older = vec![packet(3_000)];
        let pulse = for_the_pulse(history, older.clone());
        assert_eq!(pulse.me, Some(ME));
        assert_eq!(pulse.listened, [(30_000, 31_000), (35_000, 35_600)]);
        assert_eq!(pulse.texts, older);
        // What another node queued is not this one's.
        let times: Vec<u64> = pulse
            .told
            .iter()
            .map(|update| match update {
                MeshUpdate::Packet(packet) => packet.received,
                MeshUpdate::Sent(sent) => sent.at,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(times, [35_100, 35_200, 35_300]);
        assert_eq!(
            for_the_pulse(MeshHistory::default(), Vec::new()),
            History::default()
        );
    }

    #[test]
    fn the_command_that_starts_meshtasticd_is_the_lora_script_beside_the_app() {
        let exe = Path::new("/home/dmw/sdr-viewer-check/sdr-viewer");
        let beside = |path: &Path| path == Path::new("/home/dmw/sdr-viewer-check/lora.sh");
        assert_eq!(
            starter("localhost:4403", Some(exe), beside).as_deref(),
            Some("sudo /home/dmw/sdr-viewer-check/lora.sh on")
        );
        assert_eq!(
            starter("127.0.0.1:4403", Some(exe), |_| false).as_deref(),
            Some("sudo systemctl start meshtasticd")
        );
        // Another machine's is started there.
        assert_eq!(starter("192.168.1.246:4403", Some(exe), beside), None);
        assert_eq!(
            starter("[::1]:4403", None, beside).as_deref(),
            Some("sudo systemctl start meshtasticd")
        );
    }
}
