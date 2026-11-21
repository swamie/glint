//! GIF export (0.1.24). Owner: agent B. The names and signatures of `GifOpts`,
//! `make` and `CANCELLED` are fixed by CONTRACT_0124.md (rules.rs calls `make`);
//! fields may be ADDED to `GifOpts` (all with a serde default).
//!
//! How a GIF is made (`make`): the whole clip, at `fps` (never above the
//! clip's own rate) and `width` (lanczos, never wider than the clip), with
//! ffmpeg's own palette filters in two runs:
//! 1. `palettegen` reads the clip once and writes the best 256 colors for it
//!    (`stats_mode=diff`: the colors of what moves count most);
//! 2. `paletteuse` draws every frame with that palette (ordered dither, only
//!    the changed rectangle of each frame stored, which keeps files small).
//!
//! Two runs instead of one command with `split`: with `split`, paletteuse has
//! to hold every frame in memory until palettegen has seen the last one (a
//! 60 s 720p GIF at 30 fps is about 6.6 GB of frames). Two runs read the clip
//! twice but keep memory flat. Progress comes from ffmpeg's per-frame encoder
//! stats (`-stats_enc_pre pipe:1`), which, unlike `-progress`, also report
//! while the palette is being worked out.
//!
//! `max_mb > 0`: a result over the limit is made again smaller (first a lower
//! frame rate, down to `MIN_FPS`, then a smaller width, down to `MIN_WIDTH`),
//! at most `ROUNDS` times in all. If none fits, the smallest one is kept and
//! `make` still succeeds: the caller compares the file's size with the limit
//! (`over_limit`) and says so.
//!
//! The job (kind "gif", `enqueue`): the GIF is saved beside the clip as
//! "<title>.gif" (in the clips folder for clips imported in place), NOT added
//! to the library; "gif-done" { path, size, source, job_id, copied, over,
//! max_mb } tells the UI (gif.js). The job's result id is the GIF's path;
//! stage "over" is left on a done job whose GIF did not fit the limit.
//! Command: `gif_export { ids, opts } -> Vec<Job>`. Self-test: `--gif-test`.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::library;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// The error `make` returns when `cancel` stopped it.
pub const CANCELLED: &str = "Cancelled";

/// How a GIF is made. Saved in the settings as the dialog's last choices, and inside a render rule's GIF step.
/// Build one with `..Default::default()`: fields may be added.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GifOpts {
    /// Width in pixels (the height follows the picture's shape); 0 = the clip's own width.
    pub width: u32,
    /// Frames a second.
    pub fps: u32,
    /// Largest file size in MB (1 MB = 1,000,000 bytes); 0 = no limit.
    pub max_mb: u32,
    /// The dialog's "Copy to the clipboard when done" (the job does it; `make` ignores it).
    pub copy: bool,
}

impl Default for GifOpts {
    fn default() -> Self {
        GifOpts { width: 480, fps: 20, max_mb: 0, copy: false }
    }
}

/// GIF frame delays are in hundredths of a second: more than 50 a second does not play right.
const MAX_FPS: u32 = 50;
/// The size limit lowers the rate no further than this, then makes the picture smaller.
const MIN_FPS: u32 = 10;
/// ... and the width no further than this.
const MIN_WIDTH: u32 = 160;
/// Makes of one GIF at most (the first and the smaller ones).
const ROUNDS: usize = 4;
/// Share of the progress the palette run takes (it only decodes and counts colors).
const PALETTE_SHARE: f32 = 0.4;

/// The size limit in bytes (0 = none).
pub fn limit_bytes(max_mb: u32) -> u64 {
    max_mb as u64 * 1_000_000
}

/// True when a GIF of `size` bytes is over the limit of `o` (never without a limit).
pub fn over_limit(o: &GifOpts, size: u64) -> bool {
    o.max_mb > 0 && size > limit_bytes(o.max_mb)
}

/// The width and rate a GIF of a `src_w` wide clip at `src_fps` gets: never wider than the clip,
/// never faster than the clip (0 = unknown), 1..=50 frames a second.
pub fn plan(src_w: u32, src_fps: u32, o: &GifOpts) -> (u32, u32) {
    let w = if o.width == 0 || o.width > src_w { src_w } else { o.width };
    let mut fps = o.fps.clamp(1, MAX_FPS);
    if src_fps > 0 {
        fps = fps.min(src_fps);
    }
    (w.max(2), fps)
}

/// The next, smaller try after a GIF of `size` bytes at `w` x `fps` came out over `limit`:
/// first a lower rate (not under `MIN_FPS`), then a smaller width. None when nothing is left to lower.
pub fn shrink(w: u32, fps: u32, size: u64, limit: u64) -> Option<(u32, u32)> {
    if size <= limit || limit == 0 {
        return None;
    }
    // A GIF's size goes roughly with frames x pixels; aim a little under.
    let mut need = (limit as f64 / size as f64) * 0.9;
    let mut fps2 = fps;
    if fps > MIN_FPS {
        let f = ((fps as f64 * need).floor() as u32).clamp(MIN_FPS, fps - 1);
        need *= fps as f64 / f as f64;
        fps2 = f;
    }
    let mut w2 = w;
    if need < 0.97 && w > MIN_WIDTH {
        // Pixels go with the square of the width.
        w2 = ((w as f64 * need.sqrt()).floor() as u32).clamp(MIN_WIDTH, w - 1);
    }
    w2 &= !1; // an even width scales more cleanly
    w2 = w2.max(MIN_WIDTH.min(w));
    ((w2, fps2) != (w, fps)).then_some((w2, fps2))
}

/// `fps` and `scale` for the picture.
fn picture_filter(w: u32, fps: u32) -> String {
    format!("fps={fps},scale={w}:-1:flags=lanczos")
}

fn base_args() -> Vec<String> {
    ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-y"].iter().map(|s| s.to_string()).collect()
}

/// Run 1: the palette into `pal` (a PNG). The frames also go to a null output, whose per-frame stats tell the progress.
fn palette_args(input: &Path, pal: &Path, w: u32, fps: u32) -> Vec<String> {
    let mut a = base_args();
    a.extend(["-reinit_filter".into(), "0".into(), "-i".into(), input.to_string_lossy().into_owned()]);
    a.extend([
        "-filter_complex".into(),
        format!("[0:v:0]{},split[a][b];[a]palettegen=max_colors=256:stats_mode=diff[p]", picture_filter(w, fps)),
        "-map".into(),
        "[p]".into(),
        "-frames:v".into(),
        "1".into(),
        "-update".into(),
        "1".into(),
        pal.to_string_lossy().into_owned(),
        "-map".into(),
        "[b]".into(),
        "-stats_enc_pre".into(),
        "pipe:1".into(),
        "-stats_enc_pre_fmt".into(),
        "t={t}".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    a
}

/// Run 2: every frame with the palette, into the GIF `out`.
fn gif_args(input: &Path, pal: &Path, out: &Path, w: u32, fps: u32) -> Vec<String> {
    let mut a = base_args();
    a.extend(["-reinit_filter".into(), "0".into(), "-i".into(), input.to_string_lossy().into_owned(), "-i".into(), pal.to_string_lossy().into_owned()]);
    a.extend([
        "-filter_complex".into(),
        format!("[0:v:0]{}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle[g]", picture_filter(w, fps)),
        "-map".into(),
        "[g]".into(),
        "-stats_enc_pre".into(),
        "pipe:1".into(),
        "-stats_enc_pre_fmt".into(),
        "t={t}".into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-loop".into(),
        "0".into(),
        "-f".into(),
        "gif".into(),
        out.to_string_lossy().into_owned(),
    ]);
    a
}

/// Run ffmpeg, reporting 0..1 of `expected` seconds done (from the `t=` lines on stdout).
/// Kills it within about 0.1 s once `cancel` is set and returns `CANCELLED`.
fn run_ffmpeg(ffmpeg: &Path, args: &[String], expected: f64, cancel: &AtomicBool, on_frac: &mut dyn FnMut(f64)) -> Result<(), String> {
    crate::ffrun::run(ffmpeg, args, expected, cancel, on_frac).map(|_| ())
}

/// Removes every work file of one make (`<tmp>\<tag>*`) and the half-written output when dropped.
struct Cleanup {
    dir: PathBuf,
    tag: String,
    part: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in work_files(&self.dir, &self.tag) {
            let _ = fs::remove_file(&p);
        }
        let _ = fs::remove_file(&self.part);
    }
}

fn work_files(dir: &Path, tag: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(tag)).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// `<output>.part`: where the finished GIF is put before it gets its own name.
fn part_path(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// Make the GIF `output` from the whole of video `input`. `progress` gets 0..100 and never goes back.
/// Once `cancel` is set it stops within a moment, removes the partial file and returns `Err(CANCELLED)`.
/// With `o.max_mb` set and no try fitting, the smallest try is kept and this still returns Ok:
/// check the file's size with `over_limit`.
pub fn make(ffmpeg: &Path, input: &Path, output: &Path, o: &GifOpts, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    make_staged(ffmpeg, input, output, o, limit_bytes(o.max_mb), cancel, &mut |_| {}, progress)
}


/// `make` with the limit in bytes (the self-test uses limits under 1 MB) and a `stage`
/// callback: "smaller" once a try came out over the limit and a smaller one is being made.
#[allow(clippy::too_many_arguments)]
fn make_staged(ffmpeg: &Path, input: &Path, output: &Path, o: &GifOpts, limit: u64, cancel: &AtomicBool, stage: &mut dyn FnMut(&str), progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    let probe = library::probe(ffmpeg, input, false).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
    if probe.video_streams == 0 || probe.width == 0 {
        return Err("This file has no picture to make a GIF from".into());
    }
    let duration = probe.duration;
    let (mut w, mut fps) = plan(probe.width, probe.fps, o);
    let tmp = util::tmp_dir();
    let tag = format!("gif-{}", util::new_id());
    let part = part_path(output);
    let _guard = Cleanup { dir: tmp.clone(), tag: tag.clone(), part: part.clone() };
    let pal = tmp.join(format!("{tag}-palette.png"));
    let mut top = 0f32;
    let mut report = |p: f32| {
        if p > top {
            top = p;
            progress(p);
        }
    };
    // The first make has most of the bar; each smaller one a part of what is left.
    let spans: [(f32, f32); ROUNDS] = [(0.0, 90.0), (90.0, 95.0), (95.0, 98.0), (98.0, 99.5)];
    let mut best: Option<(u64, PathBuf)> = None; // the smallest try so far
    for (round, &(lo, hi)) in spans.iter().enumerate() {
        if round > 0 {
            stage("smaller");
        }
        let mid = lo + (hi - lo) * PALETTE_SHARE;
        let _ = fs::remove_file(&pal);
        run_ffmpeg(ffmpeg, &palette_args(input, &pal, w, fps), duration, cancel, &mut |f| report(lo + (mid - lo) * f as f32))?;
        if !pal.is_file() {
            return Err("ffmpeg made no palette for the GIF".into());
        }
        let file = tmp.join(format!("{tag}-{round}.gif"));
        run_ffmpeg(ffmpeg, &gif_args(input, &pal, &file, w, fps), duration, cancel, &mut |f| report(mid + (hi - mid) * f as f32))?;
        let size = fs::metadata(&file).map(|m| m.len()).map_err(|_| "ffmpeg wrote no GIF".to_string())?;
        util::log(&format!("gif: {} -> {}x?@{} = {} bytes (limit {limit}, round {})", input.display(), w, fps, size, round + 1));
        match &best {
            Some((b, _)) if *b <= size => {
                let _ = fs::remove_file(&file);
            }
            _ => {
                if let Some((_, old)) = best.take() {
                    let _ = fs::remove_file(&old);
                }
                best = Some((size, file));
            }
        }
        if limit == 0 || size <= limit {
            break;
        }
        match shrink(w, fps, size, limit) {
            Some((w2, f2)) => (w, fps) = (w2, f2),
            None => break,
        }
    }
    let (_, file) = best.ok_or("ffmpeg wrote no GIF")?;
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    // Into place: next to the output as `.part`, then its own name (so `output` only ever is a whole GIF).
    if fs::rename(&file, &part).is_err() {
        fs::copy(&file, &part).map_err(|e| format!("Could not save the GIF: {e}"))?;
    }
    fs::rename(&part, output).map_err(|e| format!("Could not save the GIF: {e}"))?;
    report(100.0);
    Ok(())
}

// ---------------------------------------------------------------------------
// The job and the command.

/// Where a GIF of clip `path` goes: beside it, or the clips folder for a clip imported in place.
fn out_dir(clips_dir: &Path, path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if crate::core::in_clips_dir(clips_dir, &path.to_string_lossy()) => p.to_path_buf(),
        _ => clips_dir.to_path_buf(),
    }
}

/// Whether the GIF goes on the clipboard: only with a running app, never in a self-test.
fn may_copy(core: &Core, want: bool) -> bool {
    want && core.app.get().is_some() && std::env::var_os("GLINT_NO_CLIPBOARD").is_none()
}

/// Queue a GIF of library clip `clip_id`.
pub fn enqueue(core: &Arc<Core>, clip_id: &str, o: &GifOpts) -> Result<Job, String> {
    let meta = core.lib.lock().unwrap().get(clip_id).ok_or("Clip not found")?;
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    if !Path::new(&meta.path).is_file() {
        return Err("The clip's video file is missing".into());
    }
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let job_id: Arc<std::sync::OnceLock<String>> = Arc::default();
    let jid = job_id.clone();
    let o = o.clone();
    let m = meta.clone();
    let clips_dir = PathBuf::from(&s.clips_dir);
    let run: Runner = Box::new(move |cancel, progress| {
        let _working = crate::update::working(); // no automatic update restart meanwhile
        let started = Instant::now();
        let core = weak.upgrade().ok_or("Glint is closing")?;
        let src = PathBuf::from(&m.path);
        if !src.is_file() {
            return Err("The clip's video file is missing".into());
        }
        let dir = out_dir(&clips_dir, &src);
        fs::create_dir_all(&dir).map_err(|e| format!("Could not use the folder {}: {e}", dir.display()))?;
        let out = util::unique_path(&dir, &util::sanitize_filename(&m.title), "gif");
        let id = m.id.clone();
        make_staged(&ffmpeg, &src, &out, &o, limit_bytes(o.max_mb), cancel, &mut |st| core.jobs.stage(&id, "gif", st), &mut |p| progress(p))?;
        let size = fs::metadata(&out).map(|md| md.len()).unwrap_or(0);
        let over = over_limit(&o, size);
        if over {
            core.jobs.stage(&m.id, "gif", "over");
        }
        let path = out.to_string_lossy().into_owned();
        let copied = may_copy(&core, o.copy) && match crate::app::files_to_clipboard(&[path.as_str()]) {
            Ok(()) => true,
            Err(e) => {
                util::log(&format!("gif: could not copy to the clipboard: {e}"));
                false
            }
        };
        util::log(&format!("gif: done in {:.1} s, {size} bytes, over={over}, copied={copied}", started.elapsed().as_secs_f64()));
        core.emit("gif-done", done_payload(&path, size, &m.id, jid.get().map(String::as_str).unwrap_or(""), copied, over, o.max_mb));
        Ok(path)
    });
    let job = core.jobs.enqueue(clip_id, &meta.title, "gif", run)?;
    let _ = job_id.set(job.id.clone());
    Ok(job)
}

/// The "gif-done" payload.
fn done_payload(path: &str, size: u64, source: &str, job_id: &str, copied: bool, over: bool, max_mb: u32) -> serde_json::Value {
    json!({ "path": path, "size": size, "source": source, "job_id": job_id, "copied": copied, "over": over, "max_mb": max_mb })
}

/// The options as the command takes them: a known rate and a sane width.
fn tidy(o: &GifOpts) -> GifOpts {
    GifOpts { fps: o.fps.clamp(1, MAX_FPS), width: if o.width == 0 { 0 } else { o.width.clamp(MIN_WIDTH, 7680) }, max_mb: o.max_mb.min(1000), copy: o.copy }
}

/// Queue a GIF of each clip. Fails only when none could be queued (with the first reason).
pub fn export(core: &Arc<Core>, ids: &[String], opts: &GifOpts) -> Result<Vec<Job>, String> {
    let o = tidy(opts);
    let mut jobs = Vec::new();
    let mut first_err = None;
    for id in ids {
        match enqueue(core, id, &o) {
            Ok(j) => jobs.push(j),
            Err(e) => {
                util::log(&format!("gif: {id} not queued: {e}"));
                first_err.get_or_insert(e);
            }
        }
    }
    match (jobs.is_empty(), first_err) {
        (true, Some(e)) => Err(e),
        _ => Ok(jobs),
    }
}

/// "Export as GIF…": one job per clip.
#[tauri::command(async)]
pub fn gif_export(core: tauri::State<'_, Arc<Core>>, ids: Vec<String>, opts: GifOpts) -> Result<Vec<Job>, String> {
    export(core.inner(), &ids, &opts)
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

/// A moving test picture with some noise (so the GIF is not tiny) and, with `audio`, a tone.
fn make_clip(ffmpeg: &Path, out: &Path, w: u32, h: u32, fps: u32, secs: u32, noise: u32, audio: bool) -> Result<(), String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(format!("testsrc2=s={w}x{h}:r={fps}:d={secs}"));
    if audio {
        a.extend(["-f".into(), "lavfi".into(), "-i".into(), format!("sine=frequency=440:sample_rate=48000:duration={secs}")]);
    }
    if noise > 0 {
        a.extend(["-vf".into(), format!("noise=alls={noise}:allf=t")]);
    }
    a.extend(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "18", "-pix_fmt", "yuv420p"].iter().map(|s| s.to_string()));
    if audio {
        a.extend(["-c:a", "aac", "-b:a", "128k"].iter().map(|s| s.to_string()));
    }
    a.push(out.to_string_lossy().into_owned());
    let o = util::hidden_cmd(ffmpeg).args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(util::last_lines(&o.stderr, 2)) }
}

/// What ffprobe says of a file: (format name, codec, width, height, r_frame_rate, frames, duration).
#[derive(Debug, Default)]
struct Facts {
    format: String,
    codec: String,
    width: u32,
    height: u32,
    rate: String,
    frames: u64,
    duration: f64,
}

fn facts(ffmpeg: &Path, file: &Path) -> Facts {
    let mut f = Facts::default();
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return f };
    let Ok(out) = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries", "stream=codec_name,width,height,r_frame_rate,nb_read_packets:format=format_name,duration", "-of", "default=nw=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return f;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k {
            "format_name" => f.format = v.into(),
            "codec_name" => f.codec = v.into(),
            "width" => f.width = v.parse().unwrap_or(0),
            "height" => f.height = v.parse().unwrap_or(0),
            "r_frame_rate" => f.rate = v.into(),
            "nb_read_packets" => f.frames = v.parse().unwrap_or(0),
            "duration" => f.duration = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    f
}

/// Files in `dir` (names only), sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// `glint.exe --gif-test <report.txt>`: headless checks of Export as GIF on made-up clips.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    std::env::set_var("GLINT_NO_CLIPBOARD", "1");
    run_self_test(&mut r);
    let failed = r.failed;
    r.line(&format!("DONE failed={failed}"));
}

fn run_self_test(r: &mut Report) {
    // 1. The pure parts.
    let o = |width, fps| GifOpts { width, fps, ..Default::default() };
    r.check("plan: 480 wide at 20 from 1920@60", plan(1920, 60, &o(480, 20)) == (480, 20), &format!("{:?}", plan(1920, 60, &o(480, 20))));
    r.check("plan: never wider than the clip", plan(320, 60, &o(480, 20)) == (320, 20), &format!("{:?}", plan(320, 60, &o(480, 20))));
    r.check("plan: width 0 = the clip's own width", plan(1280, 60, &o(0, 15)) == (1280, 15), "");
    r.check("plan: never faster than the clip", plan(1280, 30, &o(480, 50)) == (480, 30), "");
    r.check("plan: at most 50 frames a second", plan(1280, 144, &o(480, 100)) == (480, 50), "");
    r.check("plan: unknown clip rate keeps the choice", plan(1280, 0, &o(480, 15)) == (480, 15), "");
    r.check("shrink: nothing to do under the limit", shrink(480, 20, 900, 1000).is_none(), "");
    let s1 = shrink(480, 20, 1_500_000, 1_000_000);
    r.check("shrink: the rate goes down first", s1.is_some_and(|(w, f)| w == 480 && (MIN_FPS..20).contains(&f)), &format!("{s1:?}"));
    let s2 = shrink(480, 10, 2_000_000, 1_000_000);
    r.check("shrink: at the lowest rate the width goes down", s2.is_some_and(|(w, f)| f == 10 && (MIN_WIDTH..480).contains(&w) && w % 2 == 0), &format!("{s2:?}"));
    let s3 = shrink(480, 20, 20_000_000, 1_000_000);
    r.check("shrink: far over: both go down", s3.is_some_and(|(w, f)| f == MIN_FPS && w < 480), &format!("{s3:?}"));
    r.check("shrink: nothing left to lower", shrink(MIN_WIDTH, MIN_FPS, 20_000_000, 1_000_000).is_none(), "");
    let def = GifOpts::default();
    r.check("default options: 480 wide, 20 a second, no limit, no copy", def == GifOpts { width: 480, fps: 20, max_mb: 0, copy: false }, "");
    let old: Result<GifOpts, _> = serde_json::from_str(r#"{"width":320,"fps":15,"max_mb":10}"#);
    r.check("settings saved before \"copy\" existed still load", old.as_ref().is_ok_and(|g| g.width == 320 && g.fps == 15 && g.max_mb == 10 && !g.copy), &format!("{old:?}"));
    let s = crate::settings::Settings::default();
    r.check("settings: gif defaults", s.gif == GifOpts::default(), "");
    r.check("over_limit: no limit is never over", !over_limit(&o(480, 20), u64::MAX), "");
    let t = tidy(&GifOpts { width: 5, fps: 999, max_mb: 5000, copy: true });
    r.check("command options are tidied", t.width == MIN_WIDTH && t.fps == MAX_FPS && t.max_mb == 1000 && t.copy, &format!("{t:?}"));

    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let root = util::data_dir().join("clips");
    let _ = fs::remove_dir_all(&root);
    let dir = root.join("Test Game");
    let _ = fs::create_dir_all(&dir);
    let tmp = util::tmp_dir();
    let tmp_before = work_files(&tmp, "gif-");

    // 2. A 6 s 1280x720 60 fps clip with sound -> 480 wide at 20 a second.
    let src = dir.join("source.mp4");
    let built = make_clip(&ffmpeg, &src, 1280, 720, 60, 6, 10, true);
    r.check("build a 6 s 1280x720 60 fps test clip with sound", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    if built.is_err() {
        return;
    }
    let out = dir.join("source.gif");
    let mut pcts: Vec<f32> = Vec::new();
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let res = make(&ffmpeg, &src, &out, &GifOpts { width: 480, fps: 20, ..Default::default() }, &cancel, &mut |p| pcts.push(p));
    let took = t0.elapsed().as_secs_f64();
    r.check("make 480 wide at 20 a second succeeds", res.is_ok(), res.as_ref().err().map(String::as_str).unwrap_or(""));
    let f = facts(&ffmpeg, &out);
    let size = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    r.line(&format!("INFO 6 s 720p60 -> 480 wide @ 20: {size} bytes in {took:.1} s, {} progress updates", pcts.len()));
    r.check("the file is a GIF", f.format == "gif" && f.codec == "gif", &format!("{} / {}", f.format, f.codec));
    r.check("width 480, height 270 (shape kept)", f.width == 480 && f.height == 270, &format!("{}x{}", f.width, f.height));
    r.check("20 frames a second", f.rate == "20/1", &f.rate);
    r.check("length kept (6 s, 120 frames)", (f.duration - 6.0).abs() < 0.15 && (118..=121).contains(&f.frames), &format!("{:.2} s, {} frames", f.duration, f.frames));
    r.check("progress reached 100", pcts.last() == Some(&100.0), &format!("{:?}", pcts.last()));
    r.check("progress never went back", pcts.windows(2).all(|w| w[1] >= w[0]), "");
    r.check("progress was reported during the palette run and the GIF run", pcts.iter().any(|&p| p > 1.0 && p < 36.0) && pcts.iter().any(|&p| p > 36.0 && p < 90.0), &format!("{} updates", pcts.len()));
    r.check("nothing but the GIF beside the clip", names_in(&dir) == ["source.gif", "source.mp4"], &format!("{:?}", names_in(&dir)));
    let left: Vec<PathBuf> = work_files(&tmp, "gif-").into_iter().filter(|p| !tmp_before.contains(p)).collect();
    r.check("no work files left in the temp folder", left.is_empty(), &format!("{left:?}"));

    // 3. Never upscaled, never faster than the clip; width 0.
    let small = dir.join("small.mp4");
    let built = make_clip(&ffmpeg, &small, 320, 240, 30, 3, 0, false);
    r.check("build a 3 s 320x240 30 fps clip", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    let sout = dir.join("small.gif");
    let res = make(&ffmpeg, &small, &sout, &GifOpts { width: 480, fps: 50, ..Default::default() }, &cancel, &mut |_| {});
    let f = facts(&ffmpeg, &sout);
    r.check("a 320 wide clip at 480 stays 320 wide and 30 a second", res.is_ok() && f.width == 320 && f.height == 240 && f.rate == "30/1", &format!("{:?} {}x{} {}", res.err(), f.width, f.height, f.rate));
    let _ = fs::remove_file(&sout);
    let res = make(&ffmpeg, &small, &sout, &GifOpts { width: 0, fps: 10, ..Default::default() }, &cancel, &mut |_| {});
    let f = facts(&ffmpeg, &sout);
    r.check("width 0 = the clip's own width, 10 a second", res.is_ok() && f.width == 320 && f.rate == "10/1" && (29..=31).contains(&f.frames), &format!("{}x{} {} {} frames", f.width, f.height, f.rate, f.frames));
    let _ = fs::remove_file(&sout);

    // 4. The size limit: a limit under the first try is met by a smaller one.
    let first = size;
    let limit = first * 2 / 5;
    let lim_out = dir.join("limit.gif");
    let mut stages: Vec<String> = Vec::new();
    let mut pcts: Vec<f32> = Vec::new();
    let res = make_staged(&ffmpeg, &src, &lim_out, &GifOpts { width: 480, fps: 20, ..Default::default() }, limit, &cancel, &mut |s| stages.push(s.into()), &mut |p| pcts.push(p));
    let lsize = fs::metadata(&lim_out).map(|m| m.len()).unwrap_or(0);
    let f = facts(&ffmpeg, &lim_out);
    r.line(&format!("INFO limit {limit} bytes (first try {first}): {lsize} bytes at {}x{} {}", f.width, f.height, f.rate));
    r.check("size limit: the result fits", res.is_ok() && lsize > 0 && lsize <= limit, &format!("{lsize} of {limit} bytes {:?}", res.err()));
    r.check("size limit: made again smaller (stage \"smaller\")", stages.iter().any(|s| s == "smaller"), &format!("{stages:?}"));
    r.check("size limit: progress never went back and reached 100", pcts.windows(2).all(|w| w[1] >= w[0]) && pcts.last() == Some(&100.0), "");
    r.check("size limit: still a GIF of the whole clip", f.format == "gif" && (f.duration - 6.0).abs() < 0.25, &format!("{} {:.2}", f.format, f.duration));
    let _ = fs::remove_file(&lim_out);
    // A limit nothing can meet: the smallest try is kept, and it is over.
    let res = make_staged(&ffmpeg, &small, &lim_out, &GifOpts { width: 320, fps: 30, ..Default::default() }, 2_000, &cancel, &mut |_| {}, &mut |_| {});
    let tsize = fs::metadata(&lim_out).map(|m| m.len()).unwrap_or(0);
    let f = facts(&ffmpeg, &lim_out);
    r.check("a limit that cannot be met keeps the smallest try (and it is over)", res.is_ok() && tsize > 2_000 && f.format == "gif" && f.width <= 320, &format!("{tsize} bytes {}x{} {} {:?}", f.width, f.height, f.rate, res.err()));
    r.check("over_limit says so", over_limit(&GifOpts { max_mb: 1, ..Default::default() }, 1_000_001) && !over_limit(&GifOpts { max_mb: 1, ..Default::default() }, 1_000_000), "");
    let _ = fs::remove_file(&lim_out);

    // 5. Cancel: in the palette run and in the GIF run.
    for (name, at) in [("the palette run", 10.0f32), ("the GIF run", 60.0)] {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let set_at: Arc<std::sync::Mutex<Option<Instant>>> = Arc::default();
        let sa = set_at.clone();
        let mut pcts: Vec<f32> = Vec::new();
        let cout = dir.join("cancel.gif");
        let res = make(&ffmpeg, &src, &cout, &GifOpts { width: 640, fps: 30, ..Default::default() }, &cancel, &mut |p| {
            pcts.push(p);
            if p > at && !c.swap(true, Ordering::SeqCst) {
                *sa.lock().unwrap() = Some(Instant::now());
            }
        });
        let stop = set_at.lock().unwrap().map(|t| t.elapsed().as_secs_f64()).unwrap_or(f64::NAN);
        r.check(&format!("cancel in {name} returns Cancelled"), res.as_ref().err().map(String::as_str) == Some(CANCELLED), &format!("{res:?}"));
        r.check(&format!("cancel in {name} stops within a second"), stop < 1.0, &format!("{stop:.2} s"));
        r.check(&format!("cancel in {name} leaves nothing behind"), !cout.exists() && !part_path(&cout).exists() && names_in(&dir) == ["small.mp4", "source.gif", "source.mp4"], &format!("{:?}", names_in(&dir)));
        r.check(&format!("cancel in {name}: progress never went back"), pcts.windows(2).all(|w| w[1] >= w[0]), "");
        let left: Vec<PathBuf> = work_files(&tmp, "gif-").into_iter().filter(|p| !tmp_before.contains(p)).collect();
        r.check(&format!("cancel in {name}: no work files left"), left.is_empty(), &format!("{left:?}"));
    }

    // 6. Through the queue: beside the clip, not in the library, the event's fields, refusals.
    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = root.to_string_lossy().into_owned();
    }
    core.jobs.connect(&core);
    let mut meta = library::register(&ffmpeg, &small, "My clip: best", "Test Game", "clip");
    meta.folder = "Test Game".into();
    let id = meta.id.clone();
    core.lib.lock().unwrap().add(meta);
    let before = core.lib.lock().unwrap().clips.len();
    let jobs = export(&core, std::slice::from_ref(&id), &GifOpts { width: 320, fps: 15, ..Default::default() });
    r.check("gif_export queues one job of kind \"gif\"", jobs.as_ref().is_ok_and(|v| v.len() == 1 && v[0].kind == "gif"), &format!("{jobs:?}"));
    let dup = export(&core, std::slice::from_ref(&id), &GifOpts::default());
    r.check("a second GIF of the same clip is refused", dup.as_ref().err().map(String::as_str) == Some("A GIF of that clip is already being made"), &format!("{dup:?}"));
    let missing = export(&core, &["nope".to_string()], &GifOpts::default());
    r.check("an unknown clip is refused", missing.as_ref().err().map(String::as_str) == Some("Clip not found"), &format!("{missing:?}"));
    if let Ok(v) = &jobs {
        let jid = v[0].id.clone();
        let state = |jid: &str| core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();
        let until = Instant::now() + Duration::from_secs(120);
        while Instant::now() < until && matches!(state(&jid).state.as_str(), "queued" | "running") {
            thread::sleep(Duration::from_millis(50));
        }
        let j = state(&jid);
        r.check("the GIF job finishes", j.state == "done", &format!("{} {}", j.state, j.message));
        let p = PathBuf::from(&j.result_id);
        r.check("saved beside the clip as \"<title>.gif\" (title made safe for a file name)", p.parent() == Some(dir.as_path()) && p.extension().is_some_and(|e| e == "gif") && p.file_stem().is_some_and(|s| s.to_string_lossy() == util::sanitize_filename("My clip: best")), &j.result_id);
        r.check("the job's GIF is a GIF at 15 a second", facts(&ffmpeg, &p).rate == "15/1", "");
        r.check("nothing added to the library", core.lib.lock().unwrap().clips.len() == before, "");
        r.check("no \"over\" stage without a limit", j.stage != "over", &j.stage);
        let _ = fs::remove_file(&p);
    }
    let ev = done_payload("C:\\x.gif", 123, "c1", "j1", false, true, 10);
    r.check("gif-done payload has path, size, source, job_id, copied, over, max_mb", ev["path"] == "C:\\x.gif" && ev["size"] == 123 && ev["source"] == "c1" && ev["job_id"] == "j1" && ev["copied"] == false && ev["over"] == true && ev["max_mb"] == 10, &ev.to_string());
    // A clip imported in place (outside the clips folder): its GIF goes to the clips folder.
    let outside = util::data_dir().join("elsewhere");
    let _ = fs::create_dir_all(&outside);
    r.check("a clip outside the clips folder: GIF into the clips folder", out_dir(&root, &outside.join("a.mp4")) == root && out_dir(&root, &dir.join("a.mp4")) == dir, "");
    let _ = fs::remove_dir_all(&outside);
    core.jobs.shutdown();
    let left: Vec<PathBuf> = work_files(&tmp, "gif-").into_iter().filter(|p| !tmp_before.contains(p)).collect();
    r.check("no work files left at the end", left.is_empty(), &format!("{left:?}"));
    let _ = fs::remove_dir_all(&root);
}
