//! Headless benchmark: `glint.exe --bench <report.json> [--height N] [--fps N]
//! [--secs N] [--clip N] [--quality q] [--bitrate mbps] [--keys]`.
//!
//! Runs the real recorder (video + audio) without any window, measures steady
//! state cost, saves one clip and verifies it with ffprobe. `--keys` saves the
//! clip through the global hotkey path instead of calling the recorder directly.
use crate::core::{capture_cfg, Core};
use crate::library;
use crate::util;
use crate::win;
use serde_json::json;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

fn arg<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn gpu_sample() -> Option<(f64, f64)> {
    // hidden_cmd: a console child of a GUI process would otherwise pop a window and steal focus.
    let out = util::hidden_cmd(std::path::Path::new("nvidia-smi"))
        .args(["--query-gpu=utilization.encoder,utilization.gpu", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.lines().next()?.split(',').map(|v| v.trim().parse::<f64>().ok());
    Some((it.next()??, it.next()??))
}

pub fn run(args: &[String]) {
    let report_path = args.first().cloned().unwrap_or_else(|| "bench.json".into());
    let secs: u64 = arg(args, "--secs").unwrap_or(20);
    let clip_secs: u32 = arg(args, "--clip").unwrap_or(15);
    let use_keys = args.iter().any(|a| a == "--keys");

    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = util::data_dir().join("clips").to_string_lossy().into_owned();
        s.sound = false;
        if let Some(h) = arg::<u32>(args, "--height") {
            s.height = h;
        }
        if let Some(f) = arg::<u32>(args, "--fps") {
            s.fps = f;
        }
        if let Some(q) = arg::<String>(args, "--quality") {
            s.quality = q;
        }
        if let Some(b) = arg::<u32>(args, "--bitrate") {
            s.bitrate = b;
        }
        s.game_folders = args.iter().any(|a| a == "--game-folders");
        if let Some(m) = arg::<String>(args, "--method") {
            s.method = m;
        }
        // --buffer ram|mixed|disk, --ram-secs N: where the replay buffer lives.
        if let Some(b) = arg::<String>(args, "--buffer") {
            s.buffer = b;
        }
        if let Some(n) = arg::<u32>(args, "--ram-secs") {
            s.buffer_ram_secs = n;
        }
        // One binding sized to the benchmark so the buffer is exactly as large as needed.
        s.keybinds = vec![crate::settings::Keybind { id: "bench".into(), key: "F13".into(), seconds: clip_secs.max(secs as u32), ..Default::default() }];
        crate::hotkeys::set_binds(&s.keybinds);
    }
    let cfg = capture_cfg(&core.settings());
    core.start_recording();

    // Warm-up: encoder session, first keyframe, audio streams.
    thread::sleep(Duration::from_secs(4));
    let warm = core.rec.stats();
    let pid = warm.status.pid;
    let (cpu_self0, _) = win::proc_stats(0).unwrap_or((0.0, 0));
    let (cpu_ff0, _) = win::proc_stats(pid).unwrap_or((0.0, 0));
    let t0 = Instant::now();
    let frames0 = warm.frames;

    let mut gpu: Vec<(f64, f64)> = Vec::new();
    let mut ram_ff_peak = 0u64;
    for i in 0..secs {
        thread::sleep(Duration::from_secs(1));
        if i % 3 == 1 {
            if let Some(g) = gpu_sample() {
                gpu.push(g);
            }
        }
        if let Some((_, ram)) = win::proc_stats(pid) {
            ram_ff_peak = ram_ff_peak.max(ram);
        }
    }

    let dt = t0.elapsed().as_secs_f64();
    let st = core.rec.stats();
    let (cpu_self1, ram_self) = win::proc_stats(0).unwrap_or((0.0, 0));
    let (cpu_ff1, _) = win::proc_stats(pid).unwrap_or((0.0, 0));
    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let fps_actual = (st.frames - frames0) as f64 / dt;
    let restarted = st.status.pid != pid;

    // Save a clip and time it.
    let t_clip = Instant::now();
    let clip = if use_keys {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        crate::hotkeys::start(tx);
        thread::sleep(Duration::from_millis(300));
        crate::keys::tap(0x7C, &[]); // F13
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(id) if id == "bench" => core.clip_blocking(clip_secs),
            Ok(id) => Err(format!("unexpected hotkey id {id}")),
            Err(_) => Err("hotkey was not received".into()),
        }
    } else {
        core.clip_blocking(clip_secs)
    };
    let clip_ms = t_clip.elapsed().as_millis() as u64;
    core.rec.stop();

    let mut clip_json = json!({ "ok": false });
    match clip {
        Ok(meta) => {
            let probe = cfg.ffmpeg.as_ref().and_then(|f| library::probe(f, std::path::Path::new(&meta.path), true));
            clip_json = json!({
                "ok": true,
                "requested_secs": clip_secs,
                "duration_meta": meta.duration,
                "save_ms": clip_ms,
                "size_mb": meta.size as f64 / 1e6,
                "audio_tracks": meta.audio_tracks,
                "thumb": !meta.thumb.is_empty(),
                "probe": probe,
                "path": meta.path,
            });
        }
        Err(e) => clip_json["error"] = json!(e),
    }

    let avg = |f: fn(&(f64, f64)) -> f64| if gpu.is_empty() { None } else { Some(gpu.iter().map(f).sum::<f64>() / gpu.len() as f64) };
    let report = json!({
        "config": { "width": cfg.width, "height": cfg.height, "fps": cfg.fps, "quality": cfg.quality, "bitrate_mbps": cfg.bitrate, "scaled": cfg.scaled, "maxrate_mbps": cfg.maxrate() as f64 / 1e6 },
        "encoder": st.status.encoder,
        "method": st.status.method,
        "error": st.status.error,
        "ffmpeg_restarted": restarted,
        "measured_secs": dt,
        "fps_actual": fps_actual,
        "fps_ratio": fps_actual / cfg.fps as f64,
        "cpu_pct_total": ((cpu_self1 - cpu_self0) + (cpu_ff1 - cpu_ff0)) / dt / cores * 100.0,
        "cpu_pct_one_core": ((cpu_self1 - cpu_self0) + (cpu_ff1 - cpu_ff0)) / dt * 100.0,
        "cpu_pct_glint": (cpu_self1 - cpu_self0) / dt / cores * 100.0,
        "cpu_pct_ffmpeg": (cpu_ff1 - cpu_ff0) / dt / cores * 100.0,
        "ram_ffmpeg_mb": ram_ff_peak as f64 / 1e6,
        "ram_glint_mb": ram_self.saturating_sub(st.video_bytes + st.audio_bytes) as f64 / 1e6,
        "buffer_secs": st.buffer_secs,
        "ram_glint_total_mb": ram_self as f64 / 1e6,
        "buffer_mode": cfg.buffer,
        "buffer_ram_secs": cfg.buffer_ram_secs,
        "buffer_video_mb": st.video_bytes as f64 / 1e6,
        "buffer_audio_mb": st.audio_bytes as f64 / 1e6,
        "buffer_video_disk_mb": st.video_disk_bytes as f64 / 1e6,
        "buffer_audio_disk_mb": st.audio_disk_bytes as f64 / 1e6,
        "buffer_files_left": std::fs::read_dir(&cfg.buffer_dir).map(|d| d.flatten().count()).unwrap_or(0),
        "video_mbps": if st.buffer_secs > 0.0 { (st.video_bytes + st.video_disk_bytes) as f64 * 8.0 / 1e6 / st.buffer_secs } else { 0.0 },
        "gpu_encoder_pct": avg(|g| g.0),
        "gpu_util_pct": avg(|g| g.1),
        "clip": clip_json,
    });
    let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap_or_default());
}

/// `glint.exe --folders-test <report.txt>`: move, rename and delete library
/// folders on scratch files. Refuses to run without `GLINT_DATA_DIR`, so it can
/// never touch the real library.
pub fn folders_test(report_path: &str) {
    use std::path::{Path, PathBuf};
    let mut lines: Vec<String> = Vec::new();
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = std::fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let core = Core::new();
    let root: PathBuf = util::data_dir().join("clips");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    core.settings.lock().unwrap().clips_dir = root.to_string_lossy().into_owned();
    core.lib.lock().unwrap().clips.clear();
    let mut check = |name: &str, ok: bool| lines.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));

    for id in ["a", "b", "c"] {
        let path = root.join(format!("{id}.mp4"));
        let _ = std::fs::write(&path, id.as_bytes());
        core.lib.lock().unwrap().add(library::ClipMeta { id: id.into(), path: path.to_string_lossy().into_owned(), title: id.into(), ..Default::default() });
    }
    let get = |id: &str| core.lib.lock().unwrap().get(id).unwrap();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    check("create folder cleans the name", core.create_folder("Best: of?").as_deref() == Ok("Best of"));
    check("duplicate folder is refused", core.create_folder("best of").is_err());
    check("empty folder name is refused", core.create_folder("  . ").is_err());
    check("folders are listed", core.folders() == vec!["Best of".to_string()]);

    check("move two clips", core.move_clips(&ids(&["a", "b"]), "Best of") == Ok(2));
    check("file moved on disk", root.join("Best of").join("a.mp4").is_file() && !root.join("a.mp4").exists());
    check("clip records follow", get("a").folder == "Best of" && Path::new(&get("a").path).is_file() && get("c").folder.is_empty());
    check("moving again is a no-op", core.move_clips(&ids(&["a", "b"]), "Best of") == Ok(0));

    // A name clash in the target must not overwrite the file that is there.
    let _ = std::fs::write(root.join("Best of").join("c.mp4"), b"other");
    check("move with a name clash", core.move_clips(&ids(&["c"]), "Best of") == Ok(1));
    check("clash kept both files", std::fs::read(root.join("Best of").join("c.mp4")).ok().as_deref() == Some(&b"other"[..]) && get("c").path.ends_with("c (2).mp4") && std::fs::read(get("c").path).ok().as_deref() == Some(&b"c"[..]));

    check("move to a new folder creates it", core.move_clips(&ids(&["b"]), "Second") == Ok(1) && root.join("Second").join("b.mp4").is_file());
    check("rename folder", core.rename_folder("Best of", "Top").as_deref() == Ok("Top"));
    check("rename moved the files", root.join("Top").join("a.mp4").is_file() && get("a").folder == "Top" && Path::new(&get("a").path).is_file() && !root.join("Best of").exists());
    check("rename onto an existing folder is refused", core.rename_folder("Top", "Second").is_err());

    // The stray c.mp4 in Top is not in the library: deleting the folder must keep it.
    check("delete folder with a stray file keeps the folder", core.delete_folder("Top").is_err() && root.join("Top").join("c.mp4").is_file());
    check("its clips went back to the main folder", get("a").folder.is_empty() && root.join("a.mp4").is_file() && root.join("c (2).mp4").is_file());
    check("delete folder moves clips out and removes it", core.delete_folder("Second").is_ok() && !root.join("Second").exists() && root.join("b.mp4").is_file() && get("b").folder.is_empty());
    check("move back to the main folder is a no-op there", core.move_clips(&ids(&["a"]), "") == Ok(0));

    // Rescan picks up a file found in a subfolder and records its folder.
    core.rescan();
    let found = core.lib.lock().unwrap().clips.iter().find(|c| c.path.ends_with("Top\\c.mp4")).cloned();
    check("rescan indexes subfolders", found.map(|c| c.folder == "Top").unwrap_or(false));
    check("no clip record lost", ["a", "b", "c"].iter().all(|id| Path::new(&get(id).path).is_file()));

    // Game folders. a, b and c are loose in the clips folder at this point.
    for (id, game) in [("a", "Overwatch"), ("b", "Risk of Rain 2"), ("c", "Overwatch")] {
        core.lib.lock().unwrap().update(id, |c| c.game = game.into());
    }
    check("sort by game moves the loose clips", core.sort_by_game() == Ok(3));
    check("each clip is in its game's folder", get("a").folder == "Overwatch" && get("c").folder == "Overwatch" && get("b").folder == "Risk of Rain 2" && root.join("Overwatch").join("a.mp4").is_file() && root.join("Risk of Rain 2").join("b.mp4").is_file());
    check("sorting again moves nothing", core.sort_by_game() == Ok(0));
    core.settings.lock().unwrap().game_folders = true;
    check("retag with game folders on moves the file", core.set_game(&ids(&["a"]), "Fortnite").is_ok() && get("a").game == "Fortnite" && get("a").folder == "Fortnite" && root.join("Fortnite").join("a.mp4").is_file());
    // b is moved by hand to a custom folder: a retag must leave it there.
    let _ = core.move_clips(&ids(&["b"]), "Best");
    check("retag leaves a hand-filed clip alone", core.set_game(&ids(&["b"]), "Fortnite").is_ok() && get("b").game == "Fortnite" && get("b").folder == "Best");
    core.settings.lock().unwrap().game_folders = false;
    check("retag with game folders off moves nothing", core.set_game(&ids(&["c"]), "Deadlock").is_ok() && get("c").folder == "Overwatch");
    check("unknown game goes to Other", util::game_folder("  ") == "Other" && util::game_folder("Overwatch: 2?") == "Overwatch 2");

    let _ = std::fs::write(report_path, lines.join("\n") + "\n");
}

/// `glint.exe --diag <report.txt> [exe-for-icon] [discord-app-id]`: headless
/// checks of the aspect-ratio maths, game naming and overlay text, plus what
/// this machine reports (display modes, foreground game, an exe's icon, and a
/// Discord handshake if an id is given). Needs `GLINT_DATA_DIR`.
pub fn diag(report_path: &str, icon_exe: Option<&str>, discord_id: Option<&str>) {
    use crate::core::{captured_text, output_size};
    use crate::win::Foreground;
    let lines: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = std::fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let check = |name: &str, ok: bool| lines.borrow_mut().push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));

    // Aspect ratio: (display mode, the display's own shape, height setting, aspect setting) -> clip size.
    check("3:2 stretched on a 16:9 display is widened", output_size((2160, 1440), (2560, 1440), 0, "auto") == (2560, 1440));
    check("4:3 stretched in game is widened", output_size((1440, 1080), (2560, 1440), 0, "auto") == (1920, 1080));
    check("16:10 stretched on 16:9 is widened", output_size((1680, 1050), (1920, 1080), 0, "auto") == (1866, 1050));
    check("native 16:9 is untouched", output_size((2560, 1440), (2560, 1440), 0, "auto") == (2560, 1440));
    check("ultrawide is untouched and scales in proportion", output_size((3440, 1440), (3440, 1440), 1080, "auto") == (2580, 1080));
    check("a mode wider than the display is untouched", output_size((1920, 1080), (2560, 1600), 0, "auto") == (1920, 1080));
    check("stretched with a lower height setting", output_size((1280, 960), (1920, 1080), 720, "auto") == (1280, 720));
    check("'source' never changes the shape", output_size((1440, 1080), (2560, 1440), 0, "source") == (1440, 1080));
    check("a fixed ratio is applied", output_size((2560, 1440), (2560, 1440), 0, "4:3") == (1920, 1440) && output_size((1440, 1080), (1920, 1080), 0, "16:9") == (1920, 1080));
    check("a bad ratio falls back to the source", output_size((1920, 1080), (1920, 1080), 0, "wide") == (1920, 1080));
    check("17:9 mode next to 16:9 counts as 16:9", win::native_mode(&[(1920, 1080), (4096, 2160), (3840, 2160)]) == (3840, 2160));
    check("largest mode is the display's shape", win::native_mode(&[(2160, 1440), (2560, 1440), (1920, 1080)]) == (2560, 1440));

    // Game naming.
    let fg = |exe: &str, title: &str, fullscreen: bool| Foreground { pid: 1, exe: exe.into(), title: title.into(), fullscreen };
    let id = |exe: &str, title: &str, fs: bool| crate::games::identify(&fg(exe, title, fs));
    check("Steam game is named after its folder", id(r"D:\SteamLibrary\steamapps\common\Risk of Rain 2\Risk of Rain 2.exe", "", false).as_deref() == Some("Risk of Rain 2"));
    check("known exe gets its proper name", id(r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe", "", true).as_deref() == Some("Valorant"));
    check("known exe outside a game folder, windowed", id(r"C:\Program Files (x86)\Overwatch\_retail_\Overwatch.exe", "", false).as_deref() == Some("Overwatch"));
    check("Minecraft is told from Java by its title", id(r"C:\Program Files\Java\bin\javaw.exe", "Minecraft 1.21.4", false).as_deref() == Some("Minecraft") && id(r"C:\Program Files\Java\bin\javaw.exe", "Some Tool", true).is_none());
    check("a fullscreen browser is not a game", id(r"C:\Program Files\Google\Chrome\Application\chrome.exe", "YouTube", true).is_none());
    check("a launcher in a game folder is not a game", id(r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\wallpaper64.exe", "", true).is_none());
    check("a windowed unknown app is not a game", id(r"C:\Tools\thing.exe", "", false).is_none());
    check("a fullscreen unknown app is treated as a game", id(r"C:\Nowhere\MyIndieGame.exe", "", true).as_deref() == Some("MyIndieGame"));

    // Overlay text.
    let mut s = crate::settings::Settings::default();
    check("overlay text: length and game", captured_text(&s, 30.0, "Overwatch") == "Captured last 30s of Overwatch");
    check("overlay text: desktop", captured_text(&s, 600.0, "Desktop") == "Captured last 10m 00s of the desktop");
    s.toast_game = false;
    check("overlay text: length only", captured_text(&s, 30.0, "Overwatch") == "Captured last 30s");
    s.toast_length = false;
    check("overlay text: nothing", captured_text(&s, 30.0, "Overwatch").is_empty());

    // What this machine reports.
    for m in win::monitors() {
        let (w, h) = output_size((m.width, m.height), (m.native_width, m.native_height), 0, "auto");
        lines.borrow_mut().push(format!("INFO monitor {}: mode {}x{} @ {} Hz, display shape {}x{}, auto clip size {}x{}", m.index, m.width, m.height, m.hz, m.native_width, m.native_height, w, h));
    }
    match win::foreground() {
        Some(f) => lines.borrow_mut().push(format!("INFO foreground: fullscreen={} game={:?} exe={}", f.fullscreen, crate::games::identify(&f), std::path::Path::new(&f.exe).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
        None => lines.borrow_mut().push("INFO foreground: none".into()),
    }
    if let Some(exe) = icon_exe {
        let mut games = crate::games::Games::load();
        games.note("Icon Test", exe);
        let icon = games.list.iter().find(|g| g.name == "Icon Test").map(|g| g.icon.clone()).unwrap_or_default();
        check("an exe's icon is saved as a PNG", std::fs::metadata(&icon).map(|m| m.len() > 200).unwrap_or(false));
        lines.borrow_mut().push(format!("INFO icon file: {icon}"));
    }
    if let Some(id) = discord_id {
        match crate::discord::Client::connect(id) {
            Ok(_) => lines.borrow_mut().push("INFO discord: connected".into()),
            Err(e) => lines.borrow_mut().push(format!("INFO discord: {e}")),
        }
    }
    let _ = std::fs::write(report_path, lines.borrow().join("\n") + "\n");
}

