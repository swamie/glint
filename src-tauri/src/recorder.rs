//! Always-on replay buffer.
//!
//! One ffmpeg process captures the monitor on the GPU, encodes with the
//! hardware encoder and writes MPEG-TS to a pipe. This module keeps the last
//! N seconds of that stream, split at keyframes, so a clip is a cheap copy of
//! already-encoded data. The buffer lives in RAM, on disk, or both (the newest
//! seconds in RAM, older ones in files): see `spill`.
use crate::audio::{self, AudioRing, Pcm, Plan};
use crate::spill::{self, Seg};
use crate::util::{self, now_secs};
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TS: usize = 188;
const VIDEO_PID: u16 = 0x100;
const PMT_PID: u16 = 0x1000;
/// Capture-to-pipe delay that remains after the per-epoch minimum is removed.
const LATENCY: f64 = 0.012;

#[derive(Clone, Debug, PartialEq)]
pub struct CaptureCfg {
    pub ffmpeg: Option<PathBuf>,
    pub monitor: u32,
    pub width: u32,
    pub height: u32,
    pub scaled: bool,
    pub fps: u32,
    /// Monitor refresh rate.
    pub hz: u32,
    pub quality: String,
    /// Fixed video bitrate in Mbps. 0 = automatic: quality-targeted, capped by `quality`.
    pub bitrate: u32,
    pub encoder: String,
    pub method: String,
    pub cursor: bool,
    pub desktop_audio: bool,
    pub mic: bool,
    pub max_secs: f64,
    /// Where the buffer lives: "mixed" (newest `buffer_ram_secs` in RAM, the rest on disk), "ram" or "disk".
    pub buffer: String,
    pub buffer_ram_secs: f64,
    /// Folder for the buffer files.
    pub buffer_dir: PathBuf,
}

impl CaptureCfg {
    /// Video bitrate in bits/s: the fixed rate if one is set, otherwise the ceiling
    /// of the quality-targeted mode (real usage is lower there).
    pub fn maxrate(&self) -> u64 {
        if self.bitrate > 0 {
            return self.bitrate as u64 * 1_000_000;
        }
        let bpp = match self.quality.as_str() {
            "low" => 0.035,
            "medium" => 0.055,
            "ultra" => 0.12,
            _ => 0.08,
        };
        let fps_factor = (self.fps as f64 / 60.0).powf(0.6);
        ((self.width as f64 * self.height as f64 * bpp * 60.0 * fps_factor) as u64).max(1_000_000)
    }

    fn cq(&self) -> u32 {
        match self.quality.as_str() {
            "low" => 31,
            "medium" => 27,
            "ultra" => 20,
            _ => 24,
        }
    }
}

/// Where a closed GOP's bytes are.
enum Store {
    Ram(Vec<u8>),
    Disk { seg: Arc<Seg>, off: u64, len: usize },
}

/// One closed group of pictures: a keyframe and everything up to the next one.
pub struct Gop {
    pub pts: f64,
    pub arr: f64,
    pub epoch: u32,
    store: Store,
}

impl Gop {
    pub fn len(&self) -> usize {
        match &self.store {
            Store::Ram(d) => d.len(),
            Store::Disk { len, .. } => *len,
        }
    }

    pub fn on_disk(&self) -> bool {
        matches!(self.store, Store::Disk { .. })
    }

    /// Copy the GOP's bytes to `w`, reading them back from disk if they are there.
    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        match &self.store {
            Store::Ram(d) => w.write_all(d),
            Store::Disk { seg, off, len } => {
                let mut buf = vec![0u8; (*len).min(1 << 20)];
                let mut done = 0usize;
                while done < *len {
                    let n = (*len - done).min(buf.len());
                    seg.read_at(&mut buf[..n], off + done as u64)?;
                    w.write_all(&buf[..n])?;
                    done += n;
                }
                Ok(())
            }
        }
    }
}

/// The GOP still being received. It is always in RAM.
struct Open {
    pts: f64,
    arr: f64,
    epoch: u32,
    data: Vec<u8>,
}

/// One ffmpeg run. PTS restart with every run, so each keeps its own clock offset.
struct Epoch {
    id: u32,
    /// Smallest (arrival - pts) seen: maps stream time to the app clock.
    min_delta: f64,
    last_arr: f64,
}

pub struct Snapshot {
    pub header: Vec<u8>,
    pub gops: Vec<Arc<Gop>>,
    /// Wall-clock ranges the video covers; audio is cut to the same ranges.
    pub segments: Vec<(f64, f64)>,
    pub duration: f64,
}

pub struct VideoRing {
    /// Oldest first. The first `ram_start` are on disk, the rest in RAM.
    gops: VecDeque<Arc<Gop>>,
    ram_start: usize,
    cur: Option<Open>,
    /// Bytes held in RAM (closed GOPs still in RAM plus the open one).
    bytes: usize,
    /// Bytes held in buffer files.
    disk_bytes: usize,
    pat: Vec<u8>,
    pmt: Vec<u8>,
    epochs: VecDeque<Epoch>,
    epoch: u32,
    live: bool,
    carry: Vec<u8>,
    pub max_secs: f64,
    pub frames: u64,
    /// Seconds kept in RAM before a GOP goes to disk. None = RAM only, Some(0) = disk only.
    window: Option<f64>,
    dir: PathBuf,
    /// The file being filled, and how much of it is used.
    seg: Option<(Arc<Seg>, u64)>,
    /// A new file is started once the current one reaches this size, so disk
    /// space comes back in steps of this size as the buffer moves on.
    seg_limit: u64,
    /// Size of the last closed GOP: the next one is given that much room up front.
    last_len: usize,
    /// After a failed write nothing more is written until this time; the buffer stays in RAM.
    retry_at: f64,
    /// Why the buffer is not going to disk although it should ("" = it is).
    pub spill_error: String,
}

/// Below this much free space the buffer stops going to disk.
const MIN_FREE: u64 = 1 << 30;

impl VideoRing {
    fn new() -> Self {
        VideoRing {
            gops: VecDeque::new(),
            ram_start: 0,
            cur: None,
            bytes: 0,
            disk_bytes: 0,
            pat: Vec::new(),
            pmt: Vec::new(),
            epochs: VecDeque::new(),
            epoch: 0,
            live: false,
            carry: Vec::new(),
            max_secs: 35.0,
            frames: 0,
            window: None,
            dir: PathBuf::new(),
            seg: None,
            seg_limit: 64 << 20,
            last_len: 0,
            retry_at: 0.0,
            spill_error: String::new(),
        }
    }

    fn clear(&mut self) {
        // Buffer files go with the last GOP that points into them.
        self.gops.clear();
        self.ram_start = 0;
        self.cur = None;
        self.bytes = 0;
        self.disk_bytes = 0;
        self.seg = None;
        self.carry.clear();
        self.epochs.clear();
    }

    /// Choose where the buffer lives. `window`: see `spill::window`.
    fn set_spill(&mut self, window: Option<f64>, dir: PathBuf) {
        self.window = window;
        self.dir = dir;
        self.seg = None;
        self.retry_at = 0.0;
        self.spill_error.clear();
    }

    /// Append `data` to the current buffer file, starting a new one when it is full.
    fn write_seg(&mut self, data: &[u8]) -> io::Result<(Arc<Seg>, u64)> {
        let full = self.seg.as_ref().map_or(true, |(_, used)| *used > 0 && *used + data.len() as u64 > self.seg_limit);
        if full {
            if spill::free_space(&self.dir).is_some_and(|free| free < MIN_FREE) {
                return Err(io::Error::other("less than 1 GB free"));
            }
            self.seg = Some((Arc::new(Seg::create(&self.dir, "v")?), 0));
        }
        let (seg, used) = self.seg.as_mut().unwrap();
        seg.write_at(data, *used)?;
        let off = *used;
        *used += data.len() as u64;
        Ok((seg.clone(), off))
    }

    /// Move GOPs that have left the RAM window to disk.
    fn spill(&mut self, now: f64) {
        let Some(window) = self.window else { return };
        if now < self.retry_at {
            return;
        }
        while self.ram_start < self.gops.len() {
            let i = self.ram_start;
            // A GOP leaves RAM once all of it is older than the window.
            let end = self.gops.get(i + 1).map(|g| g.arr).or(self.cur.as_ref().map(|g| g.arr)).unwrap_or(now);
            if window > 0.0 && end > now - window {
                break;
            }
            let g = self.gops[i].clone();
            let Store::Ram(data) = &g.store else {
                self.ram_start += 1;
                continue;
            };
            match self.write_seg(data) {
                Ok((seg, off)) => {
                    let len = data.len();
                    // A snapshot that holds the old entry keeps its RAM copy until it is done.
                    self.gops[i] = Arc::new(Gop { pts: g.pts, arr: g.arr, epoch: g.epoch, store: Store::Disk { seg, off, len } });
                    self.bytes -= len;
                    self.disk_bytes += len;
                    self.ram_start += 1;
                    self.spill_error.clear();
                }
                Err(e) => {
                    let msg = format!("The buffer folder cannot be written ({e}); the buffer is staying in RAM. Folder: {}", self.dir.display());
                    if self.spill_error != msg {
                        util::log(&msg);
                    }
                    self.spill_error = msg;
                    self.seg = None;
                    self.retry_at = now + 10.0;
                    break;
                }
            }
        }
    }

    fn begin_epoch(&mut self, now: f64) {
        self.close_gop();
        self.carry.clear();
        self.epoch += 1;
        self.epochs.push_back(Epoch { id: self.epoch, min_delta: f64::INFINITY, last_arr: now });
        while self.epochs.len() > 64 {
            self.epochs.pop_front();
        }
        self.live = true;
    }

    fn end_epoch(&mut self) {
        self.close_gop();
        self.live = false;
    }

    fn close_gop(&mut self) {
        if let Some(mut g) = self.cur.take() {
            self.last_len = g.data.len();
            // The vector grew by doubling; give back the unused part if the GOP stays in RAM for a while.
            if self.window != Some(0.0) {
                g.data.shrink_to_fit();
            }
            self.gops.push_back(Arc::new(Gop { pts: g.pts, arr: g.arr, epoch: g.epoch, store: Store::Ram(g.data) }));
        }
    }

    fn evict(&mut self, now: f64) {
        let limit = now - self.max_secs - 3.0;
        while let Some(front) = self.gops.front() {
            // Keep a GOP while the one after it still starts inside the window.
            let next_arr = self.gops.get(1).map(|g| g.arr).or(self.cur.as_ref().map(|g| g.arr)).unwrap_or(now);
            if next_arr >= limit {
                break;
            }
            if front.on_disk() {
                self.disk_bytes -= front.len();
            } else {
                self.bytes -= front.len();
            }
            self.gops.pop_front();
            self.ram_start = self.ram_start.saturating_sub(1);
        }
    }

    /// Feed raw bytes from the encoder pipe.
    fn feed(&mut self, data: &[u8], now: f64) {
        self.carry.extend_from_slice(data);
        let mut pos = 0;
        while self.carry.len() - pos >= TS {
            if self.carry[pos] != 0x47 {
                pos += 1; // resync
                continue;
            }
            let pkt: [u8; TS] = self.carry[pos..pos + TS].try_into().unwrap();
            self.packet(&pkt, now);
            pos += TS;
        }
        self.carry.drain(..pos);
        self.evict(now);
        self.spill(now);
    }

    fn packet(&mut self, pkt: &[u8; TS], now: f64) {
        let pid = ((pkt[1] as u16 & 0x1f) << 8) | pkt[2] as u16;
        if pid == 0 {
            self.pat = pkt.to_vec();
        } else if pid == PMT_PID {
            self.pmt = pkt.to_vec();
        } else if pid == VIDEO_PID && pkt[1] & 0x40 != 0 {
            // Start of a video frame.
            let afc = (pkt[3] >> 4) & 3;
            let mut off = 4;
            let mut keyframe = false;
            if afc & 2 != 0 {
                let al = pkt[4] as usize;
                if al > 0 {
                    keyframe = pkt[5] & 0x40 != 0; // random access indicator
                }
                off = 5 + al;
            }
            self.frames += 1;
            let mut pts = None;
            if afc & 1 != 0 && off + 14 <= TS && pkt[off..off + 3] == [0, 0, 1] && pkt[off + 7] & 0x80 != 0 {
                let p = &pkt[off + 9..off + 14];
                let v = (((p[0] as u64 >> 1) & 7) << 30) | ((p[1] as u64) << 22) | ((p[2] as u64 >> 1) << 15) | ((p[3] as u64) << 7) | (p[4] as u64 >> 1);
                pts = Some(v as f64 / 90_000.0);
            }
            if let (Some(p), Some(e)) = (pts, self.epochs.back_mut()) {
                e.min_delta = e.min_delta.min(now - p);
                e.last_arr = now;
            }
            if keyframe {
                self.close_gop();
                let room = (self.last_len + self.last_len / 4).clamp(256 * 1024, 32 << 20);
                self.cur = Some(Open { pts: pts.unwrap_or(0.0), arr: now, epoch: self.epoch, data: Vec::with_capacity(room) });
            }
        }
        // Bytes before the first keyframe are useless and dropped.
        if let Some(cur) = &mut self.cur {
            cur.data.extend_from_slice(pkt);
            self.bytes += TS;
        }
    }

    /// When a GOP's first frame was on screen, on the app clock.
    fn capture_time(&self, g: &Gop) -> f64 {
        match self.epochs.iter().find(|e| e.id == g.epoch) {
            Some(e) if e.min_delta.is_finite() => g.pts + e.min_delta - LATENCY,
            _ => g.arr - LATENCY,
        }
    }

    fn buffered_secs(&self, now: f64) -> f64 {
        let first = self.gops.front().map(|g| g.arr).or(self.cur.as_ref().map(|g| g.arr));
        match first {
            Some(t) => (now - t).clamp(0.0, self.max_secs),
            None => 0.0,
        }
    }

    /// Everything needed to write the last `secs` seconds, starting on a keyframe.
    fn snapshot(&self, secs: f64, now: f64) -> Result<Snapshot, String> {
        let mut list: Vec<Arc<Gop>> = self.gops.iter().cloned().collect();
        if let Some(c) = &self.cur {
            list.push(Arc::new(Gop { pts: c.pts, arr: c.arr, epoch: c.epoch, store: Store::Ram(c.data.clone()) }));
        }
        if list.is_empty() {
            return Err("The replay buffer is empty".into());
        }
        let times: Vec<f64> = list.iter().map(|g| self.capture_time(g)).collect();
        let target = now - secs;
        let start = times.iter().rposition(|t| *t <= target).unwrap_or(0);
        let gops = list.split_off(start);
        let times = &times[start..];

        let mut segments: Vec<(f64, f64)> = Vec::new();
        let mut i = 0;
        while i < gops.len() {
            let epoch = gops[i].epoch;
            let mut j = i;
            while j + 1 < gops.len() && gops[j + 1].epoch == epoch {
                j += 1;
            }
            let last_arr = self.epochs.iter().find(|e| e.id == epoch).map(|e| e.last_arr).unwrap_or(gops[j].arr);
            // While the encoder is alive the clip runs up to now. A still screen
            // produces no new frames; the last picture stays up and audio continues.
            let is_current = epoch == self.epoch && self.live;
            let end = if is_current { now } else { last_arr - LATENCY };
            // A first GOP that began long before the requested start means the
            // screen has been still since then: begin the audio at the requested
            // point instead of replaying the whole still stretch.
            let begin = if i == 0 && target - times[0] > 1.5 { target } else { times[i] };
            if end > begin {
                segments.push((begin, end));
            }
            i = j + 1;
        }
        let duration = segments.iter().map(|(a, b)| b - a).sum();
        let mut header = self.pat.clone();
        header.extend_from_slice(&self.pmt);
        Ok(Snapshot { header, gops, segments, duration })
    }
}

#[derive(Clone, Default, Debug)]
pub struct RecStatus {
    pub encoder: String,
    pub method: String,
    pub error: String,
    pub pid: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub maxrate: u64,
}

pub struct RecStats {
    pub status: RecStatus,
    pub running: bool,
    pub frames: u64,
    /// Buffer held in RAM.
    pub video_bytes: u64,
    pub audio_bytes: u64,
    /// Buffer held in files.
    pub video_disk_bytes: u64,
    pub audio_disk_bytes: u64,
    pub buffer_secs: f64,
    pub max_secs: f64,
}

struct Inner {
    ring: Mutex<VideoRing>,
    desk: Arc<Mutex<AudioRing>>,
    mic: Arc<Mutex<AudioRing>>,
    gen: AtomicU64,
    running: AtomicBool,
    child: Mutex<Option<Child>>,
    status: Mutex<RecStatus>,
    audio_stop: Mutex<Option<Arc<AtomicBool>>>,
    cfg: Mutex<Option<CaptureCfg>>,
}

#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn ffmpeg_ok(ffmpeg: &Path, args: &[&str]) -> bool {
    util::hidden_cmd(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pick a working H.264 encoder by actually encoding two frames with each.
fn resolve_encoder(ffmpeg: &Path, wanted: &str) -> String {
    static CACHE: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);
    if wanted != "auto" {
        return wanted.to_string();
    }
    if let Some((p, e)) = CACHE.lock().unwrap().as_ref() {
        if p == ffmpeg {
            return e.clone();
        }
    }
    let mut found = "x264".to_string();
    for (name, codec) in [("nvenc", "h264_nvenc"), ("amf", "h264_amf"), ("qsv", "h264_qsv")] {
        let args = ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x240:r=30", "-frames:v", "2", "-c:v", codec, "-f", "null", "-"];
        if ffmpeg_ok(ffmpeg, &args) {
            found = name.to_string();
            break;
        }
    }
    *CACHE.lock().unwrap() = Some((ffmpeg.to_path_buf(), found.clone()));
    found
}

/// Windows Graphics Capture (ffmpeg 8+) when available, else Desktop Duplication.
fn resolve_method(ffmpeg: &Path, wanted: &str) -> String {
    static CACHE: Mutex<Option<(PathBuf, bool)>> = Mutex::new(None);
    if wanted == "wgc" || wanted == "dda" {
        return wanted.to_string();
    }
    let cached = CACHE.lock().unwrap().as_ref().filter(|(p, _)| p == ffmpeg).map(|(_, ok)| *ok);
    let has_wgc = cached.unwrap_or_else(|| {
        let ok = util::hidden_cmd(ffmpeg)
            .args(["-hide_banner", "-filters"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(" gfxcapture "))
            .unwrap_or(false);
        *CACHE.lock().unwrap() = Some((ffmpeg.to_path_buf(), ok));
        ok
    });
    if has_wgc { "wgc".into() } else { "dda".into() }
}

pub fn capture_args(cfg: &CaptureCfg, enc: &str, method: &str) -> Vec<String> {
    let (w, h) = (cfg.width, cfg.height);
    // Encoders spend bits per frame at the stream's nominal frame rate. The
    // Graphics Capture chain declares its capture rate (the `max_framerate` it is
    // given) and then thins it to the target, so a rate of R given to the encoder
    // would come out as R * fps / (declared rate). Measured: 50 Mbps asked at 140 -> 60 fps gave 23 Mbps.
    // Scale the rates handed to the encoder by the inverse.
    let mut rate_scale = 1.0f64;
    let mut src = if method == "wgc" {
        // Graphics Capture only hands out frames on display refreshes, so asking it
        // for 60 on a 280 Hz panel yields ~54 uneven frames. Capture at a refresh
        // divisor at least twice the target instead and thin the stream with a
        // drop-only selector that keeps a running target time: the average rate
        // is exact and each kept frame is at most half a target interval late.
        //
        // It must never duplicate. Graphics Capture sends nothing while the
        // picture is still, and a duplicating resampler (`fps=`) would answer a
        // minute of stillness with thousands of frames in one burst.
        let hz = cfg.hz.max(cfg.fps).max(1);
        let k = (1..=hz).take_while(|k| hz / k >= cfg.fps * 2).last().unwrap_or(1);
        // The limit is set 5 % above the rate wanted from the display (hz / k), never
        // exactly on it: frames arrive on refreshes with a little jitter, and a limit of
        // exactly one refresh interval throws away every frame that comes a hair early
        // (a 280 fps capture then lands well under 280). Frames only ever arrive on a
        // refresh, so the headroom cannot add any. Measured on an ~80 fps screen: a
        // limit of 280 delivered 4 % fewer frames than limits of 294 or 1000.
        let (cap_num, cap_den) = (hz as u64 * 21, k as u64 * 20);
        rate_scale = (cap_num as f64 / cap_den as f64 / cfg.fps.max(1) as f64).max(1.0);
        let mut s = format!("gfxcapture=monitor_idx={}:max_framerate={}/{}:capture_cursor={}", cfg.monitor, cap_num, cap_den, cfg.cursor as u8);
        if cfg.scaled {
            s += &format!(":width={w}:height={h}:resize_mode=scale:scale_mode=bicubic");
        }
        if cfg.fps < hz {
            let f = cfg.fps;
            s += &format!(",select='if(gte(t,ld(0)),st(0,max(ld(0),t-1/{f})+1/{f})*0+1,0)'");
        }
        s
    } else {
        format!("ddagrab=output_idx={}:framerate={}:draw_mouse={}", cfg.monitor, cfg.fps, cfg.cursor as u8)
    };
    let mut on_cpu = false;
    if method != "wgc" && cfg.scaled {
        src += &format!(",hwdownload,format=bgra,scale={w}:{h}:flags=bilinear");
        on_cpu = true;
    }
    if enc == "x264" && !on_cpu {
        src += ",hwdownload,format=bgra";
    } else if enc == "qsv" && !on_cpu {
        src += ",hwmap=derive_device=qsv,format=qsv";
    }

    let enc_rate = (cfg.maxrate() as f64 * rate_scale) as u64;
    let mr = enc_rate.to_string();
    let cq = cfg.cq().to_string();
    let fixed = cfg.bitrate > 0;
    // Fixed mode holds the chosen rate with a one-second buffer (constant bitrate,
    // no filler: a picture that cannot use the bits is not padded up to the rate).
    // Automatic mode targets a quality level and only caps the rate.
    let buf = if fixed { enc_rate } else { enc_rate * 2 }.to_string();
    let mut a = sv(&["-hide_banner", "-loglevel", "error", "-nostats", "-nostdin", "-filter_complex", &src]);
    match enc {
        "nvenc" => {
            let rate = w as u64 * h as u64 * cfg.fps as u64;
            let preset = if rate <= 200_000_000 { "p4" } else if rate <= 450_000_000 { "p3" } else if rate <= 700_000_000 { "p2" } else { "p1" };
            a.extend(sv(&["-c:v", "h264_nvenc", "-preset", preset, "-tune", "hq", "-bf", "0"]));
            if fixed {
                a.extend(sv(&["-rc", "cbr", "-b:v", &mr, "-maxrate", &mr, "-bufsize", &buf]));
            } else {
                a.extend(sv(&["-rc", "vbr", "-cq", &cq, "-b:v", "0", "-maxrate", &mr, "-bufsize", &buf]));
            }
        }
        "amf" => {
            a.extend(sv(&["-c:v", "h264_amf", "-usage", "transcoding", "-quality", "speed", "-bf", "0"]));
            if fixed {
                a.extend(sv(&["-rc", "cbr", "-b:v", &mr, "-maxrate", &mr, "-bufsize", &buf]));
            } else {
                a.extend(sv(&["-rc", "cqp", "-qp_i", &cq, "-qp_p", &cq]));
            }
        }
        "qsv" => {
            a.extend(sv(&["-c:v", "h264_qsv", "-preset", "veryfast", "-bf", "0"]));
            if fixed {
                a.extend(sv(&["-b:v", &mr, "-maxrate", &mr, "-bufsize", &buf]));
            } else {
                a.extend(sv(&["-global_quality", &cq]));
            }
        }
        _ => {
            a.extend(sv(&["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-pix_fmt", "yuv420p"]));
            if fixed {
                a.extend(sv(&["-b:v", &mr, "-maxrate", &mr, "-bufsize", &buf]));
            } else {
                a.extend(sv(&["-crf", &cq, "-maxrate", &mr, "-bufsize", &buf]));
            }
        }
    }
    if enc == "nvenc" {
        a.extend(sv(&["-forced-idr", "1"]));
    }
    // One keyframe per second of screen time (not per N frames: the frame rate
    // drops to almost nothing on a still desktop). Clips then start within a
    // second of the request and trim without re-encoding.
    a.extend(sv(&["-force_key_frames", "expr:gte(t,n_forced*1)", "-g", &(cfg.fps.max(1) * 2).to_string()]));
    a.extend(sv(&["-f", "mpegts", "-flush_packets", "1", "-muxdelay", "0", "-muxpreload", "0", "pipe:1"]));
    a
}

impl Recorder {
    pub fn new() -> Self {
        Recorder {
            inner: Arc::new(Inner {
                ring: Mutex::new(VideoRing::new()),
                desk: Arc::new(Mutex::new(AudioRing::new())),
                mic: Arc::new(Mutex::new(AudioRing::new())),
                gen: AtomicU64::new(0),
                running: AtomicBool::new(false),
                child: Mutex::new(None),
                status: Mutex::new(RecStatus::default()),
                audio_stop: Mutex::new(None),
                cfg: Mutex::new(None),
            }),
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::Relaxed)
    }

    pub fn current_cfg(&self) -> Option<CaptureCfg> {
        self.inner.cfg.lock().unwrap().clone()
    }

    /// Start (or restart) capture with `cfg`. The buffer is cleared.
    pub fn start(&self, cfg: CaptureCfg) {
        self.halt();
        let inner = self.inner.clone();
        let gen = inner.gen.load(Ordering::SeqCst);
        inner.running.store(true, Ordering::SeqCst);
        let window = spill::window(&cfg.buffer, cfg.buffer_ram_secs, cfg.max_secs);
        // Whatever the mode is now: a crashed Glint may have left files from an earlier one.
        spill::clean(&cfg.buffer_dir);
        {
            let mut ring = inner.ring.lock().unwrap();
            ring.clear();
            ring.max_secs = cfg.max_secs;
            ring.set_spill(window, cfg.buffer_dir.clone());
        }
        inner.desk.lock().unwrap().set_spill(window, cfg.buffer_dir.clone(), "d");
        inner.mic.lock().unwrap().set_spill(window, cfg.buffer_dir.clone(), "m");
        {
            let mut st = inner.status.lock().unwrap();
            *st = RecStatus { width: cfg.width, height: cfg.height, fps: cfg.fps, maxrate: cfg.maxrate(), ..Default::default() };
        }
        *inner.cfg.lock().unwrap() = Some(cfg.clone());

        let stop = Arc::new(AtomicBool::new(false));
        *inner.audio_stop.lock().unwrap() = Some(stop.clone());
        audio::spawn(inner.desk.clone(), inner.mic.clone(), cfg.desktop_audio, cfg.mic, cfg.max_secs + 5.0, stop);

        thread::Builder::new().name("capture".into()).spawn(move || supervise(inner, cfg, gen)).ok();
    }

    /// Stop capture and free the buffer.
    pub fn stop(&self) {
        self.halt();
        self.inner.ring.lock().unwrap().clear();
        self.inner.desk.lock().unwrap().clear();
        self.inner.mic.lock().unwrap().clear();
    }

    fn halt(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        self.inner.gen.fetch_add(1, Ordering::SeqCst);
        if let Some(stop) = self.inner.audio_stop.lock().unwrap().take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(mut c) = self.inner.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.inner.status.lock().unwrap().pid = 0;
    }

    pub fn snapshot(&self, secs: f64) -> Result<(Snapshot, Option<Pcm>, Option<Pcm>), String> {
        let now = now_secs();
        let snap = self.inner.ring.lock().unwrap().snapshot(secs, now)?;
        if snap.segments.is_empty() || snap.duration < 0.2 {
            return Err("The replay buffer is still filling".into());
        }
        // Only the plan is made under the lock; audio that is on disk is read
        // back afterwards, so the capture callback is never kept waiting.
        let desk = self.inner.desk.lock().unwrap().plan(&snap.segments);
        let mic = self.inner.mic.lock().unwrap().plan(&snap.segments);
        Ok((snap, desk.map(Plan::read), mic.map(Plan::read)))
    }

    pub fn stats(&self) -> RecStats {
        let now = now_secs();
        let ring = self.inner.ring.lock().unwrap();
        let (desk_ram, desk_disk) = self.inner.desk.lock().unwrap().bytes();
        let (mic_ram, mic_disk) = self.inner.mic.lock().unwrap().bytes();
        let mut status = self.inner.status.lock().unwrap().clone();
        if status.error.is_empty() {
            status.error = ring.spill_error.clone();
        }
        RecStats {
            status,
            running: self.is_running(),
            frames: ring.frames,
            video_bytes: ring.bytes as u64,
            audio_bytes: (desk_ram + mic_ram) as u64,
            video_disk_bytes: ring.disk_bytes as u64,
            audio_disk_bytes: (desk_disk + mic_disk) as u64,
            buffer_secs: ring.buffered_secs(now),
            max_secs: ring.max_secs,
        }
    }
}

fn alive(inner: &Inner, gen: u64) -> bool {
    inner.gen.load(Ordering::SeqCst) == gen
}

fn set_error(inner: &Inner, msg: &str) {
    inner.status.lock().unwrap().error = msg.to_string();
}

/// Keep ffmpeg running. It exits on resolution changes, secure-desktop prompts
/// and sleep; each restart begins a new epoch in the same buffer.
fn supervise(inner: Arc<Inner>, cfg: CaptureCfg, gen: u64) {
    let Some(ffmpeg) = cfg.ffmpeg.clone() else {
        set_error(&inner, "ffmpeg.exe was not found. Install ffmpeg or set its path in Settings.");
        return;
    };
    let enc = resolve_encoder(&ffmpeg, &cfg.encoder);
    let mut method = resolve_method(&ffmpeg, &cfg.method);
    let mut backoff = 1u64;
    let mut quick_fails = 0u32;

    while alive(&inner, gen) {
        {
            let mut st = inner.status.lock().unwrap();
            st.encoder = enc.clone();
            st.method = method.clone();
        }
        let args = capture_args(&cfg, &enc, &method);
        let spawned = util::hidden_cmd_prio(&ffmpeg).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let started = Instant::now();
        let mut got_data = false;
        match spawned {
            Err(e) => set_error(&inner, &format!("Could not start ffmpeg: {e}")),
            Ok(mut child) => {
                let mut out = child.stdout.take().unwrap();
                if let Some(err) = child.stderr.take() {
                    let inner2 = inner.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(err).lines().map_while(Result::ok) {
                            if !line.trim().is_empty() && alive(&inner2, gen) {
                                set_error(&inner2, line.trim());
                            }
                        }
                    });
                }
                inner.status.lock().unwrap().pid = child.id();
                {
                    // Checked under the lock: halt() bumps `gen` before taking the child.
                    let mut slot = inner.child.lock().unwrap();
                    if alive(&inner, gen) {
                        *slot = Some(child);
                    } else {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
                inner.ring.lock().unwrap().begin_epoch(now_secs());
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    match out.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if !got_data {
                                got_data = true;
                                set_error(&inner, "");
                            }
                            inner.ring.lock().unwrap().feed(&buf[..n], now_secs());
                        }
                    }
                }
                if !alive(&inner, gen) {
                    return; // start()/stop() owns the buffer and the child now
                }
                inner.ring.lock().unwrap().end_epoch();
                if let Some(mut c) = inner.child.lock().unwrap().take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                inner.status.lock().unwrap().pid = 0;
            }
        }
        if started.elapsed() > Duration::from_secs(15) {
            backoff = 1;
            quick_fails = 0;
        } else if !got_data {
            quick_fails += 1;
            // Graphics Capture refused twice in a row: fall back to Desktop Duplication.
            if quick_fails >= 2 && method == "wgc" && cfg.method == "auto" {
                method = "dda".into();
                quick_fails = 0;
            }
        }
        let wait_until = Instant::now() + Duration::from_secs(backoff);
        while Instant::now() < wait_until {
            if !alive(&inner, gen) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(8);
    }
}

// ---------------------------------------------------------------------------
// Self-test: `glint.exe --buffer-test <report.txt>`. No capture, no window:
// the rings are fed made-up data on a made-up clock.

/// One MPEG-TS packet of the video stream. `start` begins a frame (with a PTS);
/// `key` marks it as a keyframe. The rest of the packet is `fill`.
fn test_packet(start: bool, key: bool, pts: f64, fill: u8) -> [u8; TS] {
    let mut p = [fill; TS];
    p[0] = 0x47;
    p[1] = ((start as u8) << 6) | ((VIDEO_PID >> 8) as u8 & 0x1f);
    p[2] = VIDEO_PID as u8;
    if start {
        p[3] = 0x30; // adaptation field + payload
        p[4] = 1;
        p[5] = if key { 0x40 } else { 0 };
        p[6..10].copy_from_slice(&[0, 0, 1, 0xE0]);
        p[10] = 0;
        p[11] = 0;
        p[12] = 0x80;
        p[13] = 0x80; // PTS present
        p[14] = 5;
        let v = (pts * 90_000.0).round() as u64;
        p[15] = 0x21 | ((((v >> 30) & 7) as u8) << 1);
        p[16] = (v >> 22) as u8;
        p[17] = ((((v >> 15) & 0x7f) as u8) << 1) | 1;
        p[18] = (v >> 7) as u8;
        p[19] = (((v & 0x7f) as u8) << 1) | 1;
    } else {
        p[3] = 0x10;
    }
    p
}

/// GOP number `k` as bytes: a keyframe packet and `more` packets after it.
fn test_gop(k: usize, more: usize) -> Vec<u8> {
    let mut v = test_packet(true, true, k as f64, k as u8).to_vec();
    for j in 0..more {
        v.extend_from_slice(&test_packet(false, false, 0.0, (k * 31 + j) as u8));
    }
    v
}

fn test_files(dir: &Path, tag: &str) -> usize {
    let prefix = format!("glint-{}-{}-", std::process::id(), tag);
    std::fs::read_dir(dir).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).count()).unwrap_or(0)
}

pub fn self_test(report_path: &str) {
    let mut lines: Vec<String> = Vec::new();
    let mut failed = 0usize;
    let mut check = |name: &str, ok: bool, detail: String| {
        if !ok {
            failed += 1;
        }
        lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  [{detail}]") }));
    };
    let root = util::data_dir().join("buffer-test");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    const MORE: usize = 20;
    let glen = (MORE + 1) * TS;
    let bytes_of = |snap: &Snapshot| -> Vec<u8> {
        let mut out = Vec::new();
        for g in &snap.gops {
            let _ = g.write_to(&mut out);
        }
        out
    };
    let expect = |from: usize, to: usize| -> Vec<u8> { (from..=to).flat_map(|k| test_gop(k, MORE)).collect() };
    // Feed GOPs `from..=to`, one per second on the fake clock.
    let feed = |ring: &mut VideoRing, from: usize, to: usize| {
        for k in from..=to {
            ring.feed(&test_gop(k, MORE), k as f64);
        }
    };
    let ring_with = |window: Option<f64>, dir: &Path, max_secs: f64| {
        let mut r = VideoRing::new();
        r.max_secs = max_secs;
        r.set_spill(window, dir.to_path_buf());
        r.seg_limit = (glen * 2 + 10) as u64; // two GOPs per file
        r.begin_epoch(0.0);
        r
    };

    // --- spill::window
    check("mode ram keeps everything in RAM", spill::window("ram", 30.0, 605.0).is_none(), String::new());
    check("mode disk keeps nothing in RAM", spill::window("disk", 30.0, 605.0) == Some(0.0), String::new());
    check("mode mixed keeps the window in RAM", spill::window("mixed", 30.0, 605.0) == Some(30.0), String::new());
    check("mode mixed on a short buffer is RAM only", spill::window("mixed", 30.0, 35.0).is_none(), String::new());
    check("an unknown mode behaves as mixed", spill::window("", 30.0, 605.0) == Some(30.0), String::new());

    // --- mixed: newest 5 s in RAM, older on disk
    {
        let dir = root.join("mixed");
        let mut r = ring_with(Some(5.0), &dir, 60.0);
        feed(&mut r, 0, 19);
        // Closed GOPs 0..=18, GOP 19 open. GOP i is wholly older than the window when i + 1 <= 19 - 5.
        check("mixed: GOPs older than the window are on disk", r.ram_start == 14 && r.gops.iter().take(14).all(|g| g.on_disk()), format!("ram_start {}", r.ram_start));
        check("mixed: GOPs inside the window are in RAM", r.gops.iter().skip(14).all(|g| !g.on_disk()) && r.gops.len() == 19, format!("{} closed", r.gops.len()));
        check("mixed: RAM and disk byte counts", r.bytes == 6 * glen && r.disk_bytes == 14 * glen, format!("ram {} disk {} gop {glen}", r.bytes, r.disk_bytes));
        check("mixed: files roll over (two GOPs each)", test_files(&dir, "v") == 7, format!("{} files", test_files(&dir, "v")));
        let snap = r.snapshot(17.0, 19.0).unwrap();
        check("mixed: a clip across disk and RAM is byte-exact", bytes_of(&snap) == expect(2, 19), format!("{} GOPs, {} bytes", snap.gops.len(), bytes_of(&snap).len()));
        check("mixed: the clip's length", (snap.duration - 17.0).abs() < 0.1, format!("{:.3}", snap.duration));
        let short = r.snapshot(3.0, 19.0).unwrap();
        check("mixed: a short clip never touches the disk", short.gops.iter().all(|g| !g.on_disk()) && bytes_of(&short) == expect(16, 19), format!("{} GOPs", short.gops.len()));

        // The buffer moves on past the held snapshot: its data must stay readable.
        r.max_secs = 5.0;
        feed(&mut r, 20, 60);
        let live: usize = r.gops.iter().filter(|g| g.on_disk()).map(|g| g.len()).sum();
        let live_ram: usize = r.gops.iter().filter(|g| !g.on_disk()).map(|g| g.len()).sum::<usize>() + r.cur.as_ref().map_or(0, |c| c.data.len());
        check("eviction: byte counts follow the GOPs", r.disk_bytes == live && r.bytes == live_ram, format!("disk {} / {live}, ram {} / {live_ram}", r.disk_bytes, r.bytes));
        check("eviction: old GOPs are gone", r.gops.front().map_or(false, |g| g.arr >= 50.0), format!("oldest {:?}", r.gops.front().map(|g| g.arr)));
        check("eviction: a snapshot taken earlier still reads its data", bytes_of(&snap) == expect(2, 19), String::new());
        let held = test_files(&dir, "v");
        drop(snap);
        drop(short);
        let after = test_files(&dir, "v");
        check("eviction: files are deleted once nothing points into them", after < held && after <= r.gops.iter().filter(|g| g.on_disk()).count() / 2 + 2, format!("{held} -> {after} files"));
        r.clear();
        check("clear: no buffer file is left", test_files(&dir, "v") == 0 && r.bytes == 0 && r.disk_bytes == 0, format!("{} files", test_files(&dir, "v")));
    }

    // --- disk only
    {
        let dir = root.join("disk");
        let mut r = ring_with(Some(0.0), &dir, 60.0);
        feed(&mut r, 0, 19);
        check("disk: every closed GOP is on disk", r.ram_start == r.gops.len() && r.gops.iter().all(|g| g.on_disk()), format!("{} of {}", r.ram_start, r.gops.len()));
        check("disk: only the open GOP is in RAM", r.bytes == glen && r.disk_bytes == 19 * glen, format!("ram {} disk {}", r.bytes, r.disk_bytes));
        let snap = r.snapshot(10.0, 19.0).unwrap();
        check("disk: a clip is byte-exact", bytes_of(&snap) == expect(9, 19), String::new());
    }

    // --- RAM only
    {
        let dir = root.join("ram");
        let mut r = ring_with(None, &dir, 60.0);
        feed(&mut r, 0, 19);
        check("ram: nothing is written", test_files(&dir, "v") == 0 && r.disk_bytes == 0 && r.bytes == 20 * glen, format!("ram {}", r.bytes));
        let snap = r.snapshot(10.0, 19.0).unwrap();
        check("ram: a clip is byte-exact", bytes_of(&snap) == expect(9, 19), String::new());
    }

    // --- a folder that cannot be written: stay in RAM, lose nothing
    {
        let blocker = root.join("blocker");
        let _ = std::fs::write(&blocker, b"x");
        let mut r = ring_with(Some(0.0), &blocker.join("sub"), 60.0);
        feed(&mut r, 0, 9);
        check("unwritable folder: the buffer stays in RAM", r.ram_start == 0 && r.disk_bytes == 0 && r.bytes == 10 * glen, format!("ram {}", r.bytes));
        check("unwritable folder: the reason is reported", !r.spill_error.is_empty(), r.spill_error.clone());
        let snap = r.snapshot(5.0, 9.0).unwrap();
        check("unwritable folder: a clip is still byte-exact", bytes_of(&snap) == expect(4, 9), String::new());
    }

    // --- leftovers of a dead Glint are cleaned, a live one's files are not
    {
        let dir = root.join("clean");
        let _ = std::fs::create_dir_all(&dir);
        let dead = dir.join("glint-3-v-0.buf"); // no process has id 3
        let mine = dir.join(format!("glint-{}-v-999.buf", std::process::id()));
        let other = dir.join("notes.txt");
        for f in [&dead, &mine, &other] {
            let _ = std::fs::write(f, b"x");
        }
        spill::clean(&dir);
        check("clean: a dead Glint's file is removed", !dead.exists(), String::new());
        check("clean: a running Glint's file is kept", mine.exists(), String::new());
        check("clean: other files are not touched", other.exists(), String::new());
        check("free space can be read", spill::free_space(&dir.join("not").join("there")).is_some_and(|f| f > 0), String::new());
    }

    // --- audio. 1000 frames per second keeps the numbers readable.
    {
        const RATE: u32 = 1000;
        let sample = |i: u64| -> i16 { (i % 30_000) as i16 + 1 };
        // Push frames `from..to` in 10 ms packets; frame i is at time i / RATE.
        let push = |ring: &mut AudioRing, from: u64, to: u64| {
            let mut i = from;
            while i < to {
                let pkt: Vec<i16> = (i..i + 10).flat_map(|f| [sample(f), -sample(f)]).collect();
                ring.push(&pkt, (i + 10) as f64 / RATE as f64);
                i += 10;
            }
        };
        let want = |from: u64, to: u64| -> Vec<i16> { (from..to).flat_map(|f| [sample(f), -sample(f)]).collect() };
        let ring = |window: Option<f64>, dir: &Path, cap: f64| {
            let mut r = AudioRing::new();
            r.set_spill(window, dir.to_path_buf(), "d");
            r.configure(RATE, cap);
            r
        };
        let dir = root.join("audio");

        let mut a = ring(Some(5.0), &dir, 100.0);
        push(&mut a, 0, 60_000);
        let (ram, disk) = a.bytes();
        // A 10 s piece leaves RAM each time RAM reaches window + piece (15 s): five times in 60 s.
        check("audio mixed: old audio is on disk", disk == 50_000 * 4 && test_files(&dir, "d") == 5, format!("disk {disk}, {} files", test_files(&dir, "d")));
        check("audio mixed: RAM holds the window plus at most one piece", ram >= 5_000 * 4 && ram <= 20_000 * 4, format!("ram {ram}"));
        let got = a.slices(&[(3.0, 58.0)]).unwrap();
        check("audio mixed: a range across disk and RAM is exact", got.data == want(3_000, 58_000), format!("{} samples", got.data.len()));
        let got = a.slices(&[(12.5, 13.5), (39.0, 41.0), (57.0, 59.0)]).unwrap();
        let mut w = want(12_500, 13_500);
        w.extend(want(39_000, 41_000));
        w.extend(want(57_000, 59_000));
        check("audio mixed: several ranges are joined in order", got.data == w, String::new());
        let got = a.slices(&[(-2.0, 1.0), (59.0, 62.0)]).unwrap();
        let mut w = vec![0i16; 4_000];
        w.extend(want(0, 1_000));
        w.extend(want(59_000, 60_000));
        w.extend(vec![0i16; 4_000]);
        check("audio mixed: time outside the ring is silence", got.data == w, format!("{} samples", got.data.len()));

        // Same input through a RAM-only ring must read the same.
        let mut b = ring(None, &dir, 100.0);
        push(&mut b, 0, 60_000);
        check("audio: RAM-only and mixed rings read the same", b.slices(&[(0.5, 59.5)]).unwrap().data == a.slices(&[(0.5, 59.5)]).unwrap().data && b.bytes() == (60_000 * 4, 0), String::new());

        // Capacity: 30 s. Pieces wholly outside it are dropped, their files deleted.
        let mut c = ring(Some(0.0), &root.join("audio-cap"), 30.0);
        push(&mut c, 0, 100_000);
        let (ram, disk) = c.bytes();
        let held = (ram + disk) / 4;
        check("audio disk: the ring holds its capacity, at most one piece more", (30_000..=40_000).contains(&held), format!("{held} frames"));
        check("audio disk: RAM holds less than one piece", ram < 10_000 * 4, format!("ram {ram}"));
        check("audio disk: old files are deleted", test_files(&root.join("audio-cap"), "d") <= 4, format!("{} files", test_files(&root.join("audio-cap"), "d")));
        check("audio disk: the last 30 s are exact", c.slices(&[(70.0, 100.0)]).unwrap().data == want(70_000, 100_000), String::new());
        check("audio disk: audio that has left the ring is silence", c.slices(&[(10.0, 11.0)]).unwrap().data == vec![0i16; 2_000], String::new());

        // Silence needs no file.
        let sdir = root.join("audio-silence");
        let mut s = ring(Some(0.0), &sdir, 100.0);
        for i in (0..60_000u64).step_by(10) {
            s.push(&[0i16; 20], (i + 10) as f64 / RATE as f64);
        }
        check("audio: silence is not written to disk", s.bytes().1 == 0 && test_files(&sdir, "d") == 0, format!("{:?}", s.bytes()));
        check("audio: silence reads back as silence", s.slices(&[(5.0, 55.0)]).unwrap().data == vec![0i16; 100_000], String::new());

        // A gap (loopback sends nothing while the PC is silent) is padded, in pieces.
        let gdir = root.join("audio-gap");
        let mut g = ring(Some(5.0), &gdir, 100.0);
        push(&mut g, 0, 2_000);
        push(&mut g, 52_000, 54_000);
        let got = g.slices(&[(1.0, 53.0)]).unwrap();
        let mut w = want(1_000, 2_000);
        w.extend(vec![0i16; 100_000]);
        w.extend(want(52_000, 53_000));
        check("audio gap: silence fills the gap, sound stays in place", got.data == w, format!("{} samples", got.data.len()));
        check("audio gap: the gap did not pile up in RAM", g.bytes().0 <= 20_000 * 4, format!("ram {}", g.bytes().0));
        // A gap longer than the whole ring.
        push(&mut g, 400_000, 401_000);
        check("audio gap: longer than the ring", g.slices(&[(399.0, 401.0)]).unwrap().data == { let mut w = vec![0i16; 2_000]; w.extend(want(400_000, 401_000)); w }, String::new());

        // Unwritable folder.
        let mut u = ring(Some(0.0), &root.join("blocker").join("sub"), 100.0);
        push(&mut u, 0, 30_000);
        check("audio unwritable folder: stays in RAM and reads exact", u.bytes() == (30_000 * 4, 0) && u.slices(&[(0.0, 30.0)]).unwrap().data == want(0, 30_000), format!("{:?}", u.bytes()));

        // Switching a filled RAM ring to disk moves what it holds.
        let mut m = ring(None, &root.join("audio-switch"), 100.0);
        push(&mut m, 0, 50_000);
        m.set_spill(Some(5.0), root.join("audio-switch"), "d");
        check("audio: switching to disk moves the old audio out of RAM", m.bytes().0 <= 15_000 * 4 && m.bytes().1 > 0 && m.slices(&[(0.0, 50.0)]).unwrap().data == want(0, 50_000), format!("{:?}", m.bytes()));
        m.clear();
        check("audio: clear deletes the files", test_files(&root.join("audio-switch"), "d") == 0, String::new());
    }

    // --- Explorer's "select this file" argument
    {
        let arg = util::select_arg(Path::new(r"C:\Users\me\Videos\Glint\osu!\osu! 2026-11-13 07-09-18.mp4"));
        check("reveal: only the path is quoted", arg == r#"/select,"C:\Users\me\Videos\Glint\osu!\osu! 2026-11-13 07-09-18.mp4""#, arg.clone());
        let arg = util::select_arg(Path::new("C:/Clips/Risk of Rain 2/a, b.mp4"));
        check("reveal: forward slashes become backslashes", arg == r#"/select,"C:\Clips\Risk of Rain 2\a, b.mp4""#, arg.clone());
    }

    let _ = std::fs::remove_dir_all(&root);
    lines.push(format!("{} checks, {failed} failed", lines.len()));
    let _ = std::fs::write(report_path, lines.join("\n") + "\n");
}
