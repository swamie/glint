//! Always-on replay buffer.
//!
//! One ffmpeg process captures the monitor on the GPU, encodes with the
//! hardware encoder and writes MPEG-TS to a pipe. This module keeps the last
//! N seconds of that stream in RAM, split at keyframes, so a clip is a cheap
//! copy of already-encoded data. Nothing touches the disk until a clip is saved.
use crate::audio::{self, AudioRing, Pcm};
use crate::util::{self, now_secs};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
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

/// One closed group of pictures: a keyframe and everything up to the next one.
pub struct Gop {
    pub pts: f64,
    pub arr: f64,
    pub epoch: u32,
    pub data: Vec<u8>,
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
    gops: VecDeque<Arc<Gop>>,
    cur: Option<Gop>,
    bytes: usize,
    pat: Vec<u8>,
    pmt: Vec<u8>,
    epochs: VecDeque<Epoch>,
    epoch: u32,
    live: bool,
    carry: Vec<u8>,
    pub max_secs: f64,
    pub frames: u64,
}

impl VideoRing {
    fn new() -> Self {
        VideoRing {
            gops: VecDeque::new(),
            cur: None,
            bytes: 0,
            pat: Vec::new(),
            pmt: Vec::new(),
            epochs: VecDeque::new(),
            epoch: 0,
            live: false,
            carry: Vec::new(),
            max_secs: 35.0,
            frames: 0,
        }
    }

    fn clear(&mut self) {
        self.gops.clear();
        self.cur = None;
        self.bytes = 0;
        self.carry.clear();
        self.epochs.clear();
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
        if let Some(g) = self.cur.take() {
            self.gops.push_back(Arc::new(g));
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
            self.bytes -= front.data.len();
            self.gops.pop_front();
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
                self.cur = Some(Gop { pts: pts.unwrap_or(0.0), arr: now, epoch: self.epoch, data: Vec::with_capacity(256 * 1024) });
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
            list.push(Arc::new(Gop { pts: c.pts, arr: c.arr, epoch: c.epoch, data: c.data.clone() }));
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
    pub video_bytes: u64,
    pub audio_bytes: u64,
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
    // Graphics Capture chain declares its capture rate (hz/k) and then thins it
    // to the target, so a rate of R given to the encoder would come out as
    // R * fps / (hz/k). Measured: 50 Mbps asked at 140 -> 60 fps gave 23 Mbps.
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
        rate_scale = (hz as f64 / k as f64 / cfg.fps.max(1) as f64).max(1.0);
        let mut s = format!("gfxcapture=monitor_idx={}:max_framerate={}/{}:capture_cursor={}", cfg.monitor, hz, k, cfg.cursor as u8);
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
        {
            let mut ring = inner.ring.lock().unwrap();
            ring.clear();
            ring.max_secs = cfg.max_secs;
        }
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
        let desk = self.inner.desk.lock().unwrap().slices(&snap.segments);
        let mic = self.inner.mic.lock().unwrap().slices(&snap.segments);
        Ok((snap, desk, mic))
    }

    pub fn stats(&self) -> RecStats {
        let now = now_secs();
        let ring = self.inner.ring.lock().unwrap();
        let audio_bytes = (self.inner.desk.lock().unwrap().bytes() + self.inner.mic.lock().unwrap().bytes()) as u64;
        RecStats {
            status: self.inner.status.lock().unwrap().clone(),
            running: self.is_running(),
            frames: ring.frames,
            video_bytes: ring.bytes as u64,
            audio_bytes,
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
