//! The mesh's thread: `sdr_mesh`'s client, its updates handed to the
//! interface; and the pulse as text, for `--snapshot --screen 6`.

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use sdr_core::SourceInfo;
use sdr_mesh::client::{self, Timing};
use sdr_mesh::update::{BROADCAST, Content, MeshUpdate, Packet};
use sdr_tui::mesh::{Link, MeshView};
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
    let mut packets = Vec::new();
    for update in updates {
        if let MeshUpdate::Packet(packet) = &update {
            packets.push(packet.clone());
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
    let _ = writeln!(text, "\n{} packets in {secs} s.", packets.len());
    for packet in packets.iter().rev().take(MAX_PACKETS).rev() {
        let _ = writeln!(text, "{}", describe(&app.mesh, packet));
    }
    Ok(text)
}

/// One line: when, from whom to whom, what, and how it arrived.
fn describe(view: &MeshView, packet: &Packet) -> String {
    let when = chrono::DateTime::from_timestamp(packet.received as i64, 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
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

    #[test]
    fn what_the_node_hands_the_app_alone_is_not_to_all() {
        let mut minute = Packet {
            received: 36_000,
            from: 0x015b_8900,
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
        };
        let view = MeshView::new(Link::Off);
        assert!(describe(&view, &minute).contains("!015b8900 → this app "));
        minute.app_only = false;
        assert!(describe(&view, &minute).contains("!015b8900 → all "));
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
