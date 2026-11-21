//! A plugin's live stream that stays up while its picture's capture restarts (0.1.55).
//!
//! Before, a stream was one ffmpeg: capture, encoder and the connection to the server in one
//! process (`plugins::proc_start`). Desktop Duplication loses the screen whenever a game enters or
//! leaves exclusive fullscreen, ffmpeg's `ddagrab` ends on that ("Generic error in an external
//! library"), and the whole stream went down with it: every alt-tab into the game ended the stream.
//!
//! Here a stream is two kinds of process with Glint between them:
//!
//!   - the SENDER holds the connection to the server. It encodes nothing but the sound: it reads
//!     the finished video from Glint over a local socket and copies it out. Nothing a screen does
//!     can end it.
//!   - the FEEDER captures and encodes the picture and writes it to Glint (MPEG-TS on its stdout).
//!     When it ends, Glint starts another one and keeps handing its output to the same sender:
//!     viewers see the picture stand still for a second, the stream goes on.
//!
//! The two are lined up by the clock, not by counting frames: the feeder stamps every frame with
//! the time since the stream began ("{t0}" in its arguments), so a new feeder carries on where the
//! clock is, and sound and picture stay in step over any number of restarts.
//!
//! The feeder is also restarted on purpose when what it should show changes ("{source}" in its
//! arguments): the game's window appears, is minimized, goes exclusive fullscreen.
//!
//! The sound is mixed here too, from captures of its own (so a stream has sound while clipping is
//! paused, and each part has its own volume): an output device's sound, a microphone, the game's
//! own sound, and any other programs' (Discord). See `Audio`.
//!
//! When the sender itself ends (the network, the server), everything is started again after a
//! pause that grows, a few times, and then the stream ends with the reason.
use crate::audio::AudioRing;
use crate::core::Core;
use crate::plugins::Proc;
use crate::util;
use crate::win::{self, WinInfo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the picture is taken from.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Source {
    /// "monitor" (a whole screen), "game" (the game Glint sees, and nothing else), "window" (one window).
    pub kind: String,
    /// "monitor": its index in `win::monitors()`.
    pub monitor: u32,
    /// "window": the window as it was picked, and its program and title to find it again.
    pub hwnd: u64,
    pub exe: String,
    pub title: String,
    pub fps: u32,
    pub cursor: bool,
    /// The size of the picture the feeder's arguments expect.
    pub width: u32,
    pub height: u32,
    /// "nvenc", "amf", "qsv" or "x264": the form the frames have to arrive in.
    pub encoder: String,
    /// The frames go into a filter graph of the plugin's own (a scene): plain BGRA in memory, any size.
    pub raw: bool,
}

/// One part of the sound. `gain`: 1 = as it is, 0 = silent, up to 2.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Tap {
    pub on: bool,
    pub gain: f32,
    /// A device's name; "" = the Windows default. Desktop and microphone only.
    pub device: String,
    /// A program's file name ("Discord.exe"). `apps` only.
    pub exe: String,
}

/// What the stream's sound is mixed from.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Audio {
    /// Everything an output device plays (which includes the game and every other program).
    pub desktop: Tap,
    pub mic: Tap,
    /// Only the game Glint sees.
    pub game: Tap,
    /// Only these programs.
    pub apps: Vec<Tap>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Spec {
    /// The sender's ffmpeg arguments. "{video}" is where it reads the video from; the sound is
    /// s16le stereo at 48 kHz on its stdin.
    pub send: Vec<String>,
    /// The feeder's. "{source}" is the capture, as a filter chain that ends in frames the encoder
    /// named in `source.encoder` takes; "{t0}" the stream's start in microseconds (for `setpts`).
    pub feed: Vec<String>,
    pub source: Source,
    pub audio: Audio,
}

/// The stream as it runs: what a plugin may change while live, and what it is told.
pub struct Ctl {
    stop: AtomicBool,
    spec: Mutex<Spec>,
    audio: Mutex<Audio>,
    /// ("starting" | "live" | "recovering" | "reconnecting", why).
    state: Mutex<(String, String)>,
    /// How often the picture's capture was started again, and the connection.
    restarts: AtomicU32,
    reconnects: AtomicU32,
    /// What the picture shows now: "monitor", "window" or "card" (nothing to show).
    showing: Mutex<String>,
    /// The server has taken the stream at least once (the sender reported progress).
    was_live: AtomicBool,
}

impl Ctl {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
    pub fn json(&self) -> Value {
        let (state, note) = self.state.lock().unwrap().clone();
        json!({ "state": state, "note": note, "restarts": self.restarts.load(Ordering::Relaxed), "reconnects": self.reconnects.load(Ordering::Relaxed), "showing": *self.showing.lock().unwrap() })
    }
    /// A plugin's change while live: `audio` takes effect at once, `source` and `feed` with a new feeder.
    pub fn update(&self, v: &Value) {
        if let Ok(a) = serde_json::from_value::<Audio>(v["audio"].clone()) {
            if v["audio"].is_object() {
                *self.audio.lock().unwrap() = a;
            }
        }
        let mut spec = self.spec.lock().unwrap();
        if v["source"].is_object() {
            if let Ok(s) = serde_json::from_value::<Source>(v["source"].clone()) {
                spec.source = s;
            }
        }
        if let Some(feed) = v["feed"].as_array().filter(|a| !a.is_empty()) {
            spec.feed = feed.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        }
    }
    /// Sleep up to `ms`. False when the stream was stopped meanwhile.
    fn sleep(&self, ms: u64) -> bool {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            if self.stopped() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !self.stopped()
    }
}

/// How far behind the clock the sound handed to the sender is: time for the captures to deliver it.
const AUDIO_DELAY: f64 = 0.25;
const RATE: u32 = 48_000;
/// Seconds each capture's ring holds: far more than is ever read back.
const RING_SECS: f64 = 10.0;

/// Start `plugin`'s live stream `name`. Its state is a process like any other of the plugin's
/// (`plugin_proc_status`, `plugin_proc_stop`, the "plugin-proc" event), with the stream's own
/// fields beside them (`Ctl::json`).
pub fn start(core: &Arc<Core>, plugin: &str, name: &str, spec: Value, quiet: bool) -> Result<Arc<Proc>, String> {
    let spec: Spec = serde_json::from_value(spec).map_err(|e| format!("The plugin's stream is not valid: {e}"))?;
    if spec.send.is_empty() || spec.feed.is_empty() || spec.send.len() > 400 || spec.feed.len() > 400 {
        return Err("The plugin gave no command to run".into());
    }
    if !spec.send.iter().any(|a| a.contains("{video}")) || !spec.feed.iter().any(|a| a.contains("{source}")) {
        return Err("The plugin's stream is not valid".into());
    }
    let ffmpeg = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found. Install ffmpeg or set its path in Settings.")?;
    let ctl = Arc::new(Ctl {
        stop: AtomicBool::new(false),
        audio: Mutex::new(spec.audio.clone()),
        spec: Mutex::new(spec),
        state: Mutex::new(("starting".into(), String::new())),
        restarts: AtomicU32::new(0),
        reconnects: AtomicU32::new(0),
        showing: Mutex::new(String::new()),
        was_live: AtomicBool::new(false),
    });
    let p = crate::plugins::proc_new(plugin, name, Some(ctl.clone()))?;
    util::log(&format!("plugins: {plugin} started its stream {name}"));
    let (core2, p2) = (core.clone(), p.clone());
    std::thread::Builder::new()
        .name("live".into())
        .spawn(move || {
            let mixer = Mixer::start(&core2, &ctl);
            let why = supervise(&core2, &p2, &ctl, &ffmpeg, &mixer);
            mixer.stop.store(true, Ordering::SeqCst);
            crate::plugins::proc_ended(&core2, &p2, quiet, why);
        })
        .map_err(|e| format!("Could not start the stream: {e}"))?;
    core.emit("plugin-proc", p.json());
    Ok(p)
}

enum End {
    Stopped,
    /// The sender ended: worth another try.
    Lost(String),
    /// Trying again will not help.
    Fatal(String),
}

/// Run the stream until it is stopped (None) or cannot go on (why).
fn supervise(core: &Arc<Core>, p: &Arc<Proc>, ctl: &Arc<Ctl>, ffmpeg: &Path, mixer: &Arc<Mixer>) -> Option<String> {
    const WAITS: [u64; 6] = [1, 2, 4, 8, 15, 15];
    let mut fails = 0usize;
    loop {
        let began = Instant::now();
        match session(core, p, ctl, ffmpeg, mixer) {
            End::Stopped => return None,
            End::Fatal(why) => return Some(why),
            End::Lost(why) => {
                if began.elapsed() > Duration::from_secs(60) {
                    fails = 0; // it had been live: a new run of tries
                }
                // A stream that never got through (a wrong key, a wrong address) is not worth a minute of tries.
                if fails >= WAITS.len() || (fails >= 1 && !ctl.was_live.load(Ordering::SeqCst)) {
                    return Some(why);
                }
                let wait = WAITS[fails];
                fails += 1;
                ctl.reconnects.fetch_add(1, Ordering::Relaxed);
                util::log(&format!("plugins: {}'s stream lost its connection ({why}); connecting again in {wait} s (try {fails} of {})", p.plugin, WAITS.len()));
                set_state(core, p, ctl, "reconnecting", &why);
                if !ctl.sleep(wait * 1000) {
                    return None;
                }
            }
        }
    }
}

fn set_state(core: &Core, p: &Proc, ctl: &Ctl, state: &str, note: &str) {
    {
        let mut s = ctl.state.lock().unwrap();
        if s.0 == state && s.1 == note {
            return;
        }
        *s = (state.into(), note.into());
    }
    core.emit("plugin-proc", p.json());
}

fn end_child(c: &mut Child) {
    let _ = c.kill();
    let _ = c.wait();
}

/// The last thing ffmpeg said that is not a progress line.
fn last_word(log: &Mutex<VecDeque<String>>) -> String {
    log.lock().unwrap().iter().rev().find(|l| !l.starts_with("frame=") && !l.starts_with("size=")).cloned().unwrap_or_default()
}

/// One sender, from its start to its end, with as many feeders as it takes.
fn session(core: &Arc<Core>, p: &Arc<Proc>, ctl: &Arc<Ctl>, ffmpeg: &Path, mixer: &Arc<Mixer>) -> End {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return End::Fatal(format!("Glint could not open a local port for the stream: {e}")),
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let _ = listener.set_nonblocking(true);
    // Sample 0 of the sound and time 0 of the picture are the same moment.
    let base = util::now_secs() - AUDIO_DELAY;
    let t0_us = util::unix_ms() as i64 * 1000 - (AUDIO_DELAY * 1e6) as i64;

    let send: Vec<String> = ctl.spec.lock().unwrap().send.iter().map(|a| a.replace("{video}", &format!("tcp://127.0.0.1:{port}")).replace("{audio_rate}", &RATE.to_string())).collect();
    let mut sender = match util::hidden_cmd(ffmpeg).args(&send).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return End::Fatal(format!("Could not start ffmpeg: {e}")),
    };
    util::track_child(&sender);
    p.pid.store(sender.id(), Ordering::SeqCst);
    p.log.lock().unwrap().clear();
    let secrets: Vec<(String, String)> = send.iter().filter(|a| a.contains("://")).map(|a| (a.clone(), crate::plugins::redact(a))).collect();
    if let Some(err) = sender.stderr.take() {
        let p2 = p.clone();
        std::thread::spawn(move || crate::plugins::read_log(err, &p2.log, &secrets, 40));
    }
    let alive = Arc::new(AtomicBool::new(true));
    if let Some(stdin) = sender.stdin.take() {
        let (mixer, ctl, alive) = (mixer.clone(), ctl.clone(), alive.clone());
        std::thread::spawn(move || mixer.feed(&ctl, stdin, base, &alive));
    }
    let end = feeders(core, p, ctl, ffmpeg, &listener, &mut sender, t0_us);
    alive.store(false, Ordering::SeqCst);
    end_child(&mut sender);
    end
}

/// Hand the sender its video until it ends or the stream is stopped.
fn feeders(core: &Arc<Core>, p: &Arc<Proc>, ctl: &Arc<Ctl>, ffmpeg: &Path, listener: &TcpListener, sender: &mut Child, t0_us: i64) -> End {
    let lost = |p: &Proc| {
        let why = last_word(&p.log);
        End::Lost(if why.is_empty() { "ffmpeg ended".into() } else { why })
    };
    // The sender connects once it has opened its sound.
    let waited = Instant::now();
    let conn: TcpStream = loop {
        if ctl.stopped() {
            return End::Stopped;
        }
        if sender.try_wait().ok().flatten().is_some() {
            return lost(p);
        }
        match listener.accept() {
            Ok((c, _)) => break c,
            Err(_) if waited.elapsed() > Duration::from_secs(20) => return End::Lost("ffmpeg did not ask for the picture".into()),
            Err(_) => std::thread::sleep(Duration::from_millis(30)),
        }
    };
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_nodelay(true);

    const WAITS: [u64; 5] = [300, 600, 1000, 2000, 3000];
    let mut fails = 0usize;
    // Since when no feeder has run well, and the windows whose own capture does not work.
    let mut bad_since: Option<Instant> = None;
    let mut refused: Vec<u64> = Vec::new();
    loop {
        let (feed, source) = {
            let s = ctl.spec.lock().unwrap();
            (s.feed.clone(), s.source.clone())
        };
        let what = resolve(core, &source, &refused, ffmpeg);
        let args = feed_args(&feed, &what, &source, t0_us, ffmpeg);
        let mut feeder = match util::hidden_cmd(ffmpeg).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => return End::Fatal(format!("Could not start ffmpeg: {e}")),
        };
        util::track_child(&feeder);
        *ctl.showing.lock().unwrap() = what.name().into();
        let said: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(err) = feeder.stderr.take() {
            let said = said.clone();
            std::thread::spawn(move || crate::plugins::read_log(err, &said, &[], 12));
        }
        let sent = Arc::new(AtomicU64::new(0));
        let pumped = Arc::new(AtomicBool::new(false));
        if let Some((mut out, mut to)) = feeder.stdout.take().zip(conn.try_clone().ok()) {
            let (sent, pumped) = (sent.clone(), pumped.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 || to.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    sent.fetch_add(n as u64, Ordering::Relaxed);
                }
                pumped.store(true, Ordering::SeqCst);
            });
        } else {
            pumped.store(true, Ordering::SeqCst);
        }
        let started = Instant::now();
        let (mut tick, mut differs, mut switched) = (0u32, 0u32, false);
        let ended_alone = loop {
            std::thread::sleep(Duration::from_millis(100));
            if ctl.stopped() {
                end_child(&mut feeder);
                return End::Stopped;
            }
            if sender.try_wait().ok().flatten().is_some() {
                end_child(&mut feeder);
                return lost(p);
            }
            if feeder.try_wait().ok().flatten().is_some() {
                break true;
            }
            // Live once the sender reports progress: it has the picture and the server takes the stream.
            if sent.load(Ordering::Relaxed) > 0 && p.log.lock().unwrap().iter().any(|l| l.starts_with("frame=")) {
                ctl.was_live.store(true, Ordering::SeqCst);
                set_state(core, p, ctl, "live", "");
                if started.elapsed() > Duration::from_secs(10) {
                    fails = 0;
                    bad_since = None;
                }
            }
            tick += 1;
            if tick % 5 == 0 {
                // What should be shown changed (the game's window came, went, was minimized), or the
                // plugin changed the picture: twice in a row, so a window that flickers is not followed.
                let (feed_now, source_now) = {
                    let s = ctl.spec.lock().unwrap();
                    (s.feed.clone(), s.source.clone())
                };
                if feed_now != feed || source_now != source || resolve(core, &source_now, &refused, ffmpeg) != what {
                    differs += 1;
                    if differs >= 2 {
                        switched = true;
                        break false;
                    }
                } else {
                    differs = 0;
                }
            }
        };
        end_child(&mut feeder);
        // The last of this feeder's output is with the sender before the next one's begins.
        while !pumped.load(Ordering::SeqCst) {
            if ctl.stopped() {
                return End::Stopped;
            }
            if sender.try_wait().ok().flatten().is_some() {
                return lost(p);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if switched {
            util::log(&format!("plugins: {}'s stream: the picture changes (was {}); the stream stays up", p.plugin, what.name()));
            continue;
        }
        debug_assert!(ended_alone);
        let why = last_word(&said);
        let ran = started.elapsed();
        let since = *bad_since.get_or_insert_with(Instant::now);
        fails += 1;
        // A window whose own capture fails twice (exclusive fullscreen, a program that refuses): its monitor instead.
        if let What::Window(h) = what {
            if fails >= 2 && source.kind == "game" && !refused.contains(&h) {
                refused.push(h);
            }
        }
        if since.elapsed() > Duration::from_secs(30) {
            return End::Fatal(if why.is_empty() { "The screen could not be captured".into() } else { format!("The screen could not be captured: {why}") });
        }
        ctl.restarts.fetch_add(1, Ordering::Relaxed);
        let wait = WAITS[(fails - 1).min(WAITS.len() - 1)];
        util::log(&format!("plugins: {}'s stream: the capture of the {} ended after {:.1} s ({}); starting it again in {wait} ms, the stream stays up", p.plugin, what.name(), ran.as_secs_f64(), if why.is_empty() { "no reason given" } else { &why }));
        set_state(core, p, ctl, "recovering", &why);
        let until = Instant::now() + Duration::from_millis(wait);
        while Instant::now() < until {
            if ctl.stopped() {
                return End::Stopped;
            }
            if sender.try_wait().ok().flatten().is_some() {
                return lost(p);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/* ---------- the picture ---------- */

/// What a feeder captures.
#[derive(Clone, Debug, PartialEq)]
enum What {
    /// A whole screen (Desktop Duplication), by its index in `win::monitors()`.
    Monitor(u32),
    /// One window (Graphics Capture).
    Window(u64),
    /// Nothing to show: a black picture, so that the stream goes on.
    Card,
}

impl What {
    fn name(&self) -> &'static str {
        match self {
            What::Monitor(_) => "monitor",
            What::Window(_) => "window",
            What::Card => "card",
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// The window a plugin's "window" source means: the very one while it is there, else the same
/// program's window with the same title, else that program's largest.
fn find_window<'a>(wins: &'a [WinInfo], s: &Source) -> Option<&'a WinInfo> {
    let same_exe = |w: &&WinInfo| s.exe.is_empty() || file_name(&w.exe).eq_ignore_ascii_case(file_name(&s.exe));
    wins.iter().find(|w| w.hwnd == s.hwnd && s.hwnd != 0 && same_exe(w)).or_else(|| {
        if s.exe.is_empty() {
            return None;
        }
        let of: Vec<&WinInfo> = wins.iter().filter(same_exe).collect();
        of.iter().find(|w| !s.title.is_empty() && w.title == s.title).copied().or_else(|| of.into_iter().max_by_key(|w| (!w.minimized, w.width as u64 * w.height as u64)))
    })
}

/// The window of the game Glint sees: the one GAME mode records, else the largest of the game being played.
fn game_window(core: &Core, wins: &[WinInfo]) -> Option<WinInfo> {
    if crate::capmode::game_mode(&core.settings()) {
        if let Some(t) = crate::capmode::watch_view().target {
            if let Some(w) = wins.iter().find(|w| w.hwnd == t.hwnd) {
                return Some(w.clone());
            }
        }
    }
    let pid = core.playing_pid()?;
    let mut pids = win::descendants(pid);
    pids.push(pid);
    wins.iter().filter(|w| pids.contains(&w.pid)).max_by_key(|w| (w.foreground, !w.minimized, w.width as u64 * w.height as u64)).cloned()
}

fn resolve(core: &Core, s: &Source, refused: &[u64], ffmpeg: &Path) -> What {
    match s.kind.as_str() {
        "window" => match find_window(&win::windows(), s) {
            Some(w) if !w.minimized && !w.cloaked => What::Window(w.hwnd),
            _ => What::Card,
        },
        "game" => match game_window(core, &win::windows()) {
            Some(w) if w.minimized || w.cloaked => What::Card,
            // Exclusive fullscreen has no window to capture: its screen, while the game is in front.
            Some(w) if w.fullscreen && w.foreground && win::d3d_fullscreen() => What::Monitor(w.monitor),
            Some(w) if refused.contains(&w.hwnd) || !crate::recorder::has_wgc(ffmpeg) => {
                if w.foreground {
                    What::Monitor(w.monitor)
                } else {
                    What::Card
                }
            }
            Some(w) => What::Window(w.hwnd),
            None => What::Card,
        },
        _ => What::Monitor(s.monitor),
    }
}

/// "{source}": the capture as a filter chain whose frames the encoder takes (or plain BGRA for a scene).
fn chain(what: &What, s: &Source, ffmpeg: &Path) -> String {
    let (w, h) = (s.width.max(2), s.height.max(2));
    let fps = s.fps.clamp(1, 240);
    let enc = s.encoder.as_str();
    match what {
        What::Monitor(i) => {
            let mut c = format!("ddagrab=output_idx={i}:framerate={fps}:draw_mouse={}", s.cursor as u8);
            let scaled = win::monitors().get(*i as usize).is_some_and(|m| (m.width, m.height) != (w, h));
            if s.raw {
                c += ",hwdownload,format=bgra";
            } else if scaled {
                // Another shape than the stream's (the game's own monitor): whole, with black beside it.
                c += &format!(",hwdownload,format=bgra,scale={w}:{h}:flags=bilinear:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2");
                if enc != "x264" {
                    c += ",format=nv12";
                }
            } else if enc == "x264" {
                c += ",hwdownload,format=bgra";
            } else if enc == "qsv" {
                c += ",hwmap=derive_device=qsv,format=qsv";
            }
            c
        }
        What::Window(hwnd) => {
            let mut c = format!("gfxcapture=hwnd={hwnd}:max_framerate={fps}:capture_cursor={}", s.cursor as u8);
            if crate::recorder::gfx_caps(ffmpeg).1 {
                c += ":display_border=0";
            }
            c += &format!(":width={w}:height={h}:resize_mode=scale_aspect:scale_mode=bicubic");
            if s.raw || enc == "x264" {
                c += ",hwdownload,format=bgra";
            } else if enc == "qsv" {
                c += ",hwmap=derive_device=qsv,format=qsv";
            }
            c
        }
        // `realtime`: a made-up picture comes as fast as it is asked for, not at its frame rate.
        What::Card => format!("color=c=black:s={w}x{h}:r={fps},realtime,format={}", if s.raw { "bgra" } else if enc == "x264" { "yuv420p" } else { "nv12" }),
    }
}

fn feed_args(feed: &[String], what: &What, s: &Source, t0_us: i64, ffmpeg: &Path) -> Vec<String> {
    let c = chain(what, s, ffmpeg);
    let args: Vec<String> = feed.iter().map(|a| a.replace("{source}", &c).replace("{t0}", &t0_us.to_string())).collect();
    match what {
        // The monitor named so that ffmpeg cannot take another one, on the graphics card it is on (recorder.rs).
        What::Monitor(i) => crate::recorder::retarget(args, *i, win::monitor_source(*i).as_ref()),
        _ => args,
    }
}

/// The windows a stream can show, for a plugin's picker: [{ hwnd, title, exe (file name), minimized }].
pub fn windows_json() -> Vec<Value> {
    win::windows()
        .into_iter()
        .filter(|w| !w.cloaked && !w.title.trim().is_empty() && !w.exe.is_empty())
        .map(|w| json!({ "hwnd": w.hwnd, "title": w.title, "exe": file_name(&w.exe), "minimized": w.minimized, "excluded": w.excluded }))
        .collect()
}

/* ---------- the sound ---------- */

/// One program's capture.
struct ProcTap {
    /// "game", or a program's file name in lower case.
    key: String,
    pid: u32,
    ring: Arc<Mutex<AudioRing>>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

struct Mixer {
    desk: Arc<Mutex<AudioRing>>,
    mic: Arc<Mutex<AudioRing>>,
    procs: Mutex<Vec<ProcTap>>,
    stop: Arc<AtomicBool>,
}

impl Mixer {
    fn start(core: &Arc<Core>, ctl: &Arc<Ctl>) -> Arc<Mixer> {
        let m = Arc::new(Mixer { desk: Arc::new(Mutex::new(AudioRing::new())), mic: Arc::new(Mutex::new(AudioRing::new())), procs: Mutex::new(Vec::new()), stop: Arc::new(AtomicBool::new(false)) });
        let (m2, ctl2) = (m.clone(), ctl.clone());
        let _ = std::thread::Builder::new().name("live-audio".into()).spawn(move || m2.devices(&ctl2));
        let (m2, ctl2, core2) = (m.clone(), ctl.clone(), core.clone());
        let _ = std::thread::Builder::new().name("live-audio-apps".into()).spawn(move || m2.programs(&core2, &ctl2));
        m
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// The output device's sound and the microphone, as chosen; opened again when a device fails
    /// or changes, or the choice does. The streams live on this thread (cpal's are tied to one).
    fn devices(&self, ctl: &Ctl) {
        let mut logged = (String::new(), String::new());
        while !self.stopped() {
            let a = ctl.audio.lock().unwrap().clone();
            let key = |a: &Audio| (a.desktop.on, a.desktop.device.clone(), a.mic.on, a.mic.device.clone());
            let host = cpal::default_host();
            let open = |host: &cpal::Host| (if a.desktop.on { crate::audio::open_device(host, true, &a.desktop.device) } else { None }, if a.mic.on { crate::audio::open_device(host, false, &a.mic.device) } else { None });
            let (out_dev, in_dev) = open(&host);
            let names = (crate::audio::device_name(&out_dev), crate::audio::device_name(&in_dev));
            if names != logged {
                util::log(&format!("live audio: desktop \"{}\", microphone \"{}\"", names.0, names.1));
                logged = names.clone();
            }
            let failed = Arc::new(AtomicBool::new(false));
            let _s1 = out_dev.as_ref().and_then(|d| crate::audio::build_tap(d, true, self.desk.clone(), RING_SECS, failed.clone()));
            let _s2 = in_dev.as_ref().and_then(|d| crate::audio::build_tap(d, false, self.mic.clone(), RING_SECS, failed.clone()));
            let mut ticks = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                if self.stopped() {
                    return;
                }
                if failed.load(Ordering::Relaxed) || key(&ctl.audio.lock().unwrap()) != key(&a) {
                    break;
                }
                ticks += 1;
                if ticks % 16 == 0 {
                    // The Windows default changed, or a chosen device that was missing came back.
                    let (o, i) = open(&cpal::default_host());
                    if (crate::audio::device_name(&o), crate::audio::device_name(&i)) != names {
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// The game's and the chosen programs' own sound: one capture per running program, started
    /// when the program appears and ended when it goes or is no longer wanted.
    fn programs(&self, core: &Core, ctl: &Ctl) {
        // Programs whose sound could not be captured: left alone for a while.
        let mut refused: Vec<(String, u32, Instant)> = Vec::new();
        while !self.stopped() {
            let a = ctl.audio.lock().unwrap().clone();
            let mut want: Vec<(String, u32)> = Vec::new();
            if a.game.on {
                if let Some(pid) = core.playing_pid() {
                    want.push(("game".into(), pid));
                }
            }
            for app in a.apps.iter().filter(|t| t.on && !t.exe.is_empty()) {
                for pid in win::root_pids(file_name(&app.exe)) {
                    want.push((file_name(&app.exe).to_lowercase(), pid));
                }
            }
            refused.retain(|r| r.2.elapsed() < Duration::from_secs(30));
            let mut procs = self.procs.lock().unwrap();
            procs.retain(|t| {
                let keep = want.contains(&(t.key.clone(), t.pid)) && !t.done.load(Ordering::SeqCst);
                if !keep {
                    t.stop.store(true, Ordering::SeqCst);
                    if t.done.load(Ordering::SeqCst) && win::process_alive(t.pid) {
                        refused.push((t.key.clone(), t.pid, Instant::now()));
                    }
                }
                keep
            });
            for (key, pid) in want {
                if procs.iter().any(|t| t.key == key && t.pid == pid) || refused.iter().any(|r| r.0 == key && r.1 == pid) {
                    continue;
                }
                let tap = ProcTap { key: key.clone(), pid, ring: Arc::new(Mutex::new(AudioRing::new())), stop: Arc::new(AtomicBool::new(false)), done: Arc::new(AtomicBool::new(false)) };
                tap.ring.lock().unwrap().configure(RATE, RING_SECS);
                let (ring, stop, done) = (tap.ring.clone(), tap.stop.clone(), tap.done.clone());
                let started = std::thread::Builder::new().name("live-audio-app".into()).spawn(move || {
                    if let Err(e) = crate::procaudio::capture_tap(pid, &ring, &stop) {
                        util::log(&format!("live audio: the sound of {key} (process {pid}) could not be captured: {e}"));
                    }
                    done.store(true, Ordering::SeqCst);
                });
                if started.is_ok() {
                    util::log(&format!("live audio: capturing {} (process {pid})", tap.key));
                    procs.push(tap);
                }
            }
            drop(procs);
            for _ in 0..10 {
                if self.stopped() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        for t in self.procs.lock().unwrap().drain(..) {
            t.stop.store(true, Ordering::SeqCst);
        }
    }

    /// The mix of the wall-clock range `from..to` as `frames` stereo frames at 48 kHz.
    fn mix(&self, a: &Audio, from: f64, to: f64, frames: usize) -> Vec<f32> {
        let mut acc = vec![0f32; frames * 2];
        let gain = |t: &Tap| if t.on && t.gain.is_finite() { t.gain.clamp(0.0, 2.0) } else { 0.0 };
        add(&self.desk, gain(&a.desktop), from, to, &mut acc);
        add(&self.mic, gain(&a.mic), from, to, &mut acc);
        for t in self.procs.lock().unwrap().iter() {
            let g = if t.key == "game" { gain(&a.game) } else { a.apps.iter().find(|x| file_name(&x.exe).eq_ignore_ascii_case(&t.key)).map(gain).unwrap_or(0.0) };
            add(&t.ring, g, from, to, &mut acc);
        }
        acc
    }

    /// The mix into the sender's stdin, in step with the clock, until the sender is gone.
    /// Sample 0 is the sound of the moment `base`.
    fn feed(&self, ctl: &Ctl, mut stdin: ChildStdin, base: f64, alive: &AtomicBool) {
        let frame = |t: f64| (t * RATE as f64).round() as i64;
        let mut from = base;
        while alive.load(Ordering::SeqCst) && !self.stopped() {
            std::thread::sleep(Duration::from_millis(20));
            let to = util::now_secs() - AUDIO_DELAY;
            let frames = (frame(to) - frame(from)).max(0) as usize;
            if frames == 0 {
                continue;
            }
            let a = ctl.audio.lock().unwrap().clone();
            let mixed = self.mix(&a, from, to, frames);
            let bytes: Vec<u8> = mixed.iter().flat_map(|s| (s.clamp(-32768.0, 32767.0) as i16).to_le_bytes()).collect();
            if stdin.write_all(&bytes).is_err() {
                break; // the sender is gone
            }
            from = to;
        }
    }
}

/// Add `ring`'s sound of `from..to`, times `gain`, to `acc` (stereo, as many frames as it holds).
/// A ring at another sample rate than the mix's is stretched to fit.
fn add(ring: &Mutex<AudioRing>, gain: f32, from: f64, to: f64, acc: &mut [f32]) {
    if gain <= 0.0 {
        return;
    }
    let Some(plan) = ring.lock().unwrap().plan(&[(from, to)]) else { return };
    let pcm = plan.read();
    let (n, frames) = (pcm.data.len() / 2, acc.len() / 2);
    if n == 0 || frames == 0 {
        return;
    }
    if n == frames {
        for (o, s) in acc.iter_mut().zip(pcm.data.iter()) {
            *o += *s as f32 * gain;
        }
        return;
    }
    let step = n as f32 / frames as f32;
    for i in 0..frames {
        let x = i as f32 * step;
        let i0 = (x as usize).min(n - 1);
        let i1 = (i0 + 1).min(n - 1);
        let f = x - i0 as f32;
        for ch in 0..2 {
            let (s0, s1) = (pcm.data[i0 * 2 + ch] as f32, pcm.data[i1 * 2 + ch] as f32);
            acc[i * 2 + ch] += (s0 + (s1 - s0) * f) * gain;
        }
    }
}

/// `glint.exe --live-test <report.txt>`: the parts of a live stream that need no screen and no
/// server: what is shown when, the capture chains, the mix. Opens no device and starts no ffmpeg.
pub fn self_test(report_path: &str) {
    let path = PathBuf::from(report_path);
    let _ = std::fs::remove_file(&path);
    let (mut passed, mut failed) = (0u32, 0u32);
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!(" ({detail})") });
        }
    };
    let ffmpeg = PathBuf::from("ffmpeg.exe");
    let src = |kind: &str, enc: &str, raw: bool| Source { kind: kind.into(), monitor: 0, fps: 60, cursor: true, width: 1280, height: 720, encoder: enc.into(), raw, ..Default::default() };

    let card = chain(&What::Card, &src("game", "nvenc", false), &ffmpeg);
    check("card: a black picture at the stream's size, paced by the clock", card == "color=c=black:s=1280x720:r=60,realtime,format=nv12", card.clone());
    let card = chain(&What::Card, &src("game", "x264", true), &ffmpeg);
    check("card: plain frames for a scene", card.ends_with("format=bgra"), card.clone());
    let mon = chain(&What::Monitor(7), &src("monitor", "x264", false), &ffmpeg);
    check("monitor: Desktop Duplication, downloaded for x264", mon.starts_with("ddagrab=output_idx=7:framerate=60:draw_mouse=1") && mon.contains("hwdownload"), mon.clone());
    let args = feed_args(&["-filter_complex".into(), "{source},setpts=(RTCTIME-{t0})/(TB*1000000)[v]".into()], &What::Card, &src("game", "nvenc", false), 1_234_567, &ffmpeg);
    check("feeder: the capture and the stream's start are filled in", args[1] == "color=c=black:s=1280x720:r=60,realtime,format=nv12,setpts=(RTCTIME-1234567)/(TB*1000000)[v]", args[1].clone());

    let w = |hwnd: u64, exe: &str, title: &str, minimized: bool, area: u32| WinInfo { hwnd, exe: exe.into(), title: title.into(), minimized, width: area, height: 1, ..Default::default() };
    let wins = vec![w(1, r"C:\a\Notes.exe", "One", false, 10), w(2, r"C:\a\Notes.exe", "Two", false, 99), w(3, r"C:\b\Other.exe", "Two", false, 500)];
    let pick = |hwnd: u64, exe: &str, title: &str| find_window(&wins, &Source { hwnd, exe: exe.into(), title: title.into(), ..Default::default() }).map(|w| w.hwnd);
    check("window: the very window while it is there", pick(1, "notes.exe", "x") == Some(1), String::new());
    check("window: a handle now used by another program is not taken", pick(3, "Notes.exe", "Two") == Some(2), String::new());
    check("window: else the same program's window of that title, else its largest", pick(9, "Notes.exe", "One") == Some(1) && pick(9, "Notes.exe", "") == Some(2), String::new());
    check("window: a program that is not running has none", pick(9, "Gone.exe", "One").is_none(), String::new());

    // The mix: two rings of the same second, one at another sample rate.
    let ring = |rate: u32, value: i16| {
        let r = Arc::new(Mutex::new(AudioRing::new()));
        r.lock().unwrap().configure(rate, RING_SECS);
        r.lock().unwrap().push(&vec![value; rate as usize * 2], 100.0);
        r
    };
    let (a, b) = (ring(48_000, 1000), ring(44_100, 500));
    let mut acc = vec![0f32; 960 * 2];
    add(&a, 0.5, 99.5, 99.52, &mut acc);
    add(&b, 2.0, 99.5, 99.52, &mut acc);
    check("mix: each part at its own volume, any sample rate", acc.iter().all(|v| (*v - 1500.0).abs() < 1.0), format!("{:?}", &acc[..4]));
    add(&a, 0.0, 99.5, 99.52, &mut acc);
    add(&Arc::new(Mutex::new(AudioRing::new())), 1.0, 99.5, 99.52, &mut acc);
    check("mix: a part at zero, or with nothing captured, adds nothing", acc.iter().all(|v| (*v - 1500.0).abs() < 1.0), String::new());

    let ctl = Ctl { stop: AtomicBool::new(false), spec: Mutex::new(Spec::default()), audio: Mutex::new(Audio::default()), state: Mutex::new(Default::default()), restarts: AtomicU32::new(0), reconnects: AtomicU32::new(0), showing: Mutex::new(String::new()), was_live: AtomicBool::new(false) };
    ctl.update(&json!({ "audio": { "mic": { "on": true, "gain": 0.4 } }, "source": { "kind": "window", "hwnd": 5 } }));
    check("update: the sound and the picture's source change while live", ctl.audio.lock().unwrap().mic.gain == 0.4 && ctl.spec.lock().unwrap().source.hwnd == 5, String::new());
    ctl.update(&json!({ "audio": { "mic": { "on": false } } }));
    check("update: what is not named stays", ctl.spec.lock().unwrap().source.kind == "window" && !ctl.audio.lock().unwrap().mic.on, String::new());

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{passed} passed, {failed} failed");
    }
}
