//! Client side of turn-based battles (game thread): applies server events, stages the arena
//! (teleport + freeze, enemy peds, fixed camera), and runs the command menu for the local
//! player's turns. Rules and RNG live on the server; this only presents and sends choices.
//!
//! Controls (pad 0, so sa-mcp `input` can play too): forward/back (W/S) move the cursor,
//! left/right (A/D) cycle targets, enter_exit (F/Enter) confirms, jump (Shift) goes back.
//! F5 (window focused) asks the server for a battle around the local player.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sa_sdk::addr::PADS;
use sa_sdk::script::{self, cmd, Arg::*};
use sa_sdk::world;
use shared::battle::*;
use shared::{PlayerId, ServerEvent};

use crate::net::{self, NET};
use crate::sfx::{self, Sfx};

/// set_char_coordinates / create_char take a ground position.
const CENTRE_TO_FEET: f32 = 1.0;
const MESSAGE_TTL: Duration = Duration::from_secs(6);
const OUTCOME_BANNER: Duration = Duration::from_secs(4);
const STICK_THRESHOLD: i16 = 64;

// CControllerState field indices (i16 each).
const PAD_STICK_X: usize = 0;
const PAD_STICK_Y: usize = 1;
const PAD_SQUARE: usize = 14;
const PAD_TRIANGLE: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Skill,
    Target,
}

pub struct Menu {
    pub stage: Stage,
    pub skill_idx: usize,
    pub target_idx: usize,
}

pub struct ClientBattle {
    pub id: BattleId,
    pub arena: [f32; 3],
    pub heading: f32,
    pub combatants: Vec<Combatant>,
    /// Combatants controlled by this client.
    pub mine: Vec<Cid>,
    pub round: u32,
    /// (actor, skills) of the current prompt.
    pub turn: Option<(Cid, Vec<Skill>)>,
    pub menu: Menu,
    /// A choice was sent; waiting for the server's ActionResult.
    pub sent: bool,
    pub messages: VecDeque<(String, Instant)>,
    pub enemy_peds: BTreeMap<Cid, u32>,
    pub outcome: Option<(Outcome, Instant)>,
    pub log: Vec<String>,
    /// FX systems to kill at the given time.
    pub fx: Vec<(u32, Instant)>,
}

/// A ped as script commands see it: the local player (resolved via get_player_char) or a
/// script handle.
#[derive(Clone, Copy)]
enum PedRef {
    LocalPlayer,
    Handle(u32),
}

/// Effect lifetimes.
const FX_TTL: Duration = Duration::from_millis(1600);

impl ClientBattle {
    pub fn cmb(&self, cid: Cid) -> Option<&Combatant> {
        self.combatants.iter().find(|c| c.cid == cid)
    }

    pub fn my_turn(&self) -> Option<Cid> {
        self.turn.as_ref().map(|t| t.0).filter(|a| self.mine.contains(a) && !self.sent)
    }

    /// Valid targets for the skill under the cursor.
    pub fn targets(&self) -> Vec<Cid> {
        let Some((actor, skills)) = &self.turn else { return Vec::new() };
        let Some(skill) = skills.get(self.menu.skill_idx) else { return Vec::new() };
        let my_side = self.cmb(*actor).map(|c| c.side).unwrap_or(Side::Party);
        self.combatants
            .iter()
            .filter(|c| c.hp > 0)
            .filter(|c| match skill.target_kind() {
                TargetKind::Enemy => c.side != my_side,
                TargetKind::Ally => c.side == my_side,
                TargetKind::None => false,
            })
            .map(|c| c.cid)
            .collect()
    }

    fn say(&mut self, text: String) {
        self.log.push(text.clone());
        self.messages.push_back((text, Instant::now()));
        while self.messages.len() > 4 {
            self.messages.pop_front();
        }
    }
}

pub static BATTLE: Mutex<Option<ClientBattle>> = Mutex::new(None);

#[derive(Default)]
struct InputEdge {
    stick_x: i16,
    stick_y: i16,
    confirm: bool,
    back: bool,
    f5: bool,
}

static EDGE: Mutex<InputEdge> = Mutex::new(InputEdge { stick_x: 0, stick_y: 0, confirm: false, back: false, f5: false });

/// Game thread, every in-game frame.
pub unsafe fn on_frame(my_id: Option<PlayerId>) {
    let events: Vec<ServerEvent> = NET.lock().unwrap().events.drain(..).collect();
    let mut guard = BATTLE.lock().unwrap();
    for ev in events {
        apply(&mut guard, ev, my_id);
    }
    // Drop the finished battle once its banner has been shown.
    if guard.as_ref().is_some_and(|b| b.outcome.is_some_and(|(_, t)| t.elapsed() > OUTCOME_BANNER)) {
        *guard = None;
    }
    if let Some(b) = guard.as_mut() {
        b.messages.retain(|(_, t)| t.elapsed() < MESSAGE_TTL);
        reap_fx(b, false);
    }
    handle_input(&mut guard);
}

unsafe fn apply(slot: &mut Option<ClientBattle>, ev: ServerEvent, my_id: Option<PlayerId>) {
    match ev {
        ServerEvent::BattleStart { battle, arena, heading, combatants } => {
            if let Some(mut old) = slot.take() {
                teardown(&mut old);
            }
            let mine = combatants.iter().filter(|c| c.player.is_some() && c.player == my_id).map(|c| c.cid).collect();
            let mut b = ClientBattle {
                id: battle,
                arena,
                heading,
                combatants,
                mine,
                round: 0,
                turn: None,
                menu: Menu { stage: Stage::Skill, skill_idx: 0, target_idx: 0 },
                sent: false,
                messages: VecDeque::new(),
                enemy_peds: BTreeMap::new(),
                outcome: None,
                log: Vec::new(),
                fx: Vec::new(),
            };
            stage(&mut b);
            let names: Vec<&str> = b.combatants.iter().filter(|c| c.side == Side::Enemies).map(|c| c.name.as_str()).collect();
            b.say(format!("{} appear!", names.join(", ")));
            *slot = Some(b);
        }
        ServerEvent::TurnPrompt { battle, round, actor, skills, .. } => {
            let Some(b) = slot.as_mut().filter(|b| b.id == battle) else { return };
            b.round = round;
            b.turn = Some((actor, skills));
            b.sent = false;
            b.menu = Menu { stage: Stage::Skill, skill_idx: 0, target_idx: 0 };
        }
        ServerEvent::ActionResult { battle, actor, skill, target, hit, hp, text, .. } => {
            let Some(b) = slot.as_mut().filter(|b| b.id == battle) else { return };
            present_action(b, actor, skill, target, hit);
            for (cid, value) in hp {
                if let Some(c) = b.combatants.iter_mut().find(|c| c.cid == cid) {
                    c.hp = value;
                }
            }
            if let Some(t) = target {
                let down = b.cmb(t).is_some_and(|c| c.hp == 0 && c.side == Side::Enemies);
                if let (true, Some(h)) = (down, b.enemy_peds.get(&t)) {
                    let _ = script::run(&[cmd(0x0321, vec![Int(*h as i32)])]); // kill_char
                }
            }
            b.turn = None;
            b.say(text);
        }
        ServerEvent::BattleEnd { battle, outcome } => {
            let Some(b) = slot.as_mut().filter(|b| b.id == battle) else { return };
            b.turn = None;
            b.outcome = Some((outcome, Instant::now()));
            b.say(format!("{outcome:?}!"));
            sfx::play(match outcome {
                Outcome::Victory | Outcome::Fled => Sfx::Victory,
                _ => Sfx::Defeat,
            });
            teardown(b);
        }
    }
}

/// Teleports the local player to its slot, freezes it, spawns enemies, sets the camera.
unsafe fn stage(b: &mut ClientBattle) {
    let count = |side| b.combatants.iter().filter(|c| c.side == side).count();
    let (parties, foes) = (count(Side::Party), count(Side::Enemies));
    let mut cmds = Vec::new();
    let (mut pi, mut ei) = (0, 0);
    let mut enemies = Vec::new();
    for c in &b.combatants {
        match c.side {
            Side::Party => {
                let (p, h) = slot(b.arena, b.heading, Side::Party, pi, parties);
                pi += 1;
                if b.mine.contains(&c.cid) {
                    cmds.push(cmd(0x01F5, vec![Int(0), Var(1)])); // get_player_char
                    cmds.push(cmd(0x00A1, vec![Var(1), Float(p[0]), Float(p[1]), Float(p[2] - CENTRE_TO_FEET)]));
                    cmds.push(cmd(0x0173, vec![Var(1), Float(h)]));
                    cmds.push(cmd(0x01B4, vec![Int(0), Int(0)])); // freeze player
                }
            }
            Side::Enemies => {
                let (p, h) = slot(b.arena, b.heading, Side::Enemies, ei, foes);
                ei += 1;
                enemies.push((c.cid, c.model, p, h));
            }
        }
    }
    let _ = script::run(&cmds);

    for (cid, model, p, h) in enemies {
        let out = script::run(&[
            cmd(0x0247, vec![Int(model)]),
            cmd(0x038B, vec![]),
            cmd(0x009A, vec![Int(4), Int(model), Float(p[0]), Float(p[1]), Float(p[2] - CENTRE_TO_FEET), Var(2)]),
            cmd(0x0173, vec![Var(2), Float(h)]),
        ]);
        if let Some((_, handle)) = out.ok().and_then(|o| o.vars.into_iter().find(|v| v.0 == 2)) {
            b.enemy_peds.insert(cid, handle);
        }
    }

    // Side-on camera: 9 m to the right of the arena centre, 3.5 m up, looking at the middle.
    let f = heading_forward(b.heading);
    let right = [f[1], -f[0]];
    let cam = [b.arena[0] + right[0] * 9.0, b.arena[1] + right[1] * 9.0, b.arena[2] + 3.5];
    let _ = script::run(&[
        cmd(0x015F, vec![Float(cam[0]), Float(cam[1]), Float(cam[2]), Float(0.0), Float(0.0), Float(0.0)]),
        cmd(0x0160, vec![Float(b.arena[0]), Float(b.arena[1]), Float(b.arena[2]), Int(2)]),
    ]);
}

/// Which ped represents combatant `cid` on this client.
fn ped_of(b: &ClientBattle, cid: Cid) -> Option<PedRef> {
    if b.mine.contains(&cid) {
        return Some(PedRef::LocalPlayer);
    }
    let c = b.cmb(cid)?;
    match c.player {
        Some(pid) => crate::sync::PEDS.lock().unwrap().as_ref()?.get(&pid).map(|p| PedRef::Handle(p.handle)),
        None => b.enemy_peds.get(&cid).map(|h| PedRef::Handle(*h)),
    }
}

/// task_play_anim from the always-loaded "ped" block.
fn anim(p: PedRef, name: &str) -> Vec<script::Command> {
    let play = |who| cmd(0x0605, vec![who, Str(name.into()), Str("ped".into()), Float(4.0), Int(0), Int(0), Int(0), Int(0), Int(-1)]);
    match p {
        PedRef::LocalPlayer => vec![cmd(0x01F5, vec![Int(0), Var(3)]), play(Var(3))],
        PedRef::Handle(h) => vec![play(Int(h as i32))],
    }
}

/// Position of combatant `cid`'s arena slot (where effects go).
fn slot_of(b: &ClientBattle, cid: Cid) -> Option<[f32; 3]> {
    let c = b.cmb(cid)?;
    let same: Vec<Cid> = b.combatants.iter().filter(|x| x.side == c.side).map(|x| x.cid).collect();
    let i = same.iter().position(|x| *x == cid)?;
    Some(slot(b.arena, b.heading, c.side, i, same.len()).0)
}

/// Animations, effects and sounds for one resolved action.
unsafe fn present_action(b: &mut ClientBattle, actor: Cid, skill: Skill, target: Option<Cid>, hit: bool) {
    let mut cmds = Vec::new();
    let actor_ped = ped_of(b, actor);
    let target_ped = target.and_then(|t| ped_of(b, t));
    let (actor_anim, sound) = match (skill, hit) {
        (Skill::Attack, true) => ("FIGHTA_1", Sfx::Hit),
        (Skill::Fire, true) => ("FIGHTA_2", Sfx::Fire),
        (Skill::Attack | Skill::Fire, false) => ("FIGHTA_1", Sfx::Miss),
        (Skill::Heal, _) => ("IDLE_chat", Sfx::Heal),
        (Skill::Guard, _) => ("FIGHTA_block", Sfx::Guard),
        (Skill::Run, _) => ("IDLE_tired", if hit { Sfx::Confirm } else { Sfx::Miss }),
    };
    if let Some(p) = actor_ped {
        cmds.extend(anim(p, actor_anim));
    }
    if hit && matches!(skill, Skill::Attack | Skill::Fire) {
        if let Some(p) = target_ped {
            cmds.extend(anim(p, "HIT_front"));
        }
    }
    let _ = script::run(&cmds);

    if skill == Skill::Fire && hit {
        if let Some(pos) = target.and_then(|t| slot_of(b, t)) {
            let out = script::run(&[
                cmd(0x064B, vec![Str("explosion_small".into()), Float(pos[0]), Float(pos[1]), Float(pos[2]), Int(1), Var(4)]),
                cmd(0x064C, vec![Var(4)]),
            ]);
            if let Some((_, h)) = out.ok().and_then(|o| o.vars.into_iter().find(|v| v.0 == 4)) {
                b.fx.push((h, Instant::now() + FX_TTL));
            }
        }
    }
    sfx::play(sound);
}

/// Kills effects whose time is up (all of them when `all`).
unsafe fn reap_fx(b: &mut ClientBattle, all: bool) {
    let now = Instant::now();
    let (dead, live): (Vec<_>, Vec<_>) = b.fx.drain(..).partition(|(_, t)| all || *t <= now);
    b.fx = live;
    let cmds: Vec<_> = dead.iter().map(|(h, _)| cmd(0x0650, vec![Int(*h as i32)])).collect();
    if !cmds.is_empty() {
        let _ = script::run(&cmds);
    }
}

/// Unfreezes the player, restores the camera, removes enemy peds.
unsafe fn teardown(b: &mut ClientBattle) {
    reap_fx(b, true);
    let mut cmds = vec![cmd(0x01B4, vec![Int(0), Int(1)]), cmd(0x02EB, vec![]), cmd(0x0373, vec![])];
    for h in b.enemy_peds.values() {
        cmds.push(cmd(0x009B, vec![Int(*h as i32)])); // delete_char
    }
    let _ = script::run(&cmds);
}

fn pad(field: usize) -> i16 {
    world::rd::<i16>(PADS + field as u32 * 2)
}

/// Whether F5 is down while our game window is the foreground window.
fn f5_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F5};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == std::process::id() && (GetAsyncKeyState(VK_F5.0 as i32) as u16 & 0x8000) != 0
    }
}

fn handle_input(slot: &mut Option<ClientBattle>) {
    let (sx, sy, confirm, back, f5) = (pad(PAD_STICK_X), pad(PAD_STICK_Y), pad(PAD_TRIANGLE) > 0, pad(PAD_SQUARE) > 0, f5_down());
    let mut e = EDGE.lock().unwrap();
    // Edges: a stick entering the deflected zone, a button going down.
    let step = |now: i16, before: i16| -> i32 {
        if now.abs() > STICK_THRESHOLD && before.abs() <= STICK_THRESHOLD {
            now.signum() as i32
        } else {
            0
        }
    };
    let (dx, dy) = (step(sx, e.stick_x), step(sy, e.stick_y));
    let (confirm_edge, back_edge, f5_edge) = (confirm && !e.confirm, back && !e.back, f5 && !e.f5);
    *e = InputEdge { stick_x: sx, stick_y: sy, confirm, back, f5 };
    drop(e);

    if f5_edge && slot.is_none() {
        net::send_event(shared::ClientEvent::RequestBattle);
        return;
    }
    let Some(b) = slot.as_mut() else { return };
    let Some(actor) = b.my_turn() else { return };
    let skills = b.turn.as_ref().map(|t| t.1.clone()).unwrap_or_default();
    match b.menu.stage {
        Stage::Skill => {
            if dy != 0 && !skills.is_empty() {
                // Stick forward (W) is negative Y: move the cursor up.
                b.menu.skill_idx = (b.menu.skill_idx as i32 + dy).rem_euclid(skills.len() as i32) as usize;
                sfx::play(Sfx::Cursor);
            }
            if confirm_edge {
                let skill = skills[b.menu.skill_idx];
                if skill.target_kind() == TargetKind::None {
                    choose(b, actor, skill, None);
                } else if !b.targets().is_empty() {
                    b.menu.stage = Stage::Target;
                    b.menu.target_idx = 0;
                    sfx::play(Sfx::Confirm);
                }
            }
        }
        Stage::Target => {
            let targets = b.targets();
            let d = if dx != 0 { dx } else { dy };
            if d != 0 && !targets.is_empty() {
                b.menu.target_idx = (b.menu.target_idx as i32 + d).rem_euclid(targets.len() as i32) as usize;
                sfx::play(Sfx::Cursor);
            }
            if back_edge {
                b.menu.stage = Stage::Skill;
            } else if confirm_edge {
                if let Some(t) = targets.get(b.menu.target_idx) {
                    choose(b, actor, skills[b.menu.skill_idx], Some(*t));
                }
            }
        }
    }
}

fn choose(b: &mut ClientBattle, actor: Cid, skill: Skill, target: Option<Cid>) {
    net::send_event(shared::ClientEvent::ChooseAction { battle: b.id, actor, skill, target });
    b.sent = true;
    b.menu.stage = Stage::Skill;
}
