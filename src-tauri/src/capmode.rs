//! Game or Desktop clipping (0.1.27): the top bar's GAME / DESKTOP switch.
//!
//! "desktop" (the default) is Glint as it always was: the capture monitor and
//! every other monitor a clip keybind names, with all desktop sound.
//!
//! "game" records only the game: the picture is the game's WINDOW (Graphics
//! Capture's `hwnd` option, `recorder::capture_args`), the sound is the game's
//! own (process loopback, `procaudio.rs`) plus the microphone. A clip's tracks
//! are Mix (game + microphone), Game, Microphone (`clip::write_game`).
//!
//! - Detection (`found`, every 2 s from `Core::track_game`): every visible
//!   top-level window is scored (`games::rate` / `games::choose`): the user's
//!   hand pick, the user's game / not-a-game marks, the known table, games
//!   seen before, store folders, fullscreen; the foreground window and the
//!   game being recorded get a bonus. Only window facts and executable paths
//!   are used (no other process is read).
//! - No game: the recorder waits (`Recorder::wait`), the status says
//!   "waiting", a clip key shows "No game is running". A game appears: the
//!   capture starts on it; it exits: the capture stops; another game takes
//!   over: the capture moves (`follow`), into the same buffer when the window
//!   has the same size (`Pipe::move_to`, a new epoch).
//! - A size change of the window is acted on once it held for one more look
//!   (2 s), as a display mode change is.
//! - Fallback: no window capture (ffmpeg without `gfxcapture`, Desktop
//!   Duplication chosen or picked by "auto", e.g. on Windows 10), or the
//!   window refused twice in a row (`RecStatus::window_failed`): the monitor
//!   the game is on is recorded instead, still with only the game's sound.
use crate::core::{self, Core};
use crate::games::{self, Game, Marks};
use crate::recorder::{CapStats, CaptureCfg};
use crate::settings::Settings;
use crate::util;
use crate::win::{self, Monitor, WinInfo};
use serde::Serialize;
use std::sync::{Arc, Mutex, OnceLock};

pub fn game_mode(s: &Settings) -> bool {
    s.capture_mode == "game"
}

/// The window being recorded in game mode.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Target {
    pub hwnd: u64,
    pub pid: u32,
    pub exe: String,
    pub name: String,
    /// Index into `win::monitors()`.
    pub monitor: u32,
    /// Its client area (the size it had when last seen not minimized).
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
struct State {
    target: Option<Target>,
    /// A new size of the target's window seen once: (window, width, height).
    pending: Option<(u64, u32, u32)>,
    /// Windows whose capture failed twice in a row: their monitor is recorded instead.
    failed: Vec<u64>,
    /// A window capture that has not delivered its first frame yet: (pipeline id, window, since).
    silent: Option<(u64, u64, std::time::Instant)>,
    /// The last game recorded: a clip of the buffer it left after it exited is filed under it.
    last: Option<String>,
}

/// The least of the asked range a clip taken while waiting (the game exited) must hold, in seconds.
pub const MIN_WAITING_CLIP: f64 = 1.0;

/// Seconds of the asked range (the last `secs` seconds before `now`) that a snapshot's
/// wall-clock `segments` cover.
pub fn held_in_range(segments: &[(f64, f64)], secs: f64, now: f64) -> f64 {
    let from = now - secs;
    segments.iter().map(|&(a, b)| (b.min(now) - a.max(from)).max(0.0)).sum()
}

/// A snapshot taken while waiting for a game (the game exited) is worth saving: it holds at
/// least `MIN_WAITING_CLIP` seconds of the asked range.
pub fn enough_to_clip(segments: &[(f64, f64)], secs: f64) -> bool {
    held_in_range(segments, secs, util::now_secs()) >= MIN_WAITING_CLIP
}

/// The name of the last game recorded in game mode, if any.
pub fn last_game() -> Option<String> {
    state().lock().unwrap().last.clone()
}

/// A window capture that runs but has not delivered one frame in this long while the window is
/// not minimized (seen with games in exclusive fullscreen) counts as failed too.
const FIRST_FRAME_SECS: u64 = 12;

/// Whether the window capture `c` has gone silent from its start for too long (see `FIRST_FRAME_SECS`).
fn never_delivered(silent: &mut Option<(u64, u64, std::time::Instant)>, c: &CapStats, minimized: bool, now: std::time::Instant) -> bool {
    if c.window == 0 || c.frames > 0 || minimized {
        *silent = None;
        return false;
    }
    match silent {
        Some((id, w, since)) if *id == c.id && *w == c.window => now.duration_since(*since).as_secs() >= FIRST_FRAME_SECS,
        _ => {
            *silent = Some((c.id, c.window, now));
            false
        }
    }
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

fn marks<'a>(s: &'a Settings, seen: &'a [Game], current: Option<u32>) -> Marks<'a> {
    Marks { auto: s.game_auto, pick: &s.game_pick, games: &s.game_exes, not_games: &s.not_game_exes, seen, current }
}

/// Nothing marked and detection on: desktop mode's detection is exactly what it was before 0.1.27.
fn unmarked(s: &Settings) -> bool {
    s.game_auto && s.game_pick.is_empty() && s.game_exes.is_empty() && s.not_game_exes.is_empty()
}

/// Width and height to record a client area of `cw` x `ch` at: even numbers, scaled down when
/// the height setting (`cap`, 0 = none) is below the window's height. (width, height, scaled).
pub fn window_size(cw: u32, ch: u32, cap: u32) -> (u32, u32, bool) {
    let even = |v: u32| (v.max(2) / 2) * 2;
    if cap > 0 && cap < ch {
        let h = even(cap.max(240));
        let w = even((cw as f64 * h as f64 / ch.max(1) as f64).round() as u32);
        (w, h, true)
    } else {
        (even(cw), even(ch), false)
    }
}

/// The capture of target `t` on its monitor `m`: its window when `window` (window capture is
/// possible and has not failed for it), else the whole monitor (today's path). Either way
/// without desktop sound, and with a buffer for the longest clip keybind (all clip keys take the game).
pub fn cfg_for(s: &Settings, t: &Target, m: &Monitor, window: bool) -> CaptureCfg {
    let mut c = core::capture_cfg_for(s, m, s.max_clip_secs());
    c.desktop_audio = false;
    if window && t.hwnd != 0 && t.width > 0 && t.height > 0 {
        let (w, h, scaled) = window_size(t.width, t.height, s.height);
        c.window = t.hwnd;
        c.width = w;
        c.height = h;
        c.scaled = scaled;
    }
    c
}

/// This PC can capture one window: ffmpeg has Graphics Capture and the capture method (the
/// setting, or what "auto" picks: Desktop Duplication on Windows 10) is Graphics Capture.
pub fn window_capture_ok(s: &Settings) -> bool {
    util::find_exe(&s.ffmpeg_path, "ffmpeg").is_some_and(|f| crate::recorder::resolve_method(&f, &s.method) == "wgc")
}

/// Game mode's capture list: the game, or nothing (the recorder waits).
pub fn cfgs(s: &Settings) -> Vec<CaptureCfg> {
    let (target, failed) = {
        let st = state().lock().unwrap();
        let failed = st.target.as_ref().is_some_and(|t| st.failed.contains(&t.hwnd));
        (st.target.clone(), failed)
    };
    let Some(t) = target else { return Vec::new() };
    let mons = win::monitors();
    let Some(m) = mons.get(t.monitor as usize).or(mons.first()) else { return Vec::new() };
    vec![cfg_for(s, &t, m, !failed && window_capture_ok(s))]
}

/// The next target from a list of windows. `prev`: the one recorded now. `pending`: a size seen
/// once (see `State`). `alive`: whether a process still runs.
pub fn next_target(prev: Option<&Target>, wins: &[WinInfo], m: &Marks, pending: &mut Option<(u64, u32, u32)>, alive: &dyn Fn(u32) -> bool, mons: &[Monitor]) -> Option<Target> {
    let Some(i) = games::choose(wins, m) else {
        // Its window is gone for a moment (a game re-creating it) while it still runs: keep it,
        // unless the user has just said it is not the game.
        let key = |t: &Target| games::exe_key(&t.exe);
        return prev
            .filter(|p| alive(p.pid) && !wins.iter().any(|w| w.pid == p.pid) && !m.not_games.contains(&key(p)) && (m.auto || m.pick == key(p)))
            .cloned();
    };
    let w = &wins[i];
    let same_window = prev.filter(|p| p.hwnd == w.hwnd);
    let same_game = prev.filter(|p| p.pid == w.pid);
    let (mut width, mut height) = (w.width, w.height);
    if w.minimized || width == 0 || height == 0 {
        // Minimized: no frames come, the clip keeps its last picture. The size it had, else its monitor's.
        (width, height) = match same_window.or(same_game) {
            Some(p) => (p.width, p.height),
            None => mons.get(w.monitor as usize).map_or((1920, 1080), |mon| (mon.width, mon.height)),
        };
        *pending = None;
    } else if let Some(p) = same_window {
        if (width, height) == (p.width, p.height) {
            *pending = None;
        } else if *pending == Some((w.hwnd, width, height)) {
            *pending = None; // held for a second look: take it
        } else {
            *pending = Some((w.hwnd, width, height));
            (width, height) = (p.width, p.height);
        }
    } else {
        *pending = None;
    }
    // The name is looked up once per game (it may read the program's version info).
    let name = same_game.map(|p| p.name.clone()).unwrap_or_else(|| games::name_for(w, m.seen));
    Some(Target { hwnd: w.hwnd, pid: w.pid, exe: w.exe.clone(), name, monitor: w.monitor, width, height })
}

/// `Core::track_game`'s look: (name, process, executable) of the game, if any. Desktop mode:
/// the foreground window as always (with the user's marks when there are any). Game mode: all
/// windows, and the target the capture follows is updated.
pub fn found(core: &Core) -> Option<(String, u32, String)> {
    let s = core.settings();
    if !game_mode(&s) {
        if unmarked(&s) {
            return games::detect();
        }
        let seen = core.games.lock().unwrap().list.clone();
        return games::detect_marked(&marks(&s, &seen, None));
    }
    let seen = core.games.lock().unwrap().list.clone();
    let caps = core.rec.stats().captures;
    let wins = win::windows();
    let mons = win::monitors();
    let mut st = state().lock().unwrap();
    for c in caps.iter().filter(|c| c.window != 0 && c.status.window_failed) {
        if !st.failed.contains(&c.window) {
            util::log(&format!("game mode: window {:#x} could not be captured, recording its monitor instead", c.window));
            st.failed.push(c.window);
        }
    }
    if let Some(c) = caps.first() {
        let minimized = wins.iter().find(|w| w.hwnd == c.window).map_or(true, |w| w.minimized);
        let mut silent = st.silent.take();
        if never_delivered(&mut silent, c, minimized, std::time::Instant::now()) && !st.failed.contains(&c.window) {
            util::log(&format!("game mode: window {:#x} gave no picture in {FIRST_FRAME_SECS} s, recording its monitor instead", c.window));
            st.failed.push(c.window);
            silent = None;
        }
        st.silent = silent;
    }
    st.failed.retain(|h| wins.iter().any(|w| w.hwnd == *h));
    let prev = st.target.clone();
    let m = marks(&s, &seen, prev.as_ref().map(|t| t.pid));
    let mut pending = st.pending.take();
    let next = next_target(prev.as_ref(), &wins, &m, &mut pending, &win::process_alive, &mons);
    st.pending = pending;
    if next.as_ref().map(|t| (t.hwnd, t.pid)) != prev.as_ref().map(|t| (t.hwnd, t.pid)) {
        match &next {
            Some(t) => util::log(&format!("game mode: {} ({}, window {:#x}, {}x{} on monitor {})", t.name, games::exe_key(&t.exe), t.hwnd, t.width, t.height, t.monitor)),
            None => util::log("game mode: no game"),
        }
    }
    if let Some(t) = &next {
        st.last = Some(t.name.clone());
    }
    st.target = next.clone();
    next.map(|t| (t.name, t.pid, t.exe))
}

/// Whether the game being played (`pid`) stops counting although its process lives: always in
/// game mode (it follows the target), and in desktop mode once the user marked it "not a game"
/// or turned detection off without picking it.
pub fn drop_playing(s: &Settings, pid: u32) -> bool {
    if game_mode(s) {
        return true;
    }
    if unmarked(s) {
        return false;
    }
    let Some(key) = win::exe_of(pid).map(|e| games::exe_key(&e)) else { return false };
    s.not_game_exes.contains(&key) || (!s.game_auto && s.game_pick != key)
}

/// Game mode: start, move or stop the capture to match the game (after `found`).
pub fn follow(core: &Core) {
    let s = core.settings();
    if !game_mode(&s) || !core.rec.is_running() {
        return;
    }
    let cfgs = core::capture_cfgs(&s);
    if core.rec.current_cfgs() != cfgs {
        let what = match cfgs.first() {
            None => "nothing, waiting for a game".to_string(),
            Some(c) if c.window != 0 => format!("window {:#x} at {}x{} @ {} fps", c.window, c.width, c.height, c.fps),
            Some(c) => format!("the whole monitor {} at {}x{} @ {} fps", c.monitor, c.width, c.height, c.fps),
        };
        util::log(&format!("game mode: capturing {what}"));
        core.rec.start(cfgs);
        core.emit("status-changed", ());
    }
}

/// `Status::capture_mode` and `Status::capture_target` (see core.rs).
pub fn status_of(s: &Settings, running: bool, caps: &[CapStats]) -> (String, String) {
    if !game_mode(s) {
        return ("desktop".into(), String::new());
    }
    let target = if !running {
        ""
    } else {
        match caps.first() {
            None => "waiting",
            Some(c) if c.window != 0 => "window",
            Some(_) => {
                let st = state().lock().unwrap();
                if st.target.as_ref().is_some_and(|t| st.failed.contains(&t.hwnd)) { "screen_failed" } else { "screen" }
            }
        }
    };
    ("game".into(), target.into())
}

/// A setting that changes what is recorded in game mode (or the mode itself) changed.
pub fn marks_changed(old: &Settings, new: &Settings) -> bool {
    old.capture_mode != new.capture_mode || old.game_auto != new.game_auto || old.game_pick != new.game_pick || old.game_exes != new.game_exes || old.not_game_exes != new.not_game_exes
}

/// One choice of the switch's menu: the user's mark `action` on executable `exe` (`games::exe_key`).
pub fn apply_mark(s: &mut Settings, action: &str, exe: &str) -> Result<(), String> {
    let key = exe.trim().to_lowercase();
    let need = || if key.is_empty() { Err("No program was named".to_string()) } else { Ok(()) };
    match action {
        "auto-on" => s.game_auto = true,
        "auto-off" => s.game_auto = false,
        "pick" => {
            need()?;
            s.game_pick = key.clone();
            s.not_game_exes.retain(|e| *e != key);
            if !s.game_exes.contains(&key) {
                s.game_exes.push(key);
            }
        }
        "unpick" => s.game_pick.clear(),
        "not-game" => {
            need()?;
            s.game_exes.retain(|e| *e != key);
            if s.game_pick == key {
                s.game_pick.clear();
            }
            if !s.not_game_exes.contains(&key) {
                s.not_game_exes.push(key);
            }
        }
        "is-game" => {
            need()?;
            s.not_game_exes.retain(|e| *e != key);
            if !s.game_exes.contains(&key) {
                s.game_exes.push(key);
            }
        }
        _ => return Err(format!("Unknown choice \"{action}\"")),
    }
    Ok(())
}

/// A running program in the switch's menu.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MenuRow {
    /// `games::exe_key`, what a mark is kept by.
    pub exe: String,
    pub name: String,
    /// Its window's title (shortened).
    pub title: String,
    /// Detection counts it as a game.
    pub game: bool,
    /// It is the game being recorded (or played, in desktop mode).
    pub current: bool,
    pub picked: bool,
    /// The user marked it "not a game".
    pub not_game: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct GameMenu {
    pub mode: String,
    pub auto: bool,
    pub pick: String,
    /// The game being recorded / played now, if any.
    pub current: Option<MenuRow>,
    /// Running programs with a visible window, one row each: the current game first, then games, then by name.
    pub programs: Vec<MenuRow>,
}

/// Programs of Windows' own shell: never offered in the menu.
const SHELL: &[&str] = &["explorer.exe", "searchhost.exe", "shellexperiencehost.exe", "startmenuexperiencehost.exe", "textinputhost.exe", "applicationframehost.exe", "lockapp.exe", "systemsettings.exe", "dwm.exe"];
const MENU_MAX: usize = 24;

/// The menu's rows from a window list.
pub fn menu_rows(wins: &[WinInfo], m: &Marks, current: Option<u32>) -> Vec<MenuRow> {
    let mut rows: Vec<(bool, MenuRow)> = Vec::new();
    for w in wins {
        let exe = games::exe_key(&w.exe);
        if exe.is_empty() || SHELL.contains(&exe.as_str()) {
            continue;
        }
        let is_current = current == Some(w.pid);
        let game = games::rate(w, m).is_some();
        if let Some((_, r)) = rows.iter_mut().find(|(_, r)| r.exe == exe) {
            r.current |= is_current;
            r.game |= game;
            continue;
        }
        let title: String = w.title.trim().chars().take(60).collect();
        rows.push((w.minimized, MenuRow { name: games::name_for(w, m.seen), exe: exe.clone(), title, game, current: is_current, picked: m.pick == exe, not_game: m.not_games.contains(&exe) }));
    }
    let mut rows: Vec<MenuRow> = rows.into_iter().map(|(_, r)| r).collect();
    rows.sort_by(|a, b| b.current.cmp(&a.current).then(b.picked.cmp(&a.picked)).then(b.game.cmp(&a.game)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    rows.truncate(MENU_MAX);
    rows
}

type St<'a> = tauri::State<'a, Arc<Core>>;

/// The switch's right-click menu (ui/capmode.js).
#[tauri::command(async)]
pub fn game_menu(core: St) -> GameMenu {
    let s = core.settings();
    let seen = core.games.lock().unwrap().list.clone();
    let current = if game_mode(&s) { state().lock().unwrap().target.as_ref().map(|t| t.pid) } else { core.playing_pid() };
    let m = marks(&s, &seen, current);
    let programs = menu_rows(&win::windows(), &m, current);
    let cur = programs.iter().find(|r| r.current).cloned();
    GameMenu { mode: s.capture_mode.clone(), auto: s.game_auto, pick: s.game_pick.clone(), current: cur, programs }
}

/// A choice in that menu (see `apply_mark`). Returns the settings as saved.
#[tauri::command(async)]
pub fn game_mark(core: St, action: String, exe: String) -> Result<Settings, String> {
    let mut s = core.settings();
    apply_mark(&mut s, &action, &exe)?;
    util::log(&format!("game mode: {action} {exe}"));
    core.apply_settings(s);
    core.emit("status-changed", ());
    Ok(core.settings())
}

// ---------------------------------------------------------------------------
// Self-test: `glint.exe --capmode-test <report.txt>`. Made-up windows, ffmpeg
// arguments, the recorder without ffmpeg, and clips written from made-up sound
// and a synthetic video. Nothing is captured.

struct Report {
    lines: Vec<String>,
    failed: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if !ok {
            self.failed += 1;
        }
        self.lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  [{detail}]") }));
    }
}

fn wi(hwnd: u64, pid: u32, exe: &str) -> WinInfo {
    WinInfo { hwnd, pid, exe: exe.into(), title: String::new(), class: String::new(), fullscreen: false, minimized: false, foreground: false, width: 1920, height: 1080, monitor: 0 }
}

fn mon(index: u32, width: u32, height: u32, hz: u32) -> Monitor {
    Monitor { index, name: format!("DISPLAY{}", index + 1), width, height, hz, primary: index == 0, x: 0, y: 0, native_width: width, native_height: height }
}

pub fn self_test(report_path: &str) {
    let mut r = Report { lines: Vec::new(), failed: 0 };
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
    } else {
        test_settings(&mut r);
        test_detection(&mut r);
        test_targets(&mut r);
        test_args(&mut r);
        test_recorder(&mut r);
        test_clips(&mut r);
    }
    let n = r.lines.len();
    let f = r.failed;
    r.lines.push(format!("{n} checks, {f} failed"));
    let _ = std::fs::write(report_path, r.lines.join("\n") + "\n");
}

fn test_settings(r: &mut Report) {
    let d = Settings::default();
    r.check("default: desktop mode, detection on, nothing marked", d.capture_mode == "desktop" && !game_mode(&d) && d.game_auto && unmarked(&d), "");
    // A settings file from 0.1.26 (no new keys) loads with the defaults.
    let old = r#"{"clips_dir":"C:\\Clips","fps":120,"mic":false,"game_audio":true}"#;
    let s: Settings = serde_json::from_str(old).unwrap_or_default();
    r.check("old settings load: other keys kept, new ones default", s.fps == 120 && !s.mic && s.game_audio && s.capture_mode == "desktop" && s.game_auto && s.game_pick.is_empty() && s.game_exes.is_empty() && s.not_game_exes.is_empty(), &format!("fps {} mode {}", s.fps, s.capture_mode));
    let mut g = Settings { capture_mode: "game".into(), ..Default::default() };
    let _ = apply_mark(&mut g, "pick", "Hades.EXE");
    let back: Settings = serde_json::to_string(&g).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    r.check("game mode and marks survive a save", back.capture_mode == "game" && back.game_pick == "hades.exe" && back.game_exes == ["hades.exe"], &format!("{} {:?}", back.game_pick, back.game_exes));
    // Marks.
    let mut s = Settings::default();
    let _ = apply_mark(&mut s, "pick", "a.exe");
    let _ = apply_mark(&mut s, "not-game", "a.exe");
    r.check("mark: not a game undoes a pick", s.game_pick.is_empty() && s.game_exes.is_empty() && s.not_game_exes == ["a.exe"], "");
    let _ = apply_mark(&mut s, "pick", "a.exe");
    r.check("mark: picking it again clears the not-a-game mark", s.game_pick == "a.exe" && s.not_game_exes.is_empty() && s.game_exes == ["a.exe"], "");
    let _ = apply_mark(&mut s, "unpick", "");
    let _ = apply_mark(&mut s, "auto-off", "");
    r.check("mark: unpick keeps it a game; detection off", s.game_pick.is_empty() && s.game_exes == ["a.exe"] && !s.game_auto, "");
    r.check("mark: an unknown choice or a missing program is refused", apply_mark(&mut s, "boom", "x.exe").is_err() && apply_mark(&mut s, "pick", " ").is_err(), "");
    let a = Settings::default();
    let mut b = a.clone();
    b.fps = 30;
    let mut c = a.clone();
    c.capture_mode = "game".into();
    let mut e = a.clone();
    e.not_game_exes.push("x.exe".into());
    r.check("marks_changed: only the mode and the marks count", !marks_changed(&a, &b) && marks_changed(&a, &c) && marks_changed(&a, &e), "");
    r.check("drop_playing: game mode follows its target; desktop mode unmarked keeps the game", drop_playing(&c, 1) && !drop_playing(&a, 1), "");
}

fn test_detection(r: &mut Report) {
    let none: [String; 0] = [];
    let base = Marks::none();
    let pick = |p: &'static str| Marks { pick: p, ..Marks::none() };
    let chosen = |wins: &[WinInfo], m: &Marks| games::choose(wins, m).map(|i| games::exe_key(&wins[i].exe));

    let chrome = WinInfo { fullscreen: true, foreground: true, ..wi(1, 10, r"C:\Program Files\Google\Chrome\Application\chrome.exe") };
    let vlc = WinInfo { fullscreen: true, ..wi(2, 11, r"C:\Program Files\VideoLAN\VLC\vlc.exe") };
    r.check("a fullscreen browser or player is not a game", chosen(&[chrome.clone(), vlc.clone()], &base).is_none(), "");

    let cs2 = wi(3, 12, r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe");
    let notepad = WinInfo { foreground: true, ..wi(4, 13, r"C:\Windows\notepad.exe") };
    r.check("a known game in the background is found", chosen(&[notepad.clone(), cs2.clone(), chrome.clone()], &base).as_deref() == Some("cs2.exe"), "");
    r.check("its name comes from the table", games::name_for(&cs2, &[]) == "Counter-Strike 2", &games::name_for(&cs2, &[]));

    let tool = wi(5, 14, r"C:\Tools\MyEmulator\emu.exe");
    r.check("a program that is no game is not found by itself", chosen(&[tool.clone()], &base).is_none(), "");
    r.check("a hand pick wins over a known game", chosen(&[cs2.clone(), tool.clone()], &pick("emu.exe")).as_deref() == Some("emu.exe"), "");
    let small_pick = WinInfo { width: 200, height: 150, ..tool.clone() };
    r.check("a hand pick is taken at any size", chosen(&[small_pick], &pick("emu.exe")).as_deref() == Some("emu.exe"), "");

    let not = ["cs2.exe".to_string()];
    let fs_cs2 = WinInfo { fullscreen: true, foreground: true, ..cs2.clone() };
    let m = Marks { not_games: &not, ..Marks::none() };
    r.check("a not-a-game mark holds, even fullscreen in front", chosen(&[fs_cs2.clone()], &m).is_none(), "");
    let m = Marks { not_games: &not, pick: "cs2.exe", ..Marks::none() };
    r.check("a not-a-game mark beats an old pick", chosen(&[fs_cs2.clone()], &m).is_none(), "");

    let off = Marks { auto: false, ..Marks::none() };
    r.check("detection off: no pick, no game", chosen(&[fs_cs2.clone()], &off).is_none(), "");
    let off_pick = Marks { auto: false, pick: "emu.exe", ..Marks::none() };
    r.check("detection off: the pick only", chosen(&[fs_cs2.clone(), tool.clone()], &off_pick).as_deref() == Some("emu.exe"), "");

    let marked = ["emu.exe".to_string()];
    let m = Marks { games: &marked, ..Marks::none() };
    r.check("a program marked as a game is one", chosen(&[tool.clone()], &m).as_deref() == Some("emu.exe"), "");

    let hades = wi(6, 15, r"C:\Program Files (x86)\Steam\steamapps\common\Hades\x64\Hades.exe");
    let crash = wi(7, 16, r"C:\Program Files (x86)\Steam\steamapps\common\Hades\x64\CrashReporter.exe");
    let launcher = wi(8, 17, r"C:\Games\Foo\Launcher\Foo.exe");
    r.check("a store folder's game is found, its helpers are not", chosen(&[crash.clone(), launcher.clone(), hades.clone()], &base).as_deref() == Some("hades.exe") && chosen(&[crash, launcher], &base).is_none(), "");
    r.check("a Steam game is named after its folder", games::name_for(&hades, &[]) == "Hades", &games::name_for(&hades, &[]));
    let tiny = WinInfo { width: 300, height: 120, ..hades.clone() };
    r.check("a small window (a splash or pop-up) is not recorded", chosen(&[tiny], &base).is_none(), "");

    let fs_unknown = WinInfo { fullscreen: true, ..wi(9, 18, r"C:\Indie\Thing\thing.exe") };
    r.check("an unknown program filling the screen is a game", chosen(&[fs_unknown.clone()], &base).as_deref() == Some("thing.exe"), "");

    let seen = [Game { name: "My Indie".into(), exe: r"C:\Indie\Other\other.exe".into(), ..Default::default() }];
    let other = wi(10, 19, r"C:\Indie\Other\other.exe");
    let m = Marks { seen: &seen, ..Marks::none() };
    r.check("a program seen as a game before is one again, under that name", chosen(&[other.clone()], &m).as_deref() == Some("other.exe") && games::name_for(&other, &seen) == "My Indie", "");

    let mc = WinInfo { class: "GLFW30".into(), title: "Minecraft* 1.21.4".into(), ..wi(11, 20, r"C:\Users\me\AppData\Roaming\.minecraft\runtime\java-runtime-delta\bin\javaw.exe") };
    let java_tool = wi(12, 21, r"C:\Program Files\Java\jdk-21\bin\javaw.exe");
    r.check("Minecraft's Java window is found and named, another Java program is not", chosen(&[java_tool.clone(), mc.clone()], &base).as_deref() == Some("javaw.exe") && games::name_for(&mc, &[]) == "Minecraft" && chosen(&[java_tool], &base).is_none(), "");

    // Foreground and the current game.
    let valo = WinInfo { foreground: true, ..wi(13, 22, r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe") };
    let cur = Marks { current: Some(12), ..Marks::none() };
    r.check("a game played in front takes over from one in the background", chosen(&[cs2.clone(), valo.clone()], &cur).as_deref() == Some("valorant-win64-shipping.exe"), "");
    let fs_front = WinInfo { foreground: true, ..fs_unknown.clone() };
    r.check("the current game is kept over a fullscreen unknown in front", chosen(&[cs2.clone(), fs_front], &cur).as_deref() == Some("cs2.exe"), "");
    let mini = WinInfo { minimized: true, width: 0, height: 0, ..cs2.clone() };
    r.check("a minimized current game stays the game while nothing else is", chosen(&[mini.clone(), notepad.clone()], &cur).as_deref() == Some("cs2.exe"), "");
    r.check("two windows of one game: the larger one", {
        let small = WinInfo { hwnd: 30, width: 640, height: 480, ..cs2.clone() };
        games::choose(&[small, cs2.clone()], &base).is_some_and(|i| i == 1)
    }, "");

    // The menu.
    let wins = [cs2.clone(), WinInfo { hwnd: 31, ..cs2.clone() }, notepad.clone(), wi(40, 50, r"C:\Windows\explorer.exe"), tool.clone()];
    let m = Marks { current: Some(12), not_games: &none, ..Marks::none() };
    let rows = menu_rows(&wins, &m, Some(12));
    r.check("menu: one row per program, the current game first, the shell left out", rows.len() == 3 && rows[0].exe == "cs2.exe" && rows[0].current && rows[0].game && !rows.iter().any(|x| x.exe == "explorer.exe"), &format!("{:?}", rows.iter().map(|x| x.exe.as_str()).collect::<Vec<_>>()));
    let m = Marks { pick: "emu.exe", ..Marks::none() };
    let rows = menu_rows(&wins, &m, None);
    r.check("menu: the hand pick is marked and listed first", rows.first().is_some_and(|x| x.exe == "emu.exe" && x.picked && x.game), "");
}

fn test_targets(r: &mut Report) {
    let mons = [mon(0, 2560, 1440, 144), mon(1, 1920, 1080, 60)];
    let base = Marks::none();
    let alive = |_: u32| true;
    let dead = |_: u32| false;
    let g = WinInfo { foreground: true, ..wi(100, 7, r"C:\Riot Games\VALORANT\live\VALORANT.exe") };
    let mut pending = None;
    let t1 = next_target(None, std::slice::from_ref(&g), &base, &mut pending, &alive, &mons);
    r.check("target: the game's window, its size and monitor", t1.as_ref().is_some_and(|t| t.hwnd == 100 && t.pid == 7 && t.width == 1920 && t.height == 1080 && t.name == "Valorant"), &format!("{t1:?}"));
    let resized = WinInfo { width: 1280, height: 720, ..g.clone() };
    let t2 = next_target(t1.as_ref(), std::slice::from_ref(&resized), &base, &mut pending, &alive, &mons);
    r.check("target: a new size waits for a second look", t2.as_ref().is_some_and(|t| (t.width, t.height) == (1920, 1080)) && pending == Some((100, 1280, 720)), "");
    let t3 = next_target(t2.as_ref(), std::slice::from_ref(&resized), &base, &mut pending, &alive, &mons);
    r.check("target: a size that held is taken", t3.as_ref().is_some_and(|t| (t.width, t.height) == (1280, 720)) && pending.is_none(), "");
    let mini = WinInfo { minimized: true, width: 0, height: 0, ..g.clone() };
    let t4 = next_target(t3.as_ref(), std::slice::from_ref(&mini), &base, &mut pending, &alive, &mons);
    r.check("target: minimized keeps its size (no restart)", t4 == t3, "");
    let fresh_mini = WinInfo { monitor: 1, ..mini.clone() };
    let t5 = next_target(None, std::slice::from_ref(&fresh_mini), &base, &mut pending, &alive, &mons);
    r.check("target: a game found minimized takes its monitor's size", t5.as_ref().is_some_and(|t| (t.width, t.height, t.monitor) == (1920, 1080, 1)), &format!("{t5:?}"));
    let t6 = next_target(t3.as_ref(), &[], &base, &mut pending, &alive, &mons);
    r.check("target: its window gone for a moment while it runs: kept", t6 == t3, "");
    let t7 = next_target(t3.as_ref(), &[], &base, &mut pending, &dead, &mons);
    r.check("target: its process ended: no game", t7.is_none(), "");
    let not = ["valorant.exe".to_string()];
    let t8 = next_target(t3.as_ref(), &[], &Marks { not_games: &not, ..Marks::none() }, &mut pending, &alive, &mons);
    r.check("target: marked not a game while its window is away: dropped", t8.is_none(), "");

    // The capture for a target.
    let s = Settings { capture_mode: "game".into(), ..Default::default() };
    let t = Target { hwnd: 0x2a0b4, pid: 7, exe: "x.exe".into(), name: "X".into(), monitor: 1, width: 1919, height: 1079 };
    let c = cfg_for(&s, &t, &mons[1], true);
    r.check("cfg: window capture at even numbers, no desktop sound, the game's monitor", c.window == 0x2a0b4 && (c.width, c.height) == (1918, 1078) && !c.scaled && !c.desktop_audio && c.mic && c.monitor == 1 && c.hz == 60, &format!("{}x{} scaled {} monitor {}", c.width, c.height, c.scaled, c.monitor));
    let f = cfg_for(&s, &t, &mons[1], false);
    let desk = core::capture_cfg_for(&s, &mons[1], s.max_clip_secs());
    r.check("cfg: the fallback is the monitor as desktop mode records it, without desktop sound", f.window == 0 && f.width == desk.width && f.height == desk.height && !f.desktop_audio && CaptureCfg { desktop_audio: true, ..f.clone() } == desk, "");
    let capped = cfg_for(&Settings { height: 720, ..s.clone() }, &Target { width: 2560, height: 1440, ..t.clone() }, &mons[0], true);
    r.check("cfg: the height setting caps a window", (capped.width, capped.height) == (1280, 720) && capped.scaled, &format!("{}x{}", capped.width, capped.height));
    r.check("window_size: a height at or over the window's is no cap", window_size(1600, 900, 1080) == (1600, 900, false) && window_size(1600, 900, 900) == (1600, 900, false), "");
    let mut long = s.clone();
    long.keybinds.push(crate::settings::Keybind { seconds: 120, monitor: 1, ..Default::default() });
    r.check("cfg: the buffer holds the longest clip keybind, whatever monitor it names", cfg_for(&long, &t, &mons[1], true).max_secs == 125.0, "");

    // Status.
    let d = Settings::default();
    r.check("status: desktop mode", status_of(&d, true, &[]) == ("desktop".into(), String::new()), "");
    r.check("status: game mode, nothing to capture: waiting", status_of(&s, true, &[]) == ("game".into(), "waiting".into()), "");
    r.check("status: game mode paused: no target state", status_of(&s, false, &[]).1.is_empty(), "");
    let cap = |window: u64| CapStats { id: 1, monitor: 0, window, primary: true, status: Default::default(), frames: 0, video_bytes: 0, video_disk_bytes: 0, buffer_secs: 0.0, max_secs: 35.0 };
    r.check("status: the window, or the whole screen", status_of(&s, true, &[cap(5)]).1 == "window" && status_of(&s, true, &[cap(0)]).1 == "screen", "");

    // A window capture that never gives a picture.
    let t0 = std::time::Instant::now();
    let later = |s: u64| t0 + std::time::Duration::from_secs(s);
    let mut silent = None;
    let w = cap(5);
    let a = never_delivered(&mut silent, &w, false, t0);
    let b = never_delivered(&mut silent, &w, false, later(FIRST_FRAME_SECS - 1));
    let c = never_delivered(&mut silent, &w, false, later(FIRST_FRAME_SECS));
    r.check("no first frame: failed only after the wait", !a && !b && c, "");
    let mut silent = None;
    let _ = never_delivered(&mut silent, &w, false, t0);
    let mini = never_delivered(&mut silent, &w, true, later(60));
    let after = never_delivered(&mut silent, &w, false, later(61));
    let framed = never_delivered(&mut silent, &CapStats { frames: 3, ..cap(5) }, false, later(200));
    r.check("no first frame: minimized or delivering never counts, and the wait starts again", !mini && !after && !framed, "");
}

fn test_args(r: &mut Report) {
    use crate::recorder::capture_args;
    let base = CaptureCfg {
        ffmpeg: None,
        monitor: 1,
        width: 2560,
        height: 1440,
        scaled: false,
        fps: 60,
        hz: 144,
        quality: "high".into(),
        bitrate: 0,
        encoder: "nvenc".into(),
        method: "auto".into(),
        cursor: true,
        desktop_audio: true,
        mic: true,
        max_secs: 35.0,
        buffer: "ram".into(),
        buffer_ram_secs: 30.0,
        buffer_dir: util::tmp_dir(),
        window: 0,
    };
    let filter = |c: &CaptureCfg, method: &str| -> String {
        let a = capture_args(c, "nvenc", method, false, true);
        a.iter().position(|x| x == "-filter_complex").and_then(|i| a.get(i + 1).cloned()).unwrap_or_default()
    };
    let desk = filter(&base, "wgc");
    r.check("args: desktop mode is the monitor, no size given", desk.starts_with("gfxcapture=monitor_idx=1:max_framerate=") && !desk.contains("hwnd") && !desk.contains(":width="), &desk);
    let win = CaptureCfg { window: 0x50a3c, width: 1918, height: 1078, ..base.clone() };
    let wf = filter(&win, "wgc");
    r.check("args: game mode captures the window by its handle, at its size, fitted whole", wf.starts_with(&format!("gfxcapture=hwnd={}:max_framerate=", 0x50a3c)) && !wf.contains("monitor_idx") && wf.contains(":width=1918:height=1078:resize_mode=scale_aspect:scale_mode=bicubic") && wf.contains(":display_border=0"), &wf);
    let ws = CaptureCfg { scaled: true, width: 1280, height: 720, ..win.clone() };
    let sf = filter(&ws, "wgc");
    r.check("args: a capped window is scaled to the capped size", sf.contains(":width=1280:height=720:resize_mode=scale_aspect"), &sf);
    let ms = filter(&CaptureCfg { scaled: true, width: 1920, height: 1080, ..base.clone() }, "wgc");
    r.check("args: a scaled monitor is as before", ms.contains("gfxcapture=monitor_idx=1:") && ms.contains(":width=1920:height=1080:resize_mode=scale:scale_mode=bicubic"), &ms);
    r.check("args: the frame thinning stays (drop-only select)", wf.contains(",select='if(gte(t,ld(0))"), "");
    let same_rest = |a: &[String], b: &[String]| a.len() == b.len() && a.iter().zip(b).filter(|(x, y)| x != y).count() == 1;
    r.check("args: only the source differs between window and monitor", same_rest(&capture_args(&base, "nvenc", "wgc", false, true), &capture_args(&CaptureCfg { window: 7, ..base.clone() }, "nvenc", "wgc", false, true)), "");
    let fb = filter(&CaptureCfg { window: 0, ..win.clone() }, "wgc");
    r.check("args: the fallback (window 0) is the monitor capture", fb.starts_with("gfxcapture=monitor_idx=1:"), &fb);
    let dda = filter(&base, "dda");
    r.check("args: Desktop Duplication is unchanged", dda.starts_with("ddagrab=output_idx=1:framerate=60"), &dda);
}

fn test_recorder(r: &mut Report) {
    use crate::recorder::Recorder;
    // No ffmpeg: each pipeline's supervisor stops at once with an error. Nothing is captured.
    let dir = util::data_dir().join("capmode-test").join("rec");
    let cfg = |window: u64, w: u32, h: u32| CaptureCfg {
        ffmpeg: None,
        monitor: 0,
        width: w,
        height: h,
        scaled: false,
        fps: 60,
        hz: 60,
        quality: "high".into(),
        bitrate: 0,
        encoder: "x264".into(),
        method: "wgc".into(),
        cursor: true,
        desktop_audio: false,
        mic: false,
        max_secs: 35.0,
        buffer: "ram".into(),
        buffer_ram_secs: 30.0,
        buffer_dir: dir.clone(),
        window,
    };
    let rec = Recorder::new();
    let ids = |rec: &Recorder| rec.stats().captures.iter().map(|c| (c.id, c.window)).collect::<Vec<_>>();
    rec.start(Vec::new());
    r.check("recorder: no game: clipping stays on and waits with nothing to capture", rec.is_running() && rec.current_cfgs().is_empty() && rec.stats().captures.is_empty(), "");
    rec.start(vec![cfg(0x10, 1920, 1080)]);
    let a = ids(&rec);
    r.check("recorder: a game appears: its window is captured", a.len() == 1 && a[0].1 == 0x10 && rec.current_cfgs() == vec![cfg(0x10, 1920, 1080)], &format!("{a:?}"));
    rec.start(vec![cfg(0x20, 1920, 1080)]);
    let b = ids(&rec);
    r.check("recorder: another game's window of the same size: same buffer, new window", b.len() == 1 && b[0].0 == a[0].0 && b[0].1 == 0x20 && rec.current_cfgs() == vec![cfg(0x20, 1920, 1080)], &format!("{a:?} -> {b:?}"));
    rec.start(vec![cfg(0x20, 1920, 1080)]);
    r.check("recorder: the same window again changes nothing", ids(&rec) == b, "");
    rec.start(vec![cfg(0x30, 1280, 720)]);
    let c = ids(&rec);
    r.check("recorder: another size starts a new capture", c.len() == 1 && c[0].0 != b[0].0 && c[0].1 == 0x30, &format!("{c:?}"));
    rec.start(vec![cfg(0, 1280, 720)]);
    let d = ids(&rec);
    r.check("recorder: window to monitor (the fallback) is a new capture", d.len() == 1 && d[0].0 != c[0].0 && d[0].1 == 0, &format!("{d:?}"));
    rec.start(vec![cfg(0x40, 1280, 720)]);
    r.check("recorder: a monitor capture never turns into a window capture in place", ids(&rec)[0].0 != d[0].0, "");
    rec.start(Vec::new());
    r.check("recorder: the game exits: the capture stops, clipping waits", rec.is_running() && rec.current_cfgs().is_empty(), "");
    let snap = rec.snapshot(5.0, None).err().unwrap_or_default();
    r.check("recorder: a clip while waiting has nothing to save", !snap.is_empty(), &snap);
    rec.stop();
    r.check("recorder: pause stops waiting too", !rec.is_running(), "");
    // Desktop mode's capture list is what it was.
    let s = Settings::default();
    let cfgs = core::capture_cfgs(&s);
    r.check("desktop mode: monitors with desktop sound, no window", !cfgs.is_empty() && cfgs.iter().all(|c| c.window == 0 && c.desktop_audio == s.desktop_audio), "");
    let g = Settings { capture_mode: "game".into(), ..Default::default() };
    state().lock().unwrap().target = None;
    r.check("game mode with no game: nothing to capture", core::capture_cfgs(&g).is_empty(), "");
    let mons = win::monitors();
    state().lock().unwrap().target = Some(Target { hwnd: 0x99, pid: 1, exe: "x.exe".into(), name: "X".into(), monitor: 0, width: 1600, height: 900 });
    let gc = core::capture_cfgs(&g);
    let wok = window_capture_ok(&g);
    r.check("game mode with a game: one capture, its window when this PC can capture one", gc.len() == 1 && !gc[0].desktop_audio && (if wok { gc[0].window == 0x99 && (gc[0].width, gc[0].height) == (1600, 900) } else { gc[0].window == 0 }), &format!("window capture {}, {} monitors", if wok { "available" } else { "not available" }, mons.len()));
    state().lock().unwrap().failed.push(0x99);
    r.check("game mode: a window that failed twice is recorded as its monitor", core::capture_cfgs(&g).first().is_some_and(|c| c.window == 0), "");
    let caps = vec![CapStats { id: 1, monitor: 0, window: 0, primary: true, status: Default::default(), frames: 0, video_bytes: 0, video_disk_bytes: 0, buffer_secs: 0.0, max_secs: 35.0 }];
    r.check("status: that is told apart from no window capture", status_of(&g, true, &caps).1 == "screen_failed", "");
    *state().lock().unwrap() = State::default();
    let _ = std::fs::remove_dir_all(util::data_dir().join("capmode-test"));
}

/// Mean level in dB of audio stream `stream` of `file` around `freq` Hz.
fn band(ffmpeg: &std::path::Path, file: &std::path::Path, stream: u32, freq: u32) -> f64 {
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{stream}"), "-af", &format!("bandpass=f={freq}:width_type=q:w=8,bandpass=f={freq}:width_type=q:w=8,volumedetect"), "-f", "null", "-"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return -999.0 };
    String::from_utf8_lossy(&out.stderr).lines().find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok())).unwrap_or(-999.0)
}

fn tone(freq: f64, secs: f64) -> Vec<i16> {
    let n = (secs * 48_000.0) as usize;
    (0..n).flat_map(|i| {
        let v = ((i as f64 / 48_000.0) * freq * std::f64::consts::TAU).sin() * 0.3 * 32767.0;
        [v as i16, v as i16]
    }).collect()
}

fn test_clips(r: &mut Report) {
    use crate::audio::{Pcm, Plan};
    use crate::clip::{self, audio_titles};
    use crate::procaudio::{GameAudio, RingCfg};
    use crate::recorder::Snapshot;
    use std::path::Path;
    let dir = util::data_dir().join("capmode-test").join("clips");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    // The video part of a snapshot: 4 s of a synthetic picture as MPEG-TS, as the capture pipe gives it.
    let ts = dir.join("video.ts");
    let made = util::hidden_cmd(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=4", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p", "-f", "mpegts"])
        .arg(&ts)
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    let header = std::fs::read(&ts).unwrap_or_default();
    r.check("test video made", made && !header.is_empty(), "");
    if header.is_empty() {
        return;
    }
    let s = Settings { clips_dir: dir.to_string_lossy().into_owned(), game_folders: false, mic_denoise: false, capture_mode: "game".into(), ..Default::default() };
    // The game's ring (500 Hz), the microphone (3000 Hz) and desktop sound (1000 Hz) that must NOT be in the clip.
    let ga = GameAudio::new();
    ga.sync(None, RingCfg { max_secs: 1e9, window: None, dir: dir.clone() });
    ga.feed(&tone(500.0, 4.0), 100.0);
    let empty = GameAudio::new();
    let segs = [(100.0, 104.0)];
    let write = |game: &GameAudio, mic: bool, label: &str| {
        let cap = clip::Capture {
            snap: Snapshot { header: header.clone(), gops: Vec::new(), segments: segs.to_vec(), duration: 4.0 },
            desk: Some(Pcm { rate: 48_000, data: tone(1000.0, 4.0) }),
            mic: mic.then(|| Pcm { rate: 48_000, data: tone(3000.0, 4.0) }),
            game: game.plan(&segs).map(Plan::read),
            width: 320,
            height: 180,
            fps: 30,
        };
        clip::write_game(&s, cap, label)
    };
    let mut full = None;
    for (label, game, mic, want) in [("game + mic", &ga, true, vec!["Mix", "Game", "Microphone"]), ("game only", &ga, false, vec!["Game"]), ("mic only", &empty, true, vec!["Microphone"])] {
        match write(game, mic, label) {
            Ok(m) => {
                let got = audio_titles(&ffmpeg, Path::new(&m.path));
                r.check(&format!("game clip {label}: tracks {want:?}"), got == want && m.audio_tracks as usize == want.len(), &format!("{got:?}"));
                if label == "game + mic" {
                    full = Some(m);
                }
            }
            Err(e) => r.check(&format!("game clip {label}"), false, &e),
        }
    }
    let Some(full) = full else { return };
    let p = Path::new(&full.path);
    let lv = |stream: u32| (band(&ffmpeg, p, stream, 500), band(&ffmpeg, p, stream, 1000), band(&ffmpeg, p, stream, 3000));
    let (m5, m1, m3) = lv(0);
    r.check("game clip: the Mix is game + mic, nothing of the desktop", m5 - m1 > 20.0 && m3 - m1 > 20.0, &format!("500 {m5:.1}, 1000 {m1:.1}, 3000 {m3:.1} dB"));
    let (g5, g1, g3) = lv(1);
    r.check("game clip: track 2 is the game alone", g5 - g1 > 20.0 && g5 - g3 > 20.0, &format!("500 {g5:.1}, 1000 {g1:.1}, 3000 {g3:.1} dB"));
    let (k5, k1, k3) = lv(2);
    r.check("game clip: track 3 is the microphone alone", k3 - k5 > 20.0 && k3 - k1 > 20.0, &format!("500 {k5:.1}, 1000 {k1:.1}, 3000 {k3:.1} dB"));
    match crate::editor::audio_tracks(&s, &full) {
        Ok(t) => {
            let got: Vec<&str> = t.iter().map(|x| x.title.as_str()).collect();
            r.check("editor: the game clip's rows are Mix, Game, Microphone", got == ["Mix", "Game", "Microphone"], &format!("{got:?}"));
        }
        Err(e) => r.check("editor: the game clip's rows", false, &e),
    }
    match clip::trim(&s, &full, 0.0, 4.0, false) {
        Ok(m) => {
            let got = audio_titles(&ffmpeg, Path::new(&m.path));
            r.check("trim without the microphone keeps the game", got == ["Game"], &format!("{got:?}"));
        }
        Err(e) => r.check("trim without the microphone", false, &e),
    }
    // Desktop mode's layout is untouched by all this.
    let cap = clip::Capture {
        snap: Snapshot { header: header.clone(), gops: Vec::new(), segments: segs.to_vec(), duration: 4.0 },
        desk: Some(Pcm { rate: 48_000, data: tone(1000.0, 4.0) }),
        mic: Some(Pcm { rate: 48_000, data: tone(3000.0, 4.0) }),
        game: None,
        width: 320,
        height: 180,
        fps: 30,
    };
    match clip::write(&Settings { capture_mode: "desktop".into(), ..s.clone() }, cap, "desktop") {
        Ok(m) => {
            let got = audio_titles(&ffmpeg, Path::new(&m.path));
            r.check("desktop clip: still Mix, Desktop, Microphone", got == ["Mix", "Desktop", "Microphone"], &format!("{got:?}"));
        }
        Err(e) => r.check("desktop clip", false, &e),
    }
    ga.off();
    test_waiting(r, &ffmpeg, &header, &s);
    let _ = std::fs::remove_dir_all(util::data_dir().join("capmode-test"));
}

/// The game exits: the buffer is kept, a clip while waiting saves what it holds (the lead's call).
fn test_waiting(r: &mut Report, ffmpeg: &std::path::Path, ts: &[u8], s: &Settings) {
    use crate::audio::Plan;
    use crate::clip::{self, audio_titles};
    use crate::procaudio::{GameAudio, RingCfg};
    use crate::recorder::Recorder;
    use std::path::Path;
    let dir = util::data_dir().join("capmode-test").join("waiting");
    let _ = std::fs::create_dir_all(&dir);
    let cfg = |window: u64, w: u32, h: u32| CaptureCfg {
        ffmpeg: None, // no ffmpeg: nothing is ever captured; the buffer is fed by `test_feed`
        monitor: 0,
        width: w,
        height: h,
        scaled: false,
        fps: 30,
        hz: 60,
        quality: "high".into(),
        bitrate: 0,
        encoder: "x264".into(),
        method: "wgc".into(),
        cursor: true,
        desktop_audio: false,
        mic: false,
        max_secs: 35.0,
        buffer: "ram".into(),
        buffer_ram_secs: 30.0,
        buffer_dir: dir.clone(),
        window,
    };
    let rec = Recorder::new();
    rec.start(vec![cfg(0x10, 320, 180)]);
    let id0 = rec.stats().captures.first().map(|c| c.id);
    let end = util::now_secs() - 0.5;
    rec.test_feed(ts, end, &tone(3000.0, 4.0));
    // The game's own sound for the same 4 s.
    let ga = GameAudio::new();
    ga.sync(None, RingCfg { max_secs: 1e9, window: None, dir: dir.clone() });
    ga.feed(&tone(500.0, 4.0), end - 4.0);
    let before = rec.stats().video_bytes;
    rec.start(Vec::new()); // the game exits
    let st = rec.stats();
    r.check("game exits: clipping waits, nothing is captured, the buffer is kept", rec.is_running() && rec.current_cfgs().is_empty() && st.captures.is_empty() && rec.buffered_cfgs().len() == 1 && before > 0 && st.video_bytes == before, &format!("{before} -> {} bytes", st.video_bytes));
    r.check("game exits: the status says waiting", status_of(&Settings { capture_mode: "game".into(), ..Default::default() }, true, &st.captures).1 == "waiting", "");
    match rec.snapshot(30.0, None) {
        Ok((snap, desk, mic, c)) => {
            r.check("clip while waiting: the buffer still holds the end of the game", enough_to_clip(&snap.segments, 30.0) && (snap.duration - 4.0).abs() < 0.6 && c.window == 0x10, &format!("{:.2} s", snap.duration));
            let game = ga.plan(&snap.segments).map(Plan::read);
            let cap = clip::Capture { snap, desk, mic, game, width: 320, height: 180, fps: 30 };
            match clip::write_game(s, cap, "Last game") {
                Ok(m) => {
                    let got = audio_titles(ffmpeg, Path::new(&m.path));
                    let p = Path::new(&m.path);
                    let (g5, g3) = (band(ffmpeg, p, 1, 500), band(ffmpeg, p, 2, 3000));
                    r.check("clip while waiting is written: video, the game's sound and the mic of that time", got == ["Mix", "Game", "Microphone"] && (m.duration - 4.0).abs() < 0.6 && g5 > -30.0 && g3 > -30.0, &format!("{got:?} {:.2} s, game {g5:.1} dB, mic {g3:.1} dB", m.duration));
                }
                Err(e) => r.check("clip while waiting is written", false, &e),
            }
        }
        Err(e) => r.check("clip while waiting: the buffer still holds the end of the game", false, &e),
    }
    let short = rec.snapshot(0.5, None);
    r.check("clip while waiting: under a second of the asked range is not enough (toast path)", !short.as_ref().is_ok_and(|v| enough_to_clip(&v.0.segments, 0.5)), &format!("{:?}", short.as_ref().map(|v| v.0.duration).err()));
    rec.start(vec![cfg(0x10, 320, 180)]);
    r.check("the game comes back: the same capture resumes on its buffer", rec.stats().captures.first().map(|c| c.id) == id0 && rec.current_cfgs().len() == 1 && rec.snapshot(30.0, None).is_ok_and(|v| v.0.duration > 3.0), "");
    rec.start(Vec::new());
    rec.start(vec![cfg(0x20, 1280, 720)]);
    r.check("another game of another size: a new capture, the old buffer freed", rec.stats().captures.first().map(|c| c.id) != id0 && rec.buffered_cfgs().len() == 1 && rec.snapshot(30.0, None).is_err(), "");
    rec.stop();
    // A buffer that holds nothing of the asked range: the toast path.
    let old = Recorder::new();
    old.start(vec![cfg(0x30, 320, 180)]);
    old.test_feed(ts, util::now_secs() - 300.0, &[]);
    old.start(Vec::new());
    let res = old.snapshot(30.0, None);
    r.check("clip while waiting with nothing in the asked range: the toast path", old.buffered_cfgs().len() == 1 && !res.as_ref().is_ok_and(|v| enough_to_clip(&v.0.segments, 30.0)), &format!("{:?}", res.as_ref().map(|v| v.0.duration).err()));
    old.stop();
    r.check("pause frees a kept buffer", !old.is_running() && old.buffered_cfgs().is_empty(), "");
    ga.off();
}
