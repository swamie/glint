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
        // --monitor N: the capture monitor. --also-monitor N: a second clip keybind
        // for monitor N, so both monitors are buffered at once and one clip is saved from each.
        if let Some(m) = arg::<u32>(args, "--monitor") {
            s.monitor = m;
        }
        if let Some(m) = arg::<i32>(args, "--also-monitor") {
            s.keybinds.push(crate::settings::Keybind { id: "bench2".into(), key: "F14".into(), seconds: clip_secs.max(secs as u32), monitor: m, ..Default::default() });
        }
        crate::hotkeys::set_binds(&s.keybinds);
    }
    let cfgs = crate::core::capture_cfgs(&core.settings());
    let cfg = capture_cfg(&core.settings());
    core.start_recording();

    // Every ffmpeg of the recorder, for CPU and RAM.
    let ff_stats = |pids: &[u32]| pids.iter().filter_map(|p| win::proc_stats(*p)).fold((0.0, 0u64), |(c, r), (c2, r2)| (c + c2, r + r2));
    // Warm-up: encoder session, first keyframe, audio streams.
    thread::sleep(Duration::from_secs(4));
    let warm = core.rec.stats();
    let pids: Vec<u32> = warm.captures.iter().map(|c| c.status.pid).collect();
    let (cpu_self0, _) = win::proc_stats(0).unwrap_or((0.0, 0));
    let (cpu_ff0, _) = ff_stats(&pids);
    let t0 = Instant::now();
    let _ = core.status(); // starts the per-capture frame-rate sample

    let mut gpu: Vec<(f64, f64)> = Vec::new();
    let mut ram_ff_peak = 0u64;
    let mut ram_total_peak = 0u64;
    for i in 0..secs {
        thread::sleep(Duration::from_secs(1));
        if i % 3 == 1 {
            if let Some(g) = gpu_sample() {
                gpu.push(g);
            }
        }
        ram_ff_peak = ram_ff_peak.max(ff_stats(&pids).1);
        ram_total_peak = ram_total_peak.max(win::tree_working_set());
    }

    let dt = t0.elapsed().as_secs_f64();
    let st = core.rec.stats();
    let status = core.status();
    let (cpu_self1, ram_self) = win::proc_stats(0).unwrap_or((0.0, 0));
    let (cpu_ff1, _) = ff_stats(&pids);
    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    // Frames per monitor over the measured stretch.
    let fps_of = |id: u64| {
        let before = warm.captures.iter().find(|c| c.id == id).map_or(0, |c| c.frames);
        st.captures.iter().find(|c| c.id == id).map_or(0.0, |c| c.frames.saturating_sub(before) as f64 / dt)
    };
    let fps_actual = st.captures.first().map_or(0.0, |c| fps_of(c.id));
    let restarted = st.captures.iter().map(|c| c.status.pid).collect::<Vec<_>>() != pids;
    let captures_json: Vec<serde_json::Value> = st
        .captures
        .iter()
        .map(|c| {
            json!({
                "monitor": c.monitor,
                "primary": c.primary,
                "width": c.status.width,
                "height": c.status.height,
                "fps": c.status.fps,
                "fps_actual": fps_of(c.id),
                "fps_ratio": fps_of(c.id) / c.status.fps.max(1) as f64,
                "encoder": c.status.encoder,
                "method": c.status.method,
                "error": c.status.error,
                "buffer_secs": c.buffer_secs,
                "buffer_max": c.max_secs - 5.0,
                "video_mb": c.video_bytes as f64 / 1e6,
                "video_disk_mb": c.video_disk_bytes as f64 / 1e6,
                "video_mbps": if c.buffer_secs > 0.0 { (c.video_bytes + c.video_disk_bytes) as f64 * 8.0 / 1e6 / c.buffer_secs } else { 0.0 },
            })
        })
        .collect();

    // Save a clip of each buffered monitor and time it.
    let mut clips_json: Vec<serde_json::Value> = Vec::new();
    for (n, c) in cfgs.iter().enumerate() {
        let monitor = if n == 0 { -1 } else { c.monitor as i32 };
        let t_clip = Instant::now();
        let clip = if use_keys && n == 0 {
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            crate::hotkeys::start(tx);
            thread::sleep(Duration::from_millis(300));
            crate::keys::tap(0x7C, &[]); // F13
            match rx.recv_timeout(Duration::from_secs(3)) {
                Ok(id) if id == "bench" => core.clip_blocking(clip_secs, -1),
                Ok(id) => Err(format!("unexpected hotkey id {id}")),
                Err(_) => Err("hotkey was not received".into()),
            }
        } else {
            core.clip_blocking(clip_secs, monitor)
        };
        let clip_ms = t_clip.elapsed().as_millis() as u64;
        let mut clip_json = json!({ "ok": false, "monitor": c.monitor, "expected_width": c.width, "expected_height": c.height });
        match clip {
            Ok(meta) => {
                let probe = c.ffmpeg.as_ref().and_then(|f| library::probe(f, std::path::Path::new(&meta.path), true));
                let size_ok = probe.as_ref().is_some_and(|p| p.width == c.width && p.height == c.height);
                clip_json = json!({
                    "ok": true,
                    "monitor": c.monitor,
                    "expected_width": c.width,
                    "expected_height": c.height,
                    "size_matches": size_ok,
                    "meta_size": format!("{}x{} @ {}", meta.width, meta.height, meta.fps),
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
        clips_json.push(clip_json);
    }
    core.rec.stop();
    let clip_json = clips_json.first().cloned().unwrap_or(json!({ "ok": false }));

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
        "ram_total_mb": status.ram_total as f64 / 1e6,
        "ram_total_peak_mb": ram_total_peak as f64 / 1e6,
        "captures": captures_json,
        "status_captures": status.captures,
        "clip": clip_json,
        "clips": clips_json,
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

    // Renaming a clip renames its file. a sits in Fortnite\a.mp4 at this point.
    let names = |dir: &Path| std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
    let fort = root.join("Fortnite");
    let r = core.rename_clip("a", "  My: best/clip?  ");
    check("rename renames the file", r.as_ref().is_ok_and(|m| m.title == "My: best/clip?" && m.path == fort.join("My best clip.mp4").to_string_lossy()) && fort.join("My best clip.mp4").is_file() && !fort.join("a.mp4").exists());
    check("rename is saved in the index", get("a").title == "My: best/clip?" && get("a").path.ends_with("My best clip.mp4") && get("a").folder == "Fortnite" && library::Library::load().get("a").is_some_and(|c| c.path.ends_with("My best clip.mp4")));
    let r = core.rename_clip("a", "my BEST clip");
    check("a change of letter case renames the file", r.is_ok() && names(&fort).contains(&"my BEST clip.mp4".to_string()) && !names(&fort).contains(&"My best clip.mp4".to_string()) && get("a").path.ends_with("my BEST clip.mp4"));
    check("renaming to the same name changes nothing", core.rename_clip("a", "my BEST clip").is_ok_and(|m| m.path.ends_with("my BEST clip.mp4")) && names(&fort).iter().filter(|n| n.to_lowercase().starts_with("my best clip")).count() == 1);
    let _ = std::fs::write(fort.join("Taken.mp4"), b"stray");
    check("a name in use gets a number", core.rename_clip("a", "Taken").is_ok_and(|m| m.path.ends_with("Taken (2).mp4") && m.title == "Taken") && std::fs::read(fort.join("Taken.mp4")).ok().as_deref() == Some(&b"stray"[..]) && std::fs::read(fort.join("Taken (2).mp4")).ok().as_deref() == Some(&b"a"[..]));
    let before = get("a");
    check("a name with nothing usable is refused", core.rename_clip("a", " ?? .. ").is_err() && core.rename_clip("a", "   ").is_err() && get("a").path == before.path && get("a").title == before.title);
    {
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&before.path);
        let r = core.rename_clip("a", "Locked");
        let why = r.as_ref().err().cloned().unwrap_or_default();
        check(&format!("a file in use is refused and left as it was ({why})"), held.is_ok() && r.is_err_and(|e| e.contains("open in another program")) && get("a").path == before.path && get("a").title == before.title && Path::new(&before.path).is_file() && !fort.join("Locked.mp4").exists());
    }
    core.lib.lock().unwrap().add(library::ClipMeta { id: "gone".into(), path: fort.join("gone.mp4").to_string_lossy().into_owned(), title: "gone".into(), folder: "Fortnite".into(), ..Default::default() });
    check("a missing file is refused and the index kept", core.rename_clip("gone", "Back").is_err_and(|e| e.contains("missing")) && get("gone").title == "gone" && get("gone").path.ends_with("gone.mp4"));
    check("an unknown clip is refused", core.rename_clip("nope", "Name").is_err());
    core.lib.lock().unwrap().remove("gone");

    // A keybind that switches the capture monitor: to the next one, or to a named one.
    {
        use crate::settings::Keybind;
        let monitors = crate::win::monitors().len();
        let mut s = core.settings();
        s.monitor = 0;
        s.keybinds = vec![
            Keybind { id: "swap-next".into(), key: "F13".into(), action: "monitor".into(), monitor: -1, ..Default::default() },
            Keybind { id: "swap-first".into(), key: "F14".into(), action: "monitor".into(), monitor: 0, ..Default::default() },
        ];
        core.apply_settings(s);
        core.on_hotkey("swap-next");
        let after_next = core.settings().monitor;
        core.on_hotkey("swap-first");
        let after_first = core.settings().monitor;
        if monitors >= 2 {
            check("a keybind switches the capture monitor to the next one", after_next == 1);
            check("a keybind switches the capture monitor to a named one", after_first == 0);
        } else {
            check("with one monitor the switch keybind changes nothing", after_next == 0 && after_first == 0);
        }
        check("the new capture monitor is saved", crate::settings::Settings::load().monitor == after_first);
    }

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
    let fg = |exe: &str, title: &str, fullscreen: bool| Foreground { pid: 1, exe: exe.into(), title: title.into(), class: String::new(), fullscreen };
    let id = |exe: &str, title: &str, fs: bool| crate::games::identify(&fg(exe, title, fs));
    // Minecraft inside Java: found by a client's title, the window class, the runtime's folder, or a fullscreen Java window.
    {
        let java = r"C:\Program Files\Java\jre\bin\javaw.exe";
        let with_class = |class: &str| crate::games::identify(&Foreground { pid: 1, exe: java.into(), title: "Custom Client".into(), class: class.into(), fullscreen: false });
        let mc = Some("Minecraft");
        check("Minecraft clients are found by their window title", id(java, "Lunar Client (1.8.9-a1b2c3/master)", false).as_deref() == mc && id(java, "Badlion Minecraft Client v4.1 (1.8.9)", false).as_deref() == mc);
        check("Minecraft is found by its window class", with_class("GLFW30").as_deref() == mc && with_class("LWJGL").as_deref() == mc && with_class("SunAwtFrame").is_none());
        check("Minecraft is found by its launcher's Java folder", id(r"C:\Users\me\.lunarclient\jre\zulu17\bin\javaw.exe", "", false).as_deref() == mc && id(r"C:\Users\me\AppData\Roaming\.minecraft\runtime\java-runtime-gamma\bin\javaw.exe", "Untitled", false).as_deref() == mc);
        check("a fullscreen Java window is Minecraft, a windowed Java tool is not", id(java, "", true).as_deref() == mc && id(java, "Some Tool", false).is_none());
        check("a Minecraft launcher is not the game", id(r"C:\XboxGames\Minecraft Launcher\Content\Minecraft Launcher.exe", "Minecraft Launcher", true).is_none() && id(r"C:\Users\me\AppData\Local\Programs\lunarclient\Lunar Client.exe", "Lunar Client", true).is_none());
        check("a VPN filling the screen is not a game", id(r"C:\Program Files (x86)\ExpressVPN\expressvpn-ui\ExpressVPN.exe", "ExpressVPN", true).is_none());
        let c = crate::games::canonical;
        check("one name per game", c("Overwatch 2") == "Overwatch" && c("overwatch") == "overwatch" && c("Minecraft 1.8.9") == "Minecraft" && c("Lunar Client (1.8.9-abc/master)") == "Minecraft" && c("Java(TM) Platform SE binary") == "Minecraft" && c("Risk of Rain 2") == "Risk of Rain 2");
        check("names that are no game become Desktop", c("ExpressVPN") == "Desktop" && c("Screen Recording") == "Desktop" && c("Google Chrome") == "Desktop" && c("  ") == "Desktop");
        check("other Minecraft titles keep their names", c("Minecraft Dungeons") == "Minecraft Dungeons" && c("Minecraft Launcher") == "Desktop");
    }
    check("Steam game is named after its folder", id(r"D:\SteamLibrary\steamapps\common\Risk of Rain 2\Risk of Rain 2.exe", "", false).as_deref() == Some("Risk of Rain 2"));
    check("known exe gets its proper name", id(r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe", "", true).as_deref() == Some("Valorant"));
    check("known exe outside a game folder, windowed", id(r"C:\Program Files (x86)\Overwatch\_retail_\Overwatch.exe", "", false).as_deref() == Some("Overwatch"));
    check("Minecraft is told from Java by its title", id(r"C:\Program Files\Java\bin\javaw.exe", "Minecraft 1.21.4", false).as_deref() == Some("Minecraft") && id(r"C:\Program Files\Java\bin\javaw.exe", "Some Tool", false).is_none());
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

    // Which monitors are buffered, and for how long: (settings, monitor count) -> [(monitor, longest clip)].
    {
        use crate::core::capture_plan;
        use crate::settings::{Keybind, Settings};
        let bind = |monitor: i32, seconds: u32| Keybind { monitor, seconds, ..Default::default() };
        let with = |monitor: u32, binds: Vec<Keybind>| Settings { monitor, keybinds: binds, ..Default::default() };
        check("monitors: default settings buffer the capture monitor only", capture_plan(&Settings::default(), 2) == vec![(0, 600)]);
        check("monitors: a bind for monitor 2 adds its buffer", capture_plan(&with(0, vec![bind(-1, 30), bind(1, 60)]), 2) == vec![(0, 30), (1, 60)]);
        check("monitors: each monitor is buffered once", capture_plan(&with(0, vec![bind(1, 30), bind(1, 45), bind(-1, 20), bind(0, 10)]), 2) == vec![(0, 20), (1, 45)]);
        check("monitors: a bind for a monitor that is gone counts as the capture monitor", capture_plan(&with(1, vec![bind(5, 90), bind(-1, 30)]), 2) == vec![(1, 90)]);
        check("monitors: a gone capture monitor falls back to the first", capture_plan(&with(3, vec![bind(-1, 30), bind(1, 40)]), 2) == vec![(0, 30), (1, 40)]);
        check("monitors: -1 follows the capture monitor", capture_plan(&with(1, vec![bind(-1, 30), bind(0, 40)]), 2) == vec![(1, 30), (0, 40)]);
        check("monitors: a 10-minute bind on one monitor does not lengthen the other", capture_plan(&with(0, vec![bind(0, 600), bind(1, 30)]), 2) == vec![(0, 600), (1, 30)]);
        let mut other = with(0, vec![bind(1, 120), bind(1, 30)]);
        check("monitors: a capture monitor with no bind of its own keeps the shortest clip length", capture_plan(&other, 2) == vec![(0, 30), (1, 120)]);
        other.keybinds.push(Keybind { action: "toggle".into(), monitor: 0, seconds: 900, ..Default::default() });
        check("monitors: only clip binds count", capture_plan(&other, 2) == vec![(0, 30), (1, 120)]);
        check("monitors: lengths are clamped", capture_plan(&with(0, vec![bind(-1, 1), bind(1, 99_999)]), 2) == vec![(0, 5), (1, 3600)]);
        let old: Keybind = serde_json::from_str(r#"{"id":"kb-30s","key":"F8","action":"clip","seconds":30}"#).unwrap_or_default();
        let new: Keybind = serde_json::from_str(r#"{"id":"kb-m2","key":"F9","action":"clip","seconds":30,"monitor":1}"#).unwrap_or_default();
        check("a keybind saved without a monitor clips the capture monitor (-1)", old.monitor == -1 && old.id == "kb-30s" && Keybind::default().monitor == -1);
        check("a keybind's monitor is read back", new.monitor == 1 && serde_json::to_value(&new).ok().and_then(|v| v["monitor"].as_i64()) == Some(1));
        let mons = win::monitors();
        let cfgs = crate::core::capture_cfgs(&with(0, vec![bind(-1, 30), bind((mons.len() as i32) - 1, 45)]));
        check("each buffered monitor gets its own capture size and buffer length", cfgs.len() == mons.len().min(2) && cfgs.iter().all(|c| { let m = &mons[c.monitor as usize]; c.hz == m.hz && c.height == m.height }) && cfgs.last().is_some_and(|c| c.max_secs == 50.0));
    }

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

