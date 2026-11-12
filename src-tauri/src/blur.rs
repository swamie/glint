//! Wrapper around f0e's `blur` (frame-blending motion blur). Glint writes a
//! config from its own settings and drives `blur-cli`, reporting progress.
//! The config layout is blur v2's (checked against 2.23).
use crate::settings::{BlurSettings, Settings};
use crate::util;
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
pub fn render_cancellable(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    let job = util::tmp_dir().join(format!("blur-{}", util::new_id()));
    fs::create_dir_all(&job).map_err(|e| format!("Cannot create blur job folder: {e}"))?;
    let result = render_in(blur_cli, input, output, cfg, &job, cancel, &mut progress);
    let _ = fs::remove_dir_all(&job);
    result
}

fn render_in(blur_cli: &Path, input: &Path, output: &Path, cfg: &BlurSettings, job: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(f32)) -> Result<(), String> {
    let ext = input.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    let link = job.join(format!("in.{ext}"));
    if fs::hard_link(input, &link).is_err() {
        // Different drive: fall back to a copy.
        fs::copy(input, &link).map_err(|e| format!("Cannot stage the clip for Blur: {e}"))?;
    }
    fs::write(job.join(".blur-config.cfg"), config_text(cfg)).map_err(|e| format!("Cannot write blur config: {e}"))?;

    let mut child = util::hidden_cmd(blur_cli)
        .arg("-i")
        .arg(&link)
        .arg("-o")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start blur-cli: {e}"))?;

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
