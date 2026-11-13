//! Wrapper around f0e's `blur` (frame-blending motion blur). Glint writes a
//! config from its own settings and drives `blur-cli`, reporting progress.
//! The config layout is blur v2's (checked against 2.23).
use crate::library;
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
    let s = Settings::load();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg");
    let issue = ffmpeg.as_deref().and_then(library::ffprobe_for).and_then(|p| sync_issue(&p, input));
    let link = if let (Some(why), Some(ffmpeg)) = (issue, ffmpeg.as_deref()) {
        // Blur would read this file's timing differently from every player
        // (see `sync_issue`): hand it a clean copy instead.
        util::log(&format!("blur: {why}; giving Blur a re-timed copy"));
        let staged = job.join("in.mp4");
        stage_retimed(ffmpeg, &s.encoder, input, &staged, cancel)?;
        staged
    } else {
        let link = job.join(format!("in.{ext}"));
        if fs::hard_link(input, &link).is_err() {
            // Different drive: fall back to a copy.
            fs::copy(input, &link).map_err(|e| format!("Cannot stage the clip for Blur: {e}"))?;
        }
        link
    };
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
/// generation must not show.
fn stage_codec(enc: &str) -> &'static [&'static str] {
    match enc {
        "nvenc" => &["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", "16", "-b:v", "0", "-bf", "0"],
        "amf" => &["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", "16", "-qp_p", "16"],
        "qsv" => &["-c:v", "h264_qsv", "-preset", "medium", "-global_quality", "16"],
        _ => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "14"],
    }
}

/// Run a hidden ffmpeg to the end, or kill it once `cancel` is set.
fn run_cancellable(mut cmd: std::process::Command, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
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
fn stage_retimed(ffmpeg: &Path, encoder: &str, input: &Path, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("ffprobe.exe was not found")?;
    let t = probe_timing(&ffprobe, input).unwrap_or_default();
    let rate = [&t.r_rate, &t.avg_rate].into_iter().find(|r| rate_of(r).is_some()).cloned().unwrap_or_else(|| "60".into());
    let origin = t.origin;
    let first = frame_on_screen(&ffprobe, input, origin).unwrap_or(origin);
    // Half a millisecond early so rounding never drops the frame itself.
    let graph = format!("select='gte(t\\,{:.6})',setpts='if(eq(N\\,0)\\,{origin:.6}/TB\\,PTS)',fps={rate}", first - 0.0005);
    let (_, titles) = stream_summary(&ffprobe, input);
    let mut last = String::new();
    for enc in stage_encoders(encoder) {
        let mut cmd = util::hidden_cmd(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-copyts"]);
        if is_mov(input) {
            cmd.args(["-advanced_editlist", "0"]);
        }
        cmd.arg("-i").arg(input).arg("-i").arg(input);
        cmd.args(["-map", "0:v:0", "-map", "1:a?", "-dn", "-sn", "-vf", &graph]).args(stage_codec(enc));
        // The fps filter made the rate constant; `-fps_mode cfr` here dropped frames next to a long gap under -copyts.
        cmd.args(["-fps_mode", "passthrough", "-c:a", "copy", "-map_metadata", "1"]);
        // ffmpeg reads an MP4 track name back as the tag "name", which it does not write out again.
        for (i, title) in titles.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
            cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={title}"));
        }
        if origin.abs() > 1e-6 {
            cmd.args(["-output_ts_offset", &format!("{:.6}", -origin)]);
        }
        cmd.arg(out);
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
    let log = run(vec!["-map".into(), format!("0:a:{track}"), "-af".into(), "silencedetect=n=-30dB:d=0.004".into()])?;
    let mut beeps = Vec::new();
    let mut began = None;
    for l in log.lines() {
        if let Some(e) = num(l, "silence_end:") {
            began = Some(e);
        } else if let (Some(s), Some(b)) = (num(l, "silence_start:"), began) {
            beeps.push((b + s) / 2.0);
            began = None;
        }
    }
    let offsets = flashes.iter().filter_map(|v| beeps.iter().map(|a| (v - a) * 1000.0).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect();
    Ok((offsets, beeps.len()))
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
    // The cut starts on the frame on screen (`clip::trim_start`), so ffmpeg's
    // reading is in step too; it was 985 ms early here when it started between frames.
    r.check("a trim read by ffmpeg (re-encodes of it) is in sync", !in_trim.is_empty() && worst(&in_trim) < 40.0, &show(&in_trim));

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
}
