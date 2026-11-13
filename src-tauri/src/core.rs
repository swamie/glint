//! App state shared by the UI, the tray, hotkeys and the benchmark.
use crate::library::{self, ClipMeta, Library};
use crate::recorder::{CaptureCfg, Recorder};
use crate::settings::{Keybind, Settings};
use crate::{clip, discord, games, hotkeys, util, win};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

#[derive(Serialize, Clone, Default, Debug)]
pub struct Status {
    pub recording: bool,
    pub encoder: String,
    pub method: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fps_actual: f64,
    pub buffer_secs: f64,
    pub buffer_max: f64,
    /// The part of the buffer that is in RAM, and the part that is in files.
    pub buffer_bytes: u64,
    pub buffer_disk_bytes: u64,
    /// Recorder CPU (Glint + ffmpeg) as a share of the whole CPU, like Task Manager.
    pub cpu_pct: f64,
    /// Glint's own memory without the replay buffer.
    pub ram_app: u64,
    pub ram_ffmpeg: u64,
    /// Everything the program uses, as Task Manager adds it up: this process's
    /// working set (replay buffer included) plus those of all its child
    /// processes (ffmpeg and the WebView2 processes that draw the window).
    pub ram_total: u64,
    pub max_mbps: f64,
    pub error: String,
    /// Game detected as running ("" = none).
    pub game: String,
    /// Discord Rich Presence state: "off", "connected", or the reason it is not.
    pub discord: String,
    /// One per buffered monitor, the capture monitor first. The fields above
    /// describe the capture monitor; `buffer_bytes`, `buffer_disk_bytes` and
    /// `ram_total` count all of them.
    pub captures: Vec<CaptureStatus>,
}

/// One monitor's replay buffer.
#[derive(Serialize, Clone, Default, Debug)]
pub struct CaptureStatus {
    /// Index into `win::monitors()`.
    pub monitor: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fps_actual: f64,
    pub buffer_secs: f64,
    pub buffer_max: f64,
    pub error: String,
    /// It is the capture monitor (`Settings::monitor`).
    pub primary: bool,
}

/// The game being played: found in the foreground, kept while its process lives.
#[derive(Clone)]
struct Playing {
    name: String,
    pid: u32,
    /// Unix seconds when it was first seen.
    since: u64,
}

struct Sample {
    t: f64,
    /// Per capture pipeline (`CapStats::id`): frames counted at `t`, and the rate since the sample before.
    frames: HashMap<u64, (u64, f64)>,
    cpu: f64,
    cpu_pct: f64,
}

pub struct Core {
    pub settings: Mutex<Settings>,
    pub rec: Recorder,
    pub lib: Mutex<Library>,
    pub app: OnceLock<tauri::AppHandle>,
    sample: Mutex<Sample>,
    /// When each keybind last fired, to drop the second copy of one press.
    last_hotkey: Mutex<HashMap<String, std::time::Instant>>,
    pub games: Mutex<games::Games>,
    playing: Mutex<Option<Playing>>,
    pub jobs: crate::jobs::Jobs,
    discord: Mutex<String>,
}

/// Width / height the clip should have.
///
/// A "stretched" resolution (4:3, 5:4, 3:2 or 16:10 shown full-screen on a
/// wider display) reaches the capture as a narrow picture, although the player
/// sees it filling the screen. Left alone, the clip plays back narrow with
/// black bars. "auto" widens such a picture to the display's own shape, and
/// only then: a mode as wide as the display or wider is kept as it is.
pub fn target_ratio(aspect: &str, mode: (u32, u32), native: (u32, u32)) -> f64 {
    let of = |w: u32, h: u32| w.max(1) as f64 / h.max(1) as f64;
    let (src, display) = (of(mode.0, mode.1), of(native.0, native.1));
    match aspect {
        "source" => src,
        "auto" => if src < display * 0.92 { display } else { src },
        other => other
            .split_once(':')
            .and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?)))
            .filter(|(a, b)| *a > 0.0 && *b > 0.0)
            .map(|(a, b)| a / b)
            .unwrap_or(src),
    }
}

/// Size of the clip in pixels: `height` (0 = the display mode's own) at the target shape.
pub fn output_size(mode: (u32, u32), native: (u32, u32), height: u32, aspect: &str) -> (u32, u32) {
    let even = |v: u32| (v.max(2) / 2) * 2;
    let h = if height == 0 || height >= mode.1 { mode.1 } else { height.max(240) };
    let ratio = target_ratio(aspect, mode, native);
    let src = mode.0.max(1) as f64 / mode.1.max(1) as f64;
    // Same shape as the source (within 1 %): keep the source's exact proportions.
    let w = if (ratio / src - 1.0).abs() < 0.01 { mode.0 as f64 * h as f64 / mode.1.max(1) as f64 } else { h as f64 * ratio };
    (even(w.round() as u32), even(h))
}

/// The capture monitor (`Settings::monitor`), or the first one if that is gone.
pub fn capture_monitor(s: &Settings, count: usize) -> u32 {
    if (s.monitor as usize) < count { s.monitor } else { 0 }
}

/// The monitor a keybind's clip is taken from: its own, or the capture monitor
/// for -1 and for a monitor that no longer exists.
pub fn bind_monitor(s: &Settings, k: &Keybind, count: usize) -> u32 {
    resolve_monitor(s, k.monitor, count)
}

/// A keybind-style monitor number (-1 = the capture monitor) as a real index.
pub fn resolve_monitor(s: &Settings, monitor: i32, count: usize) -> u32 {
    if monitor >= 0 && (monitor as usize) < count { monitor as u32 } else { capture_monitor(s, count) }
}

/// Which monitors to buffer, and how long a clip each must be able to give:
/// the capture monitor first, then every other monitor a clip keybind names,
/// each once. A monitor's length is that of the longest clip keybind that can
/// ask for it; the capture monitor with none of its own keeps the length the
/// tray and the top bar ask for.
pub fn capture_plan(s: &Settings, count: usize) -> Vec<(u32, u32)> {
    let count = count.max(1);
    let primary = capture_monitor(s, count);
    let mut plan: Vec<(u32, Option<u32>)> = vec![(primary, None)];
    for k in s.keybinds.iter().filter(|k| k.action == "clip") {
        let m = bind_monitor(s, k, count);
        let secs = k.seconds.clamp(5, 3600);
        match plan.iter_mut().find(|(pm, _)| *pm == m) {
            Some((_, len)) => *len = Some(len.map_or(secs, |l| l.max(secs))),
            None => plan.push((m, Some(secs))),
        }
    }
    plan.into_iter().map(|(m, len)| (m, len.unwrap_or_else(|| s.default_clip_secs().clamp(5, 3600)))).collect()
}

/// Capture parameters of every monitor to buffer, the capture monitor first.
pub fn capture_cfgs(s: &Settings) -> Vec<CaptureCfg> {
    let mons = win::monitors();
    capture_plan(s, mons.len()).into_iter().filter_map(|(m, secs)| mons.get(m as usize).map(|mon| capture_cfg_for(s, mon, secs))).collect()
}

/// The capture monitor's parameters.
pub fn capture_cfg(s: &Settings) -> CaptureCfg {
    let mons = win::monitors();
    let plan = capture_plan(s, mons.len());
    capture_cfg_for(s, &mons[plan[0].0 as usize], plan[0].1)
}

/// Resolve user settings against one real monitor into concrete capture
/// parameters, with a buffer for clips of up to `clip_secs`.
pub fn capture_cfg_for(s: &Settings, m: &win::Monitor, clip_secs: u32) -> CaptureCfg {
    let (width, height) = output_size((m.width, m.height), (m.native_width, m.native_height), s.height, &s.aspect);
    CaptureCfg {
        ffmpeg: util::find_exe(&s.ffmpeg_path, "ffmpeg"),
        monitor: m.index,
        width,
        height,
        scaled: (width, height) != (m.width, m.height),
        fps: s.fps.clamp(5, m.hz.max(60)),
        hz: m.hz,
        quality: s.quality.clone(),
        bitrate: s.bitrate.min(150),
        encoder: s.encoder.clone(),
        method: s.method.clone(),
        cursor: s.cursor,
        desktop_audio: s.desktop_audio,
        mic: s.mic,
        max_secs: clip_secs as f64 + 5.0,
        buffer: s.buffer.clone(),
        buffer_ram_secs: s.buffer_ram_secs.clamp(5, 600) as f64,
        buffer_dir: crate::spill::dir(&s.buffer_dir),
    }
}

fn set_autostart(on: bool) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") else {
        return;
    };
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let _ = key.set_value("Glint", &format!("\"{}\" --minimized", exe.display()));
        }
    } else {
        let _ = key.delete_value("Glint");
    }
}

impl Core {
    pub fn new() -> Arc<Self> {
        let settings = Settings::load();
        hotkeys::set_binds(&settings.keybinds);
        // Clips tagged by an older version or an import: one name per game, and
        // "games" that are not games (a VPN, "Screen Recording") go to Desktop.
        let mut lib = Library::load();
        let retagged = lib.retag_games();
        let mut games = games::Games::load();
        let used: Vec<String> = lib.clips.iter().map(|c| c.game.clone()).collect();
        games.tidy(&used);
        for name in &used {
            games.note(name, ""); // also finds Minecraft an icon if it has none
        }
        if retagged > 0 {
            util::log(&format!("games: {retagged} clips filed under their game's one name"));
        }
        Arc::new(Core {
            settings: Mutex::new(settings),
            rec: Recorder::new(),
            lib: Mutex::new(lib),
            app: OnceLock::new(),
            sample: Mutex::new(Sample { t: util::now_secs(), frames: HashMap::new(), cpu: 0.0, cpu_pct: 0.0 }),
            last_hotkey: Mutex::new(HashMap::new()),
            games: Mutex::new(games),
            playing: Mutex::new(None),
            jobs: crate::jobs::Jobs::new(),
            discord: Mutex::new("off".into()),
        })
    }

    /// Look at the foreground window and update which game is being played.
    /// A game stays "playing" after it loses focus, until its process ends.
    pub fn track_game(&self) {
        let found = games::detect();
        let before = self.playing.lock().unwrap().as_ref().map(|p| p.name.clone());
        match found {
            Some((name, pid, exe)) => {
                {
                    let mut cur = self.playing.lock().unwrap();
                    if !cur.as_ref().is_some_and(|p| p.name == name && p.pid == pid) {
                        *cur = Some(Playing { name: name.clone(), pid, since: util::unix_ms() / 1000 });
                    }
                }
                if self.games.lock().unwrap().note(&name, &exe) {
                    self.emit("games-changed", ());
                }
            }
            None => {
                let mut cur = self.playing.lock().unwrap();
                if cur.as_ref().is_some_and(|p| !win::process_alive(p.pid)) {
                    *cur = None;
                }
            }
        }
        let after = self.playing.lock().unwrap().as_ref().map(|p| p.name.clone());
        if before != after {
            util::log(&format!("game: {}", after.as_deref().unwrap_or("none")));
            self.emit("status-changed", ());
        }
    }

    /// Name to file a clip under, as of this moment.
    fn game_now(&self) -> String {
        self.track_game();
        self.playing.lock().unwrap().as_ref().map(|p| p.name.clone()).unwrap_or_else(|| games::DESKTOP.to_string())
    }

    /// Background upkeep, every two seconds: game detection, display-mode
    /// changes (a game switching resolution), and Discord Rich Presence.
    pub fn watch(self: &Arc<Self>) {
        let core = self.clone();
        thread::spawn(move || {
            let modes = || win::monitors().iter().map(|m| (m.width, m.height, m.hz)).collect::<Vec<_>>();
            let mut seen = modes();
            let mut rpc: Option<(String, discord::Client)> = None;
            let mut shown: Option<discord::Activity> = None;
            let mut retry_at = std::time::Instant::now();
            loop {
                thread::sleep(std::time::Duration::from_secs(2));
                core.track_game();

                let now = modes();
                if now != seen {
                    seen = now;
                    if core.rec.is_running() {
                        let cfgs = capture_cfgs(&core.settings());
                        if core.rec.current_cfgs() != cfgs {
                            let what: Vec<String> = cfgs.iter().map(|c| format!("monitor {} {}x{} @ {} fps", c.monitor, c.width, c.height, c.fps)).collect();
                            util::log(&format!("display mode changed: capturing {}", what.join(", ")));
                            core.rec.start(cfgs);
                            core.emit("status-changed", ());
                        }
                    }
                }

                let s = core.settings();
                let state = if !s.discord_rpc {
                    if let Some((_, mut c)) = rpc.take() {
                        let _ = c.set(None);
                    }
                    shown = None;
                    "off".to_string()
                } else {
                    if rpc.as_ref().is_some_and(|(id, _)| *id != s.discord_app_id.trim()) {
                        rpc = None;
                        shown = None;
                        retry_at = std::time::Instant::now();
                    }
                    let mut state = core.discord.lock().unwrap().clone();
                    if rpc.is_none() && std::time::Instant::now() >= retry_at {
                        match discord::Client::connect(&s.discord_app_id) {
                            Ok(c) => {
                                rpc = Some((s.discord_app_id.trim().to_string(), c));
                                state = "connected".into();
                            }
                            Err(e) => {
                                state = e;
                                retry_at = std::time::Instant::now() + std::time::Duration::from_secs(15);
                            }
                        }
                    }
                    let want = core.rec.is_running().then(|| {
                        let playing = core.playing.lock().unwrap().clone();
                        discord::Activity {
                            details: playing.as_ref().map(|p| format!("Clipping {}", p.name)).unwrap_or_else(|| "Clipping the desktop".into()),
                            state: format!("Last {} ready to save", fmt_secs(s.max_clip_secs() as f64)),
                            since: playing.map(|p| p.since),
                        }
                    });
                    if let Some((_, c)) = rpc.as_mut() {
                        if want != shown {
                            match c.set(want.as_ref()) {
                                Ok(()) => shown = want,
                                Err(e) => {
                                    state = e;
                                    rpc = None;
                                    shown = None;
                                    retry_at = std::time::Instant::now() + std::time::Duration::from_secs(15);
                                }
                            }
                        }
                    }
                    state
                };
                let mut cur = core.discord.lock().unwrap();
                if *cur != state {
                    if state != "off" && state != "connected" {
                        util::log(&format!("discord: {state}"));
                    }
                    *cur = state;
                }
            }
        });
    }

    /// With "Start with Windows" on, make the start-up entry point at this copy of
    /// Glint. It goes stale when the program is installed or moved somewhere else.
    pub fn sync_autostart(&self) {
        if self.settings.lock().unwrap().autostart {
            set_autostart(true);
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn start_recording(&self) {
        self.rec.start(capture_cfgs(&self.settings()));
    }

    pub fn toggle_recording(&self) {
        if self.rec.is_running() {
            self.rec.stop();
        } else {
            self.start_recording();
        }
    }

    pub fn status(&self) -> Status {
        let st = self.rec.stats();
        let (cpu_self, ram_self) = win::proc_stats(0).unwrap_or((0.0, 0));
        // Every buffered monitor has its own ffmpeg.
        let (cpu_ff, ram_ff) = st
            .captures
            .iter()
            .filter(|c| c.status.pid != 0)
            .filter_map(|c| win::proc_stats(c.status.pid))
            .fold((0.0, 0u64), |(c, r), (c2, r2)| (c + c2, r + r2));
        let now = util::now_secs();
        let cpu = cpu_self + cpu_ff;
        let mut s = self.sample.lock().unwrap();
        let dt = now - s.t;
        if dt >= 0.5 {
            let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
            // A pipeline seen for the first time has no rate until the next sample.
            let frames = st.captures.iter().map(|c| (c.id, (c.frames, s.frames.get(&c.id).map_or(0.0, |(f, _)| c.frames.saturating_sub(*f) as f64 / dt)))).collect();
            s.frames = frames;
            s.cpu_pct = ((cpu - s.cpu).max(0.0) / dt / cores * 100.0).min(100.0);
            s.t = now;
            s.cpu = cpu;
        }
        let fps_of = |id: u64| if st.running { s.frames.get(&id).map_or(0.0, |(_, fps)| *fps) } else { 0.0 };
        let mons = win::monitors();
        let captures: Vec<CaptureStatus> = st
            .captures
            .iter()
            .map(|c| CaptureStatus {
                monitor: c.monitor,
                name: mons.get(c.monitor as usize).map(|m| m.name.clone()).unwrap_or_default(),
                width: c.status.width,
                height: c.status.height,
                fps: c.status.fps,
                fps_actual: fps_of(c.id),
                buffer_secs: c.buffer_secs,
                buffer_max: (c.max_secs - 5.0).max(0.0),
                error: c.status.error.clone(),
                primary: c.primary,
            })
            .collect();
        let buffer_bytes = st.video_bytes + st.audio_bytes;
        Status {
            recording: st.running,
            encoder: st.status.encoder,
            method: st.status.method,
            width: st.status.width,
            height: st.status.height,
            fps: st.status.fps,
            fps_actual: st.captures.first().map_or(0.0, |c| fps_of(c.id)),
            buffer_secs: st.buffer_secs,
            buffer_max: (st.max_secs - 5.0).max(0.0),
            buffer_bytes,
            buffer_disk_bytes: st.video_disk_bytes + st.audio_disk_bytes,
            cpu_pct: s.cpu_pct,
            ram_app: ram_self.saturating_sub(buffer_bytes),
            ram_ffmpeg: ram_ff,
            ram_total: win::tree_working_set(),
            max_mbps: st.status.maxrate as f64 / 1e6,
            error: st.status.error,
            game: self.playing.lock().unwrap().as_ref().map(|p| p.name.clone()).unwrap_or_default(),
            discord: self.discord.lock().unwrap().clone(),
            captures,
        }
    }

    pub fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) {
        use tauri::Emitter;
        if let Some(app) = self.app.get() {
            let _ = app.emit(event, payload);
        }
    }

    /// Show the overlay if it is switched on. `title` and `sub` are English; both are translated here
    /// (a `sub` that is not a fixed text, such as an error or an already built sentence, passes through unchanged).
    fn toast(&self, title: &str, sub: &str) {
        let monitor = self.settings.lock().unwrap().monitor;
        self.toast_on(title, sub, monitor);
    }

    /// `toast` on a given monitor (index into `win::monitors()`).
    fn toast_on(&self, title: &str, sub: &str, monitor: u32) {
        let (on, lang) = {
            let s = self.settings.lock().unwrap();
            (s.toast, s.language.clone())
        };
        if on {
            self.show_toast_on(crate::lang::tr(&lang, title), crate::lang::tr(&lang, sub), monitor);
        }
    }

    /// Show the clip overlay on the capture monitor with the current position and microphone state.
    pub fn show_toast(&self, title: &str, sub: &str) {
        let monitor = self.settings.lock().unwrap().monitor;
        self.show_toast_on(title, sub, monitor);
    }

    /// `show_toast` on a given monitor: the one a clip was taken from.
    pub fn show_toast_on(&self, title: &str, sub: &str, monitor: u32) {
        let Some(app) = self.app.get() else { return };
        let s = self.settings();
        // Compact is for the plain "Clip saved" only: a failure, a pause or an
        // empty buffer still gets the full box, because two stars cannot say that.
        let compact = s.toast_compact && title == crate::lang::tr(&s.language, "Clip saved");
        crate::app::show_toast(
            app,
            crate::app::Toast {
                title: title.into(),
                sub: sub.into(),
                compact,
                mic: s.toast_mic && s.mic && self.rec.is_running(),
                pos: s.toast_pos,
                monitor,
                opacity: s.toast_opacity.clamp(20, 100),
                scale: s.toast_scale.clamp(70, 160),
                secs: s.toast_secs.clamp(1.0, 15.0),
                theme: s.theme,
                css: s.custom_css,
            },
        );
    }

    /// Save the last `seconds` seconds of `monitor` (-1 = the capture monitor).
    /// Returns immediately; the mux runs on its own thread.
    pub fn clip(self: &Arc<Self>, seconds: u32, monitor: i32) {
        let core = self.clone();
        thread::spawn(move || {
            let _ = core.clip_blocking(seconds, monitor);
        });
    }

    /// Save the last `seconds` seconds of `monitor`: an index into
    /// `win::monitors()`, or -1 (or a monitor that no longer exists) for the capture monitor.
    pub fn clip_blocking(&self, seconds: u32, monitor: i32) -> Result<ClipMeta, String> {
        let s = self.settings();
        let count = win::monitors().len();
        let mon = resolve_monitor(&s, monitor, count);
        let primary = mon == capture_monitor(&s, count);
        let game = self.game_now();
        if !self.rec.is_running() {
            self.toast_on("Clipping is paused", "Resume clipping to save clips", mon);
            return Err("Clipping is paused".into());
        }
        let (snap, desk, mic, cfg) = match self.rec.snapshot(seconds as f64, if primary { None } else { Some(mon) }) {
            Ok(v) => v,
            Err(e) => {
                util::log(&format!("clip {seconds}s of monitor {mon}: nothing to save: {e}"));
                self.toast_on("Nothing to clip yet", &e, mon);
                return Err(e);
            }
        };
        if s.sound {
            crate::sound::play(&s.sound_name, s.sound_volume, &s.sound_file);
        }
        // The overlay goes where the picture came from.
        let mon = cfg.monitor;
        let cap = clip::Capture { snap, desk, mic, width: cfg.width, height: cfg.height, fps: cfg.fps };
        let started = std::time::Instant::now();
        match clip::write(&s, cap, &game) {
            Ok(meta) => {
                util::log(&format!("clip {seconds}s of monitor {mon}: saved {:.1}s, {:.1} MB in {} ms -> {}", meta.duration, meta.size as f64 / 1e6, started.elapsed().as_millis(), meta.path));
                self.lib.lock().unwrap().add(meta.clone());
                self.emit("clips-changed", meta.id.clone());
                // "last 30s" when the clip is as long as asked; its real length when the buffer held less.
                let len = if (meta.duration - seconds as f64).abs() < 2.0 { seconds as f64 } else { meta.duration };
                self.toast_on("Clip saved", &captured_text(&s, len, &meta.game), mon);
                Ok(meta)
            }
            Err(e) => {
                util::log(&format!("clip {seconds}s of monitor {mon}: failed: {e}"));
                self.toast_on("Clip failed", &e, mon);
                Err(e)
            }
        }
    }

    pub fn on_hotkey(self: &Arc<Self>, id: &str) {
        let bind = self.settings.lock().unwrap().keybinds.iter().find(|k| k.id == id).cloned();
        let Some(b) = bind else { return };
        {
            // The same press can arrive twice: from the global hook and from the
            // focused window (the hook is not fed while Glint itself has focus).
            // Kept per keybind, so two binds pressed close together both fire.
            let mut last = self.last_hotkey.lock().unwrap();
            let now = std::time::Instant::now();
            if last.get(id).is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_millis(600)) {
                return;
            }
            last.insert(id.to_string(), now);
        }
        util::log(&format!("hotkey {} -> {} {}s (monitor {})", id, b.action, b.seconds, b.monitor));
        match b.action.as_str() {
            "clip" => self.clip(b.seconds, b.monitor),
            "toggle" => {
                self.toggle_recording();
                let on = self.rec.is_running();
                self.toast(if on { "Clipping resumed" } else { "Clipping paused" }, if on { "Replay buffer is filling" } else { "Replay buffer cleared" });
                self.emit("status-changed", ());
            }
            "show" => {
                if let Some(app) = self.app.get() {
                    crate::app::show_main(app);
                }
            }
            _ => {}
        }
    }

    pub fn apply_settings(&self, mut new: Settings) {
        for k in &mut new.keybinds {
            k.seconds = k.seconds.clamp(5, 3600);
            if k.id.is_empty() {
                k.id = util::new_id();
            }
        }
        let old = {
            let mut cur = self.settings.lock().unwrap();
            std::mem::replace(&mut *cur, new.clone())
        };
        new.save();
        hotkeys::set_binds(&new.keybinds);
        if new.autostart != old.autostart {
            set_autostart(new.autostart);
        }
        if self.rec.is_running() {
            // Only the monitors whose capture changed are restarted (see `Recorder::start`).
            let cfgs = capture_cfgs(&new);
            if self.rec.current_cfgs() != cfgs {
                self.rec.start(cfgs);
            }
        }
    }

    /// Queue a Blur render of a clip. It runs in the background (see jobs.rs).
    pub fn blur_clip(self: &Arc<Self>, id: String) -> Result<(), String> {
        crate::jobs::enqueue_blur(self, id).map(|_| ())
    }

    /// Add videos that sit in the clips folder (or one of its subfolders) but
    /// are not in the index yet, drop entries whose file is gone, and bring
    /// every clip's `folder` in line with where its file is.
    pub fn rescan(&self) -> usize {
        let s = self.settings();
        let Some(ffmpeg) = util::find_exe(&s.ffmpeg_path, "ffmpeg") else { return 0 };
        let root = PathBuf::from(&s.clips_dir);
        let known: Vec<String> = self.lib.lock().unwrap().clips.iter().map(|c| c.path.to_lowercase()).collect();
        let mut added = 0;
        let mut dirs = vec![(root.clone(), String::new())];
        dirs.extend(self.folders().into_iter().map(|f| (root.join(&f), f)));
        for (dir, folder) in dirs {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let p = entry.path();
                let is_video = p.extension().map(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mkv") || e.eq_ignore_ascii_case("mov")).unwrap_or(false);
                if !is_video || !p.is_file() || known.contains(&p.to_string_lossy().to_lowercase()) {
                    continue;
                }
                let title = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut meta = library::register(&ffmpeg, &p, &title, "Imported", "import");
                meta.folder = folder.clone();
                if let Ok(t) = entry.metadata().and_then(|m| m.modified()) {
                    meta.created = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(meta.created);
                }
                self.lib.lock().unwrap().add(meta);
                added += 1;
            }
        }
        let mut lib = self.lib.lock().unwrap();
        let before = lib.clips.len();
        lib.clips.retain(|c| std::path::Path::new(&c.path).is_file());
        let mut changed = lib.clips.len() != before;
        for c in lib.clips.iter_mut() {
            let f = folder_of(&root, &c.path);
            if c.folder != f {
                c.folder = f;
                changed = true;
            }
        }
        if changed {
            lib.save();
        }
        added
    }

    /// Library folders: the subfolders of the clips folder, sorted by name.
    pub fn folders(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.settings().clips_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        out.sort_by_key(|n| n.to_lowercase());
        out
    }

    pub fn create_folder(&self, name: &str) -> Result<String, String> {
        let name = folder_name(name)?;
        let dir = PathBuf::from(self.settings().clips_dir).join(&name);
        if dir.exists() {
            return Err(format!("A folder named \"{name}\" already exists"));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create the folder: {e}"))?;
        Ok(name)
    }

    pub fn rename_folder(&self, from: &str, to: &str) -> Result<String, String> {
        let to = folder_name(to)?;
        let root = PathBuf::from(self.settings().clips_dir);
        let (old, new) = (root.join(from), root.join(&to));
        if from.is_empty() || !old.is_dir() {
            return Err("That folder no longer exists".into());
        }
        // A change of letter case only is the same directory on Windows: rename it anyway.
        if new.exists() && !from.eq_ignore_ascii_case(&to) {
            return Err(format!("A folder named \"{to}\" already exists"));
        }
        std::fs::rename(&old, &new).map_err(|e| format!("Could not rename the folder: {e}"))?;
        let mut lib = self.lib.lock().unwrap();
        for c in lib.clips.iter_mut().filter(|c| c.folder.eq_ignore_ascii_case(from)) {
            if let Some(file) = Path::new(&c.path).file_name() {
                c.path = new.join(file).to_string_lossy().into_owned();
            }
            c.folder = to.clone();
        }
        lib.save();
        Ok(to)
    }

    /// Remove a library folder. Its clips go back to the clips folder first; a
    /// folder that still holds other files afterwards is left in place.
    pub fn delete_folder(&self, name: &str) -> Result<(), String> {
        let dir = PathBuf::from(self.settings().clips_dir).join(name);
        if name.is_empty() || !dir.is_dir() {
            return Err("That folder no longer exists".into());
        }
        let ids: Vec<String> = self.lib.lock().unwrap().clips.iter().filter(|c| c.folder.eq_ignore_ascii_case(name)).map(|c| c.id.clone()).collect();
        self.move_clips(&ids, "")?;
        std::fs::remove_dir(&dir).map_err(|_| "The clips were moved out, but the folder still holds other files, so it was kept".to_string())
    }

    /// Move every clip that sits loose in the clips folder into the folder named
    /// after its game. Clips already in a folder stay where the user put them.
    pub fn sort_by_game(&self) -> Result<usize, String> {
        let loose: Vec<(String, String)> = self.lib.lock().unwrap().clips.iter().filter(|c| c.folder.is_empty()).map(|c| (c.id.clone(), util::game_folder(&c.game))).collect();
        let mut moved = 0;
        let mut folders: Vec<&String> = loose.iter().map(|(_, f)| f).collect();
        folders.sort();
        folders.dedup();
        for folder in folders {
            let ids: Vec<String> = loose.iter().filter(|(_, f)| f == folder).map(|(id, _)| id.clone()).collect();
            moved += self.move_clips(&ids, folder)?;
        }
        Ok(moved)
    }

    /// Give clips a new game name. With game folders on, a clip that sits in
    /// its old game's folder follows into the new game's folder.
    pub fn set_game(&self, ids: &[String], game: &str) -> Result<(), String> {
        let game: String = game.trim().chars().take(60).collect();
        if game.is_empty() {
            return Err("Type a game name".into());
        }
        let follow: Vec<String> = {
            let mut lib = self.lib.lock().unwrap();
            let mut follow = Vec::new();
            for c in lib.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
                if c.folder.eq_ignore_ascii_case(&util::game_folder(&c.game)) {
                    follow.push(c.id.clone());
                }
                c.game = game.clone();
            }
            lib.save();
            follow
        };
        if self.settings().game_folders && !follow.is_empty() {
            self.move_clips(&follow, &util::game_folder(&game))?;
        }
        Ok(())
    }

    /// Give a clip a new title and its file the same name: `<title>.<ext>`, in
    /// the folder it is in, made safe for the file system (" (2)" is added if
    /// another file has that name). A change of letter case only is renamed too.
    /// If the file cannot be renamed, the clip is left exactly as it was.
    pub fn rename_clip(&self, id: &str, title: &str) -> Result<ClipMeta, String> {
        let title: String = title.trim().chars().take(200).collect();
        if title.is_empty() {
            return Err("Type a name for the clip".into());
        }
        // `sanitize_filename` turns a name of nothing usable into "Clip"; refuse it instead.
        if title.chars().all(|c| "<>:\"/\\|?*.".contains(c) || c.is_whitespace() || c.is_control()) {
            return Err("That name has nothing a file name can use. Use letters or digits".into());
        }
        let meta = self.lib.lock().unwrap().get(id).ok_or("Clip not found")?;
        // A queued or running render holds the old path; renaming under it would break it.
        if self.jobs.list().iter().any(|j| j.clip_id == id && (j.state == "queued" || j.state == "running")) {
            return Err("This clip is being rendered. Rename it when the render has finished".into());
        }
        let src = PathBuf::from(&meta.path);
        if !src.is_file() {
            return Err("The clip's file is missing, so it cannot be renamed".into());
        }
        let dir = src.parent().map(Path::to_path_buf).unwrap_or_default();
        let ext = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
        let base = util::sanitize_filename(&title);
        // The first free name; the clip's own file counts as free (a case-only change).
        let same_file = |p: &Path| p.to_string_lossy().to_lowercase() == src.to_string_lossy().to_lowercase();
        let mut dest = dir.join(format!("{base}.{ext}"));
        let mut n = 2;
        while dest.exists() && !same_file(&dest) {
            dest = dir.join(format!("{base} ({n}).{ext}"));
            n += 1;
        }
        if dest.as_os_str() != src.as_os_str() {
            std::fs::rename(&src, &dest).map_err(|e| match e.raw_os_error() {
                Some(32) | Some(33) => "The clip is open in another program, so its file could not be renamed. Close the program (or the clip in the player) and try again".to_string(),
                Some(5) => "Windows did not allow the clip's file to be renamed (access denied)".to_string(),
                Some(2) | Some(3) => "The clip's file is missing, so it cannot be renamed".to_string(),
                _ => format!("The clip's file could not be renamed: {e}"),
            })?;
            util::log(&format!("rename clip {id}: {} -> {}", src.display(), dest.display()));
        }
        let path = dest.to_string_lossy().into_owned();
        self.lib
            .lock()
            .unwrap()
            .update(id, |c| {
                c.title = title;
                c.path = path;
            })
            .ok_or_else(|| "Clip not found".into())
    }

    /// Move clip files into a library folder ("" = the clips folder itself).
    /// Returns how many files moved; clips already there are skipped.
    pub fn move_clips(&self, ids: &[String], folder: &str) -> Result<usize, String> {
        let root = PathBuf::from(self.settings().clips_dir);
        let folder = if folder.is_empty() { String::new() } else { folder_name(folder)? };
        let dest = if folder.is_empty() { root.clone() } else { root.join(&folder) };
        std::fs::create_dir_all(&dest).map_err(|e| format!("Could not create the folder: {e}"))?;
        let mut moved = 0;
        let mut failed: Vec<String> = Vec::new();
        for id in ids {
            let Some(meta) = self.lib.lock().unwrap().get(id) else { continue };
            let src = PathBuf::from(&meta.path);
            if src.parent().map(|p| same_dir(p, &dest)).unwrap_or(false) {
                continue;
            }
            let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Clip".into());
            let ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
            let to = util::unique_path(&dest, &stem, &ext);
            // Rename is instant on one drive; a clip that lives on another drive is copied, then removed.
            let res = std::fs::rename(&src, &to).or_else(|_| std::fs::copy(&src, &to).and_then(|_| std::fs::remove_file(&src)));
            match res {
                Ok(()) => {
                    self.lib.lock().unwrap().update(id, |c| {
                        c.path = to.to_string_lossy().into_owned();
                        c.folder = folder.clone();
                    });
                    moved += 1;
                }
                Err(e) => failed.push(format!("{} ({e})", meta.title)),
            }
        }
        if failed.is_empty() {
            Ok(moved)
        } else {
            Err(format!("{} of {} clips could not be moved: {}", failed.len(), ids.len(), failed.join("; ")))
        }
    }
}

/// A folder name the user typed, made safe for the file system.
fn folder_name(name: &str) -> Result<String, String> {
    if name.trim().trim_matches('.').trim().is_empty() {
        return Err("Type a folder name".into());
    }
    Ok(util::sanitize_filename(name))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// The library folder a file is in: the name of its directory if that sits
/// directly inside the clips folder, otherwise "".
fn folder_of(clips_dir: &Path, path: &str) -> String {
    let Some(parent) = Path::new(path).parent() else { return String::new() };
    match (parent.parent(), parent.file_name()) {
        (Some(grand), Some(name)) if same_dir(grand, clips_dir) => name.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

/// The overlay's second line, such as "Captured last 30s of Overwatch". The
/// length and the game are each shown only if their setting is on.
pub fn captured_text(s: &Settings, secs: f64, game: &str) -> String {
    let tr = |text: &'static str| crate::lang::tr(&s.language, text);
    let what = if game == games::DESKTOP || game.is_empty() { tr("the desktop").to_string() } else { game.to_string() };
    let pattern = match (s.toast_length, s.toast_game) {
        (true, true) => tr("Captured last {len} of {game}"),
        (true, false) => tr("Captured last {len}"),
        (false, true) => tr("Captured {game}"),
        (false, false) => return String::new(),
    };
    pattern.replace("{len}", &fmt_secs_in(&s.language, secs)).replace("{game}", &what)
}

/// `fmt_secs` in the app's language: Korean writes "30초" and "10분 00초".
pub fn fmt_secs_in(lang: &str, secs: f64) -> String {
    let s = secs.round() as u64;
    match lang {
        "ko" if s >= 60 => format!("{}분 {:02}초", s / 60, s % 60),
        "ko" => format!("{s}초"),
        "el" if s >= 60 => format!("{} λεπ. {:02} δευτ.", s / 60, s % 60),
        "el" => format!("{s} δευτ."),
        _ => fmt_secs(secs),
    }
}

pub fn fmt_secs(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 60 { format!("{}m {:02}s", s / 60, s % 60) } else { format!("{s}s") }
}
