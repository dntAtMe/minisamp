//! Authoritative turn-based battle engine. Pure logic: time is passed in, randomness comes from
//! a seeded generator, and every state change is reported as `ServerEvent`s for the clients.
//!
//! Flow: `start` -> TurnPrompt(actor) -> [player chooses | AI acts | timeout guards]
//! -> ActionResult -> (show for SHOW_MS) -> next living actor ... -> BattleEnd.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shared::battle::*;
use shared::PlayerId;

pub const PLAYER_HP: i32 = 100;
pub const TURN_TIMEOUT: Duration = Duration::from_secs(30);
/// Time between an action result and the next prompt, for clients to present it.
pub const SHOW: Duration = Duration::from_millis(1500);

/// Enemy roster: (name, ped model, hp).
const ENEMIES: [(&str, i32, i32); 3] = [("Ballas Thug", 102, 60), ("Ballas Enforcer", 103, 80), ("Ballas OG", 104, 70)];

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in lo..=hi.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
    pub fn chance(&mut self, pct: i32) -> bool {
        self.range(1, 100) <= pct
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Await { actor: Cid, since: Instant },
    Showing { until: Instant },
    Done(Outcome),
}

pub struct Battle {
    pub id: BattleId,
    pub arena: [f32; 3],
    pub heading: f32,
    pub combatants: Vec<Combatant>,
    guarding: Vec<Cid>,
    /// Index into `combatants` of whoever acted last.
    turn: usize,
    pub round: u32,
    pub phase: Phase,
    rng: Rng,
    pub log: Vec<String>,
}

impl Battle {
    /// Creates the battle; returns it with the events to broadcast (BattleStart + first prompt).
    pub fn start(
        id: BattleId,
        players: &[(PlayerId, String)],
        enemies: usize,
        arena: [f32; 3],
        heading: f32,
        seed: u64,
        now: Instant,
    ) -> (Self, Vec<ServerEvent>) {
        let mut combatants = Vec::new();
        for (pid, name) in players {
            combatants.push(Combatant {
                cid: combatants.len() as Cid,
                side: Side::Party,
                player: Some(*pid),
                name: name.clone(),
                model: 0,
                hp: PLAYER_HP,
                max_hp: PLAYER_HP,
            });
        }
        for i in 0..enemies.clamp(1, 3) {
            let (name, model, hp) = ENEMIES[i];
            combatants.push(Combatant {
                cid: combatants.len() as Cid,
                side: Side::Enemies,
                player: None,
                name: name.into(),
                model,
                hp,
                max_hp: hp,
            });
        }
        let mut b = Battle {
            id,
            arena,
            heading,
            combatants,
            guarding: Vec::new(),
            turn: usize::MAX,
            round: 0,
            phase: Phase::Showing { until: now },
            rng: Rng::new(seed),
            log: Vec::new(),
        };
        let mut events = vec![ServerEvent::BattleStart { battle: id, arena, heading, combatants: b.combatants.clone() }];
        events.extend(b.advance(now));
        (b, events)
    }

    fn alive(&self, side: Side) -> impl Iterator<Item = &Combatant> {
        self.combatants.iter().filter(move |c| c.side == side && c.hp > 0)
    }

    pub fn participants(&self) -> Vec<PlayerId> {
        self.combatants.iter().filter_map(|c| c.player).collect()
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done(_))
    }

    /// Moves to the next living combatant; enemies act immediately.
    fn advance(&mut self, now: Instant) -> Vec<ServerEvent> {
        if let Some(outcome) = self.check_end() {
            return self.finish(outcome);
        }
        let n = self.combatants.len();
        for _ in 0..n {
            self.turn = if self.turn == usize::MAX { 0 } else { (self.turn + 1) % n };
            if self.turn == 0 {
                self.round += 1;
            }
            let c = &self.combatants[self.turn];
            if c.hp > 0 {
                let actor = c.cid;
                self.guarding.retain(|g| *g != actor);
                return match c.side {
                    Side::Enemies => self.enemy_turn(actor, now),
                    Side::Party => {
                        self.phase = Phase::Await { actor, since: now };
                        vec![ServerEvent::TurnPrompt {
                            battle: self.id,
                            round: self.round,
                            actor,
                            skills: Skill::PLAYER_SKILLS.to_vec(),
                            timeout_ms: TURN_TIMEOUT.as_millis() as u32,
                        }]
                    }
                };
            }
        }
        self.finish(Outcome::Aborted)
    }

    fn enemy_turn(&mut self, actor: Cid, now: Instant) -> Vec<ServerEvent> {
        let targets: Vec<Cid> = self.alive(Side::Party).map(|c| c.cid).collect();
        let target = targets[self.rng.range(0, targets.len() as i32 - 1) as usize];
        self.resolve(actor, Skill::Attack, Some(target), now)
    }

    fn check_end(&self) -> Option<Outcome> {
        if self.alive(Side::Enemies).next().is_none() {
            Some(Outcome::Victory)
        } else if self.alive(Side::Party).next().is_none() {
            Some(Outcome::Defeat)
        } else {
            None
        }
    }

    fn finish(&mut self, outcome: Outcome) -> Vec<ServerEvent> {
        self.phase = Phase::Done(outcome);
        self.log.push(format!("battle ended: {outcome:?}"));
        vec![ServerEvent::BattleEnd { battle: self.id, outcome }]
    }

    fn cmb(&self, cid: Cid) -> Option<&Combatant> {
        self.combatants.iter().find(|c| c.cid == cid)
    }

    /// A player's choice. Errors leave the battle untouched.
    pub fn choose(
        &mut self,
        player: Option<PlayerId>,
        actor: Cid,
        skill: Skill,
        target: Option<Cid>,
        now: Instant,
    ) -> Result<Vec<ServerEvent>, String> {
        let Phase::Await { actor: current, .. } = self.phase else { return Err("not waiting for an action".into()) };
        if actor != current {
            return Err(format!("it is combatant {current}'s turn, not {actor}'s"));
        }
        let owner = self.cmb(actor).and_then(|c| c.player);
        if player.is_some() && player != owner {
            return Err(format!("combatant {actor} is not controlled by player {player:?}"));
        }
        let me = self.cmb(actor).unwrap().side;
        match (skill.target_kind(), target.and_then(|t| self.cmb(t))) {
            (TargetKind::None, _) => {}
            (_, None) => return Err(format!("{} needs a target", skill.name())),
            (_, Some(t)) if t.hp <= 0 => return Err(format!("{} is down", t.name)),
            (TargetKind::Enemy, Some(t)) if t.side == me => return Err(format!("{} is not an enemy", t.name)),
            (TargetKind::Ally, Some(t)) if t.side != me => return Err(format!("{} is not an ally", t.name)),
            _ => {}
        }
        Ok(self.resolve(actor, skill, target, now))
    }

    fn resolve(&mut self, actor: Cid, skill: Skill, target: Option<Cid>, now: Instant) -> Vec<ServerEvent> {
        let actor_name = self.cmb(actor).unwrap().name.clone();
        let target_name = target.and_then(|t| self.cmb(t)).map(|c| c.name.clone()).unwrap_or_default();
        let is_enemy = self.cmb(actor).unwrap().side == Side::Enemies;
        let (mut amount, mut hit) = (0, true);
        let text = match skill {
            Skill::Attack | Skill::Fire => {
                let (pct, lo, hi) = match (skill, is_enemy) {
                    (Skill::Fire, _) => (80, 16, 24),
                    (_, true) => (85, 8, 14),
                    _ => (90, 10, 16),
                };
                hit = self.rng.chance(pct);
                if hit {
                    let t = target.unwrap();
                    amount = self.rng.range(lo, hi);
                    if self.guarding.contains(&t) {
                        amount /= 2;
                    }
                    self.damage(t, amount);
                    format!("{actor_name} uses {} on {target_name}: {amount} damage", skill.name())
                } else {
                    format!("{actor_name} uses {} on {target_name}: miss", skill.name())
                }
            }
            Skill::Heal => {
                amount = self.rng.range(20, 30);
                let t = target.unwrap();
                self.damage(t, -amount);
                format!("{actor_name} heals {target_name}: +{amount} HP")
            }
            Skill::Guard => {
                self.guarding.push(actor);
                format!("{actor_name} guards")
            }
            Skill::Run => {
                hit = self.rng.chance(50);
                if hit {
                    format!("{actor_name} runs away!")
                } else {
                    format!("{actor_name} tries to run: blocked")
                }
            }
        };
        self.log.push(text.clone());
        let mut events = vec![ServerEvent::ActionResult {
            battle: self.id,
            actor,
            skill,
            target,
            amount,
            hit,
            hp: self.combatants.iter().map(|c| (c.cid, c.hp)).collect(),
            text,
        }];
        if skill == Skill::Run && hit {
            events.extend(self.finish(Outcome::Fled));
        } else {
            self.phase = Phase::Showing { until: now + SHOW };
        }
        events
    }

    fn damage(&mut self, cid: Cid, amount: i32) {
        if let Some(c) = self.combatants.iter_mut().find(|c| c.cid == cid) {
            c.hp = (c.hp - amount).clamp(0, c.max_hp);
        }
    }

    /// Advances timers: end of presentation, turn timeout (auto-guard).
    pub fn tick(&mut self, now: Instant) -> Vec<ServerEvent> {
        match self.phase {
            Phase::Showing { until } if now >= until => self.advance(now),
            Phase::Await { actor, since } if now.duration_since(since) >= TURN_TIMEOUT => {
                self.log.push(format!("{actor} timed out"));
                self.resolve(actor, Skill::Guard, None, now)
            }
            _ => Vec::new(),
        }
    }

    /// A participating player left: their combatant is knocked out.
    pub fn player_left(&mut self, player: PlayerId, now: Instant) -> Vec<ServerEvent> {
        let Some(cid) = self.combatants.iter().find(|c| c.player == Some(player)).map(|c| c.cid) else { return Vec::new() };
        self.damage(cid, i32::MAX / 2);
        if matches!(self.phase, Phase::Await { actor, .. } if actor == cid) {
            return self.advance(now);
        }
        if let Some(o) = self.check_end() {
            return self.finish(o);
        }
        Vec::new()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "round": self.round,
            "phase": match self.phase {
                Phase::Await { actor, .. } => json!({ "await": actor }),
                Phase::Showing { .. } => json!("showing"),
                Phase::Done(o) => json!({ "done": format!("{o:?}") }),
            },
            "combatants": self.combatants.iter().map(|c| json!({
                "cid": c.cid, "side": format!("{:?}", c.side), "player": c.player, "name": c.name,
                "hp": c.hp, "max_hp": c.max_hp, "guarding": self.guarding.contains(&c.cid),
            })).collect::<Vec<_>>(),
            "log": self.log.iter().rev().take(10).rev().collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party() -> Vec<(PlayerId, String)> {
        vec![(1, "Alice".into()), (2, "Bob".into())]
    }

    fn prompt_actor(events: &[ServerEvent]) -> Option<Cid> {
        events.iter().find_map(|e| match e {
            ServerEvent::TurnPrompt { actor, .. } => Some(*actor),
            _ => None,
        })
    }

    #[test]
    fn starts_with_first_player_prompt() {
        let now = Instant::now();
        let (b, ev) = Battle::start(1, &party(), 1, [0.0; 3], 0.0, 42, now);
        assert!(matches!(ev[0], ServerEvent::BattleStart { .. }));
        assert_eq!(prompt_actor(&ev), Some(0));
        assert_eq!(b.combatants.len(), 3);
    }

    #[test]
    fn rejects_wrong_actor_owner_and_target() {
        let now = Instant::now();
        let (mut b, _) = Battle::start(1, &party(), 1, [0.0; 3], 0.0, 42, now);
        assert!(b.choose(Some(1), 1, Skill::Guard, None, now).is_err()); // not cid 1's turn
        assert!(b.choose(Some(2), 0, Skill::Guard, None, now).is_err()); // Bob can't act for Alice
        assert!(b.choose(Some(1), 0, Skill::Attack, Some(1), now).is_err()); // ally as enemy target
        assert!(b.choose(Some(1), 0, Skill::Attack, None, now).is_err()); // missing target
        assert!(b.choose(Some(1), 0, Skill::Attack, Some(2), now).is_ok());
    }

    #[test]
    fn full_battle_reaches_an_end_and_rounds_cycle() {
        let mut now = Instant::now();
        let (mut b, ev) = Battle::start(7, &party(), 2, [0.0; 3], 0.0, 1234, now);
        let mut next = prompt_actor(&ev);
        let mut steps = 0;
        while !b.is_done() && steps < 500 {
            steps += 1;
            if let Some(actor) = next.take() {
                let target = b.alive(Side::Enemies).next().map(|c| c.cid);
                let ev = b.choose(None, actor, Skill::Fire, target, now).unwrap();
                next = prompt_actor(&ev);
            }
            now += SHOW;
            let ev = b.tick(now);
            if next.is_none() {
                next = prompt_actor(&ev);
            }
        }
        assert!(b.is_done(), "battle did not finish");
        assert!(b.round >= 2);
        assert!(matches!(b.phase, Phase::Done(Outcome::Victory) | Phase::Done(Outcome::Defeat)));
    }

    #[test]
    fn timeout_auto_guards_and_moves_on() {
        let now = Instant::now();
        let (mut b, _) = Battle::start(1, &party(), 1, [0.0; 3], 0.0, 42, now);
        let ev = b.tick(now + TURN_TIMEOUT);
        assert!(matches!(&ev[0], ServerEvent::ActionResult { skill: Skill::Guard, actor: 0, .. }));
        let ev = b.tick(now + TURN_TIMEOUT + SHOW);
        assert_eq!(prompt_actor(&ev), Some(1));
    }

    #[test]
    fn same_seed_same_battle() {
        let run = |seed| {
            let now = Instant::now();
            let (mut b, _) = Battle::start(1, &party(), 1, [0.0; 3], 0.0, seed, now);
            b.choose(None, 0, Skill::Attack, Some(2), now).unwrap();
            b.combatants[2].hp
        };
        assert_eq!(run(99), run(99));
    }

    #[test]
    fn leaving_player_is_knocked_out_and_battle_continues() {
        let now = Instant::now();
        let (mut b, _) = Battle::start(1, &party(), 1, [0.0; 3], 0.0, 42, now);
        let ev = b.player_left(1, now);
        assert_eq!(prompt_actor(&ev), Some(1));
        let ev = b.player_left(2, now);
        assert!(matches!(ev.last(), Some(ServerEvent::BattleEnd { outcome: Outcome::Defeat, .. })));
    }
}
