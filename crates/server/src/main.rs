//! mini-samp server: UDP relay of player states plus a loopback admin interface.
//!
//! Usage: server [--port 7777] [--admin-port 7778] [--bind 0.0.0.0]

mod admin;
mod netsim;

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{info, warn};
use serde_json::{json, Value};
use shared::*;

use netsim::NetSim;

const PACKET_LOG_CAP: usize = 500;

#[derive(Default, Clone, Copy)]
pub struct Counters {
    pub packets_in: u64,
    pub packets_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

pub struct Client {
    pub id: PlayerId,
    pub addr: SocketAddr,
    pub name: String,
    pub state: Option<PlayerState>,
    pub last_seen: Instant,
    pub joined: Instant,
    pub counters: Counters,
    /// Sync packets that arrived with a seq not greater than the last one (reordered/duplicate).
    pub stale_syncs: u64,
    /// When the current `state` arrived.
    pub state_received: Instant,
    /// Latest client clock stamp and when it arrived (echoed back in snapshots).
    pub last_client_ms: u32,
    pub last_client_ms_at: Instant,
    /// Smoothed round-trip time (ms), None until measured.
    pub rtt_ms: Option<f32>,
}

impl Client {
    pub fn one_way_ms(&self) -> u32 {
        self.rtt_ms.map(|r| (r / 2.0) as u32).unwrap_or(0)
    }
}

pub struct LogEntry {
    pub t_ms: u64,
    pub dir: &'static str,
    pub client: Option<PlayerId>,
    pub addr: SocketAddr,
    pub kind: &'static str,
    pub bytes: usize,
    pub dropped: bool,
}

pub struct Server {
    pub started: Instant,
    pub port: u16,
    pub clients: HashMap<SocketAddr, Client>,
    next_id: PlayerId,
    pub tick: u32,
    /// Applied to inbound datagrams.
    pub sim_in: NetSim,
    /// Applied to outbound datagrams; conditions are kept equal to `sim_in`.
    pub sim_out: NetSim,
    pub log: VecDeque<LogEntry>,
    pub kinds: HashMap<&'static str, u64>,
    pub decode_errors: u64,
}

impl Server {
    fn new(port: u16) -> Self {
        Self {
            started: Instant::now(),
            port,
            clients: HashMap::new(),
            next_id: 1,
            tick: 0,
            sim_in: NetSim::new(),
            sim_out: NetSim::new(),
            log: VecDeque::new(),
            kinds: HashMap::new(),
            decode_errors: 0,
        }
    }

    fn record(&mut self, dir: &'static str, addr: SocketAddr, kind: &'static str, bytes: usize, dropped: bool) {
        let client = self.clients.get(&addr).map(|c| c.id);
        if self.log.len() == PACKET_LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            t_ms: self.started.elapsed().as_millis() as u64,
            dir,
            client,
            addr,
            kind,
            bytes,
            dropped,
        });
        *self.kinds.entry(kind).or_default() += 1;
    }

    fn send(&mut self, addr: SocketAddr, packet: &ServerPacket) {
        let data = encode(packet);
        let bytes = data.len();
        let delivered = self.sim_out.push(addr, data);
        if let Some(c) = self.clients.get_mut(&addr) {
            c.counters.packets_out += 1;
            c.counters.bytes_out += bytes as u64;
        }
        self.record("out", addr, packet.kind(), bytes, !delivered);
    }

    fn handle(&mut self, addr: SocketAddr, data: &[u8]) {
        let packet: ClientPacket = match decode(data) {
            Ok(p) => p,
            Err(e) => {
                self.decode_errors += 1;
                warn!("bad packet from {addr}: {e}");
                return;
            }
        };
        self.record("in", addr, packet.kind(), data.len(), false);
        if let Some(c) = self.clients.get_mut(&addr) {
            c.last_seen = Instant::now();
            c.counters.packets_in += 1;
            c.counters.bytes_in += data.len() as u64;
        }

        match packet {
            ClientPacket::Hello { version, name } => {
                if version != PROTOCOL_VERSION {
                    self.send(addr, &ServerPacket::Reject { reason: format!("protocol {version} != {PROTOCOL_VERSION}") });
                    return;
                }
                if let Some(id) = self.clients.get(&addr).map(|c| c.id) {
                    // Welcome was lost; resend.
                    self.send(addr, &ServerPacket::Welcome { id, tick_hz: TICK_HZ });
                    return;
                }
                if self.clients.len() >= MAX_PLAYERS {
                    self.send(addr, &ServerPacket::Reject { reason: "server full".into() });
                    return;
                }
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                let name: String = name.chars().take(MAX_NAME_LEN).collect();
                info!("player {id} '{name}' joined from {addr}");
                self.clients.insert(
                    addr,
                    Client {
                        id,
                        addr,
                        name,
                        state: None,
                        last_seen: Instant::now(),
                        joined: Instant::now(),
                        counters: Counters { packets_in: 1, bytes_in: data.len() as u64, ..Default::default() },
                        stale_syncs: 0,
                        state_received: Instant::now(),
                        last_client_ms: 0,
                        last_client_ms_at: Instant::now(),
                        rtt_ms: None,
                    },
                );
                self.send(addr, &ServerPacket::Welcome { id, tick_hz: TICK_HZ });
            }
            ClientPacket::Sync { state, echo } => {
                let now_ms = self.now_ms();
                if let Some(c) = self.clients.get_mut(&addr) {
                    match c.state {
                        Some(prev) if state.seq <= prev.seq => c.stale_syncs += 1,
                        _ => {
                            c.state = Some(state);
                            c.state_received = Instant::now();
                            c.last_client_ms = echo.sent_ms;
                            c.last_client_ms_at = Instant::now();
                            if let Some(sample) = echo.rtt_sample(now_ms) {
                                let sample = sample as f32;
                                c.rtt_ms = Some(c.rtt_ms.map_or(sample, |r| r * 0.8 + sample * 0.2));
                            }
                        }
                    }
                }
            }
            ClientPacket::KeepAlive => {}
            ClientPacket::Bye => {
                if let Some(c) = self.clients.remove(&addr) {
                    info!("player {} '{}' left", c.id, c.name);
                }
            }
        }
    }

    /// Server clock for echo stamps; never 0 (0 means "no stamp").
    pub fn now_ms(&self) -> u32 {
        (self.started.elapsed().as_millis() as u32).max(1)
    }

    fn tick(&mut self) {
        self.tick += 1;
        let timeout = Duration::from_millis(TIMEOUT_MS);
        let gone: Vec<SocketAddr> = self.clients.values().filter(|c| c.last_seen.elapsed() > timeout).map(|c| c.addr).collect();
        for addr in gone {
            if let Some(c) = self.clients.remove(&addr) {
                info!("player {} '{}' timed out", c.id, c.name);
            }
        }

        let all: Vec<PlayerSnapshot> = self
            .clients
            .values()
            .filter_map(|c| {
                c.state.map(|state| PlayerSnapshot {
                    id: c.id,
                    name: c.name.clone(),
                    state,
                    delay_ms: c.one_way_ms() + c.state_received.elapsed().as_millis() as u32,
                })
            })
            .collect();
        let now_ms = self.now_ms();
        let targets: Vec<(SocketAddr, PlayerId, Echo)> = self
            .clients
            .values()
            .map(|c| {
                let echo = Echo {
                    sent_ms: now_ms,
                    echo_ms: c.last_client_ms,
                    hold_ms: c.last_client_ms_at.elapsed().as_millis() as u32,
                };
                (c.addr, c.id, echo)
            })
            .collect();
        for (addr, id, echo) in targets {
            let players = all.iter().filter(|p| p.id != id).cloned().collect();
            self.send(addr, &ServerPacket::Snapshot { tick: self.tick, echo, players });
        }
    }

    pub fn status_json(&self) -> Value {
        let mut players: Vec<Value> = self
            .clients
            .values()
            .map(|c| {
                json!({
                    "id": c.id,
                    "name": c.name,
                    "addr": c.addr.to_string(),
                    "connected_s": c.joined.elapsed().as_secs(),
                    "last_seen_ms": c.last_seen.elapsed().as_millis() as u64,
                    "state": c.state.map(|s| json!({
                        "seq": s.seq,
                        "pos": s.pos,
                        "vel": s.vel,
                        "heading": s.heading,
                        "health": s.health,
                        "move_state": format!("{:?}", s.move_state),
                    })),
                    "packets_in": c.counters.packets_in,
                    "packets_out": c.counters.packets_out,
                    "bytes_in": c.counters.bytes_in,
                    "bytes_out": c.counters.bytes_out,
                    "stale_syncs": c.stale_syncs,
                    "rtt_ms": c.rtt_ms.map(|r| r.round()),
                })
            })
            .collect();
        players.sort_by_key(|p| p["id"].as_u64());
        json!({
            "port": self.port,
            "uptime_s": self.started.elapsed().as_secs(),
            "tick": self.tick,
            "tick_hz": TICK_HZ,
            "players": players,
            "netsim": self.sim_in.conditions.to_json(),
            "netsim_dropped": self.sim_in.dropped + self.sim_out.dropped,
            "netsim_queued": self.sim_in.queued() + self.sim_out.queued(),
            "packet_kinds": self.kinds,
            "decode_errors": self.decode_errors,
        })
    }

    pub fn kick(&mut self, id: PlayerId) -> bool {
        let addr = self.clients.values().find(|c| c.id == id).map(|c| c.addr);
        match addr {
            Some(a) => {
                self.send(a, &ServerPacket::Reject { reason: "kicked".into() });
                self.clients.remove(&a);
                true
            }
            None => false,
        }
    }
}

fn arg<T: std::str::FromStr>(name: &str, default: T) -> T {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let port = arg("--port", DEFAULT_PORT);
    let admin_port = arg("--admin-port", DEFAULT_ADMIN_PORT);

    let bind: String = arg("--bind", "0.0.0.0".to_string());
    let socket = UdpSocket::bind((bind.as_str(), port)).with_context(|| format!("bind UDP {bind}:{port}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(2)))?;
    let server = Arc::new(Mutex::new(Server::new(port)));
    admin::spawn(admin_port, server.clone())?;
    info!("mini-samp server on UDP {bind}:{port}, admin on 127.0.0.1:{admin_port}");

    let tick_every = Duration::from_millis(1000 / TICK_HZ as u64);
    let mut next_tick = Instant::now() + tick_every;
    let mut buf = [0u8; 2048];
    loop {
        let received = match socket.recv_from(&mut buf) {
            Ok((n, addr)) => Some((addr, buf[..n].to_vec())),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => None,
            // Windows reports ICMP port-unreachable from a vanished client as a recv error.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => None,
            Err(e) => return Err(e.into()),
        };

        let mut s = server.lock().unwrap();
        if let Some((addr, data)) = received {
            if !s.sim_in.push(addr, data) {
                s.record("in", addr, "Dropped", 0, true);
            }
        }
        for d in s.sim_in.take_due() {
            s.handle(d.addr, &d.data);
        }
        if Instant::now() >= next_tick {
            next_tick += tick_every;
            s.tick();
        }
        for d in s.sim_out.take_due() {
            let _ = socket.send_to(&d.data, d.addr);
        }
    }
}
