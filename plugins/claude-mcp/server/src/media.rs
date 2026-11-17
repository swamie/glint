//! What lets Claude "see" and "hear" a clip, with ffmpeg and (optionally) whisper.cpp on this computer.
//! Nothing here leaves the computer.
use crate::folders::{self, display, plugin_dir};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn cmd(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // no console window
    }
    c.stdin(Stdio::null());
    c
}

fn exists_exe(p: &Path) -> bool {
    p.is_file()
}

/// ffmpeg and ffprobe: config.json's "ffmpeg", else the ones Glint uses, else Glint's own folder, else PATH.
pub fn tools() -> (PathBuf, PathBuf) {
    static T: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    T.get_or_init(|| {
        let exe = if cfg!(windows) { ".exe" } else { "" };
        let pair = |ff: PathBuf| {
            let probe = ff.with_file_name(format!("ffprobe{exe}"));
            (ff, probe)
        };
        if let Some(p) = folders::load()["ffmpeg"].as_str().filter(|s| !s.is_empty()) {
            return pair(PathBuf::from(p));
        }
        if let Ok(v) = crate::glint::editor("tools", json!({})) {
            if let Some(p) = v["ffmpeg"].as_str().filter(|s| !s.is_empty() && Path::new(s).is_file()) {
                let probe = v["ffprobe"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| pair(PathBuf::from(p)).1);
                return (PathBuf::from(p), probe);
            }
        }
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Glint").join("bin").join(format!("ffmpeg{exe}"));
        if exists_exe(&local) {
            return pair(local);
        }
        (PathBuf::from(format!("ffmpeg{exe}")), PathBuf::from(format!("ffprobe{exe}")))
    })
    .clone()
}

fn run(mut c: Command) -> Result<Vec<u8>, String> {
    let out = c.stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("could not start {:?}: {e}", c.get_program()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(4).collect();
        return Err(format!("{:?} failed: {}", c.get_program(), tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    Ok(out.stdout)
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("glint-mcp-{}", std::process::id()));
    let _ = fs::create_dir_all(&d);
    d
}

pub fn cleanup() {
    let _ = fs::remove_dir_all(tmp());
}

/* ---------- info ---------- */

pub fn info(path: &Path) -> Result<Value, String> {
    let mut c = cmd(&tools().1);
    c.args(["-v", "error", "-show_format", "-show_streams", "-of", "json"]).arg(path);
    let v: Value = serde_json::from_slice(&run(c)?).map_err(|e| e.to_string())?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != json!(1));
    let audio: Vec<Value> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .enumerate()
        .map(|(i, s)| json!({ "track": i, "title": s["tags"]["title"].as_str().or(s["tags"]["handler_name"].as_str()).unwrap_or(""), "channels": s["channels"], "codec": s["codec_name"] }))
        .collect();
    Ok(json!({
        "path": display(path),
        "length": v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).map(|d| (d * 1000.0).round() / 1000.0),
        "size": v["format"]["size"].as_str().and_then(|s| s.parse::<u64>().ok()),
        "video": video.map(|s| json!({ "width": s["width"], "height": s["height"], "fps": s["avg_frame_rate"], "codec": s["codec_name"] })),
        "audio_tracks": audio,
        "tags": v["format"]["tags"],
    }))
}

pub fn duration(path: &Path) -> f64 {
    info(path).ok().and_then(|v| v["length"].as_f64()).unwrap_or(0.0)
}

/* ---------- seeing ---------- */

/// One frame at `t` seconds as a JPEG, at most `width` pixels wide.
pub fn frame(path: &Path, t: f64, width: u32) -> Result<Vec<u8>, String> {
    let mut c = cmd(&tools().0);
    c.args(["-v", "error", "-ss", &format!("{:.3}", t.max(0.0))]).arg("-i").arg(path);
    c.args(["-frames:v", "1", "-vf", &format!("scale='min({width},iw)':-2"), "-q:v", "5", "-f", "image2pipe", "-c:v", "mjpeg", "-"]);
    let out = run(c)?;
    if out.is_empty() {
        return Err(format!("no picture at {t:.2} s (past the end?)"));
    }
    Ok(out)
}

/// `count` frames spread over [from, to] in one picture (a grid), and the time of each.
pub fn sheet(path: &Path, from: f64, to: f64, count: usize, cols: usize) -> Result<(Vec<u8>, Vec<f64>), String> {
    let n = count.clamp(2, 36);
    let cols = cols.clamp(1, 6).min(n);
    let rows = n.div_ceil(cols);
    let dir = tmp().join(format!("sheet{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let step = (to - from).max(0.0) / n as f64;
    let mut times = Vec::new();
    for i in 0..n {
        let t = from + step * (i as f64 + 0.5);
        let jpg = frame(path, t, 320)?;
        fs::write(dir.join(format!("f{:03}.jpg", i + 1)), jpg).map_err(|e| e.to_string())?;
        times.push((t * 100.0).round() / 100.0);
    }
    let mut c = cmd(&tools().0);
    c.args(["-v", "error", "-framerate", "1", "-start_number", "1", "-i"]).arg(dir.join("f%03d.jpg"));
    c.args(["-vf", &format!("tile={cols}x{rows}:padding=4:color=black"), "-frames:v", "1", "-q:v", "5", "-f", "image2pipe", "-c:v", "mjpeg", "-"]);
    let out = run(c);
    let _ = fs::remove_dir_all(&dir);
    Ok((out?, times))
}

/* ---------- hearing: levels ---------- */

/// Loudness over time (dBFS per window) and the quiet stretches, from track `track`.
pub fn levels(path: &Path, track: u32, from: f64, to: Option<f64>, window: Option<f64>, silence_db: f64, min_silence: f64) -> Result<Value, String> {
    const RATE: usize = 8000;
    let mut c = cmd(&tools().0);
    c.args(["-v", "error", "-ss", &format!("{:.3}", from.max(0.0))]);
    if let Some(t) = to {
        c.args(["-t", &format!("{:.3}", (t - from).max(0.1))]);
    }
    c.arg("-i").arg(path).args(["-map", &format!("0:a:{track}"), "-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "-"]);
    let raw = run(c)?;
    let samples: Vec<f32> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
    if samples.is_empty() {
        return Err("no audio in that track or range".into());
    }
    let secs = samples.len() as f64 / RATE as f64;
    // Windows: as asked, else small enough for ~400 values.
    let win = window.unwrap_or_else(|| (secs / 400.0).max(0.1)).clamp(0.05, 60.0);
    let per = ((win * RATE as f64) as usize).max(1);
    let db = |chunk: &[f32]| {
        let ms = chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len().max(1) as f32;
        if ms <= 1e-10 { -100.0 } else { 10.0 * ms.log10() as f64 }
    };
    let levels: Vec<i32> = samples.chunks(per).map(|c| db(c).round() as i32).collect();
    // Quiet stretches, on 50 ms steps for precision.
    let fine = RATE / 20;
    let mut silences = Vec::new();
    let mut start: Option<f64> = None;
    for (i, c) in samples.chunks(fine).enumerate() {
        let t = from + i as f64 * 0.05;
        if db(c) < silence_db {
            start.get_or_insert(t);
        } else if let Some(s) = start.take() {
            if t - s >= min_silence {
                silences.push(json!([r2(s), r2(t)]));
            }
        }
    }
    if let Some(s) = start {
        let end = from + secs;
        if end - s >= min_silence {
            silences.push(json!([r2(s), r2(end)]));
        }
    }
    let peak = samples.iter().fold(0f32, |a, x| a.max(x.abs()));
    Ok(json!({
        "from": r2(from), "to": r2(from + secs), "window_seconds": r2(win),
        "overall_db": (db(&samples) * 10.0).round() / 10.0,
        "peak_db": if peak > 0.0 { (20.0 * peak.log10() as f64 * 10.0).round() / 10.0 } else { -100.0 },
        "levels_db": levels,
        "silences": silences,
        "note": format!("levels_db[i] is the loudness of {:.2} s starting at from + i * window_seconds. silences are [start, end] seconds quieter than {silence_db} dB for at least {min_silence} s.", win),
    }))
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/* ---------- hearing: speech to text (whisper.cpp) ---------- */

pub fn whisper_paths() -> (PathBuf, PathBuf) {
    let cfg = folders::load();
    let dir = plugin_dir().join("whisper");
    let exe = if cfg!(windows) { "whisper-cli.exe" } else { "whisper-cli" };
    let cli = cfg["whisper"]["cli"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| dir.join(exe));
    let model = cfg["whisper"]["model"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        // The first ggml-*.bin in the whisper folder.
        fs::read_dir(&dir)
            .ok()
            .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("ggml-") && n.to_string_lossy().ends_with(".bin"))))
            .unwrap_or_else(|| dir.join("ggml-base.en.bin"))
    });
    (cli, model)
}

const NO_WHISPER: &str = "Speech to text is not set up yet. The person runs this once (about 150 MB download): glint-mcp.exe setup-transcripts";

fn cache_key(path: &Path, extra: &str) -> String {
    use std::hash::{Hash, Hasher};
    let md = fs::metadata(path).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    display(path).hash(&mut h);
    md.as_ref().map(|m| m.len()).hash(&mut h);
    md.and_then(|m| m.modified().ok()).hash(&mut h);
    extra.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Speech in `path` (track `track`, optional range) as timed segments in the FILE's seconds. Cached.
pub fn transcribe(path: &Path, track: u32, from: f64, to: Option<f64>, language: &str, words: bool) -> Result<Value, String> {
    let (cli, model) = whisper_paths();
    if !cli.is_file() || !model.is_file() {
        return Err(NO_WHISPER.into());
    }
    let key = cache_key(path, &format!("{track}|{from}|{to:?}|{language}|{words}|{}", display(&model)));
    let cache = plugin_dir().join("cache").join(format!("{key}.json"));
    if let Some(v) = fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        return Ok(v);
    }
    let dir = tmp();
    let wav = dir.join(format!("{key}.wav"));
    let mut c = cmd(&tools().0);
    c.args(["-v", "error", "-y", "-ss", &format!("{:.3}", from.max(0.0))]);
    if let Some(t) = to {
        c.args(["-t", &format!("{:.3}", (t - from).max(0.1))]);
    }
    c.arg("-i").arg(path).args(["-map", &format!("0:a:{track}"), "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]).arg(&wav);
    run(c)?;
    let base = dir.join(&key);
    let mut w = cmd(&cli);
    w.arg("-m").arg(&model).arg("-f").arg(&wav).args(["-oj", "-np"]).arg("-of").arg(&base);
    w.args(["-l", if language.is_empty() { "auto" } else { language }]);
    if words {
        w.args(["-ml", "1", "-sow"]);
    }
    let r = run(w);
    let _ = fs::remove_file(&wav);
    r?;
    let json_path = base.with_extension("json");
    let text = fs::read(&json_path).map_err(|e| format!("whisper wrote no result: {e}"))?;
    let _ = fs::remove_file(&json_path);
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&text)).map_err(|e| format!("whisper's result is unreadable: {e}"))?;
    let segs: Vec<Value> = v["transcription"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let a = s["offsets"]["from"].as_f64()? / 1000.0 + from;
            let b = s["offsets"]["to"].as_f64()? / 1000.0 + from;
            let t = s["text"].as_str()?.trim().to_string();
            (!t.is_empty()).then(|| json!({ "start": r2(a), "end": r2(b), "text": t }))
        })
        .collect();
    let full = segs.iter().filter_map(|s| s["text"].as_str()).collect::<Vec<_>>().join(" ");
    let out = json!({ "path": display(path), "track": track, "language": v["result"]["language"], "segments": segs, "text": full });
    let _ = fs::create_dir_all(cache.parent().unwrap());
    let _ = fs::write(&cache, out.to_string());
    Ok(out)
}

/// Download whisper.cpp and a model into the plugin's folder (Windows: curl.exe and tar.exe come with Windows 10+).
pub fn setup_whisper(model: &str) -> Result<String, String> {
    let dir = plugin_dir().join("whisper");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (cli, _) = whisper_paths();
    if !cli.is_file() {
        if !cfg!(windows) {
            return Err(format!("Put a whisper-cli program at {} (build whisper.cpp), then run this again.", cli.display()));
        }
        let zip = dir.join("whisper-bin-x64.zip");
        eprintln!("Downloading whisper.cpp ...");
        let mut c = Command::new("curl.exe");
        c.args(["-L", "--fail", "-o"]).arg(&zip).arg("https://github.com/ggml-org/whisper.cpp/releases/latest/download/whisper-bin-x64.zip");
        if !c.status().map(|s| s.success()).unwrap_or(false) {
            return Err("Downloading whisper.cpp failed".into());
        }
        let mut t = Command::new("tar.exe");
        t.arg("-xf").arg(&zip).arg("-C").arg(&dir);
        if !t.status().map(|s| s.success()).unwrap_or(false) {
            return Err("Unpacking whisper.cpp failed".into());
        }
        let _ = fs::remove_file(&zip);
        // The zip has Release\whisper-cli.exe and its DLLs: bring them up next to each other in whisper\.
        for sub in ["Release", "release", "bin"] {
            if let Ok(rd) = fs::read_dir(dir.join(sub)) {
                for e in rd.flatten() {
                    let _ = fs::rename(e.path(), dir.join(e.file_name()));
                }
            }
        }
        if !cli.is_file() {
            return Err(format!("whisper-cli.exe was not in the download; put it at {}", cli.display()));
        }
    }
    let file = dir.join(format!("ggml-{model}.bin"));
    if !file.is_file() {
        eprintln!("Downloading the speech model {model} ...");
        let mut c = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
        c.args(["-L", "--fail", "-o"]).arg(&file).arg(format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin"));
        if !c.status().map(|s| s.success()).unwrap_or(false) {
            let _ = fs::remove_file(&file);
            return Err(format!("Downloading the model {model} failed"));
        }
    }
    let mut cfg = folders::load();
    cfg["whisper"] = json!({ "cli": display(&cli), "model": display(&file) });
    folders::save(&cfg)?;
    Ok(format!("Speech to text is ready ({model})."))
}

/* ---------- base64 for pictures ---------- */

pub fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}
