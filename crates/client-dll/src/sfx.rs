//! Procedurally synthesised battle sound effects, played with PlaySound from memory.
//!
//! Only the client whose window is in the foreground plays sounds (several test clients on one
//! machine would otherwise all beep at once); `MINISAMP_SFX=always` overrides that.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 22050;

#[derive(Clone, Copy, Debug)]
pub enum Sfx {
    Cursor,
    Confirm,
    Hit,
    Miss,
    Fire,
    Heal,
    Guard,
    Victory,
    Defeat,
}

const ALL: [Sfx; 9] = [Sfx::Cursor, Sfx::Confirm, Sfx::Hit, Sfx::Miss, Sfx::Fire, Sfx::Heal, Sfx::Guard, Sfx::Victory, Sfx::Defeat];

static BANK: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// Cheap deterministic noise.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn tone(out: &mut Vec<f32>, freq: f32, secs: f32, vol: f32, square: bool) {
    let n = (secs * RATE as f32) as usize;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let phase = (t * freq).fract();
        let s = if square { (if phase < 0.5 { 1.0 } else { -1.0 }) * 0.6 } else { (phase * TAU).sin() };
        let env = (1.0 - i as f32 / n as f32).powf(0.6) * (i as f32 / 60.0).min(1.0);
        out.push(s * env * vol);
    }
}

fn synth(sfx: Sfx) -> Vec<f32> {
    let mut v = Vec::new();
    let mut noise = Noise(0x1234_5678);
    match sfx {
        Sfx::Cursor => tone(&mut v, 1320.0, 0.035, 0.35, true),
        Sfx::Confirm => {
            tone(&mut v, 880.0, 0.05, 0.35, true);
            tone(&mut v, 1320.0, 0.07, 0.35, true);
        }
        Sfx::Hit => {
            // Noise crack over a low thump.
            let n = (0.18 * RATE as f32) as usize;
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                let env = (-t * 28.0).exp();
                v.push((noise.next() * 0.7 + (t * 90.0 * TAU).sin()) * env * 0.8);
            }
        }
        Sfx::Miss => {
            let n = (0.15 * RATE as f32) as usize;
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                let f = 900.0 - 2500.0 * t;
                v.push((t * f * TAU).sin() * (1.0 - i as f32 / n as f32) * 0.4);
            }
        }
        Sfx::Fire => {
            // Low-passed noise swelling and fading (whoosh) plus a rumble.
            let n = (0.6 * RATE as f32) as usize;
            let mut lp = 0.0;
            for i in 0..n {
                let x = i as f32 / n as f32;
                let env = (x * std::f32::consts::PI).sin().powf(0.7);
                lp += (noise.next() - lp) * (0.05 + 0.25 * x);
                let t = i as f32 / RATE as f32;
                v.push((lp * 2.5 + (t * 55.0 * TAU).sin() * 0.4) * env * 0.8);
            }
        }
        Sfx::Heal => {
            for f in [1046.5, 1318.5, 1568.0, 2093.0] {
                tone(&mut v, f, 0.09, 0.35, false);
            }
        }
        Sfx::Guard => {
            tone(&mut v, 330.0, 0.06, 0.4, true);
            tone(&mut v, 247.0, 0.1, 0.4, true);
        }
        Sfx::Victory => {
            for (f, d) in [(523.3, 0.12), (523.3, 0.12), (523.3, 0.12), (523.3, 0.36), (415.3, 0.36), (466.2, 0.36), (523.3, 0.2), (466.2, 0.1), (523.3, 0.7)] {
                tone(&mut v, f, d, 0.4, true);
            }
        }
        Sfx::Defeat => {
            for f in [392.0, 349.2, 311.1, 261.6] {
                tone(&mut v, f, 0.3, 0.4, false);
            }
        }
    }
    v
}

/// 16-bit mono PCM WAV.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    w
}

fn foreground() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid == std::process::id()
}

/// Plays `sfx` asynchronously (replacing whatever sound is playing).
pub fn play(sfx: Sfx) {
    if !(foreground() || std::env::var("MINISAMP_SFX").is_ok_and(|v| v == "always")) {
        return;
    }
    let bank = BANK.get_or_init(|| ALL.iter().map(|s| wav(&synth(*s))).collect());
    let idx = ALL.iter().position(|s| std::mem::discriminant(s) == std::mem::discriminant(&sfx)).unwrap();
    // The buffers live in a static, as SND_MEMORY|SND_ASYNC requires.
    unsafe {
        let _ = PlaySoundW(PCWSTR(bank[idx].as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_a_valid_nonempty_wav() {
        for s in ALL {
            let w = wav(&synth(s));
            assert_eq!(&w[0..4], b"RIFF");
            assert!(w.len() > 44 + 400, "{s:?} too short");
        }
    }
}
