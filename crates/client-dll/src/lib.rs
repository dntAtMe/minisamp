//! mini-samp client, loaded into gta_sa.exe (1.0 US) by an ASI loader as `minisamp.asi`.
//!
//! Inactive unless `MINISAMP_SERVER` is set (e.g. `127.0.0.1:7777`), so single-player is
//! untouched. `MINISAMP_NAME` sets the player name.
//!
//! Threads:
//! - net thread ([`net`]): UDP join/sync with the server, keeps the latest remote states
//! - game thread ([`sync`], via a chained hook on the `call Idle` site): reads the local
//!   player, drives remote player peds, runs turn-based battles ([`battle`])
//! - render ([`overlay`], chained Present hook): battle UI
//!
//! Debug state is exported as `sa_debug_json` (see sa-mcp's `plugin_query`).

mod battle;
mod debug;
mod net;
mod overlay;
mod sfx;
mod sync;

use std::ffi::c_void;
use std::sync::OnceLock;

use sa_sdk::addr;
use windows::Win32::Foundation::{BOOL, HMODULE, TRUE};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub struct Config {
    pub server: String,
    pub name: String,
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

type IdleFn = unsafe extern "C" fn(*mut c_void);
static ORIG_IDLE: OnceLock<usize> = OnceLock::new();

unsafe extern "C" fn hk_idle(arg: *mut c_void) {
    sync::on_frame();
    let orig: IdleFn = std::mem::transmute(*ORIG_IDLE.get().unwrap());
    orig(arg)
}

#[no_mangle]
unsafe extern "system" fn DllMain(_module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason != DLL_PROCESS_ATTACH {
        return TRUE;
    }
    let Ok(server) = std::env::var("MINISAMP_SERVER") else { return TRUE };
    if !sa_sdk::is_supported_game() {
        return TRUE;
    }
    let name = std::env::var("MINISAMP_NAME").unwrap_or_else(|_| format!("Player{}", std::process::id()));
    let _ = CONFIG.set(Config { server, name });

    match sa_sdk::hook::hook_call(addr::CALL_IDLE, hk_idle as *const () as usize) {
        Ok(prev) => {
            let _ = ORIG_IDLE.set(prev);
            std::thread::spawn(net::run);
        }
        Err(e) => net::set_status(format!("hook failed: {e}")),
    }
    TRUE
}
