//! Upscale for YouTube (0.1.24). Owner: agent C.
//!
//! YouTube gives a larger upload more bitrate (a 1440p or 4K upload gets a
//! far better encode than a 1080p one), so scaling a clip up right before
//! uploading makes it look better on YouTube. It adds no detail.
//!
//! This does what the "CTT Upscaler" (ctt.cx/video/ffmpeg/upscaling, its
//! `CTT Upscaler.cmd`) does, with Glint's own ffmpeg:
//! - the filter: its default "Point" (`zscale=-2:<height>:f=point`), which is
//!   nearest-neighbour; here `scale=<w>:<h>:flags=neighbor` (same result, and
//!   no zimg needed). "Smooth" is lanczos, one of its other choices.
//! - the encoders and their quality, its "High" here: NVENC `-preset p7 -rc
//!   vbr -b:v 250M -cq 18`, AMF `-quality quality -qp 12`, QuickSync
//!   `-preset veryslow -global_quality 15`, x264 `aq-mode=3 -crf 15`
//!   (preset medium here instead of slower, and 8-bit so Glint's player and
//!   every browser can play the result).
//! - sound copied (`-c:a copy`), MP4 out; a clip already that tall is refused.
//!
//! Glint's additions: frame timing passed through unchanged, track titles
//! kept, hardware H.264 past its 4096-pixel width limit goes to the same
//! card's HEVC encoder, and any hardware failure falls back to x264.
//!
//! Job kind "upscale" (jobs.rs, the compress lane). The result is a new
//! library clip "<title> (4K)" (or "(1440p)", "(1080p)"), kind "upscale",
//! beside the source. Events: "clip-upscaled" { clip, size, encoder, secs,
//! job_id, source }.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::library::{self, ClipMeta};
use crate::util;
use serde::{Deserialize, Serialize};
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

/// The error `run` returns when `cancel` stopped it.
pub const CANCELLED: &str = "Cancelled";

/// The targets the dialog offers (YouTube's 1080p, 1440p and 4K).
pub const HEIGHTS: [u32; 3] = [1080, 1440, 2160];

/// The widest picture the hardware H.264 encoders take; wider goes to HEVC.
const H264_HW_MAX_W: u32 = 4096;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// How a clip is upscaled. Saved in the settings as the dialog's last choices, and inside a render rule's upscale step.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct UpscaleOpts {
    /// The height to scale to, in pixels (2160 = 4K, 1440 = 1440p). The width follows the picture's shape.
    pub height: u32,
    /// "sharp" (nearest-neighbour, the CTT Upscaler's default) or "smooth" (lanczos).
    pub filter: String,
    /// "best", "high" (the CTT Upscaler's own settings) or "smaller".
    pub quality: String,
    /// "auto" (the graphics card's encoder, else x264), or "nvenc", "amf", "qsv", "x264".
    pub encoder: String,
}

impl Default for UpscaleOpts {
    fn default() -> Self {
        UpscaleOpts { height: 2160, filter: "sharp".into(), quality: "high".into(), encoder: "auto".into() }
    }
}

/// The short name of a target: "4K", "1440p", "1080p".
pub fn label(height: u32) -> String {
    match height {
        2160 => "4K".into(),
        4320 => "8K".into(),
        h => format!("{h}p"),
    }
}

/// The title of the result: "<title> (4K)".
pub fn result_title(title: &str, height: u32) -> String {
    format!("{title} ({})", label(height))
}

/// The picture size for `height`: the width follows the shape, both even.
pub fn target_size(src_w: u32, src_h: u32, height: u32) -> (u32, u32) {
    let h = (height / 2 * 2).max(2);
    if src_w == 0 || src_h == 0 {
        return (h * 16 / 9 / 2 * 2, h);
    }
    let w = (src_w as f64 * h as f64 / src_h as f64 / 2.0).round() as u32 * 2;
    (w.max(2), h)
}

/// Why a clip `src_h` pixels tall cannot go to `height` (None when it can).
pub fn refusal(src_h: u32, height: u32) -> Option<String> {
    if !(240..=4320).contains(&height) {
        return Some(format!("Pick a height from 240 to 4320 (not {height})"));
    }
    (src_h >= height).then(|| format!("This clip is already {src_h}p, as tall as {} or taller, so upscaling would not help", label(height)))
}

/// The ffmpeg filter for the picture.
fn video_filter(o: &UpscaleOpts, w: u32, h: u32) -> String {
    let flags = if o.filter == "smooth" { "lanczos" } else { "neighbor" };
    format!("scale={w}:{h}:flags={flags}")
}

fn q_index(quality: &str) -> usize {
    match quality {
        "best" => 0,
        "smaller" => 2,
        _ => 1,
    }
}

/// Encoder arguments for `enc` at `quality`. "high" is the CTT Upscaler's own.
fn codec_args(enc: &str, quality: &str) -> Vec<String> {
    let q = q_index(quality);
    let pick = |v: [u32; 3]| v[q].to_string();
    let a: Vec<String> = match enc {
        "nvenc" => vec!["-c:v".into(), "h264_nvenc".into(), "-preset".into(), "p7".into(), "-rc".into(), "vbr".into(), "-b:v".into(), "250M".into(), "-cq".into(), pick([15, 18, 23]), "-profile:v".into(), "high".into()],
        "hevc_nvenc" => vec!["-c:v".into(), "hevc_nvenc".into(), "-preset".into(), "p7".into(), "-rc".into(), "vbr".into(), "-b:v".into(), "250M".into(), "-cq".into(), pick([15, 18, 23]), "-tag:v".into(), "hvc1".into()],
        "amf" => {
            let qp = pick([10, 12, 18]);
            vec!["-c:v".into(), "h264_amf".into(), "-quality".into(), "quality".into(), "-rc".into(), "cqp".into(), "-qp_i".into(), qp.clone(), "-qp_p".into(), qp.clone(), "-qp_b".into(), qp]
        }
        "hevc_amf" => vec!["-c:v".into(), "hevc_amf".into(), "-quality".into(), "quality".into(), "-rc".into(), "cqp".into(), "-qp_i".into(), pick([14, 16, 20]), "-qp_p".into(), pick([16, 18, 22]), "-tag:v".into(), "hvc1".into()],
        "qsv" => vec!["-c:v".into(), "h264_qsv".into(), "-preset".into(), "veryslow".into(), "-global_quality:v".into(), pick([12, 15, 20])],
        "hevc_qsv" => vec!["-c:v".into(), "hevc_qsv".into(), "-preset".into(), "veryslow".into(), "-global_quality:v".into(), pick([15, 18, 23]), "-tag:v".into(), "hvc1".into()],
        "x264" => vec!["-c:v".into(), "libx264".into(), "-preset".into(), "medium".into(), "-x264-params".into(), "aq-mode=3".into(), "-crf".into(), pick([12, 15, 20])],
        // Any other name is passed to ffmpeg as it is (the self-test's broken encoder).
        other => vec!["-c:v".into(), other.into()],
    };
    a
}

/// The encoders to try, in order, for hardware family `hw` ("nvenc", "amf", "qsv" or "x264" for none)
/// and a result `width` pixels wide: H.264 on the card, its HEVC when wider than H.264 allows, then x264.
fn chain_for(hw: &str, width: u32) -> Vec<String> {
    let mut v = Vec::new();
    if matches!(hw, "nvenc" | "amf" | "qsv") {
        if width <= H264_HW_MAX_W {
            v.push(hw.to_string());
        }
        v.push(format!("hevc_{hw}"));
    }
    v.push("x264".into());
    v
}

fn chain(ffmpeg: &Path, o: &UpscaleOpts, width: u32) -> Vec<String> {
    let hw = match o.encoder.as_str() {
        "nvenc" | "amf" | "qsv" | "x264" => o.encoder.clone(),
        _ => crate::editor::pick_encoder(ffmpeg, "auto"),
    };
    chain_for(&hw, width)
}

/// Audio codecs an MP4 holds as they are; anything else becomes AAC.
const MP4_AUDIO: [&str; 7] = ["aac", "mp3", "ac3", "eac3", "opus", "flac", "alac"];

fn audio_codecs(ffmpeg: &Path, file: &Path) -> Vec<String> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// The whole ffmpeg command line for one try.
fn args_for(input: &Path, part: &Path, vf: &str, enc: &str, quality: &str, audio: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y", "-reinit_filter", "0", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(input.to_string_lossy().into_owned());
    a.extend(["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-vf"].iter().map(|s| s.to_string()));
    a.push(vf.to_string());
    // Every frame keeps its own time: the frame rate and timing are the clip's.
    a.extend(["-fps_mode".into(), "passthrough".into()]);
    a.extend(codec_args(enc, quality));
    a.extend(["-pix_fmt".into(), "yuv420p".into()]);
    for (i, c) in audio.iter().enumerate() {
        if MP4_AUDIO.contains(&c.as_str()) {
            a.extend([format!("-c:a:{i}"), "copy".into()]);
        } else {
            a.extend([format!("-c:a:{i}"), "aac".into(), format!("-b:a:{i}"), "320k".into()]);
        }
    }
    a.extend(["-sn", "-dn", "-movflags", "+faststart", "-f", "mp4"].iter().map(|s| s.to_string()));
    a.push(part.to_string_lossy().into_owned());
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
    crate::util::track_child(&child); // ends with Glint (0.1.41)
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

/// The work file beside `output`: "<name>.part" (removed again on any failure).
fn part_of(output: &Path) -> PathBuf {
    let name = output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "upscale.mp4".into());
    output.with_file_name(format!("{name}.part"))
}

/// Upscale video `input` into the MP4 `output` (sound copied). `progress` gets 0..100.
/// Once `cancel` is set it stops within a moment, removes the partial file and returns `Err(CANCELLED)`.
pub fn run(ffmpeg: &Path, input: &Path, output: &Path, o: &UpscaleOpts, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    run_with(ffmpeg, input, output, o, None, cancel, &mut |_| {}, progress).map(|_| ())
}

/// `run`, saying which encoder it starts (`on_encoder`, again on a fallback) and
/// returning the one that made the file. `encoders` replaces the automatic chain (the self-test).
#[allow(clippy::too_many_arguments)]
fn run_with(ffmpeg: &Path, input: &Path, output: &Path, o: &UpscaleOpts, encoders: Option<&[&str]>, cancel: &AtomicBool, on_encoder: &mut dyn FnMut(&str), progress: &mut dyn FnMut(f32)) -> Result<String, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    let p = library::probe(ffmpeg, input, false).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
    if p.video_streams == 0 || p.height == 0 {
        return Err("This file has no picture to upscale".into());
    }
    if let Some(why) = refusal(p.height, o.height) {
        return Err(why);
    }
    let (w, h) = target_size(p.width, p.height, o.height);
    let vf = video_filter(o, w, h);
    let audio = audio_codecs(ffmpeg, input);
    let list: Vec<String> = match encoders {
        Some(e) => e.iter().map(|s| s.to_string()).collect(),
        None => chain(ffmpeg, o, w),
    };
    let part = part_of(output);
    let mut top = 0f32;
    let mut report = |pct: f32| {
        if pct > top {
            top = pct;
            progress(pct);
        }
    };
    let mut last_err = String::from("No encoder could make the file");
    for enc in &list {
        on_encoder(enc);
        let started = Instant::now();
        util::log(&format!("upscale: {} {}x{} -> {w}x{h} ({}), encoder {enc}, quality {}", input.display(), p.width, p.height, o.filter, o.quality));
        let args = args_for(input, &part, &vf, enc, &o.quality, &audio);
        let res = run_ffmpeg(ffmpeg, &args, p.duration, cancel, &mut |f| report((f * 99.0) as f32));
        match res {
            Ok(()) if part.is_file() => {
                let _ = fs::remove_file(output);
                if let Err(e) = fs::rename(&part, output) {
                    let _ = fs::remove_file(&part);
                    return Err(format!("Could not save the upscaled clip: {e}"));
                }
                // The titles of the sound tracks (the editor tells Game, Microphone and the mix apart by them).
                crate::clip::restore_titles(ffmpeg, input, output);
                util::log(&format!("upscale: done with {enc} in {:.1} s", started.elapsed().as_secs_f64()));
                report(100.0);
                return Ok(enc.clone());
            }
            Ok(()) => last_err = "ffmpeg wrote no file".into(),
            Err(e) if e == CANCELLED => {
                let _ = fs::remove_file(&part);
                return Err(CANCELLED.into());
            }
            Err(e) => {
                util::log(&format!("upscale: {enc} failed: {e}"));
                last_err = e;
            }
        }
        let _ = fs::remove_file(&part);
    }
    Err(format!("Could not upscale the clip: {last_err}"))
}

// ---------------------------------------------------------------------------
// The job and the command.

/// Queue an upscale of clip `clip_id`. Refused before queueing when the clip is already that tall.
pub fn enqueue(core: &Arc<Core>, clip_id: String, o: UpscaleOpts) -> Result<Job, String> {
    let meta = core.lib.lock().unwrap().get(&clip_id).ok_or("Clip not found")?;
    let src = PathBuf::from(&meta.path);
    if !src.is_file() {
        return Err("The clip's video file is missing".into());
    }
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let src_h = if meta.height > 0 { meta.height } else { library::probe(&ffmpeg, &src, false).map(|p| p.height).unwrap_or(0) };
    if let Some(why) = refusal(src_h, o.height) {
        return Err(why);
    }
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let job_id: Arc<std::sync::OnceLock<String>> = Arc::default();
    let jid = job_id.clone();
    let m: ClipMeta = meta.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let _working = crate::update::working(); // no automatic update restart meanwhile
        let started = Instant::now();
        let core = weak.upgrade().ok_or("Glint is closing")?;
        if !src.is_file() {
            return Err("The clip's video file is missing".into());
        }
        // Beside the clip; a clip imported in place belongs to another program: the clips folder then.
        let clips_dir = PathBuf::from(&s.clips_dir);
        let beside = crate::core::in_clips_dir(&clips_dir, &m.path);
        let dir = if beside { src.parent().map(Path::to_path_buf).unwrap_or_else(|| clips_dir.clone()) } else { clips_dir.clone() };
        fs::create_dir_all(&dir).map_err(|e| format!("Could not use the folder {}: {e}", dir.display()))?;
        let title = result_title(&m.title, o.height);
        let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
        let enc = run_with(&ffmpeg, &src, &out, &o, None, cancel, &mut |e| core.jobs.stage(&m.id, "upscale", e), progress)?;
        let mut new = library::register(&ffmpeg, &out, &title, &m.game, "upscale");
        new.folder = if beside { m.folder.clone() } else { String::new() };
        core.lib.lock().unwrap().add(new.clone());
        core.emit("clips-changed", new.id.clone());
        let secs = started.elapsed().as_secs_f64();
        core.emit("clip-upscaled", json!({ "clip": new, "size": new.size, "encoder": enc, "secs": secs, "job_id": jid.get().cloned().unwrap_or_default(), "source": m.id }));
        Ok(new.id)
    });
    let job = core.jobs.enqueue(&clip_id, &meta.title, "upscale", run)?;
    let _ = job_id.set(job.id.clone());
    Ok(job)
}

type St<'a> = tauri::State<'a, Arc<Core>>;

/// Queue an upscale of each clip in `ids` (one job each). Clips that are refused are
/// left out; when none could be queued the first reason is the error.
#[tauri::command(async)]
pub fn upscale_clips(core: St, ids: Vec<String>, opts: UpscaleOpts) -> Result<Vec<Job>, String> {
    let mut jobs = Vec::new();
    let mut first_err = None;
    for id in ids {
        match enqueue(core.inner(), id, opts.clone()) {
            Ok(j) => jobs.push(j),
            Err(e) => {
                util::log(&format!("upscale: refused: {e}"));
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) if jobs.is_empty() => Err(e),
        _ => Ok(jobs),
    }
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

/// A moving test picture with `titles.len()` tone tracks carrying those titles.
fn make_clip(ffmpeg: &Path, out: &Path, w: u32, h: u32, fps: u32, secs: f64, titles: &[&str]) -> Result<(), String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(format!("testsrc2=s={w}x{h}:r={fps}:d={secs}"));
    for i in 0..titles.len() {
        a.extend(["-f".into(), "lavfi".into(), "-i".into(), format!("sine=frequency={}:sample_rate=48000:duration={secs}", 300 + 400 * i)]);
    }
    a.extend(["-map".into(), "0:v".into()]);
    for i in 0..titles.len() {
        a.extend(["-map".into(), format!("{}:a", i + 1)]);
    }
    a.extend(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "20", "-pix_fmt", "yuv420p"].iter().map(|s| s.to_string()));
    if !titles.is_empty() {
        a.extend(["-c:a", "aac", "-b:a", "128k", "-ac", "2"].iter().map(|s| s.to_string()));
    }
    for (i, t) in titles.iter().enumerate() {
        a.extend([format!("-metadata:s:a:{i}"), format!("title={t}")]);
    }
    a.push(out.to_string_lossy().into_owned());
    let o = util::hidden_cmd(ffmpeg).args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(util::last_lines(&o.stderr, 2)) }
}

/// The MD5 of audio track `i`'s packets (equal = copied as it was).
fn audio_md5(ffmpeg: &Path, file: &Path, i: usize) -> String {
    util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{i}"), "-c", "copy", "-f", "md5", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The video codec's name ("h264", "hevc").
fn video_codec(ffmpeg: &Path, file: &Path) -> String {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return String::new() };
    util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Files in `dir` whose names start with `prefix`.
fn files_like(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(prefix)).collect())
        .unwrap_or_default()
}

/// `glint.exe --upscale-test <report.txt>`: headless checks of Upscale for YouTube on made-up clips.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run_self_test(&mut r);
    let failed = r.failed;
    r.line(&format!("DONE failed={failed}"));
}

fn run_self_test(r: &mut Report) {
    // 1. The pure parts.
    let sizes: &[((u32, u32, u32), (u32, u32))] = &[
        ((1920, 1080, 2160), (3840, 2160)),
        ((1920, 1080, 1440), (2560, 1440)),
        ((1280, 720, 1080), (1920, 1080)),
        ((2560, 1440, 2160), (3840, 2160)),
        ((3440, 1440, 2160), (5160, 2160)),
        ((1080, 1920, 2160), (1216, 2160)),
        ((853, 480, 1440), (2560, 1440)),
        ((1440, 1080, 2160), (2880, 2160)),
    ];
    for &((w, h, t), want) in sizes {
        let got = target_size(w, h, t);
        r.check(&format!("size: {w}x{h} to {t} -> {}x{}", want.0, want.1), got == want, &format!("got {got:?}"));
    }
    r.check("labels: 4K, 1440p, 1080p", label(2160) == "4K" && label(1440) == "1440p" && label(1080) == "1080p", "");
    r.check("targets: 1080p, 1440p and 4K, each open to a 720p clip", HEIGHTS == [1080, 1440, 2160] && HEIGHTS.iter().all(|&h| refusal(720, h).is_none()), "");
    r.check("result title: \"Ace (4K)\"", result_title("Ace", 2160) == "Ace (4K)", &result_title("Ace", 2160));
    let why = refusal(2160, 2160).unwrap_or_default();
    r.check("refusal: a 2160p clip is not upscaled to 4K, in a plain sentence", why.contains("already 2160p") && why.contains("4K"), &why);
    r.check("refusal: a 1440p clip is refused for 1440p and taken for 4K", refusal(1440, 1440).is_some() && refusal(1440, 2160).is_none(), "");
    r.check("refusal: a height outside 240..4320", refusal(720, 10_000).is_some(), "");
    let d = UpscaleOpts::default();
    r.check("default: 4K, sharp, the CTT settings, automatic encoder", d.height == 2160 && d.filter == "sharp" && d.quality == "high" && d.encoder == "auto", &format!("{d:?}"));
    let old: UpscaleOpts = serde_json::from_str(r#"{"height":1440}"#).unwrap_or_default();
    r.check("settings: an old {height} reads with the other defaults", old.height == 1440 && old.filter == "sharp" && old.quality == "high", &format!("{old:?}"));
    r.check("filter: sharp is nearest-neighbour, smooth is lanczos", video_filter(&d, 3840, 2160) == "scale=3840:2160:flags=neighbor" && video_filter(&UpscaleOpts { filter: "smooth".into(), ..d.clone() }, 3840, 2160).ends_with("flags=lanczos"), "");
    let nv = codec_args("nvenc", "high").join(" ");
    r.check("encoder: NVENC High is the CTT Upscaler's (p7, vbr, 250M, cq 18)", nv.contains("h264_nvenc -preset p7 -rc vbr -b:v 250M -cq 18"), &nv);
    let x = codec_args("x264", "high").join(" ");
    r.check("encoder: x264 High is aq-mode 3 at crf 15", x.contains("aq-mode=3") && x.contains("-crf 15"), &x);
    r.check("encoder: Best is finer and Smaller coarser than High", codec_args("nvenc", "best").contains(&"15".to_string()) && codec_args("nvenc", "smaller").contains(&"23".to_string()), "");
    r.check("chain: card H.264, its HEVC, then x264", chain_for("nvenc", 3840) == ["nvenc", "hevc_nvenc", "x264"], &format!("{:?}", chain_for("nvenc", 3840)));
    r.check("chain: wider than 4096 skips hardware H.264", chain_for("amf", 5160) == ["hevc_amf", "x264"], &format!("{:?}", chain_for("amf", 5160)));
    r.check("chain: no card encoder means x264 only", chain_for("x264", 3840) == ["x264"], "");
    let a = args_for(Path::new("in.mp4"), Path::new("out.mp4.part"), "scale=1:1", "x264", "high", &["aac".into(), "pcm_s16le".into()]).join(" ");
    r.check("args: AAC copied, PCM made AAC, timing passed through, MP4 out", a.contains("-c:a:0 copy") && a.contains("-c:a:1 aac") && a.contains("-fps_mode passthrough") && a.contains("-f mp4 out.mp4.part"), &a);

    // 2. Real runs.
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "no ffmpeg.exe");
        return;
    };
    let tmp = util::tmp_dir().join(format!("upscale-test-{}", util::new_id()));
    let _ = fs::create_dir_all(&tmp);
    let src = tmp.join("src.mp4");
    if let Err(e) = make_clip(&ffmpeg, &src, 1280, 720, 60, 2.0, &["Game", "Microphone"]) {
        r.check("made the test clip", false, &e);
        return;
    }
    let hw = crate::editor::pick_encoder(&ffmpeg, "auto");
    r.line(&format!("INFO card encoder on this PC: {hw}"));

    // 4K, automatic encoder.
    let out = tmp.join("out 4K.mp4");
    let mut pcts: Vec<f32> = Vec::new();
    let mut encs: Vec<String> = Vec::new();
    let t0 = Instant::now();
    let res = run_with(&ffmpeg, &src, &out, &d, None, &AtomicBool::new(false), &mut |e| encs.push(e.to_string()), &mut |p| pcts.push(p));
    let secs = t0.elapsed().as_secs_f64();
    r.check("4K: the run succeeds", res.is_ok(), &format!("{res:?}"));
    r.line(&format!("INFO 4K of 2 s 720p60 with {:?} took {secs:.1} s, {} bytes", res, fs::metadata(&out).map(|m| m.len()).unwrap_or(0)));
    let (ps, po) = (library::probe(&ffmpeg, &src, true).unwrap_or_default(), library::probe(&ffmpeg, &out, true).unwrap_or_default());
    r.check("4K: 3840x2160", (po.width, po.height) == (3840, 2160), &format!("{}x{}", po.width, po.height));
    r.check("4K: same frame rate and frame count", po.fps == ps.fps && po.frames == ps.frames && ps.frames > 0, &format!("{}@{} vs {}@{}", po.frames, po.fps, ps.frames, ps.fps));
    r.check("4K: same length", (po.duration - ps.duration).abs() < 0.05, &format!("{:.3} vs {:.3}", po.duration, ps.duration));
    r.check("4K: both sound tracks, copied bit for bit", po.audio_streams == 2 && (0..2).all(|i| { let m = audio_md5(&ffmpeg, &src, i); !m.is_empty() && m == audio_md5(&ffmpeg, &out, i) }), &format!("{} tracks", po.audio_streams));
    let titles = crate::clip::audio_titles(&ffmpeg, &out);
    r.check("4K: track titles kept", titles == ["Game", "Microphone"], &format!("{titles:?}"));
    r.check("4K: H.264 (plays everywhere)", video_codec(&ffmpeg, &out) == "h264", &video_codec(&ffmpeg, &out));
    r.check("progress: never goes back and ends at 100", pcts.windows(2).all(|w| w[1] >= w[0]) && pcts.last() == Some(&100.0) && pcts.len() > 1, &format!("{} values, last {:?}", pcts.len(), pcts.last()));
    r.check("4K: the first encoder tried is the card's (or x264 without one)", encs.first().map(String::as_str) == Some(hw.as_str()) || (hw == "x264" && encs.first().map(String::as_str) == Some("x264")), &format!("{encs:?}"));
    r.check("4K: no work file left", files_like(&tmp, "out 4K").len() == 1, &format!("{:?}", files_like(&tmp, "out 4K")));

    // Ultrawide 4K (5160 wide): past hardware H.264's limit, so the card's HEVC.
    if hw == "x264" {
        r.line("INFO no card encoder: the ultrawide HEVC check is skipped");
    } else {
        let wide = tmp.join("wide.mp4");
        let _ = make_clip(&ffmpeg, &wide, 1720, 720, 30, 1.0, &["Mix"]);
        let wout = tmp.join("wide 4K.mp4");
        let mut encs: Vec<String> = Vec::new();
        let res = run_with(&ffmpeg, &wide, &wout, &d, None, &AtomicBool::new(false), &mut |e| encs.push(e.to_string()), &mut |_| {});
        let pw = library::probe(&ffmpeg, &wout, false).unwrap_or_default();
        r.check("ultrawide: 5160x2160 made with the card's HEVC", (pw.width, pw.height) == (5160, 2160) && encs.first().is_some_and(|e| e.starts_with("hevc_")) && video_codec(&ffmpeg, &wout) == "hevc", &format!("{res:?} {encs:?} {}x{} {}", pw.width, pw.height, video_codec(&ffmpeg, &wout)));
    }

    // The refusal: the 4K result is not upscaled again.
    let again = tmp.join("again.mp4");
    let res = run(&ffmpeg, &out, &again, &d, &AtomicBool::new(false), &mut |_| {});
    r.check("refusal: a 4K clip to 4K is refused, nothing written", res.as_ref().err().is_some_and(|e| e.contains("already 2160p")) && files_like(&tmp, "again").is_empty(), &format!("{res:?}"));
    let res = run(&ffmpeg, &out, &again, &UpscaleOpts { height: 1440, ..d.clone() }, &AtomicBool::new(false), &mut |_| {});
    r.check("refusal: a 4K clip to 1440p is refused", res.is_err() && files_like(&tmp, "again").is_empty(), &format!("{res:?}"));
    let junk = tmp.join("junk.mp4");
    let _ = fs::write(&junk, b"not a video");
    let res = run(&ffmpeg, &junk, &again, &d, &AtomicBool::new(false), &mut |_| {});
    r.check("a file that is not a video fails, nothing written", res.is_err() && files_like(&tmp, "again").is_empty(), &format!("{res:?}"));

    // Software, smooth, 1440p, odd source width.
    let small = tmp.join("small.mp4");
    let _ = make_clip(&ffmpeg, &small, 854, 480, 30, 1.0, &["Mix"]);
    let soft = tmp.join("soft.mp4");
    let o = UpscaleOpts { height: 1440, filter: "smooth".into(), quality: "smaller".into(), encoder: "x264".into() };
    let mut encs: Vec<String> = Vec::new();
    let res = run_with(&ffmpeg, &small, &soft, &o, None, &AtomicBool::new(false), &mut |e| encs.push(e.to_string()), &mut |_| {});
    let ps = library::probe(&ffmpeg, &soft, true).unwrap_or_default();
    r.check("software: x264 chosen makes 2562x1440 at 30 fps (even width)", res.as_deref() == Ok("x264") && encs == ["x264"] && (ps.width, ps.height, ps.fps) == (2562, 1440, 30), &format!("{res:?} {}x{}@{}", ps.width, ps.height, ps.fps));

    // The fallback: a broken first encoder, then x264.
    let fb = tmp.join("fallback.mp4");
    let mut encs: Vec<String> = Vec::new();
    let mut pcts: Vec<f32> = Vec::new();
    let res = run_with(&ffmpeg, &small, &fb, &d, Some(&["glint_no_such_encoder", "x264"]), &AtomicBool::new(false), &mut |e| encs.push(e.to_string()), &mut |p| pcts.push(p));
    let pf = library::probe(&ffmpeg, &fb, false).unwrap_or_default();
    r.check("fallback: a failing encoder falls back to x264", res.as_deref() == Ok("x264") && encs == ["glint_no_such_encoder", "x264"] && pf.height == 2160, &format!("{res:?} {encs:?} {}p", pf.height));
    r.check("fallback: progress still never goes back", pcts.windows(2).all(|w| w[1] >= w[0]) && pcts.last() == Some(&100.0), "");
    r.check("fallback: no work file left", files_like(&tmp, "fallback").len() == 1, &format!("{:?}", files_like(&tmp, "fallback")));
    let res = run_with(&ffmpeg, &small, &tmp.join("none.mp4"), &d, Some(&["glint_no_such_encoder"]), &AtomicBool::new(false), &mut |_| {}, &mut |_| {});
    r.check("fallback: when every encoder fails it says why, nothing written", res.as_ref().err().is_some_and(|e| e.starts_with("Could not upscale")) && files_like(&tmp, "none").is_empty(), &format!("{res:?}"));

    // Cancel: software 4K of an 8 s clip, stopped after a second.
    let long = tmp.join("long.mp4");
    let _ = make_clip(&ffmpeg, &long, 1280, 720, 60, 8.0, &["Game"]);
    let cut = tmp.join("cancelled.mp4");
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let (ff, l2, cut2) = (ffmpeg.clone(), long.clone(), cut.clone());
    let worker = thread::spawn(move || {
        let mut n = 0usize;
        let res = run(&ff, &l2, &cut2, &UpscaleOpts { encoder: "x264".into(), ..UpscaleOpts::default() }, &c2, &mut |_| n += 1);
        (res, Instant::now())
    });
    thread::sleep(Duration::from_millis(1200));
    let at = Instant::now();
    cancel.store(true, Ordering::SeqCst);
    let (res, ended) = worker.join().unwrap_or((Err("panicked".into()), Instant::now()));
    let took = ended.saturating_duration_since(at).as_secs_f64();
    r.check("cancel: returns Cancelled within a second", res.as_ref().err().map(String::as_str) == Some(CANCELLED) && took < 1.0, &format!("{res:?} after {took:.2} s"));
    r.check("cancel: nothing left behind", files_like(&tmp, "cancelled").is_empty(), &format!("{:?}", files_like(&tmp, "cancelled")));
    let res = run(&ffmpeg, &long, &cut, &d, &AtomicBool::new(true), &mut |_| {});
    r.check("cancel: set before the start, nothing is done", res.as_ref().err().map(String::as_str) == Some(CANCELLED) && files_like(&tmp, "cancelled").is_empty(), &format!("{res:?}"));

    let _ = fs::remove_dir_all(&tmp);
}
