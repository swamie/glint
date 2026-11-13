//! Compress for Discord: shrink a clip to fit an upload limit the user picks
//! (10, 20, 50 or 500 MB in the menu; 5..=1000 accepted).
//!
//! It is a background job of kind "compress" (see jobs.rs). The job:
//! 1. probes the clip's real length and works out a bit budget: the file must
//!    be at or under `mb` x 1,000,000 bytes (which is also under `mb` MiB); it
//!    aims for 96 % of that, less an allowance for the MP4 container. Audio is
//!    one AAC stereo track from the clip's first track (the mix) at 96 kb/s
//!    (64 kb/s when the whole budget is under 600 kb/s); video gets the rest.
//! 2. picks a picture size and frame rate that bitrate can carry (`pick_format`);
//! 3. encodes with libx264 in two passes, at below-normal priority so a game
//!    being played keeps the CPU first;
//! 4. checks the size, and if it is over, encodes pass 2 again at a
//!    proportionally lower bitrate (at most `RETRIES` more times);
//! 5. saves `<original name> (Discord 10MB).mp4` beside the original, registers
//!    it (kind "discord", same game and folder) and copies it to the clipboard;
//! 6. emits "clip-compressed" { clip, mb, size, copied, secs }.
//!
//! Work files (the encode and x264's pass logs) live in `util::tmp_dir()` under
//! one `compress-<id>` prefix and are deleted when the job ends in any way.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::library::{self, ClipMeta, Probe};
use crate::util;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

/// Smallest and largest size accepted, in MB.
pub const MIN_MB: u32 = 5;
pub const MAX_MB: u32 = 1000;
/// x264 preset of both passes. x264 already runs pass 1 with fast settings
/// (its "fast first pass": no trellis, one reference, quick motion search), and
/// the passes must agree on frame-type settings, so one preset serves both.
pub const PRESET: &str = "veryfast";
/// Share of the limit aimed for, container included.
const FILL: f64 = 0.96;
/// Allowance for the MP4 container: a fixed part plus a share of the rest.
const MUX_FIXED: f64 = 30_000.0;
const MUX_SHARE: f64 = 0.01;
/// Audio bitrate, and the lower one used when the whole budget is under `AUDIO_LOW_BELOW` kb/s.
const AUDIO_KBPS: u32 = 96;
const AUDIO_LOW_KBPS: u32 = 64;
const AUDIO_LOW_BELOW: f64 = 600.0;
/// Below this the picture is not worth watching: refuse and say what length would fit.
const MIN_VIDEO_KBPS: f64 = 150.0;
/// More than this buys nothing visible for a game clip, even at 1440p60.
const MAX_VIDEO_KBPS: f64 = 40_000.0;
/// Extra pass-2 encodes when the result is still over the limit.
const RETRIES: usize = 2;
/// Returned when the job was cancelled.
pub const CANCELLED: &str = "Cancelled";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// The limit in bytes: 1 MB = 1,000,000 bytes, so the file is also under `mb` MiB.
pub fn limit_bytes(mb: u32) -> u64 {
    mb as u64 * 1_000_000
}

/// "7.4 MB" (decimal, as Discord counts).
pub fn fmt_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

/// "45 s", "3 min 05 s".
fn fmt_len(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 60 { format!("{s} s") } else { format!("{} min {:02} s", s / 60, s % 60) }
}

/// Bits the audio and video streams may use together for a limit of `mb`.
fn usable_bits(mb: u32) -> f64 {
    ((limit_bytes(mb) as f64 * FILL - MUX_FIXED) * (1.0 - MUX_SHARE)).max(0.0) * 8.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// Audio + video, kb/s.
    pub total_kbps: f64,
    /// 0 = no audio stream.
    pub audio_kbps: u32,
    pub video_kbps: f64,
}

/// Split the budget of `mb` over `duration` seconds between audio and video.
/// Fails, saying what length would fit, when too little is left for the picture.
pub fn budget(duration: f64, mb: u32, has_audio: bool) -> Result<Budget, String> {
    if !(duration > 0.0) {
        return Err("Could not read the clip's length".into());
    }
    let bits = usable_bits(mb);
    let total = bits / duration / 1000.0;
    let audio = if !has_audio {
        0
    } else if total < AUDIO_LOW_BELOW {
        AUDIO_LOW_KBPS
    } else {
        AUDIO_KBPS
    };
    let video = (total - audio as f64).min(MAX_VIDEO_KBPS);
    if video < MIN_VIDEO_KBPS {
        let low = if has_audio { AUDIO_LOW_KBPS as f64 } else { 0.0 };
        let fits = (bits / ((MIN_VIDEO_KBPS + low) * 1000.0)).floor();
        return Err(format!("This clip is too long for {mb} MB: it is {}, and about {} fits. Trim it or pick a bigger size.", fmt_len(duration), fmt_len(fits)));
    }
    Ok(Budget { total_kbps: total, audio_kbps: audio, video_kbps: video })
}

/// Picture size and frame rate per video bitrate. First row whose bitrate the
/// budget reaches wins. "Height" is the 16:9 picture area it stands for
/// (1080 = 1920 x 1080 pixels), so ultrawide and portrait clips get the same
/// number of pixels. The kb/s are for the row's frame rate; a source with fewer
/// frames needs proportionally less (a 30 fps clip reaches 720p60's row at 1750).
///
/// | video kb/s | picture at most | fps at most | bits per pixel per frame at the threshold |
/// |-----------:|-----------------|------------:|------------------------------------------:|
/// |    12 000  | 1440p           |          60 | 0.054                                     |
/// |     6 000  | 1080p           |          60 | 0.048                                     |
/// |     3 500  |  720p           |          60 | 0.063                                     |
/// |     2 000  |  720p           |          30 | 0.072                                     |
/// |     1 000  |  480p           |          30 | 0.081                                     |
/// |   below    |  360p           |          30 |                                           |
///
/// Never upscales, keeps the aspect ratio, makes both sides even, and never
/// goes above 1440p or 60 fps (a 4K or 144 fps clip comes down to that).
pub const LADDER: &[(f64, u32, u32)] = &[(12_000.0, 1440, 60), (6_000.0, 1080, 60), (3_500.0, 720, 60), (2_000.0, 720, 30), (1_000.0, 480, 30), (0.0, 360, 30)];

/// (width, height, fps) for a `src_w` x `src_h` clip at `src_fps` (0 = unknown,
/// taken as 60) given `video_kbps`. The fps returned is a cap: a source that
/// is already slower keeps its own rate.
pub fn pick_format(src_w: u32, src_h: u32, src_fps: u32, video_kbps: f64) -> (u32, u32, u32) {
    let src_fps = if src_fps == 0 { 60 } else { src_fps };
    for &(need, height, cap) in LADDER {
        let fps = src_fps.min(cap);
        if video_kbps >= need * fps as f64 / cap as f64 {
            let (w, h) = fit(src_w, src_h, height);
            return (w, h, fps);
        }
    }
    let (w, h) = fit(src_w, src_h, 360);
    (w, h, src_fps.min(30))
}

/// Scale `w` x `h` down to at most the area of a 16:9 picture `max_h` high.
fn fit(w: u32, h: u32, max_h: u32) -> (u32, u32) {
    let even_down = |v: u32| (v & !1).max(2);
    let area = (w as f64) * (h as f64);
    let limit = max_h as f64 * 16.0 / 9.0 * max_h as f64;
    if area <= limit + 0.5 {
        return (even_down(w), even_down(h));
    }
    let f = (limit / area).sqrt();
    let even = |v: f64, cap: u32| (((v / 2.0).round() as u32) * 2).clamp(2, even_down(cap));
    (even(w as f64 * f, w), even(h as f64 * f, h))
}

/// Everything the encode needs to know, worked out before it starts.
#[derive(Debug, Clone)]
pub struct Plan {
    pub duration: f64,
    pub budget: Budget,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub src_fps: u32,
}

pub fn plan(p: &Probe, mb: u32) -> Result<Plan, String> {
    if p.video_streams == 0 || p.width == 0 || p.height == 0 {
        return Err("The clip has no picture to compress".into());
    }
    let budget = budget(p.duration, mb, p.audio_streams > 0)?;
    let (width, height, fps) = pick_format(p.width, p.height, p.fps, budget.video_kbps);
    Ok(Plan { duration: p.duration, budget, width, height, fps, src_w: p.width, src_h: p.height, src_fps: p.fps })
}

fn video_filter(plan: &Plan) -> Option<String> {
    let mut f = Vec::new();
    if (plan.width, plan.height) != (plan.src_w, plan.src_h) {
        f.push(format!("scale={}:{}:flags=lanczos,setsar=1", plan.width, plan.height));
    }
    if plan.src_fps > plan.fps {
        f.push(format!("fps={}", plan.fps));
    }
    (!f.is_empty()).then(|| f.join(","))
}

/// ffmpeg arguments for one pass. Pass 1 writes only the stats (no audio, no file).
fn pass_args(src: &Path, plan: &Plan, kbps: f64, preset: &str, log: &Path, pass: u8, out: Option<&Path>) -> Vec<String> {
    let k = kbps.round().max(1.0) as u64;
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(src.to_string_lossy().into_owned());
    a.extend(["-map".into(), "0:v:0".into()]);
    if let Some(vf) = video_filter(plan) {
        a.extend(["-vf".into(), vf]);
    }
    a.extend(
        [
            "-c:v".to_string(), "libx264".into(), "-preset".into(), preset.into(), "-profile:v".into(), "high".into(), "-pix_fmt".into(), "yuv420p".into(),
            "-b:v".into(), format!("{k}k"), "-maxrate".into(), format!("{}k", k * 3 / 2), "-bufsize".into(), format!("{}k", k * 2),
            "-pass".into(), pass.to_string(), "-passlogfile".into(), log.to_string_lossy().into_owned(),
        ],
    );
    match out {
        None => a.extend(["-an", "-sn", "-dn", "-f", "null", "-"].iter().map(|s| s.to_string())),
        Some(o) => {
            if plan.budget.audio_kbps > 0 {
                a.extend(["-map".into(), "0:a:0".into(), "-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", plan.budget.audio_kbps), "-ac".into(), "2".into()]);
            }
            a.extend(["-sn", "-dn", "-map_chapters", "-1", "-movflags", "+faststart"].iter().map(|s| s.to_string()));
            a.push(o.to_string_lossy().into_owned());
        }
    }
    a
}

/// Run ffmpeg (`-progress pipe:1`), reporting 0..1 of `expected` seconds done.
/// Kills it within about 0.1 s once `cancel` is set and returns `CANCELLED`.
fn run_ffmpeg(ffmpeg: &Path, args: &[String], expected: f64, cancel: &AtomicBool, on_frac: &mut dyn FnMut(f64)) -> Result<(), String> {
    let mut child = Command::new(ffmpeg)
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let err_reader = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let (tx, rx) = mpsc::channel::<f64>();
    if let Some(out) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                    if tx.send(us / 1e6).is_err() {
                        return;
                    }
                }
            }
        });
    }
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(t) => {
                if expected > 0.0 {
                    on_frac((t / expected).clamp(0.0, 1.0));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // Output closed: ffmpeg is finishing (the faststart rewrite). Still honour cancel.
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("ffmpeg failed: {e}")),
        }
    };
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let tail = util::last_lines(&stderr, 2);
        Err(if tail.is_empty() { format!("ffmpeg stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail })
    }
}

/// Deletes every work file of one job (`<tmp>\<tag>*`) except `keep`.
struct Cleanup {
    dir: PathBuf,
    tag: String,
    keep: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in work_files(&self.dir, &self.tag) {
            if self.keep.as_ref() != Some(&p) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn work_files(dir: &Path, tag: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(tag)).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// A finished encode, still in the temp folder.
#[derive(Debug)]
pub struct Done {
    pub file: PathBuf,
    pub size: u64,
    pub plan: Plan,
    /// Pass-2 encodes it took (1 = hit the size first time).
    pub tries: usize,
}

/// Encode `src` to fit `mb` into `<tmp>\<tag>.mp4`. `progress` gets 0..100 over
/// both passes (never backwards, 100 once the file is checked). On any error or
/// cancel every `<tag>*` work file is gone when this returns; on success only
/// the result is left.
pub fn compress(ffmpeg: &Path, src: &Path, mb: u32, tag: &str, preset: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<Done, String> {
    let probe = library::probe(ffmpeg, src, false).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
    let plan = plan(&probe, mb)?;
    let tmp = util::tmp_dir();
    let mut guard = Cleanup { dir: tmp.clone(), tag: tag.to_string(), keep: None };
    let log = tmp.join(format!("{tag}-pass"));
    let out = tmp.join(format!("{tag}.mp4"));
    let limit = limit_bytes(mb);
    let mut top = 0f32;
    let mut report = |p: f32| {
        if p > top {
            top = p;
            progress(p);
        }
    };
    // Pass 1 takes about a third of the time (fast first pass, no audio, no file).
    const PASS1: f32 = 30.0;
    let spans: [(f32, f32); RETRIES + 1] = [(PASS1, 97.0), (97.0, 98.5), (98.5, 99.5)];
    util::log(&format!(
        "compress: {} -> {mb} MB: {:.1} s, {}x{}@{} -> {}x{}@{}, video {:.0} kb/s, audio {} kb/s, preset {preset}",
        src.display(), plan.duration, plan.src_w, plan.src_h, plan.src_fps, plan.width, plan.height, plan.fps, plan.budget.video_kbps, plan.budget.audio_kbps
    ));
    let mut video = plan.budget.video_kbps;
    run_ffmpeg(ffmpeg, &pass_args(src, &plan, video, preset, &log, 1, None), plan.duration, cancel, &mut |f| report(PASS1 * f as f32))?;
    let audio_bytes = plan.budget.audio_kbps as f64 * 125.0 * plan.duration;
    let mut smallest = u64::MAX;
    for (i, &(lo, hi)) in spans.iter().enumerate() {
        let _ = fs::remove_file(&out);
        run_ffmpeg(ffmpeg, &pass_args(src, &plan, video, preset, &log, 2, Some(&out)), plan.duration, cancel, &mut |f| report(lo + (hi - lo) * f as f32))?;
        let size = fs::metadata(&out).map(|m| m.len()).map_err(|_| "ffmpeg wrote no file".to_string())?;
        if size <= limit {
            report(100.0);
            guard.keep = Some(out.clone());
            return Ok(Done { file: out, size, plan, tries: i + 1 });
        }
        smallest = smallest.min(size);
        // Over: lower the video bitrate by how much the video was over its share, and a little more.
        let target = limit as f64 * FILL;
        let scale = ((target - audio_bytes).max(1.0) / (size as f64 - audio_bytes).max(1.0)).min(1.0) * 0.97;
        util::log(&format!("compress: try {} was {} (limit {}), video {:.0} -> {:.0} kb/s", i + 1, fmt_size(size), fmt_size(limit), video, video * scale));
        video *= scale;
        if video < MIN_VIDEO_KBPS / 2.0 {
            break;
        }
    }
    Err(format!("Could not get this clip under {mb} MB: the smallest try was {}. Pick a bigger size or trim the clip.", fmt_size(smallest)))
}

/// Move a file, also across drives.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if let Err(e) = fs::copy(from, to) {
        let _ = fs::remove_file(to);
        return Err(format!("Could not save the compressed clip: {e}"));
    }
    let _ = fs::remove_file(from);
    Ok(())
}

/// Whether the result goes on the clipboard: only with a running app, never in a self-test.
fn may_copy(core: &Core) -> bool {
    core.app.get().is_some() && std::env::var_os("GLINT_NO_CLIPBOARD").is_none()
}

/// Queue a compress of clip `clip_id` to fit `mb` MB. Refuses a size outside
/// `MIN_MB..=MAX_MB` and a clip that is already small enough, before queueing.
pub fn enqueue(core: &Arc<Core>, clip_id: String, mb: u32) -> Result<Job, String> {
    if !(MIN_MB..=MAX_MB).contains(&mb) {
        return Err(format!("Pick a size from {MIN_MB} to {MAX_MB} MB"));
    }
    let meta = core.lib.lock().unwrap().get(&clip_id).ok_or("Clip not found")?;
    let src = PathBuf::from(&meta.path);
    let size = fs::metadata(&src).map(|m| m.len()).map_err(|_| "The clip's video file is missing".to_string())?;
    if size <= limit_bytes(mb) {
        return Err(format!("This clip is already under {mb} MB (it is {})", fmt_size(size)));
    }
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let m = meta.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let started = Instant::now();
        let core = weak.upgrade().ok_or("Glint is closing")?;
        if !src.is_file() {
            return Err("The clip's video file is missing".into());
        }
        let tag = format!("compress-{}", util::new_id());
        let done = compress(&ffmpeg, &src, mb, &tag, PRESET, cancel, &mut |p| progress(p))?;
        let dir = src.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&s.clips_dir));
        let stem = src.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| m.title.clone());
        let base = format!("{} (Discord {mb}MB)", util::sanitize_filename(&stem));
        let out = util::unique_path(&dir, &base, "mp4");
        let moved = move_file(&done.file, &out);
        let _ = fs::remove_file(&done.file);
        moved?;
        let title = format!("{} (Discord {mb}MB)", m.title);
        let mut new = library::register(&ffmpeg, &out, &title, &m.game, "discord");
        new.folder = m.folder.clone();
        core.lib.lock().unwrap().add(new.clone());
        core.emit("clips-changed", new.id.clone());
        let copied = may_copy(&core) && {
            let path = out.to_string_lossy().into_owned();
            match crate::app::files_to_clipboard(&[path.as_str()]) {
                Ok(()) => true,
                Err(e) => {
                    util::log(&format!("compress: could not copy to the clipboard: {e}"));
                    false
                }
            }
        };
        let secs = started.elapsed().as_secs_f64();
        util::log(&format!("compress: done in {secs:.1} s, {} in {} tries, copied={copied}", fmt_size(done.size), done.tries));
        core.emit("clip-compressed", event_payload(&new, mb, done.size, copied, secs));
        Ok(new.id)
    });
    core.jobs.enqueue(&clip_id, &meta.title, "compress", run)
}

/// The "clip-compressed" payload.
fn event_payload(clip: &ClipMeta, mb: u32, size: u64, copied: bool, secs: f64) -> serde_json::Value {
    json!({ "clip": clip, "mb": mb, "size": size, "copied": copied, "secs": secs })
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    path: PathBuf,
    failed: u32,
}

impl Report {
    fn line(&mut self, text: &str) {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{text}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if !ok {
            self.failed += 1;
        }
        let tag = if ok { "PASS" } else { "FAIL" };
        if detail.is_empty() {
            self.line(&format!("{tag} {name}"));
        } else {
            self.line(&format!("{tag} {name} - {detail}"));
        }
    }
}

fn build(ffmpeg: &Path, args: &[&str]) -> Result<(), String> {
    let out = util::hidden_cmd(ffmpeg).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(util::last_lines(&out.stderr, 2)) }
}

/// A noisy moving test picture (so it does not compress to nothing), with `tracks` tones.
fn make_clip(ffmpeg: &Path, out: &Path, w: u32, h: u32, fps: u32, secs: u32, tracks: u32) -> Result<(), String> {
    make_clip_with(ffmpeg, out, w, h, fps, secs, tracks, "noise=alls=14:allf=t", "16")
}

/// `make_clip` with its own noise filter and source quality (the bench uses a lighter, smaller source).
#[allow(clippy::too_many_arguments)]
fn make_clip_with(ffmpeg: &Path, out: &Path, w: u32, h: u32, fps: u32, secs: u32, tracks: u32, vf: &str, crf: &str) -> Result<(), String> {
    let video = format!("testsrc2=s={w}x{h}:r={fps}:d={secs}");
    let tones: Vec<String> = (0..tracks).map(|i| format!("sine=frequency={}:sample_rate=48000:duration={secs}", 300 + 400 * i)).collect();
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(video);
    for t in &tones {
        a.extend(["-f".into(), "lavfi".into(), "-i".into(), t.clone()]);
    }
    a.extend(["-map".into(), "0:v".into()]);
    for i in 0..tracks {
        a.extend(["-map".into(), format!("{}:a", i + 1)]);
    }
    a.extend(["-vf", vf, "-c:v", "libx264", "-preset", "ultrafast", "-crf", crf, "-pix_fmt", "yuv420p"].iter().map(|s| s.to_string()));
    if tracks > 0 {
        a.extend(["-c:a", "aac", "-b:a", "160k", "-ac", "2"].iter().map(|s| s.to_string()));
    }
    a.push(out.to_string_lossy().into_owned());
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    build(ffmpeg, &refs)
}

fn format_name(ffmpeg: &Path, file: &Path) -> String {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return String::new() };
    util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-show_entries", "format=format_name", "-of", "default=nw=1:nk=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Decodes from start to end without an error.
fn decodes(ffmpeg: &Path, file: &Path) -> bool {
    util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(file)
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .is_ok_and(|o| o.status.success() && o.stderr.iter().all(u8::is_ascii_whitespace))
}

/// The top-level MP4 boxes in order ("ftyp", "moov", "mdat", ...).
fn top_boxes(file: &Path) -> Vec<String> {
    let Ok(data) = fs::read(file) else { return Vec::new() };
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos + 8 <= data.len() && out.len() < 16 {
        let mut size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as u64;
        let kind = String::from_utf8_lossy(&data[pos + 4..pos + 8]).into_owned();
        if size == 1 && pos + 16 <= data.len() {
            size = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap_or([0; 8]));
        } else if size == 0 {
            size = (data.len() - pos) as u64;
        }
        out.push(kind);
        if size < 8 {
            break;
        }
        pos += size as usize;
    }
    out
}

fn wait_until(secs: f64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// `glint.exe --compress-test <report.txt>`: headless checks of Compress for Discord.
/// Set GLINT_COMPRESS_BENCH=<presets, comma separated> to also time a 30 s 1440p60 clip.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    // Never the clipboard (there is no app handle either).
    std::env::set_var("GLINT_NO_CLIPBOARD", "1");
    run_self_test(&mut r);
    let failed = r.failed;
    r.line(&format!("DONE failed={failed}"));
}

fn run_self_test(r: &mut Report) {
    // 1. The size rule.
    let table: &[((u32, u32, u32, f64), (u32, u32, u32))] = &[
        ((2560, 1440, 60, 15_000.0), (2560, 1440, 60)),
        ((2560, 1440, 144, 15_000.0), (2560, 1440, 60)),
        ((3840, 2160, 60, 30_000.0), (2560, 1440, 60)),
        ((2560, 1440, 60, 8_000.0), (1920, 1080, 60)),
        ((1920, 1080, 60, 7_000.0), (1920, 1080, 60)),
        ((1920, 1080, 60, 5_000.0), (1280, 720, 60)),
        ((1920, 1080, 60, 2_500.0), (1280, 720, 30)),
        ((1920, 1080, 30, 1_800.0), (1280, 720, 30)),
        ((1920, 1080, 60, 1_200.0), (854, 480, 30)),
        ((1920, 1080, 60, 400.0), (640, 360, 30)),
        ((1280, 720, 60, 20_000.0), (1280, 720, 60)),
        ((3440, 1440, 60, 15_000.0), (2968, 1242, 60)),
        ((1080, 1920, 60, 2_500.0), (720, 1280, 30)),
        ((1919, 1079, 60, 7_000.0), (1918, 1078, 60)),
        ((1920, 1080, 0, 2_500.0), (1280, 720, 30)),
    ];
    for &((w, h, fps, kbps), want) in table {
        let got = pick_format(w, h, fps, kbps);
        r.check(&format!("size rule: {w}x{h}@{fps} at {kbps} kb/s -> {}x{}@{}", want.0, want.1, want.2), got == want, &format!("got {got:?}"));
    }
    let b = budget(20.0, 10, true);
    r.check("budget: 20 s at 10 MB has 96 kb/s audio and about 3.7 Mb/s video", b.as_ref().is_ok_and(|b| b.audio_kbps == 96 && (3_600.0..3_800.0).contains(&b.video_kbps)), &format!("{b:?}"));
    let b = budget(150.0, 10, true);
    r.check("budget: 64 kb/s audio when the whole budget is under 600 kb/s", b.as_ref().is_ok_and(|b| b.audio_kbps == 64), &format!("{b:?}"));
    let b = budget(20.0, 10, false);
    r.check("budget: no audio stream for a silent clip", b.as_ref().is_ok_and(|b| b.audio_kbps == 0), &format!("{b:?}"));
    let b = budget(1800.0, 10, true);
    let msg = b.as_ref().err().cloned().unwrap_or_default();
    r.check("budget: a 30 min clip at 10 MB is refused, saying what length fits", msg.contains("30 min 00 s") && msg.contains("about 5 min"), &msg);
    let s = crate::settings::Settings::default();
    r.check("settings defaults: compress_mb 10, ui sounds off, hover off, volume 50", s.compress_mb == 10 && !s.ui_sounds && !s.ui_sounds_hover && s.ui_sounds_volume == 50, "");

    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let dir = util::data_dir().join("clips").join("Test Game");
    let _ = fs::remove_dir_all(util::data_dir().join("clips"));
    let _ = fs::create_dir_all(&dir);
    let tmp = util::tmp_dir();

    // 2. A 20 s 1080p60 clip with two audio tracks, compressed to 8 MB.
    let src = dir.join("source.mp4");
    let t0 = Instant::now();
    let built = make_clip(&ffmpeg, &src, 1920, 1080, 60, 20, 2);
    r.check("build 20 s 1920x1080 60 fps test clip with audio", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    if built.is_err() {
        return;
    }
    let src_size = fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
    let src_p = library::probe(&ffmpeg, &src, false).unwrap_or_default();
    r.line(&format!("INFO source {} {}x{}@{} {:.2} s, {} audio, built in {:.1} s", fmt_size(src_size), src_p.width, src_p.height, src_p.fps, src_p.duration, src_p.audio_streams, t0.elapsed().as_secs_f64()));
    let mb = 8;
    let mut pcts: Vec<f32> = Vec::new();
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let res = compress(&ffmpeg, &src, mb, "compress-test-a", PRESET, &cancel, &mut |p| pcts.push(p));
    let took = t0.elapsed().as_secs_f64();
    r.check("compress to 8 MB succeeds", res.is_ok(), res.as_ref().err().map(String::as_str).unwrap_or(""));
    if let Ok(done) = &res {
        let size = done.size;
        let p = library::probe(&ffmpeg, &done.file, false).unwrap_or_default();
        r.line(&format!(
            "INFO 20 s 1080p60 -> 8 MB: {} in {took:.1} s ({} tries), plan {}x{}@{} video {:.0} kb/s audio {} kb/s, preset {PRESET}",
            fmt_size(size), done.tries, done.plan.width, done.plan.height, done.plan.fps, done.plan.budget.video_kbps, done.plan.budget.audio_kbps
        ));
        r.check("result is at or under 8 MB (8,000,000 bytes)", size <= limit_bytes(mb), &size.to_string());
        r.check("result is above 60 % of the limit (not wastefully small)", size as f64 > 0.6 * limit_bytes(mb) as f64, &fmt_size(size));
        let fmt = format_name(&ffmpeg, &done.file);
        r.check("result is an MP4", fmt.contains("mp4"), &fmt);
        r.check("result has one video and one audio stream", p.video_streams == 1 && p.audio_streams == 1, &format!("{} video, {} audio", p.video_streams, p.audio_streams));
        r.check("result decodes without errors", decodes(&ffmpeg, &done.file), "");
        let boxes = top_boxes(&done.file);
        let moov = boxes.iter().position(|b| b == "moov");
        let mdat = boxes.iter().position(|b| b == "mdat");
        r.check("faststart: moov before mdat", moov.is_some() && mdat.is_some() && moov < mdat, &format!("{boxes:?}"));
        r.check("duration is kept", (p.duration - src_p.duration).abs() <= 0.25, &format!("{:.2} vs {:.2}", p.duration, src_p.duration));
        r.check("picture is 1280x720 at 30 fps (the rule for about 2.9 Mb/s)", (p.width, p.height, p.fps) == (1280, 720, 30) && (done.plan.width, done.plan.height, done.plan.fps) == (1280, 720, 30), &format!("{}x{}@{}", p.width, p.height, p.fps));
        r.check("progress reached 100", pcts.last() == Some(&100.0), &format!("last {:?}", pcts.last()));
        r.check("progress never went backwards", pcts.windows(2).all(|w| w[1] >= w[0]), "");
        r.check("progress reported through both passes", pcts.iter().any(|&p| p > 5.0 && p < 30.0) && pcts.iter().any(|&p| p > 40.0 && p < 97.0), &format!("{} updates", pcts.len()));
        let left: Vec<PathBuf> = work_files(&tmp, "compress-test-a").into_iter().filter(|f| f != &done.file).collect();
        r.check("no pass logs left after success", left.is_empty(), &format!("{left:?}"));
        let _ = fs::remove_file(&done.file);
    }

    // 3. Cancel half-way through pass 2.
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let set_at: Arc<std::sync::Mutex<Option<Instant>>> = Arc::default();
        let (c, sa) = (cancel.clone(), set_at.clone());
        let mut on = move |p: f32| {
            if p > 50.0 && !c.swap(true, Ordering::SeqCst) {
                *sa.lock().unwrap() = Some(Instant::now());
            }
        };
        let res = compress(&ffmpeg, &src, 6, "compress-test-c", PRESET, &cancel, &mut on);
        let stopped = set_at.lock().unwrap().map(|t| t.elapsed());
        r.check("cancel mid-way returns \"Cancelled\"", res.as_ref().err().map(String::as_str) == Some(CANCELLED), &format!("{:?}", res.as_ref().map(|d| d.size)));
        r.check("cancel stops within a second", stopped.is_some_and(|d| d < Duration::from_secs(1)), &format!("{stopped:?}"));
        let left = work_files(&tmp, "compress-test-c");
        r.check("cancel leaves no partial file and no pass logs", left.is_empty(), &format!("{left:?}"));
    }

    // 4. Through the queue and the library: refusals, a silent clip, lanes.
    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = util::data_dir().join("clips").to_string_lossy().into_owned();
        s.blur_parallel = 1;
    }
    core.jobs.connect(&core);
    let small = dir.join("small.mp4");
    let built = make_clip(&ffmpeg, &small, 320, 240, 30, 2, 1);
    r.check("build a small clip", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    let mut small_meta = library::register(&ffmpeg, &small, "small", "Test Game", "clip");
    small_meta.folder = "Test Game".into();
    let small_id = small_meta.id.clone();
    core.lib.lock().unwrap().add(small_meta);
    let res = enqueue(&core, small_id.clone(), 10);
    let msg = res.as_ref().err().cloned().unwrap_or_default();
    r.check("an already-small clip is refused before queueing", msg.starts_with("This clip is already under 10 MB (it is ") && core.jobs.list().is_empty(), &msg);
    r.check("a size under 5 MB is refused", enqueue(&core, small_id.clone(), 4).is_err(), "");
    r.check("a size over 1000 MB is refused", enqueue(&core, small_id.clone(), 1001).is_err(), "");

    let silent = dir.join("silent.mp4");
    let built = make_clip(&ffmpeg, &silent, 1280, 720, 60, 8, 0);
    let silent_size = fs::metadata(&silent).map(|m| m.len()).unwrap_or(0);
    r.check("build an 8 s 720p60 clip with no audio, over 5 MB", built.is_ok() && silent_size > limit_bytes(5), &format!("{} {:?}", fmt_size(silent_size), built.err()));
    let mut meta = library::register(&ffmpeg, &silent, "silent", "Test Game", "clip");
    meta.folder = "Test Game".into();
    let id = meta.id.clone();
    core.lib.lock().unwrap().add(meta);
    // A Blur render of the same clip holds the only Blur slot (blur_parallel = 1).
    let release = Arc::new(AtomicBool::new(false));
    let rel = release.clone();
    let blur_job = core.jobs.enqueue(&id, "silent", "blur", Box::new(move |c: &AtomicBool, _p: &mut dyn FnMut(f32)| -> Result<String, String> {
        while !rel.load(Ordering::SeqCst) && !c.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(20));
        }
        Ok(String::new())
    }));
    let _ = core.jobs.enqueue("other", "other", "blur", Box::new(|_c: &AtomicBool, _p: &mut dyn FnMut(f32)| -> Result<String, String> { Ok(String::new()) }));
    let t0 = Instant::now();
    let job = enqueue(&core, id.clone(), 5);
    r.check("a clip being blurred can still be compressed", blur_job.is_ok() && job.is_ok(), &format!("{:?}", job.as_ref().err()));
    let dup = enqueue(&core, id.clone(), 5);
    r.check("a second compress of the same clip is refused", dup.as_ref().err().map(String::as_str) == Some("That clip is already being compressed"), &format!("{:?}", dup.err()));
    let Ok(job) = job else { return };
    let state = |jid: &str| core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();
    let ok = wait_until(3.0, || state(&job.id).state == "running");
    r.check("the compress runs while the only Blur slot is busy", ok && state(&job.id).kind == "compress", &state(&job.id).state);
    let ended = wait_until(180.0, || !matches!(state(&job.id).state.as_str(), "queued" | "running"));
    release.store(true, Ordering::SeqCst);
    let j = state(&job.id);
    r.check("the queued compress of a silent clip finishes", ended && j.state == "done", &format!("{} {}", j.state, j.message));
    r.line(&format!("INFO 8 s 720p60 silent -> 5 MB through the queue: {:.1} s", t0.elapsed().as_secs_f64()));
    let new = core.lib.lock().unwrap().get(&j.result_id);
    match new {
        Some(n) => {
            let p = Path::new(&n.path);
            r.check("result registered with kind \"discord\"", n.kind == "discord", &n.kind);
            r.check("result saved beside the source as \"silent (Discord 5MB).mp4\"", p.parent() == silent.parent() && p.file_name().is_some_and(|f| f == "silent (Discord 5MB).mp4"), &n.path);
            r.check("result keeps the source's game and folder", n.game == "Test Game" && n.folder == "Test Game", &format!("{} / {}", n.game, n.folder));
            r.check("result title names the size", n.title == "silent (Discord 5MB)", &n.title);
            r.check("result is under 5 MB", n.size > 0 && n.size <= limit_bytes(5), &fmt_size(n.size));
            r.check("result of a silent clip has video and no audio", n.audio_tracks == 0 && library::probe(&ffmpeg, p, false).is_some_and(|q| q.video_streams == 1 && q.audio_streams == 0), "");
            let ev = event_payload(&n, 5, n.size, false, 1.5);
            r.check("clip-compressed payload has clip, mb, size, copied, secs", ev["clip"]["id"] == n.id.as_str() && ev["mb"] == 5 && ev["size"] == n.size && ev["copied"] == false && ev["secs"] == 1.5, &ev.to_string());
        }
        None => r.check("result registered in the library", false, &j.result_id),
    }
    let left = work_files(&tmp, "compress-");
    r.check("no work files left in the temp folder", left.is_empty(), &format!("{left:?}"));
    core.jobs.shutdown();

    // 5. Optional timing of a 30 s 1440p60 clip.
    if let Some(presets) = std::env::var_os("GLINT_COMPRESS_BENCH") {
        let big = dir.join("bench1440.mp4");
        let t0 = Instant::now();
        if make_clip_with(&ffmpeg, &big, 2560, 1440, 60, 30, 1, "noise=alls=4:allf=t", "22").is_ok() {
            r.line(&format!("INFO bench source 30 s 2560x1440@60 {} built in {:.1} s", fmt_size(fs::metadata(&big).map(|m| m.len()).unwrap_or(0)), t0.elapsed().as_secs_f64()));
            for preset in presets.to_string_lossy().split(',').map(str::trim).filter(|p| !p.is_empty()) {
                for mb in [10, 50] {
                    let t0 = Instant::now();
                    let res = compress(&ffmpeg, &big, mb, "compress-bench", preset, &AtomicBool::new(false), &mut |_| {});
                    match res {
                        Ok(d) => {
                            r.line(&format!(
                                "INFO bench 30 s 1440p60 -> {mb} MB preset {preset}: {:.1} s, {} ({} tries), {}x{}@{} video {:.0} kb/s",
                                t0.elapsed().as_secs_f64(), fmt_size(d.size), d.tries, d.plan.width, d.plan.height, d.plan.fps, d.plan.budget.video_kbps
                            ));
                            let _ = fs::remove_file(&d.file);
                        }
                        Err(e) => r.line(&format!("INFO bench {mb} MB preset {preset} failed: {e}")),
                    }
                }
            }
        }
    }
}
