//! Game-thread sync: publish the local player's state, drive one ped per remote player.
//!
//! Remote players are script-created peds. A go-to task matching the remote's move state makes
//! the game play walk/run/sprint animations; the position itself is corrected every frame by
//! moving the ped a fraction of the way toward the remote's extrapolated position (x/y only,
//! the game's physics keeps z on the ground). Large errors teleport.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use sa_sdk::addr::{GAME_STATE, GS_FRONTEND_IDLE};
use sa_sdk::script::{self, cmd, Arg::*};
use sa_sdk::world::{self, dist};
use shared::{MoveState, PlayerId, PlayerState};

use crate::net::NET;

/// Error (m) above which the ped is teleported instead of walked.
pub const SNAP_DISTANCE: f32 = 3.0;
/// Re-issue the go-to task when the goal moved this far (m). Every re-issue restarts the
/// task, so keep this coarse; the per-frame correction handles precision.
const RETARGET_DISTANCE: f32 = 2.0;
/// How far ahead (s) of the last known position a moving ped is sent, so it keeps moving
/// between task updates.
const LOOKAHEAD_S: f32 = 1.0;
/// Fraction of the horizontal error removed per frame, and the cap per frame (m).
const CORRECTION_RATE: f32 = 0.2;
const MAX_CORRECTION_STEP: f32 = 0.5;
/// Below this horizontal error (m) no correction is applied.
const CORRECTION_DEADZONE: f32 = 0.03;
/// Cap on extrapolating a state forward (s): end-to-end delay plus local age.
const MAX_EXTRAPOLATION_S: f32 = 0.5;
/// set_char_coordinates / create_char take a ground position; synced positions are the
/// ped's centre of mass, about 1 m higher.
const CENTRE_TO_FEET: f32 = 1.0;
/// Grove Street gang skins (fam1-3), picked by player id.
const SKINS: [i32; 3] = [105, 106, 107];

pub struct RemotePed {
    pub handle: u32,
    pub mode: MoveState,
    pub task_goal: Option<[f32; 3]>,
    pub target: [f32; 3],
    pub error: f32,
    pub snaps: u64,
    pub created: Instant,
}

pub static PEDS: Mutex<Option<HashMap<PlayerId, RemotePed>>> = Mutex::new(None);

pub unsafe fn on_frame() {
    // In-game frames only (Idle is not called in menus, but be defensive).
    if world::rd::<i32>(GAME_STATE) <= GS_FRONTEND_IDLE {
        return;
    }
    let player = world::player_ped();
    if player == 0 {
        return;
    }
    let vel = world::move_speed(player);
    let local = PlayerState {
        seq: 0,
        pos: world::entity_pos(player),
        vel,
        heading: world::entity_heading(player),
        health: world::ped_health(player),
        armor: world::ped_armor(player),
        move_state: MoveState::from_velocity(vel),
    };

    // Age of each remote state = sender's one-way latency + time held by the server (from the
    // snapshot) + our own one-way latency + time since we received it.
    crate::overlay::ensure_hook();
    let my_id = NET.lock().unwrap().my_id;
    crate::battle::on_frame(my_id);

    let remotes: Vec<(PlayerId, PlayerState, f32)> = {
        let mut n = NET.lock().unwrap();
        n.local = Some(local);
        let own = n.one_way_ms();
        n.remotes
            .iter()
            .map(|(id, r)| (*id, r.state, (r.delay_ms + own) as f32 / 1000.0 + r.received.elapsed().as_secs_f32()))
            .collect()
    };

    let mut guard = PEDS.lock().unwrap();
    let peds = guard.get_or_insert_with(HashMap::new);

    // Players that left.
    let gone: Vec<PlayerId> = peds.keys().filter(|id| !remotes.iter().any(|r| r.0 == **id)).copied().collect();
    for id in gone {
        if let Some(p) = peds.remove(&id) {
            let _ = script::run(&[cmd(0x009B, vec![Int(p.handle as i32)])]); // delete_char
        }
    }

    for (id, state, age) in remotes {
        let ped_ok = peds.get(&id).is_some_and(|p| world::ped_from_handle(p.handle) != 0);
        if !ped_ok {
            match spawn(id, &state) {
                Ok(p) => {
                    peds.insert(id, p);
                }
                Err(_) => continue,
            }
        }
        let p = peds.get_mut(&id).unwrap();
        drive(p, &state, age);
    }
}

unsafe fn spawn(id: PlayerId, s: &PlayerState) -> Result<RemotePed, String> {
    let model = SKINS[id as usize % SKINS.len()];
    let out = script::run(&[
        cmd(0x0247, vec![Int(model)]), // request_model
        cmd(0x038B, vec![]),           // load_all_models_now
        cmd(0x009A, vec![Int(4), Int(model), Float(s.pos[0]), Float(s.pos[1]), Float(s.pos[2] - CENTRE_TO_FEET), Var(0)]),
        cmd(0x0173, vec![Var(0), Float(s.heading)]),
    ])?;
    let handle = out.vars.iter().find(|v| v.0 == 0).map(|v| v.1).unwrap_or(0);
    if handle == 0 {
        return Err("create_char returned no handle".into());
    }
    Ok(RemotePed {
        handle,
        mode: MoveState::Idle,
        task_goal: None,
        target: s.pos,
        error: 0.0,
        snaps: 0,
        created: Instant::now(),
    })
}

/// Moves the ped part of the way toward `target` in x/y by writing its matrix position.
unsafe fn correct_xy(ped: u32, target: [f32; 3]) {
    let m = world::rd::<u32>(ped + sa_sdk::addr::ENT_MATRIX);
    if m == 0 {
        return;
    }
    let pos = (m + sa_sdk::addr::MAT_POS) as *mut f32;
    let (dx, dy) = (target[0] - *pos, target[1] - *pos.add(1));
    let err = (dx * dx + dy * dy).sqrt();
    if err < CORRECTION_DEADZONE {
        return;
    }
    let step = (err * CORRECTION_RATE).min(MAX_CORRECTION_STEP) / err;
    *pos += dx * step;
    *pos.add(1) += dy * step;
}

unsafe fn drive(p: &mut RemotePed, s: &PlayerState, age: f32) {
    let ped = world::ped_from_handle(p.handle);
    let h = Int(p.handle as i32);
    let t = age.min(MAX_EXTRAPOLATION_S);
    p.target = [s.pos[0] + s.vel[0] * t, s.pos[1] + s.vel[1] * t, s.pos[2] + s.vel[2] * t];
    let current = world::entity_pos(ped);
    p.error = dist(current, p.target);

    if p.error > SNAP_DISTANCE {
        let _ = script::run(&[
            cmd(0x00A1, vec![h.clone(), Float(p.target[0]), Float(p.target[1]), Float(p.target[2] - CENTRE_TO_FEET)]),
            cmd(0x0173, vec![h.clone(), Float(s.heading)]),
            cmd(0x0687, vec![h.clone()]), // clear_char_tasks
        ]);
        p.snaps += 1;
        p.task_goal = None;
        p.mode = MoveState::Idle;
        return;
    }

    correct_xy(ped, p.target);

    match s.move_state {
        MoveState::Idle => {
            if p.mode != MoveState::Idle {
                let _ = script::run(&[cmd(0x0687, vec![h.clone()])]);
                p.task_goal = None;
            }
            let dh = (world::entity_heading(ped) - s.heading + 540.0).rem_euclid(360.0) - 180.0;
            if dh.abs() > 5.0 {
                let _ = script::run(&[cmd(0x0173, vec![h, Float(s.heading)])]);
            }
        }
        mode => {
            let goal = [
                s.pos[0] + s.vel[0] * (t + LOOKAHEAD_S),
                s.pos[1] + s.vel[1] * (t + LOOKAHEAD_S),
                s.pos[2],
            ];
            let retarget = mode != p.mode || p.task_goal.is_none_or(|g| dist(g, goal) > RETARGET_DISTANCE);
            if retarget {
                let walk_style = match mode {
                    MoveState::Walk => 4,
                    MoveState::Sprint => 7,
                    _ => 6,
                };
                // task_go_straight_to_coord char x y z mode timeout_ms
                let _ = script::run(&[cmd(0x05D3, vec![h, Float(goal[0]), Float(goal[1]), Float(goal[2]), Int(walk_style), Int(2000)])]);
                p.task_goal = Some(goal);
            }
        }
    }
    p.mode = s.move_state;
}
