//! The client thread's loop: connect to `meshtasticd`, ask for its
//! configuration, read what it hears, and connect again when the
//! connection goes (the daemon restarts itself after a change of its
//! settings).

use std::io::{self, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::session::Session;
use crate::update::MeshUpdate;

/// `meshtasticd`'s API port.
pub const PORT: u16 = 4403;

/// How long to wait, and how often to speak.
#[derive(Debug, Clone)]
pub struct Timing {
    /// Before connecting again.
    pub retry: Duration,
    /// Seconds between heartbeats, so the node keeps the connection.
    pub heartbeat_s: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            retry: Duration::from_secs(5),
            heartbeat_s: 60,
        }
    }
}

/// "localhost" → "localhost:4403"; an address with a port stays.
pub fn with_port(addr: &str) -> String {
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{PORT}")
    }
}

/// A TCP connection to `addr` that gives up on a read now and then, so
/// that the loop can look at the clock and the stop flag. Every address
/// of the name is tried: "localhost" is often ::1 first, and meshtasticd
/// listens on IPv4 only.
pub fn tcp(addr: &str) -> io::Result<TcpStream> {
    let mut failed = io::Error::new(ErrorKind::NotFound, "no such host");
    for target in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&target, Duration::from_secs(3)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_millis(200)))?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(err) => failed = err,
        }
    }
    Err(failed)
}

/// Talks to the node at `addr` until `stop` is set or `sink` says no one
/// listens any more (returns false). `connect` opens a connection,
/// `clock` tells Unix seconds.
pub fn run<S: Read + Write>(
    addr: &str,
    mut connect: impl FnMut(&str) -> io::Result<S>,
    mut sink: impl FnMut(MeshUpdate) -> bool,
    stop: &AtomicBool,
    clock: impl Fn() -> u64,
    timing: &Timing,
) {
    let mut attempt: u32 = 0;
    while !stop.load(Ordering::Relaxed) {
        attempt = attempt.wrapping_add(1);
        if !sink(MeshUpdate::Connecting(addr.to_string())) {
            return;
        }
        let end = match connect(addr) {
            Ok(stream) => talk(stream, attempt, &mut sink, stop, &clock, timing),
            Err(err) => End::Lost(err.to_string()),
        };
        let End::Lost(why) = end else {
            return;
        };
        if !sink(MeshUpdate::Lost(why)) {
            return;
        }
        wait(timing.retry, stop);
    }
}

/// Why a connection ended.
enum End {
    /// Asked to: the stop flag, or nobody listens.
    Stopped,
    Lost(String),
}

/// One connection, from the request to its end.
fn talk<S: Read + Write>(
    mut stream: S,
    attempt: u32,
    sink: &mut impl FnMut(MeshUpdate) -> bool,
    stop: &AtomicBool,
    clock: &impl Fn() -> u64,
    timing: &Timing,
) -> End {
    let mut session = Session::new(attempt);
    if let Err(err) = stream.write_all(&session.hello()) {
        return End::Lost(err.to_string());
    }
    let mut last_beat = clock();
    let mut buf = [0_u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        match stream.read(&mut buf) {
            Ok(0) => return End::Lost("meshtasticd closed the connection".into()),
            Ok(n) => {
                let now = clock();
                for update in session.on_bytes(&buf[..n], now) {
                    if !sink(update) {
                        return End::Stopped;
                    }
                }
            }
            Err(err) if is_quiet(&err) => {}
            Err(err) => return End::Lost(err.to_string()),
        }
        let now = clock();
        if now.saturating_sub(last_beat) >= timing.heartbeat_s {
            if let Err(err) = stream.write_all(&Session::heartbeat()) {
                return End::Lost(err.to_string());
            }
            last_beat = now;
        }
    }
    End::Stopped
}

/// A read that only found nothing yet.
fn is_quiet(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

/// Sleeps `how_long`, looking at `stop` every tenth of a second.
fn wait(how_long: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(100);
    let mut left = how_long;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let nap = left.min(step);
        std::thread::sleep(nap);
        left -= nap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;
    use crate::proto::{self, from_radio, to_radio};
    use prost::Message;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A connection that reads what it was given, then ends; what is
    /// written to it is kept.
    struct Fake {
        reads: VecDeque<io::Result<Vec<u8>>>,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.reads.pop_front() {
                None => Ok(0),
                Some(Err(err)) => Err(err),
                Some(Ok(bytes)) => {
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
            }
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn from_radio(payload: from_radio::PayloadVariant) -> Vec<u8> {
        let message = proto::FromRadio {
            id: 0,
            payload_variant: Some(payload),
        };
        frame::encode(&message.encode_to_vec())
    }

    fn sent(written: &[u8]) -> Vec<to_radio::PayloadVariant> {
        let mut deframer = frame::Deframer::default();
        let messages = deframer.push(written);
        let decoded = messages
            .iter()
            .map(|m| proto::ToRadio::decode(&m[..]).unwrap());
        decoded.filter_map(|m| m.payload_variant).collect()
    }

    const NOW: Timing = Timing {
        retry: Duration::ZERO,
        heartbeat_s: 60,
    };

    #[test]
    fn it_asks_reads_and_connects_again_when_the_connection_goes() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut connections = VecDeque::from([Ok(Fake {
            reads: VecDeque::from([
                Ok(from_radio(from_radio::PayloadVariant::MyInfo(
                    proto::MyNodeInfo {
                        my_node_num: 5,
                        ..proto::MyNodeInfo::default()
                    },
                ))),
                Err(io::Error::from(ErrorKind::WouldBlock)),
                // The first connection's request is number 1.
                Ok(from_radio(from_radio::PayloadVariant::ConfigCompleteId(1))),
            ]),
            written: Arc::clone(&written),
        })]);
        let mut updates = Vec::new();
        run(
            "node:4403",
            |addr| {
                assert_eq!(addr, "node:4403");
                connections
                    .pop_front()
                    .unwrap_or_else(|| Err(io::Error::from(ErrorKind::ConnectionRefused)))
            },
            |update| {
                updates.push(update);
                updates.len() < 5
            },
            &AtomicBool::new(false),
            || 1000,
            &NOW,
        );
        assert_eq!(updates[0], MeshUpdate::Connecting("node:4403".into()));
        assert!(matches!(&updates[1], MeshUpdate::Ready(ready) if ready.me == 5));
        assert_eq!(
            updates[2],
            MeshUpdate::Lost("meshtasticd closed the connection".into())
        );
        assert_eq!(updates[3], MeshUpdate::Connecting("node:4403".into()));
        assert!(matches!(&updates[4], MeshUpdate::Lost(why) if why.contains("refused")));
        assert_eq!(
            sent(&written.lock().unwrap()),
            [to_radio::PayloadVariant::WantConfigId(1)]
        );
    }

    #[test]
    fn a_quiet_connection_gets_a_heartbeat_a_minute() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let quiet = (0..8).map(|_| Err(io::Error::from(ErrorKind::TimedOut)));
        let mut connection = Some(Fake {
            reads: quiet.collect(),
            written: Arc::clone(&written),
        });
        // Every look at the clock is 25 s later.
        let now = Cell::new(1000);
        let clock = || {
            now.set(now.get() + 25);
            now.get()
        };
        run(
            "node:4403",
            |_| {
                connection
                    .take()
                    .ok_or_else(|| io::Error::from(ErrorKind::ConnectionRefused))
            },
            |update| !matches!(update, MeshUpdate::Lost(_)),
            &AtomicBool::new(false),
            clock,
            &NOW,
        );
        let sent = sent(&written.lock().unwrap());
        assert!(matches!(sent[0], to_radio::PayloadVariant::WantConfigId(_)));
        let beats = sent[1..]
            .iter()
            .filter(|m| matches!(m, to_radio::PayloadVariant::Heartbeat(_)));
        assert_eq!(beats.count(), 2, "{sent:?}");
    }

    #[test]
    fn the_stop_flag_ends_it_without_a_word() {
        let stop = AtomicBool::new(true);
        let mut updates = Vec::new();
        run(
            "node:4403",
            |_| -> io::Result<Fake> { panic!("no connection once stopped") },
            |update| {
                updates.push(update);
                true
            },
            &stop,
            || 0,
            &NOW,
        );
        assert!(updates.is_empty());
    }

    /// A real node: `SDR_MESH_ADDR=192.168.1.246 cargo test -p sdr-mesh
    /// -- --ignored --nocapture live_` prints what 15 s of it bring.
    #[test]
    #[ignore = "needs a meshtasticd"]
    fn live_meshtasticd() {
        let addr = with_port(&std::env::var("SDR_MESH_ADDR").unwrap_or("localhost".into()));
        let clock = || {
            let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            since.map_or(0, |d| d.as_secs())
        };
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_secs(15));
                stop.store(true, Ordering::Relaxed);
            });
            let print = |update| {
                println!("{update:?}");
                true
            };
            run(&addr, tcp, print, &stop, clock, &Timing::default());
        });
    }

    #[test]
    fn localhost_reaches_a_daemon_that_listens_on_ipv4_only() {
        // meshtasticd listens on 0.0.0.0; "localhost" may be ::1 first.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = tcp(&format!("localhost:{port}")).unwrap();
        assert!(stream.peer_addr().unwrap().is_ipv4());
    }

    #[test]
    fn an_address_without_a_port_gets_the_apis() {
        assert_eq!(with_port("localhost"), "localhost:4403");
        assert_eq!(with_port("192.168.1.246:4404"), "192.168.1.246:4404");
    }
}
