//! Clip sounds. Every built-in sound is synthesised here, so Glint ships no
//! audio files. A custom file (WAV, MP3 or anything else ffmpeg reads) can be
//! used instead: it is decoded once, cut to the part the user chose and set to
//! the chosen volume, and that copy is kept in memory so it plays at once.
use crate::settings::Settings;
use std::collections::HashMap;
use std::path::Path;
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
    wav_file(&pcm, 1)
}

/// A complete WAV file around 16-bit PCM at `RATE` with `channels` channels.
fn wav_file(pcm: &[u8], channels: u16) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2 * channels as u32).to_le_bytes());
    w.extend_from_slice(&(2 * channels).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

/// The longest custom sound that is played, in seconds: a clip sound is a short signal.
pub const CUSTOM_MAX_SECS: f32 = 15.0;

/// The part of a custom file that is played: `start` seconds in, to `end`
/// (0 or not after `start` = to the file's end), at most `CUSTOM_MAX_SECS`.
pub fn custom_span(start: f32, end: f32) -> (f32, f32) {
    let start = if start.is_finite() { start.clamp(0.0, 3600.0) } else { 0.0 };
    let len = if end.is_finite() && end > start + 0.02 { (end - start).min(CUSTOM_MAX_SECS) } else { CUSTOM_MAX_SECS };
    (start, len)
}

/// Volume in steps of 5 (5..=100), as the custom sound is prepared and kept.
fn volume_step(volume: u32) -> u32 {
    ((volume.min(100) + 4) / 5 * 5).max(5)
}

/// What a prepared custom sound was made from: the file (with its size and
/// time, so a replaced file is noticed), the part and the volume.
fn custom_key(file: &str, start: f32, end: f32, volume: u32) -> String {
    let (start, len) = custom_span(start, end);
    let meta = std::fs::metadata(file.trim()).ok();
    let stamp = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
    format!("{}|{}|{stamp}|{start:.3}|{len:.3}|{}", file.trim(), meta.map(|m| m.len()).unwrap_or(0), volume_step(volume))
}

/// Decode the chosen part of `file` with ffmpeg to a WAV file in memory:
/// stereo, 16 bit, `RATE`, the volume applied (100 = the file's own loudness).
pub fn decode_custom(ffmpeg: &Path, file: &str, start: f32, end: f32, volume: u32) -> Result<Vec<u8>, String> {
    let (start, len) = custom_span(start, end);
    let gain = volume_step(volume) as f32 / 100.0;
    let out = crate::util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{start:.3}"), "-i"])
        .arg(file.trim())
        .args(["-vn", "-t", &format!("{len:.3}"), "-af", &format!("volume={gain:.3}"), "-ac", "2", "-ar", &RATE.to_string(), "-f", "s16le", "pipe:1"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let frames = out.stdout.len() / 4;
    if !out.status.success() || frames < RATE as usize / 100 {
        return Err("That sound file could not be read".into());
    }
    Ok(wav_file(&out.stdout[..frames * 4], 2))
}

/// The custom sound as prepared (key, WAV). One at a time: before it is
/// replaced, a sound that is still playing from it is stopped.
static CUSTOM: Mutex<Option<(String, Vec<u8>)>> = Mutex::new(None);

/// Prepare the custom sound of `s` if it is not ready yet (decodes the file:
/// call it off the clip path). `min_volume` as in `play`. True when ready.
pub fn warm(s: &Settings, min_volume: u32) -> bool {
    if s.sound_name != "custom" || !Path::new(s.sound_file.trim()).is_file() {
        return false;
    }
    let volume = s.sound_volume.max(min_volume);
    let key = custom_key(&s.sound_file, s.sound_start, s.sound_end, volume);
    if CUSTOM.lock().unwrap().as_ref().is_some_and(|(k, _)| *k == key) {
        return true;
    }
    let Some(ffmpeg) = crate::util::find_exe(&s.ffmpeg_path, "ffmpeg") else { return false };
    match decode_custom(&ffmpeg, &s.sound_file, s.sound_start, s.sound_end, volume) {
        Ok(data) => {
            let mut slot = CUSTOM.lock().unwrap();
            // The old copy may be playing right now: stop it before its memory goes.
            unsafe {
                let _ = PlaySoundW(PCWSTR::null(), HMODULE::default(), SND_ASYNC);
            }
            *slot = Some((key, data));
            true
        }
        Err(e) => {
            crate::util::log(&format!("clip sound: {e} ({})", s.sound_file.trim()));
            false
        }
    }
}

/// Play the clip sound of `s` without blocking. A custom file plays from its
/// prepared copy (`warm`); if that is not ready, a .wav plays as it is and
/// anything else falls back to the chirp. `min_volume` lifts a very low
/// setting for the Settings page's Play button (0 for a real clip).
pub fn play(s: &Settings, min_volume: u32) {
    let volume = s.sound_volume.max(min_volume);
    if volume == 0 {
        return;
    }
    let (id, file) = (s.sound_name.as_str(), s.sound_file.trim());
    if id == "custom" && Path::new(file).is_file() {
        let key = custom_key(file, s.sound_start, s.sound_end, volume);
        {
            let slot = CUSTOM.lock().unwrap();
            if let Some((k, data)) = slot.as_ref() {
                if *k == key {
                    unsafe {
                        let _ = PlaySoundW(PCWSTR(data.as_ptr() as *const u16), HMODULE::default(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
                    }
                    return;
                }
            }
        }
        if file.to_ascii_lowercase().ends_with(".wav") {
            let wide: Vec<u16> = file.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let _ = PlaySoundW(PCWSTR(wide.as_ptr()), HMODULE::default(), SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
            }
            return;
        }
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

/// Checks of the custom sound's preparation for `--polish-test`: (name, ok, detail).
/// A one-second tone is made with ffmpeg as WAV and MP3 and decoded; no sound is played.
pub fn self_checks() -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    let mut add = |name: &str, ok: bool, detail: String| out.push((name.to_string(), ok, detail));
    add("custom sound: no end means to the end, capped", custom_span(0.5, 0.0) == (0.5, CUSTOM_MAX_SECS) && custom_span(0.5, 0.2) == (0.5, CUSTOM_MAX_SECS), format!("{:?}", custom_span(0.5, 0.0)));
    add("custom sound: a part is start and length", custom_span(0.25, 0.75) == (0.25, 0.5) && custom_span(f32::NAN, f32::INFINITY) == (0.0, CUSTOM_MAX_SECS) && custom_span(-3.0, 100.0) == (0.0, CUSTOM_MAX_SECS), format!("{:?}", custom_span(0.25, 0.75)));
    add("custom sound: volume in steps of 5, never 0", volume_step(0) == 5 && volume_step(42) == 45 && volume_step(100) == 100 && volume_step(900) == 100, format!("{} {}", volume_step(0), volume_step(42)));
    let Some(ffmpeg) = crate::util::find_exe("", "ffmpeg") else {
        add("custom sound: ffmpeg found for the decode checks", false, "ffmpeg.exe not found".into());
        return out;
    };
    let dir = crate::util::tmp_dir().join("soundtest");
    let _ = std::fs::create_dir_all(&dir);
    let peak = |wav: &[u8]| wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0;
    let secs = |wav: &[u8]| (wav.len() - 44) as f32 / (RATE as f32 * 4.0);
    for ext in ["wav", "mp3"] {
        let file = dir.join(format!("tone.{ext}"));
        let made = crate::util::hidden_cmd(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-af", "volume=6dB", "-ac", "1"])
            .arg(&file)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            add(&format!("custom sound: a test {ext} was made"), false, "ffmpeg could not write it".into());
            continue;
        }
        let path = file.to_string_lossy().into_owned();
        let full = decode_custom(&ffmpeg, &path, 0.0, 0.0, 100);
        let part = decode_custom(&ffmpeg, &path, 0.25, 0.75, 100);
        let half = decode_custom(&ffmpeg, &path, 0.25, 0.75, 50);
        match (full, part, half) {
            (Ok(full), Ok(part), Ok(half)) => {
                add(&format!("custom sound ({ext}): the whole file is about a second of stereo WAV"), &full[..4] == b"RIFF" && full[22] == 2 && (secs(&full) - 1.0).abs() < 0.08, format!("{:.3} s", secs(&full)));
                add(&format!("custom sound ({ext}): 0.25 to 0.75 s is half a second"), (secs(&part) - 0.5).abs() < 0.03, format!("{:.3} s", secs(&part)));
                let (a, b) = (peak(&part), peak(&half));
                add(&format!("custom sound ({ext}): volume 50 is half as loud as 100"), a > 0.1 && (b / a - 0.5).abs() < 0.06, format!("{a:.3} -> {b:.3}"));
            }
            (a, b, c) => add(&format!("custom sound ({ext}): decodes"), false, format!("{:?} {:?} {:?}", a.err(), b.err(), c.err())),
        }
    }
    add("custom sound: a file that is not audio is refused", decode_custom(&ffmpeg, &dir.join("missing.wav").to_string_lossy(), 0.0, 0.0, 100).is_err(), String::new());
    let _ = std::fs::remove_dir_all(&dir);
    out
}