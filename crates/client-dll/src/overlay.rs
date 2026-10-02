//! Battle UI, drawn with GDI onto the back buffer right before Present (chained vtable hook).
//! Classic JRPG layout: enemy panel at the top, a blue command window, party HP and the
//! message log at the bottom.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::{Interface, HRESULT};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Direct3D9::{
    IDirect3DDevice9, IDirect3DSurface9, D3DBACKBUFFER_TYPE_MONO, D3DFMT_X8R8G8B8, D3DPOOL_DEFAULT, D3DSURFACE_DESC, D3DTEXF_NONE,
};
use windows::Win32::Graphics::Gdi::*;

use shared::battle::{Outcome, Side};

use crate::battle::{Stage, BATTLE};

type PresentFn = unsafe extern "system" fn(*mut c_void, *const RECT, *const RECT, HWND, *const RGNDATA) -> HRESULT;

static ORIG_PRESENT: OnceLock<usize> = OnceLock::new();
/// Diagnostics for sa_debug_json: frames drawn, last draw error.
pub static DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PRESENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static LAST_ERROR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn hooked() -> bool {
    ORIG_PRESENT.get().is_some()
}

/// Game thread: hook Present once the device exists. Hooking after sa-bridge (which hooks in
/// menu frames) makes us the outer hook, so its screenshots include this UI.
pub fn ensure_hook() {
    if ORIG_PRESENT.get().is_some() || sa_sdk::d3d::device() == 0 {
        return;
    }
    if let Ok(prev) = unsafe { sa_sdk::d3d::hook_device_slot(sa_sdk::d3d::SLOT_PRESENT, hk_present as *const () as usize) } {
        let _ = ORIG_PRESENT.set(prev);
    }
}

unsafe extern "system" fn hk_present(this: *mut c_void, src: *const RECT, dst: *const RECT, hwnd: HWND, dirty: *const RGNDATA) -> HRESULT {
    PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match draw(this) {
        Ok(true) => {
            DRAWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(false) => {}
        Err(e) => *LAST_ERROR.lock().unwrap() = e.to_string(),
    }
    let orig: PresentFn = std::mem::transmute(*ORIG_PRESENT.get().unwrap());
    orig(this, src, dst, hwnd, dirty)
}

const BLUE: u32 = rgb(16, 24, 88);
const WHITE: u32 = rgb(240, 240, 240);
const GREY: u32 = rgb(140, 140, 160);
const YELLOW: u32 = rgb(255, 220, 80);
const GREEN: u32 = rgb(60, 200, 90);
const RED: u32 = rgb(210, 50, 50);
const DARK: u32 = rgb(30, 30, 40);

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

struct Painter {
    hdc: HDC,
    /// Window rectangles to copy to the back buffer.
    windows: std::cell::RefCell<Vec<RECT>>,
    font: HFONT,
    big: HFONT,
    line_h: i32,
}

impl Painter {
    unsafe fn rect(&self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let brush = CreateSolidBrush(COLORREF(color));
        FillRect(self.hdc, &RECT { left: x, top: y, right: x + w, bottom: y + h }, brush);
        let _ = DeleteObject(brush);
    }

    /// Blue window with a white border.
    unsafe fn window(&self, x: i32, y: i32, w: i32, h: i32) {
        self.windows.borrow_mut().push(RECT { left: x, top: y, right: x + w, bottom: y + h });
        self.rect(x, y, w, h, WHITE);
        self.rect(x + 2, y + 2, w - 4, h - 4, BLUE);
    }

    unsafe fn text(&self, x: i32, y: i32, color: u32, s: &str) {
        SetTextColor(self.hdc, COLORREF(color));
        let wide: Vec<u16> = s.encode_utf16().collect();
        let _ = TextOutW(self.hdc, x, y, &wide);
    }

    unsafe fn width(&self, s: &str) -> i32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(self.hdc, &wide, &mut size);
        size.cx
    }

    /// Greedy word wrap to `max_w` pixels.
    unsafe fn wrap(&self, s: &str, max_w: i32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut cur = String::new();
        for word in s.split_whitespace() {
            let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if self.width(&candidate) > max_w && !cur.is_empty() {
                lines.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = candidate;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    unsafe fn bar(&self, x: i32, y: i32, w: i32, h: i32, frac: f32) {
        self.rect(x, y, w, h, DARK);
        let fill = (w as f32 * frac.clamp(0.0, 1.0)) as i32;
        self.rect(x, y, fill, h, if frac > 0.3 { GREEN } else { RED });
    }
}

/// GDI cannot attach to the (non-lockable) back buffer, and D3D9 cannot copy a render target
/// into an offscreen plain surface. The UI consists of opaque windows, so it is drawn with GDI
/// onto an offscreen plain surface and only the window rectangles are copied to the back
/// buffer. The surface is cached per size (render thread only; default-pool surfaces die on
/// device Reset, so a failed copy drops it).
struct Scratch(IDirect3DSurface9, i32, i32);
unsafe impl Send for Scratch {}
static SCRATCH: std::sync::Mutex<Option<Scratch>> = std::sync::Mutex::new(None);

unsafe fn scratch(device: &IDirect3DDevice9, w: i32, h: i32) -> windows::core::Result<IDirect3DSurface9> {
    let mut slot = SCRATCH.lock().unwrap();
    if let Some(Scratch(s, sw, sh)) = slot.as_ref() {
        if (*sw, *sh) == (w, h) {
            return Ok(s.clone());
        }
    }
    let mut surf: Option<IDirect3DSurface9> = None;
    device.CreateOffscreenPlainSurface(w as u32, h as u32, D3DFMT_X8R8G8B8, D3DPOOL_DEFAULT, &mut surf, std::ptr::null_mut())?;
    let surf = surf.unwrap();
    *slot = Some(Scratch(surf.clone(), w, h));
    Ok(surf)
}

/// Ok(true) when something was drawn.
unsafe fn draw(raw_device: *mut c_void) -> windows::core::Result<bool> {
    let guard = BATTLE.lock().unwrap();
    let Some(b) = guard.as_ref() else { return Ok(false) };

    let device = IDirect3DDevice9::from_raw_borrowed(&raw_device).ok_or_else(|| windows::core::Error::from(HRESULT(-1)))?;
    let bb = device.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
    let mut desc = D3DSURFACE_DESC::default();
    bb.GetDesc(&mut desc)?;
    let (w, h) = (desc.Width as i32, desc.Height as i32);
    let step = |what: &str, e: windows::core::Error| windows::core::Error::new(e.code(), format!("{what}: {}", e.message()));
    let surf = scratch(device, w, h).map_err(|e| step("CreateOffscreenPlainSurface", e))?;
    let mut hdc = HDC::default();
    surf.GetDC(&mut hdc).map_err(|e| step("GetDC", e))?;

    let line_h = (h / 26).max(14);
    let font = CreateFontW(line_h, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, ANTIALIASED_QUALITY.0 as u32, (DEFAULT_PITCH.0 | FF_SWISS.0) as u32, windows::core::w!("Arial"));
    let big = CreateFontW(line_h * 3, 0, 0, 0, FW_HEAVY.0 as i32, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, ANTIALIASED_QUALITY.0 as u32, (DEFAULT_PITCH.0 | FF_SWISS.0) as u32, windows::core::w!("Arial"));
    let old_font = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);
    let p = Painter { hdc, font, big, line_h, windows: Default::default() };
    let pad = line_h / 2;

    // Enemies (top).
    let enemies: Vec<_> = b.combatants.iter().filter(|c| c.side == Side::Enemies).collect();
    let targets = b.targets();
    let picking = b.my_turn().is_some() && b.menu.stage == Stage::Target;
    let selected = targets.get(b.menu.target_idx).copied();
    let top_h = pad * 2 + line_h * enemies.len() as i32 * 2;
    p.window(pad, pad, w / 2, top_h);
    for (i, e) in enemies.iter().enumerate() {
        let y = pad * 2 + i as i32 * line_h * 2;
        let marker = if picking && selected == Some(e.cid) { "> " } else { "  " };
        let color = if e.hp == 0 { GREY } else if picking && selected == Some(e.cid) { YELLOW } else { WHITE };
        p.text(pad * 2, y, color, &format!("{marker}{}", e.name));
        p.bar(pad * 2 + line_h, y + line_h, w / 4, line_h / 2, e.hp as f32 / e.max_hp as f32);
        p.text(pad * 3 + line_h + w / 4, y + line_h / 2, color, &format!("{}/{}", e.hp, e.max_hp));
    }

    // Bottom row: commands | party | messages.
    let bottom_h = line_h * 7;
    let y0 = h - bottom_h - pad;
    let cmd_w = w / 5;
    p.window(pad, y0, cmd_w, bottom_h);
    let actor_name = b.turn.as_ref().and_then(|t| b.cmb(t.0)).map(|c| c.name.clone()).unwrap_or_default();
    if b.my_turn().is_some() {
        let skills = &b.turn.as_ref().unwrap().1;
        for (i, s) in skills.iter().enumerate() {
            let sel = i == b.menu.skill_idx;
            let color = if sel { YELLOW } else { WHITE };
            p.text(pad * 2, y0 + pad + i as i32 * line_h, color, &format!("{}{}", if sel { "> " } else { "  " }, s.name()));
        }
    } else if b.outcome.is_none() {
        let msg = if b.sent { "..." } else if actor_name.is_empty() { "" } else { "Waiting for" };
        p.text(pad * 2, y0 + pad, GREY, msg);
        p.text(pad * 2, y0 + pad + line_h, WHITE, &actor_name);
    }

    let party_x = pad * 2 + cmd_w;
    let party_w = w * 2 / 5;
    p.window(party_x, y0, party_w, bottom_h);
    let mut row = 0;
    for c in b.combatants.iter().filter(|c| c.side == Side::Party) {
        let y = y0 + pad + row * line_h * 2;
        row += 1;
        let up = b.turn.as_ref().is_some_and(|t| t.0 == c.cid);
        let color = if c.hp == 0 { GREY } else if up { YELLOW } else { WHITE };
        let me = if b.mine.contains(&c.cid) { " (you)" } else { "" };
        p.text(party_x + pad, y, color, &format!("{}{}{}", if up { "> " } else { "  " }, c.name, me));
        p.text(party_x + party_w - line_h * 5, y, color, &format!("{}/{}", c.hp, c.max_hp));
        p.bar(party_x + pad + line_h, y + line_h, party_w - line_h * 3, line_h / 3, c.hp as f32 / c.max_hp as f32);
    }

    let log_x = party_x + party_w + pad;
    let log_w = w - log_x - pad;
    p.window(log_x, y0, log_w, bottom_h);
    p.text(log_x + pad, y0 + pad, GREY, &format!("Round {}", b.round));
    // Newest messages that fit, oldest first.
    let lines: Vec<String> = b.messages.iter().flat_map(|(m, _)| p.wrap(m, log_w - pad * 2)).collect();
    let fit = ((bottom_h - pad * 2) / line_h - 1).max(1) as usize;
    for (i, line) in lines.iter().skip(lines.len().saturating_sub(fit)).enumerate() {
        p.text(log_x + pad, y0 + pad + (i as i32 + 1) * line_h, WHITE, line);
    }

    // Outcome banner.
    if let Some((o, t)) = b.outcome {
        if t.elapsed().as_secs_f32() < 4.0 {
            SelectObject(hdc, p.big);
            let (label, color) = match o {
                Outcome::Victory => ("VICTORY", YELLOW),
                Outcome::Defeat => ("DEFEAT", RED),
                Outcome::Fled => ("ESCAPED", WHITE),
                Outcome::Aborted => ("ABORTED", GREY),
            };
            p.window(w / 2 - p.line_h * 7, h / 3 - p.line_h, p.line_h * 14, p.line_h * 5);
            p.text(w / 2 - p.line_h * 5, h / 3, DARK, label);
            p.text(w / 2 - p.line_h * 5 - 3, h / 3 - 3, color, label);
        }
    }

    SelectObject(hdc, old_font);
    let _ = DeleteObject(p.font);
    let _ = DeleteObject(p.big);
    surf.ReleaseDC(hdc).map_err(|e| step("ReleaseDC", e))?;
    for r in p.windows.borrow().iter() {
        if let Err(e) = device.StretchRect(&surf, r, &bb, r, D3DTEXF_NONE) {
            *SCRATCH.lock().unwrap() = None;
            return Err(step("StretchRect", e));
        }
    }
    Ok(true)
}
