//! Desktop (WASAPI loopback) and microphone capture into wall-clock-anchored RAM rings.
use crate::util::now_secs;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Interleaved stereo s16 samples.
pub struct Pcm {
    pub rate: u32,
    pub data: Vec<i16>,
}

/// Ring of stereo s16 audio. Frame `n` was captured at `t0 + n / rate` on the
/// app clock, so any wall-clock range can be cut out later and lined up with video.
pub struct AudioRing {
    rate: u32,
    buf: VecDeque<i16>,
    cap_frames: usize,
    t0: f64,
    written: u64,
    active: bool,
}

impl AudioRing {
    pub fn new() -> Self {
        AudioRing { rate: 48_000, buf: VecDeque::new(), cap_frames: 0, t0: 0.0, written: 0, active: false }
    }

    pub fn configure(&mut self, rate: u32, max_secs: f64) {
        let cap = (rate as f64 * max_secs) as usize;
        if rate != self.rate || !self.active {
            self.buf.clear();
            self.active = false;
            self.written = 0;
        }
        self.rate = rate;
        self.cap_frames = cap;
        self.trim();
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.active = false;
        self.written = 0;
    }

    pub fn bytes(&self) -> usize {
        self.buf.len() * 2
    }

    fn trim(&mut self) {
        let cap = self.cap_frames * 2;
        if self.buf.len() > cap {
            let excess = self.buf.len() - cap;
            self.buf.drain(..excess);
        }
    }

    fn push_silence(&mut self, frames: usize) {
        let frames = frames.min(self.cap_frames);
        self.buf.extend(std::iter::repeat(0i16).take(frames * 2));
    }

    /// Append a packet that finished arriving at `now`. Loopback delivers nothing
    /// while the system is silent, so gaps are padded to stay on the wall clock.
    pub fn push(&mut self, stereo: &[i16], now: f64) {
        let n = stereo.len() / 2;
        if n == 0 || self.cap_frames == 0 {
            return;
        }
        let rate = self.rate as f64;
        if !self.active {
            self.t0 = now - n as f64 / rate;
            self.written = 0;
            self.active = true;
        }
        let expected_end = ((now - self.t0) * rate) as i64;
        let diff = expected_end - (self.written as i64 + n as i64);
        let threshold = (self.rate / 10) as i64;
        let mut data = stereo;
        if diff > threshold {
            self.push_silence(diff as usize);
            self.written += diff as u64;
        } else if diff < -threshold {
            // Device clock ran ahead of the wall clock: drop the surplus.
            let drop = ((-diff) as usize).min(n);
            data = &stereo[drop * 2..];
        }
        self.buf.extend(data.iter().copied());
        self.written += (data.len() / 2) as u64;
        self.trim();
    }

    /// Audio for a list of wall-clock ranges, concatenated. Missing audio is silence.
    pub fn slices(&self, segs: &[(f64, f64)]) -> Option<Pcm> {
        if !self.active {
            return None;
        }
        let rate = self.rate as f64;
        let have = (self.buf.len() / 2) as i64;
        let first = self.written as i64 - have;
        let end = self.written as i64;
        let mut out: Vec<i16> = Vec::new();
        for &(a, b) in segs {
            let ia = ((a - self.t0) * rate).round() as i64;
            let ib = ((b - self.t0) * rate).round() as i64;
            if ib <= ia {
                continue;
            }
            let lo = ia.clamp(first, end);
            let hi = ib.clamp(first, end);
            out.extend(std::iter::repeat(0i16).take(((lo - ia) * 2) as usize));
            if hi > lo {
                let s = ((lo - first) * 2) as usize;
                let e = ((hi - first) * 2) as usize;
                out.extend(self.buf.range(s..e).copied());
            }
            out.extend(std::iter::repeat(0i16).take(((ib - hi) * 2) as usize));
        }
        Some(Pcm { rate: self.rate, data: out })
    }
}

fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Fold any channel layout down to stereo (front L/R plus centre).
fn downmix(frame: &[f32]) -> (f32, f32) {
    match frame.len() {
        0 => (0.0, 0.0),
        1 => (frame[0], frame[0]),
        2 => (frame[0], frame[1]),
        _ => (frame[0] + 0.7 * frame[2], frame[1] + 0.7 * frame[2]),
    }
}

fn build_stream(device: &cpal::Device, loopback: bool, ring: Arc<Mutex<AudioRing>>, max_secs: f64, failed: Arc<AtomicBool>) -> Option<Stream> {
    let cfg = if loopback { device.default_output_config().ok()? } else { device.default_input_config().ok()? };
    let rate = cfg.sample_rate().0;
    let ch = cfg.channels() as usize;
    let fmt = cfg.sample_format();
    let scfg: StreamConfig = cfg.into();
    ring.lock().unwrap().configure(rate, max_secs);

    let err = move |_e| failed.store(true, Ordering::Relaxed);
    let mut scratch: Vec<i16> = Vec::new();
    let mut frame = vec![0f32; ch.max(1)];
    // One closure body per sample format; `conv` turns a sample into f32.
    macro_rules! stream {
        ($t:ty, $conv:expr) => {
            device.build_input_stream(
                &scfg,
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let now = now_secs();
                    scratch.clear();
                    for f in data.chunks_exact(ch.max(1)) {
                        for (dst, s) in frame.iter_mut().zip(f.iter()) {
                            *dst = $conv(*s);
                        }
                        let (l, r) = downmix(&frame);
                        scratch.push(to_i16(l));
                        scratch.push(to_i16(r));
                    }
                    if let Ok(mut ring) = ring.lock() {
                        ring.push(&scratch, now);
                    }
                },
                err,
                None,
            )
        };
    }
    let stream = match fmt {
        SampleFormat::F32 => stream!(f32, |s: f32| s),
        SampleFormat::I16 => stream!(i16, |s: i16| s as f32 / 32768.0),
        SampleFormat::U16 => stream!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0),
        SampleFormat::I32 => stream!(i32, |s: i32| s as f32 / 2_147_483_648.0),
        _ => return None,
    }
    .ok()?;
    stream.play().ok()?;
    Some(stream)
}

fn device_name(d: &Option<cpal::Device>) -> String {
    d.as_ref().and_then(|d| d.name().ok()).unwrap_or_default()
}

/// Run capture until `stop` is set. Streams are rebuilt when a device errors
/// out or the Windows default device changes.
pub fn spawn(desk: Arc<Mutex<AudioRing>>, mic: Arc<Mutex<AudioRing>>, want_desk: bool, want_mic: bool, max_secs: f64, stop: Arc<AtomicBool>) {
    if !want_desk {
        desk.lock().unwrap().clear();
    }
    if !want_mic {
        mic.lock().unwrap().clear();
    }
    if !want_desk && !want_mic {
        return;
    }
    thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let host = cpal::default_host();
                let failed = Arc::new(AtomicBool::new(false));
                let out_dev = if want_desk { host.default_output_device() } else { None };
                let in_dev = if want_mic { host.default_input_device() } else { None };
                let names = (device_name(&out_dev), device_name(&in_dev));
                let _s1 = out_dev.as_ref().and_then(|d| build_stream(d, true, desk.clone(), max_secs, failed.clone()));
                let _s2 = in_dev.as_ref().and_then(|d| build_stream(d, false, mic.clone(), max_secs, failed.clone()));
                let mut ticks = 0u32;
                loop {
                    thread::sleep(Duration::from_millis(250));
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if failed.load(Ordering::Relaxed) {
                        break;
                    }
                    ticks += 1;
                    if ticks % 16 == 0 {
                        let host = cpal::default_host();
                        let cur = (
                            device_name(&if want_desk { host.default_output_device() } else { None }),
                            device_name(&if want_mic { host.default_input_device() } else { None }),
                        );
                        if cur != names {
                            break;
                        }
                    }
                }
                thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}
