//! Wrapper around f0e's `blur` (frame-blending motion blur). Glint writes a
//! config from its own settings and drives `blur-cli`, reporting progress.
//! The config layout is blur v2's (checked against 2.23).
use crate::library;
use crate::settings::{BlurSettings, Settings};
use crate::util;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// The Blur build that ships with Glint: `%LOCALAPPDATA%\Glint\blur`, or a
/// `blur` folder beside Glint.exe. It must sit on a short path: its bundled
/// VapourSynth fails to load from paths near Windows' 260-character limit.
pub fn bundled() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Glint").join("blur"));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(dir.join("blur"));
    }
    dirs.into_iter().map(|d| d.join("blur-cli.exe")).find(|p| p.is_file())
}

/// A path set by the user wins, then the bundled build, then `PATH`.
pub fn find(s: &Settings) -> Option<PathBuf> {
    if !s.blur_path.trim().is_empty() {
        return util::find_exe(&s.blur_path, "blur-cli");
    }
    bundled().or_else(|| util::find_exe("", "blur-cli"))
}

/// File extension of the rendered clip.
pub fn container(b: &BlurSettings) -> String {
    let c: String = b.container.trim().trim_start_matches('.').chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if b.advanced && !c.is_empty() { c.to_lowercase() } else { "mp4".into() }
}

/// One config line can hold one value: drop line breaks from free text.
fn one_line(s: &str, fallback: &str) -> String {
    let t = s.replace(['\r', '\n'], " ");
    let t = t.trim();
    if t.is_empty() { fallback.to_string() } else { t.to_string() }
}

/// The Blur settings as they suit this PC, given the brands of its graphics cards (`present`:
/// "nvidia" | "amd" | "intel" | "other", from `win::hardware`). `gpu_type` (default "nvidia") stays
/// when the PC has a card of that brand; otherwise it becomes the card Glint encodes on (NVIDIA,
/// else AMD, else Intel), so the GPU encoding default works on AMD and Intel PCs too. With no card
/// of a known brand, GPU encoding is turned off (Blur would fail without one). Only the config
/// Blur is given changes; the saved settings stay as the user set them.
pub fn for_this_pc(b: &BlurSettings, present: &[&str]) -> BlurSettings {
    const BRANDS: [&str; 3] = ["nvidia", "amd", "intel"];
    let mut out = b.clone();
    let wanted = b.gpu_type.trim().to_lowercase();
    if !(BRANDS.contains(&wanted.as_str()) && present.contains(&wanted.as_str())) {
        match BRANDS.iter().find(|v| present.contains(v)) {
            Some(v) => out.gpu_type = v.to_string(),
            None => out.gpu_encoding = false,
        }
    }
    out
}

/// Checks of `for_this_pc` for `--polish-test`: (name, ok, detail).
pub fn gpu_self_checks() -> Vec<(String, bool, String)> {
    let base = BlurSettings::default();
    let mut out = vec![("blur: GPU encoding is on by default".to_string(), base.gpu_encoding, String::new())];
    let cases: [(&str, &[&str], &str, bool); 8] = [
        ("nvidia", &["nvidia"], "nvidia", true),
        ("nvidia", &["amd"], "amd", true),
        ("nvidia", &["intel"], "intel", true),
        ("nvidia", &["other", "intel", "amd"], "amd", true),
        ("intel", &["nvidia", "intel"], "intel", true),
        ("amd", &["nvidia", "intel"], "nvidia", true),
        ("bogus", &["intel"], "intel", true),
        ("nvidia", &["other"], "nvidia", false),
    ];
    for (wanted, present, want_type, want_enc) in cases {
        let b = BlurSettings { gpu_type: wanted.into(), ..BlurSettings::default() };
        let got = for_this_pc(&b, present);
        out.push((
            format!("blur gpu type {wanted:?} on {present:?} -> {want_type}, GPU encoding {want_enc}"),
            got.gpu_type == want_type && got.gpu_encoding == want_enc,
            format!("{} {}", got.gpu_type, got.gpu_encoding),
        ));
    }
    let off = for_this_pc(&BlurSettings { gpu_encoding: false, ..BlurSettings::default() }, &["amd"]);
    out.push(("blur: GPU encoding left off stays off".into(), !off.gpu_encoding && off.gpu_type == "amd", String::new()));
    out
}

/// A complete blur v2 config, in the layout blur itself writes.
pub fn config_text(b: &BlurSettings) -> String {
    let pick = |v: &str, allowed: &[&str]| -> String { if allowed.contains(&v) { v.to_string() } else { allowed[0].to_string() } };
    let mut t = format!(
        "- blur\n\
         blur: {blur}\n\
         blur amount: {amount}\n\
         blur output fps: {fps}\n\
         blur weighting: {weighting}\n\
         blur gamma: {gamma}\n\
         \n\
         - interpolation\n\
         interpolate: {interpolate}\n\
         interpolated fps: {interp}\n\
         interpolation method: {interp_method}\n\
         \n\
         - pre-interpolation\n\
         pre-interpolate: {pre}\n\
         pre-interpolated fps: {pre_fps}\n\
         \n\
         - deduplication\n\
         deduplicate: {dedup}\n\
         deduplicate method: {dedup_method}\n\
         \n\
         - rendering\n\
         encode preset: {preset}\n\
         quality: {quality}\n\
         preview: false\n\
         detailed filenames: false\n\
         copy dates: {copy_dates}\n\
         \n\
         - gpu acceleration\n\
         gpu decoding: {gpu_dec}\n\
         gpu interpolation: {gpu_int}\n\
         gpu encoding: {gpu_enc}\n\
         gpu type (nvidia/amd/intel): {gpu_type}\n\
         rife gpu number: {rife_gpu}\n\
         \n\
         - timescale\n\
         timescale: {timescale}\n\
         input timescale: {ts_in}\n\
         output timescale: {ts_out}\n\
         adjust timescaled audio pitch: {pitch}\n\
         \n\
         - filters\n\
         filters: {filters}\n\
         brightness: {brightness}\n\
         saturation: {saturation}\n\
         contrast: {contrast}\n\
         \n\
         - advanced\n\
         advanced: {advanced}\n",
        blur = b.blur,
        amount = b.amount.clamp(0.0, 10.0),
        fps = b.output_fps.clamp(1, 1000),
        weighting = one_line(&b.weighting, "equal"),
        gamma = b.gamma.clamp(0.1, 10.0),
        interpolate = b.interpolate,
        interp = one_line(&b.interpolated_fps, "5x"),
        interp_method = pick(&b.interpolation_method, &["svp", "rife"]),
        pre = b.pre_interpolate,
        pre_fps = one_line(&b.pre_interpolated_fps, "360"),
        dedup = b.deduplicate,
        dedup_method = pick(&b.deduplicate_method, &["svp", "rife", "old"]),
        preset = one_line(&b.encode_preset, "h264"),
        quality = b.quality.clamp(0, 51),
        copy_dates = b.copy_dates,
        gpu_dec = b.gpu_decoding,
        gpu_int = b.gpu_interpolation,
        gpu_enc = b.gpu_encoding,
        gpu_type = pick(&b.gpu_type, &["nvidia", "amd", "intel"]),
        rife_gpu = b.rife_gpu.max(-1),
        timescale = b.timescale,
        ts_in = b.input_timescale.clamp(0.01, 100.0),
        ts_out = b.output_timescale.clamp(0.01, 100.0),
        pitch = b.adjust_audio_pitch,
        filters = b.filters,
        brightness = b.brightness.clamp(0.0, 10.0),
        saturation = b.saturation.clamp(0.0, 10.0),
        contrast = b.contrast.clamp(0.0, 10.0),
        advanced = b.advanced,
    );
    if b.advanced {
        t += &format!(
            "\n\
             - advanced deduplication\n\
             deduplicate range: {range}\n\
             deduplicate threshold: {threshold}\n\
             \n\
             - advanced rendering\n\
             video container: {container}\n\
             custom ffmpeg filters: {filters}\n\
             debug: {debug}\n\
             \n\
             - advanced blur\n\
             blur weighting gaussian std dev: {std_dev}\n\
             blur weighting gaussian mean: {mean}\n\
             blur weighting gaussian bound: {bound}\n\
             \n\
             - advanced interpolation\n\
             svp interpolation preset: {svp_preset}\n\
             svp interpolation algorithm: {svp_algo}\n\
             interpolation block size: {block}\n\
             interpolation mask area: {mask}\n\
             rife model: {rife_model}\n",
            range = b.deduplicate_range.max(-1),
            threshold = b.deduplicate_threshold.clamp(0.0, 1.0),
            container = container(b),
            filters = one_line(&b.ffmpeg_filters, ""),
            debug = b.debug,
            std_dev = b.gaussian_std_dev,
            mean = b.gaussian_mean,
            bound = one_line(&b.gaussian_bound, "[0,2]"),
            svp_preset = one_line(&b.svp_preset, "weak"),
            svp_algo = b.svp_algorithm,
            block = b.block_size,
            mask = b.mask_area,
            rife_model = one_line(&b.rife_model, "rife-v4.26_ensembleFalse"),
        );
    }
    t
}

/// Kill a process and everything it started (blur-cli spawns vspipe and ffmpeg).
fn kill_tree(pid: u32) {
    let _ = util::hidden_cmd(Path::new("taskkill"))
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Render `input` to `output`. `progress` gets 0..100 as blur reports it.
///
/// blur reads a `.blur-config.cfg` beside the input before its global config.
/// (`-c <config>` is not used: 1.92 hangs on it and 2.23 did not pick the file
/// up.) So each render gets a private job folder holding a hard link to the
/// clip plus that config, and the clips folder stays clean.
pub fn render(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, progress: impl FnMut(f32)) -> Result<(), String> {
    render_cancellable(blur_cli, input, output, cfg, &AtomicBool::new(false), progress)
}

/// The error `render_cancellable` returns when `cancel` stopped the render.
pub const CANCELLED: &str = "Cancelled";

/// `render`, stoppable: once `cancel` is set, blur-cli and everything it
/// started are killed within about a quarter of a second, the partial
/// `output` is removed, and the result is `Err(CANCELLED)`.
pub fn render_cancellable(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, cancel: &AtomicBool, progress: impl FnMut(f32)) -> Result<(), String> {
    render_staged(blur_cli, input, output, cfg, cancel, &mut |_| {}, progress)
}

/// `render_cancellable` that also says what happens before Blur's first
/// percent, which can be a long time: `stage` gets "prepare" while the clip is
/// re-timed for Blur (only when it needs that), then "start" once blur-cli
/// runs. Blur reports nothing while it reads the clip in and renders its first
/// frames, so without this a render looked stuck at 0 %.
pub fn render_staged(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, cancel: &AtomicBool, stage: &mut dyn FnMut(&str), mut progress: impl FnMut(f32)) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    let job = util::tmp_dir().join(format!("blur-{}", util::new_id()));
    fs::create_dir_all(&job).map_err(|e| format!("Cannot create blur job folder: {e}"))?;
    let result = render_in(blur_cli, input, output, cfg, &job, cancel, stage, &mut progress);
    let _ = fs::remove_dir_all(&job);
    result
}

#[allow(clippy::too_many_arguments)]
fn render_in(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, job: &Path, cancel: &AtomicBool, stage: &mut dyn FnMut(&str), progress: &mut impl FnMut(f32)) -> Result<(), String> {
    let ext = input.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    let s = Settings::load();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg");
    let issue = ffmpeg.as_deref().and_then(library::ffprobe_for).and_then(|p| sync_issue(&p, input));
    if issue.is_some() && ffmpeg.is_some() {
        stage("prepare");
    }
    let link = stage_input(ffmpeg.as_deref(), &s.encoder, issue, input, &ext, job, cancel)?;
    stage("start");
    // The GPU type and GPU encoding as this PC's cards allow (the "nvidia" default on an AMD or Intel PC).
    let hw = crate::win::hardware();
    let present: Vec<&str> = hw.gpus.iter().map(|g| g.vendor.as_str()).collect();
    let fit = for_this_pc(cfg, &present);
    if fit.gpu_type != cfg.gpu_type || fit.gpu_encoding != cfg.gpu_encoding {
        util::log(&format!("blur: gpu type {} -> {}, gpu encoding {} -> {} (cards: {present:?})", cfg.gpu_type, fit.gpu_type, cfg.gpu_encoding, fit.gpu_encoding));
    }
    fs::write(job.join(".blur-config.cfg"), config_text(&fit)).map_err(|e| format!("Cannot write blur config: {e}"))?;
    run_blur(blur_cli, &link, output, cancel, progress)
}

/// The file Blur is given, in `job`: a re-timed copy when `issue` says Blur
/// would read `input` out of step, else (or when that copy cannot be made) a
/// hard link to `input`, or a copy of it on another drive.
fn stage_input(ffmpeg: Option<&Path>, encoder: &str, issue: Option<String>, input: &Path, ext: &str, job: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let mut staged = None;
    if let (Some(why), Some(ffmpeg)) = (issue, ffmpeg) {
        // Blur would read this file's timing differently from every player
        // (see `sync_issue`): hand it a clean copy instead.
        util::log(&format!("blur: {why}; giving Blur a re-timed copy"));
        let copy = job.join("in.mp4");
        match stage_retimed(ffmpeg, encoder, input, &copy, cancel) {
            Ok(()) => staged = Some(copy),
            Err(e) if e == CANCELLED => return Err(e),
            // Rendering possibly out of step beats not rendering at all.
            Err(e) => util::log(&format!("blur: the re-timed copy failed ({e}); giving Blur the clip as it is")),
        }
    }
    if let Some(p) = staged {
        return Ok(p);
    }
    let link = job.join(format!("in.{ext}"));
    if fs::hard_link(input, &link).is_err() {
        // Different drive: fall back to a copy.
        fs::copy(input, &link).map_err(|e| format!("Cannot stage the clip for Blur: {e}"))?;
    }
    Ok(link)
}

/// Run blur-cli on `link` (its config sits beside it) until it finishes, stalls or is cancelled.
///
/// blur-cli writes its "12.3% complete" lines (two short ones a second)
/// through a buffer that, when the output is a pipe, is only handed on when
/// it is full or the program ends (measured with `--blur-start-test`: all 40
/// lines of a 20 s render arrived in the same instant, as it ended; through a
/// console the first came 1.6 s after the start). That was most of "Blur
/// takes long to start": the job showed "Starting Blur…" until then. So
/// blur-cli gets a console of its own, without a window (CREATE_NO_WINDOW, as
/// every tool Glint starts), where nothing is held back, and Glint reads that
/// console's last lines (`console::run`). The pipe stays as the fallback.
fn run_blur(blur_cli: &Path, link: &Path, output: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(f32)) -> Result<(), String> {
    run_blur_seen(blur_cli, link, output, cancel, progress, &mut |_| {})
}

/// `run_blur`; `seen` gets each console line as it appears or changes (for `--blur-start-test`).
fn run_blur_seen(blur_cli: &Path, link: &Path, output: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(f32), seen: &mut dyn FnMut(&str)) -> Result<(), String> {
    if console::usable() {
        let args = [OsStr::new("-i"), link.as_os_str(), OsStr::new("-o"), output.as_os_str()];
        let mut shown = -1.0f32;
        match console::run(blur_cli, &args, cancel, &mut |fresh| {
            for row in fresh {
                seen(row);
            }
            if let Some(p) = fresh.iter().rev().find_map(|r| percent_of(r)) {
                if p > shown {
                    shown = p;
                    progress(p);
                }
            }
        }) {
            Ok(console::Outcome::Cancelled) => {
                let _ = fs::remove_file(output);
                return Err(CANCELLED.into());
            }
            Ok(console::Outcome::Stalled) => {
                let _ = fs::remove_file(output);
                return Err("Blur stopped responding and was cancelled".into());
            }
            Ok(console::Outcome::Exited(code, tail)) => {
                if code != 0 || !output.is_file() {
                    return Err(format!("Blur render failed: {tail}"));
                }
                return Ok(());
            }
            // Not started that way: the pipe below.
            Err(e) => util::log(&format!("blur: no console for blur-cli ({e}); reading its output through a pipe")),
        }
    }
    run_blur_piped(blur_cli, link, output, cancel, progress)
}

/// The percent in a line blur-cli prints ("12.3% complete (15/131, 29.07 fps)").
fn percent_of(line: &str) -> Option<f32> {
    let at = line.find("% complete")?;
    let head = &line[..at];
    let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '.')).map(|i| i + 1).unwrap_or(0);
    head[start..].parse::<f32>().ok().filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
}

/// `run_blur` through a pipe: progress arrives late (see `run_blur`).
fn run_blur_piped(blur_cli: &Path, link: &Path, output: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(f32)) -> Result<(), String> {
    let mut child = util::hidden_cmd(blur_cli)
        .arg("-i")
        .arg(link)
        .arg("-o")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start blur-cli: {e}"))?;
    crate::util::track_child(&child); // ends with Glint (0.1.41)

    // Read progress on a helper thread so a stalled render can be detected and killed.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(mut out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut line = String::new();
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for ch in String::from_utf8_lossy(&buf[..n]).chars() {
                    if ch == '\n' || ch == '\r' {
                        if !line.trim().is_empty() && tx.send(line.trim().to_string()).is_err() {
                            return;
                        }
                        line.clear();
                    } else {
                        line.push(ch);
                    }
                }
            }
        });
    }
    let mut tail = String::new();
    let mut last_line = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill_tree(child.id());
            let _ = child.wait();
            let _ = fs::remove_file(output);
            return Err(CANCELLED.into());
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                last_line = Instant::now();
                if line.contains("% complete") {
                    if let Some(pct) = line.split('%').next().and_then(|p| p.trim().parse::<f32>().ok()) {
                        progress(pct);
                    }
                }
                tail = line;
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) if last_line.elapsed() < Duration::from_secs(180) => {}
            Err(RecvTimeoutError::Timeout) => {
                kill_tree(child.id());
                let _ = child.wait();
                let _ = fs::remove_file(output);
                return Err("Blur stopped responding and was cancelled".into());
            }
        }
    }
    let status = child.wait().map_err(|e| format!("blur-cli failed: {e}"))?;
    if !status.success() || !output.is_file() {
        return Err(format!("Blur render failed: {tail}"));
    }
    Ok(())
}

/// blur-cli in a console of its own that has no window, and that console's
/// last lines read back (see `run_blur`). Glint, which has no console, attaches
/// to it for a moment every 300 ms, reads the lines at the cursor and lets go.
mod console {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: [i16; 4],
        max_window: Coord,
    }

    #[repr(C)]
    struct StartupInfo {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_chars: u32,
        y_chars: u32,
        fill: u32,
        flags: u32,
        show_window: u16,
        reserved2_len: u16,
        reserved2: *mut u8,
        std_input: Handle,
        std_output: Handle,
        std_error: Handle,
    }

    #[repr(C)]
    struct ProcessInfo {
        process: Handle,
        thread: Handle,
        pid: u32,
        tid: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateProcessW(app: *const u16, cmd: *mut u16, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, cwd: *const u16, si: *const StartupInfo, pi: *mut ProcessInfo) -> i32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
        fn CloseHandle(h: Handle) -> i32;
        fn GetConsoleCP() -> u32;
        fn AttachConsole(pid: u32) -> i32;
        fn FreeConsole() -> i32;
        fn GetStdHandle(which: u32) -> Handle;
        fn SetStdHandle(which: u32, h: Handle) -> i32;
        fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *const c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
        fn GetConsoleScreenBufferInfo(h: Handle, info: *mut BufferInfo) -> i32;
        fn ReadConsoleOutputCharacterW(h: Handle, buf: *mut u16, len: u32, at: Coord, read: *mut u32) -> i32;
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }

    /// While Glint is attached to Blur's console (a few milliseconds at a time) it is a member of
    /// that console: a Ctrl+C or Ctrl+Break event raised there (a program stopping its helpers that
    /// way) would reach Glint too, and the default handler ends the process. This one takes those
    /// two events and does nothing. Added by the lead in review; registered once, before the first attach.
    unsafe extern "system" fn ignore_ctrl(event: u32) -> i32 {
        i32::from(event <= 1) // CTRL_C_EVENT = 0, CTRL_BREAK_EVENT = 1: handled; the rest go on as usual
    }
    static CTRL_GUARD: std::sync::Once = std::sync::Once::new();

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE.
    const STD: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    /// Lines read: the cursor's and the ones above it.
    const ROWS: i16 = 6;
    /// A process can be attached to one console at a time: renders running side by side take turns.
    static ATTACH: Mutex<()> = Mutex::new(());

    pub enum Outcome {
        /// Exit code and the last line it printed.
        Exited(u32, String),
        Cancelled,
        Stalled,
    }

    /// True when Glint has no console of its own (the app, not a build started from a terminal), so it can attach to another.
    pub fn usable() -> bool {
        let _turn = ATTACH.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { GetConsoleCP() == 0 }
    }

    /// One argument as CreateProcess's command line wants it.
    fn quote(arg: &OsStr) -> Vec<u16> {
        let (q, bs) = (u16::from(b'"'), u16::from(b'\\'));
        let s: Vec<u16> = arg.encode_wide().collect();
        if !s.is_empty() && !s.iter().any(|&c| c == u16::from(b' ') || c == u16::from(b'\t') || c == q) {
            return s;
        }
        let mut out = vec![q];
        let mut run = 0usize;
        for &c in &s {
            if c == bs {
                run += 1;
                continue;
            }
            let n = if c == q { run * 2 + 1 } else { run };
            out.extend(std::iter::repeat(bs).take(n));
            run = 0;
            out.push(c);
        }
        out.extend(std::iter::repeat(bs).take(run * 2));
        out.push(q);
        out
    }

    /// The lines at and above the cursor of process `pid`'s console, or None when it cannot be read (yet).
    fn read_rows(pid: u32) -> Option<Vec<String>> {
        let _turn = ATTACH.lock().unwrap_or_else(|e| e.into_inner());
        CTRL_GUARD.call_once(|| unsafe {
            SetConsoleCtrlHandler(Some(ignore_ctrl), 1);
        });
        unsafe {
            let saved = STD.map(|n| GetStdHandle(n));
            if AttachConsole(pid) == 0 {
                return None;
            }
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            // GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING.
            let h = CreateFileW(name.as_ptr(), 0xC000_0000, 3, std::ptr::null(), 3, 0, std::ptr::null_mut());
            let mut rows = None;
            if !h.is_null() && h as isize != -1 {
                let mut info = BufferInfo::default();
                if GetConsoleScreenBufferInfo(h, &mut info) != 0 && info.size.x > 0 {
                    let width = info.size.x as usize;
                    let mut buf = vec![0u16; width];
                    let mut out = Vec::new();
                    for y in (info.cursor.y - ROWS).max(0)..=info.cursor.y {
                        let mut n = 0u32;
                        if ReadConsoleOutputCharacterW(h, buf.as_mut_ptr(), width as u32, Coord { x: 0, y }, &mut n) != 0 {
                            out.push(String::from_utf16_lossy(&buf[..(n as usize).min(width)]).trim_end().to_string());
                        }
                    }
                    rows = Some(out);
                }
                CloseHandle(h);
            }
            FreeConsole();
            for (n, h) in STD.iter().zip(saved) {
                SetStdHandle(*n, h);
            }
            rows
        }
    }

    /// Run `exe args` in a console without a window until it ends, stalls (nothing new for 180 s) or `cancel` is set.
    /// `fresh` gets the lines that appeared or changed since the last look, top to bottom.
    pub fn run(exe: &Path, args: &[&OsStr], cancel: &AtomicBool, fresh: &mut dyn FnMut(&[String])) -> Result<Outcome, String> {
        let mut cmd = quote(exe.as_os_str());
        for a in args {
            cmd.push(u16::from(b' '));
            cmd.extend(quote(a));
        }
        cmd.push(0);
        let mut si: StartupInfo = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<StartupInfo>() as u32;
        let mut pi: ProcessInfo = unsafe { std::mem::zeroed() };
        // No STARTF_USESTDHANDLES: the program writes to its new console, not to a pipe.
        let ok = unsafe { CreateProcessW(std::ptr::null(), cmd.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_NO_WINDOW, std::ptr::null(), std::ptr::null(), &si, &mut pi) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        unsafe { CloseHandle(pi.thread) };
        let mut last: Vec<String> = Vec::new();
        let mut changed = Instant::now();
        let mut readable = false;
        let outcome = loop {
            if cancel.load(Ordering::SeqCst) {
                super::kill_tree(pi.pid);
                unsafe { WaitForSingleObject(pi.process, 10_000) };
                break Outcome::Cancelled;
            }
            let ended = unsafe { WaitForSingleObject(pi.process, 300) } == 0;
            if let Some(rows) = read_rows(pi.pid) {
                readable = true;
                if rows != last {
                    changed = Instant::now();
                    let new: Vec<String> = rows.iter().filter(|r| !r.is_empty() && !last.contains(r)).cloned().collect();
                    if !new.is_empty() {
                        fresh(&new);
                    }
                    last = rows;
                }
            }
            if ended {
                let mut code = 1u32;
                unsafe { GetExitCodeProcess(pi.process, &mut code) };
                let tail = last.iter().rev().find(|r| !r.is_empty()).cloned().unwrap_or_default();
                break Outcome::Exited(code, tail);
            }
            // Only once the console could be read: otherwise silence says nothing.
            if readable && changed.elapsed() > Duration::from_secs(180) {
                super::kill_tree(pi.pid);
                unsafe { WaitForSingleObject(pi.process, 10_000) };
                break Outcome::Stalled;
            }
        };
        unsafe { CloseHandle(pi.process) };
        Ok(outcome)
    }

    /// Checks of the command-line quoting for `--blur-start-test`: (name, ok, detail).
    pub fn self_checks() -> Vec<(String, bool, String)> {
        let cases = [("plain", "plain"), (r"C:\Glint Clips\a b.mp4", r#""C:\Glint Clips\a b.mp4""#), (r"C:\x y\", r#""C:\x y\\""#), ("", r#""""#), (r#"say "hi""#, r#""say \"hi\"""#)];
        cases
            .iter()
            .map(|(arg, want)| {
                let got = String::from_utf16_lossy(&quote(OsStr::new(arg)));
                (format!("console: {arg:?} is passed as {want}"), got == *want, got)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Timing Blur reads differently from a player
//
// Blur decodes the video with VapourSynth (BestSource), which numbers frames
// from the first one in the file and ignores MP4 edit lists, then muxes the
// input's audio back with ffmpeg, which honours them. A lossless trim
// (`clip::trim`, a stream copy that starts between keyframes) keeps the frames
// from the keyframe before the cut and hides them with an edit list, so Blur's
// video came out up to a second (one GOP) late against its audio (measured
// +120 ms and +364 ms). Such a file is re-timed first (`stage_retimed`); every
// other file is hard-linked as before. Variable frame rate on its own is fine:
// Blur resamples by timestamp at the stream's nominal rate (measured within
// 9 ms on a replay-buffer-like clip), so buffer clips and editor exports are
// handed over untouched.

/// Stream timing of a file as ffprobe (and so every player) sees it.
#[derive(Debug, Default)]
struct Timing {
    video_start: Option<f64>,
    audio_starts: Vec<f64>,
    /// The video's nominal and average rates, as ffprobe writes them ("144/1").
    r_rate: String,
    avg_rate: String,
    /// The first video packets are only there to decode from: an edit list hides them.
    preroll: bool,
    /// The file's start time: where playback begins.
    origin: f64,
}

/// A frame rate ffmpeg's `num/den` text gives, if it is one Blur can work at.
fn rate_of(r: &str) -> Option<f64> {
    let (n, d) = r.split_once('/').unwrap_or((r, "1"));
    let f = n.trim().parse::<f64>().ok()? / d.trim().parse::<f64>().ok()?;
    Some(f).filter(|f| f.is_finite() && (1.0..=1000.0).contains(f))
}

fn probe_timing(ffprobe: &Path, file: &Path) -> Option<Timing> {
    let run = |args: &[&str]| -> Option<String> {
        let out = util::hidden_cmd(ffprobe).args(args).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let flat = run(&["-v", "error", "-show_entries", "stream=codec_type,start_time,r_frame_rate,avg_frame_rate:format=start_time", "-of", "flat"])?;
    // streams.stream.<n>.<field>="<value>", format.start_time="<value>"
    let mut streams: std::collections::BTreeMap<u32, std::collections::HashMap<String, String>> = Default::default();
    let mut origin = 0.0;
    for line in flat.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        if key == "format.start_time" {
            origin = value.trim().trim_matches('"').parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0);
            continue;
        }
        let mut parts = key.split('.').skip(2);
        let (Some(n), Some(field)) = (parts.next().and_then(|n| n.parse::<u32>().ok()), parts.next()) else { continue };
        streams.entry(n).or_default().insert(field.to_string(), value.trim().trim_matches('"').to_string());
    }
    let mut t = Timing { origin, ..Default::default() };
    let start = |s: &std::collections::HashMap<String, String>| s.get("start_time").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    for s in streams.values() {
        match s.get("codec_type").map(String::as_str) {
            Some("video") if t.video_start.is_none() => {
                t.video_start = start(s);
                t.r_rate = s.get("r_frame_rate").cloned().unwrap_or_default();
                t.avg_rate = s.get("avg_frame_rate").cloned().unwrap_or_default();
            }
            Some("audio") => t.audio_starts.extend(start(s)),
            _ => {}
        }
    }
    if let Some(v) = t.video_start {
        let packets = run(&["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", "-read_intervals", "%+#8"]).unwrap_or_default();
        t.preroll = packets.lines().any(|l| {
            let (pts, flags) = l.split_once(',').unwrap_or((l, ""));
            flags.contains('D') || pts.trim().parse::<f64>().is_ok_and(|p| p < v - 0.0005)
        });
    }
    Some(t)
}

/// Why Blur would put `file`'s video out of step with its audio, or None when
/// it can take the file as it is (a clip straight from the replay buffer, an
/// editor export).
pub fn sync_issue(ffprobe: &Path, file: &Path) -> Option<String> {
    let t = probe_timing(ffprobe, file)?;
    let v = t.video_start?;
    if t.preroll {
        return Some("the clip was cut between keyframes (its first frames are hidden by an edit list)".into());
    }
    // Blur's video always starts at the beginning; audio that starts first would run ahead.
    if let Some(a) = t.audio_starts.iter().copied().reduce(f64::min).filter(|a| v - a > 0.005) {
        return Some(format!("the video starts {:.0} ms after the audio", (v - a) * 1000.0));
    }
    if rate_of(&t.r_rate).is_none() {
        return Some(format!("the video's frame rate \"{}\" is not usable", t.r_rate));
    }
    None
}

/// Encoders to try for the staged copy, best first; x264 always works.
fn stage_encoders(wanted: &str) -> &'static [&'static str] {
    match wanted {
        "nvenc" => &["nvenc", "x264"],
        "amf" => &["amf", "x264"],
        "qsv" => &["qsv", "x264"],
        "x264" => &["x264"],
        _ => &["nvenc", "amf", "qsv", "x264"],
    }
}

/// Near-transparent settings: Blur re-encodes the result anyway, so this
/// generation must not show. The fastest presets with a lower quantiser: the
/// file is temporary, so only time and quality count (`--blur-start-test` with
/// GLINT_BLUR_START_VARIANTS, a 10.2 s 1080p 144 fps trim, PSNR against a
/// lossless copy: NVENC p4 cq 16 3.5 s 60.8 dB -> p1 cq 14 2.7 s 61.7 dB;
/// x264 veryfast crf 14 7.9 s 47.8 dB -> ultrafast crf 12 4.4 s 58.7 dB).
/// AMF and QSV are as in 0.1.23 (not measurable on this PC).
fn stage_codec(enc: &str) -> &'static [&'static str] {
    match enc {
        "nvenc" => &["-c:v", "h264_nvenc", "-preset", "p1", "-tune", "hq", "-rc", "vbr", "-cq", "14", "-b:v", "0", "-bf", "0"],
        "amf" => &["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", "16", "-qp_p", "16"],
        "qsv" => &["-c:v", "h264_qsv", "-preset", "medium", "-global_quality", "16"],
        _ => &["-c:v", "libx264", "-preset", "ultrafast", "-crf", "12"],
    }
}

/// Run a hidden ffmpeg to the end, or kill it once `cancel` is set.
fn run_cancellable(mut cmd: std::process::Command, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    crate::util::track_child(&child); // ends with Glint (0.1.41)
    let err = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = e.read_to_end(&mut buf);
            buf
        })
    });
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("ffmpeg failed: {e}")),
        }
    };
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(util::last_lines(&stderr, 2))
    }
}

/// MP4 / MOV: the files that carry edit lists.
fn is_mov(p: &Path) -> bool {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).is_some_and(|e| matches!(e.as_str(), "mp4" | "mov" | "m4v"))
}

/// The time of the video frame on screen when playback starts (`origin`), on
/// the timeline of the video read WITHOUT edit-list processing: the last frame
/// at or before it, else the first frame. ffmpeg's own edit-list handling is
/// not used here because it starts the video at the first frame AFTER the
/// edit's start, which on a variable-rate clip can be a second or more late
/// (measured 1.32 s on a trim that began on a still screen).
fn frame_on_screen(ffprobe: &Path, file: &Path, origin: f64) -> Option<f64> {
    let mut cmd = util::hidden_cmd(ffprobe);
    cmd.args(["-v", "error"]);
    if is_mov(file) {
        cmd.args(["-advanced_editlist", "0"]);
    }
    // A GOP is at most a couple of seconds; 5000 packets cover it at any frame rate.
    let out = cmd.args(["-select_streams", "v:0", "-show_entries", "packet=pts_time", "-of", "csv=p=0", "-read_intervals", "%+#5000"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let pts: Vec<f64> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok()).collect();
    pts.iter().copied().filter(|p| *p <= origin + 1e-4).reduce(f64::max).or_else(|| pts.iter().copied().reduce(f64::min))
}

/// Write `out`: `input` as a correct player shows it, starting at 0 with no
/// hidden frames, video at a constant rate (the clip's nominal one; a still
/// screen's gap is filled with the frame on screen) and every audio track
/// copied untouched (titles and order kept). Only the video is re-encoded, on
/// the hardware encoder when there is one.
///
/// The video is read with edit lists off and timestamps kept (`-copyts`), the
/// frame on screen at the start is pinned there and everything before it is
/// dropped. The audio comes from a second, normal reading of the same file:
/// ffmpeg applies audio edit lists to the sample.
/// What `stage_retimed` runs ffmpeg with, worked out once per clip.
struct StagePlan {
    graph: String,
    titles: Vec<String>,
    origin: f64,
}

fn stage_plan(ffprobe: &Path, input: &Path) -> StagePlan {
    let t = probe_timing(ffprobe, input).unwrap_or_default();
    let rate = [&t.r_rate, &t.avg_rate].into_iter().find(|r| rate_of(r).is_some()).cloned().unwrap_or_else(|| "60".into());
    let origin = t.origin;
    let first = frame_on_screen(ffprobe, input, origin).unwrap_or(origin);
    // Half a millisecond early so rounding never drops the frame itself.
    let graph = format!("select='gte(t\\,{:.6})',setpts='if(eq(N\\,0)\\,{origin:.6}/TB\\,PTS)',fps={rate}", first - 0.0005);
    let (_, titles) = stream_summary(ffprobe, input);
    StagePlan { graph, titles, origin }
}

/// The ffmpeg command that writes the re-timed copy: `decode` goes before the video input,
/// `codec` holds the video encoder's arguments.
fn stage_cmd(ffmpeg: &Path, input: &Path, out: &Path, plan: &StagePlan, decode: &[&str], codec: &[&str]) -> std::process::Command {
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-copyts"]);
    if is_mov(input) {
        cmd.args(["-advanced_editlist", "0"]);
    }
    // Input options belong to the next -i only: the audio input is read normally, edit lists applied.
    cmd.args(decode).args(["-reinit_filter", "0"]).arg("-i").arg(input).arg("-i").arg(input);
    cmd.args(["-map", "0:v:0", "-map", "1:a?", "-dn", "-sn", "-vf", &plan.graph]).args(codec);
    // The fps filter made the rate constant; `-fps_mode cfr` here dropped frames next to a long gap under -copyts.
    cmd.args(["-fps_mode", "passthrough", "-c:a", "copy", "-map_metadata", "1"]);
    // ffmpeg reads an MP4 track name back as the tag "name", which it does not write out again.
    for (i, title) in plan.titles.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
        cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={title}"));
    }
    if plan.origin.abs() > 1e-6 {
        cmd.args(["-output_ts_offset", &format!("{:.6}", -plan.origin)]);
    }
    cmd.arg(out);
    cmd
}

fn stage_retimed(ffmpeg: &Path, encoder: &str, input: &Path, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("ffprobe.exe was not found")?;
    let plan = stage_plan(&ffprobe, input);
    let mut last = String::new();
    for enc in stage_encoders(encoder) {
        let cmd = stage_cmd(ffmpeg, input, out, &plan, &[], stage_codec(enc));
        match run_cancellable(cmd, cancel) {
            Ok(()) if fs::metadata(out).is_ok_and(|m| m.len() > 0) => {
                // "copy dates" in Blur takes the input's file time.
                if let Ok(m) = fs::metadata(input).and_then(|m| m.modified()) {
                    if let Ok(f) = fs::File::options().write(true).open(out) {
                        let _ = f.set_modified(m);
                    }
                }
                return Ok(());
            }
            Err(e) if e == CANCELLED => {
                let _ = fs::remove_file(out);
                return Err(e);
            }
            r => {
                last = r.err().unwrap_or_else(|| "ffmpeg wrote no file".into());
                util::log(&format!("blur: staging with {enc} failed ({last})"));
                let _ = fs::remove_file(out);
            }
        }
    }
    Err(format!("Could not prepare the clip for Blur: {last}"))
}

// ---------------------------------------------------------------------------
// Self-test: `glint.exe --blur-sync-test <report.txt>`

struct Report {
    path: PathBuf,
    failed: u32,
}

impl Report {
    fn line(&mut self, s: &str) {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{s}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if !ok {
            self.failed += 1;
        }
        let d = if detail.is_empty() { String::new() } else { format!(" ({detail})") };
        self.line(&format!("{} {name}{d}", if ok { "PASS" } else { "FAIL" }));
    }
}

/// Flash and beep instants of the test clip: irregular gaps, so an offset of
/// a whole gap cannot pass for sync. On the 144 fps grid.
fn test_events() -> Vec<f64> {
    (0..7).map(|k| ((0.3 + 0.9 * k as f64 + 0.03 * (k * k) as f64) * 144.0).round() / 144.0).collect()
}

fn ff(ffmpeg: &Path, args: &[&str]) -> Result<(), String> {
    let out = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(util::last_lines(&out.stderr, 2)) }
}

/// A clip like the replay buffer writes: 144 fps nominal, variable rate
/// (repeats dropped through a still stretch, irregular thinning elsewhere),
/// a keyframe each second, MPEG-TS stream-copied into MP4 with three AAC
/// tracks (Mix, Desktop, Microphone). White flashes and 1 kHz beeps mark the
/// same instants.
fn make_test_clip(ffmpeg: &Path, dir: &Path) -> Result<PathBuf, String> {
    let ev = test_events();
    let len = 2.0 / 144.0;
    let between = |a: f64, b: f64| ev.iter().map(|t| format!("between(t,{:.5},{:.5})", t + a, t + b)).collect::<Vec<_>>().join("+");
    let on = between(-0.0002, len - 0.0004);
    let second = between(len / 2.0 - 0.0002, len - 0.0004);
    let esc = |s: &str| s.replace(',', "\\,");
    let wav = dir.join("a.wav");
    let ts = dir.join("v.ts");
    let src = dir.join("src.mp4");
    let p = |p: &Path| p.to_string_lossy().into_owned();
    ff(ffmpeg, &["-f", "lavfi", "-i", &format!("aevalsrc='0.5*sin(2*PI*1000*t)*({on})':s=48000:d=8"), "-ac", "2", &p(&wav)])?;
    // Two flash frames that differ, so dropping repeats keeps both.
    let graph = format!(
        "[0:v][1:v]overlay=x=120:y=67:enable='lt(t\\,3)+gte(t\\,5)',drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='{}',drawbox=x=0:y=0:w=iw:h=ih:color=0xE0E0E0:t=fill:enable='{}',mpdecimate=hi=64:lo=32:frac=0.1:max=0,select='not(eq(mod(n\\,7)\\,3))+between(t\\,3\\,5)'",
        esc(&on),
        esc(&second)
    );
    ff(
        ffmpeg,
        &[
            "-f", "lavfi", "-i", "color=c=black:s=320x180:r=144:d=8", "-f", "lavfi", "-i", "testsrc2=s=80x45:r=144:d=8,lutyuv=y=val*0.4",
            "-filter_complex", &graph, "-fps_mode", "vfr", "-c:v", "libx264", "-preset", "veryfast", "-bf", "0", "-g", "288", "-x264-params", "scenecut=0",
            "-force_key_frames", "expr:gte(t,n_forced*1)", "-pix_fmt", "yuv420p", "-f", "mpegts", "-muxdelay", "0", "-muxpreload", "0", &p(&ts),
        ],
    )?;
    let (w, t, s) = (p(&wav), p(&ts), p(&src));
    ff(
        ffmpeg,
        &[
            "-dts_delta_threshold", "1000000", "-f", "mpegts", "-i", &t, "-i", &w, "-i", &w, "-i", &w, "-map", "0:v:0", "-map", "1:a", "-map", "2:a", "-map", "3:a",
            "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop", "-metadata:s:a:2", "title=Microphone",
            "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "2", "-movflags", "+faststart", &s,
        ],
    )?;
    Ok(src)
}

/// Video minus audio, in ms, for each flash in `file` (as a player shows it),
/// against the nearest beep on audio track `track`. Also the beep count.
fn av_offsets(ffmpeg: &Path, file: &Path, track: usize) -> Result<(Vec<f64>, usize), String> {
    let run = |args: Vec<String>| -> Result<String, String> {
        let out = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostats", "-nostdin", "-i"]).arg(file).args(args).args(["-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(String::from_utf8_lossy(&out.stderr).into_owned()) } else { Err(util::last_lines(&out.stderr, 2)) }
    };
    let num = |l: &str, key: &str| l.split(key).nth(1).and_then(|r| r.split_whitespace().next()).and_then(|v| v.parse::<f64>().ok());
    let log = run(["-map", "0:v:0", "-vf", "signalstats,metadata=mode=print:key=lavfi.signalstats.YAVG"].map(String::from).to_vec())?;
    let mut frames: Vec<(f64, f64)> = Vec::new();
    let mut at = None;
    for l in log.lines() {
        if let Some(t) = num(l, "pts_time:") {
            at = Some(t);
        } else if let (Some(y), Some(t)) = (num(l, "YAVG="), at.take()) {
            frames.push((t, y));
        }
    }
    let mut ys: Vec<f64> = frames.iter().map(|f| f.1).collect();
    ys.sort_by(f64::total_cmp);
    let base = ys.get(ys.len() * 3 / 10).copied().unwrap_or(0.0);
    // A flash is a run of bright frames; its instant is the brightness-weighted mean,
    // which a symmetric blur leaves in place.
    let mut flashes = Vec::new();
    let mut run_acc: Option<(f64, f64)> = None;
    for (t, y) in frames.iter().copied().chain(std::iter::once((f64::MAX, 0.0))) {
        if y > base + 30.0 {
            let w = y - base;
            let r = run_acc.get_or_insert((0.0, 0.0));
            r.0 += w;
            r.1 += w * t;
        } else if let Some((w, s)) = run_acc.take() {
            flashes.push(s / w);
        }
    }
    let beeps = beep_times(ffmpeg, file, track)?;
    let offsets = flashes.iter().filter_map(|v| beeps.iter().map(|a| (v - a) * 1000.0).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect();
    Ok((offsets, beeps.len()))
}

/// The middle of each beep on audio track `track` of `file`, in seconds as a player plays it.
fn beep_times(ffmpeg: &Path, file: &Path, track: usize) -> Result<Vec<f64>, String> {
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostats", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{track}"), "-af", "silencedetect=n=-30dB:d=0.004", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(util::last_lines(&out.stderr, 2));
    }
    let num = |l: &str, key: &str| l.split(key).nth(1).and_then(|r| r.split_whitespace().next()).and_then(|v| v.parse::<f64>().ok());
    let mut beeps = Vec::new();
    let mut began = None;
    for l in String::from_utf8_lossy(&out.stderr).lines() {
        if let Some(e) = num(l, "silence_end:") {
            began = Some(e);
        } else if let (Some(s), Some(b)) = (num(l, "silence_start:"), began) {
            beeps.push((b + s) / 2.0);
            began = None;
        }
    }
    Ok(beeps)
}

fn worst(d: &[f64]) -> f64 {
    d.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

fn show(d: &[f64]) -> String {
    d.iter().map(|v| format!("{v:+.1}")).collect::<Vec<_>>().join(" ")
}

/// (codec_type, duration) for each stream, plus the audio tracks' titles ("" = none).
fn stream_summary(ffprobe: &Path, file: &Path) -> (Vec<(String, f64)>, Vec<String>) {
    let out = util::hidden_cmd(ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type,duration:stream_tags=title,name", "-of", "flat"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut map: std::collections::BTreeMap<u32, (String, f64, String)> = Default::default();
    for line in out.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"').to_string();
        let parts: Vec<&str> = key.split('.').collect();
        let Some(n) = parts.get(2).and_then(|n| n.parse::<u32>().ok()) else { continue };
        let e = map.entry(n).or_default();
        match parts.get(3..).unwrap_or_default() {
            ["codec_type"] => e.0 = value,
            ["duration"] => e.1 = value.parse().unwrap_or(0.0),
            ["tags", "title"] | ["tags", "name"] => e.2 = value,
            _ => {}
        }
    }
    (map.values().map(|(c, d, _)| (c.clone(), *d)).collect(), map.values().filter(|e| e.0 == "audio").map(|e| e.2.clone()).collect())
}

pub fn sync_self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    let dir = util::tmp_dir().join(format!("blur-sync-test-{}", util::new_id()));
    let _ = fs::create_dir_all(&dir);
    let started = Instant::now();
    run_sync_test(&mut r, &dir);
    if std::env::var_os("GLINT_KEEP_TEST_FILES").is_none() {
        let _ = fs::remove_dir_all(&dir);
    } else {
        r.line(&format!("KEPT {}", dir.display()));
    }
    let failed = r.failed;
    r.line(&format!("DONE failed={failed} secs={:.1}", started.elapsed().as_secs_f64()));
}

fn run_sync_test(r: &mut Report, dir: &Path) {
    let s = Settings::load();
    let (Some(ffmpeg), Some(ffprobe)) = (util::find_exe(&s.ffmpeg_path, "ffmpeg"), util::find_exe("", "ffprobe")) else {
        r.check("ffmpeg and ffprobe found", false, "");
        return;
    };
    let src = match make_test_clip(&ffmpeg, dir) {
        Ok(p) => p,
        Err(e) => return r.check("test clip made", false, &e),
    };
    let n_events = test_events().len();
    match av_offsets(&ffmpeg, &src, 0) {
        Ok((d, beeps)) => r.check("test clip: every flash has its beep, in sync", d.len() == n_events && beeps == n_events && worst(&d) < 12.0, &show(&d)),
        Err(e) => r.check("test clip measured", false, &e),
    }
    r.check("a clip straight from the buffer (VFR) goes to Blur untouched", sync_issue(&ffprobe, &src).is_none(), &format!("{:?}", sync_issue(&ffprobe, &src)));

    // The editor's "trim only" export and the library's Trim: a stream copy
    // that starts between keyframes (keyframes each second), on a still
    // screen: the frame on screen at 3.4 s was sent at 3.28 s, the next at 4.38 s.
    let meta = library::register(&ffmpeg, &src, "sync test", "Desktop", "clip");
    let (a, b) = (3.4, 7.6);
    let expect = test_events().iter().filter(|t| **t > a + 0.01 && **t < b - 0.03).count();
    let trimmed = match crate::clip::trim(&s, &meta, a, b, true) {
        Ok(m) => m,
        Err(e) => return r.check("lossless trim", false, &e),
    };
    let tpath = PathBuf::from(&trimmed.path);
    let issue = sync_issue(&ffprobe, &tpath);
    r.check("a trim cut between keyframes is detected", issue.is_some(), "");
    // For information: ffmpeg's own reading of such a trim (what Glint's
    // re-encoding paths see) starts the video at the first frame after the cut.
    let (in_trim, _) = av_offsets(&ffmpeg, &tpath, 0).unwrap_or_default();
    r.line(&format!("INFO the trim as ffmpeg reads it, video minus audio in ms: {}", show(&in_trim)));
    // The frame on screen at the cut is re-timed to the cut (`clip::trim_start`),
    // so ffmpeg's reading is in step too; it was 985 ms early here when the cut
    // started between frames as it was.
    r.check("a trim read by ffmpeg (re-encodes of it) is in sync", !in_trim.is_empty() && worst(&in_trim) < 40.0, &show(&in_trim));
    // ... and the cut starts at the asked time, not on the frame 120 ms before
    // it (0.1.12), so no sound the user cut away comes back.
    // Each beep's place against its instant, in ms; the source's own figure
    // (how the measure reads a beep) is taken off.
    let ev = test_events();
    let late = |file: &Path, shift: f64| -> Vec<f64> {
        let beeps = beep_times(&ffmpeg, file, 0).unwrap_or_default();
        beeps.iter().filter_map(|bt| ev.iter().map(|e| (bt - (e - shift)) * 1000.0).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect()
    };
    let own = late(&src, 0.0);
    let bias = if own.is_empty() { 0.0 } else { own.iter().sum::<f64>() / own.len() as f64 };
    let early: Vec<f64> = late(&tpath, a).iter().map(|v| v - bias).collect();
    let bound = crate::clip::TRIM_EARLY_MAX * 1000.0;
    let ok = early.len() == expect && early.iter().all(|e| *e < bound && *e > -12.0);
    r.check(&format!("a trim starts no more than {bound:.0} ms before the asked time"), ok, &format!("ms early, per beep: {} (measure's own offset {bias:+.1} taken off)", show(&early)));
    let len = trimmed.duration;
    r.check("a trim is as long as asked", (len - (b - a)).abs() < 0.03, &format!("{len:.3} s for {:.3} s", b - a));

    // What Blur is handed instead.
    let staged = dir.join("staged.mp4");
    let t0 = Instant::now();
    let res = stage_retimed(&ffmpeg, &s.encoder, &tpath, &staged, &AtomicBool::new(false));
    r.check("re-timed copy written", res.is_ok(), &format!("{res:?} in {:.1}s", t0.elapsed().as_secs_f64()));
    if res.is_ok() {
        let t = probe_timing(&ffprobe, &staged).unwrap_or_default();
        r.check("re-timed copy: video starts at 0, nothing hidden", t.video_start.is_some_and(|v| v.abs() < 0.001) && !t.preroll, &format!("{t:?}"));
        r.check("re-timed copy: every audio track starts at 0", t.audio_starts.len() == 3 && t.audio_starts.iter().all(|a| a.abs() < 0.001), &format!("{:?}", t.audio_starts));
        let (rr, ar) = (rate_of(&t.r_rate).unwrap_or(0.0), rate_of(&t.avg_rate).unwrap_or(0.0));
        r.check("re-timed copy: constant 144 fps", (rr - 144.0).abs() < 0.01 && (ar - 144.0).abs() < 0.5, &format!("r {} avg {}", t.r_rate, t.avg_rate));
        r.check("re-timed copy needs no further fixing", sync_issue(&ffprobe, &staged).is_none(), "");
        let (streams, titles) = stream_summary(&ffprobe, &staged);
        r.check("re-timed copy keeps the three tracks and their names", titles == ["Mix", "Desktop", "Microphone"], &format!("{titles:?}"));
        let vd = streams.iter().find(|s| s.0 == "video").map(|s| s.1).unwrap_or(0.0);
        let ad = streams.iter().find(|s| s.0 == "audio").map(|s| s.1).unwrap_or(0.0);
        r.check("re-timed copy: video and audio equally long", (vd - ad).abs() < 0.03 && (vd - trimmed.duration).abs() < 0.05, &format!("video {vd:.3} audio {ad:.3} trim {:.3}", trimmed.duration));
        for track in [0, 2] {
            let (d, _) = av_offsets(&ffmpeg, &staged, track).unwrap_or_default();
            r.check(&format!("re-timed copy in sync (track {})", track + 1), d.len() == expect && worst(&d) < 12.0, &show(&d));
        }
    }

    // A re-timed copy that cannot be made (here: no ffmpeg where it should be):
    // Blur is handed the clip as it is and the render still happens. A cancel still cancels.
    let job = dir.join("job-fallback");
    let _ = fs::create_dir_all(&job);
    let bogus = dir.join("missing").join("ffmpeg.exe");
    let res = stage_input(Some(&bogus), "x264", Some("test".into()), &tpath, "mp4", &job, &AtomicBool::new(false));
    let same = res.as_ref().ok().and_then(|p| fs::metadata(p).ok()).map(|m| m.len()) == fs::metadata(&tpath).ok().map(|m| m.len());
    r.check("a failed re-timed copy falls back to the clip as it is", res.is_ok() && same, &format!("{res:?}"));
    let job = dir.join("job-cancel");
    let _ = fs::create_dir_all(&job);
    let res = stage_input(Some(&ffmpeg), &s.encoder, Some("test".into()), &tpath, "mp4", &job, &AtomicBool::new(true));
    r.check("a cancel while re-timing still cancels", res.as_ref().err().is_some_and(|e| e == CANCELLED), &format!("{res:?}"));

    // An editor export that re-encodes (two ranges): already clean, handed over untouched.
    let opts: Result<crate::editor::ExportOpts, _> = serde_json::from_value(serde_json::json!({ "speed": 1.0, "segments": [{ "start": 0.5, "end": 2.9 }, { "start": 4.1, "end": 7.6 }] }));
    match opts.map_err(|e| e.to_string()).and_then(|o| crate::editor::export(&s, &meta, &o, &mut |_| {})) {
        Ok(m) => {
            let p = PathBuf::from(&m.path);
            let issue = sync_issue(&ffprobe, &p);
            r.check("a re-encoded editor export goes to Blur untouched", issue.is_none(), &format!("{issue:?}"));
            let (d, _) = av_offsets(&ffmpeg, &p, 0).unwrap_or_default();
            r.check("the re-encoded export plays in sync", d.len() >= 4 && worst(&d) < 12.0, &show(&d));
        }
        Err(e) => r.check("editor export", false, &e),
    }

    // The real thing, when Blur is installed.
    let Some(cli) = find(&s) else {
        r.line("SKIP blur-cli.exe not found: the render through Blur was not checked");
        return;
    };
    let cfg = BlurSettings::default();
    let out = dir.join("blurred.mp4");
    let t0 = Instant::now();
    let res = render(&cli, &tpath, &out, &cfg, |_| {});
    r.check("Blur renders the trim", res.is_ok(), &format!("{res:?} in {:.1}s", t0.elapsed().as_secs_f64()));
    if res.is_ok() {
        for track in [0, 2] {
            let (d, beeps) = av_offsets(&ffmpeg, &out, track).unwrap_or_default();
            r.check(&format!("Blur's result in sync (track {})", track + 1), d.len() == expect && beeps == expect && worst(&d) < 25.0, &show(&d));
        }
        let (streams, _) = stream_summary(&ffprobe, &out);
        let vd = streams.iter().find(|s| s.0 == "video").map(|s| s.1).unwrap_or(0.0);
        let ad = streams.iter().find(|s| s.0 == "audio").map(|s| s.1).unwrap_or(0.0);
        r.check("Blur's result: video as long as the audio", (vd - ad).abs() < 0.05, &format!("video {vd:.3} audio {ad:.3}"));
        r.check("Blur's result keeps three audio tracks", streams.iter().filter(|s| s.0 == "audio").count() == 3, "");
    }
    render_sync_checks(r, &ffmpeg, &ffprobe, &cli, &s, dir);
}

/// Per frame of `file`: (time, mean grey of a 16x16 reduction).
fn frame_lumas(ffmpeg: &Path, file: &Path) -> Vec<(f64, f64)> {
    let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-map", "0:v:0", "-vf", "scale=16:16,format=gray,showinfo", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stderr).lines().filter_map(|l| {
        let p = l.find("pts_time:")?;
        let m = l.find("mean:[")?;
        Some((l[p + 9..].split_whitespace().next()?.parse().ok()?, l[m + 6..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?))
    }).collect()
}

/// Picture minus sound (seconds) at each beep of `file`: the flash's centre is the brightness-weighted centre of
/// the frames within 0.25 s of the beep (Blur's blending smears one flash over its neighbours: its centre is
/// what the eye takes as the moment). Positive = the picture is late.
fn smear_offsets(ffmpeg: &Path, file: &Path) -> Vec<(f64, f64)> {
    let frames = frame_lumas(ffmpeg, file);
    // The beep's onset (where silence ends), not its middle: `beep_times` gives the middle, 30 ms and more
    // into a 60 ms beep, which read as the picture 38 ms early.
    let beeps: Vec<f64> = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostats", "-nostdin", "-i"]).arg(file)
        .args(["-map", "0:a:0", "-af", "silencedetect=n=-30dB:d=0.004", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output()
        .map(|o| String::from_utf8_lossy(&o.stderr).lines().filter_map(|l| l.split("silence_end:").nth(1)?.split_whitespace().next()?.parse().ok()).collect())
        .unwrap_or_default();
    let mut sorted: Vec<f64> = frames.iter().map(|f| f.1).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let base = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    beeps.iter().filter_map(|&b| {
        let near: Vec<&(f64, f64)> = frames.iter().filter(|f| (f.0 - b).abs() < 0.25 && f.1 > base + 3.0).collect();
        let w: f64 = near.iter().map(|f| f.1 - base).sum();
        (w > 0.0).then(|| (b, near.iter().map(|f| f.0 * (f.1 - base)).sum::<f64>() / w - b))
    }).collect()
}

/// 0.1.41, a user's report ("when I render some of my videos the gameplay is delayed against the sound"): the
/// real Blur render path (`render`: Glint's hand-off, staging included) on clips with a flash and a beep each
/// second: 60 and 30 fps, a variable frame rate, sound longer than the picture, a trim that starts between
/// keyframes, Glint's own buffer clip; with the owner's Blur settings when they can be read, the defaults,
/// interpolation on and off, and an output frame rate other than the source's. Each: the offsets at the start,
/// the middle and the end (a growing one = a rate problem, a constant one = a start problem).
fn render_sync_checks(r: &mut Report, ffmpeg: &Path, ffprobe: &Path, cli: &Path, s: &Settings, dir: &Path) {
    let _ = ffprobe;
    // The owner's own settings, read only (never written).
    let owner: Option<BlurSettings> = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("Glint").join("settings.json"))
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .map(|x| x.blur);
    let mut cfgs: Vec<(&str, BlurSettings)> = Vec::new();
    if let Some(o) = owner.clone() {
        cfgs.push(("the owner's settings", o));
    } else {
        r.line("INFO the owner's Blur settings could not be read: the defaults only");
    }
    let d = BlurSettings::default();
    cfgs.push(("defaults", d.clone()));
    let base = owner.unwrap_or_else(|| d.clone());
    cfgs.push(("interpolation on", BlurSettings { interpolate: true, ..base.clone() }));
    cfgs.push(("interpolation off", BlurSettings { interpolate: false, ..base.clone() }));
    cfgs.push(("output 30 fps from a 60 fps source", BlurSettings { output_fps: 30, ..base.clone() }));
    // The clips.
    let mut clips: Vec<(String, PathBuf, f64)> = Vec::new();
    for (name, fps, vfr, extra) in [("60 fps", 60u32, false, 0.0), ("30 fps", 30, false, 0.0), ("variable rate", 60, true, 0.0), ("sound longer than the picture", 60, false, 0.7)] {
        let p = dir.join(format!("rsync {name}.mp4"));
        if crate::editor::flash_clip(ffmpeg, &p, fps, 8.0, vfr, extra) {
            clips.push((name.to_string(), p, fps as f64));
        }
    }
    // A trim that starts between keyframes (an edit list hides the first frames): Glint's lossless cut of the 60 fps clip.
    if let Some((_, p60, _)) = clips.first().cloned() {
        let meta = library::register(ffmpeg, &p60, "rsync trim", "Desktop", "clip");
        let ts = Settings { clips_dir: dir.to_string_lossy().into_owned(), game_folders: false, ..s.clone() };
        match crate::clip::trim(&ts, &meta, 2.37, 7.9, true) {
            Ok(m) => clips.push(("a trim starting between keyframes".into(), PathBuf::from(m.path), 60.0)),
            Err(e) => r.line(&format!("INFO the trim was not made: {e}")),
        }
    }
    if let Ok(p) = make_test_clip(ffmpeg, &dir.join("rsync-buffer")).or_else(|_| make_test_clip(ffmpeg, dir)) {
        clips.push(("Glint's own buffer clip (144 fps, VFR)".into(), p, 144.0));
    }
    r.check("Blur render sync: the test clips were made", clips.len() >= 6, &format!("{} of 6", clips.len()));
    for (cname, cfg) in &cfgs {
        for (name, file, fps) in &clips {
            if *cname == "output 30 fps from a 60 fps source" && (*fps - 60.0).abs() > 0.5 {
                continue;
            }
            let out = dir.join(format!("rsync out {cname} {name}.mp4").replace(' ', "_"));
            let t0 = Instant::now();
            let res = render(cli, file, &out, cfg, |_| {});
            if let Err(e) = res {
                r.check(&format!("Blur render ({cname}): {name}"), false, &e);
                continue;
            }
            let offs = smear_offsets(ffmpeg, &out);
            let frame = 1.0 / (if cfg.blur { cfg.output_fps.max(1) as f64 } else { *fps });
            let pick = |k: usize| offs.get(k).map(|x| x.1 * 1000.0).unwrap_or(f64::NAN);
            let (a, m, z) = (pick(0), pick(offs.len() / 2), pick(offs.len().saturating_sub(1)));
            let worst = offs.iter().fold(0.0f64, |w, x| w.max(x.1.abs()));
            let ok = offs.len() >= 4 && worst <= frame + 0.004;
            r.check(
                &format!("Blur render ({cname}), {name}: the flash's centre on its beep within a frame, start to end"),
                ok,
                &format!("{} beeps; start {a:+.1} ms, middle {m:+.1} ms, end {z:+.1} ms ({:+.2} / {:+.2} / {:+.2} frames at {:.0} fps) in {:.0} s", offs.len(), a / 1000.0 / frame, m / 1000.0 / frame, z / 1000.0 / frame, 1.0 / frame, t0.elapsed().as_secs_f64()),
            );
            let _ = fs::remove_file(&out);
        }
    }
}

// ---------------------------------------------------------------------------
// Self-test: `glint.exe --blur-start-test <report.txt>`: where the wait before
// Blur's first percent goes, on a short synthetic 1080p clip.

/// One blur-cli run with a time on every line it prints: (lines as (seconds, text), seconds to the
/// first "% complete", seconds in all, ok).
fn timed_blur(blur_cli: &Path, input: &Path, output: &Path) -> (Vec<(f64, String)>, Option<f64>, f64, bool) {
    let t0 = Instant::now();
    let child = util::hidden_cmd(blur_cli).arg("-i").arg(input).arg("-o").arg(output).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return (vec![(0.0, "could not start blur-cli".into())], None, 0.0, false) };
    let (tx, rx) = std::sync::mpsc::channel::<(f64, String)>();
    let pipes: Vec<Box<dyn Read + Send>> = [child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>)].into_iter().flatten().collect();
    for (k, mut pipe) in pipes.into_iter().enumerate() {
        let tx = tx.clone();
        let tag = if k == 0 { "out" } else { "err" };
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut line = String::new();
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for ch in String::from_utf8_lossy(&buf[..n]).chars() {
                    if ch == '\n' || ch == '\r' {
                        if !line.trim().is_empty() {
                            let _ = tx.send((t0.elapsed().as_secs_f64(), format!("[{tag}] {}", line.trim())));
                        }
                        line.clear();
                    } else {
                        line.push(ch);
                    }
                }
            }
        });
    }
    drop(tx);
    let mut lines = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(l) => lines.push(l),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                kill_tree(child.id());
                lines.push((t0.elapsed().as_secs_f64(), "killed: no output for 120 s".into()));
                break;
            }
        }
    }
    let ok = child.wait().is_ok_and(|s| s.success()) && output.is_file();
    let first = lines.iter().find(|l| l.1.contains("% complete")).map(|l| l.0);
    (lines, first, t0.elapsed().as_secs_f64(), ok)
}

/// `vspipe --info` on a script that only opens `file` the way Blur's script does (BestSource, no
/// cache): how long Blur's reading-in of the clip takes before any frame is made.
fn time_index(blur_cli: &Path, file: &Path, dir: &Path) -> Result<f64, String> {
    let vspipe = blur_cli.parent().ok_or("no folder")?.join("lib").join("vapoursynth").join("vspipe.exe");
    if !vspipe.is_file() {
        return Err(format!("{} not found", vspipe.display()));
    }
    let script = dir.join("index.vpy");
    let src = file.to_string_lossy().replace('\\', "/");
    fs::write(&script, format!("from vapoursynth import core\ncore.bs.VideoSource(source=r\"{src}\", cachemode=0).set_output()\n")).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let out = util::hidden_cmd(&vspipe).arg("--info").arg(&script).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    let secs = t0.elapsed().as_secs_f64();
    if out.status.success() { Ok(secs) } else { Err(util::last_lines(&out.stderr, 3)) }
}

/// PSNR (dB, "inf" as 99) of `a` against `b`, frame by frame.
fn psnr(ffmpeg: &Path, a: &Path, b: &Path) -> Option<f64> {
    let out = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostats", "-nostdin", "-i"]).arg(a).arg("-i").arg(b).args(["-lavfi", "[0:v][1:v]psnr", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    let v = text.lines().rev().find_map(|l| l.split("average:").nth(1).and_then(|r| r.split_whitespace().next()).map(str::to_string))?;
    if v == "inf" { Some(99.0) } else { v.parse().ok() }
}

/// The re-timed copy made several ways: time, and quality against a lossless one.
fn stage_variants(r: &mut Report, ffmpeg: &Path, ffprobe: &Path, input: &Path, dir: &Path) {
    let plan = stage_plan(ffprobe, input);
    let reference = dir.join("ref.mp4");
    let t0 = Instant::now();
    let ok = run_cancellable(stage_cmd(ffmpeg, input, &reference, &plan, &[], &["-c:v", "libx264", "-preset", "ultrafast", "-qp", "0"]), &AtomicBool::new(false));
    r.line(&format!("TIME lossless reference: {:.2} s ({ok:?})", t0.elapsed().as_secs_f64()));
    let t0 = Instant::now();
    let _ = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"]).arg(input).args(["-map", "0:v:0", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    r.line(&format!("TIME decode only (software): {:.2} s", t0.elapsed().as_secs_f64()));
    let t0 = Instant::now();
    let _ = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-hwaccel", "cuda", "-i"]).arg(input).args(["-map", "0:v:0", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    r.line(&format!("TIME decode only (cuda): {:.2} s", t0.elapsed().as_secs_f64()));
    let nv = |preset: &'static str, tune: &'static str| -> Vec<&'static str> { vec!["-c:v", "h264_nvenc", "-preset", preset, "-tune", tune, "-rc", "vbr", "-cq", "16", "-b:v", "0", "-bf", "0"] };
    let variants: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("nvenc p4 hq (0.1.23)", vec![], nv("p4", "hq")),
        ("nvenc p1 hq", vec![], nv("p1", "hq")),
        ("nvenc p1 hq cq 14", vec![], vec!["-c:v", "h264_nvenc", "-preset", "p1", "-tune", "hq", "-rc", "vbr", "-cq", "14", "-b:v", "0", "-bf", "0"]),
        ("x264 veryfast crf 14 (0.1.23)", vec![], vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "14"]),
        ("x264 ultrafast crf 12", vec![], vec!["-c:v", "libx264", "-preset", "ultrafast", "-crf", "12"]),
        ("x264 ultrafast crf 10", vec![], vec!["-c:v", "libx264", "-preset", "ultrafast", "-crf", "10"]),
    ];
    for (k, (name, decode, codec)) in variants.iter().enumerate() {
        let out = dir.join(format!("v{k}.mp4"));
        let t0 = Instant::now();
        let res = run_cancellable(stage_cmd(ffmpeg, input, &out, &plan, decode, codec), &AtomicBool::new(false));
        let secs = t0.elapsed().as_secs_f64();
        let q = psnr(ffmpeg, &out, &reference).map(|v| format!("{v:.2} dB")).unwrap_or("?".into());
        let mb = fs::metadata(&out).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
        let synced = res.is_ok() && sync_issue(ffprobe, &out).is_none();
        r.line(&format!("TIME stage {name}: {secs:.2} s, PSNR {q}, {mb:.1} MB, clean {synced} {}", res.err().unwrap_or_default()));
    }
}

pub fn start_self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    let dir = util::tmp_dir().join(format!("blur-start-test-{}", util::new_id()));
    let _ = fs::create_dir_all(&dir);
    let started = Instant::now();
    run_start_test(&mut r, &dir);
    if std::env::var_os("GLINT_KEEP_TEST_FILES").is_none() {
        let _ = fs::remove_dir_all(&dir);
    }
    let failed = r.failed;
    r.line(&format!("DONE failed={failed} secs={:.1}", started.elapsed().as_secs_f64()));
}

fn run_start_test(r: &mut Report, dir: &Path) {
    for (line, want) in [("12.3% complete (15/131, 29.07 fps)", Some(12.3)), ("100.0% complete (131/131)", Some(100.0)), ("0.8% complete (1/131)", Some(0.8)), ("Rendering 'in'", None), ("render finished in 4.73s", None)] {
        let got = percent_of(line);
        r.check(&format!("percent read from {line:?}"), got.zip(want).map_or(got.is_none() && want.is_none(), |(g, w)| (g - w).abs() < 1e-4), &format!("{got:?}"));
    }
    for (name, ok, detail) in console::self_checks() {
        r.check(&name, ok, &detail);
    }
    let s = Settings::load();
    let (Some(ffmpeg), Some(ffprobe)) = (util::find_exe(&s.ffmpeg_path, "ffmpeg"), util::find_exe("", "ffprobe")) else {
        return r.check("ffmpeg and ffprobe found", false, "");
    };
    let secs = std::env::var("GLINT_BLUR_START_SECS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(6.0).clamp(2.0, 12.0);
    // 1080p at 144 fps, a keyframe each second, three AAC tracks: like a buffer clip, short.
    let src = dir.join("src.mp4");
    let t0 = Instant::now();
    let p = |p: &Path| p.to_string_lossy().into_owned();
    let made = ff(
        &ffmpeg,
        &[
            "-f", "lavfi", "-i", &format!("testsrc2=s=1920x1080:r=144:d={secs}"), "-f", "lavfi", "-i", &format!("sine=f=1000:d={secs}:sample_rate=48000"),
            "-map", "0:v", "-map", "1:a", "-map", "1:a", "-map", "1:a", "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop", "-metadata:s:a:2", "title=Microphone",
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-bf", "0", "-g", "144", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-ac", "2", &p(&src),
        ],
    );
    if let Err(e) = made {
        return r.check("test clip made", false, &e);
    }
    r.line(&format!("INFO test clip: 1920x1080 144 fps, {secs:.1} s, made in {:.1} s", t0.elapsed().as_secs_f64()));
    let meta = library::register(&ffmpeg, &src, "start test", "Desktop", "clip");
    let trimmed = match crate::clip::trim(&s, &meta, 1.4, secs - 0.4, true) {
        Ok(m) => PathBuf::from(m.path),
        Err(e) => return r.check("lossless trim", false, &e),
    };

    // The timing probe, on a clip that needs nothing and on a trim that needs re-timing.
    for (name, file) in [("buffer clip", &src), ("lossless trim", &trimmed)] {
        let t0 = Instant::now();
        let issue = sync_issue(&ffprobe, file);
        r.line(&format!("TIME probe ({name}): {:.0} ms -> {issue:?}", t0.elapsed().as_secs_f64() * 1000.0));
    }
    // The re-timed copy.
    let staged = dir.join("staged.mp4");
    let t0 = Instant::now();
    let res = stage_retimed(&ffmpeg, &s.encoder, &trimmed, &staged, &AtomicBool::new(false));
    let stage_secs = t0.elapsed().as_secs_f64();
    r.check("re-timed copy written", res.is_ok(), &format!("{res:?}"));
    r.line(&format!("TIME re-timed copy of a {:.1} s trim: {stage_secs:.2} s (encoder setting {:?})", secs - 1.8, s.encoder));
    r.check("re-timed copy needs no further fixing", res.is_ok() && sync_issue(&ffprobe, &staged).is_none(), "");
    if std::env::var_os("GLINT_BLUR_START_VARIANTS").is_some() {
        stage_variants(r, &ffmpeg, &ffprobe, &trimmed, dir);
        return; // measuring the staging only: no blur-cli run
    }

    let Some(cli) = find(&s) else {
        r.line("SKIP blur-cli.exe not found: Blur's own start was not measured");
        return;
    };
    match time_index(&cli, &staged, dir) {
        Ok(t) => r.line(&format!("TIME Blur's reading-in of the clip (vspipe, BestSource index): {t:.2} s")),
        Err(e) => r.line(&format!("INFO vspipe index not timed: {e}")),
    }
    let hw = crate::win::hardware();
    let present: Vec<&str> = hw.gpus.iter().map(|g| g.vendor.as_str()).collect();
    // The way Glint runs it (a console without a window), and with GLINT_BLUR_START_PIPE also the 0.1.23 way (a pipe).
    let mut runs = vec!["console"];
    if std::env::var_os("GLINT_BLUR_START_PIPE").is_some() {
        runs.insert(0, "pipe");
    }
    r.check("console: blur-cli can be given a console (Glint has none of its own)", console::usable(), "");
    for name in runs {
        let job = dir.join(format!("job-{name}"));
        let _ = fs::create_dir_all(&job);
        let link = job.join("in.mp4");
        let _ = fs::hard_link(&staged, &link).or_else(|_| fs::copy(&staged, &link).map(|_| ()));
        let _ = fs::write(job.join(".blur-config.cfg"), config_text(&for_this_pc(&BlurSettings::default(), &present)));
        let out = job.join("out.mp4");
        let (lines, first, total, ok) = if name == "pipe" {
            timed_blur(&cli, &link, &out)
        } else {
            let t0 = Instant::now();
            let lines = std::cell::RefCell::new(Vec::<(f64, String)>::new());
            let mut first = None;
            let mut pcts = Vec::new();
            let res = run_blur_seen(&cli, &link, &out, &AtomicBool::new(false), &mut |p| {
                first.get_or_insert(t0.elapsed().as_secs_f64());
                pcts.push(p);
            }, &mut |row| lines.borrow_mut().push((t0.elapsed().as_secs_f64(), row.to_string())));
            let rising = pcts.windows(2).all(|w| w[1] >= w[0]);
            r.check("console: the percent only goes up", rising && !pcts.is_empty(), &format!("{} values", pcts.len()));
            (lines.into_inner(), first, t0.elapsed().as_secs_f64(), res.is_ok() && out.is_file())
        };
        r.check(&format!("blur-cli ({name}) renders"), ok, "");
        for (at, l) in lines.iter().filter(|l| !l.1.contains("% complete")).take(40) {
            r.line(&format!("  {at:7.2} s  {l}"));
        }
        let pcts: Vec<&(f64, String)> = lines.iter().filter(|l| l.1.contains("% complete")).collect();
        for (at, l) in pcts.iter().take(3).chain(pcts.last()) {
            r.line(&format!("  {at:7.2} s  {l}"));
        }
        // When the percent lines arrive: in bursts (a full pipe buffer at a time) or one by one.
        let mut bursts: Vec<(f64, usize)> = Vec::new();
        for (at, _) in &pcts {
            match bursts.last_mut() {
                Some(b) if at - b.0 < 0.05 => b.1 += 1,
                _ => bursts.push((*at, 1)),
            }
        }
        r.line(&format!("INFO {} percent lines arrived in {} bursts: {}", pcts.len(), bursts.len(), bursts.iter().take(12).map(|b| format!("{:.2}s x{}", b.0, b.1)).collect::<Vec<_>>().join(", ")));
        r.line(&format!("TIME blur-cli ({name}): first percent at {}, done at {total:.2} s", first.map(|f| format!("{f:.2} s")).unwrap_or("never".into())));
    }
}
