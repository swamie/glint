//! App state shared by the UI, the tray, hotkeys and the benchmark.
use crate::library::{self, ClipMeta, Library};
use crate::recorder::{CaptureCfg, Recorder};
use crate::settings::{Keybind, Settings};
use crate::{clip, games, hotkeys, util, win};
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
    /// The separate game audio track: "off", "waiting" (no game), "capturing",
    /// "failed" (this game could not be captured), "unavailable" (Windows too old).
    pub game_audio: String,
    /// The capture monitor's yellow Graphics Capture border (`RecStatus::border`): "", "off" or "failed".
    pub border: String,
    /// One per buffered monitor, the capture monitor first. The fields above
    /// describe the capture monitor; `buffer_bytes`, `buffer_disk_bytes` and
    /// `ram_total` count all of them.
    pub captures: Vec<CaptureStatus>,
    /// The GAME / DESKTOP switch (capmode.rs): "desktop" or "game".
    pub capture_mode: String,
    /// In game mode while clipping: "waiting" (no game), "window" (the game's window),
    /// "screen" (its monitor: window capture is not available) or "screen_failed" (its
    /// window could not be captured). "" otherwise.
    pub capture_target: String,
    /// 0.1.41: what game mode decided and why (gamedetect::Why); code "" outside it.
    pub capture_why: crate::gamedetect::Why,
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
    /// The clip the "edit" keybind wants open in the editor, until the page takes it (	ake_pending_edit).
    pending_edit: Mutex<Option<String>>,
    pub games: Mutex<games::Games>,
    playing: Mutex<Option<Playing>>,
    pub jobs: crate::jobs::Jobs,
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

/// What a clip keybind with monitor number `monitor` saves, for the log: "the capture monitor,
/// Monitor 1 (DISPLAY2, 2560x1440)" or "Monitor 2 (DISPLAY1, 1920x1080), which this keybind names;
/// the capture monitor is Monitor 1 (DISPLAY2, 2560x1440)".
pub fn clip_target_text(s: &Settings, mons: &[win::Monitor], monitor: i32) -> String {
    let cap = capture_monitor(s, mons.len());
    let to = resolve_monitor(s, monitor, mons.len());
    let named = monitor >= 0 && (monitor as usize) < mons.len();
    if !named {
        let why = if monitor >= 0 { " (this keybind's own monitor is not connected)" } else { "" };
        format!("the capture monitor, {}{why}", win::monitor_text(mons, cap))
    } else if to == cap {
        format!("{}, which this keybind names; it is also the capture monitor", win::monitor_text(mons, to))
    } else {
        format!("{}, which this keybind names; the capture monitor is {}", win::monitor_text(mons, to), win::monitor_text(mons, cap))
    }
}

/// One log line per clip keybind: which monitor it saves (`clip_target_text`).
pub fn keybind_targets(s: &Settings, mons: &[win::Monitor]) -> Vec<String> {
    s.keybinds.iter().filter(|k| k.action == "clip" && !k.key.is_empty()).map(|k| format!("keybind {} ({}) clips {}", k.id, k.key, clip_target_text(s, mons, k.monitor))).collect()
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
    // Game mode (capmode.rs): the game's window, or nothing while no game runs.
    if crate::capmode::game_mode(s) {
        return crate::capmode::cfgs(s);
    }
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
        window: 0,
        game: false,
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
        util::set_export_priority(&settings.export_priority);
        // Temporary export files a killed or crashed run left behind (0.1.41).
        std::thread::spawn(crate::projexport::sweep_tmp);
        hotkeys::set_binds(&settings.keybinds);
        crate::audio::set_devices(&settings.desktop_device, &settings.mic_device);
        crate::audio::set_gains(settings.desktop_gain_db, settings.mic_gain_db, settings.game_gain_db);
        if settings.sound_name == "custom" {
            let s = settings.clone();
            std::thread::spawn(move || { crate::sound::warm(&s, 0); });
        }
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
            pending_edit: Mutex::new(None),
            games: Mutex::new(games),
            playing: Mutex::new(None),
            jobs: crate::jobs::Jobs::new(),
        })
    }

    /// A game that is running now, if any: the one being played (it stays "playing" until its
    /// process ends), else one found among the running programs (started before Glint, or never
    /// in front). An automatic update never restarts Glint while this is Some.
    pub fn game_running(&self) -> Option<String> {
        if let Some(p) = self.playing.lock().unwrap().as_ref() {
            if win::process_alive(p.pid) {
                return Some(p.name.clone());
            }
        }
        let seen = self.games.lock().unwrap().list.clone();
        games::game_among(&win::process_paths(), &seen)
    }

    /// The process of the game being played now, if any (the GAME / DESKTOP switch's menu, capmode.rs).
    pub fn playing_pid(&self) -> Option<u32> {
        self.playing.lock().unwrap().as_ref().map(|p| p.pid)
    }

    /// Look at the foreground window and update which game is being played.
    /// A game stays "playing" after it loses focus, until its process ends.
    pub fn track_game(&self) {
        // Desktop mode: the foreground window, as always (with the user's marks, if any). Game
        // mode: every visible window is scored and the one to record is picked (capmode.rs).
        let found = crate::capmode::found(self);
        let before = self.playing.lock().unwrap().as_ref().map(|p| p.name.clone());
        match found {
            Some((name, pid, exe)) => {
                {
                    let mut cur = self.playing.lock().unwrap();
                    if !cur.as_ref().is_some_and(|p| p.name == name && p.pid == pid) {
                        *cur = Some(Playing { name: name.clone(), pid });
                    }
                }
                if self.games.lock().unwrap().note(&name, &exe) {
                    self.emit("games-changed", ());
                }
            }
            None => {
                // Game mode follows what it records; a program marked "not a game" stops counting at once.
                let pid = self.playing.lock().unwrap().as_ref().map(|p| p.pid);
                if pid.is_some_and(|pid| !win::process_alive(pid) || crate::capmode::drop_playing(&self.settings(), pid)) {
                    *self.playing.lock().unwrap() = None;
                }
            }
        }
        let after = self.playing.lock().unwrap().as_ref().map(|p| p.name.clone());
        if before != after {
            util::log(&format!("game: {}", after.as_deref().unwrap_or("none")));
            self.emit("status-changed", ());
        }
        crate::capmode::follow(self); // game mode: the capture moves with the game
        self.sync_game_audio();
    }

    /// Start, switch or stop the game's own audio capture (the "Game" track):
    /// it follows the current game's process while the setting is on and
    /// clipping runs, and nothing runs otherwise.
    pub fn sync_game_audio(&self) {
        let ga = crate::procaudio::global();
        // Game mode records the game's own sound whatever the "Separate game audio track" setting says.
        let on = { let s = self.settings.lock().unwrap(); s.game_audio || crate::capmode::game_mode(&s) };
        // With the captures parked (game mode, the game exited) the game's ring is kept as theirs is.
        let cfgs = self.rec.buffered_cfgs();
        if !on || !self.rec.is_running() || cfgs.is_empty() {
            if ga.status() != "off" {
                ga.off();
            }
            return;
        }
        let pid = self.playing.lock().unwrap().as_ref().map(|p| p.pid);
        // Same length and place as the desktop and microphone rings (see `Recorder::start`).
        let max_secs = cfgs.iter().map(|c| c.max_secs).fold(0.0, f64::max);
        let first = &cfgs[0];
        let cfg = crate::procaudio::RingCfg { max_secs: max_secs + 5.0, window: crate::spill::window(&first.buffer, first.buffer_ram_secs, max_secs), dir: first.buffer_dir.clone() };
        ga.sync(pid, cfg);
    }

    /// Name to file a clip under, as of this moment.
    fn game_now(&self) -> String {
        // Game mode: the game being recorded (a look now could move the capture under the clip).
        let game_mode = crate::capmode::game_mode(&self.settings());
        if !game_mode {
            self.track_game();
        }
        let playing = self.playing.lock().unwrap().as_ref().map(|p| p.name.clone());
        // Game mode after the game exited: a clip of what is left is the last game's.
        let playing = if game_mode { playing.or_else(crate::capmode::last_game) } else { playing };
        playing.unwrap_or_else(|| games::DESKTOP.to_string())
    }

    /// Background upkeep, every two seconds: game detection, display-mode
    /// changes (a game switching resolution). (Discord Rich Presence lived here until 0.1.24; the owner had it removed.)
    pub fn watch(self: &Arc<Self>) {
        // 0.1.46: the end of the Windows session stops the captures quietly and starts nothing (session.rs).
        let ending = self.clone();
        crate::session::watch(Box::new(move || {
            crate::util::log("session: ending; the captures are stopped");
            ending.rec.stop();
        }));
        let core = self.clone();
        thread::spawn(move || {
            let modes = || win::monitors().iter().map(|m| (m.width, m.height, m.hz)).collect::<Vec<_>>();
            let mut seen = modes();
            let mut pending_mode: Option<Vec<(u32, u32, u32)>> = None;
            loop {
                thread::sleep(std::time::Duration::from_secs(2));
                // 0.1.52: a monitor plugged in, turned off or made the main one renumbers the others.
                core.follow_monitors();
                core.track_game();

                let now = modes();
                // A new mode is acted on once it held for one more check (2 s): a game that is
                // starting often switches the display more than once, and each capture restart
                // is a new Graphics Capture session (and a new chance for Windows' yellow border).
                // Until then Graphics Capture keeps its size, cropping or padding the picture.
                if now == seen {
                    pending_mode = None;
                } else if pending_mode.as_ref() != Some(&now) {
                    pending_mode = Some(now);
                } else {
                    pending_mode = None;
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

    /// 0.1.52: the monitor numbers in the settings follow their monitors (`Settings::follow_monitors`),
    /// and the monitor list goes to the log whenever it changes. Called when Glint starts, before a
    /// capture starts and every few seconds from the status poll; true when a number moved.
    pub fn follow_monitors(&self) -> bool {
        static LAST: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mons = win::monitors();
        let mut listed = false;
        // The cheap part decides; the handles and the cards' outputs are only read for the log.
        let key: Vec<String> = mons.iter().map(|m| format!("{} {} {} {}x{} {},{} {}", m.index, m.name, m.primary, m.width, m.height, m.x, m.y, m.id)).collect();
        {
            let mut last = LAST.lock().unwrap();
            if *last != key {
                util::log(&format!("monitors: {} connected{}", mons.len(), if last.is_empty() { "" } else { " (the list changed)" }));
                for l in win::describe_monitors(&mons) {
                    util::log(&format!("  {l}"));
                }
                *last = key;
                listed = true;
            }
        }
        let ids: Vec<String> = mons.iter().map(|m| m.id.clone()).collect();
        let (changed, notes, now) = {
            let mut s = self.settings.lock().unwrap();
            let before = s.monitor;
            let (keep, notes) = s.follow_monitors(&ids);
            if keep {
                s.save();
            }
            (keep && (!notes.is_empty() || s.monitor != before), notes, s.clone())
        };
        for n in &notes {
            util::log(&format!("monitors: {n}"));
        }
        if listed || !notes.is_empty() {
            for l in keybind_targets(&now, &mons) {
                util::log(&format!("  {l}"));
            }
        }
        if changed {
            hotkeys::set_binds(&now.keybinds);
            if self.rec.is_running() {
                let cfgs = capture_cfgs(&now);
                if !cfgs.is_empty() && self.rec.current_cfgs() != cfgs {
                    self.rec.start(cfgs);
                }
            }
            self.emit("settings-changed", ());
            self.emit("status-changed", ());
        }
        changed
    }

    pub fn start_recording(&self) {
        self.follow_monitors();
        // Game mode: look for the game first, so the capture starts on it rather than waiting 2 s.
        if crate::capmode::game_mode(&self.settings()) {
            self.track_game();
        }
        self.rec.start(capture_cfgs(&self.settings()));
    }

    pub fn toggle_recording(&self) {
        if self.rec.is_running() {
            self.rec.stop();
        } else {
            self.start_recording();
        }
        self.sync_game_audio();
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
        let (capture_mode, capture_target) = crate::capmode::status_of(&self.settings(), st.running, &st.captures);
        let game_audio = crate::procaudio::global();
        let (game_ram, game_disk) = game_audio.bytes();
        let buffer_bytes = st.video_bytes + st.audio_bytes + game_ram as u64;
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
            buffer_disk_bytes: st.video_disk_bytes + st.audio_disk_bytes + game_disk as u64,
            cpu_pct: s.cpu_pct,
            ram_app: ram_self.saturating_sub(buffer_bytes),
            ram_ffmpeg: ram_ff,
            ram_total: win::tree_working_set(),
            max_mbps: st.status.maxrate as f64 / 1e6,
            error: st.status.error,
            game: self.playing.lock().unwrap().as_ref().map(|p| p.name.clone()).unwrap_or_default(),
            game_audio: game_audio.status(),
            border: if st.running { st.status.border.clone() } else { String::new() },
            captures,
            capture_mode,
            capture_target,
            capture_why: crate::capmode::why_of(&self.settings(), st.running),
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
                colors: s.theme_colors,
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
    pub fn clip_blocking(self: &Arc<Self>, seconds: u32, monitor: i32) -> Result<ClipMeta, String> {
        let s = self.settings();
        let game_mode = crate::capmode::game_mode(&s);
        // Game mode: a key of the capture monitor clips the game; a key naming another monitor clips
        // that monitor's own buffer, which GAME mode keeps as desktop mode does (0.1.46).
        let count = win::monitors().len();
        let mon = resolve_monitor(&s, monitor, count);
        let primary = mon == capture_monitor(&s, count);
        let game = self.game_now();
        if !self.rec.is_running() {
            self.toast_on("Clipping is paused", "Resume clipping to save clips", mon);
            return Err("Clipping is paused".into());
        }
        // Game mode with no game running: the buffer of the game that exited is kept, so whatever
        // of the asked range it still holds (at least a second) is saved; with nothing, a toast.
        let game_clip = game_mode && primary;
        let waiting = game_clip && !self.rec.game_capturing();
        let snapped = if game_clip { self.rec.snapshot_game(seconds as f64) } else { self.rec.snapshot(seconds as f64, if primary { None } else { Some(mon) }) };
        if waiting && !snapped.as_ref().is_ok_and(|v| crate::capmode::enough_to_clip(&v.0.segments, seconds as f64)) {
            util::log(&format!("clip {seconds}s: no game is running (game mode) and nothing in the buffer for that time"));
            self.toast_on("No game is running", "Glint records the game as soon as one starts", mon);
            return Err("No game is running".into());
        }
        let (snap, desk, mic, cfg) = match snapped {
            Ok(v) => v,
            Err(e) => {
                util::log(&format!("clip {seconds}s of {}: nothing to save: {e}", win::monitor_text(&win::monitors(), mon)));
                self.toast_on("Nothing to clip yet", &e, mon);
                return Err(e);
            }
        };
        if s.sound {
            crate::sound::play(&s, 0);
        }
        // The overlay goes where the picture came from.
        let mon = cfg.monitor;
        // The game's own sound for the same time, when it was captured for any of it.
        // A monitor's clip in GAME mode (0.1.46) is taken like a desktop clip, with the sound GAME mode
        // records: the microphone, the desktop's sound when asked for there, the game's track when on.
        let game_pcm = if s.game_audio || game_clip { crate::procaudio::global().plan(&snap.segments).map(crate::audio::Plan::read) } else { None };
        // Game mode: nothing of the desktop's sound, only the game's and the microphone, unless the
        // user asked for the desktop's sound there too. Then the clip has the usual tracks
        // (Mix, Desktop, Microphone, Game) around the game's picture; track 1 holds the game too
        // when the desktop's sound lacks it (clip::game_missing_from_desktop, 0.1.35).
        let game_only = crate::capmode::game_only(&s) && game_clip;
        let desk = if game_only { None } else { desk };
        let cap = clip::Capture { snap, desk, mic, game: game_pcm, width: cfg.width, height: cfg.height, fps: cfg.fps };
        let started = std::time::Instant::now();
        // The clip is listed and announced as soon as its file is written; its thumbnail (one more
        // ffmpeg run) is made after that, so "Clip saved" does not wait for it.
        match clip::write_quick(&s, cap, &game, game_only) {
            Ok(meta) => {
                util::log(&format!("clip {seconds}s of {}: saved {:.1}s, {:.1} MB in {} ms -> {}", win::monitor_text(&win::monitors(), mon), meta.duration, meta.size as f64 / 1e6, started.elapsed().as_millis(), meta.path));
                // Until the picture is made, the missing-thumbnail retry (polish.rs) leaves this clip alone.
                crate::import::set_thumb_pending(&meta.id, true);
                self.lib.lock().unwrap().add(meta.clone());
                self.emit("clips-changed", meta.id.clone());
                // "last 30s" when the clip is as long as asked; its real length when the buffer held less.
                let len = if (meta.duration - seconds as f64).abs() < 2.0 { seconds as f64 } else { meta.duration };
                self.toast_on("Clip saved", &captured_text(&s, len, &meta.game), mon);
                let (core, id, path, duration, ffmpeg_path) = (self.clone(), meta.id.clone(), PathBuf::from(&meta.path), meta.duration, s.ffmpeg_path.clone());
                std::thread::spawn(move || {
                    let thumb = util::find_exe(&ffmpeg_path, "ffmpeg").map(|f| crate::library::make_thumb(&f, &path, &id, crate::library::thumb_at(duration))).unwrap_or_default();
                    crate::import::set_thumb_pending(&id, false); // made or not: the retry may take it from here
                    if !thumb.is_empty() {
                        let listed = core.lib.lock().unwrap().update(&id, |c| c.thumb = thumb.clone()).is_some();
                        if listed {
                            core.emit("clips-changed", ()); // no id: this is not a new clip, so no sound
                        } else {
                            let _ = std::fs::remove_file(&thumb); // the clip was deleted meanwhile
                        }
                    }
                });
                Ok(meta)
            }
            Err(e) => {
                util::log(&format!("clip {seconds}s of {}: failed: {e}", win::monitor_text(&win::monitors(), mon)));
                self.toast_on("Clip failed", &e, mon);
                Err(e)
            }
        }
    }

    /// The clip the "edit" keybind asked for, handed out once.
    pub fn take_pending_edit(&self) -> Option<String> {
        self.pending_edit.lock().unwrap().take()
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
        if b.action == "clip" {
            // Which screen this press saves, in the window's own words: a keybind that names a
            // monitor saves that one whatever Settings > Capture > Monitor says.
            let s = self.settings();
            util::log(&format!("hotkey {} -> clip {}s of {}", id, b.seconds, clip_target_text(&s, &win::monitors(), b.monitor)));
        } else {
            util::log(&format!("hotkey {} -> {} {}s (monitor {}{})", id, b.action, b.seconds, b.monitor, if b.monitor_id.is_empty() { String::new() } else { format!(", {}", b.monitor_id) }));
        }
        match b.action.as_str() {
            "clip" => self.clip(b.seconds, b.monitor),
            "toggle" => {
                self.toggle_recording();
                let on = self.rec.is_running();
                self.toast(if on { "Clipping resumed" } else { "Clipping paused" }, if on { "Replay buffer is filling" } else { "Replay buffer cleared" });
                self.emit("status-changed", ());
            }
            // A plugin's own action ("plugin:<id>:<action>", plugins.rs): nothing happens when its
            // plugin is not installed or is switched off.
            a if a.starts_with("plugin:") => crate::plugins::fire(self, a),
            "show" => {
                if let Some(app) = self.app.get() {
                    crate::app::show_main(app);
                }
            }
            // "Edit the last clip" (0.1.25): bring Glint up with the newest clip open in the editor.
            // The page takes the clip's id itself (`take_pending_edit`): on "edit-last" when the
            // window is open, or when it starts if this key had to open it.
            "edit" => {
                let newest = self.lib.lock().unwrap().visible().into_iter().max_by_key(|c| c.created).map(|c| c.id);
                let Some(id) = newest else {
                    self.toast("No clip to edit yet", "Save a clip first");
                    return;
                };
                *self.pending_edit.lock().unwrap() = Some(id);
                if let Some(app) = self.app.get() {
                    crate::app::show_main(app);
                }
                self.emit("edit-last", ());
            }
            // Switch the capture monitor: to the keybind's own monitor, or with -1 to
            // the next one in Windows' list. Clip keybinds set to "capture monitor",
            // the tray and the top bar's Clip button all follow it. A monitor that was
            // not being buffered starts with an empty buffer.
            "monitor" => {
                let mons = win::monitors();
                if mons.len() < 2 {
                    return;
                }
                let mut s = self.settings();
                let cur = capture_monitor(&s, mons.len());
                let to = if b.monitor >= 0 && (b.monitor as usize) < mons.len() { b.monitor as u32 } else { (cur + 1) % mons.len() as u32 };
                if to != cur {
                    s.monitor = to;
                    self.apply_settings(s);
                    util::log(&format!("capture monitor: {} -> {}", win::monitor_text(&mons, cur), win::monitor_text(&mons, to)));
                }
                let m = &mons[to as usize];
                self.toast_on("Capture monitor", &format!("Monitor {} · {}×{}", m.index + 1, m.width, m.height), to);
                // The window re-reads the settings: its Monitor dropdown and keybind labels show the capture monitor.
                self.emit("settings-changed", ());
                self.emit("status-changed", ());
            }
            _ => {}
        }
    }

    pub fn apply_settings(&self, mut new: Settings) {
        {
            // A monitor number changed by hand takes that monitor's id (0.1.52).
            let ids = win::monitor_ids();
            let old = self.settings.lock().unwrap().clone();
            new.name_monitors(&old, &ids);
        }
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
        util::set_export_priority(&new.export_priority);
        hotkeys::set_binds(&new.keybinds);
        // A running audio capture switches device on its own (audio.rs); nothing else restarts.
        crate::audio::set_devices(&new.desktop_device, &new.mic_device);
        crate::audio::set_gains(new.desktop_gain_db, new.mic_gain_db, new.game_gain_db);
        // A custom clip sound is decoded now, so the next clip key plays it at once.
        if new.sound_name == "custom" && (new.sound_name != old.sound_name || new.sound_file != old.sound_file || new.sound_start != old.sound_start || new.sound_end != old.sound_end || new.sound_volume != old.sound_volume || new.ffmpeg_path != old.ffmpeg_path) {
            let s = new.clone();
            std::thread::spawn(move || { crate::sound::warm(&s, 0); });
        }
        if new.autostart != old.autostart {
            set_autostart(new.autostart);
        }
        // The GAME / DESKTOP switch or a game mark changed: look for the game again now (capmode.rs).
        if crate::capmode::marks_changed(&old, &new) {
            // Switched by hand: the game's capture and its buffer go; a monitor capture that stays the
            // same in the other mode (a monitor a clip keybind names) keeps running with its buffer (0.1.46).
            if old.capture_mode != new.capture_mode && self.rec.is_running() {
                self.rec.drop_game();
            }
            self.track_game();
        }
        if self.rec.is_running() {
            // Only the monitors whose capture changed are restarted (see `Recorder::start`).
            let cfgs = capture_cfgs(&new);
            if cfgs.is_empty() && old.capture_mode != new.capture_mode {
                // Switched to GAME with no game and no other monitor to buffer: nothing is kept.
                self.rec.stop();
                self.rec.wait();
            } else if self.rec.current_cfgs() != cfgs {
                self.rec.start(cfgs);
            }
        }
        if new.game_audio != old.game_audio || self.rec.is_running() {
            self.sync_game_audio();
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
        let names = self.folders();
        let mut lib = self.lib.lock().unwrap();
        let before = lib.clips.len();
        lib.clips.retain(|c| c.cloud_only || std::path::Path::new(&c.path).is_file());
        let mut changed = lib.clips.len() != before;
        // A draft keeps the folder of its first clip (its export goes there); its file is in `.drafts`.
        // A clip imported in place (its file outside the clips folder) keeps the folder it was
        // filed in while that folder exists: the folder only lists it, the file never moves.
        for c in lib.clips.iter_mut().filter(|c| !c.draft) {
            let f = if in_clips_dir(&root, &c.path) {
                folder_of(&root, &c.path)
            } else {
                names.iter().find(|n| n.eq_ignore_ascii_case(&c.folder)).cloned().unwrap_or_default()
            };
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
        for c in lib.clips.iter_mut().filter(|c| !c.draft && c.folder.eq_ignore_ascii_case(from)) {
            // Only files that were inside the folder moved with it; a clip imported in place keeps its path.
            let inside = Path::new(&c.path).parent().is_some_and(|p| same_dir(p, &old));
            if let Some(file) = Path::new(&c.path).file_name().filter(|_| inside) {
                c.path = new.join(file).to_string_lossy().into_owned();
            }
            c.folder = to.clone();
        }
        lib.save();
        drop(lib);
        // The folder keeps its color under its new name.
        let mut s = self.settings.lock().unwrap();
        if s.carry_folder_color(from, Some(&to)) {
            s.save();
        }
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
        std::fs::remove_dir(&dir).map_err(|_| "The clips were moved out, but the folder still holds other files, so it was kept".to_string())?;
        // A folder that is gone loses its color (a kept one keeps it).
        let mut s = self.settings.lock().unwrap();
        if s.carry_folder_color(name, None) {
            s.save();
        }
        Ok(())
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
            for c in lib.clips.iter_mut().filter(|c| !c.draft && ids.contains(&c.id)) {
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
        let meta = self.lib.lock().unwrap().get_saved(id)?;
        // A queued or running render (or "Edit together" of it) holds the old path; renaming under it would break it.
        if self.jobs.busy(id) {
            return Err("This clip is being rendered. Rename it when the render has finished".into());
        }
        // A clip imported in place belongs to another program: only its title changes, never its file (0.1.18).
        if meta.kind == "import" && !in_clips_dir(Path::new(&self.settings().clips_dir), &meta.path) {
            return self.lib.lock().unwrap().update(id, |c| c.title = title).ok_or_else(|| "Clip not found".into());
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

    /// The exported clip `id` takes the place of clips `ids` (since 0.1.23, when
    /// the owner asked for "replace the clip" beside "save as a new copy").
    /// Each of `ids` is removed with `bin` (the Recycle Bin; a test passes its
    /// own) and dropped from the library. With `adopt` (one clip was trimmed
    /// or edited) the new clip then takes the first one's name (its file too,
    /// as `rename_clip` does), date and favourite, so it sits where the
    /// original was. A clip a job still uses, or whose file cannot be removed,
    /// stays and is named in `failed`. Nothing is removed unless the new
    /// clip's file exists.
    pub fn replace_clips(&self, id: &str, ids: &[String], adopt: bool, bin: &dyn Fn(&Path) -> Result<(), String>) -> Result<Replaced, String> {
        let new = self.lib.lock().unwrap().get_saved(id)?;
        if !Path::new(&new.path).is_file() {
            return Err("The exported clip's file is missing, so nothing was replaced".into());
        }
        let _once = crate::library::batch(&self.lib); // library.json is written once, at the end
        let mut out = Replaced::default();
        let mut first: Option<ClipMeta> = None;
        let mut seen: Vec<&String> = Vec::new();
        for (n, old) in ids.iter().enumerate() {
            if old == id || seen.contains(&old) {
                continue;
            }
            seen.push(old);
            // A draft, or a clip that is gone already: nothing to replace.
            let Ok(meta) = self.lib.lock().unwrap().get_saved(old) else { continue };
            if self.jobs.busy(old) {
                out.failed.push(format!("{}: it is being rendered", meta.title));
                continue;
            }
            let file = Path::new(&meta.path);
            if file.exists() {
                // The player that showed the clip may need a moment to let go of the file.
                let mut res = bin(file);
                for _ in 0..5 {
                    if res.is_ok() {
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(200));
                    res = bin(file);
                }
                if let Err(e) = res {
                    out.failed.push(format!("{}: {e}", meta.title));
                    continue;
                }
            }
            if !meta.thumb.is_empty() {
                let _ = std::fs::remove_file(&meta.thumb);
            }
            self.lib.lock().unwrap().remove(old);
            util::log(&format!("replace: clip {old} ({}) replaced by {id}", meta.path));
            if n == 0 {
                first = Some(meta);
            }
            out.replaced.push(old.clone());
        }
        if let (true, Some(o)) = (adopt, first) {
            // The name, and the file's name with it. If the file cannot be renamed the title still follows.
            if let Err(e) = self.rename_clip(id, &o.title) {
                util::log(&format!("replace: {id} keeps its file name ({e})"));
                self.lib.lock().unwrap().update(id, |c| c.title = o.title.clone());
            }
            self.lib.lock().unwrap().update(id, |c| {
                c.created = o.created;
                c.favorite = o.favorite;
            });
        }
        out.clip = self.lib.lock().unwrap().get(id).unwrap_or(new);
        Ok(out)
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
        let _once = crate::library::batch(&self.lib); // library.json is written once, at the end
        for id in ids {
            let Some(meta) = self.lib.lock().unwrap().get(id) else { continue };
            let src = PathBuf::from(&meta.path);
            // A draft's file stays in `.drafts`.
            if meta.draft || src.parent().map(|p| same_dir(p, &dest)).unwrap_or(false) {
                continue;
            }
            // A clip imported in place belongs to another program: its file is never moved (nor
            // copied and deleted). The library folder only lists it.
            if meta.kind == "import" && !in_clips_dir(&root, &meta.path) {
                if !meta.folder.eq_ignore_ascii_case(&folder) {
                    self.lib.lock().unwrap().update(id, |c| c.folder = folder.clone());
                    moved += 1;
                }
                continue;
            }
            // A queued or running render (or "Edit together") reads the file where it is.
            if self.jobs.busy(id) {
                failed.push(format!("{} (it is being rendered)", meta.title));
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

/// What `Core::replace_clips` did.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Replaced {
    /// The new clip as it is now (with the original's name when it adopted one).
    pub clip: ClipMeta,
    /// Ids of the clips that were removed.
    pub replaced: Vec<String>,
    /// Why each of the others stayed: "<title>: <reason>".
    pub failed: Vec<String>,
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

/// Whether a file is inside the clips folder (at any depth). Clips imported in
/// place are not: their files belong to the program that made them.
pub fn in_clips_dir(clips_dir: &Path, path: &str) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let root = norm(clips_dir);
    !root.is_empty() && norm(Path::new(path)).starts_with(&format!("{root}\\"))
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
