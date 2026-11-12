//! Desktop (WASAPI loopback) and microphone capture into wall-clock-anchored rings.
use crate::spill::Seg;
use crate::util::now_secs;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Interleaved stereo s16 samples.
pub struct Pcm {
    pub rate: u32,
    pub data: Vec<i16>,
}

/// Audio leaves RAM in pieces of this many seconds, one file each.
const CHUNK_SECS: usize = 10;

/// A piece of the ring that has left RAM. Frames `first .. first + frames`.
struct Chunk {
    /// The file with the samples (s16le stereo). `None` = the piece was pure
    /// silence, which needs no file: loopback is silent most of the time.
    seg: Option<Arc<Seg>>,
    first: u64,
    frames: usize,
}

/// One step of reading audio out of the ring.
enum Part {
    Silence(usize),
    Ram(Vec<i16>),
    Disk { seg: Arc<Seg>, off: u64, frames: usize },
}

/// What `AudioRing::plan` found for a list of ranges. Made under the ring's
/// lock; `read` then fetches the parts that are on disk without holding it.
pub struct Plan {
    rate: u32,
    parts: Vec<Part>,
}

impl Plan {
    pub fn read(self) -> Pcm {
        let frames: usize = self.parts.iter().map(|p| match p {
            Part::Silence(n) => *n,
            Part::Ram(d) => d.len() / 2,
            Part::Disk { frames, .. } => *frames,
        }).sum();
        let mut out: Vec<i16> = Vec::with_capacity(frames * 2);
        for p in self.parts {
            match p {
                Part::Silence(n) => out.extend(std::iter::repeat(0i16).take(n * 2)),
                Part::Ram(d) => out.extend_from_slice(&d),
                Part::Disk { seg, off, frames } => {
                    let mut bytes = vec![0u8; frames * 4];
                    // A piece that cannot be read back is silence, not a failed clip.
                    if seg.read_at(&mut bytes, off).is_err() {
                        bytes.fill(0);
                    }
                    out.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
                }
            }
        }
        Pcm { rate: self.rate, data: out }
    }
}

/// Ring of stereo s16 audio. Frame `n` was captured at `t0 + n / rate` on the
/// app clock, so any wall-clock range can be cut out later and lined up with video.
///
/// The newest audio is in `buf`. With a RAM window set, older audio is moved
/// to files in pieces (`chunks`), which sit before `buf` without a gap.
pub struct AudioRing {
    rate: u32,
    buf: VecDeque<i16>,
    cap_frames: usize,
    t0: f64,
    written: u64,
    active: bool,
    /// Seconds kept in RAM. None = everything stays in RAM.
    window: Option<f64>,
    dir: PathBuf,
    tag: &'static str,
    chunks: VecDeque<Chunk>,
    /// Frames in `chunks`, and the bytes of those that have a file.
    disk_frames: usize,
    disk_bytes: usize,
    /// A write failed: stay in RAM until the capture is started again.
    broken: bool,
}

impl AudioRing {
    pub fn new() -> Self {
        AudioRing {
            rate: 48_000,
            buf: VecDeque::new(),
            cap_frames: 0,
            t0: 0.0,
            written: 0,
            active: false,
            window: None,
            dir: PathBuf::new(),
            tag: "a",
            chunks: VecDeque::new(),
            disk_frames: 0,
            disk_bytes: 0,
            broken: false,
        }
    }

    pub fn configure(&mut self, rate: u32, max_secs: f64) {
        let cap = (rate as f64 * max_secs) as usize;
        if rate != self.rate || !self.active {
            self.clear();
        }
        self.rate = rate;
        self.cap_frames = cap;
        self.trim();
    }

    /// Choose where older audio lives. `window`: see `spill::window`. Audio
    /// already buffered stays readable wherever it is.
    pub fn set_spill(&mut self, window: Option<f64>, dir: PathBuf, tag: &'static str) {
        self.window = window;
        self.dir = dir;
        self.tag = tag;
        self.broken = false;
        if window.is_some() {
            self.trim();
            self.buf.shrink_to_fit();
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.chunks.clear();
        self.disk_frames = 0;
        self.disk_bytes = 0;
        self.active = false;
        self.written = 0;
    }

    /// Bytes held in RAM and in files.
    pub fn bytes(&self) -> (usize, usize) {
        (self.buf.len() * 2, self.disk_bytes)
    }

    /// Move the oldest `n` frames of `buf` out of RAM. False if the file could not be written.
    fn spill_chunk(&mut self, n: usize) -> bool {
        let first = self.written - (self.buf.len() / 2) as u64;
        let seg = if self.buf.range(..n * 2).all(|s| *s == 0) {
            None
        } else {
            let bytes: Vec<u8> = self.buf.range(..n * 2).flat_map(|s| s.to_le_bytes()).collect();
            match Seg::create(&self.dir, self.tag).and_then(|seg| seg.write_at(&bytes, 0).map(|_| seg)) {
                Ok(seg) => Some(Arc::new(seg)),
                Err(e) => {
                    crate::util::log(&format!("audio buffer: cannot write to {} ({e}); staying in RAM", self.dir.display()));
                    self.broken = true;
                    return false;
                }
            }
        };
        self.buf.drain(..n * 2);
        if seg.is_some() {
            self.disk_bytes += n * 4;
        }
        self.disk_frames += n;
        self.chunks.push_back(Chunk { seg, first, frames: n });
        true
    }

    fn trim(&mut self) {
        if let Some(window) = self.window {
            let chunk = self.rate as usize * CHUNK_SECS;
            let keep = (window * self.rate as f64) as usize;
            while !self.broken && self.buf.len() / 2 >= keep + chunk {
                if !self.spill_chunk(chunk) {
                    break;
                }
            }
        }
        // Drop pieces that lie wholly outside the capacity. The ring may hold up
        // to one piece more than its capacity, never less than it.
        let mut total = self.disk_frames + self.buf.len() / 2;
        while let Some(c) = self.chunks.front() {
            if total - c.frames < self.cap_frames {
                break;
            }
            total -= c.frames;
            self.disk_frames -= c.frames;
            if c.seg.is_some() {
                self.disk_bytes -= c.frames * 4;
            }
            self.chunks.pop_front();
        }
        if self.chunks.is_empty() {
            let cap = self.cap_frames * 2;
            if self.buf.len() > cap {
                let excess = self.buf.len() - cap;
                self.buf.drain(..excess);
            }
        }
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
            let diff = diff as usize;
            let fill = diff.min(self.cap_frames);
            if diff >= self.cap_frames {
                // The gap is longer than the whole ring: nothing older is needed.
                self.buf.clear();
                self.chunks.clear();
                self.disk_frames = 0;
                self.disk_bytes = 0;
            }
            self.written += (diff - fill) as u64;
            // In pieces, so a long silence never sits in RAM all at once when the ring spills to disk.
            let piece = if self.window.is_some() { self.rate as usize * CHUNK_SECS } else { fill };
            let mut left = fill;
            while left > 0 {
                let m = left.min(piece.max(1));
                self.buf.extend(std::iter::repeat(0i16).take(m * 2));
                self.written += m as u64;
                left -= m;
                self.trim();
            }
        } else if diff < -threshold {
            // Device clock ran ahead of the wall clock: drop the surplus.
            let drop = ((-diff) as usize).min(n);
            data = &stereo[drop * 2..];
        }
        self.buf.extend(data.iter().copied());
        self.written += (data.len() / 2) as u64;
        self.trim();
    }

    /// Where the audio for a list of wall-clock ranges is. Missing audio is silence.
    pub fn plan(&self, segs: &[(f64, f64)]) -> Option<Plan> {
        if !self.active {
            return None;
        }
        let rate = self.rate as f64;
        let end = self.written as i64;
        let buf_first = end - (self.buf.len() / 2) as i64;
        let first = self.chunks.front().map(|c| c.first as i64).unwrap_or(buf_first);
        let mut parts: Vec<Part> = Vec::new();
        for &(a, b) in segs {
            let ia = ((a - self.t0) * rate).round() as i64;
            let ib = ((b - self.t0) * rate).round() as i64;
            if ib <= ia {
                continue;
            }
            let lo = ia.clamp(first, end);
            let hi = ib.clamp(first, end);
            if hi <= lo {
                // Wholly before or after what the ring holds.
                parts.push(Part::Silence((ib - ia) as usize));
                continue;
            }
            if lo > ia {
                parts.push(Part::Silence((lo - ia) as usize));
            }
            if hi > lo {
                for c in &self.chunks {
                    let (c0, c1) = (c.first as i64, c.first as i64 + c.frames as i64);
                    let (x, y) = (lo.max(c0), hi.min(c1));
                    if y > x {
                        parts.push(match &c.seg {
                            Some(seg) => Part::Disk { seg: seg.clone(), off: (x - c0) as u64 * 4, frames: (y - x) as usize },
                            None => Part::Silence((y - x) as usize),
                        });
                    }
                }
                let x = lo.max(buf_first);
                if hi > x {
                    let s = ((x - buf_first) * 2) as usize;
                    let e = ((hi - buf_first) * 2) as usize;
                    parts.push(Part::Ram(self.buf.range(s..e).copied().collect()));
                }
            }
            if ib > hi {
                parts.push(Part::Silence((ib - hi) as usize));
            }
        }
        Some(Plan { rate: self.rate, parts })
    }

    /// Audio for a list of wall-clock ranges, concatenated.
    pub fn slices(&self, segs: &[(f64, f64)]) -> Option<Pcm> {
        self.plan(segs).map(Plan::read)
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
