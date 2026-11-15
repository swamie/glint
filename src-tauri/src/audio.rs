//! Desktop (WASAPI loopback) and microphone capture into wall-clock-anchored rings.
use crate::spill::Seg;
use crate::util::now_secs;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::path::PathBuf;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

    /// Bring the ring up to `now` with silence when nothing has arrived for a
    /// while. Process loopback sends no packets at all while its process is
    /// silent; without this the ring would stop short of the present until the
    /// next sound. Starts the clock on a ring that has had nothing yet.
    pub fn pad_to(&mut self, now: f64) {
        if self.cap_frames == 0 {
            return;
        }
        if !self.active {
            self.t0 = now;
            self.written = 0;
            self.active = true;
            return;
        }
        let behind = ((now - self.t0) * self.rate as f64) as i64 - self.written as i64;
        if behind > (self.rate / 10) as i64 {
            // One silent frame ending at `now`: `push` fills the gap before it in pieces.
            self.push(&[0, 0], now);
        }
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

/* ---------- the mixer of Settings → Audio: a gain per source, and its live level ---------- */

/// A source with a fader and a meter in the mixer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Desktop = 0,
    Mic = 1,
    Game = 2,
}

/// The fader's range in dB. The gain is applied as the sound is captured, so
/// the rings, every clip and the meters hold what the fader made of it.
pub const GAIN_DB_MIN: f32 = -40.0;
pub const GAIN_DB_MAX: f32 = 12.0;

const UNITY: u32 = 0x3f80_0000; // 1.0f32
/// Linear gain per source, as f32 bits.
static GAIN: [AtomicU32; 3] = [AtomicU32::new(UNITY), AtomicU32::new(UNITY), AtomicU32::new(UNITY)];
/// The loudest sample (0..1, times a million) of the left and the right
/// channel of each source since `levels` last read them.
static PEAK: [[AtomicU32; 2]; 3] = [
    [AtomicU32::new(0), AtomicU32::new(0)],
    [AtomicU32::new(0), AtomicU32::new(0)],
    [AtomicU32::new(0), AtomicU32::new(0)],
];

/// A fader position in dB as a linear factor (clamped to the fader's range; not a number = 0 dB).
pub fn db_to_gain(db: f32) -> f32 {
    let db = if db.is_finite() { db.clamp(GAIN_DB_MIN, GAIN_DB_MAX) } else { 0.0 };
    10f32.powf(db / 20.0)
}

/// Set the three faders (dB). Takes effect on the next audio packet; nothing restarts.
pub fn set_gains(desktop_db: f32, mic_db: f32, game_db: f32) {
    for (i, db) in [desktop_db, mic_db, game_db].into_iter().enumerate() {
        GAIN[i].store(db_to_gain(db).to_bits(), Ordering::Relaxed);
    }
}

/// The linear gain of `src` right now.
pub fn gain(src: Source) -> f32 {
    f32::from_bits(GAIN[src as usize].load(Ordering::Relaxed))
}

/// Report the loudest left and right sample (0..1, after the gain) of a packet of `src`.
pub fn note_peak(src: Source, left: f32, right: f32) {
    for (ch, v) in [left, right].into_iter().enumerate() {
        PEAK[src as usize][ch].fetch_max((v.clamp(0.0, 1.0) * 1_000_000.0) as u32, Ordering::Relaxed);
    }
}

/// One source's level for the meters: the loudest left and right sample
/// since the last call, in dB below full scale (-120 = silence).
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub left: f32,
    pub right: f32,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub desktop: Level,
    pub mic: Level,
    pub game: Level,
}

/// Read the levels and start a new measuring interval.
pub fn levels() -> Levels {
    let db = |a: &AtomicU32| {
        let v = a.swap(0, Ordering::Relaxed) as f32 / 1_000_000.0;
        if v > 0.000_001 { (20.0 * v.log10()).max(-120.0) } else { -120.0 }
    };
    let one = |s: Source| Level { left: db(&PEAK[s as usize][0]), right: db(&PEAK[s as usize][1]) };
    Levels { desktop: one(Source::Desktop), mic: one(Source::Mic), game: one(Source::Game) }
}

fn build_stream(device: &cpal::Device, loopback: bool, ring: Arc<Mutex<AudioRing>>, max_secs: f64, failed: Arc<AtomicBool>) -> Option<Stream> {
    let source = if loopback { Source::Desktop } else { Source::Mic };
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
                    // The mixer's fader for this source, and the loudest sample of the packet for its meter.
                    let g = gain(source);
                    let (mut pl, mut pr) = (0f32, 0f32);
                    for f in data.chunks_exact(ch.max(1)) {
                        for (dst, s) in frame.iter_mut().zip(f.iter()) {
                            *dst = $conv(*s);
                        }
                        let (l, r) = downmix(&frame);
                        let (l, r) = (l * g, r * g);
                        pl = pl.max(l.abs());
                        pr = pr.max(r.abs());
                        scratch.push(to_i16(l));
                        scratch.push(to_i16(r));
                    }
                    note_peak(source, pl, pr);
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

/// The devices chosen in Settings → Audio, by name ("" = the Windows default),
/// and a counter that moves on every change so a running capture switches.
static CHOICE: Mutex<(String, String, u64)> = Mutex::new((String::new(), String::new(), 0));

/// Set the output device whose sound is recorded and the microphone. A running
/// capture rebuilds its streams within a quarter of a second; nothing else restarts.
pub fn set_devices(desk: &str, mic: &str) {
    let mut c = CHOICE.lock().unwrap();
    if c.0 != desk || c.1 != mic {
        c.0 = desk.to_string();
        c.1 = mic.to_string();
        c.2 += 1;
    }
}

fn choice() -> (String, String, u64) {
    CHOICE.lock().unwrap().clone()
}

/// Which of `names` the user's choice `want` is: the exact name first, then the
/// same name in other case or with spaces around it. None = use the default
/// ("" chosen, or the device is not there: unplugged).
pub fn pick(names: &[String], want: &str) -> Option<usize> {
    let w = want.trim();
    if w.is_empty() {
        return None;
    }
    names.iter().position(|n| n == want).or_else(|| names.iter().position(|n| n.trim().eq_ignore_ascii_case(w)))
}

/// The device to capture: the chosen one when it is there, else the Windows default.
fn open_device(host: &cpal::Host, output: bool, want: &str) -> Option<cpal::Device> {
    if !want.trim().is_empty() {
        let devs: Vec<cpal::Device> = match if output { host.output_devices() } else { host.input_devices() } {
            Ok(d) => d.collect(),
            Err(_) => Vec::new(),
        };
        let names: Vec<String> = devs.iter().map(|d| d.name().unwrap_or_default()).collect();
        if let Some(i) = pick(&names, want) {
            return devs.into_iter().nth(i);
        }
    }
    if output { host.default_output_device() } else { host.default_input_device() }
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub default: bool,
}

#[derive(serde::Serialize, Clone, Debug, Default)]
pub struct DeviceList {
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

/// The output and input devices Windows has now, the default ones marked. Opens no stream.
pub fn list_devices() -> DeviceList {
    // A thread of its own: cpal sets COM up per thread, away from the caller's apartment.
    thread::spawn(|| {
        let host = cpal::default_host();
        let one = |devs: Option<Vec<cpal::Device>>, def: String| -> Vec<DeviceInfo> {
            let mut out: Vec<DeviceInfo> = Vec::new();
            for d in devs.unwrap_or_default() {
                let Ok(name) = d.name() else { continue };
                if name.is_empty() || out.iter().any(|o| o.name == name) {
                    continue;
                }
                out.push(DeviceInfo { default: name == def, name });
            }
            out
        };
        let outputs = one(host.output_devices().ok().map(|d| d.collect()), device_name(&host.default_output_device()));
        let inputs = one(host.input_devices().ok().map(|d| d.collect()), device_name(&host.default_input_device()));
        DeviceList { outputs, inputs }
    })
    .join()
    .unwrap_or_default()
}

/// Run capture until `stop` is set. Streams are rebuilt when a device errors
/// out, the device in use changes (the Windows default, the choice in
/// Settings → Audio, or a chosen device that was missing comes back).
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
            let mut logged = (String::new(), String::new());
            while !stop.load(Ordering::Relaxed) {
                let host = cpal::default_host();
                let failed = Arc::new(AtomicBool::new(false));
                let (want_out, want_in, gen) = choice();
                let out_dev = if want_desk { open_device(&host, true, &want_out) } else { None };
                let in_dev = if want_mic { open_device(&host, false, &want_in) } else { None };
                let names = (device_name(&out_dev), device_name(&in_dev));
                if names != logged {
                    crate::util::log(&format!("audio: desktop \"{}\", microphone \"{}\"", names.0, names.1));
                    logged = names.clone();
                }
                let _s1 = out_dev.as_ref().and_then(|d| build_stream(d, true, desk.clone(), max_secs, failed.clone()));
                let _s2 = in_dev.as_ref().and_then(|d| build_stream(d, false, mic.clone(), max_secs, failed.clone()));
                let mut ticks = 0u32;
                loop {
                    thread::sleep(Duration::from_millis(250));
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if failed.load(Ordering::Relaxed) || CHOICE.lock().unwrap().2 != gen {
                        break;
                    }
                    ticks += 1;
                    if ticks % 16 == 0 {
                        let host = cpal::default_host();
                        let cur = (
                            device_name(&if want_desk { open_device(&host, true, &want_out) } else { None }),
                            device_name(&if want_mic { open_device(&host, false, &want_in) } else { None }),
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

/// `glint.exe --audio-test <report.txt>`: choosing a device by name with the
/// fallback, the settings keys, old settings files. It lists the devices
/// Windows has but opens no stream: nothing is recorded or played.
pub fn self_test(report_path: &str) {
    use std::io::Write;
    let path = PathBuf::from(report_path);
    let _ = std::fs::remove_file(&path);
    let (mut passed, mut failed) = (0u32, 0u32);
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok { passed += 1 } else { failed += 1 }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  [{detail}]") });
        }
    };
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        check("refused", false, "set GLINT_DATA_DIR so the real settings are never touched".into());
        return;
    }

    // Choosing a device by name.
    let names: Vec<String> = ["Speakers (Realtek(R) Audio)", "Headphones (USB Audio)", "Microphone (USB Audio)", "Headphones (USB Audio)"].iter().map(|s| s.to_string()).collect();
    check("pick: \"\" is the Windows default", pick(&names, "").is_none() && pick(&names, "   ").is_none(), String::new());
    check("pick: the exact name", pick(&names, "Headphones (USB Audio)") == Some(1), format!("{:?}", pick(&names, "Headphones (USB Audio)")));
    check("pick: other case and spaces around it still match", pick(&names, "  speakers (realtek(r) audio) ") == Some(0), format!("{:?}", pick(&names, "  speakers (realtek(r) audio) ")));
    check("pick: an exact match wins over a case match", pick(&["mic".to_string(), "Mic".to_string()], "Mic") == Some(1), String::new());
    check("pick: an unplugged device falls back to the default", pick(&names, "Arctis 7 Game").is_none(), String::new());
    check("pick: no device at all falls back to the default", pick(&[], "Headphones (USB Audio)").is_none(), String::new());
    check("pick: a part of a name is not a match", pick(&names, "Headphones").is_none(), String::new());

    // The mixer (0.1.32): faders in dB, the gain a capture reads, the meters' levels. Nothing is captured here.
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    check("mixer: 0 dB is unity, -6 dB about half, +6 dB about double", near(db_to_gain(0.0), 1.0) && near(db_to_gain(-6.0206), 0.5) && near(db_to_gain(6.0206), 2.0), format!("{} {}", db_to_gain(-6.0206), db_to_gain(6.0206)));
    check("mixer: the fader stops at its ends, and not-a-number is 0 dB", near(db_to_gain(-500.0), db_to_gain(GAIN_DB_MIN)) && near(db_to_gain(99.0), db_to_gain(GAIN_DB_MAX)) && near(db_to_gain(f32::NAN), 1.0), String::new());
    set_gains(-6.0206, 0.0, 6.0206);
    check("mixer: each source reads its own fader", near(gain(Source::Desktop), 0.5) && near(gain(Source::Mic), 1.0) && near(gain(Source::Game), 2.0), format!("{} {} {}", gain(Source::Desktop), gain(Source::Mic), gain(Source::Game)));
    set_gains(0.0, 0.0, 0.0);
    let _ = levels();
    note_peak(Source::Mic, 0.5, 0.1);
    note_peak(Source::Mic, 0.25, 1.0);
    let lv = levels();
    check("mixer: a meter shows the loudest sample of each channel since the last reading", near(lv.mic.left, -6.02) && near(lv.mic.right, 0.0), format!("{:?}", lv.mic));
    check("mixer: sources that got nothing are silent", lv.desktop.left <= -119.0 && lv.game.right <= -119.0, format!("{:?} {:?}", lv.desktop, lv.game));
    let again = levels();
    check("mixer: a reading starts a new interval", again.mic.left <= -119.0 && again.mic.right <= -119.0, format!("{:?}", again.mic));
    let old: Result<crate::settings::Settings, _> = serde_json::from_str(r#"{"mic":true,"desktop_audio":true}"#);
    check("mixer: settings from before the faders load with 0 dB", old.as_ref().is_ok_and(|s| s.desktop_gain_db == 0.0 && s.mic_gain_db == 0.0 && s.game_gain_db == 0.0), String::new());

    // The choice reaches a running capture through a counter that moves only on a change.
    let g0 = choice().2;
    set_devices("A", "B");
    let g1 = choice().2;
    set_devices("A", "B");
    let g2 = choice().2;
    set_devices("A", "");
    let c = choice();
    check("set_devices: a change moves the counter, the same choice does not", g1 == g0 + 1 && g2 == g1 && c.2 == g1 + 1 && c.0 == "A" && c.1.is_empty(), format!("{g0} {g1} {g2} {}", c.2));
    set_devices("", "");

    // The settings keys.
    let d = crate::settings::Settings::default();
    check("settings: both devices default to \"\" (the Windows default)", d.desktop_device.is_empty() && d.mic_device.is_empty(), String::new());
    let old: Result<crate::settings::Settings, _> = serde_json::from_str(r#"{"desktop_audio":true,"mic":false,"mic_denoise":true,"game_audio":true}"#);
    check("settings: a file from before 0.1.27 loads, devices \"\"", old.as_ref().is_ok_and(|s| s.desktop_device.is_empty() && s.mic_device.is_empty() && !s.mic && s.mic_denoise && s.game_audio), format!("{:?}", old.as_ref().err()));
    let mut s = crate::settings::Settings::default();
    s.desktop_device = "Headphones (USB Audio)".into();
    s.mic_device = "Microphone (Яндекс 🎤)".into();
    let json = serde_json::to_value(&s).unwrap_or_default();
    check("settings: saved under the keys desktop_device and mic_device", json["desktop_device"] == "Headphones (USB Audio)" && json["mic_device"] == "Microphone (Яндекс 🎤)", String::new());
    let back: Option<crate::settings::Settings> = serde_json::from_value(json).ok();
    check("settings: the names come back unchanged", back.is_some_and(|b| b.desktop_device == s.desktop_device && b.mic_device == s.mic_device), String::new());

    // The device list (no stream is opened).
    let t0 = std::time::Instant::now();
    let list = list_devices();
    let ms = t0.elapsed().as_millis();
    let detail = format!("{} outputs, {} inputs, {ms} ms", list.outputs.len(), list.inputs.len());
    let ok = |v: &[DeviceInfo]| v.iter().filter(|d| d.default).count() <= 1 && v.iter().all(|d| !d.name.is_empty()) && v.iter().enumerate().all(|(i, d)| v.iter().position(|e| e.name == d.name) == Some(i));
    check("list: names are set, unique, at most one default per list", ok(&list.outputs) && ok(&list.inputs), detail.clone());
    check("list: an output is marked default when there are outputs", list.outputs.is_empty() || list.outputs.iter().any(|d| d.default), detail.clone());
    check("list: quick enough for the Refresh button", ms < 3000, detail.clone());
    let json = serde_json::to_value(&list).unwrap_or_default();
    check("list: JSON shape { outputs: [{ name, default }], inputs }", json["outputs"].is_array() && json["inputs"].is_array() && list.outputs.first().map_or(true, |_|json["outputs"][0]["name"].is_string() && json["outputs"][0]["default"].is_boolean()), String::new());
    let names: Vec<String> = list.outputs.iter().map(|d| d.name.clone()).collect();
    check("list: every listed output is found again by its name", names.iter().enumerate().all(|(i, n)| pick(&names, n) == Some(i)), detail);

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "DONE passed={passed} failed={failed}");
    }
}
