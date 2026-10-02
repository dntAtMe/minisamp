//! `sa_debug_json` export for sa-mcp's `plugin_query` (called on the game thread).
//!
//! Follows sa-mcp's convention so `compare_instances` can match players exactly:
//! top-level `net_id` (our player id) and `remotes[].net_id` + `remotes[].position`.

use serde_json::{json, Value};

use crate::net::NET;
use crate::sync::PEDS;

fn pos(p: [f32; 3]) -> Value {
    let r = |v: f32| (v as f64 * 100.0).round() / 100.0;
    json!({ "x": r(p[0]), "y": r(p[1]), "z": r(p[2]) })
}

fn snapshot() -> Value {
    let cfg = crate::CONFIG.get();
    let n = NET.lock().unwrap();
    let peds = PEDS.lock().unwrap();
    let remotes: Vec<Value> = n
        .remotes
        .iter()
        .map(|(id, r)| {
            let ped = peds.as_ref().and_then(|m| m.get(id));
            let ped_ptr = ped.map(|p| sa_sdk::world::ped_from_handle(p.handle)).unwrap_or(0);
            json!({
                "net_id": id,
                "name": r.name,
                "net_position": pos(r.state.pos),
                "net_move_state": format!("{:?}", r.state.move_state),
                "net_seq": r.state.seq,
                "age_ms": r.received.elapsed().as_millis() as u64,
                "delay_ms": r.delay_ms,
                "updates": r.updates,
                "handle": ped.map(|p| p.handle),
                "ped": format!("{ped_ptr:#x}"),
                "position": (ped_ptr != 0).then(|| pos(sa_sdk::world::entity_pos(ped_ptr))),
                "target": ped.map(|p| pos(p.target)),
                "error_m": ped.map(|p| (p.error as f64 * 100.0).round() / 100.0),
                "ped_mode": ped.map(|p| format!("{:?}", p.mode)),
                "snaps": ped.map(|p| p.snaps),
                "ped_age_s": ped.map(|p| p.created.elapsed().as_secs()),
            })
        })
        .collect();
    json!({
        "plugin": "minisamp",
        "server": cfg.map(|c| c.server.clone()),
        "name": cfg.map(|c| c.name.clone()),
        "status": n.status,
        "net_id": n.my_id,
        "local": n.local.map(|s| json!({ "position": pos(s.pos), "move_state": format!("{:?}", s.move_state), "heading": s.heading })),
        "stats": {
            "sent": n.stats.sent,
            "received": n.stats.received,
            "snapshots": n.stats.snapshots,
            "last_tick": n.stats.last_tick,
            "decode_errors": n.stats.decode_errors,
            "last_server_packet_ms": n.last_server_packet.map(|t| t.elapsed().as_millis() as u64),
            "rtt_ms": n.rtt_ms.map(|r| r.round()),
        },
        "remotes": remotes,
    })
}

/// Writes JSON into `buf` (up to `cap` bytes) and returns the full length needed.
///
/// # Safety
/// `buf` must be valid for `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn sa_debug_json(buf: *mut u8, cap: u32) -> u32 {
    let text = snapshot().to_string();
    let n = text.len().min(cap as usize);
    std::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
    text.len() as u32
}
