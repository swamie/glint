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
//!
//! 0.1.41: the decision is `gamedetect::decide` (one pure function, see that file): layered
//! evidence of what is a game, the pick that always wins, and a stated source. Exclusive
//! fullscreen is recorded from its monitor by Desktop Duplication (Graphics Capture cannot see
//! it); a monitor source pauses while the game is minimized or not in front; every change is
//! held two looks and logged once with its reason (`State::why`, `Status::capture_why`).
use crate::core::{self, Core};
use crate::gamedetect::{self, Source, Why};
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

/// A clip of game mode without desktop sound: `clip::write_game` (Mix of game + microphone,
/// Game, Microphone). With desktop sound asked for (0.1.33) the clip takes `clip::write`: the
/// usual tracks, and track 1 holds the game too when the desktop sound lacks it (0.1.35).
pub fn game_only(s: &Settings) -> bool {
    game_mode(s) && !s.game_desktop_audio
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
    silent: Option<(u64, u64, u32, std::time::Instant)>,
    /// The last game recorded: a clip of the buffer it left after it exited is filed under it.
    last: Option<String>,
    /// 0.1.41: what `gamedetect::decide` keeps between looks, its last source and reason, and the
    /// log line written for it (one line per change).
    mem: gamedetect::Memory,
    source: Option<Source>,
    why: Why,
    logged: String,
    /// Every program the last look counted as a game, with its evidence (for `--detect-watch`).
    cands: Vec<gamedetect::Cand>,
}

/// What the last look decided, for the test tool `--detect-watch` (detectwatch.rs).
#[derive(Clone, Debug, Default)]
pub struct WatchView {
    pub why: Why,
    pub source: Option<Source>,
    pub target: Option<Target>,
    pub candidates: Vec<gamedetect::Cand>,
    /// Windows whose capture gave no picture, and windows seen in exclusive fullscreen.
    pub failed: Vec<u64>,
    pub exclusive: Vec<u64>,
}

pub fn watch_view() -> WatchView {
    let st = state().lock().unwrap();
    let mut failed = st.mem.failed.clone();
    failed.extend(st.failed.iter().copied().filter(|h| !st.mem.failed.contains(h)));
    WatchView { why: st.why.clone(), source: st.source, target: st.target.clone(), candidates: st.cands.clone(), failed, exclusive: st.mem.exclusive.clone() }
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
///
/// The wait is per ffmpeg run (`RecStatus::pid`), not per capture: the recorder restarts ffmpeg when
/// the encoder refuses the capture's frames (AMD's AMF on integrated Radeon graphics: two failed runs,
/// then frames through system memory). Counted across those runs, the wait ran out about a second
/// after the encoder was fixed, on a window that was fine, and GAME mode recorded the whole monitor
/// for the rest of the window's life (seen with tModLoader on a Radeon 840M, 0.1.44). Between runs
/// (no ffmpeg) nothing is counted; a window ffmpeg itself cannot capture is the recorder's
/// `window_failed`.
fn never_delivered(silent: &mut Option<(u64, u64, u32, std::time::Instant)>, c: &CapStats, minimized: bool, now: std::time::Instant) -> bool {
    if c.window == 0 || c.frames > 0 || minimized {
        *silent = None;
        return false;
    }
    if c.status.pid == 0 {
        return false;
    }
    match silent {
        Some((id, w, pid, since)) if *id == c.id && *w == c.window && *pid == c.status.pid => now.duration_since(*since).as_secs() >= FIRST_FRAME_SECS,
        _ => {
            *silent = Some((c.id, c.window, c.status.pid, now));
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
/// without desktop sound unless the user asked for it in game mode too (`game_desktop_audio`:
/// the game's picture with everything that is heard), and with a buffer for the longest clip
/// keybind (all clip keys take the game).
pub fn cfg_for(s: &Settings, t: &Target, m: &Monitor, window: bool) -> CaptureCfg {
    let mut c = core::capture_cfg_for(s, m, s.max_clip_secs());
    c.desktop_audio = s.game_desktop_audio;
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
    let (target, failed, source) = {
        let st = state().lock().unwrap();
        let failed = st.target.as_ref().is_some_and(|t| st.failed.contains(&t.hwnd) || st.mem.failed.contains(&t.hwnd));
        (st.target.clone(), failed, st.source)
    };
    let Some(t) = target else { return Vec::new() };
    let mons = win::monitors();
    let Some(m) = mons.get(t.monitor as usize).or(mons.first()) else { return Vec::new() };
    vec![match source {
        // Paused (minimized behind a monitor source, hidden, blocked) or no game: nothing runs, the buffer stays.
        Some(Source::Park) | Some(Source::Wait) => return Vec::new(),
        Some(Source::MonitorDda) => cfg_dda(s, &t, m),
        Some(Source::Monitor) => cfg_for(s, &t, m, false),
        Some(Source::Window) | None => cfg_for(s, &t, m, !failed && window_capture_ok(s)),
    }]
}

/// The game's monitor by Desktop Duplication (`ddagrab`): what an exclusive-fullscreen game is
/// recorded with, since Graphics Capture sees neither its window nor its monitor's picture of it.
pub fn cfg_dda(s: &Settings, t: &Target, m: &Monitor) -> CaptureCfg {
    let mut c = cfg_for(s, t, m, false);
    c.method = "dda".into();
    c
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
    // The name is looked up once per game (it may read the program's version info).
    let name = prev.filter(|p| p.pid == w.pid).map(|p| p.name.clone()).unwrap_or_else(|| games::name_for(w, m.seen));
    Some(settle(prev, w, name, pending, mons))
}

/// The target for window `w` (named `name`), with the size rule: a new size of the same window is
/// taken once it held for a second look; minimized keeps the size it had (else its monitor's).
pub fn settle(prev: Option<&Target>, w: &WinInfo, name: String, pending: &mut Option<(u64, u32, u32)>, mons: &[Monitor]) -> Target {
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
    Target { hwnd: w.hwnd, pid: w.pid, exe: w.exe.clone(), name, monitor: w.monitor, width, height }
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
    let lib = gamedetect::library(); // the cached index; a refresh runs in the background
    if let Some(fg) = wins.iter().find(|w| w.foreground) {
        gamedetect::library_hint(&fg.exe, &lib);
    }
    let method = util::find_exe(&s.ffmpeg_path, "ffmpeg").map(|f| crate::recorder::resolve_method(&f, &s.method));
    let env = gamedetect::Env { window_capture: method.as_deref() == Some("wgc"), dda: method.as_deref() == Some("dda"), d3d_fullscreen: win::d3d_fullscreen(), now: clock() };
    let mut st = state().lock().unwrap();
    for c in caps.iter().filter(|c| c.window != 0 && c.status.window_failed) {
        if !st.mem.failed.contains(&c.window) {
            util::log(&format!("game mode: window {:#x} could not be captured", c.window));
            st.mem.failed.push(c.window);
        }
    }
    if let Some(c) = caps.first() {
        let minimized = wins.iter().find(|w| w.hwnd == c.window).map_or(true, |w| w.minimized);
        let mut silent = st.silent.take();
        if never_delivered(&mut silent, c, minimized, std::time::Instant::now()) && !st.mem.failed.contains(&c.window) {
            util::log(&format!("game mode: window {:#x} gave no picture in {FIRST_FRAME_SECS} s", c.window));
            st.mem.failed.push(c.window);
            silent = None;
        }
        st.silent = silent;
    }
    st.failed.retain(|h| wins.iter().any(|w| w.hwnd == *h));
    let prefs = gamedetect::Prefs { auto: s.game_auto, pick: &s.game_pick, pick_path: &s.game_pick_path, games: &s.game_exes, not_games: &s.not_game_exes, seen: &seen };
    let mut mem = std::mem::take(&mut st.mem);
    let d = gamedetect::decide(&wins, &lib, &prefs, &env, &mut mem, &win::process_alive);
    st.mem = mem;
    let prev = st.target.clone();
    let next = match d.window {
        Some(i) => {
            let mut pending = st.pending.take();
            let t = settle(prev.as_ref(), &wins[i], d.why.game.clone(), &mut pending, &mons);
            st.pending = pending;
            Some(t)
        }
        // Waiting for its window (re-created) or paused: the target stays (clip names, its sound).
        None if d.source == Source::Park => prev.clone(),
        None => None,
    };
    // One log line per change of what is decided (never per look).
    let key = format!("{}|{}|{:?}|{:?}", d.why.code, d.why.game, d.source, next.as_ref().map(|t| t.hwnd));
    if key != st.logged {
        let detail = match &next {
            Some(t) => format!(" ({}, window {:#x}, {}x{} on monitor {}, by {})", games::exe_key(&t.exe), t.hwnd, t.width, t.height, t.monitor + 1, source_name(d.source, method.as_deref().unwrap_or("?"))),
            None => String::new(),
        };
        util::log(&format!("game mode: {}{detail}", d.why.line()));
        st.logged = key;
    }
    st.why = d.why;
    st.cands = d.candidates;
    st.source = Some(d.source);
    if let Some(t) = &next {
        st.last = Some(t.name.clone());
    }
    st.target = next.clone();
    next.map(|t| (t.name, t.pid, t.exe))
}

/// How a source is captured, for the log.
fn source_name(s: Source, method: &str) -> &'static str {
    match s {
        Source::Window => "window capture (Graphics Capture)",
        Source::MonitorDda => "its monitor (Desktop Duplication)",
        Source::Monitor if method == "dda" => "its monitor (Desktop Duplication)",
        Source::Monitor => "its monitor (Graphics Capture)",
        Source::Park => "nothing for now (paused, the buffer kept)",
        Source::Wait => "nothing",
    }
}

/// Seconds since Glint started, on a steady clock (`gamedetect::Env::now`).
fn clock() -> f64 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// The reason of the last decision (`Status::capture_why`): empty in desktop mode or while paused.
pub fn why_of(s: &Settings, running: bool) -> Why {
    if !game_mode(s) || !running {
        return Why::default();
    }
    state().lock().unwrap().why.clone()
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
                if st.target.as_ref().is_some_and(|t| st.failed.contains(&t.hwnd) || st.mem.failed.contains(&t.hwnd) || st.mem.exclusive.contains(&t.hwnd)) { "screen_failed" } else { "screen" }
            }
        }
    };
    ("game".into(), target.into())
}

/// A setting that changes what is recorded in game mode (or the mode itself) changed.
pub fn marks_changed(old: &Settings, new: &Settings) -> bool {
    old.capture_mode != new.capture_mode || old.game_desktop_audio != new.game_desktop_audio || old.game_auto != new.game_auto || old.game_pick != new.game_pick || old.game_pick_path != new.game_pick_path || old.game_exes != new.game_exes || old.not_game_exes != new.not_game_exes
}

/// One choice of the switch's menu: the user's mark `action` on executable `exe`: its file name
/// (`games::exe_key`, as before) or, from 0.1.41, its full path (a pick then keeps the path too).
pub fn apply_mark(s: &mut Settings, action: &str, exe: &str) -> Result<(), String> {
    let raw = exe.trim();
    let path = (raw.contains('\\') || raw.contains('/')).then(|| raw.replace('/', "\\").to_lowercase());
    let key = match &path {
        Some(p) => games::exe_key(p),
        None => raw.to_lowercase(),
    };
    let need = || if key.is_empty() { Err("No program was named".to_string()) } else { Ok(()) };
    match action {
        "auto-on" => s.game_auto = true,
        "auto-off" => s.game_auto = false,
        "pick" => {
            need()?;
            // A pick by name only keeps the path it had when it is the same program.
            if path.is_some() || s.game_pick != key {
                s.game_pick_path = path.clone().unwrap_or_default();
            }
            s.game_pick = key.clone();
            s.not_game_exes.retain(|e| *e != key);
            if !s.game_exes.contains(&key) {
                s.game_exes.push(key);
            }
        }
        "unpick" => {
            s.game_pick.clear();
            s.game_pick_path.clear();
        }
        "not-game" => {
            need()?;
            s.game_exes.retain(|e| *e != key);
            if s.game_pick == key {
                s.game_pick.clear();
                s.game_pick_path.clear();
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
    /// 0.1.41: its full path (a pick keeps it), and why it counts as a game (`gamedetect::Ev`
    /// codes: "pick", "library:steam", "fullscreen", ...), empty when it does not.
    pub path: String,
    pub why: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GameMenu {
    pub mode: String,
    pub auto: bool,
    pub pick: String,
    /// The reason of the last decision (game mode), shown as the menu's "Why?" line.
    pub decision: Why,
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
    menu_rows_with(wins, m, "", &gamedetect::Library::default(), current)
}

/// `menu_rows` with the picked program's path and the installed libraries: each row says why it
/// counts as a game (the same evidence `gamedetect::decide` uses).
pub fn menu_rows_with(wins: &[WinInfo], m: &Marks, pick_path: &str, lib: &gamedetect::Library, current: Option<u32>) -> Vec<MenuRow> {
    let prefs = gamedetect::Prefs { auto: m.auto, pick: m.pick, pick_path, games: m.games, not_games: m.not_games, seen: m.seen };
    let mut rows: Vec<(bool, MenuRow)> = Vec::new();
    for w in wins {
        let exe = games::exe_key(&w.exe);
        if exe.is_empty() || w.cloaked || SHELL.contains(&exe.as_str()) {
            continue;
        }
        let is_current = current == Some(w.pid);
        let ev = gamedetect::evidence(w, &prefs, lib, false);
        let why: Vec<String> = ev.as_ref().filter(|e| gamedetect::points(e) >= gamedetect::GAME_MIN).map(|e| e.iter().map(|x| x.code.clone()).collect()).unwrap_or_default();
        let game = !why.is_empty() || games::rate(w, m).is_some();
        if let Some((_, r)) = rows.iter_mut().find(|(_, r)| r.exe == exe) {
            r.current |= is_current;
            r.game |= game;
            if r.why.is_empty() {
                r.why = why;
            }
            continue;
        }
        let title: String = w.title.trim().chars().take(60).collect();
        rows.push((w.minimized, MenuRow { name: gamedetect::name_of(w, m.seen, lib), exe: exe.clone(), title, game, current: is_current, picked: m.pick == exe, not_game: m.not_games.contains(&exe), path: w.exe.clone(), why }));
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
    let programs = menu_rows_with(&win::windows(), &m, &s.game_pick_path, &gamedetect::library(), current);
    let cur = programs.iter().find(|r| r.current).cloned();
    let decision = if game_mode(&s) { state().lock().unwrap().why.clone() } else { Why { code: "desktop".into(), ..Default::default() } };
    GameMenu { mode: s.capture_mode.clone(), auto: s.game_auto, pick: s.game_pick.clone(), decision, current: cur, programs }
}

/// The menu's "Choose a program…": a file dialog for an .exe, picked by its path. None: cancelled.
#[tauri::command(async)]
pub fn game_pick_exe(core: St) -> Result<Option<Settings>, String> {
    let Some(file) = rfd::FileDialog::new().add_filter("Programs", &["exe"]).pick_file() else { return Ok(None) };
    let mut s = core.settings();
    apply_mark(&mut s, "pick", &file.to_string_lossy())?;
    util::log(&format!("game mode: pick {} (chosen by hand)", s.game_pick));
    core.apply_settings(s);
    core.emit("status-changed", ());
    Ok(Some(core.settings()))
}

/// A choice in that menu (see `apply_mark`). Returns the settings as saved.
#[tauri::command(async)]
pub fn game_mark(core: St, action: String, exe: String) -> Result<Settings, String> {
    let mut s = core.settings();
    apply_mark(&mut s, &action, &exe)?;
    util::log(&format!("game mode: {action} {}", if exe.contains('\\') { games::exe_key(&exe) } else { exe.clone() }));
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
    WinInfo { hwnd, pid, exe: exe.into(), title: String::new(), class: String::new(), fullscreen: false, minimized: false, foreground: false, width: 1920, height: 1080, monitor: 0, ..Default::default() }
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
        test_rebuild(&mut r);
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
    // 0.1.33: the desktop's sound can be recorded in game mode too.
    let both = Settings { game_desktop_audio: true, ..s.clone() };
    r.check("cfg: game mode with desktop sound asked for records it, window and fallback alike", cfg_for(&both, &t, &mons[1], true).desktop_audio && cfg_for(&both, &t, &mons[1], false).desktop_audio && cfg_for(&both, &t, &mons[1], true).window == 0x2a0b4, "");
    r.check("cfg: desktop sound in game mode is off unless asked for, and a mark that needs a new look", !Settings::default().game_desktop_audio && marks_changed(&s, &both), "");
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
    let run = |pid: u32| CapStats { status: crate::recorder::RecStatus { pid, ..Default::default() }, ..cap(5) };
    let w = run(100);
    let a = never_delivered(&mut silent, &w, false, t0);
    let b = never_delivered(&mut silent, &w, false, later(FIRST_FRAME_SECS - 1));
    let c = never_delivered(&mut silent, &w, false, later(FIRST_FRAME_SECS));
    r.check("no first frame: failed only after the wait", !a && !b && c, "");
    let mut silent = None;
    let _ = never_delivered(&mut silent, &w, false, t0);
    let mini = never_delivered(&mut silent, &w, true, later(60));
    let after = never_delivered(&mut silent, &w, false, later(61));
    let framed = never_delivered(&mut silent, &CapStats { frames: 3, ..run(100) }, false, later(200));
    r.check("no first frame: minimized or delivering never counts, and the wait starts again", !mini && !after && !framed, "");
    // Recorded on a Radeon 840M (tModLoader, 0.1.44): AMF refused the GPU frames twice (ffmpeg
    // restarted, nothing between runs) and frames came through system memory from the third run,
    // about 18 s after the start. Counted per capture, the window "gave no picture" at 12 s.
    let mut silent = None;
    let r1 = never_delivered(&mut silent, &run(4101), false, t0);
    let gap = never_delivered(&mut silent, &run(0), false, later(7));
    let r2 = never_delivered(&mut silent, &run(4102), false, later(8));
    let r2b = never_delivered(&mut silent, &run(4102), false, later(14));
    let r3 = never_delivered(&mut silent, &run(4103), false, later(17));
    let r3b = never_delivered(&mut silent, &run(4103), false, later(18));
    r.check("no first frame: the wait is per ffmpeg run, so encoder restarts never fail the window", !r1 && !gap && !r2 && !r2b && !r3 && !r3b, &format!("{r1} {gap} {r2} {r2b} {r3} {r3b}"));
    let mut silent = None;
    let _ = never_delivered(&mut silent, &run(4200), false, t0);
    let still = never_delivered(&mut silent, &run(4200), false, later(FIRST_FRAME_SECS));
    r.check("no first frame: one ffmpeg run that captures nothing for the whole wait still fails the window", still, "");
}

/// 0.1.41: the rebuilt detection (gamedetect.rs) and what capmode.rs does with its decision.
fn test_rebuild(r: &mut Report) {
    let dir = util::data_dir().join("capmode-test").join("libraries");
    gamedetect::self_test(&mut |name, ok, detail| r.check(name, ok, &detail), &dir);
    // Settings saved by 0.1.40 (a pick by file name, no path) load and mean the same.
    let old = r#"{"capture_mode":"game","game_auto":false,"game_pick":"hades.exe","game_exes":["hades.exe","overwatch.exe"],"not_game_exes":["chrome.exe"]}"#;
    let s: Settings = serde_json::from_str(old).unwrap_or_default();
    r.check("0.1.40 settings: mode, pick, marks kept; the new pick path is empty", game_mode(&s) && !s.game_auto && s.game_pick == "hades.exe" && s.game_pick_path.is_empty() && s.game_exes.len() == 2 && s.not_game_exes == ["chrome.exe"], &format!("{} {:?}", s.game_pick, s.game_pick_path));
    let hades = WinInfo { foreground: true, ..wi(7, 70, r"C:\Program Files (x86)\Steam\steamapps\common\Hades\x64\Hades.exe") };
    let prefs = gamedetect::Prefs { auto: s.game_auto, pick: &s.game_pick, pick_path: &s.game_pick_path, games: &s.game_exes, not_games: &s.not_game_exes, seen: &[] };
    let env = gamedetect::Env { window_capture: true, dda: false, d3d_fullscreen: false, now: 0.0 };
    let d = gamedetect::decide(std::slice::from_ref(&hades), &gamedetect::Library::default(), &prefs, &env, &mut gamedetect::Memory::default(), &|_| true);
    r.check("0.1.40 settings: the pick by file name is recorded as before, detection off", d.window == Some(0) && d.source == Source::Window && d.why.game == "Hades", &format!("{:?}", d.why));
    // A pick from the menu or "Choose a program…" keeps the full path; by name it keeps the path of the same program.
    let mut p = Settings::default();
    let _ = apply_mark(&mut p, "pick", r"D:\Games\Foo\Launcher\Foo Launcher.EXE");
    let by_path = p.game_pick == "foo launcher.exe" && p.game_pick_path == r"d:\games\foo\launcher\foo launcher.exe";
    let _ = apply_mark(&mut p, "pick", "foo launcher.exe");
    let kept = p.game_pick_path == r"d:\games\foo\launcher\foo launcher.exe";
    let _ = apply_mark(&mut p, "pick", "other.exe");
    let other = p.game_pick == "other.exe" && p.game_pick_path.is_empty();
    let _ = apply_mark(&mut p, "pick", r"C:\x\y.exe");
    let _ = apply_mark(&mut p, "unpick", "");
    r.check("pick: by path keeps the path; the same name again keeps it; another name drops it; unpick clears both", by_path && kept && other && p.game_pick.is_empty() && p.game_pick_path.is_empty(), &format!("{} {}", p.game_pick, p.game_pick_path));
    let a = Settings::default();
    let b = Settings { game_pick_path: "c:\\x.exe".into(), ..a.clone() };
    r.check("pick: a new pick path is a change that needs a new look", marks_changed(&a, &b), "");
    // Exclusive fullscreen: the game's monitor by Desktop Duplication, all else as the monitor fallback.
    let mons = [mon(0, 2560, 1440, 144), mon(1, 1920, 1080, 60)];
    let s = Settings { capture_mode: "game".into(), ..Default::default() };
    let t = Target { hwnd: 0x77, pid: 7, exe: "x.exe".into(), name: "X".into(), monitor: 1, width: 1920, height: 1080 };
    let dda = cfg_dda(&s, &t, &mons[1]);
    r.check("cfg: exclusive fullscreen records the game's monitor by Desktop Duplication", dda.method == "dda" && dda.window == 0 && dda.monitor == 1 && CaptureCfg { method: s.method.clone(), ..dda.clone() } == cfg_for(&s, &t, &mons[1], false), &format!("{} monitor {}", dda.method, dda.monitor));
    let a = crate::recorder::capture_args(&dda, "nvenc", "dda", false, true);
    r.check("args: that capture is ddagrab of the game's monitor", a.iter().any(|x| x.starts_with("ddagrab=output_idx=1:")), "");
    // The source decided is what `cfgs` hands the recorder.
    {
        let mut st = state().lock().unwrap();
        *st = State::default();
        st.target = Some(t.clone());
        st.source = Some(Source::Park);
    }
    let parked = core::capture_cfgs(&s).is_empty();
    state().lock().unwrap().source = Some(Source::MonitorDda);
    let ex = core::capture_cfgs(&s);
    state().lock().unwrap().source = Some(Source::Monitor);
    let mo = core::capture_cfgs(&s);
    r.check("cfgs: paused gives nothing (the buffer is kept), exclusive gives Desktop Duplication, a refused window its monitor", parked && ex.first().is_some_and(|c| c.method == "dda" && c.window == 0) && mo.first().is_some_and(|c| c.window == 0 && c.method == s.method), "");
    state().lock().unwrap().why = Why { code: "exclusive".into(), game: "X".into(), monitor: 1, evidence: vec!["known".into()] };
    r.check("status: the reason is given in game mode while clipping, not in desktop mode or paused", why_of(&s, true).code == "exclusive" && why_of(&Settings::default(), true).code.is_empty() && why_of(&s, false).code.is_empty(), "");
    *state().lock().unwrap() = State::default();
    // Monitors by device name: a position that does not match no longer means the main monitor.
    let mut m2 = mons.to_vec();
    m2[1].x = 2560;
    r.check("monitor of a window: by its device name first, then by position, else the main one", win::monitor_index_of(&m2, "DISPLAY2", -5, -5) == 1 && win::monitor_index_of(&m2, "", 2560, 0) == 1 && win::monitor_index_of(&m2, "DISPLAY9", 7, 7) == 0, "");
    // The menu says why each program counts.
    let lib = gamedetect::Library { installs: vec![gamedetect::Install { store: "epic", name: "Rocket League".into(), dir: r"d:\epic\rocketleague\".into() }] };
    let rl = wi(9, 90, r"D:\Epic\rocketleague\Binaries\Win64\RocketLeague.exe");
    let tool = wi(10, 91, r"C:\Tools\thing.exe");
    let cloaked = WinInfo { cloaked: true, ..wi(11, 92, r"C:\Games2\hidden.exe") };
    let rows = menu_rows_with(&[rl, tool, cloaked], &Marks::none(), "", &lib, None);
    r.check("menu: each program says why it is a game (known, Epic library), others nothing; hidden windows are left out", rows.len() == 2 && rows[0].why.contains(&"library:epic".to_string()) && rows[0].why.contains(&"known".to_string()) && rows[1].why.is_empty() && !rows[1].game && rows[0].path.ends_with("RocketLeague.exe"), &format!("{:?}", rows.iter().map(|x| (&x.exe, &x.why)).collect::<Vec<_>>()));
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
    // GAME mode with desktop sound (0.1.33) and a desktop output that is silent (the game plays on
    // other headphones): a user's clips had only the microphone on track 1 (0.1.35).
    let both = Settings { game_desktop_audio: true, ..s.clone() };
    r.check("clip path: game mode takes write_game, with desktop sound write", game_only(&s) && !game_only(&both) && !game_only(&Settings::default()), "");
    let cap = clip::Capture {
        snap: Snapshot { header: header.clone(), gops: Vec::new(), segments: segs.to_vec(), duration: 4.0 },
        desk: Some(Pcm { rate: 48_000, data: vec![0; 4 * 48_000 * 2] }),
        mic: Some(Pcm { rate: 48_000, data: tone(3000.0, 4.0) }),
        game: ga.plan(&segs).map(Plan::read),
        width: 320,
        height: 180,
        fps: 30,
    };
    match clip::write(&both, cap, "game with desktop sound") {
        Ok(m) => {
            let p = Path::new(&m.path);
            let got = audio_titles(&ffmpeg, p);
            let (m5, m3) = (band(&ffmpeg, p, 0, 500), band(&ffmpeg, p, 0, 3000));
            r.check("game mode with desktop sound, silent desktop output: track 1 holds the game and the mic", got == ["Mix", "Desktop", "Microphone", "Game"] && m5 > -40.0 && m3 > -40.0, &format!("{got:?}, 500 {m5:.1}, 3000 {m3:.1} dB"));
        }
        Err(e) => r.check("game mode with desktop sound, silent desktop output", false, &e),
    }
    ga.off();
    test_switch(r, &header);
    test_waiting(r, &ffmpeg, &header, &s);
    let _ = std::fs::remove_dir_all(util::data_dir().join("capmode-test"));
}

/// The Desktop switch (or GAME mode's desktop sound) flipped while clipping runs: the picture's
/// buffer is kept and only the shared audio capture follows. No device is opened here.
fn test_switch(r: &mut Report, ts: &[u8]) {
    use crate::recorder::Recorder;
    let dir = util::data_dir().join("capmode-test").join("switch");
    let _ = std::fs::create_dir_all(&dir);
    let cfg = |desk: bool, mic: bool| CaptureCfg {
        ffmpeg: None,
        monitor: 0,
        width: 320,
        height: 180,
        scaled: false,
        fps: 30,
        hz: 60,
        quality: "high".into(),
        bitrate: 0,
        encoder: "x264".into(),
        method: "wgc".into(),
        cursor: true,
        desktop_audio: desk,
        mic,
        max_secs: 35.0,
        buffer: "ram".into(),
        buffer_ram_secs: 30.0,
        buffer_dir: dir.clone(),
        window: 0x10,
    };
    let rec = Recorder::new();
    let starts0 = rec.test_no_devices();
    rec.start(vec![cfg(false, false)]);
    let id0 = rec.stats().captures.first().map(|c| c.id);
    rec.test_feed(ts, util::now_secs() - 0.5, &tone(3000.0, 4.0));
    let s1 = rec.test_no_devices();
    rec.start(vec![cfg(true, false)]); // desktop sound switched on
    let s2 = rec.test_no_devices();
    let kept = rec.snapshot(30.0, None).is_ok_and(|v| v.0.duration > 3.0);
    r.check("switch: desktop sound on keeps the picture's capture and its buffer", rec.stats().captures.first().map(|c| c.id) == id0 && kept, &format!("{:?} -> {:?}", id0, rec.stats().captures.first().map(|c| c.id)));
    r.check("switch: the shared audio capture is restarted for it", s1 == starts0 + 1 && s2 == s1 + 1 && rec.current_cfgs() == vec![cfg(true, false)], &format!("starts {starts0} {s1} {s2}"));
    rec.start(vec![cfg(true, false)]);
    r.check("switch: the same switches again restart nothing", rec.test_no_devices() == s2, "");
    rec.start(vec![cfg(false, false)]); // and off again: the desktop ring is dropped
    let desk = rec.snapshot(30.0, None).ok().and_then(|v| v.1);
    r.check("switch: desktop sound off drops the desktop ring, the picture stays", rec.stats().captures.first().map(|c| c.id) == id0 && desk.is_none(), "");
    let other = CaptureCfg { width: 640, height: 360, ..cfg(false, false) };
    rec.start(vec![other]);
    r.check("switch: a change of the picture still restarts the capture", rec.stats().captures.first().map(|c| c.id) != id0, "");
    rec.stop();
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
