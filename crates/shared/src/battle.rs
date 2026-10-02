//! Turn-based battle messages. The server owns the battle (rules, RNG, AI); clients only
//! present it and send the local player's choices. All battle traffic uses the reliable
//! channel, since every event must arrive exactly once and in order.

use serde::{Deserialize, Serialize};

use crate::PlayerId;

pub type BattleId = u32;
/// Combatant id, unique within one battle.
pub type Cid = u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Party,
    Enemies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Skill {
    Attack,
    Fire,
    Heal,
    Guard,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Enemy,
    Ally,
    None,
}

impl Skill {
    pub const PLAYER_SKILLS: [Skill; 5] = [Skill::Attack, Skill::Fire, Skill::Heal, Skill::Guard, Skill::Run];

    pub fn name(self) -> &'static str {
        match self {
            Skill::Attack => "Attack",
            Skill::Fire => "Fire",
            Skill::Heal => "Heal",
            Skill::Guard => "Guard",
            Skill::Run => "Run",
        }
    }

    pub fn target_kind(self) -> TargetKind {
        match self {
            Skill::Attack | Skill::Fire => TargetKind::Enemy,
            Skill::Heal => TargetKind::Ally,
            Skill::Guard | Skill::Run => TargetKind::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Combatant {
    pub cid: Cid,
    pub side: Side,
    /// Owning player for party members.
    pub player: Option<PlayerId>,
    pub name: String,
    /// Ped model clients spawn for enemies (party members are the players themselves).
    pub model: i32,
    pub hp: i32,
    pub max_hp: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Victory,
    Defeat,
    Fled,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerEvent {
    BattleStart { battle: BattleId, arena: [f32; 3], heading: f32, combatants: Vec<Combatant> },
    /// `actor` is up; its owner should choose one of `skills` within `timeout_ms`.
    TurnPrompt { battle: BattleId, round: u32, actor: Cid, skills: Vec<Skill>, timeout_ms: u32 },
    /// One resolved action. `hp` carries the authoritative hp of every combatant afterwards.
    ActionResult {
        battle: BattleId,
        actor: Cid,
        skill: Skill,
        target: Option<Cid>,
        amount: i32,
        hit: bool,
        hp: Vec<(Cid, i32)>,
        text: String,
    },
    BattleEnd { battle: BattleId, outcome: Outcome },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientEvent {
    /// Start a battle around the sender (server picks nearby players and enemies).
    RequestBattle,
    ChooseAction { battle: BattleId, actor: Cid, skill: Skill, target: Option<Cid> },
}

/// Unit vector the game's heading (degrees, 0 = north, counter-clockwise) points along.
pub fn heading_forward(heading: f32) -> [f32; 2] {
    let r = heading.to_radians();
    [-r.sin(), r.cos()]
}

/// Where combatant `index` of `count` on `side` stands, and which way it faces. The party
/// lines up 3 m behind the arena centre facing `heading`, enemies 3 m in front facing back.
pub fn slot(arena: [f32; 3], heading: f32, side: Side, index: usize, count: usize) -> ([f32; 3], f32) {
    let f = heading_forward(heading);
    let right = [f[1], -f[0]];
    let (depth, facing) = match side {
        Side::Party => (-3.0, heading),
        Side::Enemies => (3.0, (heading + 180.0).rem_euclid(360.0)),
    };
    let lateral = (index as f32 - (count as f32 - 1.0) / 2.0) * 1.8;
    let pos = [
        arena[0] + f[0] * depth + right[0] * lateral,
        arena[1] + f[1] * depth + right[1] * lateral,
        arena[2],
    ];
    (pos, facing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_face_each_other() {
        let (p, ph) = slot([0.0, 0.0, 10.0], 0.0, Side::Party, 0, 1);
        let (e, eh) = slot([0.0, 0.0, 10.0], 0.0, Side::Enemies, 0, 1);
        assert!((p[1] + 3.0).abs() < 1e-4 && (e[1] - 3.0).abs() < 1e-4);
        assert_eq!((ph, eh), (0.0, 180.0));
        let (a, _) = slot([0.0; 3], 0.0, Side::Party, 0, 2);
        let (b, _) = slot([0.0; 3], 0.0, Side::Party, 1, 2);
        assert!((a[0] - b[0]).abs() > 1.7);
    }
}
