//! Wire protocol between the mini-samp server and game clients.
//!
//! Transport: UDP, one bincode-encoded packet per datagram. No reliability layer: clients send
//! their latest state at [`TICK_HZ`], the server answers with full snapshots of everyone else at
//! the same rate, so a lost packet is simply superseded by the next one. Joining is retried
//! (`Hello` until `Welcome`), leaving is `Bye` or a timeout.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 7777;
/// JSON-lines admin/debug interface of the server (loopback only).
pub const DEFAULT_ADMIN_PORT: u16 = 7778;
pub const PROTOCOL_VERSION: u16 = 2;
pub const TICK_HZ: u32 = 20;
pub const MAX_PLAYERS: usize = 32;
/// Clients that send nothing for this long are dropped.
pub const TIMEOUT_MS: u64 = 5000;
pub const MAX_NAME_LEN: usize = 24;
/// Joined clients without a player yet (loading, menus) send `KeepAlive` this often.
pub const KEEPALIVE_MS: u64 = 1000;

pub type PlayerId = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveState {
    Idle,
    Walk,
    Run,
    Sprint,
    Air,
}

impl MoveState {
    /// Classify from horizontal speed (m/s) and vertical speed.
    pub fn from_velocity(v: [f32; 3]) -> Self {
        let horizontal = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if v[2].abs() > 2.0 {
            MoveState::Air
        } else if horizontal < 0.4 {
            MoveState::Idle
        } else if horizontal < 3.0 {
            MoveState::Walk
        } else if horizontal < 6.5 {
            MoveState::Run
        } else {
            MoveState::Sprint
        }
    }
}

/// On-foot player state, sent by the owning client every tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    /// Client-side sequence number, increments every send.
    pub seq: u32,
    pub pos: [f32; 3],
    /// m/s
    pub vel: [f32; 3],
    /// degrees, 0 = north, counter-clockwise
    pub heading: f32,
    pub health: f32,
    pub armor: f32,
    pub move_state: MoveState,
}

/// Clock echo used to measure round-trip time without synchronised clocks: each side stamps
/// packets with its own millisecond clock and echoes the peer's latest stamp plus how long it
/// held it before replying, so `rtt = now - echo - hold`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Echo {
    /// Sender's clock (ms) when sending; never 0.
    pub sent_ms: u32,
    /// Latest `sent_ms` received from the peer (0 = none yet).
    pub echo_ms: u32,
    /// How long ago (ms) that peer stamp was received.
    pub hold_ms: u32,
}

impl Echo {
    /// RTT sample (ms) from an echo received at local time `now_ms`, if it carries one.
    pub fn rtt_sample(&self, now_ms: u32) -> Option<u32> {
        (self.echo_ms != 0).then(|| now_ms.wrapping_sub(self.echo_ms).saturating_sub(self.hold_ms))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientPacket {
    Hello { version: u16, name: String },
    Sync { state: PlayerState, echo: Echo },
    /// Sent while joined but not in the world, so the server does not time the client out.
    KeepAlive,
    Bye,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub name: String,
    pub state: PlayerState,
    /// Estimated age (ms) of `state` when the server sent this snapshot: the owner's one-way
    /// latency plus how long the server has held it. Receivers add their own one-way latency.
    pub delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerPacket {
    Welcome { id: PlayerId, tick_hz: u32 },
    Reject { reason: String },
    /// Everyone except the receiving player who has sent at least one state.
    Snapshot { tick: u32, echo: Echo, players: Vec<PlayerSnapshot> },
}

pub fn encode<T: Serialize>(packet: &T) -> Vec<u8> {
    bincode::serialize(packet).expect("serialization should not fail")
}

pub fn decode<'a, T: Deserialize<'a>>(data: &'a [u8]) -> Result<T, bincode::Error> {
    bincode::deserialize(data)
}

impl ClientPacket {
    pub fn kind(&self) -> &'static str {
        match self {
            ClientPacket::Hello { .. } => "Hello",
            ClientPacket::Sync { .. } => "Sync",
            ClientPacket::KeepAlive => "KeepAlive",
            ClientPacket::Bye => "Bye",
        }
    }
}

impl ServerPacket {
    pub fn kind(&self) -> &'static str {
        match self {
            ServerPacket::Welcome { .. } => "Welcome",
            ServerPacket::Reject { .. } => "Reject",
            ServerPacket::Snapshot { .. } => "Snapshot",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = PlayerState {
            seq: 7,
            pos: [1.0, 2.0, 3.0],
            vel: [0.0; 3],
            heading: 90.0,
            health: 100.0,
            armor: 0.0,
            move_state: MoveState::Run,
        };
        let p = ClientPacket::Sync { state: s, echo: Echo::default() };
        match decode::<ClientPacket>(&encode(&p)).unwrap() {
            ClientPacket::Sync { state: back, .. } => assert_eq!(back, s),
            other => panic!("wrong packet {other:?}"),
        }
    }

    #[test]
    fn rtt_from_echo() {
        let e = Echo { sent_ms: 0, echo_ms: 1000, hold_ms: 20 };
        assert_eq!(e.rtt_sample(1120), Some(100));
        assert_eq!(Echo::default().rtt_sample(5), None);
    }

    #[test]
    fn move_state_thresholds() {
        assert_eq!(MoveState::from_velocity([0.0, 0.0, 0.0]), MoveState::Idle);
        assert_eq!(MoveState::from_velocity([0.0, 1.5, 0.0]), MoveState::Walk);
        assert_eq!(MoveState::from_velocity([4.0, 0.0, 0.0]), MoveState::Run);
        assert_eq!(MoveState::from_velocity([0.0, 8.0, 0.0]), MoveState::Sprint);
        assert_eq!(MoveState::from_velocity([0.0, 0.0, 5.0]), MoveState::Air);
    }
}
