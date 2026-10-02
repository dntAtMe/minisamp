//! Loopback admin interface: one JSON request per line, one JSON response per line.
//!
//! Requests:
//! - `{"cmd":"status"}`
//! - `{"cmd":"netsim"}` (get) or `{"cmd":"netsim","latency_ms":100,"jitter_ms":20,"loss_pct":5}` (set)
//! - `{"cmd":"packets","limit":50,"kind":"Sync"}` recent packet log, newest last
//! - `{"cmd":"kick","id":1}`
//! - `{"cmd":"shutdown"}` exits the process after answering
//!
//! Responses: `{"ok":true,"data":...}` or `{"ok":false,"error":"..."}`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{json, Value};

use crate::Server;

pub fn spawn(port: u16, server: Arc<Mutex<Server>>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let server = server.clone();
            std::thread::spawn(move || {
                let _ = serve(stream, server);
            });
        }
    });
    Ok(())
}

fn serve(stream: TcpStream, server: Arc<Mutex<Server>>) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => match execute(&req, &server) {
                Ok(data) => json!({ "ok": true, "data": data }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
            Err(e) => json!({ "ok": false, "error": format!("bad request: {e}") }),
        };
        writeln!(writer, "{resp}")?;
        if req_is_shutdown(&line) {
            log::info!("shutdown requested over admin port");
            std::process::exit(0);
        }
    }
    Ok(())
}

fn req_is_shutdown(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|v| v["cmd"] == "shutdown")
}

fn execute(req: &Value, server: &Arc<Mutex<Server>>) -> Result<Value, String> {
    let mut s = server.lock().unwrap();
    match req.get("cmd").and_then(Value::as_str).unwrap_or("") {
        "status" => Ok(s.status_json()),
        "netsim" => {
            let mut c = s.sim_in.conditions;
            if let Some(v) = req.get("latency_ms").and_then(Value::as_u64) {
                c.latency_ms = v.min(5000) as u32;
            }
            if let Some(v) = req.get("jitter_ms").and_then(Value::as_u64) {
                c.jitter_ms = v.min(5000) as u32;
            }
            if let Some(v) = req.get("loss_pct").and_then(Value::as_f64) {
                c.loss_pct = v.clamp(0.0, 100.0) as f32;
            }
            s.sim_in.conditions = c;
            s.sim_out.conditions = c;
            Ok(c.to_json())
        }
        "packets" => {
            let limit = req.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let kind = req.get("kind").and_then(Value::as_str);
            let entries: Vec<Value> = s
                .log
                .iter()
                .filter(|e| kind.is_none_or(|k| e.kind == k))
                .rev()
                .take(limit)
                .map(|e| {
                    json!({
                        "t_ms": e.t_ms, "dir": e.dir, "client": e.client, "addr": e.addr.to_string(),
                        "kind": e.kind, "bytes": e.bytes, "dropped": e.dropped,
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Ok(json!({ "count": entries.len(), "packets": entries }))
        }
        "kick" => {
            let id = req.get("id").and_then(Value::as_u64).ok_or("id is required")? as u16;
            if s.kick(id) {
                Ok(json!({ "kicked": id }))
            } else {
                Err(format!("no player {id}"))
            }
        }
        "shutdown" => Ok(json!({ "shutting_down": true })),
        other => Err(format!("unknown cmd {other:?}")),
    }
}
