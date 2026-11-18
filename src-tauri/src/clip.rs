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
    /// The game's own sound (process loopback), None when it was not captured
    /// for any of the clip's time. Becomes the last track, titled "Game".
    pub game: Option<Pcm>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

fn write_pcm(path: &Path, pcm: &Pcm) -> std::io::Result<()> {
    // i16 -> little-endian bytes; x86/ARM Windows are little-endian already.
    let bytes = unsafe { std::slice::from_raw_parts(pcm.data.as_ptr() as *const u8, pcm.data.len() * 2) };
    fs::write(path, bytes)
}

/// The first free `<base>.<ext>` / `<base> (n).<ext>` in `dir`, created empty
/// on the spot: two clips saved in the same second (one per monitor) must not
/// both pick the same name before either file exists. ffmpeg overwrites it (`-y`).
pub(crate) fn reserve_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    loop {
        let p = util::unique_path(dir, base, ext);
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            _ => return p,
        }
    }
}

/// Mux the snapshot into `<clips_dir>/<game> <timestamp>.mp4`.
///
/// Audio layout: with both sources, track 1 is the mix (what players and chat
/// apps use), track 2 desktop only, track 3 mic only. With game audio, a
/// "Game" track is added LAST (Mix, Desktop, Microphone, Game; or Desktop,
/// Game), so the index of every other track stays what it always was. The mix
/// stays desktop + mic: the game is already part of the desktop sound.
/// Without desktop sound the mix is mic + game (Mix, Microphone, Game), so
/// track 1 still holds everything that was heard.
pub fn write(s: &Settings, cap: Capture, game: &str) -> Result<ClipMeta, String> {
    write_as(s, cap, game, false)
}

/// `write` for a clip of game mode (capmode.rs, 0.1.27): no desktop sound, and the tracks are
/// Mix (game + microphone), Game, Microphone (`audio_layout_game`).
pub fn write_game(s: &Settings, mut cap: Capture, game: &str) -> Result<ClipMeta, String> {
    cap.desk = None;
    write_as(s, cap, game, true)
}

fn write_as(s: &Settings, mut cap: Capture, game: &str, game_mode: bool) -> Result<ClipMeta, String> {
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
    // Settings → Storage → Clip names (0.1.18); the default blocks give "<game> <date> <time>" as before.
    let title = crate::naming::clip_title(&s.clip_name, game, cap.snap.duration);
    let id = util::new_id();
    let tmp = util::tmp_dir();

    let mut inputs: Vec<(PathBuf, u32, &str)> = Vec::new();
    for (pcm, tag, name) in [(&cap.desk, "d", "Desktop"), (&cap.mic, "m", "Microphone"), (&cap.game, "g", "Game")] {
        if let Some(p) = pcm {
            if !p.data.is_empty() {
                let f = tmp.join(format!("{id}_{tag}.pcm"));
                if let Err(e) = write_pcm(&f, p) {
                    for (f, _, _) in &inputs {
                        let _ = fs::remove_file(f);
                    }
                    return Err(format!("Cannot write temp audio: {e}"));
                }
                inputs.push((f, p.rate, name));
            }
        }
    }
    let out = reserve_path(&dir, &title, "mp4");

    let mut cmd = util::hidden_cmd(&ffmpeg);
    // A still screen leaves long gaps between video timestamps. Keep them: by default
    // ffmpeg closes gaps over 10 s in MPEG-TS input, which would pull video ahead of audio.
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-dts_delta_threshold", "1000000", "-f", "mpegts", "-i", "pipe:0"]);
    for (f, rate, _) in &inputs {
        cmd.args(["-f", "s16le", "-ar", &rate.to_string(), "-ac", "2", "-i"]).arg(f);
    }
    let names: Vec<&str> = inputs.iter().map(|(_, _, name)| *name).collect();
    // The desktop's sound holds the game's only when the game plays on the output Glint records
    // (0.1.35: a game on other headphones gave a track 1 with the microphone alone).
    let game_in_mix = !game_mode && game_missing_from_desktop(cap.desk.as_ref(), cap.game.as_ref());
    let titles = if game_mode { audio_layout_game(&mut cmd, &names) } else { audio_layout(&mut cmd, &names, game_in_mix) };
    util::log(&audio_note(&cap, &titles, game_mode, game_in_mix));
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
        Ok(c) => {
            crate::util::track_child(&c); // ends with Glint (0.1.41)
            c
        }
        Err(e) => {
            cleanup(&inputs);
            let _ = fs::remove_file(&out);
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
        thumb: library::make_thumb(&ffmpeg, &out, &id, library::thumb_at(duration)),
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
        audio_tracks: titles.len() as u32,
        folder,
        joins: Vec::new(),
        join_names: Vec::new(),
        draft: false,
        cloud_only: false,
    })
}

/// Add the `-map`, title and disposition arguments for the audio inputs
/// `names` (ffmpeg inputs 1, 2, ... in that order; input 0 is the video).
/// Returns the titles of the audio tracks written, in order.
///
/// Desktop + Microphone get their mix as track 1; a "Game" input is always
/// the last track and is part of the mix only when there is no "Desktop"
/// input (the desktop sound holds the game): Microphone + Game then get their
/// mix as track 1, because players, chat apps, Compress for Discord and joins
/// take track 1 only.
///
/// `game_in_mix` (0.1.35): the desktop's sound does NOT hold the game's (it
/// plays on another output; see `game_missing_from_desktop`), so the game is
/// mixed into track 1 beside the desktop: Mix (desktop + mic + game), Desktop,
/// Microphone, Game; or Mix (desktop + game), Desktop, Game. Track 1 always
/// holds everything that was recorded.
pub fn audio_layout(cmd: &mut std::process::Command, names: &[&str], game_in_mix: bool) -> Vec<&'static str> {
    let game = names.iter().position(|n| *n == "Game");
    let base: Vec<(usize, &str)> = names.iter().copied().enumerate().filter(|(_, n)| *n != "Game").collect();
    let has_desk = base.iter().any(|(_, n)| *n == "Desktop");
    // The inputs of track 1: every source but the game, and the game where the desktop lacks it.
    let mut mixed: Vec<usize> = base.iter().map(|(i, _)| *i).collect();
    if let Some(g) = game.filter(|_| !has_desk || game_in_mix) {
        mixed.push(g);
    }
    let mut maps: Vec<String> = Vec::new();
    let mut titles: Vec<&'static str> = Vec::new();
    let title_of = |n: &str| -> &'static str {
        match n {
            "Desktop" => "Desktop",
            "Microphone" => "Microphone",
            "Game" => "Game",
            _ => "Audio",
        }
    };
    if mixed.len() >= 2 {
        let ins: String = mixed.iter().map(|i| format!("[{}:a]", i + 1)).collect();
        cmd.arg("-filter_complex").arg(format!("{ins}amix=inputs={}:duration=longest:normalize=0[mix]", mixed.len()));
        maps.push("[mix]".into());
        titles.push("Mix");
    }
    for (i, n) in &base {
        maps.push(format!("{}:a", i + 1));
        titles.push(title_of(n));
    }
    if let Some(g) = game {
        maps.push(format!("{}:a", g + 1));
        titles.push("Game");
    }
    cmd.args(["-map", "0:v:0"]);
    for m in &maps {
        cmd.arg("-map").arg(m);
    }
    for (i, t) in titles.iter().enumerate() {
        cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={t}"));
        if titles.len() > 1 {
            cmd.arg(format!("-disposition:a:{i}")).arg(if i == 0 { "default" } else { "0" });
        }
    }
    titles
}

/// The track layout of a game-mode clip (0.1.27): with the game's sound and the microphone,
/// track 1 is their mix (what players, Discord and joins take), then the game alone, then the
/// microphone alone; one source alone is the only track. Same titles as `audio_layout`.
pub fn audio_layout_game(cmd: &mut std::process::Command, names: &[&str]) -> Vec<&'static str> {
    let game = names.iter().position(|n| *n == "Game");
    let mic = names.iter().position(|n| *n == "Microphone");
    let mut maps: Vec<String> = Vec::new();
    let mut titles: Vec<&'static str> = Vec::new();
    if let (Some(g), Some(m)) = (game, mic) {
        cmd.arg("-filter_complex").arg(format!("[{}:a][{}:a]amix=inputs=2:duration=longest:normalize=0[mix]", g + 1, m + 1));
        maps.push("[mix]".into());
        titles.push("Mix");
    }
    for (at, title) in [(game, "Game"), (mic, "Microphone")] {
        if let Some(i) = at {
            maps.push(format!("{}:a", i + 1));
            titles.push(title);
        }
    }
    cmd.args(["-map", "0:v:0"]);
    for m in &maps {
        cmd.arg("-map").arg(m);
    }
    for (i, t) in titles.iter().enumerate() {
        cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={t}"));
        if titles.len() > 1 {
            cmd.arg(format!("-disposition:a:{i}")).arg(if i == 0 { "default" } else { "0" });
        }
    }
    titles
}

/// The loudest sample of `p` in dB below full scale; None when it has no samples.
/// Pure digital silence gives -120.
pub fn peak_db(p: &Pcm) -> Option<f64> {
    if p.data.is_empty() {
        return None;
    }
    let peak = p.data.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f64 / 32768.0;
    Some(if peak > 1e-6 { (20.0 * peak.log10()).max(-120.0) } else { -120.0 })
}

/// How a source reads in the clip's log line: "none" (not recorded), "empty", "silent" or its peak.
fn source_note(p: Option<&Pcm>) -> String {
    match p.map(|p| (p, peak_db(p))) {
        None => "none".into(),
        Some((_, None)) => "empty".into(),
        Some((p, Some(db))) if db <= -90.0 => format!("silent ({:.1} s)", p.data.len() as f64 / 2.0 / p.rate.max(1) as f64),
        Some((p, Some(db))) => format!("peak {db:.1} dB ({:.1} s)", p.data.len() as f64 / 2.0 / p.rate.max(1) as f64),
    }
}

/// One log line per clip: what went in, how loud, and what track 1 holds.
fn audio_note(cap: &Capture, titles: &[&str], game_mode: bool, game_in_mix: bool) -> String {
    let track1 = match titles.first() {
        None => "no sound".to_string(),
        Some(&"Mix") => {
            let mut v: Vec<&str> = Vec::new();
            if cap.desk.as_ref().is_some_and(|p| !p.data.is_empty()) {
                v.push("Desktop");
            }
            if cap.mic.as_ref().is_some_and(|p| !p.data.is_empty()) {
                v.push("Microphone");
            }
            if titles.contains(&"Game") && (game_mode || game_in_mix || !v.contains(&"Desktop")) {
                v.push("Game");
            }
            format!("Mix of {}", v.join(" + "))
        }
        Some(t) => t.to_string(),
    };
    format!(
        "clip audio{}: Desktop {}, Microphone {}, Game {}; tracks {}; track 1 = {track1}{}",
        if game_mode { " (game mode)" } else { "" },
        source_note(cap.desk.as_ref()),
        source_note(cap.mic.as_ref()),
        source_note(cap.game.as_ref()),
        titles.join(", "),
        if game_in_mix { " (the desktop sound did not hold the game's, so the game was mixed in)" } else { "" }
    )
}

/// Whether the desktop's sound lacks the game's sound although the game was heard: the game
/// plays on another output than the one Glint records (gaming headsets with a "Game" and a
/// "Chat" device, a game set to the headphones while Windows' default is the speakers), or
/// nothing at all came from that output. Then track 1 needs the game mixed in.
///
/// Measured on the waveforms, not the levels (a desktop with music or chat as loud as the game
/// must still count as lacking it): in up to 8 one-second windows where the game is loudest
/// (above -50 dBFS), both are taken to mono at 8 kHz and the desktop is searched for the
/// game's waveform within 100 ms either side (the two captures' clocks differ a little). A
/// window holds the game when the normalized correlation reaches 0.35 (a desktop that is the
/// game plus something as loud gives about 0.7; unrelated sound about 0); a desktop under
/// -70 dBFS there does not hold it. The desktop lacks the game when fewer than half of the
/// windows hold it. The faders of Settings → Audio scale a whole source, which a correlation
/// does not see. A game that is silent throughout, or no game, changes nothing.
pub fn game_missing_from_desktop(desk: Option<&Pcm>, game: Option<&Pcm>) -> bool {
    const LOW: f64 = 8000.0;
    const LAG: usize = 800; // 100 ms at 8 kHz
    let Some(game) = game.filter(|p| !p.data.is_empty()) else { return false };
    let Some(desk) = desk.filter(|p| !p.data.is_empty()) else { return false };
    let secs = |p: &Pcm| p.data.len() as f64 / 2.0 / p.rate.max(1) as f64;
    // Mono at 8 kHz from `t0` for `n` samples: the mean of the frames in each 1/8000 s (0 outside the PCM).
    let low = |p: &Pcm, t0: f64, n: usize| -> Vec<f32> {
        let rate = p.rate.max(1) as f64;
        let frames = p.data.len() / 2;
        (0..n)
            .map(|k| {
                let t = t0 + k as f64 / LOW;
                if t < 0.0 {
                    return 0.0;
                }
                let a = (t * rate) as usize;
                let b = (((t + 1.0 / LOW) * rate) as usize).max(a + 1).min(frames);
                if a >= b {
                    return 0.0;
                }
                let sum: f64 = (a..b).map(|i| (p.data[i * 2] as f64 + p.data[i * 2 + 1] as f64) * 0.5).sum();
                (sum / (b - a) as f64 / 32768.0) as f32
            })
            .collect()
    };
    let rms = |v: &[f32]| (v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / v.len().max(1) as f64).sqrt();
    // The game's loudest one-second windows.
    let whole = secs(game).min(secs(desk)).floor() as usize;
    let mut wins: Vec<(f64, usize)> = (0..whole).map(|w| (rms(&low(game, w as f64, LOW as usize)), w)).filter(|(r, _)| *r > 10f64.powf(-50.0 / 20.0)).collect();
    if wins.is_empty() {
        return false;
    }
    wins.sort_by(|a, b| b.0.total_cmp(&a.0));
    wins.truncate(8);
    let n = LOW as usize;
    let mut held = 0usize;
    for (_, w) in &wins {
        let g = low(game, *w as f64, n);
        let d = low(desk, *w as f64 - LAG as f64 / LOW, n + 2 * LAG);
        if rms(&d) < 10f64.powf(-70.0 / 20.0) {
            continue;
        }
        let gg: f64 = g.iter().map(|x| (*x as f64) * (*x as f64)).sum();
        // Sliding energy of the desktop under the window.
        let mut dd: f64 = d[..n].iter().map(|x| (*x as f64) * (*x as f64)).sum();
        let mut best = 0f64;
        for lag in 0..=2 * LAG {
            if lag > 0 {
                let (out, inn) = (d[lag - 1] as f64, d[lag + n - 1] as f64);
                dd = (dd - out * out + inn * inn).max(0.0);
            }
            if dd <= 1e-12 || gg <= 1e-12 {
                continue;
            }
            let dot: f64 = g.iter().zip(&d[lag..lag + n]).map(|(a, b)| *a as f64 * *b as f64).sum();
            best = best.max(dot / (gg * dd).sqrt());
        }
        if best >= 0.35 {
            held += 1;
        }
    }
    held * 2 < wins.len()
}

/// Titles of the audio streams of `file`, in stream order ("" = untitled).
/// ffmpeg's MP4 reader gives a track title back as the tag "name"; either is taken.
pub fn audio_titles(ffmpeg: &Path, file: &Path) -> Vec<String> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    let Ok(out) = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index:stream_tags=title,name", "-of", "default=nw=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut v: Vec<(Option<String>, Option<String>)> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.starts_with("index=") {
            v.push((None, None));
        } else if let Some(last) = v.last_mut() {
            if let Some(x) = line.strip_prefix("TAG:title=") {
                last.0 = Some(x.trim().to_string()).filter(|x| !x.is_empty());
            } else if let Some(x) = line.strip_prefix("TAG:name=") {
                last.1 = Some(x.trim().to_string()).filter(|x| !x.is_empty());
            }
        }
    }
    v.into_iter().map(|(a, b)| a.or(b).unwrap_or_default()).collect()
}

/// Give `out` (made from `src` by another program, such as Blur) the audio
/// track titles of `src` again, when `src` has a "Game" track: the editor tells
/// Mix, Desktop, Microphone and Game apart by their titles, and an untitled
/// 4-track file would show the mix as a row of its own. A stream copy; `out`
/// is left as it is when the track counts differ or anything fails.
pub fn restore_titles(ffmpeg: &Path, src: &Path, out: &Path) {
    let want = audio_titles(ffmpeg, src);
    if !want.iter().any(|t| t == "Game") {
        return;
    }
    let have = audio_titles(ffmpeg, out);
    if have == want || have.len() != want.len() {
        return;
    }
    let ext = out.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    let tmp = out.with_extension(format!("titles.{ext}"));
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(out).args(["-map", "0", "-c", "copy"]);
    for (i, t) in want.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
        cmd.arg(format!("-metadata:s:a:{i}")).arg(format!("title={t}"));
    }
    if ext.eq_ignore_ascii_case("mp4") || ext.eq_ignore_ascii_case("mov") {
        cmd.args(["-movflags", "+faststart"]);
    }
    let ok = cmd.arg(&tmp).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if ok && audio_titles(ffmpeg, &tmp) == want && fs::rename(&tmp, out).is_ok() {
        return;
    }
    let _ = fs::remove_file(&tmp);
}

/// The shortest lossless cut (seconds); the editor re-encodes a shorter kept piece instead (0.1.41).
pub const MIN_TRIM: f64 = 0.5;

/// The most a lossless cut may start before the asked time (checked by
/// `--blur-sync-test`). The cut starts AT the asked time; see `trim_start`.
pub const TRIM_EARLY_MAX: f64 = 0.05;

/// The `-ss` value of a lossless cut asked for at `start`, and the `-bsf:v`
/// that goes with it (None when nothing needs it).
///
/// Clips from the buffer have a variable frame rate: on a still screen the
/// frame on screen at `start` (the last one at or before it) can be a second
/// old. A cut that starts between two frames is right in a player (the MP4
/// edit list says where it starts), but ffmpeg's MP4 reader moves the first
/// frame it shows to 0, which is the first frame AFTER the cut, so every
/// re-encode of the trim (editor export, Compress for Discord, a join) had its
/// picture early by that gap: 5 ms in motion, up to 1.3 s on a still screen.
/// Starting the cut on that frame instead (0.1.12) brought back up to that
/// much of the sound the user cut away and made the trim longer than asked.
///
/// So the cut starts exactly at `start`, and the frame on screen there is
/// re-timed to the cut (its presentation time only, so decoding is untouched):
/// a player shows that same frame from the cut on either way, and ffmpeg now
/// finds a frame exactly at the start and moves nothing. `setts` sees packet
/// times relative to the `-ss` point.
fn trim_start(ffmpeg: &Path, file: &Path, start: f64) -> (String, Option<String>) {
    let ss = format!("{start:.6}");
    let bsf = match crate::editor::frames_on_screen(ffmpeg, file, &[start]).first().copied().flatten() {
        // Frames are at least 4 ms apart, so the 1 ms window holds that frame only.
        Some(f) if f >= 0.0 && f < start - 0.0005 => {
            let rel = f - start;
            Some(format!("setts=pts='if(between(PTS*TB\\,{:.6}\\,{:.6})\\,0\\,PTS)'", rel - 0.001, (rel + 0.001).min(0.0)))
        }
        _ => None,
    };
    (ss, bsf)
}

/// Where an export or trim of `src` is written, and its library folder: beside
/// the source; for a draft ("Edit together") in `<clips>\<its folder>`, or the
/// clips folder itself when that folder is gone.
pub fn output_dir(s: &Settings, src: &ClipMeta) -> (PathBuf, String) {
    let clips = PathBuf::from(&s.clips_dir);
    if src.draft {
        let d = clips.join(&src.folder);
        return if !src.folder.is_empty() && d.is_dir() { (d, src.folder.clone()) } else { (clips, String::new()) };
    }
    (Path::new(&src.path).parent().map(Path::to_path_buf).unwrap_or(clips), src.folder.clone())
}

/// Cut `[start, end]` out of a clip without re-encoding. It starts at `start`
/// exactly (an edit list hides the frames from the keyframe before; see `trim_start`).
/// With `include_mic == false` the microphone is left out: the tracks titled
/// "Mix" and "Microphone" are dropped (Mix, Desktop, Microphone, Game keeps
/// Desktop and Game; Mix, Microphone, Game keeps Game). An untitled 3-track
/// clip keeps track 2 (desktop) only.
///
/// A draft ("Edit together") is cut into its folder in the clips folder, and
/// a cut of the whole draft is the joined clip itself: its title, kind
/// "joined", its joins and part names.
pub fn trim(s: &Settings, src: &ClipMeta, start: f64, end: f64, include_mic: bool) -> Result<ClipMeta, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    if end - start < MIN_TRIM {
        return Err("The selection is too short".into());
    }
    let src_path = PathBuf::from(&src.path);
    let (dir, folder) = output_dir(s, src);
    let duration = if src.duration > 0.0 { src.duration } else { library::probe(&ffmpeg, &src_path, false).map(|p| p.duration).unwrap_or(0.0) };
    let whole_draft = src.draft && include_mic && start <= 0.002 && duration > 0.0 && end >= duration - 0.002;
    let title = if whole_draft { src.title.clone() } else { format!("{} (trim)", src.title) };
    let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
    let (ss, bsf) = trim_start(&ffmpeg, &src_path, start);
    let mut cmd = util::hidden_cmd(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &ss, "-to", &format!("{end:.3}"), "-i"]).arg(&src_path);
    if let Some(b) = &bsf {
        cmd.args(["-bsf:v", b]);
    }
    let names = audio_titles(&ffmpeg, &src_path);
    let titled = names.iter().any(|n| n == "Mix" || n == "Microphone");
    let kept: Vec<usize> = if !include_mic && titled {
        let keep: Vec<usize> = (0..names.len()).filter(|&i| names[i] != "Mix" && names[i] != "Microphone").collect();
        cmd.args(["-map", "0:v:0"]);
        for (o, i) in keep.iter().enumerate() {
            cmd.arg("-map").arg(format!("0:a:{i}")).arg(format!("-disposition:a:{o}")).arg(if o == 0 { "default" } else { "0" });
        }
        keep
    } else if !include_mic && src.audio_tracks >= 3 {
        cmd.args(["-map", "0:v:0", "-map", "0:a:1", "-disposition:a:0", "default"]);
        if src.audio_tracks >= 4 {
            cmd.args(["-map", "0:a:3", "-disposition:a:1", "0"]);
            vec![1, 3]
        } else {
            vec![1]
        }
    } else {
        cmd.args(["-map", "0"]);
        (0..names.len()).collect()
    };
    // A stream copy loses the track titles (MP4 hands them back as "name", which
    // is not written out again); the editor tells the tracks apart by them.
    for (o, i) in kept.iter().enumerate() {
        if let Some(n) = names.get(*i).filter(|n| !n.is_empty()) {
            cmd.arg(format!("-metadata:s:a:{o}")).arg(format!("title={n}"));
        }
    }
    cmd.args(["-c", "copy", "-movflags", "+faststart"]).arg(&out);
    let output = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    if !output.status.success() || !out.is_file() {
        let _ = fs::remove_file(&out);
        return Err(format!("Trim failed: {}", util::last_lines(&output.stderr, 2)));
    }
    let mut meta = library::register(&ffmpeg, &out, &title, &src.game, if whole_draft { "joined" } else { "trim" });
    // The file's average rate reflects how busy the screen was, not the capture setting.
    meta.fps = src.fps;
    // Written beside the source (a draft: in its folder), so it sits in that library folder.
    meta.folder = folder;
    if whole_draft {
        meta.joins = src.joins.clone();
        meta.join_names = src.join_names.clone();
    }
    Ok(meta)
}
