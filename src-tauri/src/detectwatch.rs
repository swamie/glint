//! Test-only: `glint.exe --detect-watch <dir> [options]`. Watches GAME mode on this PC with real
//! games, through the same path a running Glint takes: a headless `Core` in game mode whose
//! two-second look (`Core::track_game`: `capmode::found`, `gamedetect::decide`, `capmode::follow`,
//! the game's sound) runs exactly as in the app. A normal run never takes this path.
//!
//! Every look is one JSON line in `<dir>/looks.jsonl`: the window in front, the decision and its
//! reason, every candidate with its evidence and points, the windows whose capture failed or that
//! run in exclusive fullscreen, and what ffmpeg really captures (`RecStatus::source`, frames, error).
//! Each change of what is recorded is written to `<dir>/watch.log` with the full window list, and a
//! few seconds later a real clip is saved from the replay buffer (`Core::clip_blocking`, the clip key's
//! path) and checked: frames and distinct frames, four pictures as PNG with their mean color, a
//! picture of the monitor taken at the same moment for comparison, and the level of each sound track.
//!
//! Options: `--secs N` (how long to watch, default 900), `--pick <exe or path>` (a hand pick, through
//! the menu's own `apply_mark`), `--method auto|wgc|dda`, `--encoder <e>`, `--clip-secs N` (default 5),
//! `--every N` (also clip every N seconds), `--settle N` (seconds after a change before the clip,
//! default 7). Refuses to run without `GLINT_DATA_DIR`. Reads only window facts, as the app does; it
//! never opens, reads or touches a game's process. Window titles and paths stay in `<dir>`: keep that
//! folder out of the repository.
use crate::core::Core;
use crate::{capmode, games, util, win};
use serde_json::json;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

fn opt<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn append(path: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// The effective DPI of every monitor handle (scaling 100 % = 96).
fn monitor_dpis() -> Vec<(String, u32)> {
    use windows::Win32::Graphics::Gdi::HMONITOR;
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    win::monitor_handles()
        .into_iter()
        .map(|(h, name)| {
            let (mut x, mut y) = (0u32, 0u32);
            let ok = unsafe { GetDpiForMonitor(HMONITOR(h as usize as *mut _), MDT_EFFECTIVE_DPI, &mut x, &mut y) }.is_ok();
            (name, if ok { x } else { 0 })
        })
        .collect()
}

fn source_name(s: Option<crate::gamedetect::Source>) -> String {
    s.map(|v| format!("{v:?}")).unwrap_or_else(|| "-".into())
}

/// One window as the log shows it.
fn win_line(w: &win::WinInfo) -> String {
    format!(
        "{:<30} hwnd {:#x} pid {} class {:<26} {}x{} mon {} popup {} caption {} full {} min {} fg {} cloaked {} excluded {} hosted {} title {:?}",
        games::exe_key(&w.exe),
        w.hwnd,
        w.pid,
        w.class.chars().take(26).collect::<String>(),
        w.width,
        w.height,
        w.monitor + 1,
        w.popup as u8,
        w.caption as u8,
        w.fullscreen as u8,
        w.minimized as u8,
        w.foreground as u8,
        w.cloaked as u8,
        w.excluded as u8,
        w.hosted as u8,
        w.title.chars().take(50).collect::<String>()
    )
}

/// Run ffmpeg with `args`, return (stdout, stderr).
fn ff(ffmpeg: &Path, args: &[&str]) -> (Vec<u8>, String) {
    match util::hidden_cmd(ffmpeg).args(args).stdin(Stdio::null()).output() {
        Ok(o) => (o.stdout, String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (Vec::new(), e.to_string()),
    }
}

/// What a saved clip holds: (frames, distinct frames, duration, [(time, mean RGB, png)], [(track, mean dB, max dB)]).
fn inspect_clip(ffmpeg: &Path, clip: &Path, out_dir: &Path, tag: &str) -> serde_json::Value {
    let c = clip.to_string_lossy().into_owned();
    // Frames and distinct frames (a frozen or failed capture repeats one picture).
    let (md5, _) = ff(ffmpeg, &["-hide_banner", "-v", "error", "-i", &c, "-map", "0:v", "-f", "framemd5", "-"]);
    let md5 = String::from_utf8_lossy(&md5).into_owned();
    let hashes: Vec<&str> = md5.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split(',').last()).map(str::trim).collect();
    let mut distinct: Vec<&str> = hashes.clone();
    distinct.sort_unstable();
    distinct.dedup();
    // Duration and streams from ffmpeg's own description.
    let (_, info) = ff(ffmpeg, &["-hide_banner", "-i", &c]);
    let duration = info
        .lines()
        .find_map(|l| l.trim().strip_prefix("Duration: "))
        .and_then(|d| d.split(',').next())
        .and_then(|d| {
            let p: Vec<f64> = d.split(':').filter_map(|x| x.trim().parse().ok()).collect();
            (p.len() == 3).then(|| p[0] * 3600.0 + p[1] * 60.0 + p[2])
        })
        .unwrap_or(0.0);
    let video_line = info.lines().find(|l| l.contains("Video:")).unwrap_or("").trim().to_string();
    let titles: Vec<String> = info.lines().filter(|l| l.trim_start().starts_with("title")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()).collect();
    // Four pictures across the clip, each with its mean color.
    let mut pics = Vec::new();
    for k in 1..=4 {
        let t = duration * k as f64 / 5.0;
        let png = out_dir.join(format!("{tag}-frame{k}.png"));
        let ts = format!("{t:.2}");
        let _ = ff(ffmpeg, &["-hide_banner", "-v", "error", "-y", "-ss", &ts, "-i", &c, "-frames:v", "1", &png.to_string_lossy()]);
        let (px, _) = ff(ffmpeg, &["-hide_banner", "-v", "error", "-ss", &ts, "-i", &c, "-frames:v", "1", "-vf", "scale=1:1:flags=area", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
        let rgb = (px.len() >= 3).then(|| [px[0], px[1], px[2]]);
        pics.push(json!({ "t": (t * 100.0).round() / 100.0, "rgb": rgb, "png": png.file_name().map(|f| f.to_string_lossy().into_owned()) }));
    }
    // The level of each sound track.
    let tracks = info.matches("Audio:").count();
    let mut levels = Vec::new();
    for n in 0..tracks {
        let map = format!("0:a:{n}");
        let (_, e) = ff(ffmpeg, &["-hide_banner", "-i", &c, "-map", &map, "-af", "volumedetect", "-f", "null", "-"]);
        let grab = |key: &str| e.lines().find_map(|l| l.split(key).nth(1)).and_then(|v| v.trim().trim_end_matches(" dB").parse::<f64>().ok());
        levels.push(json!({ "track": n, "title": titles.get(n + 1).or(titles.get(n)).cloned().unwrap_or_default(), "mean_db": grab("mean_volume:"), "max_db": grab("max_volume:") }));
    }
    json!({ "frames": hashes.len(), "distinct": distinct.len(), "duration": duration, "video": video_line, "pictures": pics, "audio": levels })
}

pub fn run(args: &[String]) {
    let dir = PathBuf::from(args.first().cloned().unwrap_or_else(|| "detect-watch".into()));
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join("watch.log");
    let looks = dir.join("looks.jsonl");
    let clips_log = dir.join("clips.jsonl");
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        append(&log, "REFUSED: set GLINT_DATA_DIR so the real settings and library are never touched");
        return;
    }
    crate::livecheck::dpi_aware();
    let secs: u64 = opt(args, "--secs").unwrap_or(900);
    let clip_secs: u32 = opt(args, "--clip-secs").unwrap_or(5);
    let every: u64 = opt(args, "--every").unwrap_or(0);
    let settle: u64 = opt(args, "--settle").unwrap_or(7);
    let pick: Option<String> = opt(args, "--pick");

    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.capture_mode = "game".into();
        s.clips_dir = dir.join("clips").to_string_lossy().into_owned();
        s.sound = false;
        s.toast = false;
        if let Some(m) = opt::<String>(args, "--method") {
            s.method = m;
        }
        if let Some(e) = opt::<String>(args, "--encoder") {
            s.encoder = e;
        }
        if let Some(p) = &pick {
            if let Err(e) = capmode::apply_mark(&mut s, "pick", p) {
                append(&log, &format!("pick refused: {e}"));
            }
        }
        s.keybinds = vec![crate::settings::Keybind { id: "watch".into(), key: "F13".into(), seconds: 30, ..Default::default() }];
        crate::hotkeys::set_binds(&s.keybinds);
    }
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg");

    // This PC.
    let mut head = String::new();
    let _ = writeln!(head, "== detect-watch started; Glint {}", env!("CARGO_PKG_VERSION"));
    let hw = win::hardware();
    let _ = writeln!(head, "windows build {}; GPUs {:?}; encoder for this PC {}", hw.windows_build, hw.gpus.iter().map(|g| g.name.clone()).collect::<Vec<_>>(), hw.encoder);
    let method = ffmpeg.as_ref().map(|f| crate::recorder::resolve_method(f, &s.method));
    let b = crate::capture2::border_info();
    let _ = writeln!(head, "ffmpeg {:?}; gfxcapture {:?}; method setting {:?} -> {:?}; border: can_remove {} (main {:?}, desktop apps {:?}, device {:?})", ffmpeg.as_ref().map(|f| f.file_name().unwrap_or_default().to_string_lossy().into_owned()), ffmpeg.as_ref().map(|f| crate::recorder::gfx_caps(f)), s.method, method, b.can_remove, b.main, b.desktop_apps, b.device);
    let dpis = monitor_dpis();
    for m in win::monitors() {
        let src = win::monitor_source(m.index);
        let dpi = dpis.iter().find(|(n, _)| n.eq_ignore_ascii_case(&m.name)).map_or(0, |(_, d)| *d);
        let _ = writeln!(head, "monitor {} {}{}: {}x{} @{} Hz at ({},{}), native {}x{}, scaling {}%, dxgi output {:?}", m.index + 1, m.name, if m.primary { " (main)" } else { "" }, m.width, m.height, m.hz, m.x, m.y, m.native_width, m.native_height, dpi * 100 / 96, src.and_then(|s| s.dxgi_output));
    }
    let _ = writeln!(head, "pick {:?} (path {:?}); auto {}; clips of {clip_secs} s, {settle} s after each change{}", s.game_pick, s.game_pick_path, s.game_auto, if every > 0 { format!(", and every {every} s") } else { String::new() });
    append(&log, head.trim_end());

    core.start_recording();
    core.sync_game_audio();
    let t0 = Instant::now();
    let mut last_key = String::new();
    let mut due: Option<(Instant, String)> = None;
    let mut last_clip = Instant::now();
    let mut n_clip = 0u32;
    while t0.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(2));
        core.track_game();
        let v = capmode::watch_view();
        let st = core.rec.stats();
        let wins = win::windows();
        let fg = wins.iter().find(|w| w.foreground);
        let caps: Vec<serde_json::Value> = st
            .captures
            .iter()
            .map(|c| json!({ "window": format!("{:#x}", c.window), "monitor": c.monitor + 1, "method": c.status.method, "encoder": c.status.encoder, "source": c.status.source, "frames": c.frames, "size": format!("{}x{}", c.status.width, c.status.height), "error": c.status.error, "window_failed": c.status.window_failed, "border": c.status.border }))
            .collect();
        let target = v.target.as_ref().map(|t| json!({ "exe": games::exe_key(&t.exe), "name": t.name, "hwnd": format!("{:#x}", t.hwnd), "size": format!("{}x{}", t.width, t.height), "monitor": t.monitor + 1 }));
        let cands: Vec<serde_json::Value> = v.candidates.iter().map(|c| json!({ "exe": c.exe, "name": c.name, "score": c.score, "evidence": c.evidence })).collect();
        let t = t0.elapsed().as_secs_f64();
        let line = json!({
            "t": (t * 10.0).round() / 10.0,
            "front": fg.map(|w| games::exe_key(&w.exe)),
            "d3d_fullscreen": win::d3d_fullscreen(),
            "why": v.why.line(),
            "code": v.why.code,
            "source": source_name(v.source),
            "target": target,
            "candidates": cands,
            "failed": v.failed.iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>(),
            "exclusive": v.exclusive.iter().map(|h| format!("{h:#x}")).collect::<Vec<_>>(),
            "captures": caps,
        });
        append(&looks, &line.to_string());
        // A change of what is recorded: the whole picture into the log, and a clip once it settled.
        let src_now = st.captures.first().map(|c| c.status.source.clone()).unwrap_or_default();
        let key = format!("{}|{}|{}|{:?}|{}", v.why.code, v.why.game, source_name(v.source), v.target.as_ref().map(|t| (t.hwnd, t.width, t.height)), src_now);
        if key != last_key {
            let mut block = String::new();
            let _ = writeln!(block, "\n[{t:7.1} s] {} -> source {}; ffmpeg source {:?}", v.why.line(), source_name(v.source), src_now);
            let _ = writeln!(block, "  target {}", line["target"]);
            let _ = writeln!(block, "  candidates {}", line["candidates"]);
            let _ = writeln!(block, "  failed {} exclusive {} d3d_fullscreen {}", line["failed"], line["exclusive"], line["d3d_fullscreen"]);
            let _ = writeln!(block, "  captures {}", line["captures"]);
            for w in &wins {
                let _ = writeln!(block, "    {}", win_line(w));
            }
            append(&log, block.trim_end());
            last_key = key;
            due = Some((Instant::now() + Duration::from_secs(settle), v.why.code.clone()));
        }
        if every > 0 && last_clip.elapsed() >= Duration::from_secs(every) && due.is_none() {
            due = Some((Instant::now(), "every".into()));
        }
        let Some((at, why)) = due.clone() else { continue };
        if Instant::now() < at {
            continue;
        }
        due = None;
        last_clip = Instant::now();
        n_clip += 1;
        let tag = format!("clip{n_clip:03}");
        // The monitor as the user sees it now, for comparison with the clip's pictures.
        let mon = v.target.as_ref().map_or(0, |t| t.monitor);
        let ref_png = dir.join(format!("{tag}-monitor.png"));
        let ref_rgb = match crate::monprev::grab_raw(mon, 640) {
            Ok((px, w, h, _)) => {
                let _ = std::fs::write(&ref_png, crate::monprev::png_rgb(&px, w as u32, h as u32).unwrap_or_default());
                let n = (px.len() / 4).max(1) as u64;
                let sum = |o: usize| (px.chunks_exact(4).map(|p| p[o] as u64).sum::<u64>() / n) as u8;
                Some([sum(2), sum(1), sum(0)])
            }
            Err(_) => None,
        };
        let result = match core.clip_blocking(clip_secs, -1) {
            Ok(meta) => {
                let info = ffmpeg.as_ref().map(|f| inspect_clip(f, Path::new(&meta.path), &dir, &tag)).unwrap_or(json!(null));
                json!({ "tag": tag, "t": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0, "after": why, "why": v.why.line(), "source": source_name(v.source), "ffmpeg_source": src_now, "game": meta.game, "size": format!("{}x{}", meta.width, meta.height), "tracks": meta.audio_tracks, "monitor_now_rgb": ref_rgb, "clip": info })
            }
            Err(e) => json!({ "tag": tag, "t": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0, "after": why, "why": v.why.line(), "source": source_name(v.source), "error": e }),
        };
        append(&clips_log, &result.to_string());
        let c = &result["clip"];
        append(&log, &format!("  -> {tag}: {} | frames {} distinct {} | pictures {} | audio {}", result.get("error").map(|e| e.to_string()).unwrap_or_else(|| result["size"].to_string()), c["frames"], c["distinct"], c["pictures"].as_array().map(|p| p.iter().map(|x| x["rgb"].to_string()).collect::<Vec<_>>().join(" ")).unwrap_or_default(), c["audio"]));
    }
    core.rec.stop();
    crate::procaudio::global().off();
    append(&log, &format!("== detect-watch ended after {:.0} s, {n_clip} clips", t0.elapsed().as_secs_f64()));
}
