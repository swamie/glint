//! Render rules (0.1.24): a named chain of steps (color look, canvas layout
//! (0.1.27), Blur, upscale, compress, GIF, copy) run on a clip with one click.
//!
//! A rule lives in `Settings::rules` (the UI edits it, ui/rules.js). Running it
//! queues one job of kind "rule" per clip (the compress lane of jobs.rs). The
//! steps work on temporary files (`util::tmp_dir()`, all named `rule-<id>-…`)
//! and only the final result is kept: a video becomes a library clip
//! "<clip title> (<rule name>)" of kind "rule", a GIF is saved beside the clip
//! and is not added to the library. A step with nothing to do is skipped; a
//! failing step fails the job with the step's name and the reason. When a job
//! ends well, "rule-done" is sent (see `finish`).
use crate::core::Core;
use crate::editor::{Canvas, CanvasPiece, Color};
use crate::settings::CanvasPreset;
use crate::gif::GifOpts;
use crate::jobs::{Job, Runner};
use crate::library::{self, ClipMeta};
use crate::settings::Settings;
use crate::upscale::UpscaleOpts;
use crate::{blur, compress, editor, util};
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

/// The step kinds, in the order the page offers them.
pub const KINDS: [&str; 7] = ["color", "canvas", "blur", "upscale", "compress", "gif", "copy"];

/// The error a step returns when the cancel flag stopped it.
pub const CANCELLED: &str = "Cancelled";
/// What the Blur button says when Blur is missing (jobs.rs `enqueue_blur`).
pub const BLUR_MISSING: &str = "blur-cli.exe was not found. Install Blur or set its path under Blur settings.";
/// Why an order was changed (the UI shows the same sentences).
pub const WHY_ONCE: &str = "Each kind of block can be used once.";
pub const WHY_GIF: &str = "A GIF is not a video, so only Copy can come after it.";
pub const WHY_COPY: &str = "Copy is always the last block.";
pub const WHY_UNKNOWN: &str = "An unknown block was left out.";
/// 0.1.27: the Canvas block (with Color, which the same export does) runs before every other step.
pub const WHY_CANVAS: &str = "Canvas goes first, with Color next to it (one export does both).";
/// A Canvas block whose layout was never picked.
pub const NO_LAYOUT: &str = "Pick a layout for the Canvas block.";
pub const NO_STEPS: &str = "This rule has no blocks yet. Add one first.";

/// One step of a rule. Only the fields of its kind are used; the others keep their defaults.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Step {
    /// One of `KINDS`.
    pub kind: String,
    /// "color": the look, by name: one of the editor's built-in looks (`BUILTIN_LOOKS`) or one of `Settings::color_presets`.
    pub look: String,
    /// "compress": the size to fit, in MB (1 MB = 1,000,000 bytes), `compress::MIN_MB..=MAX_MB`.
    pub mb: u32,
    /// "upscale": what upscale.rs is asked for.
    pub upscale: UpscaleOpts,
    /// "gif": what gif.rs is asked for.
    pub gif: GifOpts,
    /// "canvas": the saved layout (`Settings::canvas_presets`), by id; found by `layout_name`
    /// when that id is gone (saving a layout again under the same name gives it a new id).
    pub layout: String,
    pub layout_name: String,
}

impl Default for Step {
    fn default() -> Self {
        Step { kind: String::new(), look: "Vivid".into(), mb: 50, upscale: UpscaleOpts::default(), gif: GifOpts::default(), layout: String::new(), layout_name: String::new() }
    }
}

impl Step {
    pub fn of(kind: &str) -> Self {
        Step { kind: kind.into(), ..Default::default() }
    }
}

/// One render rule: a name and its steps, run in order on a clip.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub steps: Vec<Step>,
}

/// The editor's built-in looks (ui/editor.js `ED_PRESETS`, same names and numbers; the self-test compares them).
pub const BUILTIN_LOOKS: &[(&str, [f32; 4])] = &[
    ("None", [1.0, 1.0, 1.0, 1.0]),
    ("Vivid", [1.0, 1.08, 1.35, 1.0]),
    ("Punchy", [1.0, 1.25, 1.15, 0.95]),
    ("Soft", [1.02, 0.88, 0.9, 1.08]),
    ("Bright", [1.1, 1.0, 1.05, 1.15]),
    ("Moody", [0.94, 1.15, 0.8, 0.9]),
    ("Black and white", [1.0, 1.12, 0.0, 1.0]),
];

/// The color of look `name`: a built-in look first, then the user's own presets.
pub fn look_color(s: &Settings, name: &str) -> Option<Color> {
    let mk = |v: [f32; 4]| Color { brightness: v[0], contrast: v[1], saturation: v[2], gamma: v[3] };
    if let Some((_, v)) = BUILTIN_LOOKS.iter().find(|(n, _)| *n == name) {
        return Some(mk(*v));
    }
    s.color_presets.iter().find(|p| p.name == name).map(|p| mk([p.brightness, p.contrast, p.saturation, p.gamma]))
}

/// The saved layout a Canvas block names: by id, else by name (see `Step::layout`).
pub fn find_layout<'a>(s: &'a Settings, st: &Step) -> Option<&'a CanvasPreset> {
    s.canvas_presets
        .iter()
        .find(|p| !st.layout.is_empty() && p.id == st.layout)
        .or_else(|| s.canvas_presets.iter().find(|p| !st.layout_name.is_empty() && p.name == st.layout_name))
}

// ---------------------------------------------------------------------------
// A saved layout on another clip (ports of ui/canvas.js; the self-test compares numbers)

/// Smallest source box side, fraction of the clip's picture (canvas.js `CV_SRC_MIN`).
const CV_SRC_MIN: f64 = 0.02;

/// canvas.js `cvEven`: even, 16..=4096.
fn cv_even(x: f64) -> f64 {
    let x = if x.is_finite() { x } else { 0.0 };
    ((x / 2.0).round() * 2.0).clamp(16.0, 4096.0)
}

/// canvas.js `cvNorm`: a saved canvas made safe to use (sizes, boxes and background kept in range).
pub fn canvas_norm(c: &Canvas) -> Canvas {
    let n = |x: f64, d: f64| if x.is_finite() { x } else { d };
    let pieces = c
        .pieces
        .iter()
        .map(|p| {
            let sw = n(p.sw, 1.0).clamp(CV_SRC_MIN, 1.0);
            let sh = n(p.sh, 1.0).clamp(CV_SRC_MIN, 1.0);
            let mut q = p.clone(); // a clone, so fields other agents add to a piece are kept
            q.sx = n(p.sx, 0.0).clamp(0.0, 1.0 - sw);
            q.sy = n(p.sy, 0.0).clamp(0.0, 1.0 - sh);
            q.sw = sw;
            q.sh = sh;
            q.x = n(p.x, 0.0).clamp(-4.0, 4.0);
            q.y = n(p.y, 0.0).clamp(-4.0, 4.0);
            q.w = n(p.w, 1.0).clamp(0.005, 8.0);
            q.h = n(p.h, 1.0).clamp(0.005, 8.0);
            q
        })
        .collect();
    let b = c.background.as_str();
    let hex = b.len() == 7 && b.starts_with('#') && b[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    let background = if b == "blur" || b == "black" || hex { b.to_string() } else { "black".to_string() };
    let mut out = c.clone();
    out.width = cv_even(c.width);
    out.height = cv_even(c.height);
    out.background = background;
    out.pieces = pieces;
    out
}

/// canvas.js `cvRefit`: piece `p` of canvas `c` with its source box given the pixel shape of its
/// place again, for a clip picture of `src_w` x `src_h`: about as much of the picture, at the same
/// side of it, inside it.
pub fn canvas_refit(p: &CanvasPiece, c: &Canvas, src_w: f64, src_h: f64) -> CanvasPiece {
    let a = (p.w * c.width) / (p.h * c.height);
    let ax = if p.sw < 0.999 { (p.sx / (1.0 - p.sw)).clamp(0.0, 1.0) } else { 0.5 };
    let ay = if p.sh < 0.999 { (p.sy / (1.0 - p.sh)).clamp(0.0, 1.0) } else { 0.5 };
    let area = (p.sw * src_w * p.sh * src_h).max(1.0);
    let mut wpx = (area * a).sqrt();
    let mut hpx = wpx / a;
    let k = 1f64.min(src_w / wpx).min(src_h / hpx);
    wpx *= k;
    hpx *= k;
    let (sw, sh) = (wpx / src_w, hpx / src_h);
    let mut q = p.clone();
    q.sw = sw;
    q.sh = sh;
    q.sx = (1.0 - sw) * ax;
    q.sy = (1.0 - sh) * ay;
    q
}

/// Saved canvas `c` for a clip picture of `w` x `h` (what the editor's "My layouts" does: canvas.js `applyPreset`).
pub fn canvas_for_clip(c: &Canvas, w: u32, h: u32) -> Canvas {
    let mut n = canvas_norm(c);
    let pieces = n.pieces.iter().map(|p| canvas_refit(p, &n, w as f64, h as f64)).collect();
    n.pieces = pieces;
    n
}

// ---------------------------------------------------------------------------
// Order rules and words (pure)

/// `steps` in an order that can work, and why it was changed (None = it was fine):
/// unknown kinds and second uses of a kind are left out, Copy goes last, and
/// anything but Copy that came after a GIF goes before it (a GIF is not a video).
pub fn fix_order(steps: &[Step]) -> (Vec<Step>, Option<&'static str>) {
    let mut why: Option<&'static str> = None;
    let mut note = |w: &'static str| {
        if why.is_none() {
            why = Some(w);
        }
    };
    let mut out: Vec<Step> = Vec::new();
    for st in steps {
        if !KINDS.contains(&st.kind.as_str()) {
            note(WHY_UNKNOWN);
        } else if out.iter().any(|o| o.kind == st.kind) {
            note(WHY_ONCE);
        } else {
            out.push(st.clone());
        }
    }
    if let Some(g) = out.iter().position(|s| s.kind == "gif") {
        let after: Vec<usize> = (g + 1..out.len()).filter(|&i| out[i].kind != "copy").collect();
        if !after.is_empty() {
            note(WHY_GIF);
            let moved: Vec<Step> = after.iter().map(|&i| out[i].clone()).collect();
            for &i in after.iter().rev() {
                out.remove(i);
            }
            for (k, st) in moved.into_iter().enumerate() {
                out.insert(g + k, st);
            }
        }
    }
    if let Some(c) = out.iter().position(|s| s.kind == "copy") {
        if c + 1 != out.len() {
            note(WHY_COPY);
            let st = out.remove(c);
            out.push(st);
        }
    }
    // Canvas (and Color, which goes into the same export) first: the canvas is made from the clip itself.
    if out.iter().any(|s| s.kind == "canvas") {
        let front = |s: &Step| s.kind == "canvas" || s.kind == "color";
        let group: Vec<Step> = out.iter().filter(|s| front(s)).cloned().collect();
        if !out.iter().take(group.len()).all(front) {
            note(WHY_CANVAS);
            out.retain(|s| !front(s));
            for (k, st) in group.into_iter().enumerate() {
                out.insert(k, st);
            }
        }
    }
    (out, why)
}

/// The name of a step kind, as the page and error messages say it.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "color" => "Color",
        "canvas" => "Canvas",
        "blur" => "Blur",
        "upscale" => "Upscale",
        "compress" => "Compress",
        "gif" => "GIF",
        "copy" => "Copy",
        _ => "Step",
    }
}

/// A target height in words: "4K", "1440p".
fn height_words(h: u32) -> String {
    crate::upscale::label(h)
}

/// One step in words: "Color: Vivid", "Blur", "Upscale to 4K", "Compress to 50 MB", "GIF 480 px, 20 fps", "Copy".
pub fn step_words(st: &Step) -> String {
    match st.kind.as_str() {
        "color" => format!("Color: {}", st.look),
        "canvas" if !st.layout_name.is_empty() => format!("Canvas: {}", st.layout_name),
        "upscale" => format!("Upscale to {}", height_words(st.upscale.height)),
        "compress" => format!("Compress to {} MB", st.mb),
        "gif" => {
            let w = if st.gif.width == 0 { "full size".to_string() } else { format!("{} px", st.gif.width) };
            let cap = if st.gif.max_mb > 0 { format!(", up to {} MB", st.gif.max_mb) } else { String::new() };
            format!("GIF {w}, {} fps{cap}", st.gif.fps)
        }
        k => kind_label(k).to_string(),
    }
}

/// What a rule does, in words: "Color: Vivid → Blur → Compress to 50 MB → Copy".
pub fn describe(rule: &Rule) -> String {
    if rule.steps.is_empty() {
        return "No blocks yet".into();
    }
    rule.steps.iter().map(step_words).collect::<Vec<_>>().join(" → ")
}

/// Whether `rule` can run with settings `s`; the reason when not.
pub fn check(s: &Settings, rule: &Rule) -> Result<(), String> {
    if rule.steps.is_empty() {
        return Err(NO_STEPS.into());
    }
    let (fixed, why) = fix_order(&rule.steps);
    if let Some(w) = why.filter(|_| fixed != rule.steps) {
        return Err(w.into());
    }
    for st in &rule.steps {
        match st.kind.as_str() {
            "color" if look_color(s, &st.look).is_none() => return Err(format!("The color look \"{}\" no longer exists. Pick another one in the rule.", st.look)),
            "compress" if !(compress::MIN_MB..=compress::MAX_MB).contains(&st.mb) => return Err(format!("Pick a size from {} to {} MB", compress::MIN_MB, compress::MAX_MB)),
            "canvas" if st.layout.is_empty() && st.layout_name.is_empty() => return Err(NO_LAYOUT.into()),
            "canvas" if find_layout(s, st).is_none() => {
                return Err(format!("The canvas layout \"{}\" no longer exists. Pick another one in the rule.", if st.layout_name.is_empty() { &st.layout } else { &st.layout_name }))
            }
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Running the steps

/// How long a step takes compared with the others, for one progress bar over all of them.
fn weight(kind: &str) -> f32 {
    match kind {
        "color" | "canvas" => 1.0,
        "blur" => 3.0,
        "upscale" => 2.0,
        "compress" => 1.5,
        "gif" => 1.0,
        _ => 0.1,
    }
}

/// Runs the Blur step: (input, output, cancel, progress 0..100).
pub(crate) type BlurFn = Box<dyn Fn(&Path, &Path, &AtomicBool, &mut dyn FnMut(f32)) -> Result<(), String>>;

/// What the steps need.
pub(crate) struct Tools {
    pub ffmpeg: PathBuf,
    pub settings: Settings,
    /// The Blur step (blur-cli for real; a stand-in in the self-test). None: a Blur step fails.
    pub blur: Option<BlurFn>,
}

/// What the steps made.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The result: a temporary file, or the source itself when no step made a file.
    pub file: PathBuf,
    /// Some step made a file (else every step was skipped).
    pub made: bool,
    /// The result is a GIF.
    pub gif: bool,
    /// The rule ends with Copy.
    pub copy: bool,
    /// Kinds that had nothing to do.
    pub skipped: Vec<String>,
    /// The GIF step's size limit could not be met (the smallest try was kept).
    pub over_limit: bool,
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// Run ffmpeg with `-progress pipe:1`; `on_frac` gets 0..1 of `expected` seconds.
/// Once `cancel` is set ffmpeg is killed within a tenth of a second and the result is `Err(CANCELLED)`.
fn run_ff(ffmpeg: &Path, args: &[String], expected: f64, cancel: &AtomicBool, on_frac: &mut dyn FnMut(f64)) -> Result<(), String> {
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
            Ok(secs) => {
                if expected > 0.0 {
                    on_frac((secs / expected).clamp(0.0, 1.0));
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

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// The color step: the editor's export of look `look` on the whole clip (same
/// filter, same encoder choice and fallback); sound copied with its track titles.
/// Ok(None) when the look changes nothing.
fn color_step(tools: &Tools, cur: &Path, out: &Path, look: &str, cancel: &AtomicBool, report: &mut dyn FnMut(f32)) -> Result<Option<PathBuf>, String> {
    let c = look_color(&tools.settings, look).ok_or_else(|| format!("the color look \"{look}\" no longer exists"))?;
    let Some(vf) = editor::look_filter(&tools.ffmpeg, cur, &c) else { return Ok(None) };
    ENCODES.fetch_add(1, Ordering::SeqCst);
    let duration = library::probe(&tools.ffmpeg, cur, false).map(|p| p.duration).unwrap_or(0.0);
    let titles = track_titles(&tools.ffmpeg, cur);
    let build = |enc: &str| -> Vec<String> {
        let mut a = sv(&["-hide_banner", "-loglevel", "error", "-nostdin", "-nostats", "-y", "-i"]);
        a.push(cur.to_string_lossy().into_owned());
        a.extend(sv(&["-map", "0:v:0", "-map", "0:a?", "-vf"]));
        a.push(vf.clone());
        a.extend(editor::video_codec_args(enc));
        a.extend(sv(&["-fps_mode", "vfr", "-force_key_frames", "expr:gte(t,n_forced*1)", "-c:a", "copy"]));
        // A stream copy into MP4 drops the track names: write them again (Mix, Desktop, Microphone, Game).
        for (i, title) in titles.iter().enumerate().filter(|(_, x)| !x.is_empty()) {
            a.push(format!("-metadata:s:a:{i}"));
            a.push(format!("title={title}"));
        }
        a.extend(sv(&["-movflags", "+faststart", "-progress", "pipe:1"]));
        a.push(out.to_string_lossy().into_owned());
        a
    };
    let enc = editor::pick_encoder(&tools.ffmpeg, &tools.settings.encoder);
    let mut res = run_ff(&tools.ffmpeg, &build(&enc), duration, cancel, &mut |f| report(f as f32 * 100.0));
    if res.is_err() && !cancel.load(Ordering::SeqCst) && enc != "x264" {
        // A busy hardware encoder (the recorder and a game hold its sessions): the CPU always works.
        util::log(&format!("rules: color step: {enc} failed ({}), retrying with libx264", res.as_ref().err().map(String::as_str).unwrap_or("")));
        let _ = fs::remove_file(out);
        res = run_ff(&tools.ffmpeg, &build("x264"), duration, cancel, &mut |f| report(f as f32 * 100.0));
    }
    if let Err(e) = res {
        let _ = fs::remove_file(out);
        return Err(e);
    }
    if !fs::metadata(out).is_ok_and(|m| m.len() > 0) {
        let _ = fs::remove_file(out);
        return Err("ffmpeg wrote no file".into());
    }
    Ok(Some(out.to_path_buf()))
}

/// Video encodes started by the color and canvas steps (the self-test counts them: Color with Canvas is one).
static ENCODES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The canvas step: the editor's export (`editor::export` with `ExportOpts::canvas`) of the whole
/// clip onto saved layout `step`, its pieces fitted to this clip's picture; with `look` (a Color
/// block in the same rule) the same export also applies that look. The file is written into the
/// temporary folder as `<stem> (edit).mp4` (a stand-in clip that is a "draft" whose clips folder
/// is the temporary folder: `clip::output_dir`). The export cannot be stopped part-way: on cancel
/// this returns at once and the export's thread removes its file when ffmpeg is done.
fn canvas_step(tools: &Tools, cur: &Path, stem: &str, step: &Step, look: Option<&str>, cancel: &AtomicBool, report: &mut dyn FnMut(f32)) -> Result<Option<PathBuf>, String> {
    let preset = find_layout(&tools.settings, step).ok_or_else(|| format!("the canvas layout \"{}\" no longer exists", step.layout_name))?;
    let color = match look {
        Some(l) => look_color(&tools.settings, l).ok_or_else(|| format!("the color look \"{l}\" no longer exists"))?,
        None => Color::default(),
    };
    let p = library::probe(&tools.ffmpeg, cur, false).filter(|p| p.width > 0 && p.height > 0 && p.duration > 0.0).ok_or("Could not read the clip's picture size")?;
    let canvas = canvas_for_clip(&preset.canvas, p.width, p.height);
    let tmp = util::tmp_dir();
    let meta = ClipMeta { path: cur.to_string_lossy().into_owned(), title: stem.to_string(), duration: p.duration, width: p.width, height: p.height, fps: p.fps, draft: true, ..Default::default() };
    let mut settings = tools.settings.clone();
    settings.clips_dir = tmp.to_string_lossy().into_owned();
    settings.ffmpeg_path = tools.ffmpeg.to_string_lossy().into_owned();
    let opts = editor::ExportOpts { color, canvas: Some(canvas), ..Default::default() };
    ENCODES.fetch_add(1, Ordering::SeqCst);
    let abandoned = Arc::new(AtomicBool::new(false));
    let gone = abandoned.clone();
    let (tx, rx) = mpsc::channel::<Result<f32, Result<ClipMeta, String>>>();
    thread::spawn(move || {
        let ptx = tx.clone();
        let res = editor::export(&settings, &meta, &opts, &mut |v| {
            let _ = ptx.send(Ok(v));
        });
        if gone.load(Ordering::SeqCst) {
            // Cancelled meanwhile: nobody takes the result.
            if let Ok(m) = &res {
                let _ = fs::remove_file(&m.path);
                let _ = fs::remove_file(&m.thumb);
            }
        }
        let _ = tx.send(Err(res));
    });
    let res = loop {
        if cancel.load(Ordering::SeqCst) {
            abandoned.store(true, Ordering::SeqCst);
            return Err(CANCELLED.into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(v)) => report(v),
            Ok(Err(res)) => break res,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err("the export stopped".into()),
        }
    };
    let m = res?;
    let _ = fs::remove_file(&m.thumb); // the stand-in clip's thumbnail: it never goes into the library
    let file = PathBuf::from(&m.path);
    if !fs::metadata(&file).is_ok_and(|x| x.len() > 0) {
        let _ = fs::remove_file(&file);
        return Err("ffmpeg wrote no file".into());
    }
    if file.parent() != Some(tmp.as_path()) {
        let _ = fs::remove_file(&file);
        return Err("the export went to the wrong folder".into());
    }
    // The export copies the sound, and a stream copy into MP4 drops the track names: write them again (as the color step does).
    keep_titles(&tools.ffmpeg, cur, &file);
    Ok(Some(file))
}

/// Give `out` the audio track titles of `src` again (a stream copy); left as it is when nothing is to be done or anything fails.
fn keep_titles(ffmpeg: &Path, src: &Path, out: &Path) {
    let want = track_titles(ffmpeg, src);
    let have = track_titles(ffmpeg, out);
    if want.iter().all(String::is_empty) || have == want || have.len() != want.len() {
        return;
    }
    let tmp = out.with_extension("titles.mp4");
    let mut a = sv(&["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]);
    a.push(out.to_string_lossy().into_owned());
    a.extend(sv(&["-map", "0", "-c", "copy"]));
    for (i, title) in want.iter().enumerate().filter(|(_, x)| !x.is_empty()) {
        a.push(format!("-metadata:s:a:{i}"));
        a.push(format!("title={title}"));
    }
    a.extend(sv(&["-movflags", "+faststart"]));
    a.push(tmp.to_string_lossy().into_owned());
    let ok = util::hidden_cmd(ffmpeg).args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !(ok && track_titles(ffmpeg, &tmp) == want && fs::rename(&tmp, out).is_ok()) {
        let _ = fs::remove_file(&tmp);
    }
}

/// Run `steps` on `src` in order. Every file made is `<tmp>\<tag>-…`; the caller
/// removes them all afterwards (`TmpGuard`) once it has taken the result.
/// `progress` gets 0..100 over all steps (never backwards), `stage` the kind of
/// each step as it starts.
pub(crate) fn run_steps(tools: &Tools, steps: &[Step], src: &Path, tag: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(f32), stage: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    let tmp = util::tmp_dir();
    // With a Canvas block the Color block's look goes into the canvas export (one encode, not two).
    let merged_look: Option<String> = steps.iter().any(|s| s.kind == "canvas").then(|| steps.iter().find(|s| s.kind == "color").map(|s| s.look.clone())).flatten();
    let merged = |st: &Step| st.kind == "color" && merged_look.is_some();
    let total: f32 = steps.iter().filter(|s| !merged(s)).map(|s| weight(&s.kind)).sum::<f32>().max(0.001);
    let mut base = 0f32;
    let mut top = 0f32;
    let mut cur = src.to_path_buf();
    let mut out = Outcome::default();
    for (i, st) in steps.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        if merged(st) {
            continue; // done by the canvas step
        }
        stage(&st.kind);
        let w = weight(&st.kind);
        let file = |ext: &str| tmp.join(format!("{tag}-{i}-{}.{ext}", st.kind));
        let res: Result<Option<PathBuf>, String> = {
            let mut report = |p: f32| {
                let v = ((base + w * p.clamp(0.0, 100.0) / 100.0) / total * 100.0).min(99.5);
                if v > top {
                    top = v;
                    progress(v);
                }
            };
            match st.kind.as_str() {
                "color" => color_step(tools, &cur, &file("mp4"), &st.look, cancel, &mut report),
                "canvas" => canvas_step(tools, &cur, &format!("{tag}-{i}-canvas"), st, merged_look.as_deref(), cancel, &mut report),
                "blur" => match &tools.blur {
                    None => Err(BLUR_MISSING.into()),
                    Some(run) => {
                        let o = file(&blur::container(&tools.settings.blur));
                        run(&cur, &o, cancel, &mut report).map(|_| {
                            // Blur drops the audio track titles; the editor needs them back (a clip with a Game track).
                            crate::clip::restore_titles(&tools.ffmpeg, &cur, &o);
                            Some(o)
                        })
                    }
                },
                "upscale" => {
                    let h = library::probe(&tools.ffmpeg, &cur, false).map(|p| p.height).unwrap_or(0);
                    if h > 0 && h >= st.upscale.height {
                        Ok(None) // already at least that tall
                    } else {
                        let o = file("mp4");
                        crate::upscale::run(&tools.ffmpeg, &cur, &o, &st.upscale, cancel, &mut report).map(|_| Some(o))
                    }
                }
                "compress" => {
                    let size = fs::metadata(&cur).map(|m| m.len()).unwrap_or(0);
                    if size > 0 && size <= compress::limit_bytes(st.mb) {
                        Ok(None) // already under the size
                    } else {
                        let opts = tools.settings.compress.opts();
                        compress::compress_with(&tools.ffmpeg, &cur, st.mb, &format!("{tag}-{i}-compress"), &opts, cancel, &mut report).map(|d| Some(d.file))
                    }
                }
                "gif" => {
                    let o = file("gif");
                    crate::gif::make(&tools.ffmpeg, &cur, &o, &st.gif, cancel, &mut report).map(|_| {
                        // With a size limit gif.rs keeps its smallest try even when nothing fit: say so in the result.
                        out.over_limit = crate::gif::over_limit(&st.gif, fs::metadata(&o).map(|m| m.len()).unwrap_or(0));
                        Some(o)
                    })
                }
                "copy" => {
                    out.copy = true;
                    Ok(None)
                }
                k => Err(format!("unknown step \"{k}\"")),
            }
        };
        match res {
            Ok(Some(p)) => {
                if cur != src {
                    let _ = fs::remove_file(&cur); // the step before's file is not needed any more
                }
                cur = p;
                out.made = true;
                out.gif = st.kind == "gif";
            }
            Ok(None) => {
                if st.kind != "copy" {
                    out.skipped.push(st.kind.clone());
                }
            }
            Err(e) => {
                if cancel.load(Ordering::SeqCst) {
                    return Err(CANCELLED.into());
                }
                return Err(format!("{}: {e}", kind_label(&st.kind)));
            }
        }
        base += w;
        let v = (base / total * 100.0).min(99.5);
        if v > top {
            top = v;
            progress(v);
        }
    }
    out.file = cur;
    progress(100.0);
    Ok(out)
}

/// Removes every temporary file and folder of one job (`<tmp>\<tag>*`) when dropped.
pub(crate) struct TmpGuard(pub String);

impl Drop for TmpGuard {
    fn drop(&mut self) {
        for p in tmp_files(&self.0) {
            if p.is_dir() {
                let _ = fs::remove_dir_all(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// The temporary files whose names start with `tag`.
fn tmp_files(tag: &str) -> Vec<PathBuf> {
    fs::read_dir(util::tmp_dir())
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(tag)).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Move a file, also across drives.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if let Err(e) = fs::copy(from, to) {
        let _ = fs::remove_file(to);
        return Err(format!("Could not save the result: {e}"));
    }
    let _ = fs::remove_file(from);
    Ok(())
}

/// Where a result of clip `m` goes, and its library folder: beside the clip; for
/// a clip imported in place (another program's file, outside the clips folder)
/// into its library folder inside the clips folder, else the clips folder itself.
pub(crate) fn out_dir(s: &Settings, m: &ClipMeta) -> (PathBuf, String) {
    let clips = PathBuf::from(&s.clips_dir);
    if m.draft || crate::core::in_clips_dir(&clips, &m.path) {
        return crate::clip::output_dir(s, m);
    }
    let d = clips.join(&m.folder);
    if !m.folder.is_empty() && d.is_dir() { (d, m.folder.clone()) } else { (clips, String::new()) }
}

/// Copying to the clipboard: only with a running app and never in a self-test.
fn may_copy(core: &Core) -> bool {
    core.app.get().is_some() && std::env::var_os("GLINT_NO_CLIPBOARD").is_none()
}

/// Keep the result of a finished run: a video into the library beside the clip,
/// a GIF beside the clip (not in the library). Sends "rule-done":
/// `{ job_id, source, rule, name, clip (new library clip or null), path, size, gif, copied, skipped, nothing, over_limit }`
/// (`nothing`: every step was skipped, no file was made). Returns the new clip's id ("" for a GIF or nothing).
fn finish(core: &Arc<Core>, tools: &Tools, m: &ClipMeta, rule: &Rule, done: &Outcome, job_id: &str) -> Result<String, String> {
    let s = &tools.settings;
    let mut path = done.file.clone();
    let mut clip: Option<ClipMeta> = None;
    if done.made {
        let (dir, folder) = out_dir(s, m);
        fs::create_dir_all(&dir).map_err(|e| format!("Could not use the folder {}: {e}", dir.display()))?;
        let title = format!("{} ({})", m.title, rule.name.trim());
        let ext = done.file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "mp4".into());
        path = util::unique_path(&dir, &util::sanitize_filename(&title), &ext);
        move_file(&done.file, &path)?;
        if !done.gif {
            let mut new = library::register(&tools.ffmpeg, &path, &title, &m.game, "rule");
            new.folder = folder;
            core.lib.lock().unwrap().add(new.clone());
            core.emit("clips-changed", new.id.clone());
            clip = Some(new);
        }
    }
    let copied = done.copy && may_copy(core) && {
        let p = path.to_string_lossy().into_owned();
        match crate::app::files_to_clipboard(&[p.as_str()]) {
            Ok(()) => true,
            Err(e) => {
                util::log(&format!("rules: could not copy to the clipboard: {e}"));
                false
            }
        }
    };
    let size = fs::metadata(&path).map(|x| x.len()).unwrap_or(0);
    util::log(&format!("rules: \"{}\" on {} -> {} ({}), skipped {:?}, copied={copied}", rule.name, m.title, path.display(), compress::fmt_size(size), done.skipped));
    core.emit(
        "rule-done",
        json!({ "job_id": job_id, "source": m.id, "rule": rule.id, "name": rule.name, "clip": clip, "path": path.to_string_lossy(), "size": size, "gif": done.gif, "copied": copied, "skipped": done.skipped, "nothing": !done.made, "over_limit": done.over_limit }),
    );
    Ok(clip.map(|c| c.id).unwrap_or_default())
}

/// Queue rule `rule` on clip `clip_id` (one job of kind "rule"). Refused before
/// queueing when the rule cannot run or Blur is needed and missing.
pub fn enqueue(core: &Arc<Core>, rule: &Rule, clip_id: String) -> Result<Job, String> {
    let meta = core.lib.lock().unwrap().get(&clip_id).ok_or("Clip not found")?;
    let s = core.settings();
    check(&s, rule)?;
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let blur_cli = if rule.steps.iter().any(|st| st.kind == "blur") { Some(blur::find(&s).ok_or(BLUR_MISSING)?) } else { None };
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let job_id: Arc<std::sync::OnceLock<String>> = Arc::default();
    let jid = job_id.clone();
    let rule = rule.clone();
    let m = meta.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let _working = crate::update::working(); // no automatic update restart meanwhile
        let core = weak.upgrade().ok_or("Glint is closing")?;
        let src = PathBuf::from(&m.path);
        if !src.is_file() {
            return Err("The clip's video file is missing".into());
        }
        let cfg = s.blur.clone();
        let blur: Option<BlurFn> = blur_cli.map(|cli| -> BlurFn { Box::new(move |i: &Path, o: &Path, c: &AtomicBool, p: &mut dyn FnMut(f32)| blur::render_staged(&cli, i, o, &cfg, c, &mut |_| {}, |x| p(x))) });
        let tools = Tools { ffmpeg, settings: s, blur };
        let tag = format!("rule-{}", util::new_id());
        let _guard = TmpGuard(tag.clone());
        let started = Instant::now();
        let done = run_steps(&tools, &rule.steps, &src, &tag, cancel, progress, &mut |k| core.jobs.stage(&m.id, "rule", k))?;
        let id = finish(&core, &tools, &m, &rule, &done, jid.get().map(String::as_str).unwrap_or(""))?;
        util::log(&format!("rules: done in {:.1} s", started.elapsed().as_secs_f64()));
        Ok(id)
    });
    let job = core.jobs.enqueue(&clip_id, &meta.title, "rule", run)?;
    let _ = job_id.set(job.id.clone());
    Ok(job)
}

/// `rules_run`: rule `rule_id` on every clip of `ids`, one job each. A clip that
/// is refused comes back as a job that was never queued (empty `id`, state
/// "error", `message` = why). Err when nothing could be queued.
pub fn run_ids(core: &Arc<Core>, rule_id: &str, ids: &[String]) -> Result<Vec<Job>, String> {
    let s = core.settings();
    let rule = s.rules.iter().find(|r| r.id == rule_id).cloned().ok_or("That render rule no longer exists")?;
    check(&s, &rule)?;
    if ids.is_empty() {
        return Err("No clip was picked".into());
    }
    if rule.steps.iter().any(|st| st.kind == "blur") && blur::find(&s).is_none() {
        return Err(BLUR_MISSING.into());
    }
    let mut out = Vec::new();
    for id in ids {
        match enqueue(core, &rule, id.clone()) {
            Ok(j) => out.push(j),
            Err(e) => out.push(Job { clip_id: id.clone(), kind: "rule".into(), state: "error".into(), message: e, ..Default::default() }),
        }
    }
    if out.iter().all(|j| j.id.is_empty()) {
        return Err(out[0].message.clone());
    }
    Ok(out)
}

type St<'a> = tauri::State<'a, Arc<Core>>;

/// Run render rule `rule` (its id) on clips `ids`: one job of kind "rule" each.
#[tauri::command(async)]
pub fn rules_run(core: St, rule: String, ids: Vec<String>) -> Result<Vec<Job>, String> {
    run_ids(core.inner(), &rule, &ids)
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    path: PathBuf,
    passed: u32,
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
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        let tag = if ok { "PASS" } else { "FAIL" };
        if detail.is_empty() || ok {
            self.line(&format!("{tag} {name}"));
        } else {
            self.line(&format!("{tag} {name} - {detail}"));
        }
    }
}

/// `glint.exe --rules-test <report.txt>`: headless checks of render rules. Blur is
/// a stand-in (blur-cli is never started), the clipboard is never touched.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), passed: 0, failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    std::env::set_var("GLINT_NO_CLIPBOARD", "1");
    pure_checks(&mut r);
    media_checks(&mut r);
    let (p, f) = (r.passed, r.failed);
    r.line(&format!("DONE passed={p} failed={f}"));
}

fn kinds(steps: &[Step]) -> Vec<&str> {
    steps.iter().map(|s| s.kind.as_str()).collect()
}

fn steps_of(list: &[&str]) -> Vec<Step> {
    list.iter().map(|k| Step::of(k)).collect()
}

fn pure_checks(r: &mut Report) {
    // Order rules.
    let ok_order = steps_of(&["color", "blur", "compress", "copy"]);
    let (f, why) = fix_order(&ok_order);
    r.check("order: a good order is kept, no reason", f == ok_order && why.is_none(), &format!("{:?} {why:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["gif", "color"]));
    r.check("order: a video step after a GIF moves before it", kinds(&f) == ["color", "gif"] && why == Some(WHY_GIF), &format!("{:?} {why:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["gif", "copy"]));
    r.check("order: Copy may follow a GIF", kinds(&f) == ["gif", "copy"] && why.is_none(), &format!("{:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["copy", "blur", "compress"]));
    r.check("order: Copy goes last", kinds(&f) == ["blur", "compress", "copy"] && why == Some(WHY_COPY), &format!("{:?} {why:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["blur", "color", "blur", "sparkle"]));
    r.check("order: a kind twice and an unknown kind are left out", kinds(&f) == ["blur", "color"] && why == Some(WHY_ONCE), &format!("{:?} {why:?}", kinds(&f)));
    let (f, _) = fix_order(&steps_of(&["copy", "gif", "upscale", "compress"]));
    r.check("order: copy, gif, upscale, compress -> upscale, compress, gif, copy", kinds(&f) == ["upscale", "compress", "gif", "copy"], &format!("{:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["blur", "canvas"]));
    r.check("order: Canvas moves before Blur", kinds(&f) == ["canvas", "blur"] && why == Some(WHY_CANVAS), &format!("{:?} {why:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["blur", "color", "compress", "canvas", "copy"]));
    r.check("order: Canvas and Color go first together, Copy stays last", kinds(&f) == ["color", "canvas", "blur", "compress", "copy"] && why == Some(WHY_CANVAS), &format!("{:?} {why:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["canvas", "color", "upscale", "gif"]));
    r.check("order: Canvas then Color is kept as it is", kinds(&f) == ["canvas", "color", "upscale", "gif"] && why.is_none(), &format!("{:?} {why:?}", kinds(&f)));
    let (f, _) = fix_order(&steps_of(&["gif", "canvas", "copy"]));
    r.check("order: Canvas after a GIF moves before it", kinds(&f) == ["canvas", "gif", "copy"], &format!("{:?}", kinds(&f)));
    let (f, why) = fix_order(&steps_of(&["blur", "color"]));
    r.check("order: without Canvas, Color may still come after Blur", kinds(&f) == ["blur", "color"] && why.is_none(), &format!("{:?}", kinds(&f)));
    let mut s = Settings::default();
    s.canvas_presets.push(CanvasPreset { id: "L1".into(), name: "Vertical".into(), canvas: Canvas::default() });
    let with_layout = |mut v: Vec<Step>| -> Vec<Step> {
        for st in v.iter_mut().filter(|x| x.kind == "canvas") {
            st.layout = "L1".into();
            st.layout_name = "Vertical".into();
        }
        v
    };
    let all_fixed = KINDS;
    let mut perm_ok = true;
    // Every order of every pair and triple ends in a form `check` accepts.
    for a in all_fixed {
        for b in all_fixed {
            for c in all_fixed {
                let (f, _) = fix_order(&with_layout(steps_of(&[a, b, c])));
                let rule = Rule { id: "x".into(), name: "x".into(), steps: f.clone() };
                let gif_ok = f.iter().position(|s| s.kind == "gif").map_or(true, |g| f[g + 1..].iter().all(|s| s.kind == "copy"));
                let copy_ok = f.iter().position(|s| s.kind == "copy").map_or(true, |i| i + 1 == f.len());
                if check(&s, &rule).is_err() || !gif_ok || !copy_ok || fix_order(&f).1.is_some() {
                    perm_ok = false;
                }
            }
        }
    }
    r.check("order: every fixed order of 3 blocks is valid and stays as it is", perm_ok, "");

    // Words.
    let rule = Rule { id: "r1".into(), name: "Render rule 1".into(), steps: vec![Step { kind: "color".into(), look: "Vivid".into(), ..Default::default() }, Step::of("blur"), Step { kind: "compress".into(), mb: 50, ..Default::default() }, Step::of("copy")] };
    let w = describe(&rule);
    r.check("words: Color: Vivid → Blur → Compress to 50 MB → Copy", w == "Color: Vivid → Blur → Compress to 50 MB → Copy", &w);
    let up = Step { kind: "upscale".into(), upscale: UpscaleOpts { height: 2160, ..Default::default() }, ..Default::default() };
    let up2 = Step { kind: "upscale".into(), upscale: UpscaleOpts { height: 1440, ..Default::default() }, ..Default::default() };
    let g = Step { kind: "gif".into(), gif: GifOpts { width: 480, fps: 15, max_mb: 10, ..Default::default() }, ..Default::default() };
    let w2 = [step_words(&up), step_words(&up2), step_words(&g)];
    r.check("words: Upscale to 4K, Upscale to 1440p, GIF 480 px, 15 fps, up to 10 MB", w2 == ["Upscale to 4K", "Upscale to 1440p", "GIF 480 px, 15 fps, up to 10 MB"], &format!("{w2:?}"));
    r.check("words: an empty rule", describe(&Rule::default()) == "No blocks yet", "");

    // Checks.
    r.check("check: an empty rule is refused", check(&s, &Rule::default()).err().as_deref() == Some(NO_STEPS), "");
    let bad = Rule { steps: steps_of(&["gif", "blur"]), ..Default::default() };
    r.check("check: an order that cannot work is refused with its reason", check(&s, &bad).err().as_deref() == Some(WHY_GIF), "");
    let gone = Rule { steps: vec![Step { kind: "color".into(), look: "Not a look".into(), ..Default::default() }], ..Default::default() };
    r.check("check: a color look that no longer exists is refused", check(&s, &gone).is_err_and(|e| e.contains("Not a look")), "");
    let tiny = Rule { steps: vec![Step { kind: "compress".into(), mb: 2, ..Default::default() }], ..Default::default() };
    r.check("check: a size under 5 MB is refused", check(&s, &tiny).is_err(), "");
    let mut s2 = Settings::default();
    s2.color_presets.push(crate::settings::ColorPreset { name: "Mine".into(), brightness: 1.2, contrast: 1.0, saturation: 1.0, gamma: 1.0 });
    let mine = look_color(&s2, "Mine");
    r.check("looks: the user's own preset is found by name", mine.as_ref().is_some_and(|c| (c.brightness - 1.2).abs() < 1e-6), "");
    r.check("looks: a built-in look is found by name", look_color(&s, "Black and white").is_some_and(|c| c.saturation == 0.0 && (c.contrast - 1.12).abs() < 1e-6), "");

    canvas_pure_checks(r, &s);

    // The built-in looks are the editor's (ui/editor.js ED_PRESETS), read from the source tree when it is there.
    let js = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/editor.js")).unwrap_or_default();
    if js.is_empty() {
        r.line("INFO ui/editor.js not found: the built-in looks were not compared");
    } else {
        let mut same = true;
        let mut seen = 0;
        for line in js.lines().filter(|l| l.trim_start().starts_with("{ name: \"")) {
            let name = line.split('"').nth(1).unwrap_or("");
            let num = |key: &str| -> f32 {
                if line.contains("...ED_NO_COLOR") {
                    return 1.0;
                }
                line.split(&format!("{key}: ")).nth(1).and_then(|v| v.split([',', ' ', '}']).next()).and_then(|v| v.parse().ok()).unwrap_or(f32::NAN)
            };
            let Some((_, v)) = BUILTIN_LOOKS.iter().find(|(n, _)| *n == name) else {
                same = false;
                continue;
            };
            seen += 1;
            let js_v = [num("brightness"), num("contrast"), num("saturation"), num("gamma")];
            if js_v.iter().zip(v).any(|(a, b)| (a - b).abs() > 1e-6) {
                same = false;
            }
        }
        r.check("looks: the built-in looks have the editor's names and numbers", same && seen == BUILTIN_LOOKS.len(), &format!("{seen} of {} matched", BUILTIN_LOOKS.len()));
    }

    // Saved form.
    let back: Result<Rule, _> = serde_json::from_str(&serde_json::to_string(&rule).unwrap_or_default());
    r.check("settings: a rule round-trips through JSON", back.as_ref().is_ok_and(|b| *b == rule), "");
    let old: Result<Settings, _> = serde_json::from_str(r#"{"rules":[{"id":"a","name":"Old"},{"id":"b","name":"N","steps":[{"kind":"compress","mb":25}]}]}"#);
    let ok = old.as_ref().is_ok_and(|o| o.rules.len() == 2 && o.rules[0].steps.is_empty() && o.rules[1].steps[0].mb == 25 && o.rules[1].steps[0].look == "Vivid" && o.rules_after_export.is_empty());
    r.check("settings: rules with missing fields load with defaults", ok, &format!("{:?}", old.err()));
}

fn piece(v: [f64; 8]) -> CanvasPiece {
    CanvasPiece { sx: v[0], sy: v[1], sw: v[2], sh: v[3], x: v[4], y: v[5], w: v[6], h: v[7], ..Default::default() }
}

fn canvas_of(w: f64, h: f64, bg: &str, pieces: &[[f64; 8]]) -> Canvas {
    Canvas { width: w, height: h, background: bg.into(), pieces: pieces.iter().map(|p| piece(*p)).collect(), ..Default::default() }
}

/// A Canvas block that names layout `id` / `name`.
fn canvas_step_of(id: &str, name: &str) -> Step {
    Step { kind: "canvas".into(), layout: id.into(), layout_name: name.into(), ..Default::default() }
}

/// The Canvas block's words, checks and the layout maths (cvNorm + cvRefit of ui/canvas.js).
fn canvas_pure_checks(r: &mut Report, s: &Settings) {
    let rule = Rule { steps: vec![canvas_step_of("L1", "Vertical"), Step::of("copy")], ..Default::default() };
    r.check("words: Canvas: Vertical → Copy", describe(&rule) == "Canvas: Vertical → Copy", &describe(&rule));
    r.check("check: a rule with a Canvas block whose layout exists runs", check(s, &rule).is_ok(), &format!("{:?}", check(s, &rule)));
    let renamed = Rule { steps: vec![canvas_step_of("old-id", "Vertical")], ..Default::default() };
    r.check("check: a layout saved again under its name (new id) is still found", check(s, &renamed).is_ok() && find_layout(s, &renamed.steps[0]).is_some_and(|p| p.id == "L1"), "");
    let gone = Rule { steps: vec![canvas_step_of("L9", "Deleted one")], ..Default::default() };
    let e = check(s, &gone);
    r.check("check: a deleted layout is refused in plain words", e.as_ref().is_err_and(|e| e.contains("\"Deleted one\" no longer exists")), &format!("{e:?}"));
    let none = Rule { steps: vec![Step::of("canvas")], ..Default::default() };
    r.check("check: a Canvas block with no layout picked is refused", check(s, &none).err().as_deref() == Some(NO_LAYOUT), "");

    // cvNorm + cvRefit (canvas.js `applyPreset`), expected numbers from node running ui/canvas.js.
    #[allow(clippy::type_complexity)]
    let cases: [(Canvas, (u32, u32), (f64, f64, &str), Vec<[f64; 8]>); 4] = [
        (canvas_of(1080.0, 1920.0, "blur", &[[0.341796875, 0.0, 0.31640625, 1.0, 0.0, 0.0, 1.0, 1.0]]), (1280, 1024), (1080.0, 1920.0, "blur"), vec![[0.3113317643984552, 0.08073725421878941, 0.3773364712030895, 0.8385254915624212, 0.0, 0.0, 1.0, 1.0]]),
        (
            canvas_of(1080.0, 1920.0, "#112233", &[[0.64, 0.0, 0.36, 0.2667, 0.0, 0.0, 1.0, 0.24], [0.0, 0.7, 0.3, 0.3, 0.0, 0.74, 1.0, 0.26]]),
            (2560, 1080),
            (1080.0, 1920.0, "#112233"),
            vec![[0.6918868061338901, 0.0, 0.3081131938661099, 0.3116127511297743, 0.0, 0.0, 1.0, 0.24], [0.0, 0.6859818358638987, 0.28660762426785075, 0.31401816413610123, 0.0, 0.74, 1.0, 0.26]],
        ),
        (canvas_of(1081.0, 1079.0, "nope", &[[0.1, 0.2, 0.5, 0.4, -0.1, 0.25, 1.2, 0.5]]), (1280, 720), (1082.0, 1080.0, "black"), vec![[0.09598077100843327, 0.20515190512437867, 0.5200961449578336, 0.3845442846268641, -0.1, 0.25, 1.2, 0.5]]),
        (canvas_of(1080.0, 1920.0, "black", &[[0.0, 0.0, 1.0, 1.0, 0.0, 0.25, 1.0, 0.5]]), (1440, 1080), (1080.0, 1920.0, "black"), vec![[0.07812500000000006, 0.0, 0.8437499999999999, 1.0, 0.0, 0.25, 1.0, 0.5]]),
    ];
    for (k, (c, (w, h), (cw, ch, bg), want)) in cases.iter().enumerate() {
        let got = canvas_for_clip(c, *w, *h);
        let mut worst = 0f64;
        for (p, e) in got.pieces.iter().zip(want) {
            for (a, b) in [p.sx, p.sy, p.sw, p.sh, p.x, p.y, p.w, p.h].iter().zip(e) {
                worst = worst.max((a - b).abs());
            }
        }
        let ok = got.width == *cw && got.height == *ch && got.background == *bg && got.pieces.len() == want.len() && worst < 1e-12;
        r.check(&format!("layout maths: case {} gives canvas.js's numbers ({w}x{h} clip)", k + 1), ok, &format!("{}x{} {} worst {worst:e}", got.width, got.height, got.background));
    }
    // On a clip of another shape every piece shows a box of its place's pixel shape (never stretched).
    let three = canvas_of(1080.0, 1920.0, "blur", &[[0.64, 0.0, 0.36, 0.2, 0.0, 0.0, 1.0, 0.24], [0.2, 0.0, 0.6, 1.0, 0.0, 0.24, 1.0, 0.5], [0.0, 0.8, 0.3, 0.2, 0.0, 0.74, 1.0, 0.26]]);
    let mut stretch = 0f64;
    for (w, h) in [(1920u32, 1080u32), (1440, 1080), (2560, 1080), (1080, 1920)] {
        let got = canvas_for_clip(&three, w, h);
        for p in &got.pieces {
            let place = (p.w * got.width) / (p.h * got.height);
            let source = (p.sw * w as f64) / (p.sh * h as f64);
            let inside = p.sx >= -1e-9 && p.sy >= -1e-9 && p.sx + p.sw <= 1.0 + 1e-9 && p.sy + p.sh <= 1.0 + 1e-9;
            stretch = stretch.max(if inside { (place / source - 1.0).abs() } else { 1.0 });
        }
    }
    r.check("layout maths: on 16:9, 4:3, 21:9 and 9:16 clips no piece is stretched and every box is inside the picture", stretch < 1e-9, &format!("{stretch:e}"));
    let old: Result<Rule, _> = serde_json::from_str(r#"{"id":"a","name":"Old","steps":[{"kind":"color","look":"Soft"},{"kind":"compress","mb":25}]}"#);
    r.check("settings: a rule saved before the Canvas block loads (no layout fields)", old.as_ref().is_ok_and(|o| o.steps.len() == 2 && o.steps.iter().all(|st| st.layout.is_empty() && st.layout_name.is_empty()) && fix_order(&o.steps).1.is_none()), &format!("{:?}", old.err()));
}

/// A test picture (`noise` makes it big, as a real clip is) with one tone per title, the tracks titled.
fn make_clip(ffmpeg: &Path, out: &Path, w: u32, h: u32, fps: u32, secs: u32, titles: &[&str], noise: bool) -> Result<(), String> {
    let mut a = sv(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"]);
    a.push(format!("testsrc2=s={w}x{h}:r={fps}:d={secs}"));
    for (k, _) in titles.iter().enumerate() {
        a.extend(sv(&["-f", "lavfi", "-i"]));
        a.push(format!("sine=frequency={}:sample_rate=48000:duration={secs}", 300 + 400 * k));
    }
    a.extend(sv(&["-map", "0:v"]));
    for k in 0..titles.len() {
        a.push("-map".into());
        a.push(format!("{}:a", k + 1));
    }
    if noise {
        a.extend(sv(&["-vf", "noise=alls=16:allf=t"]));
    }
    a.extend(sv(&["-c:v", "libx264", "-preset", "ultrafast", "-crf", if noise { "12" } else { "18" }, "-pix_fmt", "yuv420p", "-g"]));
    a.push(fps.to_string());
    if !titles.is_empty() {
        a.extend(sv(&["-c:a", "aac", "-b:a", "128k", "-ac", "2"]));
        for (k, t) in titles.iter().enumerate() {
            a.push(format!("-metadata:s:a:{k}"));
            a.push(format!("title={t}"));
        }
    }
    a.push(out.to_string_lossy().into_owned());
    let o = util::hidden_cmd(ffmpeg).args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(util::last_lines(&o.stderr, 2)) }
}

/// The audio track titles of `file` (title or name tag), in order.
fn track_titles(ffmpeg: &Path, file: &Path) -> Vec<String> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    let Ok(o) = util::hidden_cmd(&ffprobe).args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index:stream_tags=title,name", "-of", "default=nw=1"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output() else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if line.starts_with("index=") {
            out.push(String::new());
        } else if let Some(v) = line.strip_prefix("TAG:title=").or_else(|| line.strip_prefix("TAG:name=")) {
            if let Some(last) = out.last_mut() {
                if last.is_empty() {
                    *last = v.trim().to_string();
                }
            }
        }
    }
    out
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

/// One frame at `at` seconds, 64x36 RGB.
fn frame_rgb(ffmpeg: &Path, file: &Path, at: f64) -> Vec<u8> {
    util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{at:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", "scale=64:36:flags=area", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default()
}

/// Mean difference of two frames, 0..255 (255 when they cannot be compared).
fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 255.0;
    }
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

/// A Blur stand-in for the self-test: a light box blur with ffmpeg, stoppable like blur-cli.
fn stand_in_blur(ffmpeg: PathBuf, calls: Arc<std::sync::atomic::AtomicU32>) -> BlurFn {
    Box::new(move |i: &Path, o: &Path, c: &AtomicBool, p: &mut dyn FnMut(f32)| {
        calls.fetch_add(1, Ordering::SeqCst);
        let d = library::probe(&ffmpeg, i, false).map(|x| x.duration).unwrap_or(0.0);
        let mut a = sv(&["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]);
        a.push(i.to_string_lossy().into_owned());
        a.extend(sv(&["-map", "0:v:0", "-map", "0:a?", "-vf", "boxblur=2:1", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "16", "-c:a", "aac", "-progress", "pipe:1"]));
        a.push(o.to_string_lossy().into_owned());
        let res = run_ff(&ffmpeg, &a, d, c, &mut |f| p(f as f32 * 100.0));
        if res.is_err() {
            let _ = fs::remove_file(o);
        }
        res
    })
}

fn wait_until(secs: f64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    cond()
}

fn media_checks(r: &mut Report) {
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let root = util::data_dir().join("clips");
    let _ = fs::remove_dir_all(&root);
    let dir = root.join("Test Game");
    let _ = fs::create_dir_all(&dir);
    for p in tmp_files("rule-") {
        let _ = fs::remove_file(p);
    }
    let titles = ["Mix", "Desktop"];
    let small = dir.join("small.mp4");
    let big = dir.join("big.mp4");
    let built = make_clip(&ffmpeg, &small, 640, 360, 30, 4, &titles, false).and_then(|_| make_clip(&ffmpeg, &big, 1280, 720, 30, 8, &titles, true));
    r.check("build test clips (4 s 360p, 8 s 720p with noise), two titled tracks", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    if built.is_err() {
        return;
    }
    let big_size = fs::metadata(&big).map(|m| m.len()).unwrap_or(0);
    r.line(&format!("INFO big clip {}", compress::fmt_size(big_size)));
    let mut s = Settings::default();
    s.clips_dir = root.to_string_lossy().into_owned();
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let tools = Tools { ffmpeg: ffmpeg.clone(), settings: s.clone(), blur: Some(stand_in_blur(ffmpeg.clone(), calls.clone())) };
    let no_cancel = AtomicBool::new(false);
    let vivid = Step { kind: "color".into(), look: "Vivid".into(), ..Default::default() };

    // 1. Color alone: the same picture as the editor's export of that look; sound and titles kept.
    {
        let tag = "rule-test-color";
        let _g = TmpGuard(tag.into());
        let res = run_steps(&tools, &[vivid.clone()], &small, tag, &no_cancel, &mut |_| {}, &mut |_| {});
        r.check("color step runs", res.is_ok(), res.as_ref().err().map(String::as_str).unwrap_or(""));
        if let Ok(o) = &res {
            let meta = library::register(&ffmpeg, &small, "small", "Test Game", "clip");
            let opts = editor::ExportOpts { color: look_color(&s, "Vivid").unwrap_or_default(), ..Default::default() };
            match editor::export(&s, &meta, &opts, &mut |_| {}) {
                Ok(ed) => {
                    let (a, b, src) = (frame_rgb(&ffmpeg, &o.file, 2.0), frame_rgb(&ffmpeg, Path::new(&ed.path), 2.0), frame_rgb(&ffmpeg, &small, 2.0));
                    let (same, changed) = (mean_diff(&a, &b), mean_diff(&a, &src));
                    r.line(&format!("INFO color step vs editor export: mean difference {same:.2}; vs the source {changed:.2} (0..255)"));
                    r.check("color step looks the same as the editor's export of the look", same < 2.0, &format!("{same:.2}"));
                    r.check("color step changed the picture", changed > 1.0, &format!("{changed:.2}"));
                    let _ = fs::remove_file(&ed.path);
                    let _ = fs::remove_file(&ed.thumb);
                }
                Err(e) => r.check("editor export of the same look", false, &e),
            }
            let p = library::probe(&ffmpeg, &o.file, false).unwrap_or_default();
            r.check("color step keeps both audio tracks and their titles", p.audio_streams == 2 && track_titles(&ffmpeg, &o.file) == titles, &format!("{} tracks {:?}", p.audio_streams, track_titles(&ffmpeg, &o.file)));
            let sp = library::probe(&ffmpeg, &small, false).unwrap_or_default();
            r.check("color step keeps the length", (p.duration - sp.duration).abs() < 0.2, &format!("{:.2} vs {:.2}", p.duration, sp.duration));
        }
        let none = Step { kind: "color".into(), look: "None".into(), ..Default::default() };
        let res = run_steps(&tools, &[none], &small, tag, &no_cancel, &mut |_| {}, &mut |_| {});
        r.check("color step with the look None is skipped", res.as_ref().is_ok_and(|o| !o.made && o.skipped == ["color"] && o.file == small), "");
    }
    r.check("color: no temporary files left", tmp_files("rule-test-color").is_empty(), "");

    canvas_media_checks(r, &ffmpeg, &s, &dir, &small, &big, &titles);

    // 2. Color → Blur (stand-in) → Compress → Copy on the big clip: progress, stages, size.
    if big_size > compress::limit_bytes(5) {
        let tag = "rule-test-chain";
        let steps = vec![vivid.clone(), Step::of("blur"), Step { kind: "compress".into(), mb: 5, ..Default::default() }, Step::of("copy")];
        let mut pcts: Vec<f32> = Vec::new();
        let mut stages: Vec<String> = Vec::new();
        let t0 = Instant::now();
        let res = {
            let _g = TmpGuard(tag.into());
            let res = run_steps(&tools, &steps, &big, tag, &no_cancel, &mut |p| pcts.push(p), &mut |k| stages.push(k.to_string()));
            if let Ok(o) = &res {
                let size = fs::metadata(&o.file).map(|m| m.len()).unwrap_or(u64::MAX);
                r.line(&format!("INFO color + blur stand-in + compress to 5 MB: {} in {:.1} s", compress::fmt_size(size), t0.elapsed().as_secs_f64()));
                r.check("chain: the result is at or under 5 MB", size <= compress::limit_bytes(5), &compress::fmt_size(size));
                r.check("chain: the result is an MP4 with sound", format_name(&ffmpeg, &o.file).contains("mp4") && library::probe(&ffmpeg, &o.file, false).is_some_and(|p| p.audio_streams == 1 && p.video_streams == 1), "");
                r.check("chain: ends with Copy, nothing skipped", o.copy && o.made && !o.gif && o.skipped.is_empty(), &format!("{o:?}"));
                let left: Vec<PathBuf> = tmp_files(tag).into_iter().filter(|p| *p != o.file).collect();
                r.check("chain: only the result is left before it is taken", left.is_empty(), &format!("{left:?}"));
            }
            res
        };
        r.check("chain: color, blur stand-in, compress, copy succeed", res.is_ok(), res.as_ref().err().map(String::as_str).unwrap_or(""));
        r.check("chain: the Blur step ran (stand-in, blur-cli never started)", calls.load(Ordering::SeqCst) == 1, "");
        r.check("chain: stages in order", stages == ["color", "blur", "compress", "copy"], &format!("{stages:?}"));
        r.check("chain: progress never goes back and ends at 100", pcts.windows(2).all(|w| w[1] >= w[0]) && pcts.last() == Some(&100.0), "");
        r.check("chain: progress moves inside each step", pcts.iter().any(|&p| p > 2.0 && p < 15.0) && pcts.iter().any(|&p| p > 20.0 && p < 60.0) && pcts.iter().any(|&p| p > 70.0 && p < 99.0), &format!("{} updates", pcts.len()));
        r.check("chain: temporary files are gone afterwards", tmp_files(tag).is_empty(), &format!("{:?}", tmp_files(tag)));
    } else {
        r.line(&format!("INFO the big clip is only {}: the compress chain was not run", compress::fmt_size(big_size)));
    }

    // 3. Skipped steps: already under the size, already taller than the upscale target.
    {
        let tag = "rule-test-skip";
        let _g = TmpGuard(tag.into());
        let steps = vec![Step { kind: "upscale".into(), upscale: UpscaleOpts { height: 360, ..Default::default() }, ..Default::default() }, Step { kind: "compress".into(), mb: 500, ..Default::default() }];
        let res = run_steps(&tools, &steps, &small, tag, &no_cancel, &mut |_| {}, &mut |_| {});
        r.check("skip: upscale to its own height and compress to 500 MB are both skipped", res.as_ref().is_ok_and(|o| !o.made && o.skipped == ["upscale", "compress"] && o.file == small), &format!("{res:?}"));
    }

    // 4. Cancel: in the color step, in the Blur step, in the compress step.
    for (what, steps, at) in [
        ("color", vec![vivid.clone(), Step { kind: "compress".into(), mb: 5, ..Default::default() }], 5.0f32),
        ("blur", vec![Step::of("blur"), Step { kind: "compress".into(), mb: 5, ..Default::default() }], 20.0),
        ("compress", vec![vivid.clone(), Step { kind: "compress".into(), mb: 5, ..Default::default() }], 60.0),
    ] {
        let tag = format!("rule-test-cancel-{what}");
        let cancel = Arc::new(AtomicBool::new(false));
        let set_at: Arc<std::sync::Mutex<Option<Instant>>> = Arc::default();
        let (c, sa) = (cancel.clone(), set_at.clone());
        let mut on = move |p: f32| {
            if p > at && !c.swap(true, Ordering::SeqCst) {
                *sa.lock().unwrap() = Some(Instant::now());
            }
        };
        let mut stage_at = String::new();
        let res = {
            let _g = TmpGuard(tag.clone());
            let c2 = cancel.clone();
            run_steps(&tools, &steps, &big, &tag, &c2, &mut on, &mut |k| {
                if !c2.load(Ordering::SeqCst) {
                    stage_at = k.to_string();
                }
            })
        };
        let took = set_at.lock().unwrap().map(|x| x.elapsed());
        r.check(&format!("cancel in the {what} step returns Cancelled"), res.as_ref().err().map(String::as_str) == Some(CANCELLED), &format!("{:?}", res.as_ref().map(|o| o.file.clone())));
        r.check(&format!("cancel in the {what} step stops within a second"), took.is_some_and(|d| d < Duration::from_millis(1500)), &format!("{took:?}, stage {stage_at}"));
        r.check(&format!("cancel in the {what} step leaves no temporary file"), tmp_files(&tag).is_empty(), &format!("{:?}", tmp_files(&tag)));
    }

    // 5. GIF and upscale (agents B and C build these; their stubs say "not built yet").
    {
        let tag = "rule-test-gif";
        let _g = TmpGuard(tag.into());
        let steps = vec![vivid.clone(), Step { kind: "gif".into(), gif: GifOpts { width: 320, fps: 10, max_mb: 0, ..Default::default() }, ..Default::default() }, Step::of("copy")];
        match run_steps(&tools, &steps, &small, tag, &no_cancel, &mut |_| {}, &mut |_| {}) {
            Err(e) if e.contains("not built yet") => r.line(&format!("INFO gif step: {e}")),
            Err(e) => r.check("gif step runs", false, &e),
            Ok(o) => {
                let p = library::probe(&ffmpeg, &o.file, false).unwrap_or_default();
                r.check("gif step makes a GIF 320 px wide, the result is a GIF", o.gif && o.copy && format_name(&ffmpeg, &o.file) == "gif" && p.width == 320, &format!("{} {}x{}", format_name(&ffmpeg, &o.file), p.width, p.height));
            }
        }
    }
    {
        let tag = "rule-test-up";
        let _g = TmpGuard(tag.into());
        let steps = vec![Step { kind: "upscale".into(), upscale: UpscaleOpts { height: 720, ..Default::default() }, ..Default::default() }];
        match run_steps(&tools, &steps, &small, tag, &no_cancel, &mut |_| {}, &mut |_| {}) {
            Err(e) if e.contains("not built yet") => r.line(&format!("INFO upscale step: {e}")),
            Err(e) => r.check("upscale step runs", false, &e),
            Ok(o) => {
                let p = library::probe(&ffmpeg, &o.file, false).unwrap_or_default();
                r.check("upscale step makes the clip 720 px tall with its sound", o.made && p.height == 720 && p.audio_streams == 2, &format!("{}x{} {} tracks", p.width, p.height, p.audio_streams));
            }
        }
    }
    r.check("no temporary rule files left at all", tmp_files("rule-").is_empty(), &format!("{:?}", tmp_files("rule-")));

    queue_checks(r, &ffmpeg, &root, &dir, &small, &big);
}

/// The Canvas block on real (synthetic) clips: the result has the canvas's size, with Color it is
/// still one export, a deleted layout is refused, nothing is left behind.
fn canvas_media_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path, small: &Path, big: &Path, titles: &[&str]) {
    let mut s = s.clone();
    // Made on a 16:9 clip: the gameplay in the middle, a strip of the top right corner above it.
    let layout = canvas_of(360.0, 640.0, "blur", &[[0.64, 0.0, 0.36, 0.1215, 0.0, 0.0, 1.0, 0.24], [0.2, 0.0, 0.6, 1.0, 0.0, 0.24, 1.0, 0.5]]);
    s.canvas_presets.push(CanvasPreset { id: "LV".into(), name: "Test vertical".into(), canvas: layout });
    let tools = Tools { ffmpeg: ffmpeg.to_path_buf(), settings: s.clone(), blur: None };
    let no_cancel = AtomicBool::new(false);
    let square = dir.join("square.mp4");
    let built = make_clip(ffmpeg, &square, 480, 360, 30, 3, titles, false);
    r.check("canvas: build a 4:3 test clip", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    let cv = canvas_step_of("LV", "Test vertical");
    let vivid = Step { kind: "color".into(), look: "Vivid".into(), ..Default::default() };
    let tag = "rule-test-canvas";
    let _g = TmpGuard(tag.into());
    let mut alone: Option<PathBuf> = None;
    for (what, clip) in [("16:9", small), ("4:3", square.as_path())] {
        if !clip.is_file() {
            continue;
        }
        let before = ENCODES.load(Ordering::SeqCst);
        let mut stages: Vec<String> = Vec::new();
        let res = run_steps(&tools, &[cv.clone()], clip, &format!("{tag}-{}", what.replace(':', "x")), &no_cancel, &mut |_| {}, &mut |k| stages.push(k.into()));
        match &res {
            Ok(o) => {
                let p = library::probe(ffmpeg, &o.file, false).unwrap_or_default();
                let sp = library::probe(ffmpeg, clip, false).unwrap_or_default();
                r.check(&format!("canvas on a {what} clip: the result is 360 x 640, one export"), o.made && p.width == 360 && p.height == 640 && ENCODES.load(Ordering::SeqCst) - before == 1 && stages == ["canvas"], &format!("{}x{} encodes {} stages {stages:?}", p.width, p.height, ENCODES.load(Ordering::SeqCst) - before));
                r.check(&format!("canvas on a {what} clip: length, both tracks and their titles kept"), (p.duration - sp.duration).abs() < 0.2 && p.audio_streams == 2 && track_titles(ffmpeg, &o.file) == titles, &format!("{:.2} vs {:.2}, {} tracks {:?}", p.duration, sp.duration, p.audio_streams, track_titles(ffmpeg, &o.file)));
                r.check(&format!("canvas on a {what} clip: written in the temporary folder, nothing beside the clip"), o.file.parent() == Some(util::tmp_dir().as_path()) && !fs::read_dir(dir).map(|d| d.filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().contains("(edit)"))).unwrap_or(true), &o.file.to_string_lossy());
                if what == "16:9" {
                    alone = Some(o.file.clone());
                }
            }
            Err(e) => r.check(&format!("canvas on a {what} clip runs"), false, e),
        }
    }
    // Color and Canvas: one export, the look is in it.
    let before = ENCODES.load(Ordering::SeqCst);
    let mut stages: Vec<String> = Vec::new();
    let mut pcts: Vec<f32> = Vec::new();
    let res = run_steps(&tools, &[vivid.clone(), cv.clone()], small, &format!("{tag}-color"), &no_cancel, &mut |p| pcts.push(p), &mut |k| stages.push(k.into()));
    match &res {
        Ok(o) => {
            let p = library::probe(ffmpeg, &o.file, false).unwrap_or_default();
            let n = ENCODES.load(Ordering::SeqCst) - before;
            r.check("color + canvas: one export, 360 x 640, the stage is canvas", n == 1 && p.width == 360 && p.height == 640 && stages == ["canvas"] && o.skipped.is_empty(), &format!("encodes {n} {}x{} stages {stages:?} skipped {:?}", p.width, p.height, o.skipped));
            let d = alone.as_ref().map(|a| mean_diff(&frame_rgb(ffmpeg, a, 1.5), &frame_rgb(ffmpeg, &o.file, 1.5))).unwrap_or(0.0);
            r.line(&format!("INFO color + canvas vs canvas alone: mean difference {d:.2} (0..255)"));
            r.check("color + canvas: the look is applied (differs from the canvas alone)", d > 1.0, &format!("{d:.2}"));
            r.check("color + canvas: progress never goes back and ends at 100", pcts.windows(2).all(|w| w[1] >= w[0]) && pcts.last() == Some(&100.0), "");
        }
        Err(e) => r.check("color + canvas runs", false, e),
    }
    // A deleted layout: the step fails with the block's name (check() refuses it before; this is the run).
    let gone = canvas_step_of("nope", "Deleted");
    let res = run_steps(&tools, &[gone], small, &format!("{tag}-gone"), &no_cancel, &mut |_| {}, &mut |_| {});
    r.check("canvas: a deleted layout fails the step and makes nothing", res.as_ref().is_err_and(|e| e.starts_with("Canvas:") && e.contains("Deleted")), &format!("{:?}", res.as_ref().map(|o| o.file.clone())));
    // Cancel during the export: returns at once, the export's file goes when ffmpeg is done.
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let mut set_at: Option<Instant> = None;
    let res = run_steps(&tools, &[cv.clone()], big, &format!("{tag}-cancel"), &cancel, &mut |p| {
        if p > 1.0 && set_at.is_none() {
            cancel.store(true, Ordering::SeqCst);
            set_at = Some(Instant::now());
        }
    }, &mut |_| {});
    let took = set_at.map(|x| x.elapsed());
    r.check("canvas: cancel returns Cancelled within a second", res.as_ref().err().map(String::as_str) == Some(CANCELLED) && took.is_some_and(|d| d < Duration::from_millis(1000)), &format!("{took:?} after {:.1} s", t0.elapsed().as_secs_f64()));
    let cleaned = wait_until(30.0, || tmp_files(&format!("{tag}-cancel")).is_empty());
    r.check("canvas: after a cancel the export's file is removed once it ends", cleaned, &format!("{:?}", tmp_files(&format!("{tag}-cancel"))));
    drop(_g);
    r.check("canvas: no temporary files left", tmp_files(tag).is_empty(), &format!("{:?}", tmp_files(tag)));
    let _ = fs::remove_file(&square);
}

/// Through the queue and the library, the way the app runs a rule.
fn queue_checks(r: &mut Report, ffmpeg: &Path, root: &Path, dir: &Path, small: &Path, big: &Path) {
    let core = Core::new();
    let rule = Rule { id: "r-test".into(), name: "Share".into(), steps: vec![Step { kind: "color".into(), look: "Soft".into(), ..Default::default() }, Step { kind: "compress".into(), mb: 5, ..Default::default() }, Step::of("copy")] };
    let skip_rule = Rule { id: "r-skip".into(), name: "Small".into(), steps: vec![Step { kind: "compress".into(), mb: 500, ..Default::default() }] };
    let blur_rule = Rule { id: "r-blur".into(), name: "Blurry".into(), steps: vec![Step::of("blur")] };
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = root.to_string_lossy().into_owned();
        s.rules = vec![rule.clone(), skip_rule.clone(), blur_rule.clone(), Rule { id: "r-gone".into(), name: "Vertical".into(), steps: vec![canvas_step_of("deleted", "Old layout")] }];
        s.blur_path = util::data_dir().join("no blur here").join("blur-cli.exe").to_string_lossy().into_owned();
    }
    core.jobs.connect(&core);
    let add = |p: &Path, title: &str| {
        let mut m = library::register(ffmpeg, p, title, "Test Game", "clip");
        m.folder = "Test Game".into();
        let id = m.id.clone();
        core.lib.lock().unwrap().add(m);
        id
    };
    let big_id = add(big, "big");
    let small_id = add(small, "small");
    let state = |jid: &str| core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();

    r.check("rules_run: an unknown rule is refused", run_ids(&core, "nope", &[big_id.clone()]).is_err_and(|e| e == "That render rule no longer exists"), "");
    let res = run_ids(&core, "r-gone", &[small_id.clone()]);
    r.check("rules_run: a rule whose canvas layout was deleted is refused, nothing queued", res.as_ref().is_err_and(|e| e.contains("\"Old layout\" no longer exists")) && core.jobs.list().is_empty(), &format!("{res:?}"));
    if blur::find(&core.settings()).is_none() {
        let res = run_ids(&core, "r-blur", &[small_id.clone()]);
        r.check("rules_run: a rule with Blur is refused before queueing when Blur is missing", res.as_ref().err().map(String::as_str) == Some(BLUR_MISSING) && core.jobs.list().is_empty(), &format!("{res:?}"));
    } else {
        r.line("INFO blur-cli was found on this PC even with a wrong path set: the missing-Blur refusal was not tested (and Blur was not started)");
    }

    let big_size = fs::metadata(big).map(|m| m.len()).unwrap_or(0);
    if big_size > compress::limit_bytes(5) {
        let res = run_ids(&core, "r-test", &[big_id.clone(), "missing".into()]);
        let jobs = res.clone().unwrap_or_default();
        r.check("rules_run: one job per clip, a refused clip comes back unqueued with why", jobs.len() == 2 && !jobs[0].id.is_empty() && jobs[0].kind == "rule" && jobs[1].id.is_empty() && jobs[1].state == "error" && jobs[1].message == "Clip not found", &format!("{res:?}"));
        let dup = enqueue(&core, &rule, big_id.clone());
        r.check("a second run of a rule on the same clip is refused", dup.as_ref().err().map(String::as_str) == Some("That clip is already going through a render rule"), &format!("{:?}", dup.err()));
        if let Some(job) = jobs.first().filter(|j| !j.id.is_empty()) {
            let mut stages: Vec<String> = Vec::new();
            let ended = wait_until(240.0, || {
                let j = state(&job.id);
                if !j.stage.is_empty() && stages.last() != Some(&j.stage) {
                    stages.push(j.stage.clone());
                }
                !matches!(j.state.as_str(), "queued" | "running")
            });
            let j = state(&job.id);
            r.check("queued rule finishes", ended && j.state == "done", &format!("{} {}", j.state, j.message));
            r.line(&format!("INFO stages seen in the job list: {stages:?}"));
            r.check("the job's stage names its steps", stages.iter().any(|x| x == "color" || x == "compress"), &format!("{stages:?}"));
            match core.lib.lock().unwrap().get(&j.result_id) {
                Some(n) => {
                    let p = Path::new(&n.path);
                    r.check("result: a library clip of kind \"rule\" named \"big (Share)\"", n.kind == "rule" && n.title == "big (Share)", &format!("{} {}", n.kind, n.title));
                    r.check("result: beside the source, in its folder, same game", p.parent() == Some(dir) && n.folder == "Test Game" && n.game == "Test Game", &n.path);
                    r.check("result: under 5 MB", n.size > 0 && n.size <= compress::limit_bytes(5), &compress::fmt_size(n.size));
                }
                None => r.check("result registered in the library", false, &j.result_id),
            }
        }
        r.check("queue: no temporary rule files left", tmp_files("rule-").is_empty(), &format!("{:?}", tmp_files("rule-")));
    }

    // Every step skipped: the job ends well and makes nothing.
    let n_before = core.lib.lock().unwrap().clips.len();
    let res = run_ids(&core, "r-skip", &[small_id.clone()]);
    if let Some(job) = res.as_ref().ok().and_then(|v| v.first().cloned()) {
        let ended = wait_until(30.0, || !matches!(state(&job.id).state.as_str(), "queued" | "running"));
        let j = state(&job.id);
        r.check("a rule whose every step is skipped ends well and adds no clip", ended && j.state == "done" && j.result_id.is_empty() && core.lib.lock().unwrap().clips.len() == n_before, &format!("{} {}", j.state, j.message));
    } else {
        r.check("a rule whose every step is skipped is queued", false, &format!("{res:?}"));
    }

    // A clip imported in place (outside the clips folder): its result goes into the clips folder.
    let outside = util::data_dir().join("Other program");
    let _ = fs::create_dir_all(&outside);
    let ext_clip = outside.join("theirs.mp4");
    if fs::copy(small, &ext_clip).is_ok() {
        let m = library::register(ffmpeg, &ext_clip, "theirs", "Test Game", "import");
        let mut m2 = m.clone();
        m2.folder = "Test Game".into();
        let (d, f) = out_dir(&core.settings(), &m2);
        r.check("a clip imported in place: its result goes to its folder in the clips folder", d == dir && f == "Test Game", &d.to_string_lossy());
        let (d2, f2) = out_dir(&core.settings(), &m);
        r.check("a clip imported in place, no folder: the clips folder", d2 == root && f2.is_empty(), &d2.to_string_lossy());
        let mine = library::register(ffmpeg, small, "small", "Test Game", "clip");
        r.check("a clip in the clips folder: its result goes beside it", out_dir(&core.settings(), &mine).0 == dir, "");
    }
    core.jobs.shutdown();
}
