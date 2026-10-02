//! Network condition simulator: delays, jitters and drops datagrams in both directions.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions {
    /// Added one-way delay in each direction (RTT grows by twice this).
    pub latency_ms: u32,
    /// Uniform random extra delay 0..=jitter_ms per packet.
    pub jitter_ms: u32,
    /// Drop probability per packet, 0-100.
    pub loss_pct: f32,
}

impl Conditions {
    pub fn to_json(self) -> Value {
        json!({ "latency_ms": self.latency_ms, "jitter_ms": self.jitter_ms, "loss_pct": self.loss_pct })
    }
}

pub struct Delayed {
    pub due: Instant,
    pub addr: SocketAddr,
    pub data: Vec<u8>,
}

pub struct NetSim {
    pub conditions: Conditions,
    rng: u64,
    queue: Vec<Delayed>,
    pub dropped: u64,
}

impl NetSim {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) ^ 0x9E37_79B9_7F4A_7C15;
        Self { conditions: Conditions::default(), rng: seed | 1, queue: Vec::new(), dropped: 0 }
    }

    fn next_f32(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Returns false when the packet was dropped.
    pub fn push(&mut self, addr: SocketAddr, data: Vec<u8>) -> bool {
        let c = self.conditions;
        if c.loss_pct > 0.0 && self.next_f32() * 100.0 < c.loss_pct {
            self.dropped += 1;
            return false;
        }
        let jitter = if c.jitter_ms > 0 { (self.next_f32() * (c.jitter_ms as f32 + 1.0)) as u64 } else { 0 };
        let due = Instant::now() + Duration::from_millis(c.latency_ms as u64 + jitter);
        self.queue.push(Delayed { due, addr, data });
        true
    }

    /// Packets whose delay has elapsed, in due order.
    pub fn take_due(&mut self) -> Vec<Delayed> {
        let now = Instant::now();
        let (mut due, rest): (Vec<_>, Vec<_>) = self.queue.drain(..).partition(|d| d.due <= now);
        self.queue = rest;
        due.sort_by_key(|d| d.due);
        due
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}
