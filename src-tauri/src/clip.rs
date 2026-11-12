//! Turn a buffer snapshot into an MP4: video is stream-copied, audio is encoded to AAC.
use crate::audio::Pcm;
use crate::library::{self, ClipMeta};
use crate::recorder::Snapshot;
use crate::settings::Settings;
use crate::util;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub struct Capture {
    pub snap: Snapshot,
    pub desk: Option<Pcm>,
    pub mic: Option<Pcm>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

fn write_pcm(path: &Path, pcm: &Pcm) -> std::io::Result<()> {
    // i16 -> little-endian bytes; x86/ARM Windows are little-endian already.
    let bytes = unsafe { std::slice::from_raw_parts(pcm.data.as_ptr() as *const u8, pcm.data.len() * 2) };
    fs::write(path, bytes)
}

/// Mux the snapshot into `<clips_dir>/<game> <timestamp>.mp4`.
///
/// Audio layout: with both sources, track 1 is the mix (what players and chat
/// apps use), track 2 desktop only, track 3 mic only.
pub fn write(s: &Settings, mut cap: Capture, game: &str) -> Result<ClipMeta, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    if s.mic_denoise {
        if let Some(mic) = cap.mic.as_mut().filter(|m| !m.data.is_empty()) {
            let started = std::time::Instant::now();
            crate::denoise::process(mic);
            util::log(&format!("mic noise suppression: {:.1}s of audio in {} ms", mic.data.len() as f64 / 2.0 / mic.rate.max(1) as f64, started.elapsed().as_millis()));
        }
    }
    // With game folders on, the clip goes to `<clips folder>\<game>`, which is also its library folder.
    let folder = if s.game_folders { util::game_folder(game) } else { String::new() };
    let dir = if folder.is_empty() { PathBuf::from(&s.clips_dir) } else { PathBuf::from(&s.clips_dir).join(&folder) };
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create clips folder: {e}"))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
    let title = format!("{} {}", util::sanitize_filename(game), stamp);
    let out = util::unique_path(&dir, &title, "mp4");
    let id = util::new_id();
    let tmp = util::tmp_dir();

    let mut inputs: Vec<(PathBuf, u32, &str)> = Vec::new();
    for (pcm, tag, name) in [(&cap.desk, "d", "Desktop"), (&cap.mic, "m", "Microphone")] {
        if let Some(p) = pcm {
            if !p.data.is_empty() {
                let f = tmp.join(format!("{id}_{tag}.pcm"));
                write_pcm(&f, p).map_err(|e| format!("Cannot write temp audio: {e}"))?;
                inputs.push((f, p.rate, name));
            }
        }
    }

    let mut cmd = util::hidden_cmd(&ffmpeg);
    // A still screen leaves long gaps between video timestamps. Keep them: by default
    // ffmpeg closes gaps over 10 s in MPEG-TS input, which would pull video ahead of audio.
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-dts_delta_threshold", "1000000", "-f", "mpegts", "-i", "pipe:0"]);
    for (f, rate, _) in &inputs {
        cmd.args(["-f", "s16le", "-ar", &rate.to_string(), "-ac", "2", "-i"]).arg(f);
    }
    match inputs.len() {
        2 => {
            cmd.args(["-filter_complex", "[1:a][2:a]amix=inputs=2:duration=longest:normalize=0[mix]"]);
            cmd.args(["-map", "0:v:0", "-map", "[mix]", "-map", "1:a", "-map", "2:a"]);
            cmd.args(["-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop", "-metadata:s:a:2", "title=Microphone"]);
            cmd.args(["-disposition:a:0", "default", "-disposition:a:1", "0", "-disposition:a:2", "0"]);
        }
        1 => {
            cmd.args(["-map", "0:v:0", "-map", "1:a"]);
            cmd.arg("-metadata:s:a:0").arg(format!("title={}", inputs[0].2));
        }
        _ => {
            cmd.args(["-map", "0:v:0"]);
        }
    }
    // The "fast" AAC coder halves audio encode time on long clips (measured 11.8 s -> 5.5 s for 10 min x 3 tracks).
    cmd.args(["-c:v", "copy", "-c:a", "aac", "-aac_coder", "fast", "-b:a", "160k", "-ar", "48000", "-ac", "2", "-movflags", "+faststart"]);
    cmd.arg("-metadata").arg(format!("title={title}"));
    cmd.arg(&out);

    let cleanup = |inputs: &[(PathBuf, u32, &str)]| {
        for (f, _, _) in inputs {
            let _ = fs::remove_file(f);
        }
    };
    let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            cleanup(&inputs);
            return Err(format!("Could not start ffmpeg: {e}"));
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A broken pipe here means ffmpeg already failed; its stderr explains why.
        // GOPs that have gone to disk are read back from the buffer files here.
        let _ = stdin.write_all(&cap.snap.header).and_then(|_| cap.snap.gops.iter().try_for_each(|g| g.write_to(&mut stdin)));
    }
    let result = child.wait_with_output();
    cleanup(&inputs);
    let output = result.map_err(|e| format!("ffmpeg failed: {e}"))?;
    if !output.status.success() || !out.is_file() {
        let _ = fs::remove_file(&out);
        return Err(format!("ffmpeg could not write the clip: {}", util::last_lines(&output.stderr, 2)));
    }

    let duration = cap.snap.duration;
    Ok(ClipMeta {
        thumb: library::make_thumb(&ffmpeg, &out, &id, (duration * 0.5).min(5.0)),
        id,
        path: out.to_string_lossy().into_owned(),
        title,
        game: game.to_string(),
        created: util::unix_ms(),
        duration,
        size: fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        width: cap.width,
        height: cap.height,
        fps: cap.fps,
        favorite: false,
        kind: "clip".into(),
        audio_tracks: match inputs.len() {
            2 => 3,
            n => n as u32,
        },
        folder,
    })
}

/// Cut `[start, end]` out of a clip without re-encoding (snaps to keyframes, 1 s apart).
/// With `include_mic == false` on a 3-track clip, only the desktop track is kept.
pub fn trim(s: &Settings, src: &ClipMeta, start: f64, end: f64, include_mic: bool) -> Result<ClipMeta, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    if end - start < 0.5 {
        return Err("The selection is too short".into());
    }
    let src_path = PathBuf::from(&src.path);
    let dir = src_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&s.clips_dir));
    let title = format!("{} (trim)", src.title);
    let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
    let mut cmd = util::hidden_cmd(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{start:.3}"), "-to", &format!("{end:.3}"), "-i"]).arg(&src_path);
    if !include_mic && src.audio_tracks >= 3 {
        cmd.args(["-map", "0:v:0", "-map", "0:a:1", "-disposition:a:0", "default"]);
    } else {
        cmd.args(["-map", "0"]);
    }
    cmd.args(["-c", "copy", "-movflags", "+faststart"]).arg(&out);
    let output = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    if !output.status.success() || !out.is_file() {
        let _ = fs::remove_file(&out);
        return Err(format!("Trim failed: {}", util::last_lines(&output.stderr, 2)));
    }
    let mut meta = library::register(&ffmpeg, &out, &title, &src.game, "trim");
    // The file's average rate reflects how busy the screen was, not the capture setting.
    meta.fps = src.fps;
    // Written beside the source, so it sits in the same library folder.
    meta.folder = src.folder.clone();
    Ok(meta)
}
