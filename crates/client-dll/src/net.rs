//! Network thread: join, send the local state at TICK_HZ, receive snapshots.

use std::collections::{BTreeMap, VecDeque};
use std::net::UdpSocket;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use shared::reliable::{ReliableIn, ReliableOut};
use shared::*;

pub struct Remote {
    pub name: String,
    pub state: PlayerState,
    pub received: Instant,
    /// Age of `state` (ms) when the server sent it, see `PlayerSnapshot::delay_ms`.
    pub delay_ms: u32,
    pub updates: u64,
}

#[derive(Default)]
pub struct Stats {
    pub sent: u64,
    pub received: u64,
    pub snapshots: u64,
    pub last_tick: u32,
    pub decode_errors: u64,
}

pub struct Net {
    pub my_id: Option<PlayerId>,
    pub status: String,
    /// Latest local state from the game thread; `seq` is assigned when sending.
    pub local: Option<PlayerState>,
    pub remotes: BTreeMap<PlayerId, Remote>,
    pub stats: Stats,
    pub last_server_packet: Option<Instant>,
    /// Latest server clock stamp and when it arrived (echoed back in Sync).
    pub last_server_ms: u32,
    pub last_server_ms_at: Option<Instant>,
    /// Smoothed round-trip time to the server (ms).
    pub rtt_ms: Option<f32>,
    pub rel_out: ReliableOut<ClientEvent>,
    pub rel_in: ReliableIn<ServerEvent>,
    /// Reliable server events delivered in order, consumed by the game thread.
    pub events: VecDeque<ServerEvent>,
}

impl Net {
    pub fn one_way_ms(&self) -> u32 {
        self.rtt_ms.map(|r| (r / 2.0) as u32).unwrap_or(0)
    }
}

/// Client clock for echo stamps; never 0.
fn now_ms() -> u32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    (START.get_or_init(Instant::now).elapsed().as_millis() as u32).max(1)
}

pub static NET: Mutex<Net> = Mutex::new(Net {
    my_id: None,
    status: String::new(),
    local: None,
    remotes: BTreeMap::new(),
    stats: Stats { sent: 0, received: 0, snapshots: 0, last_tick: 0, decode_errors: 0 },
    last_server_packet: None,
    last_server_ms: 0,
    last_server_ms_at: None,
    rtt_ms: None,
    rel_out: ReliableOut::new(),
    rel_in: ReliableIn::new(),
    events: VecDeque::new(),
});

/// Queues a reliable event for the server (any thread).
pub fn send_event(ev: ClientEvent) {
    NET.lock().unwrap().rel_out.push(ev);
}

pub fn set_status(s: String) {
    NET.lock().unwrap().status = s;
}

const HELLO_EVERY: Duration = Duration::from_millis(500);

pub fn run() {
    let cfg = crate::CONFIG.get().unwrap();
    let socket = match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect(&cfg.server).map(|_| s)) {
        Ok(s) => s,
        Err(e) => return set_status(format!("socket error: {e}")),
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(5)));
    set_status(format!("connecting to {}", cfg.server));

    let send_every = Duration::from_millis(1000 / TICK_HZ as u64);
    let mut next_send = Instant::now();
    let mut last_hello: Option<Instant> = None;
    let mut last_keepalive = Instant::now();
    let mut seq: u32 = 0;
    let mut buf = [0u8; 4096];

    loop {
        // Outgoing.
        let now = Instant::now();
        let packet = {
            let n = NET.lock().unwrap();
            if n.my_id.is_none() {
                if last_hello.is_none_or(|t| t.elapsed() >= HELLO_EVERY) {
                    last_hello = Some(now);
                    Some(ClientPacket::Hello { version: PROTOCOL_VERSION, name: cfg.name.clone() })
                } else {
                    None
                }
            } else if now >= next_send {
                next_send = now + send_every;
                match n.local {
                    Some(mut s) => {
                        seq += 1;
                        s.seq = seq;
                        let echo = Echo {
                            sent_ms: now_ms(),
                            echo_ms: n.last_server_ms,
                            hold_ms: n.last_server_ms_at.map_or(0, |t| t.elapsed().as_millis() as u32),
                        };
                        Some(ClientPacket::Sync { state: s, echo })
                    }
                    None if last_keepalive.elapsed() >= Duration::from_millis(KEEPALIVE_MS) => {
                        last_keepalive = now;
                        Some(ClientPacket::KeepAlive)
                    }
                    None => None,
                }
            } else {
                None
            }
        };
        // Reliable channel (only once joined).
        let reliable: Vec<ClientPacket> = {
            let mut n = NET.lock().unwrap();
            if n.my_id.is_some() {
                let resend = Duration::from_millis(n.rtt_ms.map_or(150.0, |r| (r * 1.5).max(100.0)) as u64);
                n.rel_out.due(now, resend).into_iter().map(|(seq, msg)| ClientPacket::Reliable { seq, msg }).collect()
            } else {
                Vec::new()
            }
        };
        for p in packet.into_iter().chain(reliable) {
            if socket.send(&encode(&p)).is_ok() {
                NET.lock().unwrap().stats.sent += 1;
            }
        }

        // Incoming.
        if let Ok(len) = socket.recv(&mut buf) {
            if let Some(reply) = handle(&buf[..len]) {
                if socket.send(&encode(&reply)).is_ok() {
                    NET.lock().unwrap().stats.sent += 1;
                }
            }
        }

        // Lost the server: rejoin.
        let mut n = NET.lock().unwrap();
        if n.my_id.is_some() && n.last_server_packet.is_some_and(|t| t.elapsed() > Duration::from_millis(TIMEOUT_MS)) {
            n.my_id = None;
            n.remotes.clear();
            n.status = format!("lost connection to {}, rejoining", cfg.server);
        }
    }
}

/// Applies one server packet; returns a packet to send back (reliable acks).
fn handle(data: &[u8]) -> Option<ClientPacket> {
    let mut n = NET.lock().unwrap();
    let packet: ServerPacket = match decode(data) {
        Ok(p) => p,
        Err(_) => {
            n.stats.decode_errors += 1;
            return None;
        }
    };
    n.stats.received += 1;
    n.last_server_packet = Some(Instant::now());
    match packet {
        ServerPacket::Welcome { id, .. } => {
            if n.my_id.is_none() {
                n.my_id = Some(id);
                // A restarted server counts ticks from 0 again, and reliable sequence numbers
                // restart with every session.
                n.stats.last_tick = 0;
                n.rel_out = ReliableOut::new();
                n.rel_in = ReliableIn::new();
                n.status = format!("connected as player {id}");
            }
        }
        ServerPacket::Reject { reason } => {
            n.my_id = None;
            n.status = format!("rejected: {reason}");
        }
        ServerPacket::Reliable { seq, msg } => {
            let delivered = n.rel_in.receive(seq, msg);
            n.events.extend(delivered);
            return Some(ClientPacket::Ack { upto: n.rel_in.ack_value() });
        }
        ServerPacket::Ack { upto } => n.rel_out.ack(upto),

        ServerPacket::Snapshot { tick, echo, players } => {
            // Snapshots can arrive out of order under jitter; ignore older ones.
            if tick <= n.stats.last_tick {
                return None;
            }
            n.last_server_ms = echo.sent_ms;
            n.last_server_ms_at = Some(Instant::now());
            if let Some(sample) = echo.rtt_sample(now_ms()) {
                let sample = sample as f32;
                n.rtt_ms = Some(n.rtt_ms.map_or(sample, |r| r * 0.8 + sample * 0.2));
            }
            n.stats.last_tick = tick;
            n.stats.snapshots += 1;
            let now = Instant::now();
            n.remotes.retain(|id, _| players.iter().any(|p| p.id == *id));
            for p in players {
                let entry = n.remotes.entry(p.id).or_insert(Remote {
                    name: p.name.clone(),
                    state: p.state,
                    received: now,
                    delay_ms: p.delay_ms,
                    updates: 0,
                });
                if p.state.seq != entry.state.seq || entry.updates == 0 {
                    entry.received = now;
                    entry.updates += 1;
                }
                entry.name = p.name;
                entry.state = p.state;
                entry.delay_ms = p.delay_ms;
            }
        }
    }
    None
}
