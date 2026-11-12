//! Clip sounds. Every built-in sound is synthesised here, so Glint ships no
//! audio files. A custom .wav file can be used instead.
use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 44_100;

/// (id, name shown in settings). "custom" is handled apart: it plays `sound_file`.
pub const SOUNDS: [(&str, &str); 16] = [
    ("chirp", "Chirp"),
    ("blip", "Blip"),
    ("double", "Double blip"),
    ("ding", "Ding"),
    ("chime", "Chime"),
    ("glass", "Glass"),
    ("pop", "Pop"),
    ("bubble", "Bubble"),
    ("click", "Click"),
    ("tick", "Tick"),
    ("shutter", "Camera shutter"),
    ("arcade", "Arcade"),
    ("levelup", "Level up"),
    ("laser", "Laser"),
    ("drop", "Drop"),
    ("knock", "Knock"),
];

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Square,
    Saw,
    Tri,
}

/// Mix a tone into `b`: starts at `at` seconds, lasts `dur`, glides from `f0`
/// to `f1` Hz. The envelope is a 3 ms attack and a `(1 - t)^shape` fall.
fn tone(b: &mut Vec<f32>, at: f32, dur: f32, f0: f32, f1: f32, wave: Wave, amp: f32, shape: f32) {
    let start = (at * RATE as f32) as usize;
    let n = (dur * RATE as f32) as usize;
    if b.len() < start + n {
        b.resize(start + n, 0.0);
    }
    let mut phase = 0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        phase += (f0 + (f1 - f0) * t) / RATE as f32;
        let p = phase.fract();
        let v = match wave {
            Wave::Sine => (p * std::f32::consts::TAU).sin(),
            Wave::Square => if p < 0.5 { 0.6 } else { -0.6 },
            Wave::Saw => (2.0 * p - 1.0) * 0.7,
            Wave::Tri => 4.0 * (p - 0.5).abs() - 1.0,
        };
        let env = (i as f32 / (0.003 * RATE as f32)).min(1.0) * (1.0 - t).powf(shape);
        b[start + i] += v * env * amp;
    }
}

/// Mix a burst of noise into `b`. `smooth` in 0..1 dulls it (0 = bright hiss).
fn noise(b: &mut Vec<f32>, at: f32, dur: f32, amp: f32, shape: f32, smooth: f32) {
    let start = (at * RATE as f32) as usize;
    let n = (dur * RATE as f32) as usize;
    if b.len() < start + n {
        b.resize(start + n, 0.0);
    }
    let mut seed = 0x2545_F491u32;
    let mut low = 0f32;
    for i in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
        low += (white - low) * (1.0 - smooth);
        let t = i as f32 / n as f32;
        let env = (i as f32 / (0.002 * RATE as f32)).min(1.0) * (1.0 - t).powf(shape);
        b[start + i] += low * env * amp;
    }
}

fn synth(id: &str) -> Vec<f32> {
    use Wave::*;
    let mut b = Vec::new();
    match id {
        "blip" => tone(&mut b, 0.0, 0.08, 1245.0, 1245.0, Sine, 1.0, 2.0),
        "double" => {
            tone(&mut b, 0.0, 0.07, 880.0, 880.0, Sine, 1.0, 1.5);
            tone(&mut b, 0.1, 0.1, 1320.0, 1320.0, Sine, 1.0, 2.0);
        }
        "ding" => {
            tone(&mut b, 0.0, 0.7, 1568.0, 1568.0, Sine, 1.0, 3.0);
            tone(&mut b, 0.0, 0.6, 3136.0, 3136.0, Sine, 0.35, 4.0);
            tone(&mut b, 0.0, 0.4, 4704.0, 4704.0, Sine, 0.15, 5.0);
        }
        "chime" => {
            for (i, f) in [1046.5, 1318.5, 1568.0].into_iter().enumerate() {
                tone(&mut b, i as f32 * 0.09, 0.5, f, f, Sine, 1.0, 3.0);
                tone(&mut b, i as f32 * 0.09, 0.3, f * 2.0, f * 2.0, Sine, 0.2, 4.0);
            }
        }
        "glass" => {
            tone(&mut b, 0.0, 0.55, 2637.0, 2637.0, Sine, 1.0, 4.0);
            tone(&mut b, 0.0, 0.5, 3951.0, 3951.0, Sine, 0.5, 5.0);
            tone(&mut b, 0.0, 0.3, 5274.0, 5274.0, Sine, 0.25, 6.0);
        }
        "pop" => tone(&mut b, 0.0, 0.07, 520.0, 140.0, Sine, 1.0, 1.5),
        "bubble" => tone(&mut b, 0.0, 0.12, 280.0, 980.0, Sine, 1.0, 1.2),
        "click" => {
            noise(&mut b, 0.0, 0.016, 1.0, 2.0, 0.2);
            tone(&mut b, 0.0, 0.02, 2400.0, 1800.0, Sine, 0.5, 2.0);
        }
        "tick" => tone(&mut b, 0.0, 0.035, 2000.0, 2000.0, Tri, 1.0, 2.0),
        "shutter" => {
            noise(&mut b, 0.0, 0.03, 1.0, 1.5, 0.35);
            tone(&mut b, 0.0, 0.02, 1700.0, 1200.0, Sine, 0.4, 2.0);
            noise(&mut b, 0.075, 0.055, 0.8, 2.0, 0.55);
        }
        "arcade" => {
            tone(&mut b, 0.0, 0.07, 988.0, 988.0, Square, 1.0, 0.6);
            tone(&mut b, 0.07, 0.32, 1319.0, 1319.0, Square, 1.0, 2.5);
        }
        "levelup" => {
            for (i, f) in [523.3, 659.3, 784.0, 1046.5].into_iter().enumerate() {
                tone(&mut b, i as f32 * 0.065, if i == 3 { 0.34 } else { 0.11 }, f, f, Tri, 1.0, if i == 3 { 2.5 } else { 1.0 });
            }
        }
        "laser" => tone(&mut b, 0.0, 0.17, 1900.0, 260.0, Saw, 1.0, 1.3),
        "drop" => {
            tone(&mut b, 0.0, 0.08, 1175.0, 1175.0, Sine, 1.0, 1.5);
            tone(&mut b, 0.09, 0.16, 784.0, 784.0, Sine, 1.0, 2.0);
        }
        "knock" => {
            tone(&mut b, 0.0, 0.07, 190.0, 95.0, Sine, 1.0, 2.0);
            noise(&mut b, 0.0, 0.012, 0.3, 2.0, 0.7);
            tone(&mut b, 0.12, 0.08, 175.0, 88.0, Sine, 0.9, 2.0);
            noise(&mut b, 0.12, 0.012, 0.25, 2.0, 0.7);
        }
        // "chirp", and anything unknown.
        _ => tone(&mut b, 0.0, 0.17, 720.0, 1240.0, Sine, 1.0, 1.6),
    }
    b
}

/// A complete 16-bit mono WAV file. `volume` is 0..=100.
pub fn wav(id: &str, volume: u32) -> Vec<u8> {
    let samples = synth(id);
    let peak = samples.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    // 100 = 60 % of full scale: loud, with headroom over game audio.
    let gain = 0.6 * (volume.min(100) as f32 / 100.0) / peak;
    let pcm: Vec<u8> = samples.iter().flat_map(|v| ((v * gain * 32767.0) as i16).to_le_bytes()).collect();
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

/// Play a clip sound without blocking. `id` is one of `SOUNDS` or "custom"
/// (then `file` is a .wav path; its own loudness is used). Falls back to the
/// chirp if the custom file is missing.
pub fn play(id: &str, volume: u32, file: &str) {
    if volume == 0 {
        return;
    }
    if id == "custom" && std::path::Path::new(file.trim()).is_file() {
        let wide: Vec<u16> = file.trim().encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = PlaySoundW(PCWSTR(wide.as_ptr()), HMODULE::default(), SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
        }
        return;
    }
    // An async sound plays from the buffer it was given, so each rendered sound
    // is kept for the life of the process. Volume moves in steps of 5 to bound that.
    static CACHE: Mutex<Option<HashMap<(String, u32), &'static [u8]>>> = Mutex::new(None);
    let id = if SOUNDS.iter().any(|(s, _)| *s == id) { id } else { "chirp" };
    let step = (volume.min(100) + 4) / 5 * 5;
    let data: &'static [u8] = {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        *map.entry((id.to_string(), step)).or_insert_with(|| Box::leak(wav(id, step).into_boxed_slice()))
    };
    unsafe {
        let _ = PlaySoundW(PCWSTR(data.as_ptr() as *const u16), HMODULE::default(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}
